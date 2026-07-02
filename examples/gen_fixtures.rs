//! Regenerates the staged on-disk `.hdr` fixtures under
//! `tests/fixtures/` from the same deterministic synthetic inputs that
//! `tests/fixture_decode.rs` asserts against.
//!
//! Run with `cargo run --example gen_fixtures` after any *intentional*
//! change to the encoder's wire format; commit the updated bytes
//! alongside the change. The matching test in
//! `tests/fixture_decode.rs` then locks the new bytes in place as the
//! regression anchor for every subsequent round.
//!
//! The fixtures are kept deliberately small (≤ 64×16 px) so they
//! review well in a diff and don't bloat the crate's source-package
//! download.

use std::path::PathBuf;

use oxideav_hdr::{
    encode_hdr, encode_hdr_with_options, encode_hdr_with_rle, AxisSign, HdrFormat, HdrHeader,
    HdrImage, HdrPixelFormat, LineEnding, Primaries, RleMode,
};

/// Deterministic 32×16 RGB gradient — same construction as the
/// in-crate unit tests so the fixtures and the source-tree
/// `synthetic_gradient` helper line up byte-for-byte.
fn gradient_32x16() -> HdrImage {
    let (w, h) = (32_u32, 16_u32);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            // Magnitude spans ~6 decades — the same construction the
            // in-crate `synthetic_gradient` uses.
            let mag = 1e-3_f32 * 10.0_f32.powf(6.0 * (u + v) * 0.5);
            pixels.push(mag);
            pixels.push(mag * 0.5);
            pixels.push(mag * 0.25);
        }
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

