//! Criterion micro-benchmarks for the Radiance HDR decoder.
//!
//! Mirrors `benches/encode.rs`: the same three synthetic images are
//! first encoded in-memory in each of the three on-disk scanline
//! flavours, then `parse_hdr` is timed on the resulting byte buffers:
//!
//! * `new_rle`      — `0x02 0x02 hi lo` marker + per-channel run/literal
//!   chains (the post-1991 flavour every modern writer emits).
//! * `old_rle`      — per-pixel literals with `(1, 1, 1, n)` repeat
//!   sentinels (pre-1991 grammar), decoded through the default
//!   `FallbackMode::OldRle` branch.
//! * `uncompressed` — flat `4 × width` literal quads per scanline,
//!   decoded through `parse_hdr_with_options` with
//!   `FallbackMode::Uncompressed` (the documented pairing for the
//!   flat flavour, which never misreads a literal `(1, 1, 1, *)`
//!   pixel as a run sentinel).
//!
//! Throughput is reported in *decoded* float bytes
//! (`width × height × 3 × 4`) so the three flavours are directly
//! comparable to each other and to the encoder benches, independent of
//! how well each flavour compresses on the wire.
//!
//! Each input is synthesised inline from a fixed colour / formula —
//! no external fixture, no third-party reference image.
//!
//! Run with:
//!
//! ```sh
//! CARGO_TARGET_DIR=/tmp/oxideav-hdr-target \
//!   cargo bench --bench decode -- --warm-up-time 1 --measurement-time 3
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use oxideav_hdr::{
    encode_hdr_with_rle, parse_hdr, parse_hdr_with_options, FallbackMode, HdrImage, RleMode,
};

/// Build a flat single-colour image of the given dimensions.
fn solid_image(width: u32, height: u32, rgb: [f32; 3]) -> HdrImage {
    let n = (width as usize) * (height as usize);
    let mut pixels = Vec::with_capacity(n * 3);
    for _ in 0..n {
        pixels.push(rgb[0]);
        pixels.push(rgb[1]);
        pixels.push(rgb[2]);
    }
    HdrImage::new_rgb96f(width, height, pixels)
}

/// Build a deterministic gradient where every pixel differs from its
/// neighbours on at least one channel, defeating the run decoder's
/// long-run path so most of the time is spent in the literal path.
fn gradient_image(width: u32, height: u32) -> HdrImage {
    let w = width as usize;
    let h = height as usize;
    let mut pixels = Vec::with_capacity(w * h * 3);
    let fw = (width as f32).max(1.0);
    let fh = (height as f32).max(1.0);
    for y in 0..h {
        let v = (y as f32) / fh;
        for x in 0..w {
            let u = (x as f32) / fw;
            // Horizontal ramp / vertical ramp / diagonal cross-fade,
            // floored at 0.05 to stay out of the all-zero-pixel
            // collapse region of the shared-exponent encoder.
            pixels.push(u + 0.05);
            pixels.push(v + 0.05);
            pixels.push((u + v) * 0.5 + 0.05);
        }
    }
    HdrImage::new_rgb96f(width, height, pixels)
}

fn bench_decode(c: &mut Criterion) {
    let cases: [(&str, HdrImage); 3] = [
        ("small_flat_64x64", solid_image(64, 64, [0.5, 0.5, 0.5])),
        ("medium_gradient_256x256", gradient_image(256, 256)),
        (
            "large_solid_1024x1024",
            solid_image(1024, 1024, [0.25, 0.5, 0.75]),
        ),
    ];

    for (label, img) in &cases {
        // Pre-encode once per flavour; the bench times decode only.
        let new_rle = encode_hdr_with_rle(img, RleMode::New).expect("encode (new RLE) failed");
        let old_rle = encode_hdr_with_rle(img, RleMode::Old).expect("encode (old RLE) failed");
        let flat =
            encode_hdr_with_rle(img, RleMode::Uncompressed).expect("encode (uncompressed) failed");

        // One-shot sanity pass so a decode regression fails loudly here
        // instead of silently timing an error path.
        for (bytes, fallback) in [
            (&new_rle, FallbackMode::OldRle),
            (&old_rle, FallbackMode::OldRle),
            (&flat, FallbackMode::Uncompressed),
        ] {
            let back = parse_hdr_with_options(bytes, fallback).expect("sanity decode failed");
            assert_eq!(back.width, img.width);
            assert_eq!(back.height, img.height);
        }

        // Decoded-output bytes (3 × f32 per pixel), not wire bytes —
        // keeps the three flavours on a common denominator.
        let decoded_bytes = (img.width as u64) * (img.height as u64) * 3 * 4;
        let mut group = c.benchmark_group(format!("decode/{label}"));
        group.throughput(Throughput::Bytes(decoded_bytes));

        group.bench_function("new_rle", |b| {
            b.iter(|| {
                let out = parse_hdr(black_box(&new_rle)).expect("decode (new RLE) failed");
                black_box(out);
            });
        });

        group.bench_function("old_rle", |b| {
            b.iter(|| {
                let out = parse_hdr(black_box(&old_rle)).expect("decode (old RLE) failed");
                black_box(out);
            });
        });

        group.bench_function("uncompressed", |b| {
            b.iter(|| {
                let out = parse_hdr_with_options(black_box(&flat), FallbackMode::Uncompressed)
                    .expect("decode (uncompressed) failed");
                black_box(out);
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_decode);
criterion_main!(benches);
