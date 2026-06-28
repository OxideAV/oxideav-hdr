//! End-to-end roundtrip + spot checks via the public crate API.
//!
//! Lives outside `src/` so it exercises the shipped re-exports and
//! catches accidental visibility regressions.

use oxideav_hdr::{
    encode_hdr, encode_hdr_with_full_options, encode_hdr_with_options, encode_hdr_with_rle,
    parse_hdr, parse_hdr_with_options, AxisSign, FallbackMode, HdrFormat, HdrImage, HdrPixelFormat,
    LineEnding, MagicLine, Orientation, Primaries, RleMode,
};

/// Same gradient construction as the in-crate unit tests, kept here to
/// avoid making the unit-test helper part of the public surface.
fn gradient(w: u32, h: u32) -> HdrImage {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            let mag = 1e-3_f32 * 10.0_f32.powf(6.0 * (u + v) * 0.5);
            pixels.push(mag);
            pixels.push(mag * 0.5);
            pixels.push(mag * 0.25);
        }
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

#[test]
fn public_api_roundtrips_gradient() {
    let src = gradient(48, 24);
    let bytes = encode_hdr(&src).unwrap();
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.width, 48);
    assert_eq!(back.height, 24);
    assert_eq!(back.pixel_format, HdrPixelFormat::Rgb96f);
    // Smoke-check the first and last pixel — bounds of the magnitude
    // range exercise both ends of the shared-exponent encoder.
    let last = back.pixels.len() - 3;
    assert!(back.pixels[0] > 0.0 && back.pixels[0] < 0.01);
    assert!(back.pixels[last] > 100.0 && back.pixels[last] < 10_000.0);
}

#[test]
fn header_records_passthrough() {
    use oxideav_hdr::Primaries;
    let mut src = gradient(16, 8);
    src.header.exposure = Some(1.5);
    src.header.gamma = Some(2.4);
    src.header.software = Some("oxideav-hdr round 1 selftest".to_owned());
    src.header.colorcorr = Some([1.10, 0.95, 0.80]);
    // PRIMARIES is defined by Radiance as eight space-separated floats:
    // `Rx Ry Gx Gy Bx By Wx Wy`. Use sRGB / Rec.709 primaries plus D65
    // — round-trip should preserve the field.
    src.header.primaries = Some(Primaries::SRGB);
    let bytes = encode_hdr(&src).unwrap();
    // Header should appear in the leading bytes. We slice up to the
    // double-newline that ends the KEY=VALUE block and check for our
    // records in that prefix only.
    let blank = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .expect("encoded HDR should have a header terminator");
    let head = std::str::from_utf8(&bytes[..blank]).expect("header is ASCII");
    assert!(head.contains("EXPOSURE="));
    assert!(head.contains("GAMMA="));
    assert!(head.contains("SOFTWARE=oxideav-hdr round 1 selftest"));
    assert!(head.contains("COLORCORR=1.1 0.95 0.8"));
    assert!(head.contains("PRIMARIES=0.64 0.33"));
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.exposure, Some(1.5));
    assert_eq!(back.header.gamma, Some(2.4));
    assert_eq!(
        back.header.software.as_deref(),
        Some("oxideav-hdr round 1 selftest"),
    );
    assert_eq!(back.header.colorcorr, Some([1.10, 0.95, 0.80]));
    let p = back.header.primaries.expect("PRIMARIES round-trip lost");
    assert!((p.red.0 - 0.640).abs() < 1e-4);
    assert!((p.white.0 - 0.3127).abs() < 1e-4);
}

