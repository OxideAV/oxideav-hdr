# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Other

- round 344 (depth — bit-exact round-trip surface): add `HdrImage::from_rgbe_quads` / `to_rgbe_quads`, a byte-level view of the picture alongside the lossy float-in/float-out API. `to_rgbe_quads` re-derives the exact on-disk `[R, G, B, E]` quad per pixel (the same quads `encode_hdr` commits to the wire); `from_rgbe_quads` builds a float image by decoding a quad slice. Because the shared-exponent quantiser is idempotent on the *normalised* quad subset the encoder produces (dominant mantissa `≥ 128`, decoded magnitude above the `1e-32` black floor — the property `frexp` guarantees), a picture built from such quads re-encodes byte-for-byte; the idempotence invariant and its sub-`1e-32`-floor boundary are pinned by new `src/rgbe.rs` tests and documented on `rgb_to_rgbe`. New `tests/rgbe_roundtrip_matrix.rs` proves the resulting bit-exact contract end-to-end across the cross-product of resolution variants (square / wide / tall / single-row / single-column / min-new-RLE-width / just-under) × all eight resolution-string orientations × every encoder RLE flavour (New / Old / Auto / Uncompressed, each decoded with its matching `FallbackMode`), for both RGBE and XYZE pictures, under LF and CRLF line endings, and alongside the full typed-header round-trip (EXPOSURE / GAMMA / PIXASPECT / COLORCORR / PRIMARIES / SOFTWARE / VIEW). The quad streams come from a small deterministic in-tree LCG — no external property-test crate, keeping the clean-room + dependency discipline. The matrix exercises ≥200 round-trip cases plus the XYZE / CRLF / typed-header arms
- round 332 (depth — profile micro-opt): bulk-fill the RLE repeat-run decode in `src/rle.rs`. Both `decode_new_rle` (the post-1991 adaptive scheme's per-channel repeat code) and `decode_old_rle` (the pre-1991 chained-sentinel run) previously expanded a repeat run with a byte-at-a-time `push` loop — `for _ in 0..run { ch.push(value) }` (one channel) / four parallel `push` calls per repeated pixel (old-RLE). Each `push` re-checked the `Vec`'s spare capacity even though the channel buffers are pre-reserved with `Vec::with_capacity(width)` at scanline start and the decoder already enforces `written + run <= width` before filling, so no reallocation can occur. Replacing the loops with a single `Vec::resize(len + run, value)` lets the compiler lower the fill to a `memset` over the freshly-grown tail. Measured on the existing `decode` Criterion bench (Apple Silicon, release, single-thread): the run-heavy `1024×1024 solid` decode dropped from 3.78 ms → 2.35 ms (new-RLE, ~38 %) and 3.79 ms → 2.30 ms (old-RLE, ~39 %); the `256×256 gradient` new-RLE decode from 211 µs → 169 µs (~20 %). The literal-only `uncompressed` path (no repeat runs) is unchanged at ~3.99 ms, confirming the gain is isolated to the run-expansion fill. Decode output is bit-identical — only the fill mechanism changed; the full round-trip suite plus two new targeted tests (`new_rle_long_repeat_run_bulk_fill`, `old_rle_long_repeat_run_bulk_fill`, each pinning the grown channel length and repeated values for a 500–600-pixel run that spans the bulk-fill path) cover it. `BENCHMARKS.md` throughput rows and the rank-11 hotspot entry are updated with the new numbers
- round 327: add `HdrImage::square_pixel_dimensions` + `HdrImage::display_aspect_ratio` — PIXASPECT-corrected display geometry. The staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 PIXASPECT row) defines `PIXASPECT=` as the *pixel* aspect ratio "pixel height / pixel width" and warns it is explicitly **not** the image aspect ratio: a factor `p` means each stored pixel is `p` times as tall as wide, so a consumer that draws the raw `width × height` sample grid on a square-pixel display squashes the picture vertically by `p`. `square_pixel_dimensions` returns the float `(width, height·p)` shape a viewer should present (width unchanged, height stretched by the cumulative `PIXASPECT` product the decoder folds into `HdrHeader::pixaspect`, via `effective_pixaspect`), and `display_aspect_ratio` returns the displayed `width/(height·p)` width:height ratio — which differs from the naive sample-grid ratio `width/height` exactly when pixels are non-square (e.g. a square 512×512 grid stored with `PIXASPECT=2` displays at 0.5). Both fold the same permissive guards the `recover_*` helpers use: a degenerate cumulative factor (`0.0`/non-finite/negative) is treated as the `1.0` identity and a zero-height picture yields a `1.0` ratio, so a malformed header can never produce a `0`/non-finite display size. Non-mutating — the integer `width`/`height` sample-grid dimensions and the header are untouched; only the proportions a viewer presents change. Eight unit tests cover the square-pixel identity, height stretch (`p>1`) and compression (`p<1`), the display-ratio-vs-sample-grid divergence, the cumulative-product fold, the degenerate-factor identity fallback, and the zero-height guard
- round 323: add `HdrImage::scene_referred_luminance_buffer` — a non-mutating per-pixel *physical* photometric-luminance buffer (lumens/sr/m²) computed from the recovered scene-referred radiance rather than the stored float samples. It composes two rules from the staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`): §1's "EXPOSURE / COLORCORR are already applied to the pixels; to recover original radiances (watts/sr/m²) divide file values by the product of all EXPOSURE settings" (with the complementary per-primary COLORCORR division), then §"Physical interpretation"'s `179*(0.265R+0.670G+0.065B)` (RGBE) / `179*Y` (XYZE) projection. Where the existing `luminance_buffer` applies the luminance formula to the stored samples verbatim (file-referred), this method first divides out the cumulative `EXPOSURE` product and per-channel `COLORCORR` triple the writer baked in, so the result is the genuine physical quantity for files carrying those records; the two agree exactly when neither record is present or both are the identity. Degenerate cumulative factors (`EXPOSURE` of `0.0`/non-finite, any `COLORCORR` component `0.0`/non-finite) are treated as "no recovery applied" for that record — the same permissive handling `recover_original_radiance` / `recover_original_colorcorr` use — so a malformed header can never turn the luminance buffer into NaN / ∞. Non-mutating: pixels and header are untouched (no in-place clear like the `recover_*` helpers). Seven unit tests cover identity-without-records agreement with `luminance_buffer`, exposure-only and colorcorr-only recovery, the combined composition, the XYZE `179*Y` branch with recovery, and the degenerate-factor identity fallback
- round 319 (depth — fuzz): add a fifth `cargo-fuzz` target `colorconv` driving the float-domain colour pipeline (XYZE↔RGB conversion, primaries-matrix derivation, photometric luminance, and the eight tone-mapping operators) on *verbatim* fuzz-byte `f32` samples. The four existing byte-surface targets only ever feed the conversion + tone-map code floats laundered through the RGBE shared-exponent quantiser (always finite, non-negative, bounded), so the `3×3` matrix inversion and the per-operator transcendentals were unreachable with NaN / ±inf / negative / subnormal samples and degenerate `PRIMARIES` records. The new target reinterprets raw fuzz bytes as `f32` pixel + chromaticity values to reach those numeric paths directly, asserting each call returns without panicking / overflowing and that buffer-length + `Rgb24` byte-count invariants hold (the quantiser's NaN/inf→`0..=255` clamp is the property under test)
- round 316: preserve header program/command lines per the format note ("the `#?…` identifier followed by one or more lines giving the programs used to produce the picture, interspersed with variable assignments"); a non-comment header line without `=` is now kept verbatim in `HdrHeader::commands` and re-emitted right after the magic line on encode, instead of rejecting the whole file with "header line without '='" — renderer-produced `.hdr` files routinely carry such command lines
- round 313: tolerate surrounding whitespace in the `FORMAT=` value (`.trim()` before matching the two valid pixel formats, matching the lenient whitespace handling every sibling typed header field already applies); interior non-format tokens still rejected as unsupported

