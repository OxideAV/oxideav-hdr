//! Criterion micro-benchmarks for the per-pixel float stages that run
//! after decode / before encode:
//!
//! * `xyz/*`     — whole-image XYZE↔RGB conversion (`convert_image_xyz_to_rgb`
//!   / `convert_image_rgb_to_xyz`), both supported working spaces. The
//!   conversion mutates the buffer in place, so each iteration runs on
//!   a fresh clone via `iter_batched` (the clone cost is excluded from
//!   the measurement).
//! * `tonemap/*` — all eight tone-mapping operators driven through the
//!   public `tone_map` entry point on the same 256×256 input, default
//!   parameterisation for each operator (gamma 2.2, Reinhard white
//!   point 4.0, Hable linear white 11.2, Drago bias 0.85, etc.).
//!
//! The shared input is a 256×256 gradient whose radiance spans ~6
//! decades (1e-3 → 1e3) so the log/pow-heavy operators see realistic
//! magnitudes rather than a flat mid-grey fast path.
//!
//! Throughput is reported in pixels (`Throughput::Elements`) so the
//! per-pixel cost of each operator can be read straight off the
//! Melem/s figure.
//!
//! Run with:
//!
//! ```sh
//! CARGO_TARGET_DIR=/tmp/oxideav-hdr-target \
//!   cargo bench --bench pixels -- --warm-up-time 1 --measurement-time 3
//! ```

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use oxideav_hdr::{
    convert_image_rgb_to_xyz, convert_image_xyz_to_rgb, tone_map, HdrImage, RgbColorSpace, ToneMap,
};

/// Build a gradient whose radiance ramps from 1e-3 in the top-left
/// corner up to 1e3 in the bottom-right with a per-channel weighting,
/// so every magnitude band of the operators gets exercised.
fn wide_range_gradient(w: u32, h: u32) -> HdrImage {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            // Magnitude spans ~6 decades.
            let mag = 1e-3_f32 * 10.0_f32.powf(6.0 * (u + v) * 0.5);
            pixels.push(mag);
            pixels.push(mag * 0.5);
            pixels.push(mag * 0.25);
        }
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

fn bench_xyz(c: &mut Criterion) {
    let img = wide_range_gradient(256, 256);
    let n_px = (img.width as u64) * (img.height as u64);

    let mut group = c.benchmark_group("xyz/gradient_256x256");
    group.throughput(Throughput::Elements(n_px));

    for (space_label, space) in [
        ("radiance", RgbColorSpace::Radiance),
        ("srgb", RgbColorSpace::Srgb),
    ] {
        group.bench_function(format!("rgb_to_xyz/{space_label}"), |b| {
            b.iter_batched(
                || img.clone(),
                |mut work| {
                    convert_image_rgb_to_xyz(&mut work, space);
                    black_box(work);
                },
                BatchSize::LargeInput,
            );
        });

        group.bench_function(format!("xyz_to_rgb/{space_label}"), |b| {
            b.iter_batched(
                || img.clone(),
                |mut work| {
                    convert_image_xyz_to_rgb(&mut work, space);
                    black_box(work);
                },
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

fn bench_tonemap(c: &mut Criterion) {
    let img = wide_range_gradient(256, 256);
    let n_px = (img.width as u64) * (img.height as u64);

    let ops: [(&str, ToneMap); 8] = [
        ("linear", ToneMap::Linear { exposure: 1.0 }),
        (
            "gamma",
            ToneMap::Gamma {
                exposure: 1.0,
                gamma: 2.2,
            },
        ),
        ("reinhard", ToneMap::Reinhard { exposure: 1.0 }),
        (
            "reinhard_extended",
            ToneMap::ReinhardExtended {
                exposure: 1.0,
                white_point: 4.0,
            },
        ),
        (
            "reinhard_luminance",
            ToneMap::ReinhardLuminance {
                exposure: 1.0,
                white_point: 4.0,
            },
        ),
        (
            "hable",
            ToneMap::Hable {
                exposure: 1.0,
                linear_white: 11.2,
            },
        ),
        (
            "drago",
            ToneMap::Drago {
                exposure: 1.0,
                scene_max: 1e3,
                bias: 0.85,
            },
        ),
        ("aces", ToneMap::Aces { exposure: 1.0 }),
    ];

    let mut group = c.benchmark_group("tonemap/gradient_256x256");
    group.throughput(Throughput::Elements(n_px));

    for (label, op) in ops {
        group.bench_function(label, |b| {
            b.iter(|| {
                let out = tone_map(black_box(&img), black_box(op));
                black_box(out);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_xyz, bench_tonemap);
criterion_main!(benches);
