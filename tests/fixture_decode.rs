//! On-disk `.hdr` fixture regression anchor.
//!
//! `tests/fixtures/*.hdr` are byte-stable artefacts produced by
//! `examples/gen_fixtures.rs` from deterministic synthetic inputs.
//! They lock the encoder's wire format down so an unintentional drift
//! in the magic line, the `KEY=VALUE` block, the resolution-line axis
//! flags, or either RLE-coded pixel section gets caught by a
//! file-level diff rather than a subtle pixel-comparison regression.
//!
//! For each fixture we (a) decode it via the standalone public API
//! and assert the recovered structural fields, then (b) re-encode the
//! decoded image and assert that the bytes match the on-disk file
//! exactly. The combined chain pins both the decoder's parse logic
//! and the encoder's emit logic against a single committed reference.
//!
//! Regenerate after any *intentional* wire-format change with
//! `cargo run --example gen_fixtures`, then commit the updated bytes
//! together with the change.

use std::path::PathBuf;

use oxideav_hdr::{
    encode_hdr, encode_hdr_with_options, encode_hdr_with_rle, parse_hdr, parse_hdr_with_options,
    AxisSign, FallbackMode, HdrFormat, HdrPixelFormat, LineEnding, RleMode,
};

fn fixture_path(name: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR is set by cargo for every `cargo test`
    // invocation — the fixtures live at a stable relative path
    // regardless of where the test binary itself ends up.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    [manifest_dir, "tests", "fixtures", name].iter().collect()
}