## [0.0.4](https://github.com/OxideAV/oxideav-hdr/compare/v0.0.3...v0.0.4) - 2026-06-15

### Other

- round 310: cumulative VIEW= merge per format-note header-variable rule
- named Orientation enum for the 8 resolution-string forms (§2 table)
- add `pixels` target driving the RLE scanline inner loops (r299)
- accept general #?<identifier> magic line + preserve it round-trip
- round 285 depth — full Criterion suite (decode all 3 scanline flavours, encode +Uncompressed, XYZE<->RGB, 8 tonemap ops) + ranked hotspot table in BENCHMARKS.md
- round 275 — reject duplicate FORMAT header record per spec §1
- round 269 — rgbe_channel_scale shared decode-factor inspector
- round 261 — rgbe_is_zero_pixel bool sentinel inspector
- round 257 — rgbe_unbiased_exponent inspector
- round 252 — effective_exposure / effective_colorcorr inspectors
- drop release-plz.toml — use release-plz defaults across the workspace
- round 248 — MagicLine option for the legacy '#?RGBE' identifier
- complete neutralisation of vendor possessive references in README
- replace named-vendor enumeration in README with generic provenance
- round 231 — wide-gamut image-level XYZE↔RGB converters
- round 226 — chromaticity-derived RGB ↔ XYZ matrices from Primaries
- round 220 — spec-canonical original-radiance / colorcorr recovery
- round 214 — PRIMARIES reference-manual default helper
- round 208 — cumulative PIXASPECT + effective_pixaspect helper
- round 202 — HdrLimits resource guard + cargo-fuzz harness

