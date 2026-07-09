//! End-to-end coverage for the `GAMMA=` transfer-exponent subsystem
//! driven through the public `encode_hdr` / `parse_hdr` boundary.
//!
//! The staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`, "The
//! `GAMMA=` header variable") documents `GAMMA=g` as a de-facto extension
//! recording that the stored channels have already been gamma-encoded with
//! exponent `g`; a honouring reader recovers linear radiance as
//! `stored^g`, defaulting to `1.0` (linear) when absent, and a
//! fully-specified decode linearises **first**, then divides out
//! `COLORCORR` and `EXPOSURE`. These tests confirm the record survives a
//! real file round-trip and that the `HdrImage` helpers implement that
//! order end-to-end (not merely on an in-memory buffer).

use oxideav_hdr::{encode_hdr, parse_hdr, HdrImage};

/// Build a picture whose stored channels the encoder can represent
/// exactly enough for the shared-exponent round-trip, then attach a
/// `GAMMA=` header.
fn gamma_image(gamma: f32) -> HdrImage {
    // Width >= 8 so the default new-RLE encoder accepts the scanline.
    // Values are sums of small negative powers of two so the RGBE
    // shared-exponent quantiser reproduces them without loss.
    let base = [0.5_f32, 0.25, 0.125];
    let pixels: Vec<f32> = (0..8).flat_map(|_| base).collect();
    let mut img = HdrImage::new_rgb96f(8, 1, pixels);
    img.header.gamma = Some(gamma);
    img
}

#[test]
fn gamma_header_survives_file_round_trip() {
    let src = gamma_image(2.2);
    let bytes = encode_hdr(&src).unwrap();
    // The header text must carry the record verbatim.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    assert!(head.contains("GAMMA=2.2"), "GAMMA= not emitted: {head}");
    let back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.gamma, Some(2.2));
    // effective_gamma reads the decoded slot, not the 1.0 default.
    assert!((back.effective_gamma() - 2.2).abs() < 1e-6);
}

#[test]
fn absent_gamma_decodes_to_linear_default() {
    let pixels: Vec<f32> = (0..8).flat_map(|_| [0.5_f32, 0.25, 0.125]).collect();
    let src = HdrImage::new_rgb96f(8, 1, pixels);
    let bytes = encode_hdr(&src).unwrap();
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    assert!(
        !head.contains("GAMMA="),
        "unexpected GAMMA= emitted: {head}"
    );
    let back = parse_hdr(&bytes).unwrap();
    assert!(back.header.gamma.is_none());
    // Default 1.0 ⇒ linearisation is the identity.
    assert!((back.effective_gamma() - 1.0).abs() < 1e-6);
    let before = back.pixels.clone();
    let mut lin = back.clone();
    lin.linearize_gamma();
    assert_eq!(lin.pixels, before, "absent GAMMA must be identity");
}

#[test]
fn decode_then_linearize_recovers_linear_channels() {
    // Encode a *linear* picture, gamma-encode it on the writer side, run
    // it through a real file round-trip, and confirm the decoder's
    // linearisation restores the linear channels to shared-exponent
    // precision.
    let linear: Vec<f32> = (0..8).flat_map(|_| [0.5_f32, 0.25, 0.125]).collect();
    let mut src = HdrImage::new_rgb96f(8, 1, linear.clone());
    assert!(src.apply_gamma_encoding(2.2));
    assert_eq!(src.header.gamma, Some(2.2));

    let bytes = encode_hdr(&src).unwrap();
    let mut back = parse_hdr(&bytes).unwrap();
    assert_eq!(back.header.gamma, Some(2.2));
    back.linearize_gamma();
    assert!(back.header.gamma.is_none());

    // RGBE quantisation plus the gamma power round-trip: loosen tolerance
    // to the ~1% the format's ±1-in-200 mantissa allows.
    for (a, b) in back.pixels.iter().zip(linear.iter()) {
        assert!((a - b).abs() < 2e-2, "{a} vs {b}");
    }
}

#[test]
fn full_decode_order_linearises_before_dividing_records() {
    // GAMMA + EXPOSURE + COLORCORR together. Construct stored channels so
    // that (stored^g) / (EXPOSURE * COLORCORR) == a known radiance, then
    // confirm the one-shot recovery reproduces it and clears every slot.
    // radiance target (1,1,1); g=2 so stored = sqrt(E*CC_i).
    let e = 4.0_f32;
    let cc = [1.0_f32, 4.0, 9.0];
    let stored: Vec<f32> = cc.iter().map(|c| (e * c).sqrt()).collect();
    let mut img = HdrImage::new_rgb96f(1, 1, stored);
    img.header.gamma = Some(2.0);
    img.header.exposure = Some(e);
    img.header.colorcorr = Some(cc);

    let expect = img.linear_scene_referred_radiance_buffer();
    img.recover_linear_scene_referred_radiance();
    for (a, b) in img.pixels.iter().zip(expect.iter()) {
        assert!((a - b).abs() < 1e-6, "mutator vs buffer: {a} vs {b}");
    }
    for c in &img.pixels {
        assert!((c - 1.0).abs() < 1e-5, "recovered radiance {c} != 1.0");
    }
    assert!(img.header.gamma.is_none());
    assert!(img.header.exposure.is_none());
    assert!(img.header.colorcorr.is_none());
}