#[test]
fn encoder_honours_increasing_y_flag() {
    // Build an image whose top-left pixel is unique, then ask the
    // encoder to emit `+Y H +X W` (bottom-up). After re-decoding (which
    // canonicalises back to top-down) the unique pixel should still
    // appear in the top-left.
    let w = 16_u32;
    let h = 8_u32;
    let mut pixels = vec![0.1_f32; (w * h * 3) as usize];
    // Mark the canonical top-left pixel with a distinctive bright red.
    pixels[0] = 5.0;
    pixels[1] = 0.0;
    pixels[2] = 0.0;
    let mut src = HdrImage::new_rgb96f(w, h, pixels);
    src.header.y_sign = AxisSign::Increasing; // +Y → bottom-up rows on disk

    let bytes = encode_hdr(&src).unwrap();
    // Find the resolution line — first non-empty line after the blank.
    let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let res_start = blank + 2;
    let res_end = res_start + bytes[res_start..].iter().position(|&b| b == b'\n').unwrap();
    let resline = std::str::from_utf8(&bytes[res_start..res_end]).unwrap();
    assert!(
        resline.starts_with("+Y "),
        "expected +Y H ... but got: {resline:?}"
    );

    // Decode and check the top-left pixel survives.
    let back = parse_hdr(&bytes).unwrap();
    assert!(
        (back.pixels[0] - 5.0).abs() < 0.1,
        "lost top-left marker: {}",
        back.pixels[0]
    );
    // And it really did get flipped on disk: with +Y, the on-disk
    // first scanline is the canonical bottom row, so a re-decode
    // pre-flip would have read 0.1 (the rest of the image) at offset
    // 0. The fact that we see 5.0 means the decoder *did* flip on
    // the way back, which is exactly the round-trip property we want.
}

#[test]
fn encoder_writes_x_first_resolution_line_when_requested() {
    // Round 4: the encoder honours `x_first = true`. Resolution line
    // starts with the X flag, the canonical buffer is transposed on
    // the way out, and the decoder applies the inverse transform so
    // round-trip pixels match.
    let w = 16_u32;
    let h = 8_u32;
    let mut pixels = vec![0.1_f32; (w * h * 3) as usize];
    // Distinct top-left marker.
    pixels[0] = 7.0;
    pixels[1] = 0.0;
    pixels[2] = 0.0;
    // Distinct bottom-right marker as well so a partial transpose can
    // be diagnosed.
    let last = pixels.len() - 3;
    pixels[last] = 0.0;
    pixels[last + 1] = 9.0;
    pixels[last + 2] = 0.0;
    let mut src = HdrImage::new_rgb96f(w, h, pixels);
    src.header.x_first = true;
    let bytes = encode_hdr(&src).unwrap();
    let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let res_start = blank + 2;
    let res_end = res_start + bytes[res_start..].iter().position(|&b| b == b'\n').unwrap();
    let resline = std::str::from_utf8(&bytes[res_start..res_end]).unwrap();
    assert!(
        resline.starts_with("-X ") || resline.starts_with("+X "),
        "expected X-first resolution line, got: {resline:?}"
    );
    // The X-value listed in the resolution line is the original image
    // width, the Y-value is the original image height — regardless of
    // which one comes first on the wire.
    assert!(
        resline.contains(&format!(" {w} ")) && resline.ends_with(&format!(" {h}")),
        "expected '... {w} ... {h}', got: {resline:?}"
    );
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.width, w);
    assert_eq!(back.height, h);
    assert!(back.header.x_first);
    assert!(
        (back.pixels[0] - 7.0).abs() < 0.1,
        "top-left pixel lost across x_first round-trip: {}",
        back.pixels[0]
    );
    let last = back.pixels.len() - 3;
    assert!(
        (back.pixels[last + 1] - 9.0).abs() < 0.1,
        "bottom-right pixel lost across x_first round-trip: {}",
        back.pixels[last + 1]
    );
}