### Changed

- Round 310 (spec-conformance — cumulative `VIEW=` merge): the header
  parser now folds multiple `VIEW=` records together per the format
  note's §1 header-variable table — "Multiple assignments are cumulative
  inasmuch as new view options add to or override old ones" — instead of
  the previous whole-string last-wins overwrite. A Radiance view string
  is an optional leading command/program token run (e.g. `rvu` / `rpict`)
  followed by `-v<x>` option groups; the new merge is structural and
  needs no per-flag argument-count table (the format note doesn't publish
  one): each `-v<x>` group present in a later record overrides the same
  flag in the accumulated view ("override old ones"), a flag only in the
  later record is appended in first-seen order ("add to"), a flag only in
  the accumulator is preserved, and the later record's leading command
  prefix replaces the earlier one (it describes the present picture). The
  merge rebuilds the value as a single space-separated string so it still
  round-trips verbatim through the existing `VIEW=` writer. Through round
  309 a second `VIEW=` record silently dropped every option the earlier
  record carried — e.g. a re-render pass that only re-stated `-vp`
  erased the original `-vd` / `-vu` / `-vh` view geometry. The
  single-record happy path is unchanged (the first record seeds the
  accumulator with its literal value). Five new unit tests pin the
  override-in-place case, the add-new-option case, the combined
  override-and-add case (earlier-only option survives, first-seen order
  preserved), the single-record pass-through, and a three-record
  left-to-right fold; the stale `last_view_record_wins_when_stacked`
  test that encoded the old whole-string-overwrite premise is rewritten
  in place. The encoder, the `VIEW=` round-trip test, and the standalone
  (`default-features = false`) build are unchanged.

### Added

- Round 305: a named `Orientation` enum capturing all eight legal
  Radiance resolution-string forms from the format note's §2 table
  (`Standard` = `-Y N +X M`, `FlipX`, `Rotate180`, `FlipY`,
  `Rotate90Cw`, `Rotate90CwFlipY`, `Rotate90Ccw`, `Rotate90CcwFlipY`).
  `Orientation::from_axis_fields` / `to_axis_fields` convert losslessly
  to/from the `HdrHeader`'s low-level `(y_sign, x_sign, x_first)` triple
  (proven a total mutual inverse over all `2 × 2 × 2` combinations);
  `is_x_first` and `resolution_template` expose the X-first flag and the
  printf-style template string. `HdrHeader::orientation` /
  `set_orientation` let callers read or set the on-disk scanline layout
  by geometric name rather than by re-deriving it from the raw flags;
  the encoder's existing axis-flag machinery drives the actual wire
  format, so a canonical top-down buffer round-trips through any of the
  eight orientations back to the same `(y, x)` layout (covered by a new
  asymmetric 8×4 end-to-end encoder test plus six header-level unit
  tests). `Orientation` is re-exported at the crate root.

- Round 299 (depth — fuzzing): a fourth `cargo-fuzz` target `pixels`
  under `fuzz/fuzz_targets/pixels.rs`. It wraps a *fuzz-controlled pixel
  section* in a valid container envelope (magic + blank-line terminator
  + a fuzz-chosen `-Y H +X W` resolution line with small, bounded
  dimensions derived from two fuzz bytes), then decodes the result under
  **both** `FallbackMode` branches (`OldRle` + `Uncompressed`). This
  drives the corpus straight into the new-RLE / old-RLE / uncompressed
  scanline inner loops — the run-code grammar the existing `decode`
  target reaches only by chance (it must first synthesise the whole
  magic + blank-line + resolution-line prefix from random bytes) and the
  `roundtrip` target never reaches at all (it only ever decodes the
  encoder's own well-formed output). A 3.1 M-run session (91 s on Apple
  Silicon, default `HdrLimits`, ASan) surfaced no panics, integer
  overflows, out-of-bounds reads, or over-allocations; no `src/` changes
  were required. The libFuzzer dictionary recovered the `0x02 0x02`
  new-RLE marker and `(1,1,1,*)` old-RLE sentinel shapes, confirming the
  inner loops are exercised. Standalone `default-features = false` build
  like the other three targets, so it never links `oxideav-core`.

