//! Criterion micro-benchmarks for the Radiance HDR encoder fast path.
//!
//! Three representative inputs, each driven through the three
//! `RleMode` variants the encoder exposes:
//!
//! * `small_flat_64x64`        — 64×64 single-colour image. Smallest
//!   width still inside the new-RLE addressable range, exercises the
//!   per-scanline marker plus run-coder warm-up overhead.
//! * `medium_gradient_256x256` — 256×256 deterministic gradient that
//!   varies across all three channels. Imitates a photographic-ish
//!   payload (few long runs, lots of literals) so the encoder spends
//!   most of its time in the literal-emit path.
//! * `large_solid_1024x1024`   — 1024×1024 single-colour image. The
//!   classic long-run case; every scanline should compress into a
//!   single RLE chain after the row marker.
//!
//! Each input is synthesised inline from a fixed colour / formula —
//! no external fixture, no third-party reference image.
//!
//! Run with:
//!
//! ```sh
//! CARGO_TARGET_DIR=/tmp/oxideav-hdr-r131-target \
//!   cargo bench --bench encode -- --warm-up-time 1 --measurement-time 3
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use oxideav_hdr::{encode_hdr_with_rle, HdrImage, RleMode};

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
/// neighbours on at least one channel, defeating the encoder's
/// long-run path so most of the time is spent emitting literals.
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
            // Three channels picked so neither short nor long runs
            // dominate: a horizontal ramp, a vertical ramp, and a
            // diagonal cross-fade. The +0.05 floor keeps values out of
            // the underflow-to-zero region the RGBE encoder collapses
            // into the all-zero pixel.
            pixels.push(u + 0.05);
            pixels.push(v + 0.05);
            pixels.push((u + v) * 0.5 + 0.05);
        }
    }
    HdrImage::new_rgb96f(width, height, pixels)
}

fn bench_encode(c: &mut Criterion) {
    let cases: [(&str, HdrImage); 3] = [
        ("small_flat_64x64", solid_image(64, 64, [0.5, 0.5, 0.5])),
        ("medium_gradient_256x256", gradient_image(256, 256)),
        (
            "large_solid_1024x1024",
            solid_image(1024, 1024, [0.25, 0.5, 0.75]),
        ),
    ];

    for (label, img) in &cases {
        let raw_bytes = (img.width as u64) * (img.height as u64) * 3 * 4;
        let mut group = c.benchmark_group(format!("encode/{label}"));
        // Throughput in raw float bytes consumed by the encoder; lets
        // Criterion report MB/s alongside the wall time.
        group.throughput(Throughput::Bytes(raw_bytes));

        group.bench_function("new_rle", |b| {
            b.iter(|| {
                let out = encode_hdr_with_rle(black_box(img), RleMode::New)
                    .expect("encode (new RLE) failed");
                black_box(out);
            });
        });

        group.bench_function("old_rle", |b| {
            b.iter(|| {
                let out = encode_hdr_with_rle(black_box(img), RleMode::Old)
                    .expect("encode (old RLE) failed");
                black_box(out);
            });
        });

        group.bench_function("auto_rle", |b| {
            b.iter(|| {
                let out = encode_hdr_with_rle(black_box(img), RleMode::Auto)
                    .expect("encode (auto RLE) failed");
                black_box(out);
            });
        });

        group.bench_function("uncompressed", |b| {
            b.iter(|| {
                let out = encode_hdr_with_rle(black_box(img), RleMode::Uncompressed)
                    .expect("encode (uncompressed) failed");
                black_box(out);
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_encode);
criterion_main!(benches);
