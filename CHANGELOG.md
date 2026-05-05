# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2](https://github.com/OxideAV/oxideav-hdr/compare/v0.0.1...v0.0.2) - 2026-05-05

### Other

- clippy needless_range_loop fix in solid_colour_roundtrips_via_repeat_run

### Added

- Initial release: pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader +
  writer covering the standard new-RLE pixel encoding plus the older
  pre-1991 sentinel-pixel old-RLE format on the read path.
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