- Round 292 (general `#?` magic line): the decoder now accepts the full
  class of header-id lines the staged format note documents — the magic
  bytes `#?` (`HDRSTR`) followed by any non-empty caller-supplied
  identifier (`newheader(s)` emits `#?` then `s`), not only the two
  canonical `#?RADIANCE` / `#?RGBE` spellings. The parsed identifier is
  preserved verbatim in the new `HdrHeader::magic_id` field (with the
  `#?` prefix stripped); an empty `#?` line and a first line lacking the
  `#?` prefix are both still rejected. On the write side, the new
  `MagicLine::Custom(String)` variant emits an arbitrary identifier, and
  `encode_hdr_preserving_magic` reproduces the decoded `magic_id` so a
  decode→encode round-trip keeps the original `#?…` line instead of
  rewriting every file's identifier to `#?RADIANCE`. `MagicLine` is now
  `Clone` (no longer `Copy`) to carry the owned `Custom` string. Eleven
  new unit tests cover identifier capture (LF + CRLF), custom-program
  acceptance, empty/missing-prefix rejection, byte-identity of
  `Custom("RADIANCE")` with the named variant, and the
  preserving-magic round-trip.

- Round 285 (depth — benchmark suite): the Criterion harness now covers
  the crate's whole hot surface. `benches/decode.rs` times `parse_hdr`
  on all three on-disk scanline flavours (new-RLE, old-RLE, and
  uncompressed via `parse_hdr_with_options` +
  `FallbackMode::Uncompressed`); `benches/pixels.rs` times the
  whole-image XYZE↔RGB conversion in both working spaces plus all 8
  tone-mapping operators; `benches/encode.rs` gains the
  `RleMode::Uncompressed` flavour alongside `New` / `Old` / `Auto`.
  Measured numbers and a ranked per-pixel hotspot table land in the new
  `BENCHMARKS.md`, which names `ToneMap::Drago`'s per-channel
  recomputation of loop-invariant transcendentals (`log_bias`,
  `log_max`, `log_max_denom` — constant per image, 40.6 ns/px vs the
  16.8 ns/px runner-up) as the next profile-optimisation target.
  Benches and docs only — `src/` is byte-identical to round 275.

### Changed

- Round 275 (spec-conformance — single-`FORMAT` enforcement): the
  header parser now rejects a picture that carries more than one
  `FORMAT=` record with an `Invalid` error rather than silently
  last-wins overwriting it. This enforces the staged spec at
  `docs/image/hdr/radiance-hdr-rgbe-format.md` §1: "**At most one**
  `FORMAT` line is allowed; it must be `32-bit_rle_rgbe` or
  `32-bit_rle_xyze` for a valid Radiance picture." Two distinct
  pixel-format declarations leave the scanline section ambiguous, so a
  duplicate is treated as a malformed file (the rule is structural —
  even two identical `FORMAT=` lines are rejected). Single-`FORMAT`
  files and headers with no `FORMAT` record (defaulting to RGBE) are
  unaffected.

### Added

