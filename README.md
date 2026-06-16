# oxideav-hdr

Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader + writer for the
[oxideav](https://github.com/OxideAV/oxideav-workspace) workspace.

Radiance RGBE is the shared-exponent floating-point image format,
originally described in *Real Pixels* (Graphics Gems II, 1991). The
on-disk representation packs three 8-bit RGB mantissa bytes plus one
shared 8-bit biased exponent into 4 bytes per pixel, then RLE-codes
each scanline. The decoder produces packed `f32` RGB triples; the
encoder takes the same shape and emits a complete file with the
canonical `-Y H +X W` axis flags.

Clean-room implementation against the published format documentation.
No external library source consulted.

## Coverage

| Feature                      | Read | Write |
|------------------------------|:----:|:-----:|
| `#?RADIANCE` / `#?RGBE` magic|  Y   |   Y (default `#?RADIANCE`; `encode_hdr_with_full_options(_, _, _, MagicLine::Rgbe)` for the legacy spelling) |
| `#?<identifier>` general magic line (the staged note's `HDRSTR = "#?"` + caller-supplied identifier — `#?RADIANCE` / `#?RGBE` are just the common spellings; any non-empty token after `#?` is a valid header-id line and is preserved verbatim in `HdrHeader::magic_id`) | Y (parsed into `magic_id`; empty `#?` rejected) | Y (`MagicLine::Custom(String)` emits an arbitrary identifier; `encode_hdr_preserving_magic` reproduces the decoded `magic_id` so a decode→encode round-trip keeps the original `#?…` line instead of rewriting it to `#?RADIANCE`) |
| `KEY=VALUE` header records   |  Y   |   Y   |
| Header program/command lines (the format note's "`#?…` identifier followed by one or more lines giving the programs used to produce the picture, interspersed with variable assignments" — a non-comment header line carrying no `=`, e.g. `rpict -vp 0 0 0 scene.oct`, is kept verbatim in `HdrHeader::commands` and re-emitted right after the magic line, rather than rejecting renderer-produced files) | Y (preserved in read order) | Y (emitted ahead of `FORMAT=`) |
| `FORMAT` declared **at most once** (a second `FORMAT=` record is rejected as invalid per the staged spec's "at most one FORMAT line is allowed", rather than last-wins overwriting an ambiguous pixel-format declaration; the value is trimmed of surrounding whitespace before matching the two valid pixel formats — `FORMAT= 32-bit_rle_rgbe ` parses — consistent with the spec's "value up until the end of line" and every sibling typed field, while interior non-format tokens stay rejected as unsupported) | Y (enforced) | Y (single) |
| `EXPOSURE` / `GAMMA` / `PIXASPECT` / `SOFTWARE` | Y | Y |
| `VIEW=` renderer view-parameter record | Y | Y |
| Multiple `VIEW=` records merged cumulatively (a later `-v<x>` option group overrides the same flag in the accumulated view, genuinely-new flags are appended, the later command prefix wins — per the format note's "cumulative inasmuch as new view options add to or override old ones" rule, not whole-string last-wins) | Y | n/a |
| Multiple `EXPOSURE` / `COLORCORR` / `PIXASPECT` records stacked multiplicatively | Y | n/a |
| `COLORCORR` (3-float)        |  Y   |   Y   |
| `HdrImage::effective_pixaspect` (header value or reference-manual default `1.0`) | helper | n/a |
| `HdrImage::effective_exposure` (header value or staged-spec default `1.0` per "no `EXPOSURE` ⇒ none applied") | helper | n/a |
| `HdrImage::effective_colorcorr` (header value or staged-spec default `[1.0, 1.0, 1.0]` per "should have unit brightness") | helper | n/a |
| `PRIMARIES` (8-float chromaticity) | Y |   Y   |
| `HdrImage::effective_primaries` (header value or reference-manual default `0.640 0.330 0.290 0.600 0.150 0.060 1/3 1/3` (default origin primaries) with equal-energy white) | helper | n/a |
| All 8 axis-flag combinations |  Y   |  Y (Y-first + X-first transpose) |
| `Orientation` enum naming all 8 resolution-string forms (`Standard` = `-Y N +X M`, `FlipX`, `Rotate180`, `FlipY`, `Rotate90Cw`, `Rotate90CwFlipY`, `Rotate90Ccw`, `Rotate90CcwFlipY` per the format note's §2 table) with `from_axis_fields` / `to_axis_fields` (a total mutual inverse over the `(y_sign, x_sign, x_first)` triple), `is_x_first`, `resolution_template`, plus `HdrHeader::orientation` / `set_orientation` to read or set the on-disk scanline layout by geometric name | helper | helper |
| 32-bit_rle_rgbe pixels       |  Y   |   Y   |
| `rgbe_unbiased_exponent([u8; 4]) -> Option<i32>` (returns the spec-§3 `byte - 128` shared exponent, or `None` for the all-zero sentinel pixel — `Some(1)` for the spec-canonical worked example `(128, 64, 32, 129)`) | inspector | n/a |
| `rgbe_is_zero_pixel([u8; 4]) -> bool` (`bool`-returning sentinel inspector keying off `rgbe[3] == 0` per spec §3's "no valid pixel with exponent byte 0" rule — the boolean counterpart to `rgbe_unbiased_exponent` for call sites that don't need the exponent value) | inspector | n/a |
| `rgbe_channel_scale([u8; 4]) -> Option<f32>` (the spec-§3 decode-formula factor `f = ldexp(1.0, byte − (128 + 8))` such that each channel equals `mantissa * f`, or `None` for the all-zero sentinel — `Some(2⁻⁷)` for the spec-canonical worked example `(128, 64, 32, 129)`; completes the quad-inspector trio) | inspector | n/a |
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
| Primaries-aware photometric luminance (the fixed RGBE weights `(0.265, 0.670, 0.065)` are the CIE-Y row of Greg Ward's standard-primaries RGB→XYZ matrix, so a non-standard `PRIMARIES=` record — wide-gamut P3 / Rec. 2020 / custom 8-float — has different luminance weights, namely the Y row of *its* matrix; `rgbe_luminance_coeffs_from_primaries` derives them, `luminance_lm_per_sr_per_m2_with_primaries` and `HdrImage::luminance_buffer_with_effective_primaries` apply them — XYZE ignores primaries since its Y is already CIE Y, degenerate records fall back to the fixed coefficients) | helper | n/a |

An opt-in, env-gated test suite cross-validates encode/decode against
an external Radiance-capable image tool when one is present on `PATH`
(black-box validator only; it skips cleanly when absent).

Committed on-disk regression fixtures live under
[`tests/fixtures/`](tests/fixtures/) (`gradient_32x16_newrle.hdr`,
`solid_16x8_oldrle.hdr`, `gradient_32x16_crlf_plusY.hdr`,
`flat_4x2_uncompressed.hdr`). Between them they exercise every typed
`KEY=VALUE` slot the decoder recognises plus an untyped extra record,
both `\n` and `\r\n` line endings, the canonical `-Y H +X W` and the
non-default `+Y H +X W` axis orders, and all three pixel-section
encodings the spec enumerates (new-RLE, old-RLE, uncompressed). The
matching `tests/fixture_decode.rs` integration test decodes each one,
asserts the recovered structure, and re-encodes it with byte-identity
against the committed file. Regenerate after an intentional
wire-format change with `cargo run --example gen_fixtures`.

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

The full Criterion suite, measured numbers and the ranked hotspot table
live in [`BENCHMARKS.md`](BENCHMARKS.md). Three bench targets cover the
crate's hot surface end-to-end:

* [`benches/encode.rs`](benches/encode.rs) — `encode_hdr_with_rle` in
  all four modes (`New`, `Old`, `Auto`, `Uncompressed`) on three
  inline-synthesised inputs.
* [`benches/decode.rs`](benches/decode.rs) — `parse_hdr` on the same
  inputs pre-encoded in each of the three on-disk scanline flavours
  (new-RLE / old-RLE / uncompressed).
* [`benches/pixels.rs`](benches/pixels.rs) — XYZE↔RGB whole-image
  conversion (both working spaces) and all 8 tone-mapping operators.

Headlines (Apple Silicon laptop): both codec directions run at
2.1–3.5 GiB/s of float-side pixels in every flavour, dominated by the
per-pixel shared-exponent conversion rather than wire handling; XYZ
conversion is memory-bound at ~0.34 ns/px; tone-mapping operators
range from 3.7 ns/px (`Linear`) to 40.6 ns/px (`Drago`). The
`reorient_for_axis_flags` `Cow` fast path avoids any alloc/memcpy on
the canonical `-Y H +X W` axis.

## Fuzzing

A `cargo-fuzz` harness under [`fuzz/`](fuzz/) ships five libFuzzer
targets covering the public decode + encode + colour-conversion surface
end-to-end. The
harness uses the standalone (`default-features = false`) build so it
never links `oxideav-core` — the targets exercise only the
framework-free `parse_hdr` / `encode_hdr` path that downstream
image-library consumers actually call.

| Target       | What it stresses                                                                                 |
|--------------|--------------------------------------------------------------------------------------------------|
| `decode`     | `parse_hdr(arbitrary bytes)` — every code path the decoder can take on hostile input. The `HdrLimits` default (max 32 767 × 32 767, ≤ 256 MiB pixel buffer) caps the worst-case allocation so libFuzzer doesn't OOM. |
| `roundtrip`  | Synthesise a fuzz-driven small picture, run `encode_hdr` → `parse_hdr`, assert structure survives end-to-end. Catches encoder/decoder asymmetries. |
| `headers`    | Prepend a valid `#?RADIANCE\n` magic and a minimal `-Y 1 +X 8\n` resolution line so libFuzzer's coverage gradient focuses the corpus on the text `KEY=VALUE` parse (EXPOSURE / COLORCORR / PRIMARIES floats, comment lines, mid-line `=`). |
| `pixels`     | Wrap a *fuzz-controlled pixel section* in a valid container envelope (magic + blank line + a fuzz-chosen `-Y H +X W` resolution line with small, bounded dimensions), then decode it under **both** `FallbackMode` branches. Forces the corpus straight into the new-RLE / old-RLE / uncompressed inner loops. |
| `colorconv`  | Drive the **float-domain colour pipeline** — XYZE↔RGB conversion (named-space, arbitrary-`PRIMARIES`, and `_with_effective_` forms), `rgb_to_xyz_matrix_from_primaries` / `xyz_to_rgb_matrix_from_primaries` (the `3×3` inversion), `luminance_lm_per_sr_per_m2`, and all eight tone-mapping operators — on raw fuzz bytes reinterpreted **verbatim** as `f32`. The four byte-surface targets only feed this code floats laundered through the RGBE quantiser (finite, non-negative, bounded), so NaN / ±inf / negative / subnormal samples and degenerate chromaticity records reach the matrix inversion and per-operator transcendentals only here. Asserts every call returns without panicking and that buffer-length / `Rgb24` byte-count invariants hold. |

Run any target with:

```sh
cd fuzz
cargo +nightly fuzz run decode      # or roundtrip / headers / pixels / colorconv
```

The harness is `cargo-fuzz` standard layout — `fuzz/Cargo.toml` declares
its own `[workspace]` block so the umbrella workspace never tries to
build the `nightly`-only libfuzzer dependency.

## License

MIT — see [`LICENSE`](LICENSE).