#[test]
fn encoder_round_trips_all_eight_axis_orderings() {
    // Exhaustively verify each (y_sign, x_sign, x_first) combination
    // round-trips losslessly via the public API.
    use oxideav_hdr::AxisSign::{Decreasing, Increasing};
    let w = 16_u32;
    let h = 8_u32;
    let mut pixels = vec![0.0_f32; (w * h * 3) as usize];
    // Encode a per-pixel signature so any reordering bug shows up.
    for y in 0..h as usize {
        for x in 0..w as usize {
            let off = (y * w as usize + x) * 3;
            pixels[off] = (y as f32) * 100.0 + x as f32;
            pixels[off + 1] = (y as f32) + 0.5;
            pixels[off + 2] = (x as f32) * 0.25;
        }
    }
    for &x_first in &[false, true] {
        for &y_sign in &[Decreasing, Increasing] {
            for &x_sign in &[Increasing, Decreasing] {
                let mut img = HdrImage::new_rgb96f(w, h, pixels.clone());
                img.header.x_first = x_first;
                img.header.y_sign = y_sign;
                img.header.x_sign = x_sign;
                let bytes = encode_hdr(&img).unwrap_or_else(|e| {
                    panic!("encode failed: {y_sign:?} {x_sign:?} x_first={x_first} → {e}")
                });
                let back = parse_hdr(&bytes).unwrap();
                assert_eq!(back.width, w);
                assert_eq!(back.height, h);
                assert_eq!(back.header.x_first, x_first);
                assert_eq!(back.header.y_sign, y_sign);
                assert_eq!(back.header.x_sign, x_sign);
                // Pixels should match within shared-exponent precision.
                for i in 0..pixels.len() {
                    let a = pixels[i];
                    let b = back.pixels[i];
                    let err = (a - b).abs();
                    // Bigger samples carry the shared-exponent — allow
                    // ~1% relative error or 1.0 absolute (small samples
                    // sharing a large neighbour's exponent can drift).
                    let pixel_idx = i / 3;
                    let pmax = pixels[pixel_idx * 3..pixel_idx * 3 + 3]
                        .iter()
                        .fold(0.0_f32, |m, v| m.max(v.abs()));
                    assert!(
                        err < pmax / 100.0 || err < 1.0,
                        "axis {y_sign:?} {x_sign:?} x_first={x_first} pixel {i}: {a} vs {b}"
                    );
                }
            }
        }
    }
}

#[test]
fn rle_mode_auto_falls_back_to_old_for_narrow_widths() {
    // Width = 4 is below the new-RLE marker's 8-pixel minimum. With
    // `RleMode::Auto` the encoder should silently pick the old-RLE
    // path; `RleMode::New` would have returned an error.
    let w = 4_u32;
    let h = 6_u32;
    let pixels = vec![0.5_f32; (w * h * 3) as usize];
    let src = HdrImage::new_rgb96f(w, h, pixels);
    assert!(encode_hdr_with_rle(&src, RleMode::New).is_err());
    let bytes = encode_hdr_with_rle(&src, RleMode::Auto).unwrap();
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.width, w);
    assert_eq!(back.height, h);
    for &v in &back.pixels {
        assert!((v - 0.5).abs() < 1e-2, "value drift: {v}");
    }
}

#[test]
fn crlf_line_ending_roundtrips_via_public_api() {
    // Round 5: encoder honours `LineEnding::Crlf` on the magic line,
    // KEY=VALUE records, blank-line terminator and resolution line. The
    // pixel payload that follows is untouched.
    let w = 16_u32;
    let h = 4_u32;
    let mut pixels = vec![0.0_f32; (w * h * 3) as usize];
    for (i, p) in pixels.iter_mut().enumerate() {
        *p = (i as f32 + 1.0) * 0.01;
    }
    let mut src = HdrImage::new_rgb96f(w, h, pixels.clone());
    src.header.software = Some("oxideav-hdr/round5-crlf".to_owned());
    let bytes = encode_hdr_with_options(&src, RleMode::New, LineEnding::Crlf).unwrap();
    assert!(bytes.starts_with(b"#?RADIANCE\r\n"));
    // Blank-line terminator must be `\r\n\r\n` (not bare `\n\n`).
    assert!(bytes.windows(4).any(|w| w == b"\r\n\r\n"));
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.width, w);
    assert_eq!(back.height, h);
    assert_eq!(
        back.header.software.as_deref(),
        Some("oxideav-hdr/round5-crlf")
    );
    // Sample a couple of pixels — the pixel payload is binary, so CRLF
    // shouldn't have touched it.
    for (i, (&a, &b)) in pixels.iter().zip(back.pixels.iter()).enumerate() {
        assert!(
            (a - b).abs() < 0.02 || (a - b).abs() / a.max(1e-9) < 0.05,
            "pixel {i}: {a} vs {b}",
        );
    }
}