- Round 269 (spec-compliance — `rgbe_channel_scale` inspector): new
  `pub fn rgbe_channel_scale(rgbe: [u8; 4]) -> Option<f32>` on the
  `rgbe` module (re-exported at the crate root), completing the
  quad-inspector trio started by the round-257
  `rgbe_unbiased_exponent` and round-261 `rgbe_is_zero_pixel`
  inspectors. Returns the shared per-channel scale factor `f` of an
  RGBE pixel — the value such that each decoded channel equals
  `mantissa_byte as f32 * f` — or `None` when the exponent byte is
  the all-zero sentinel the staged spec at
  `docs/image/hdr/radiance-hdr-rgbe-format.md` §3 documents as
  "exactly black; the zero exponent is the sentinel for 'no value',
  so there is no valid pixel with exponent byte 0". The factor is the
  spec-§3 decode formula verbatim ("remove bias + 8-bit scale":
  `f = ldexp(1.0, rgbe[3] - (128 + 8))` — the excess-128 exponent
  bias plus the `-8` the 256-mantissa scale contributes), computed
  with the same `ldexp` helper `rgbe_to_rgb` uses internally so the
  two paths agree bit-exactly. For the spec-canonical worked example
  `(R,G,B)=(1.0, 0.5, 0.25) -> bytes (128, 64, 32, 129)` the
  inspector returns `Some(2^-7)` and `mantissa * f` recovers all
  three channels exactly. Seven new unit tests pin the contract: the
  all-zero quad returns `None`; mantissas are not inspected (the
  sentinel keys off the exponent byte alone, so `[255, 255, 255, 0]`
  and `[7, 11, 200, 0]` both return `None`); the worked example
  returns exactly `0.0078125` with all three channels recovered by
  exact-equality assertions; boundary bytes pin the
  `f = 2^(byte - 136)` formula (`136 -> 1.0` unit-scale boundary,
  `135 -> 0.5`, `137 -> 2.0`, `1 -> 2^-135` subnormal-but-exact,
  `255 -> 2^119`) and an exhaustive walk confirms every non-sentinel
  scale is finite and strictly positive; an exhaustive (every
  exponent byte) cross-check against `rgbe_to_rgb` confirms
  `decoded[i] == mantissa[i] * f` bit-exactly on the non-sentinel
  branch and `[0.0, 0.0, 0.0]` on the sentinel branch; a second
  exhaustive cross-check pins the trio invariants
  (`rgbe_channel_scale(p).is_none() == rgbe_is_zero_pixel(p)` and
  `f == 2^n / 256` with `n` from `rgbe_unbiased_exponent`); and a
  round-trip through `rgb_to_rgbe` confirms `mantissa * f` of the
  encoder's quad recovers the encode input exactly for a
  power-of-two triple (and the sentinel for a black encode). Useful
  for the call site that actually multiplies — e.g. a luminance
  reduction folding the three mantissa bytes through their weights
  with the shared scale applied once, or a single-channel probe that
  wants `rgbe[1] as f32 * f` without building the other two channels
  the way `rgbe_to_rgb` does. The existing `rgbe_to_rgb` /
  `rgb_to_rgbe` / `rgbe_unbiased_exponent` / `rgbe_is_zero_pixel`
  primitives, the round-1..268 happy path, and the standalone
  (`default-features = false`) build are bit-identical — the
  inspector is purely additive.

- Round 261 (spec-compliance — `rgbe_is_zero_pixel` sentinel inspector):
  new `pub fn rgbe_is_zero_pixel(rgbe: [u8; 4]) -> bool` on the `rgbe`
  module (re-exported at the crate root), the `bool`-returning
  counterpart to the round-257 `rgbe_unbiased_exponent` inspector.
  Returns `true` when the pixel is the all-zero sentinel the staged
  spec at `docs/image/hdr/radiance-hdr-rgbe-format.md` §3 documents as
  "exactly black; the zero exponent is the sentinel for 'no value', so
  there is no valid pixel with exponent byte 0", and `false` otherwise.
  The sentinel test keys off the exponent byte alone (`rgbe[3] == 0`),
  matching the rule embedded in `rgbe_unbiased_exponent` and
  `rgbe_to_rgb` — the spec is explicit that exponent byte `0` is the
  "no value" marker regardless of the mantissa values, so
  `[255, 255, 255, 0]` and `[7, 11, 200, 0]` both report `true`. Six
  new unit tests pin the contract: the canonical `[0, 0, 0, 0]`
  sentinel returns `true`; mantissas are not inspected (the same
  sentinel shape with arbitrary mantissas still returns `true`); the
  boundary exponent bytes (1, 127, 128, 129, 255) plus the
  spec-canonical worked example `(128, 64, 32, 129)` all return
  `false`; an exhaustive (every exponent byte × two mantissa shapes)
  cross-check confirms `rgbe_is_zero_pixel(p) ==
  rgbe_unbiased_exponent(p).is_none()` so the two inspectors compose
  losslessly; an exhaustive cross-check against `rgbe_to_rgb` confirms
  the boolean tracks the decoder's black-branch decision exactly; and
  a round-trip through `rgb_to_rgbe` confirms the inspector reports
  the sentinel for black-encode inputs (including the defensive
  negative / non-finite clamp branch the encoder documents). The
  existing `rgbe_unbiased_exponent` / `rgbe_to_rgb` / `rgb_to_rgbe`
  primitives, the round-1..260 happy path, and the standalone
  (`default-features = false`) build are bit-identical — the
  inspector is purely additive. Useful for the "is this pixel the
  sentinel?" call site that doesn't need the exponent value (e.g. a
  scanline walk that skips sentinel pixels before a luminance scan,
  or a fuzz oracle counting sentinel pixels) where the
  `Option::is_none()` unwrap on the existing inspector is incidental
  noise.