fn read_fixture(name: &str) -> Vec<u8> {
    let p = fixture_path(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", p.display()))
}

#[test]
fn gradient_32x16_newrle_decode_and_reencode() {
    // -----------------------------------------------------------------
    // Fixture 1: `gradient_32x16_newrle.hdr`
    //
    // Default encoder settings:
    //   - bare `\n` line endings,
    //   - canonical `-Y H +X W` axis order,
    //   - new-RLE pixel section,
    //   - FORMAT=32-bit_rle_rgbe + no other typed header records.
    // -----------------------------------------------------------------
    let bytes = read_fixture("gradient_32x16_newrle.hdr");
    let img = parse_hdr(&bytes).expect("parse gradient new-RLE fixture");

    // Resolution survives the round trip.
    assert_eq!(img.width, 32);
    assert_eq!(img.height, 16);
    assert_eq!(img.pixel_format, HdrPixelFormat::Rgb96f);
    assert_eq!(img.pixels.len(), 32 * 16 * 3);

    // Default axis flags (Y-first, decreasing Y, increasing X).
    assert!(!img.header.x_first);
    assert_eq!(img.header.y_sign, AxisSign::Decreasing);
    assert_eq!(img.header.x_sign, AxisSign::Increasing);

    // FORMAT typed slot is populated; no other records.
    assert!(matches!(img.header.format, HdrFormat::Rgbe));
    assert!(img.header.exposure.is_none());
    assert!(img.header.gamma.is_none());
    assert!(img.header.software.is_none());
    assert!(img.header.view.is_none());
    assert!(img.header.colorcorr.is_none());
    assert!(img.header.primaries.is_none());
    assert!(img.header.other.is_empty());

    // First and last pixel sit at the expected magnitude extremes of
    // the documented `1e-3 * 10^(6*(u+v)*0.5)` gradient — guards
    // against an off-by-one in the decoder's row / column walk that a
    // structural assert alone would miss.
    let r0 = img.pixels[0];
    let last = img.pixels.len() - 3;
    let r_last = img.pixels[last];
    assert!(r0 > 0.0 && r0 < 0.01, "top-left R = {r0}");
    assert!(
        r_last > 100.0 && r_last < 10_000.0,
        "bottom-right R = {r_last}",
    );

    // The decoder + encoder are byte-stable against the committed
    // fixture — re-emit and compare.
    let reencoded = encode_hdr(&img).expect("re-encode gradient new-RLE");
    assert_eq!(
        reencoded, bytes,
        "re-encoded bytes drifted from gradient_32x16_newrle.hdr",
    );
}

#[test]
fn solid_16x8_oldrle_decode_and_reencode() {
    // -----------------------------------------------------------------
    // Fixture 2: `solid_16x8_oldrle.hdr`
    //
    // Old-RLE pixel section + every typed header slot populated
    // (EXPOSURE / GAMMA / SOFTWARE / VIEW / COLORCORR / PRIMARIES).
    // -----------------------------------------------------------------
    let bytes = read_fixture("solid_16x8_oldrle.hdr");
    let img = parse_hdr(&bytes).expect("parse solid old-RLE fixture");

    assert_eq!(img.width, 16);
    assert_eq!(img.height, 8);
    assert_eq!(img.pixels.len(), 16 * 8 * 3);

    // Every typed header slot survives the wire round trip with its
    // documented value.
    assert_eq!(img.header.exposure, Some(1.25));
    assert_eq!(img.header.gamma, Some(2.2));
    assert_eq!(
        img.header.software.as_deref(),
        Some("oxideav-hdr fixture v1"),
    );
    assert_eq!(
        img.header.view.as_deref(),
        Some("rvu -vp 0 0 10 -vd 0 0 -1"),
    );
    let cc = img.header.colorcorr.expect("COLORCORR missing");
    assert!((cc[0] - 1.10).abs() < 1e-5);
    assert!((cc[1] - 1.00).abs() < 1e-5);
    assert!((cc[2] - 0.95).abs() < 1e-5);
    let p = img.header.primaries.expect("PRIMARIES missing");
    assert!((p.red.0 - 0.640).abs() < 1e-4);
    assert!((p.white.0 - 0.3127).abs() < 1e-4);

    // Solid colour — every pixel decodes to (0.5, 0.25, 0.125) within
    // shared-exponent quantisation noise.
    for px in img.pixels.chunks_exact(3) {
        assert!((px[0] - 0.500).abs() < 0.01, "R drift: {}", px[0]);
        assert!((px[1] - 0.250).abs() < 0.01, "G drift: {}", px[1]);
        assert!((px[2] - 0.125).abs() < 0.01, "B drift: {}", px[2]);
    }

    // Re-emit with the same RLE mode and verify byte-identity.
    let reencoded = encode_hdr_with_rle(&img, RleMode::Old).expect("re-encode solid old-RLE");
    assert_eq!(
        reencoded, bytes,
        "re-encoded bytes drifted from solid_16x8_oldrle.hdr",
    );
}

#[test]
fn gradient_32x16_crlf_plusy_decode_and_reencode() {
    // -----------------------------------------------------------------
    // Fixture 3: `gradient_32x16_crlf_plusY.hdr`
    //
    // CRLF text section + non-default axis order (`+Y H +X W`,
    // bottom-up) + PIXASPECT typed slot + a caller-stashed
    // `OXIDEAV=fixture-r192` extra record.
    // -----------------------------------------------------------------
    let bytes = read_fixture("gradient_32x16_crlf_plusY.hdr");
    let img = parse_hdr(&bytes).expect("parse gradient CRLF fixture");

    assert_eq!(img.width, 32);
    assert_eq!(img.height, 16);
    assert_eq!(img.pixels.len(), 32 * 16 * 3);

    // +Y, +X, Y-first.
    assert!(!img.header.x_first);
    assert_eq!(img.header.y_sign, AxisSign::Increasing);
    assert_eq!(img.header.x_sign, AxisSign::Increasing);

    // Typed slots: PIXASPECT populated, untyped OXIDEAV preserved in
    // the `other` list.
    assert_eq!(img.header.pixaspect, Some(1.0));
    assert!(img
        .header
        .other
        .iter()
        .any(|(k, v)| k == "OXIDEAV" && v == "fixture-r192"));

    // The decoder canonicalises to top-down `(y, x)`. The gradient is
    // monotonic in both axes so the canonical buffer always has its
    // brightest pixel in the (decoded) bottom-right corner regardless
    // of how the file was oriented on disk.
    let last = img.pixels.len() - 3;
    assert!(img.pixels[last] > img.pixels[0]);

    // Re-emit with the same options (new-RLE + CRLF, +Y +X axis
    // preserved by the decoded `header`) and verify byte-identity.
    let reencoded = encode_hdr_with_options(&img, RleMode::New, LineEnding::Crlf)
        .expect("re-encode gradient CRLF");
    assert_eq!(
        reencoded, bytes,
        "re-encoded bytes drifted from gradient_32x16_crlf_plusY.hdr",
    );
}

#[test]
fn flat_4x2_uncompressed_decode_and_reencode() {
    // -----------------------------------------------------------------
    // Fixture 4: `flat_4x2_uncompressed.hdr`
    //
    // Round 196: exercise the spec's third scanline flavour
    // ("Uncompressed — each scanline is M pixels × 4 bytes"). Width 4
    // is below the new-RLE marker's `8..=32767` addressable range, so
    // the on-disk scanline is a flat `4 * width` byte array of RGBE
    // quads. The matching `RleMode::Uncompressed` encoder produces no
    // marker and no sentinels.
    //
    // The fixture is read via `parse_hdr_with_options(..., Uncompressed)`
    // — the historical `parse_hdr` would fall back to old-RLE and could
    // misinterpret a literal `(1, 1, 1, *)` quad as a run sentinel,
    // per the round 196 read-side spec gap.
    // -----------------------------------------------------------------
    let bytes = read_fixture("flat_4x2_uncompressed.hdr");
    let img = parse_hdr_with_options(&bytes, FallbackMode::Uncompressed)
        .expect("parse flat 4x2 uncompressed fixture");

    assert_eq!(img.width, 4);
    assert_eq!(img.height, 2);
    assert_eq!(img.pixel_format, HdrPixelFormat::Rgb96f);
    assert_eq!(img.pixels.len(), 4 * 2 * 3);

    // Default axis flags (Y-first, decreasing Y, increasing X).
    assert!(!img.header.x_first);
    assert_eq!(img.header.y_sign, AxisSign::Decreasing);
    assert_eq!(img.header.x_sign, AxisSign::Increasing);
    assert!(matches!(img.header.format, HdrFormat::Rgbe));

    // Sanity-check a couple of pixels.  The gen_fixtures construction
    // sets pixel (3,1) to (R, G, B) = (2.0, 1.5, 1.0); after the
    // shared-exponent round-trip the recovered values are within the
    // documented ~1% precision.
    // Pixel (x=3, y=1) lives at row-major offset `(y * W + x) * 3`.
    let off_31 = (4 + 3) * 3;
    let r31 = img.pixels[off_31];
    let g31 = img.pixels[off_31 + 1];
    let b31 = img.pixels[off_31 + 2];
    assert!(
        (r31 - 2.0).abs() < 0.05,
        "pixel (3,1) R drifted: {r31} (want ~2.0)"
    );
    assert!(
        (g31 - 1.5).abs() < 0.05,
        "pixel (3,1) G drifted: {g31} (want ~1.5)"
    );
    assert!(
        (b31 - 1.0).abs() < 0.05,
        "pixel (3,1) B drifted: {b31} (want ~1.0)"
    );

    // Pixel section is exactly 4*2*4 = 32 bytes — the on-disk size
    // confirms the encoder did NOT emit a new-RLE marker (4 bytes) or
    // old-RLE sentinels.
    let blank = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .expect("blank-line terminator missing");
    let resline_end = blank
        + 2
        + bytes[blank + 2..]
            .iter()
            .position(|&b| b == b'\n')
            .expect("resolution-line terminator missing");
    let pixel_section_len = bytes.len() - (resline_end + 1);
    assert_eq!(
        pixel_section_len,
        4 * 2 * 4,
        "uncompressed pixel section should be exactly 4*W*H bytes — got {pixel_section_len}",
    );

    // Re-emit with the same options and verify byte-identity.
    let reencoded =
        encode_hdr_with_rle(&img, RleMode::Uncompressed).expect("re-encode flat uncompressed");
    assert_eq!(
        reencoded, bytes,
        "re-encoded bytes drifted from flat_4x2_uncompressed.hdr",
    );
}

#[test]
fn xyze_24x10_newrle_decode_luminance_and_reencode() {
    // -----------------------------------------------------------------
    // Fixture 5: `xyze_24x10_newrle.hdr`
    //
    // Round 383: the first committed `FORMAT=32-bit_rle_xyze` fixture.
    // Besides pinning the XYZE wire round-trip, it anchors the
    // spec-conformance fix to the XYZE luminance semantics: per the
    // staged spec's §"Physical interpretation" the Y primary of an
    // XYZE file is *already* lumens/sr/m², so `luminance_buffer` must
    // return the stored Y verbatim (no 179× efficacy factor) and the
    // scene-referred luminance divides only the EXPOSURE record out.
    // -----------------------------------------------------------------
    let bytes = read_fixture("xyze_24x10_newrle.hdr");
    let img = parse_hdr(&bytes).expect("parse xyze new-RLE fixture");

    assert_eq!(img.width, 24);
    assert_eq!(img.height, 10);
    assert_eq!(img.pixel_format, HdrPixelFormat::Rgb96f);
    assert_eq!(img.pixels.len(), 24 * 10 * 3);

    // Typed slots: XYZE format, EXPOSURE=2, reference-manual PRIMARIES.
    assert!(matches!(img.header.format, HdrFormat::Xyze));
    assert_eq!(img.header.exposure, Some(2.0));
    let p = img.header.primaries.expect("PRIMARIES missing");
    assert!((p.red.0 - 0.640).abs() < 1e-4);
    assert!((p.white.0 - 1.0 / 3.0).abs() < 1e-3);

    // XYZE luminance is the stored Y channel *verbatim* — for every
    // pixel, bit-for-bit, with no 179× factor.
    let lum = img.luminance_buffer();
    assert_eq!(lum.len(), 24 * 10);
    for (i, px) in img.pixels.chunks_exact(3).enumerate() {
        assert_eq!(
            lum[i].to_bits(),
            px[1].to_bits(),
            "pixel {i}: luminance {} != stored Y {}",
            lum[i],
            px[1]
        );
    }

    // Scene-referred luminance divides the EXPOSURE=2 record back out:
    // exactly half the stored Y (2^-1 is an exact f32 scale).
    let scene = img.scene_referred_luminance_buffer();
    for (i, px) in img.pixels.chunks_exact(3).enumerate() {
        assert_eq!(
            scene[i].to_bits(),
            (px[1] * 0.5).to_bits(),
            "pixel {i}: scene luminance {} != Y/2 {}",
            scene[i],
            px[1] * 0.5
        );
    }

    // The construction keeps chromaticity constant (X = 0.9·Y,
    // Z = 1.2·Y): spot-check the ratios within shared-exponent
    // quantisation noise, and the ~4-decade Y ramp's extremes.
    let y0 = img.pixels[1];
    let last = img.pixels.len() - 3;
    let y_last = img.pixels[last + 1];
    assert!(y0 > 0.04 && y0 < 0.06, "top-left Y = {y0}");
    assert!(
        y_last > 100.0 && y_last < 1000.0,
        "bottom-right Y = {y_last}"
    );
    for px in img.pixels.chunks_exact(3) {
        assert!(
            (px[0] / px[1] - 0.9).abs() < 0.02,
            "X/Y = {}",
            px[0] / px[1]
        );
        assert!(
            (px[2] / px[1] - 1.2).abs() < 0.02,
            "Z/Y = {}",
            px[2] / px[1]
        );
    }

    // Byte-stable against the committed fixture.
    let reencoded = encode_hdr(&img).expect("re-encode xyze new-RLE");
    assert_eq!(
        reencoded, bytes,
        "re-encoded bytes drifted from xyze_24x10_newrle.hdr",
    );
}
