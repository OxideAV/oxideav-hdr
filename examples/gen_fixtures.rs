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
    encode_hdr, encode_hdr_with_options, encode_hdr_with_rle, AxisSign, HdrImage, LineEnding,
    Primaries, RleMode,
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
}