- Round 257 (spec-compliance — `rgbe_unbiased_exponent` inspector):
  new `pub fn rgbe_unbiased_exponent(rgbe: [u8; 4]) -> Option<i32>`
  on the `rgbe` module (re-exported at the crate root), returning
  the unbiased shared exponent of an RGBE pixel — the integer `n`
  such that each channel equals `(mantissa / 256) * 2^n` — or
  `None` when the pixel's exponent byte is the all-zero sentinel
  the staged spec at `docs/image/hdr/radiance-hdr-rgbe-format.md`
  §3 documents as "exactly black; the zero exponent is the sentinel
  for 'no value', so there is no valid pixel with exponent byte 0".
  The on-disk exponent byte carries an excess-128 bias per the same
  spec section ("The exponent byte carries an excess-128 bias"); the
  inspector returns `rgbe[3] as i32 - 128`. Pinned by seven new unit
  tests: the all-zero quad returns `None`; mantissas are not
  inspected (the sentinel keys off the exponent byte alone, so
  `[255, 255, 255, 0]` and `[7, 11, 200, 0]` both return `None`);
  the spec-canonical worked example `(R,G,B)=(1.0, 0.5, 0.25) ->
  bytes (128, 64, 32, 129)` returns `Some(1)`; boundary bytes
  (1, 127, 128, 129, 255) pin the bias formula across the full
  non-sentinel range; a cross-check against `rgbe_to_rgb` confirms
  the returned `n` satisfies `decoded[i] == mantissa[i] / 256 *
  2^n` exactly; and a round-trip through `rgb_to_rgbe` confirms the
  inspector reads back the exponent the encoder selected (and
  reflects the all-zero sentinel for a black-pixel encode). Useful
  for the "what magnitude does this pixel sit at?" use-case where
  building the three `f32` channels would be wasted work — e.g.
  picking a per-pixel auto-exposure factor without fully decoding
  the picture, or filtering out the sentinel pixels before a
  luminance scan. The existing `rgbe_to_rgb` / `rgb_to_rgbe`
  primitives, the round-1..256 happy path, and the standalone
  (`default-features = false`) build are bit-identical — the
  inspector is purely additive.

- Round 252 (spec-compliance — `effective_exposure` / `effective_colorcorr`
  inspectors): two new `HdrImage` helpers,
  `effective_exposure() -> f32` and `effective_colorcorr() -> [f32; 3]`,
  that mirror the round-208 `effective_pixaspect` and round-214
  `effective_primaries` shape: each reads the typed
  [`HdrHeader`] slot and substitutes the staged-spec default when no
  record was present, without perturbing the underlying slot. The
  staged spec at `docs/image/hdr/radiance-hdr-rgbe-format.md` §1
  documents `EXPOSURE=` as a "cumulative" multiplier "already applied
  to all pixels" with the explicit "No `EXPOSURE` ⇒ none applied"
  default of `1.0` (the identity factor), and `COLORCORR=` as a
  per-primary multiplier that "should have unit brightness so it does
  not change overall brightness", giving the absent-record default of
  the per-channel identity triple `[1.0, 1.0, 1.0]`. Through round 251
  the only way to read the cumulative factor with the spec-documented
  default applied was to write the `header.exposure.unwrap_or(1.0)` /
  `header.colorcorr.unwrap_or([1.0; 3])` boilerplate at every call
  site; the new helpers do the substitution in one call. Callers that
  need to distinguish "file declared `EXPOSURE=1.0` explicitly" from
  "no record was present" can still match on the typed slot directly.
  Seven new unit tests pin the contract: each helper returns the
  spec-default when the slot is `None`, returns the header value
  verbatim when the slot is set (including the explicit `1.0` /
  `[1.0, 1.0, 1.0]` cases that fold into the default branch), and
  leaves the underlying `HdrHeader::exposure` / `HdrHeader::colorcorr`
  slot untouched (the typed-slot inspector contract). The existing
  `apply_exposure` / `apply_colorcorr` /
  `recover_original_radiance` / `recover_original_colorcorr`
  mutators, the round-1..251 happy path, and the standalone
  (`default-features = false`) build are bit-identical — the
  inspectors are purely additive.