#[test]
fn view_record_round_trips_via_public_api() {
    let mut src = HdrImage::new_rgb96f(16, 2, vec![0.5_f32; 16 * 2 * 3]);
    let view = "rpict -vp 1 2 3 -vd 0 0 -1 -vu 0 1 0 -vh 60 -vv 40";
    src.header.view = Some(view.to_owned());
    let bytes = encode_hdr(&src).unwrap();
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.view.as_deref(), Some(view));
}

#[test]
fn apply_exposure_and_colorcorr_chain_after_decode() {
    // Round 5: apply_exposure / apply_colorcorr fold the parsed
    // multiplicative factors into the pixel buffer in place.
    let mut src = HdrImage::new_rgb96f(8, 2, vec![1.0_f32; 8 * 2 * 3]);
    src.header.exposure = Some(0.5);
    src.header.colorcorr = Some([2.0, 1.0, 0.5]);
    let bytes = encode_hdr_with_rle(&src, RleMode::Old).unwrap();
    let mut back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.exposure, Some(0.5));
    assert_eq!(back.header.colorcorr, Some([2.0, 1.0, 0.5]));
    back.apply_exposure();
    back.apply_colorcorr();
    assert!(back.header.exposure.is_none());
    assert!(back.header.colorcorr.is_none());
    // Each pixel should have been multiplied by 0.5 then componentwise
    // by [2, 1, 0.5] → effective [1.0, 0.5, 0.25] starting from
    // [1, 1, 1]. Allow ~1.5% for shared-exponent quantisation.
    for px in back.pixels.chunks_exact(3) {
        assert!((px[0] - 1.0).abs() < 0.02, "R: {}", px[0]);
        assert!((px[1] - 0.5).abs() < 0.02, "G: {}", px[1]);
        assert!((px[2] - 0.25).abs() < 0.02, "B: {}", px[2]);
    }
}

#[test]
fn uncompressed_rle_roundtrips_narrow_image_through_public_api() {
    // Round 196: end-to-end exercise of `RleMode::Uncompressed` +
    // `FallbackMode::Uncompressed`. Width 4 is too narrow for the
    // new-RLE marker (which needs `8 <= W <= 32767`), and we want the
    // decoder to NOT engage the old-RLE sentinel grammar. The on-disk
    // pixel section should be exactly `4 * W * H` bytes.
    let w = 4_u32;
    let h = 3_u32;
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for i in 0..(w * h) as usize {
        let v = (i as f32 + 1.0) * 0.05;
        pixels.push(v);
        pixels.push(v * 0.5);
        pixels.push(v * 0.25);
    }
    let src = HdrImage::new_rgb96f(w, h, pixels.clone());
    let bytes = encode_hdr_with_rle(&src, RleMode::Uncompressed).unwrap();

    // Compute the on-disk pixel section size and confirm it equals
    // 4 * W * H — no marker, no sentinels.
    let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let res_end = blank + 2 + bytes[blank + 2..].iter().position(|&b| b == b'\n').unwrap();
    let payload_len = bytes.len() - (res_end + 1);
    assert_eq!(payload_len, (w * h * 4) as usize);

    let back = parse_hdr_with_options(&bytes, FallbackMode::Uncompressed).unwrap();
    assert_eq!(back.width, w);
    assert_eq!(back.height, h);
    for (i, (a, b)) in pixels.iter().zip(back.pixels.iter()).enumerate() {
        let err = (a - b).abs();
        let rel = err / a.max(1e-30);
        assert!(rel < 0.03, "pixel {i}: src={a} back={b} rel={rel}");
    }
}

