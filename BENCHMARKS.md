# oxideav-hdr — benchmark suite (round 285)

Criterion micro-benchmarks covering the whole hot surface of the crate:
both directions of the Radiance RGBE codec in all three on-disk
scanline flavours, the XYZE↔RGB working-space conversion, and all
eight tone-mapping operators.

| Bench target | File | What it times |
|--------------|------|---------------|
| `encode` | [`benches/encode.rs`](benches/encode.rs) | `encode_hdr_with_rle` in `New`, `Old`, `Auto`, `Uncompressed` modes |
| `decode` | [`benches/decode.rs`](benches/decode.rs) | `parse_hdr` on new-RLE / old-RLE wires, `parse_hdr_with_options` + `FallbackMode::Uncompressed` on flat wires |
| `pixels` | [`benches/pixels.rs`](benches/pixels.rs) | `convert_image_rgb_to_xyz` / `convert_image_xyz_to_rgb` (both working spaces) and `tone_map` with each of the 8 operators |

All inputs are synthesised inline (solid colours, deterministic
gradients) — no external fixture and no third-party reference image.
The `pixels` input is a 256×256 gradient spanning ~6 decades of
radiance (1e-3 → 1e3) so the log/pow-heavy operators see realistic
magnitudes.

Reproduce any suite with:

```sh
CARGO_TARGET_DIR=/tmp/oxideav-hdr-target \
  cargo bench -p oxideav-hdr --bench decode -- --warm-up-time 1 --measurement-time 3
```

Numbers below were collected on an Apple Silicon laptop (release
profile, single-threaded). Read them as relative cost between paths,
not absolute platform numbers; run-to-run spread is a few percent.

## Codec throughput

Throughput is normalised to the *float-side* buffer
(`width × height × 3 × 4` bytes) on both directions, so encode, decode
and the three flavours are directly comparable regardless of how well
each flavour compresses on the wire.

### Decode (`parse_hdr*`)

| Input | Flavour | Median time | Throughput | ns/pixel |
|-------|--------:|------------:|-----------:|---------:|
| 64×64 solid | new-RLE | 19.0 µs | 2.41 GiB/s | 4.63 |
| 64×64 solid | old-RLE | 19.5 µs | 2.34 GiB/s | 4.77 |
| 64×64 solid | uncompressed | 20.5 µs | 2.24 GiB/s | 4.99 |
| 256×256 gradient | new-RLE | 169 µs | 4.35 GiB/s | 2.57 |
| 256×256 gradient | old-RLE | 309 µs | 2.38 GiB/s | 4.72 |
| 256×256 gradient | uncompressed | 264 µs | 2.78 GiB/s | 4.03 |
| 1024×1024 solid | new-RLE | 2.35 ms | 4.98 GiB/s | 2.24 |
| 1024×1024 solid | old-RLE | 2.30 ms | 5.09 GiB/s | 2.20 |
| 1024×1024 solid | uncompressed | 3.99 ms | 2.94 GiB/s | 3.81 |

### Encode (`encode_hdr_with_rle`)

| Input | Flavour | Median time | Throughput | ns/pixel |
|-------|--------:|------------:|-----------:|---------:|
| 64×64 solid | new-RLE | 19.5 µs | 2.35 GiB/s | 4.76 |
| 64×64 solid | old-RLE | 16.0 µs | 2.86 GiB/s | 3.90 |
| 64×64 solid | auto | 19.2 µs | 2.38 GiB/s | 4.69 |
| 64×64 solid | uncompressed | 19.8 µs | 2.32 GiB/s | 4.82 |
| 256×256 gradient | new-RLE | 342 µs | 2.14 GiB/s | 5.21 |
| 256×256 gradient | old-RLE | 267 µs | 2.74 GiB/s | 4.08 |
| 256×256 gradient | auto | 341 µs | 2.15 GiB/s | 5.20 |
| 256×256 gradient | uncompressed | 308 µs | 2.38 GiB/s | 4.70 |
| 1024×1024 solid | new-RLE | 5.03 ms | 2.33 GiB/s | 4.80 |
| 1024×1024 solid | old-RLE | 3.82 ms | 3.07 GiB/s | 3.64 |
| 1024×1024 solid | auto | 4.95 ms | 2.37 GiB/s | 4.72 |
| 1024×1024 solid | uncompressed | 4.93 ms | 2.38 GiB/s | 4.70 |

## Per-pixel float stages (`pixels` bench, 256×256 = 65 536 px)

