# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Round 214 (spec-compliance — `PRIMARIES` reference-manual default):
  new `HdrImage::effective_primaries()` helper, mirroring the round-208
  `effective_pixaspect` convenience. Per the staged spec
  (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 PRIMARIES row), when
  a Radiance picture omits the `PRIMARIES=` record, consumers are
  expected to assume Greg Ward's original Radiance primaries with an
  equal-energy reference white (`0.640 0.330 0.290 0.600 0.150 0.060
  1/3 1/3` for R, G, B, W). Through round 213 the decoder left
  `header.primaries = None` for files without the record and consumers
  had to know to substitute `Primaries::RADIANCE` themselves; the new
  helper does the substitution in one call (returning the literal
  `Primaries::RADIANCE` constant when the slot is `None`) without
  perturbing `HdrHeader::primaries`, so callers that need to
  distinguish "file declared default-equal primaries explicitly" from
  "no record was present" can still match on the typed slot directly.
  Three new image-module tests pin the default-when-absent branch
  (including a numeric check against the staged spec's literal value
  table), the header-value-when-set branch (sRGB), and the
  `PRIMARIES=` record round-trip through `Primaries::to_record_string`
  / `Primaries::from_record_str`. The existing fixtures and the
  round-192 regression tests pass unchanged — the helper is purely
  additive.

- Round 208 (spec-compliance — cumulative `PIXASPECT`): closed the
  last "multiple records stack" gap from the Radiance reference
  manual. The reference manual lists `PIXASPECT=` alongside
  `EXPOSURE=` and `COLORCORR=` as a *cumulative* (multiplicative)
  record — when several appear, the effective pixel aspect ratio is
  their product, and the default when no record is present is `1.0`
  (square pixels). Through round 207 the decoder kept only the last
  `PIXASPECT=` value seen (overwrite semantics); round 208 folds the
  values into a running product, matching the multiple-`EXPOSURE` /
  multiple-`COLORCORR` paths added in earlier rounds. The new
  `HdrImage::effective_pixaspect` helper returns the resulting `f32`
  with the reference-manual `1.0` default substituted when no record
  was present, so consumers can avoid the `Option::unwrap_or`
  ceremony at every call site. Two new decoder unit tests pin the
  cumulative stack (three records `2.0 * 0.5 * 1.25 = 1.25`) and the
  single-record no-regression path, and two new image-module tests
  cover the helper's default-vs-set branches. The single-record happy
  path is bit-identical to round 207 (the running product is
  initialised from the first value, so one record decodes to its
  literal value); existing fixtures and the round-192 regression
  test pass unchanged.

- Round 202 (depth mode — hardening + fuzz harness): new
  `HdrLimits` decoder resource-limit type plus the matching
  `parse_hdr_with_limits` / `parse_hdr_with_options_and_limits` public
  entry points, and a `cargo-fuzz` harness under `fuzz/` with three
  libFuzzer targets (`decode`, `roundtrip`, `headers`). The default
  `HdrLimits` (max 32 767 × 32 767, ≤ 256 MiB pixel buffer) match the
  new-RLE marker's addressability ceiling and gate the
  `width × height × 12 byte` allocation in `decode_pixel_rows` so an
  attacker-crafted resolution line like `-Y 2_000_000_000 +X 2_000_000_000`
  is rejected at parse time with the new
  `HdrError::TooLarge` variant — round 1..201 the same input would have
  attempted an unbounded allocation (and either OOM'd the host or
  wrapped the `usize` multiplication on 64-bit hosts). `parse_hdr` keeps
  its existing signature and threads through the default limits, so the
  round 1..201 happy path stays bit-identical; existing callers see no
  observable change. Six new unit tests pin the limit-enforcement
  contract (per-axis caps, pixel-byte cap, custom relax via
  `parse_hdr_with_options_and_limits`, `HdrLimits::unbounded` opt-out)
  and the fuzz crate is wired with `default-features = false` so it
  builds against the framework-free standalone surface. Closes the
  hostile-input attack surface the existing test corpus didn't exercise.

