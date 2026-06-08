# oxideav-hdr

Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader + writer for the
[oxideav](https://github.com/OxideAV/oxideav-workspace) workspace.

the shared-exponent floating-point image format, originally
described in *Real Pixels* (Graphics Gems II, 1991) and implemented
across the Radiance synthetic-imaging system. The on-disk
representation packs three 8-bit RGB mantissa bytes plus one shared
8-bit biased exponent into 4 bytes per pixel, then RLE-codes each
scanline. The decoder produces packed `f32` RGB triples; the encoder
takes the same shape and emits a complete file with the canonical
`-Y H +X W` axis flags.

Clean-room implementation against the published format documentation
(the published format documentation). No external library source consulted.

## Coverage (round 261)

| Feature                      | Read | Write |
|------------------------------|:----:|:-----:|
| `#?RADIANCE` / `#?RGBE` magic|  Y   |   Y (default `#?RADIANCE`; `encode_hdr_with_full_options(_, _, _, MagicLine::Rgbe)` for the legacy spelling) |
| `KEY=VALUE` header records   |  Y   |   Y   |
| `EXPOSURE` / `GAMMA` / `PIXASPECT` / `SOFTWARE` | Y | Y |
| `VIEW=` renderer view-parameter record | Y | Y |
| Multiple `EXPOSURE` / `COLORCORR` / `PIXASPECT` records stacked multiplicatively | Y | n/a |
| `COLORCORR` (3-float)        |  Y   |   Y   |
| `HdrImage::effective_pixaspect` (header value or reference-manual default `1.0`) | helper | n/a |
| `HdrImage::effective_exposure` (header value or staged-spec default `1.0` per "no `EXPOSURE` ⇒ none applied") | helper | n/a |
| `HdrImage::effective_colorcorr` (header value or staged-spec default `[1.0, 1.0, 1.0]` per "should have unit brightness") | helper | n/a |
| `PRIMARIES` (8-float chromaticity) | Y |   Y   |
| `HdrImage::effective_primaries` (header value or reference-manual default `0.640 0.330 0.290 0.600 0.150 0.060 1/3 1/3` (default origin primaries) with equal-energy white) | helper | n/a |
| All 8 axis-flag combinations |  Y   |  Y (Y-first + X-first transpose) |
| 32-bit_rle_rgbe pixels       |  Y   |   Y   |
| `rgbe_unbiased_exponent([u8; 4]) -> Option<i32>` (returns the spec-§3 `byte - 128` shared exponent, or `None` for the all-zero sentinel pixel — `Some(1)` for the spec-canonical worked example `(128, 64, 32, 129)`) | inspector | n/a |
| `rgbe_is_zero_pixel([u8; 4]) -> bool` (`bool`-returning sentinel inspector keying off `rgbe[3] == 0` per spec §3's "no valid pixel with exponent byte 0" rule — the boolean counterpart to `rgbe_unbiased_exponent` for call sites that don't need the exponent value) | inspector | n/a |
| 32-bit_rle_xyze pixels       |  Y   |   Y (with helpers in `xyz`) |
| New RLE (`0x02 0x02 hi lo`)  |  Y   |   Y   |
| Old RLE (sentinel pixels)    |  Y   |   Y (`RleMode::Old`) |
| Auto-RLE (width heuristic)   |  -   |   Y (`RleMode::Auto`) |
| Uncompressed (flat `4 * W` byte) scanlines | Y (`parse_hdr_with_options(_, FallbackMode::Uncompressed)`) | Y (`RleMode::Uncompressed`) |
| CRLF line endings            |  Y   |   Y (`LineEnding::Crlf`) |
| Decoder resource limits (`HdrLimits`) | Y (default 32 767 × 32 767, ≤ 256 MiB pixel buffer, `parse_hdr_with_limits` / `parse_hdr_with_options_and_limits` for custom) | n/a |
| `HdrImage::apply_exposure`   |  decode helper |  n/a |
| `HdrImage::apply_colorcorr`  |  decode helper |  n/a |
| `HdrImage::recover_original_radiance` (spec-canonical undo of `EXPOSURE=` — divides the buffer by the cumulative factor to reconstruct scene-referred radiance, per the staged spec's "divide file values by the product of all EXPOSURE settings" rule) | decode helper | n/a |
| `HdrImage::recover_original_colorcorr` (per-channel reciprocal of `apply_colorcorr` — reconstructs pre-correction radiance for files that carry `COLORCORR=`) | decode helper | n/a |
| XYZE ↔ RGB (sRGB / Radiance) |  -   | helpers |
| `rgb_to_xyz_matrix_from_primaries` / `xyz_to_rgb_matrix_from_primaries` (derive a linear `RGB ↔ XYZ` matrix from any `Primaries` record's eight CIE xy floats per BT.709 §3 / IEC 61966-2-1 Annex C — works for `P3_D65`, `REC2020`, and arbitrary 8-float `PRIMARIES=` records the named `RgbColorSpace` enum doesn't cover) | n/a | helpers |
| `convert_image_xyz_to_rgb_with_primaries` / `convert_image_rgb_to_xyz_with_primaries` + `_with_effective_primaries` wide-gamut whole-image converters (in-place; pick the chromaticity record explicitly or thread the file's own `PRIMARIES=` via `effective_primaries`; return `bool` and leave the buffer / format tag untouched on degenerate input) | helpers | helpers |
| `Primaries::SRGB` / `RADIANCE` / `P3_D65` / `REC2020` constants | n/a | constants |
| Tone-mapping (Linear / Gamma / Reinhard / ReinhardExtended / ReinhardLuminance / Hable / Drago / ACES) | - | helpers |
| Radiance photometric luminance (`179 * (0.265 R + 0.670 G + 0.065 B)` for RGBE; `179 * Y` for XYZE) | helper (`luminance_lm_per_sr_per_m2`, `HdrImage::luminance_buffer`) | n/a |

Cross-validated against ImageMagick 7's HDR codec (encoder output is
decodable by `magick`, ImageMagick-written `.hdr` files round-trip
through our decoder, XYZE↔RGB matrix tracks ImageMagick's chroma
adaptation within the format's shared-exponent precision).

Round 192 also stages three committed on-disk regression fixtures
under [`tests/fixtures/`](tests/fixtures/) (`gradient_32x16_newrle.hdr`,
`solid_16x8_oldrle.hdr`, `gradient_32x16_crlf_plusY.hdr`), and round
196 adds a fourth (`flat_4x2_uncompressed.hdr`) that pins the third
on-disk scanline flavour from the staged spec: 4 × 2 pixels written
as a flat `4 * width` byte RGBE quad array with no marker and no
sentinels, paired with the new `RleMode::Uncompressed` writer + the
`FallbackMode::Uncompressed` reader option. Between them they exercise
every typed `KEY=VALUE` slot the decoder recognises plus an untyped
extra record, both `\n` and `\r\n` line endings, the canonical
`-Y H +X W` and the non-default `+Y H +X W` axis orders, and all
three pixel-section encodings the staged spec enumerates (new-RLE,
old-RLE, uncompressed). The matching `tests/fixture_decode.rs` integration test
decodes each one, asserts the recovered structure, and re-encodes it
with byte-identity against the committed file — drift in either
direction is caught by a file-level diff rather than a subtle
pixel-comparison regression. The uncompressed fixture's pixel
section is asserted to be exactly `4 * W * H` bytes (no marker, no
sentinels), confirming the encoder honours the requested flavour at
the on-disk byte level. Regenerate after an intentional wire-format
change with `cargo run --example gen_fixtures`.

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

## Fuzzing

Round 202 added a `cargo-fuzz` harness under [`fuzz/`](fuzz/) with three
libFuzzer targets covering the public decode + encode surface end-to-end.
The harness uses the standalone (`default-features = false`) build so it
never links `oxideav-core` — the targets exercise only the
framework-free `parse_hdr` / `encode_hdr` path that downstream
image-library consumers actually call.

| Target       | What it stresses                                                                                 |
|--------------|--------------------------------------------------------------------------------------------------|
| `decode`     | `parse_hdr(arbitrary bytes)` — every code path the decoder can take on hostile input. The new round-202 `HdrLimits` default (max 32 767 × 32 767, ≤ 256 MiB pixel buffer) caps the worst-case allocation so libFuzzer doesn't OOM. |
| `roundtrip`  | Synthesise a fuzz-driven small picture, run `encode_hdr` → `parse_hdr`, assert structure survives end-to-end. Catches encoder/decoder asymmetries. |
| `headers`    | Prepend a valid `#?RADIANCE\n` magic and a minimal `-Y 1 +X 8\n` resolution line so libFuzzer's coverage gradient focuses the corpus on the text `KEY=VALUE` parse (EXPOSURE / COLORCORR / PRIMARIES floats, comment lines, mid-line `=`). |

Run any target with:

```sh
cd fuzz
cargo +nightly fuzz run decode      # or roundtrip / headers
```

The harness is `cargo-fuzz` standard layout — `fuzz/Cargo.toml` declares
its own `[workspace]` block so the umbrella workspace never tries to
build the `nightly`-only libfuzzer dependency.

## License

MIT — see [`LICENSE`](LICENSE).
