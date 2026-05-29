# oxideav-hdr

Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader + writer for the
[oxideav](https://github.com/OxideAV/oxideav-workspace) workspace.

Greg Ward's shared-exponent floating-point image format, originally
described in *Real Pixels* (Graphics Gems II, 1991) and implemented
across the Radiance synthetic-imaging system. The on-disk
representation packs three 8-bit RGB mantissa bytes plus one shared
8-bit biased exponent into 4 bytes per pixel, then RLE-codes each
scanline. The decoder produces packed `f32` RGB triples; the encoder
takes the same shape and emits a complete file with the canonical
`-Y H +X W` axis flags.

Clean-room implementation against the published format documentation
(*Real Pixels*, the `radsite.lbl.gov` Radiance Reference Manual). No
Radiance source / `image` crate's `hdr` submodule / Greg Ward's
reference C code consulted.

## Coverage (round 5)

| Feature                      | Read | Write |
|------------------------------|:----:|:-----:|
| `#?RADIANCE` / `#?RGBE` magic|  Y   |   Y   |
| `KEY=VALUE` header records   |  Y   |   Y   |
| `EXPOSURE` / `GAMMA` / `PIXASPECT` / `SOFTWARE` | Y | Y |
| `VIEW=` renderer view-parameter record | Y | Y |
| Multiple `EXPOSURE` / `COLORCORR` records stacked multiplicatively | Y | n/a |
| `COLORCORR` (3-float)        |  Y   |   Y   |
| `PRIMARIES` (8-float chromaticity) | Y |   Y   |
| All 8 axis-flag combinations |  Y   |  Y (Y-first + X-first transpose) |
| 32-bit_rle_rgbe pixels       |  Y   |   Y   |
| 32-bit_rle_xyze pixels       |  Y   |   Y (with helpers in `xyz`) |
| New RLE (`0x02 0x02 hi lo`)  |  Y   |   Y   |
| Old RLE (sentinel pixels)    |  Y   |   Y (`RleMode::Old`) |
| Auto-RLE (width heuristic)   |  -   |   Y (`RleMode::Auto`) |
| CRLF line endings            |  Y   |   Y (`LineEnding::Crlf`) |
| `HdrImage::apply_exposure`   |  decode helper |  n/a |
| `HdrImage::apply_colorcorr`  |  decode helper |  n/a |
| XYZE ↔ RGB (sRGB / Radiance) |  -   | helpers |
| `Primaries::SRGB` / `RADIANCE` / `P3_D65` / `REC2020` constants | n/a | constants |
| Tone-mapping (Linear / Gamma / Reinhard / ReinhardExtended / ReinhardLuminance / Hable / Drago / ACES) | - | helpers |
| Radiance photometric luminance (`179 * (0.265 R + 0.670 G + 0.065 B)` for RGBE; `179 * Y` for XYZE) | helper (`luminance_lm_per_sr_per_m2`, `HdrImage::luminance_buffer`) | n/a |

Cross-validated against ImageMagick 7's HDR codec (encoder output is
decodable by `magick`, ImageMagick-written `.hdr` files round-trip
through our decoder, XYZE↔RGB matrix tracks ImageMagick's chroma
adaptation within the format's shared-exponent precision).

## Standalone vs registry-integrated

Default `registry` Cargo feature on:

```toml
oxideav-hdr = "0.0"
```

Pulls `oxideav-core` and exposes the `Decoder` / `Encoder` trait
surface plus a `register()` entry point. Tone-maps to `Rgb24` at the
framework boundary so the generic `VideoFrame` representation stays
representable; the float dynamic range is preserved on the standalone
API.

Image-library use cases that just want a framework-free
`parse_hdr` / `encode_hdr`:

```toml
oxideav-hdr = { version = "0.0", default-features = false }
```

Skips the `oxideav-core` dependency entirely and exposes only
crate-local `HdrImage` / `HdrPixelFormat` / `HdrError` types.

## Public API

```rust
use oxideav_hdr::{encode_hdr, parse_hdr, HdrImage};

let bytes = std::fs::read("scene.hdr").unwrap();
let img: HdrImage = parse_hdr(&bytes).unwrap();
// img.pixels is width*height*3 packed f32 RGB, top-down memory order.

let back = encode_hdr(&img).unwrap();
// `back` round-trips img to the same shared-exponent precision.
```

## Performance

Criterion micro-benchmarks live in [`benches/encode.rs`](benches/encode.rs)
and exercise the three `RleMode` paths (`New`, `Old`, `Auto`) on three
representative inline-synthesised inputs. Numbers below were collected
with `cargo bench --bench encode -- --warm-up-time 1 --measurement-time
3` on an Apple Silicon laptop (release profile, single-threaded, no
prefetch tweaks); they are reproducible run-to-run within a few percent
but should be read as relative throughput between the modes rather
than absolute platform numbers.

| Input                                | RLE mode | Median time | Throughput (raw float bytes) |
|--------------------------------------|---------:|------------:|-----------------------------:|
| 64×64 solid colour                   | `New`    | 20.4 µs     | 2.24 GiB/s                   |
| 64×64 solid colour                   | `Old`    | 17.1 µs     | 2.67 GiB/s                   |
| 64×64 solid colour                   | `Auto`   | 19.8 µs     | 2.31 GiB/s                   |
| 256×256 deterministic gradient       | `New`    | 356 µs      | 2.06 GiB/s                   |
| 256×256 deterministic gradient       | `Old`    | 295 µs      | 2.48 GiB/s                   |
| 256×256 deterministic gradient       | `Auto`   | 359 µs      | 2.04 GiB/s                   |
| 1024×1024 solid colour (long runs)   | `New`    | 4.99 ms     | 2.35 GiB/s                   |
| 1024×1024 solid colour (long runs)   | `Old`    | 3.95 ms     | 2.97 GiB/s                   |
| 1024×1024 solid colour (long runs)   | `Auto`   | 4.98 ms     | 2.36 GiB/s                   |

Observations:

* `Auto` tracks `New` within noise on every input, as expected — the
  three widths (64, 256, 1024) all sit comfortably inside the
  `8..=32767` new-RLE addressable range, so `Auto` selects `New`.
* `Old` is consistently the fastest variant in absolute time. The
  output it produces is also typically larger (no per-scanline `0x02
  0x02` marker + run-pack), so the fewer-cycles-per-pixel win comes
  with a wire-size penalty — pick `New` (or `Auto`) for compression,
  `Old` only when targeting legacy consumers that don't recognise the
  post-1991 marker.
* All paths sit in the 2.0–3.0 GiB/s range against the raw `f32` input
  buffer, dominated by the `f32 → RGBE` shared-exponent conversion and
  the per-pixel channel-deinterleave into the four single-channel
  staging buffers `encode_scanline` consumes.
* Round 179 closed the round-131 follow-up note about
  `reorient_for_axis_flags`'s unconditional `pixels.to_vec()`:
  `reorient_for_axis_flags` now returns `Cow<'_, [f32]>` and the
  canonical `-Y H +X W` axis (the encoder default, no flip / no
  transpose) is served as `Cow::Borrowed(&image.pixels)` — the
  ~12 MiB alloc/memcpy per 1024×1024 default-axis encode is gone.
  Mirrored / transposed headers still pay the allocation since the
  on-disk layout genuinely differs from the canonical buffer.
  Repeating the round-131 quick bench against the post-r179 encoder
  shows the 1024×1024 solid `new_rle` path moving from a
  median ~4.99 ms (2.35 GiB/s) to a median ~4.70 ms (2.49 GiB/s) on
  the same Apple Silicon laptop, a ~6% improvement that lands
  squarely on the alloc-elimination axis (the rgb_to_rgbe loop +
  the four per-channel staging-buffer fills still dominate the
  remaining wall time).

## License

MIT — see [`LICENSE`](LICENSE).