- Round 196: closed the read/write spec gap for the third on-disk
  scanline flavour the staged Radiance spec enumerates ("Uncompressed
  — each scanline is M pixels × 4 bytes"). The encoder gains
  `RleMode::Uncompressed`, which emits a flat `4 * width` byte RGBE
  quad array per scanline with no `0x02 0x02 hi lo` marker and no
  `(1, 1, 1, *)` sentinels. The decoder gains
  `FallbackMode::Uncompressed` and a matching `parse_hdr_with_options`
  entry point: when the new-RLE marker probe fails, the fallback is
  configurable between the historical `OldRle` (sentinel-aware,
  default of `parse_hdr` for backwards compatibility) and the new
  `Uncompressed` (every quad is a literal RGBE pixel — the spec's
  documented "read the scanline flat" fallback). The Uncompressed
  fallback is the right choice for any file whose pixel section
  contains a legitimate `(1, 1, 1, *)` quad — the OldRle fallback
  would misinterpret it as a run sentinel. A fourth committed on-disk
  fixture (`tests/fixtures/flat_4x2_uncompressed.hdr`, 77 bytes)
  pins the encoder + decoder round-trip and asserts the pixel section
  is exactly `4 * W * H` bytes (no marker, no sentinels). Six new
  unit tests and one end-to-end public-API round-trip cover the new
  surface; the existing `parse_hdr` / `encode_hdr` API is unchanged
  and the round-1..195 default behaviour is preserved bit-for-bit.

### Changed

- Round 196 Hat-3 audit pass on `src/rle.rs` + `src/encoder.rs`:
  rephrased two doc-comment references to "Greg Ward's reference
  writer" / "Greg Ward's adaptive new-RLE" to attribute the
  documented behaviour to the staged spec at
  `docs/image/hdr/radiance-hdr-rgbe-format.md` instead of the
  reference C implementation. All other Greg Ward citations refer to
  the format-spec author or the published format documents and are
  preserved unchanged. No code derivation, no source consultation —
  the rephrasing is documentation hygiene, not a correctness fix.

## [0.0.3](https://github.com/OxideAV/oxideav-hdr/compare/v0.0.2...v0.0.3) - 2026-05-30

### Other

- round 192 — on-disk .hdr regression-anchor fixtures + decode/re-encode test
- round 189 — Radiance photometric reduction helpers
- round 179 — zero-copy fast path on canonical axis
- round 131 — Criterion encoder fast-path bench (encode_hdr new/old/auto RLE)
- round 5: VIEW header slot + CRLF write + apply_exposure/colorcorr helpers
- round 4: X-first axis flags + EXPOSURE/COLORCORR stacking + P3-D65/Rec2020 primaries + ReinhardLuminance tonemap
- round 3: COLORCORR + PRIMARIES header fields, Hable / Drago / Reinhard-extended tonemaps, RleMode::Auto, y_sign/x_sign encoder honour
- separate round 2 entries from 0.0.2 section
- Round 2: old-RLE encoder + XYZE↔RGB + tone-mapping helpers

### Added

- Round 192: staged on-disk `.hdr` regression-anchor fixtures under
  `crates/oxideav-hdr/tests/fixtures/` (`gradient_32x16_newrle.hdr`,
  `solid_16x8_oldrle.hdr`, `gradient_32x16_crlf_plusY.hdr`) plus a
  matching `tests/fixture_decode.rs` integration test that decodes
  each one, asserts the recovered dimensions / axis flags / typed
  header slots / pixel-magnitude extremes, then re-encodes the
  decoded image with the same options and asserts byte-identity
  against the committed file. The combined chain pins both the
  decoder's parse logic and the encoder's emit logic to a single
  committed reference per RLE flavour. `examples/gen_fixtures.rs`
  regenerates the bytes from the same deterministic synthetic inputs
  whenever an intentional wire-format change is made (`cargo run
  --example gen_fixtures`). The three fixtures between them exercise
  every typed `KEY=VALUE` slot the decoder recognises (FORMAT /
  EXPOSURE / GAMMA / SOFTWARE / VIEW / PIXASPECT / COLORCORR /
  PRIMARIES) plus an untyped `OXIDEAV=` extra record, both `\n` and
  `\r\n` line endings, the canonical `-Y H +X W` axis order and the
  non-default `+Y H +X W` (bottom-up) order, and both the new-RLE
  and old-RLE pixel-section encodings. Closes the #1057 staged-
  fixture follow-up. Total committed fixture footprint: ~4.6 KiB
  across three files.

- Round 189: photometric-luminance helper
  `oxideav_hdr::luminance_lm_per_sr_per_m2(pixel, format)` plus the
  `HdrImage::luminance_buffer()` whole-image variant. Implements the
  Radiance reference manual's "Physical interpretation" reduction —
  `179 * (0.265*R + 0.670*G + 0.065*B)` for `FORMAT=32-bit_rle_rgbe`
  and `179 * Y` for `FORMAT=32-bit_rle_xyze` — so callers can convert
  decoded scene-referred radiance into lumens / steradian / m² without
  re-deriving the coefficients. Re-exports the underlying
  `WHTEFFICACY` (= 179.0 lm/W) and `RGBE_BRIGHT_COEFFS` constants for
  consumers that want to apply the formula by hand. Six new unit
  tests pin the (R, G, B) weights to the documented values, exercise
  the XYZE pass-through branch, and lock the `WHTEFFICACY` constant
  in place against accidental edit. The reduction matches Radiance's
  `luminance(col)` macro in `src/common/color.h`.

### Changed

- Round 179: `encode_hdr_with_options` no longer allocates a fresh
  `Vec<f32>` for the canonical `-Y H +X W` axis (the encoder default).
  `reorient_for_axis_flags` returns `Cow<'_, [f32]>` and the fast path
  now borrows `&image.pixels` directly — the ~12 MiB heap alloc +
  memcpy per 1024×1024 default-axis encode that the round-131 PERF
  note flagged is gone. Mirrored / transposed axes still pay the
  allocation since the on-disk layout genuinely differs from the
  canonical buffer. Two new unit tests
  (`reorient_canonical_axis_borrows_input_buffer`,
  `reorient_flipped_axis_returns_owned_reordering`) lock the
  borrow-vs-own contract in place by pointer-identity check + by
  exercising the slow path's roundtrip. Re-running the round-131
  Criterion bench against the new fast path shows the
  1024×1024 solid `new_rle` median moving 4.99 ms → 4.70 ms (a ~6%
  throughput improvement against the raw `f32` input buffer); the
  remaining cycles are dominated by `rgb_to_rgbe` plus the four
  per-channel staging-buffer fills inside `write_pixel_rows`.

### Added

- Round 131 (depth mode): Criterion micro-benchmark
  `benches/encode.rs` driving the encoder fast path through all three
  `RleMode` variants (`New`, `Old`, `Auto`) on three representative
  inline-synthesised inputs (64×64 solid, 256×256 gradient, 1024×1024
  solid). Headline numbers captured in the new README "Performance"
  section. `criterion = "0.5"` added as a `[dev-dependencies]` entry
  only (no runtime closure impact). No encoder algorithmic changes
  this round; a `// PERF:` note on `reorient_for_axis_flags`'s
  unconditional `pixels.to_vec()` (≈ 12 MiB alloc/memcpy per
  1024×1024 default-axis encode) flags the obvious follow-up.

- Round 5: typed `HdrHeader::view` slot for the Radiance `VIEW=` record
  (the renderer's view-parameter string — `-vp`, `-vd`, `-vu`, `-vh`,
  `-vv`, … flags concatenated). Previously fell through to
  `HdrHeader::other`; now decoded into the typed slot and re-emitted by
  the encoder. Last record wins when stacked across rerender passes.
- Round 5: `LineEnding::{Lf,Crlf}` plus `encode_hdr_with_options` —
  full encoder parity with the existing read-side CRLF support. Magic
  line, `KEY=VALUE` records, blank-line terminator and resolution line
  honour the chosen line ending; the binary pixel payload that follows
  is untouched. Default `encode_hdr` / `encode_hdr_with_rle` stays on
  bare `\n` to match every shipped fixture in the Radiance reference
  distribution.
- Round 5: `HdrImage::apply_exposure` / `HdrImage::apply_colorcorr`
  helpers — fold the parsed multiplicative `EXPOSURE=` / `COLORCORR=`
  factors into the float pixel buffer in place and clear the header
  slot. The decoder still returns the raw shared-exponent samples so
  callers that want untouched radiance values keep them; callers that
  want the post-exposure / post-correction values now have a one-liner.
- Round 4: encoder fully honours `HdrHeader::x_first` — the four
  X-first axis-flag combinations (`±X W ±Y H`) now produce on-disk
  files with the requested resolution-line ordering, transposing the
  canonical top-down `(y, x)` buffer on the way out so each on-disk
  scanline holds one column's worth of Y samples. The decoder also
  gained X-first scanline-count + transpose support; a previous
  off-by-one in the loop counts (only Y-first was ever exercised end
  to end) is fixed. All 8 axis-flag combinations now round-trip
  exhaustively via the public API (covered by
  `encoder_round_trips_all_eight_axis_orderings`).
- Round 4: multiple `EXPOSURE=` records in the same file are now
  stacked multiplicatively per the Radiance reference manual
  (`exposure = ∏ values`). Same rule applied to multiple
  `COLORCORR=` records (element-wise product across occurrences). The
  single-record case is preserved; the stacking only changes behaviour
  when a file has more than one record of the same kind.
- Round 4: two new named `Primaries` constants — `Primaries::P3_D65`
  (Display P3, SMPTE RP 431-2 primaries with D65 white per the
  Display P3 specification) and `Primaries::REC2020` (ITU-R BT.2020-2
  Table 4 ultra-wide-gamut primaries with D65 white). Both round-trip
  losslessly via `to_record_string` / `from_record_str`.
- Round 4: `ToneMap::ReinhardLuminance` — Reinhard 2002 applied to
  per-pixel luminance (BT.709 coefficients) with the chroma carried
  through proportionally. Preserves colour saturation across the
  tone-mapped range where the per-channel variant desaturates
  highlights.
- Round 3: typed `HdrHeader::colorcorr` slot for the Radiance
  `COLORCORR=R G B` per-channel correction record. Decoder parses
  three floats, encoder writes them, round-trip preserves the value.
- Round 3: typed `HdrHeader::primaries` slot backed by a new
  `Primaries` struct holding the eight CIE xy chromaticity floats
  `Rx Ry Gx Gy Bx By Wx Wy` that the Radiance `PRIMARIES=` record
  carries. `Primaries::SRGB` and `Primaries::RADIANCE` constants
  match the IEC 61966-2-1 Annex C and Greg Ward equal-energy
  primaries respectively.
- Round 3: encoder honours `HdrHeader::y_sign` and `HdrHeader::x_sign`
  for the four Y-first axis-flag orderings (`-Y H +X W`,
  `+Y H +X W`, `-Y H -X W`, `+Y H -X W`). The four X-first
  orderings are canonicalised back to Y-first on write — the produced
  file is still valid but loses that single bit of header
  information across the round-trip. Doc comment in
  `encode_hdr_with_rle` spells this out.
- Round 3: `RleMode::Auto` — encoder picks `RleMode::New` for widths
  in the new-RLE range `8..=32767` and falls back to `RleMode::Old`
  for narrower or wider images. Callers that don't want to think
  about the marker's addressable range can pass `Auto` instead of
  juggling explicit `New` / `Old`.
- Round 3: three new tone-mapping operators in
  `oxideav_hdr::tonemap`:
  - `ReinhardExtended` — Reinhard's modified operator
    `(v * (1 + v/W²)) / (1 + v)` with an explicit `white_point`.
    Lets very-bright samples actually reach display white (per
    Reinhard et al. 2002 §3.1) where the unmodified Reinhard
    asymptotes from below.
  - `Hable` — John Hable's "Uncharted 2" filmic curve
    (GDC 2010 derivation): five-knot rational function with a
    `linear_white` normalisation. Designed for game-style filmic
    response with crisp shadows and rolled-off highlights.
  - `Drago` — Drago / Myszkowski / Annen / Chiba EUROGRAPHICS 2003
    adaptive logarithmic operator with a `scene_max` and `bias`
    parameter. Maps wide-range scenes perceptually uniformly across
    orders of magnitude.
- Round 3: tests cover all three new operators plus the new
  `ReinhardExtended` white-point handling, the new axis-flag
  honour, and the `RleMode::Auto` heuristic.

### Round 2 additions (still in `[Unreleased]` window)

- Round 2: old-RLE encoder (`encode_scanline_old_rle`) — the
  pre-1991 per-pixel literal + chained `(1, 1, 1, n)` sentinel-run
  format. Exposed via `encode_hdr_with_rle(image, RleMode::Old)` for
  callers targeting legacy viewers or images outside the new-RLE
  width range.
- Round 2: XYZE ↔ RGB conversion helpers in `oxideav_hdr::xyz`.
  Forward and inverse matrices for sRGB / Rec. 709 (D65 white) and
  Greg Ward's original Radiance primaries (E white).
  `convert_image_xyz_to_rgb` / `convert_image_rgb_to_xyz` mutate an
  `HdrImage` in place and flip the header's FORMAT tag accordingly.
- Round 2: tone-mapping helpers in `oxideav_hdr::tonemap`: `Linear`,
  `Gamma`, Reinhard 2002, and Krzysztof Narkowicz's polynomial ACES
  fit. All apply the sRGB OETF on the way out (except `Linear`) and
  quantise to packed 8-bit `Rgb24`. Convenience for downstream
  consumers that need a display-ready preview from the float buffer.
- Round 2: cross-validation against ImageMagick 7's HDR codec
  (`tests/imagemagick_xvalidate.rs`): our encoder output decodes,
  ImageMagick-written files parse, XYZE↔RGB conversion round-trips
  through ImageMagick within the format's precision. Tests skip
  automatically if `magick` isn't on `PATH`.

## [0.0.2](https://github.com/OxideAV/oxideav-hdr/compare/v0.0.1...v0.0.2) - 2026-05-05

### Other

- clippy needless_range_loop fix in solid_colour_roundtrips_via_repeat_run

## [0.0.1] - Initial release

### Added

- Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader + writer covering
  the standard new-RLE pixel encoding plus the older pre-1991
  sentinel-pixel old-RLE format on the read path.
- Header parser handles `#?RADIANCE` and `#?RGBE` magic lines, an
  arbitrary list of `KEY=VALUE` metadata records (FORMAT, EXPOSURE,
  GAMMA, SOFTWARE, COLORCORR, PIXASPECT, …), the empty-line
  terminator, and the resolution line (`-Y h +X w` and the seven other
  axis-flag combinations).
- 32-bit RGBE pixel encoding: shared exponent biased by 128, mantissa
  channels in `0..=255`. Decode produces `f32` per channel, encode
  builds the shared-exponent representation from `f32` input.
- New-RLE encoder writes per-scanline `0x02 0x02 hi lo` headers with
  the four channels RLE-coded independently. Decoder accepts both
  RLE styles and degenerate scanlines (raw 4-byte pixels with no run
  marker).
- Standalone-friendly: `oxideav-core` is optional behind the default-on
  `registry` cargo feature. Image-library consumers can depend on
  `oxideav-hdr` with `default-features = false` for a framework-free
  build.
