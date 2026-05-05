//! End-to-end roundtrip + spot checks via the public crate API.
//!
//! Lives outside `src/` so it exercises the shipped re-exports and
//! catches accidental visibility regressions.

use oxideav_hdr::{encode_hdr, parse_hdr, HdrImage, HdrPixelFormat};

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
    let mut src = gradient(16, 8);
    src.header.exposure = Some(1.5);
    src.header.gamma = Some(2.4);
    src.header.software = Some("oxideav-hdr round 1 selftest".to_owned());
    src.header
        .other
        .push(("PRIMARIES".into(), "0.64 0.33 0.30 0.60".into()));
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
    assert!(head.contains("PRIMARIES=0.64 0.33 0.30 0.60"));
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.exposure, Some(1.5));
    assert_eq!(back.header.gamma, Some(2.4));
    assert_eq!(
        back.header.software.as_deref(),
        Some("oxideav-hdr round 1 selftest"),
    );
    assert!(back
        .header
        .other
        .iter()
        .any(|(k, v)| k == "PRIMARIES" && v == "0.64 0.33 0.30 0.60"));
}
