# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
