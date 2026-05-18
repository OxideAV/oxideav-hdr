//! End-to-end roundtrip + spot checks via the public crate API.
//!
//! Lives outside `src/` so it exercises the shipped re-exports and
//! catches accidental visibility regressions.

use oxideav_hdr::{
    encode_hdr, encode_hdr_with_rle, parse_hdr, AxisSign, HdrImage, HdrPixelFormat, RleMode,
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
fn encoder_canonicalises_x_first_request_back_to_y_first() {
    // The encoder doesn't implement the X-first orderings yet — when
    // the caller asks for one it falls back to the canonical Y-first
    // layout (`-Y H +X W`). Round-tripping a synthetic `x_first = true`
    // flag therefore loses that single header bit but keeps the
    // pixels intact.
    let w = 16_u32;
    let h = 8_u32;
    let mut pixels = vec![0.1_f32; (w * h * 3) as usize];
    pixels[0] = 7.0;
    pixels[1] = 0.0;
    pixels[2] = 0.0;
    let mut src = HdrImage::new_rgb96f(w, h, pixels);
    src.header.x_first = true;
    let bytes = encode_hdr(&src).unwrap();
    let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let res_start = blank + 2;
    let res_end = res_start + bytes[res_start..].iter().position(|&b| b == b'\n').unwrap();
    let resline = std::str::from_utf8(&bytes[res_start..res_end]).unwrap();
    assert!(
        resline.starts_with("-Y ") || resline.starts_with("+Y "),
        "expected Y-first canonicalisation, got: {resline:?}"
    );
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.width, w);
    assert_eq!(back.height, h);
    assert!(
        (back.pixels[0] - 7.0).abs() < 0.1,
        "top-left pixel lost across canonicalisation: {}",
        back.pixels[0]
    );
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