| Stage | Median time | ns/pixel | Mpx/s |
|-------|------------:|---------:|------:|
| `xyz_to_rgb` (Radiance space) | 22.2 µs | 0.34 | 2 950 |
| `rgb_to_xyz` (Radiance space) | 22.7 µs | 0.35 | 2 890 |
| `rgb_to_xyz` (sRGB space) | 22.2 µs | 0.34 | 2 950 |
| `xyz_to_rgb` (sRGB space) | 22.6 µs | 0.35 | 2 890 |
| `tone_map` Linear | 245 µs | 3.74 | 268 |
| `tone_map` Gamma (2.2) | 545 µs | 8.32 | 120 |
| `tone_map` Reinhard | 781 µs | 11.9 | 84 |
| `tone_map` ReinhardExtended | 812 µs | 12.4 | 81 |
| `tone_map` Aces | 958 µs | 14.6 | 68 |
| `tone_map` Hable | 972 µs | 14.8 | 67 |
| `tone_map` ReinhardLuminance | 1.10 ms | 16.8 | 60 |
| `tone_map` Drago | 2.66 ms | 40.6 | 25 |

## Ranked hotspot table

Per-pixel cost across the whole crate surface, slowest first
(256×256 medium workload unless noted):

| Rank | Path | ns/pixel | Where the time goes |
|-----:|------|---------:|---------------------|
| 1 | `tonemap::drago` | 40.6 | Per-channel `drago()` recomputes loop-invariant transcendentals on **every** call: `log_bias = bias.ln() / 0.5f32.ln()`, `log_max = (1 + lwmax).ln()`, `log_max_denom = (2 + 8·1^log_bias).ln()` — all constant for the whole image — plus the genuinely per-sample `powf` + two `ln` and the shared sRGB OETF `powf`. ~2.4× the next-slowest operator. |
| 2 | `tonemap::reinhard_luminance` | 16.8 | Three sRGB OETF `powf` calls + luminance weighting + extended curve per pixel. |
| 3 | `tonemap::hable` | 14.8 | `hable_curve(linear_white)` normalisation denominator is recomputed per pixel (loop-invariant, hoistable) + three sRGB OETF `powf`. |
| 4 | `tonemap::aces` | 14.6 | Rational polynomial is cheap; the three sRGB OETF `powf` calls dominate. |
| 5 | `tonemap::reinhard_extended` / `reinhard` | 12.4 / 11.9 | Same sRGB OETF `powf` floor; curve itself is a handful of flops. |
| 6 | `tonemap::gamma` | 8.3 | Three `powf(1/γ)` per pixel, no OETF. |
| 7 | encode new-RLE | 5.2 | `rgb_to_rgbe` shared-exponent conversion + 4-channel staging-buffer deinterleave + run scan. |
| 8 | decode old-RLE / encode uncompressed | 4.7 | Sentinel probe per quad / literal quad emit. |
| 9 | encode old-RLE / decode uncompressed | 4.1 / 4.05 | |
| 10 | `tonemap::linear` | 3.7 | No `powf` anywhere — confirms `powf` is the operator floor, not `tone_map`'s loop or the output `Vec` push. |
| 11 | decode new-RLE / old-RLE | 2.2–2.6 | Repeat-run decode now bulk-fills each channel with one `resize` (`memset`) instead of a byte-at-a-time `push` loop (round 332): the 1024×1024 solid decode dropped to 2.24 (new-RLE) / 2.20 (old-RLE) ns/px — a ~38 % wall-time cut on run-heavy inputs — so the remaining cost is the per-pixel RGBE→f32 reconstruct + channel re-interleave, not the run expansion. The literal-heavy uncompressed path (no runs) is unchanged at 3.81. |
| 12 | `convert_image_rgb_to_xyz` / `xyz_to_rgb` | 0.34 | 9 mul + 6 add per pixel; effectively memory-bandwidth bound. Saturated. |

## Next PROFILE-OPT target

**`src/tonemap.rs` — `ToneMap::Drago`, then the shared per-pixel
dispatch.** Concretely, in cost order:

1. Hoist the three loop-invariant transcendentals out of `drago()`
   (`log_bias`, `log_max`, `log_max_denom` depend only on `bias` /
   `scene_max`, both fixed for the whole image). Expected ≥2× on the
   Drago row, taking it from 2.4× outlier to pack-median.
2. Same hoist for Hable's `hable_curve(linear_white)` denominator
   (constant per image, currently evaluated per pixel).
3. The ~9–10 ns/px sRGB OETF `powf` floor shared by ranks 2–5 and the
   per-pixel `match op` dispatch inside `tone_map`'s loop are the
   structural follow-up (per-operator specialised loops with hoisted
   constants).

Secondary codec-side target (separate, smaller win): the decode
hot path spends its time in the RGBE→f32 reconstruct + four-channel
staging-buffer plumbing rather than wire parsing (rank-11 observation),
so a future round can fuse the per-channel buffers into a direct
pixel-buffer write.

Round 332 landed the rank-11 repeat-run bulk-fill micro-opt above
(`src/rle.rs` `decode_new_rle` / `decode_old_rle`): the byte-at-a-time
`push` loops became single `Vec::resize` calls. Behavioural guarantee:
the decode output is bit-identical (only the fill mechanism changed),
covered by `new_rle_long_repeat_run_bulk_fill` /
`old_rle_long_repeat_run_bulk_fill` plus the existing round-trip suite.
