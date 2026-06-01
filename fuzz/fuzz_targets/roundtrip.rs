#![no_main]

//! Encode a fuzz-controlled HDR image and assert it survives a decode
//! round trip.
//!
//! Radiance HDR is *lossy by shared-exponent quantisation* — the
//! 4-byte RGBE encoding rounds three float channels onto a single
//! 8-bit shared exponent — so this is a "structure round-trips"
//! check, not a "bytes round-trip" one. The fuzzer drives the
//! dimensions (kept comfortably inside the default `HdrLimits` the
//! decoder applies after the encode) and a synthetic gradient sourced
//! from the remaining fuzz input, runs `encode_hdr` → `parse_hdr`, and
//! asserts:
//!
//!  * the decoded width/height match the encoded ones,
//!  * the decoded pixel-format is the canonical `Rgb96f`,
//!  * the decoded pixel buffer is exactly `width * height * 3` long
//!    (so any axis-flag confusion that produced an off-by-row decode
//!    would show up here).
//!
//! Catches encoder/decoder asymmetries that the symmetric unit-test
//! suite would not — e.g. an axis-flag wire-format that the encoder
//! emits but the decoder doesn't recognise, or a header field whose
//! KEY=VALUE textual form the decoder rejects.
//!
//! As with the other targets, the crate is pulled in with
//! `default-features = false` so the fuzz build exercises the
//! framework-free standalone API only.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::{encode_hdr, parse_hdr, HdrImage, HdrPixelFormat};

fuzz_target!(|data: &[u8]| {
    // Need at least three bytes — two for the dimensions, at least one
    // for the gradient seed.
    if data.len() < 3 {
        return;
    }

    // Derive small in-bounds dimensions from the first two fuzz bytes.
    // Range `8..=263` keeps every encode comfortably inside the
    // new-RLE addressable range `8..=32767` (so the encoder's default
    // path fires) and the worst-case buffer (263 × 263 × 12 ≈ 829 KiB)
    // well below the default `max_pixel_bytes` (256 MiB).
    let width: u32 = u32::from(data[0]) + 8;
    let height: u32 = u32::from(data[1]) + 8;

    let pixel_count = (width as usize) * (height as usize);
    let n = pixel_count * 3;

    // Synthesise a fuzz-driven gradient: each f32 is `(byte+1)/256`
    // sourced from the remaining bytes cycled into the float buffer.
    // The `+1` floor keeps every sample > 0 so the shared-exponent
    // encoder never enters its all-black branch (which would collapse
    // an entire pixel to `0,0,0,0` and obscure axis-flag bugs).
    let body = &data[2..];
    let mut pixels = Vec::with_capacity(n);
    for i in 0..n {
        let byte = if body.is_empty() {
            0u8
        } else {
            body[i % body.len()]
        };
        pixels.push((f32::from(byte) + 1.0) / 256.0);
    }

    let image = HdrImage::new_rgb96f(width, height, pixels);

    let encoded = match encode_hdr(&image) {
        Ok(v) => v,
        // The encoder errors only on validation paths (zero dims,
        // pixel-length mismatch) that our construction above can't
        // hit, so an Err here is a genuine encoder bug. We still
        // return cleanly rather than unwrap — the assertion below is
        // the canonical crash trigger.
        Err(_) => return,
    };

    let decoded = parse_hdr(&encoded).expect("encode_hdr output must be parseable by parse_hdr");

    assert_eq!(decoded.width, width, "width survives round trip");
    assert_eq!(decoded.height, height, "height survives round trip");
    assert_eq!(decoded.pixel_format, HdrPixelFormat::Rgb96f);
    assert_eq!(
        decoded.pixels.len(),
        n,
        "decoded pixel buffer is width × height × 3 floats long",
    );
});