#[test]
fn rle_mode_auto_uses_new_for_normal_widths() {
    // Width = 32 is in the new-RLE range — Auto should pick New, so
    // the scanline marker `0x02 0x02` appears immediately after the
    // resolution line.
    let w = 32_u32;
    let h = 4_u32;
    let pixels = vec![0.3_f32; (w * h * 3) as usize];
    let src = HdrImage::new_rgb96f(w, h, pixels);
    let bytes = encode_hdr_with_rle(&src, RleMode::Auto).unwrap();
    // Locate the pixel-section start (first byte after the resolution
    // line's `\n`).
    let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let res_start = blank + 2;
    let res_end = res_start + bytes[res_start..].iter().position(|&b| b == b'\n').unwrap();
    let payload_start = res_end + 1;
    assert_eq!(&bytes[payload_start..payload_start + 2], &[0x02, 0x02]);
}

#[test]
fn scene_referred_recovery_survives_encode_decode_via_public_api() {
    // Build a picture whose stored channels are a known scene-referred
    // radiance scaled by EXPOSURE and COLORCORR the writer "baked in",
    // encode it, decode it, and confirm both the non-mutating
    // `scene_referred_radiance_buffer` and the in-place
    // `recover_scene_referred_radiance` recover the original radiance
    // end-to-end through the public API — the round-trip the round-366
    // recovery subsystem promises.
    let w = 16_u32;
    let h = 8_u32;
    // Pick a constant scene radiance so the shared-exponent quantiser is
    // exact (power-of-two-ish magnitudes recover to high precision).
    let radiance = [0.5_f32, 0.25, 0.125];
    let exposure = 4.0_f32;
    let colorcorr = [2.0_f32, 1.0, 0.5];
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..(w * h) {
        pixels.push(radiance[0] * exposure * colorcorr[0]);
        pixels.push(radiance[1] * exposure * colorcorr[1]);
        pixels.push(radiance[2] * exposure * colorcorr[2]);
    }
    let mut src = HdrImage::new_rgb96f(w, h, pixels);
    src.header.exposure = Some(exposure);
    src.header.colorcorr = Some(colorcorr);

    let bytes = encode_hdr(&src).unwrap();
    let back = parse_hdr(&bytes).unwrap();
    // The decoder folds the records into the typed slots.
    assert_eq!(back.header.exposure, Some(exposure));
    assert_eq!(back.header.colorcorr, Some(colorcorr));

    // Non-mutating recovery: slots survive, buffer holds radiance.
    let recovered = back.scene_referred_radiance_buffer();
    for px in recovered.chunks_exact(3) {
        assert!(
            (px[0] - radiance[0]).abs() < 1e-2,
            "{} vs {}",
            px[0],
            radiance[0]
        );
        assert!(
            (px[1] - radiance[1]).abs() < 1e-2,
            "{} vs {}",
            px[1],
            radiance[1]
        );
        assert!(
            (px[2] - radiance[2]).abs() < 1e-2,
            "{} vs {}",
            px[2],
            radiance[2]
        );
    }
    assert_eq!(back.header.exposure, Some(exposure));
    assert_eq!(back.header.colorcorr, Some(colorcorr));

    // In-place recovery: leaves identical values and clears the slots.
    let mut back2 = parse_hdr(&bytes).unwrap();
    back2.recover_scene_referred_radiance();
    for (a, b) in back2.pixels.iter().zip(recovered.iter()) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }
    assert!(back2.header.exposure.is_none());
    assert!(back2.header.colorcorr.is_none());
    // After recovery + clear, a re-encode no longer carries the records.
    let reencoded = encode_hdr(&back2).unwrap();
    let blank = reencoded.windows(2).position(|w| w == b"\n\n").unwrap();
    let header_text = &reencoded[..blank];
    assert!(
        !header_text.windows(9).any(|w| w == b"EXPOSURE="),
        "EXPOSURE= should be gone after recovery"
    );
    assert!(
        !header_text.windows(10).any(|w| w == b"COLORCORR="),
        "COLORCORR= should be gone after recovery"
    );
}