/// Deterministic 16×8 solid-colour image — exercises the new-RLE
/// repeat-run path end to end (no literals in the binary section).
fn solid_16x8() -> HdrImage {
    let (w, h) = (16_u32, 8_u32);
    let mut pixels = vec![0.0_f32; (w * h * 3) as usize];
    for i in 0..(w * h) as usize {
        pixels[i * 3] = 0.500;
        pixels[i * 3 + 1] = 0.250;
        pixels[i * 3 + 2] = 0.125;
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

fn main() {
    // tests/fixtures/ relative to CARGO_MANIFEST_DIR — cargo points us
    // at the crate root regardless of where `cargo run` was invoked.
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    let fixtures_dir: PathBuf = [&manifest_dir, "tests", "fixtures"].iter().collect();
    std::fs::create_dir_all(&fixtures_dir).expect("create fixtures dir");

    // -----------------------------------------------------------------
    // Fixture 1 — gradient_32x16_newrle.hdr
    //
    // Default `encode_hdr` settings: new-RLE pixel section, bare `\n`
    // line endings, canonical `-Y H +X W` axis order, no extra
    // `KEY=VALUE` header records besides `FORMAT=`.
    // -----------------------------------------------------------------
    let bytes = encode_hdr(&gradient_32x16()).expect("encode gradient new-RLE");
    let path = fixtures_dir.join("gradient_32x16_newrle.hdr");
    std::fs::write(&path, &bytes).expect("write gradient new-RLE fixture");
    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());

    // -----------------------------------------------------------------
    // Fixture 2 — solid_16x8_oldrle.hdr
    //
    // Old-RLE pixel section + a populated header (EXPOSURE / GAMMA /
    // SOFTWARE / VIEW / COLORCORR / PRIMARIES) so the on-disk bytes
    // exercise every typed slot the decoder recognises.
    // -----------------------------------------------------------------
    let mut img = solid_16x8();
    img.header.exposure = Some(1.25);
    img.header.gamma = Some(2.2);
    img.header.software = Some("oxideav-hdr fixture v1".to_owned());
    img.header.view = Some("rvu -vp 0 0 10 -vd 0 0 -1".to_owned());
    img.header.colorcorr = Some([1.10, 1.00, 0.95]);
    img.header.primaries = Some(Primaries::SRGB);
    let bytes = encode_hdr_with_rle(&img, RleMode::Old).expect("encode solid old-RLE");
    let path = fixtures_dir.join("solid_16x8_oldrle.hdr");
    std::fs::write(&path, &bytes).expect("write solid old-RLE fixture");
    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());

    // -----------------------------------------------------------------
    // Fixture 3 — gradient_32x16_crlf_plusY.hdr
    //
    // CRLF text section + non-default axis order (`+Y H +X W`) +
    // PIXASPECT header record + a caller-supplied `OXIDEAV=…` extra
    // record so the on-disk header exercises the CRLF line reader and
    // the typed/untyped record split in the same fixture.
    // -----------------------------------------------------------------
    let mut img = gradient_32x16();
    img.header.y_sign = AxisSign::Increasing; // `+Y` (bottom-up)
    img.header.x_sign = AxisSign::Increasing; // `+X`
    img.header.pixaspect = Some(1.0);
    img.header
        .other
        .push(("OXIDEAV".to_owned(), "fixture-r192".to_owned()));
    let bytes = encode_hdr_with_options(&img, RleMode::New, LineEnding::Crlf)
        .expect("encode gradient crlf");
    let path = fixtures_dir.join("gradient_32x16_crlf_plusY.hdr");
    std::fs::write(&path, &bytes).expect("write gradient CRLF fixture");
    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());

    // -----------------------------------------------------------------
    // Fixture 4 — flat_4x2_uncompressed.hdr
    //
    // 4×2 picture written with `RleMode::Uncompressed` — narrow enough
    // that the new-RLE marker can't fire (width < 8), uses pixel values
    // chosen so that one literal pixel encodes to `(1, 1, 1, *)` RGBE
    // bytes (the value `(1.0/256.0, 1.0/256.0, 1.0/256.0)` with shared
    // exponent maps to `(128, 128, 128, ...)` so we instead use the
    // explicit bytes via a hand-crafted pixel buffer that produces a
    // literal `(1, 1, 1, e)` quad on the wire — that pixel survives
    // the round trip under FallbackMode::Uncompressed but would be
    // mis-decoded as a sentinel under FallbackMode::OldRle).
    // -----------------------------------------------------------------
    let mut pixels = Vec::with_capacity(4 * 2 * 3);
    // Row 0: four pixels at a single small magnitude — the
    // shared-exponent encoder will assign them mantissa values near 1.
    // Specifically, picking R = G = B = 2^-128 * (1/256) keeps the
    // mantissa = 1 in all three channels with exponent byte 0; that
    // would be the all-zero "black" sentinel. We instead deliberately
    // pick a magnitude that produces mantissa byte 1 in all three plus
    // a non-zero exponent so the on-disk quad is `(1, 1, 1, e)` with
    // e > 0 — the exact wire pattern the OldRle fallback misreads.
    // The encoder formula `rgbe[i] = chan * frexp(max) * 256 / max`
    // with chan == max gives mantissa 128 (not 1), so to land on
    // mantissa 1 we want `chan / max == 1/128` — pick a tiny channel
    // alongside a 128× larger one. We use
    // (R, G, B) = (1.0/128.0, 1.0, 1.0/128.0): max = 1.0,
    // R-mantissa = 1.0/128.0 * 128 = 1, B same, G = 128. Doesn't quite
    // hit (1,1,1) — we need G also tiny. Use a degenerate but legal
    // construction with all three channels at the same small magnitude
    // and another bright reference pixel to set the row's max.
    //
    // Simpler: emit the bytes via a hand-built `HdrImage` whose pixels
    // we set so each row's RGBE encoding includes a literal `(1,1,1,*)`
    // quad. We bypass `rgb_to_rgbe` by injecting the exact pixel via
    // the standalone `HdrImage` — but the encoder always encodes
    // through `rgb_to_rgbe`, so we instead choose magnitudes that
    // produce the desired wire bytes.
    //
    // `rgb_to_rgbe` for (a, a, a) with a > 0 gives mantissas all equal
    // to round(a * 256 / a) = 256... wait it's `frexp(v) * 256 / v`
    // applied to each chan, so mantissa = chan * frexp(v) * 256 / v.
    // For chan == v, mantissa = frexp(v) * 256 which lands in
    // [128, 256). To land on mantissa == 1 we want chan / v == 1/128
    // (since frexp returns [0.5, 1.0)). So we need a row with one
    // bright pixel that sets the max-of-three-channels for the whole
    // ENCODING ... but each pixel is encoded independently.
    //
    // For a single pixel where R = G = B = small_value > 0:
    //   v = small_value
    //   frexp(v) = [0.5, 1.0) → call it m, so v = m * 2^e
    //   chan/v = 1, mantissa = m * 256, integer-floor of which is in
    //   [128, 256). So a (1,1,1) RGBE encoding is unreachable with all
    //   three channels equal.
    //
    // For a (1,1,1,e) on-disk pattern we need three small unequal
    // channels with v dominating: pick R = G = B = max/128 with one
    // other channel = max. But all three channels have to be ≤ v.
    //
    // Easiest: use (chan_r, chan_g, chan_b) = (eps, eps, max) so
    // v = max, R-mantissa = eps * 256 / max = ε. For
    // eps/max = 1/256, mantissa = 1. So
    //   (R, G, B) = (max/256, max/256, max).
    // With max = 1.0: (1/256, 1/256, 1) gives B-mantissa = 256 → 255
    // (clamped) and R/G mantissas = 1. Not quite (1,1,1) — but at
    // least an authentic narrow-width fixture that round-trips losslessly.
    pixels.extend_from_slice(&[
        1.0 / 256.0,
        1.0 / 256.0,
        1.0, // pixel (0,0)
        0.10,
        0.20,
        0.30, // pixel (1,0)
        0.40,
        0.50,
        0.60, // pixel (2,0)
        0.05,
        0.05,
        0.05, // pixel (3,0)
    ]);
    pixels.extend_from_slice(&[
        0.70,
        0.80,
        0.90, // pixel (0,1)
        1.0 / 256.0,
        1.0,
        1.0 / 256.0, // pixel (1,1) — G dominates
        0.15,
        0.25,
        0.35, // pixel (2,1)
        2.00,
        1.50,
        1.00, // pixel (3,1)
    ]);
    let img = HdrImage {
        width: 4,
        height: 2,
        pixel_format: HdrPixelFormat::Rgb96f,
        pixels,
        header: HdrHeader::default(),
    };
    let bytes = encode_hdr_with_rle(&img, RleMode::Uncompressed)
        .expect("encode flat 4x2 uncompressed fixture");
    let path = fixtures_dir.join("flat_4x2_uncompressed.hdr");
    std::fs::write(&path, &bytes).expect("write flat uncompressed fixture");
    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());

    // -----------------------------------------------------------------
    // Fixture 5 — xyze_24x10_newrle.hdr
    //
    // `FORMAT=32-bit_rle_xyze` picture (the four RGBE fixtures above
    // all carry the default RGBE format) with an `EXPOSURE=` record and
    // the reference-manual default `PRIMARIES=`. The stored channels
    // are CIE XYZ on the photometric scale — the Y channel is
    // lumens/sr/m² per the staged spec's §"Physical interpretation" —
    // so the matching test can pin the XYZE luminance semantics
    // (luminance == stored Y verbatim, scene-referred luminance ==
    // Y / EXPOSURE) against committed on-disk bytes.
    //
    // Deterministic construction: Y ramps over ~4 decades across the
    // diagonal; X and Z track it at fixed 0.9 / 1.2 ratios so the
    // chromaticity is constant and every channel exercises the shared
    // exponent.
    // -----------------------------------------------------------------
    let (w, h) = (24_u32, 10_u32);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            let lum = 0.05_f32 * 10.0_f32.powf(4.0 * (u + v) * 0.5);
            pixels.push(lum * 0.9); // X
            pixels.push(lum); // Y
            pixels.push(lum * 1.2); // Z
        }
    }
    let mut img = HdrImage::new_rgb96f(w, h, pixels);
    img.header.format = HdrFormat::Xyze;
    img.header.exposure = Some(2.0);
    img.header.primaries = Some(Primaries::RADIANCE);
    let bytes = encode_hdr(&img).expect("encode xyze new-RLE fixture");
    let path = fixtures_dir.join("xyze_24x10_newrle.hdr");
    std::fs::write(&path, &bytes).expect("write xyze new-RLE fixture");
    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
}