- Round 248 (spec-compliance — `MagicLine` encoder option for the legacy
  `#?RGBE` identifier): new public `MagicLine` enum
  (`Radiance` / `Rgbe`) and a matching maximum-control entry point
  `encode_hdr_with_full_options(image, rle, line_ending, magic)`. The
  staged spec at `docs/image/hdr/radiance-hdr-rgbe-format.md` §1
  documents `#?RADIANCE` and `#?RGBE` as equivalent identifier lines
  ("some files / writers use the equivalent `#?RGBE`"). The decoder has
  accepted both spellings since round 1 (`parse_header` checks both
  literals); through round 247 the encoder hard-coded the `#?RADIANCE`
  spelling on every write path, so a caller round-tripping a file whose
  original magic was `#?RGBE` couldn't reproduce the original byte
  sequence and a downstream consumer that only recognises the legacy
  identifier couldn't be fed an oxideav-hdr-produced file. The new
  entry point lets the caller pick which spelling to emit; the
  `MagicLine::Radiance` branch is byte-identical to the existing
  `encode_hdr_with_options` output (a regression test pins this), and
  the `MagicLine::Rgbe` branch differs from the `Radiance` output only
  in the four extra bytes the `RADIANCE` spelling carries. The
  `encode_hdr` / `encode_hdr_with_rle` / `encode_hdr_with_options`
  signatures and the round-1..247 happy path are bit-identical — the
  helper is purely additive. Five new unit tests pin the contract:
  every existing entry point still emits `#?RADIANCE\n` (or
  `#?RADIANCE\r\n` under CRLF), the `MagicLine::Radiance` branch of the
  full-options entry point reproduces the `encode_hdr_with_options`
  output byte-for-byte, the `MagicLine::Rgbe` branch produces a file
  with the legacy identifier whose remaining bytes are identical to the
  `MagicLine::Radiance` output (length differs by exactly the four-byte
  spelling delta), `MagicLine::Rgbe` honours `LineEnding::Crlf` (the
  identifier ends in `\r\n` matching the rest of the text section), and
  every typed `KEY=VALUE` slot still round-trips through the decoder
  when the legacy magic is in play. The standalone
  (`default-features = false`) build path is unchanged.

- Round 231 (spec-compliance — wide-gamut image-level XYZE↔RGB
  converters): four new `xyz` module helpers,
  `convert_image_xyz_to_rgb_with_primaries(image, primaries) -> bool`,
  `convert_image_rgb_to_xyz_with_primaries(image, primaries) -> bool`,
  `convert_image_xyz_to_rgb_with_effective_primaries(image) -> bool`,
  and `convert_image_rgb_to_xyz_with_effective_primaries(image) -> bool`,
  that walk the picture's float buffer in-place using the matrix the
  round-226 `rgb_to_xyz_matrix_from_primaries` /
  `xyz_to_rgb_matrix_from_primaries` derives from an arbitrary
  [`Primaries`] record. Through round 230 the only whole-image XYZE↔RGB
  converters the crate shipped were the round-2
  `convert_image_xyz_to_rgb` / `convert_image_rgb_to_xyz` pair, which
  hard-code an [`RgbColorSpace`] enum variant (sRGB or Radiance). Files
  that carried a wide-gamut `PRIMARIES=` record (e.g.
  `Primaries::P3_D65` or `Primaries::REC2020`, both added in round 4)
  or a custom 8-float record from a niche renderer had no equivalent
  whole-image API — consumers had to call
  `rgb_to_xyz_matrix_from_primaries` themselves and walk the float
  buffer manually, or fall back to the sRGB converter (which is wrong
  for wide-gamut content). The new helpers wire the round-226 derived
  matrix into the existing chunks-of-three walk + format-tag flip, and
  expose two further `_with_effective_primaries` convenience wrappers
  that thread the file's own [`HdrImage::effective_primaries`] in
  (header value when set, reference-manual default
  [`Primaries::RADIANCE`] when no record was present) so the most
  common XYZE→RGB call shape becomes a single `bool`-returning method
  with no plumbing. Seven new unit tests pin the contract: a
  full RGB→XYZ→RGB round-trip through the P3-D65 chromaticity-derived
  matrix recovers the input buffer within `f32` precision, the new
  `_with_primaries(_, Primaries::SRGB)` path produces numerically the
  same buffer as the round-2 `convert_image_xyz_to_rgb(_,
  RgbColorSpace::Srgb)` (within `1e-3`), the `_with_effective_primaries`
  wrappers thread `header.primaries` through unchanged (verified
  against an explicit call with the same record), the
  `_with_effective_primaries` variants fall back to
  `Primaries::RADIANCE` when the slot is `None`, the degenerate
  `yW = 0` record short-circuits to `false` and leaves both the pixel
  buffer and the format tag untouched (so a caller can recover with a
  named matrix without first re-deriving the float channels), and a
  full RGB→XYZ→RGB round-trip through the effective-primaries
  wrappers with a P3-D65 header round-trips losslessly. The existing
  `convert_image_xyz_to_rgb` / `convert_image_rgb_to_xyz` signatures
  and the round-1..230 happy paths are bit-identical — the new helpers
  are purely additive. The standalone (`default-features = false`)
  build path is unchanged.