/// The full on-wire option matrix the `encode` fuzz target relies on,
/// pinned as a deterministic integration test: every [`RleMode`] flavour
/// × both [`LineEnding`]s × both [`MagicLine`] spellings × all eight
/// [`Orientation`]s × both [`HdrFormat`]s, each carrying a header with
/// all seven typed records plus a command line, must survive an
/// `encode_hdr_with_full_options` → `parse_hdr_with_options` round trip
/// with dimensions, `FORMAT`, orientation and every typed record intact.
///
/// This is the ground truth the fuzz target asserts against on arbitrary
/// input; pinning it as a unit test means a header-writer / parser
/// asymmetry surfaces in CI even without a fuzz run.
#[test]
fn full_option_matrix_round_trips_typed_header_and_orientation() {
    let exposure = 0.625_f32; // 160/256 — exactly representable
    let gamma = 1.5_f32;
    let pixaspect = 0.75_f32;
    let colorcorr = [0.5_f32, 0.25_f32, 0.125_f32];
    let primaries = Primaries {
        red: (0.625, 0.328125),
        green: (0.296875, 0.59375),
        blue: (0.15625, 0.0625),
        white: (0.3125, 0.328125),
    };

    let rles = [
        RleMode::New,
        RleMode::Old,
        RleMode::Auto,
        RleMode::Uncompressed,
    ];
    let eols = [LineEnding::Lf, LineEnding::Crlf];
    let magics = [MagicLine::Radiance, MagicLine::Rgbe];
    let orientations = [
        Orientation::Standard,
        Orientation::FlipX,
        Orientation::Rotate180,
        Orientation::FlipY,
        Orientation::Rotate90Cw,
        Orientation::Rotate90CwFlipY,
        Orientation::Rotate90Ccw,
        Orientation::Rotate90CcwFlipY,
    ];
    let formats = [HdrFormat::Rgbe, HdrFormat::Xyze];

    let (w, h) = (12u32, 9u32); // both axes inside new-RLE 8..=32767
    let mut cases = 0usize;

    for &rle in &rles {
        for &eol in &eols {
            for magic in &magics {
                for &orientation in &orientations {
                    for &format in &formats {
                        let mut img = gradient(w, h);
                        img.header.format = format;
                        img.header.set_orientation(orientation);
                        img.header.exposure = Some(exposure);
                        img.header.gamma = Some(gamma);
                        img.header.pixaspect = Some(pixaspect);
                        img.header.colorcorr = Some(colorcorr);
                        img.header.primaries = Some(primaries);
                        img.header.software = Some("oxideav-test 1.0".to_string());
                        img.header.view = Some("rvu -vp 0 0 1 -vd 0 0 -1".to_string());
                        img.header
                            .commands
                            .push("rpict -vf scene.vp scene.oct".to_string());

                        let bytes = encode_hdr_with_full_options(&img, rle, eol, magic.clone())
                            .expect("encode must succeed for in-range dims");

                        let fallback = match rle {
                            RleMode::Uncompressed => FallbackMode::Uncompressed,
                            _ => FallbackMode::OldRle,
                        };
                        let back = parse_hdr_with_options(&bytes, fallback)
                            .expect("encoder output must decode");

                        let tag = format!("{rle:?}/{eol:?}/{magic:?}/{orientation:?}/{format:?}");
                        assert_eq!(back.width, w, "{tag}: width");
                        assert_eq!(back.height, h, "{tag}: height");
                        assert_eq!(back.pixels.len(), (w * h * 3) as usize, "{tag}: buf len");
                        assert_eq!(back.header.format, format, "{tag}: FORMAT");
                        assert_eq!(back.header.orientation(), orientation, "{tag}: orientation");

                        let approx = |a: f32, b: f32| (a - b).abs() < 1e-4;
                        assert!(
                            back.header.exposure.map(|e| approx(e, exposure)) == Some(true),
                            "{tag}: EXPOSURE {:?}",
                            back.header.exposure
                        );
                        assert!(
                            back.header.gamma.map(|g| approx(g, gamma)) == Some(true),
                            "{tag}: GAMMA {:?}",
                            back.header.gamma
                        );
                        assert!(
                            back.header.pixaspect.map(|p| approx(p, pixaspect)) == Some(true),
                            "{tag}: PIXASPECT {:?}",
                            back.header.pixaspect
                        );
                        let cc = back.header.colorcorr.expect("COLORCORR present");
                        assert!(
                            approx(cc[0], colorcorr[0])
                                && approx(cc[1], colorcorr[1])
                                && approx(cc[2], colorcorr[2]),
                            "{tag}: COLORCORR {cc:?}"
                        );
                        let p = back.header.primaries.expect("PRIMARIES present");
                        assert!(approx(p.red.0, primaries.red.0), "{tag}: PRIMARIES red.x");
                        assert!(
                            approx(p.white.1, primaries.white.1),
                            "{tag}: PRIMARIES white.y"
                        );
                        assert_eq!(
                            back.header.software.as_deref(),
                            Some("oxideav-test 1.0"),
                            "{tag}: SOFTWARE"
                        );
                        assert!(
                            back.header
                                .commands
                                .iter()
                                .any(|c| c == "rpict -vf scene.vp scene.oct"),
                            "{tag}: command line {:?}",
                            back.header.commands
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    // 4 RLE × 2 EOL × 2 magic × 8 orientation × 2 format = 256 cases.
    assert_eq!(cases, 256, "expected the full 256-case matrix");
}

/// Build a minimal valid HDR text container (magic + `FORMAT` + blank
/// line + resolution line) for a single `width × 1` old-RLE scanline,
/// appending the caller-supplied raw scanline bytes. Width is kept below
/// 8 so the new-RLE marker can never fire and `parse_hdr`'s default
/// `FallbackMode::OldRle` governs the scanline.
fn old_rle_one_row_file(width: usize, scanline: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"#?RADIANCE\n");
    bytes.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    bytes.extend_from_slice(b"\n");
    bytes.extend_from_slice(format!("-Y 1 +X {width}\n").as_bytes());
    bytes.extend_from_slice(scanline);
    bytes
}

#[test]
fn public_parse_rejects_leading_old_rle_sentinel_first_scanline() {
    // The first scanline of the picture cannot begin with a run-length
    // sentinel `(1, 1, 1, n)` — there is no previous pixel for it to
    // repeat. `parse_hdr` (default `FallbackMode::OldRle`) must surface
    // an error rather than silently decoding a black run. Width 4 keeps
    // the scanline off the new-RLE marker path.
    let scanline = [
        0x01, 0x01, 0x01, 0x03, // illegal leading sentinel
        0x10, 0x20, 0x30, 0x80, // a literal that would follow
    ];
    let file = old_rle_one_row_file(4, &scanline);
    let err = parse_hdr(&file).unwrap_err();
    assert!(
        err.to_string().contains("leading sentinel"),
        "expected leading-sentinel rejection, got: {err}"
    );
}

#[test]
fn public_parse_accepts_old_rle_literal_then_sentinel_first_scanline() {
    // The positive control: a first scanline that opens with a literal
    // (establishing the previous pixel) followed by a sentinel run still
    // decodes through the public API. Four pixels: one literal, then a
    // 3× repeat of it.
    let scanline = [
        0x10, 0x20, 0x30, 0x80, // literal establishes prev
        0x01, 0x01, 0x01, 0x03, // repeat it 3× → 4 pixels total
    ];
    let file = old_rle_one_row_file(4, &scanline);
    let img = parse_hdr(&file).unwrap();
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 1);
    // All four decoded pixels share the literal's RGBE, so the decoded
    // float RGB triples are all identical and strictly positive.
    let p0 = [img.pixels[0], img.pixels[1], img.pixels[2]];
    for px in 0..4 {
        let off = px * 3;
        assert_eq!(
            [img.pixels[off], img.pixels[off + 1], img.pixels[off + 2]],
            p0
        );
    }
    assert!(p0[0] > 0.0 && p0[1] > 0.0 && p0[2] > 0.0);
}