- Round 226 (spec-compliance — chromaticity-derived `RGB ↔ XYZ`
  matrices): two new `xyz` module helpers,
  `rgb_to_xyz_matrix_from_primaries(p: Primaries) -> Option<[[f32; 3];
  3]>` and `xyz_to_rgb_matrix_from_primaries(p) -> Option<[[f32; 3];
  3]>`, that derive a full linear `RGB → CIE XYZ` (and inverse) matrix
  from any [`Primaries`] record's eight CIE xy chromaticity floats
  using the standard primary-construction procedure documented in
  BT.709 §3 / IEC 61966-2-1 Annex C. Through round 225 the crate only
  shipped pre-computed matrices for two named `RgbColorSpace` variants
  (`Srgb`, `Radiance`); files that carried a wide-gamut `PRIMARIES=`
  record (e.g. `Primaries::P3_D65` or `Primaries::REC2020`, both
  added in round 4) or a custom 8-float record from a niche renderer
  had no equivalent matrix path — consumers had to either fall back to
  the sRGB matrix (wrong for wide-gamut content) or hand-derive the
  matrix from the eight floats themselves. The new helpers do the
  derivation in-crate using only the existing `Primaries` struct, a
  3×3 cofactor expansion, and `f32` arithmetic. Eight new unit tests
  pin the contract: the derived sRGB / Radiance matrices match the
  hard-coded `RgbColorSpace` constants within `f32` precision (1e-3),
  `[1, 1, 1]^T` maps to the correct CIE XYZ for every named primaries
  constant the crate ships (sRGB / Radiance / P3-D65 / Rec.2020), the
  forward and inverse matrices are mutual inverses, the helpers reject
  degenerate `yW = 0` and zero-Y-primary records by returning `None`
  rather than emitting `inf`s, and the P3-D65 / Rec.2020 derived
  matrices map unit RGB to the nominal D65 XYZ `(0.9505, 1.0000,
  1.0890)` within `f32` precision. The existing `rgb_to_xyz_matrix` /
  `xyz_to_rgb_matrix` constants and the round-1..225 happy path are
  bit-identical — the helpers are purely additive.

- Round 220 (spec-compliance — original-radiance recovery): two new
  `HdrImage` helpers, `recover_original_radiance` and
  `recover_original_colorcorr`, that divide the float buffer by the
  cumulative `EXPOSURE=` / `COLORCORR=` factors stored in
  `HdrHeader` and clear the slots. The staged spec
  (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 EXPOSURE /
  COLORCORR rows) defines both records as multipliers "already
  applied" to the pixels at write time — the stored channel
  `c = original * EXPOSURE * COLORCORR_i`. Recovering the
  scene-referred radiance is therefore the divide-by-the-product
  operation the spec describes verbatim ("to recover original
  radiances divide file values by the product of all EXPOSURE
  settings"). The existing `apply_exposure` / `apply_colorcorr`
  helpers post-multiply by the recorded factor (the renderer-side
  adjustment idiom); the new helpers are their spec-canonical
  inverse, so consumers that need true scene-referred radiance for
  downstream radiometric work no longer have to reach into
  `HdrHeader::exposure` / `colorcorr` and roll the divide
  themselves. Degenerate edge cases (`None` slot, `0.0`, non-finite
  factors, the trivial `1.0` / `[1.0, 1.0, 1.0]` no-op) are handled
  explicitly — the slot is cleared but the pixel buffer is never
  written with `NaN` / `inf`. Eleven new unit tests pin the
  divide-and-clear contract, the no-op edge cases, the
  inverse-of-`apply_*` round-trip, and the stacked-records case
  where the decoder folds multiple `EXPOSURE=` records into a single
  running product (a single divide by that product undoes the whole
  stack). The existing `apply_*` semantics, the round-192 fixture
  regression tests, and the standalone (`default-features = false`)
  build path are all unchanged — the helpers are purely additive.

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
