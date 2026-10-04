# oxideav-hdr

[![CI](https://github.com/OxideAV/oxideav-hdr/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-hdr/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-hdr.svg)](https://crates.io/crates/oxideav-hdr) [![docs.rs](https://docs.rs/oxideav-hdr/badge.svg)](https://docs.rs/oxideav-hdr) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader + writer for the
[oxideav](https://github.com/OxideAV/oxideav-workspace) workspace.

Radiance RGBE is the shared-exponent floating-point image format,
originally described in *Real Pixels* (Graphics Gems II, 1991). The
on-disk representation packs three 8-bit RGB mantissa bytes plus one
shared 8-bit biased exponent into 4 bytes per pixel, then RLE-codes
each scanline. The decoder produces one packed little-endian `f32` RGB
plane (`RgbF32Le`, scene-referred linear light); the encoder takes the
same shape and emits a complete file with the header's axis flags
(canonical `-Y H +X W` by default).

The crate root follows the OxideAV
[image-crate API contract](../../IMAGE_CRATE_API.md); the Radiance
depth (header records, orientation algebra, exposure / gamma / colour
helpers, RGBE quad inspectors) lives under its own names alongside.

Clean-room implementation against the published format documentation.
No external library source consulted.

## Standalone use

```toml
oxideav-hdr = { version = "0.0", default-features = false }
```

```rust
let bytes = std::fs::read("in.hdr")?;
if oxideav_hdr::probe(&bytes) {
    let info = oxideav_hdr::info(&bytes)?;           // header only: width, height, RgbF32Le, colour
    let img = oxideav_hdr::decode(&bytes)?;          // HdrImage: one packed f32 RGB plane
    let floats: Vec<f32> = img.pixels();             // width * height * 3 linear radiance samples
    let rgb8: Vec<u8> = img.to_rgb8();               // clamp [0, 1] × 255 (no tone curve)
    let (w, h) = (img.width(), img.height());

    let opts = oxideav_hdr::EncodeOptions::default().with_software("my tool".to_string());
    let out = oxideav_hdr::encode(&img, &opts)?;
    std::fs::write("out.hdr", out)?;

    // 8-bit in: bytes are linear / 255 unless `with_input_gamma(2.2)`.
    let _ = oxideav_hdr::encode_rgb8(w, h, &rgb8, &opts)?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Root vocabulary: `probe`, `info -> ImageInfo`, `decode -> HdrImage`,
`decode_with(&DecodeOptions)`, `decode_rgb8 -> RgbImage`,
`decode_rgba8 -> RgbaImage`, `decode_from<R: Read>`,
`encode(&HdrImage, &EncodeOptions)`, `encode_rgb8`, `encode_rgba8`,
`encode_to<W: Write>`; types `HdrImage { width, height, format, planes,
color, metadata, header }`, `Plane`, `ColorInfo`, `ColorRange`,
`Metadata`, `RgbImage`, `RgbaImage`, `ImageInfo`, `PixelFormat` (=
`HdrPixelFormat`), `HdrError` (= `Error`: `InvalidData`, `Unsupported`,
`LimitExceeded`, `Io`). A Radiance file holds one picture, so there is
no `decode_all` and no palette.

`HdrImage` is built with the fallible constructors `new` / `packed` /
`from_f32` / `from_rgb8` / `from_rgba8` / `from_rgbe_quads` (geometry
validated, so `to_rgb8` / `to_rgba8` never fail). The float view is
`pixels()` (tight copy), `pixel(x, y)`, `for_each_pixel`, `map_pixels`
/ `map_samples` (in place); the byte view is `as_bytes()` / `into_raw()`.

The pre-contract names (`parse_hdr*`, `encode_hdr*`, `HdrLimits`,
`HdrImage::new_rgb96f`, `HdrPixelFormat::Rgb96f`, `register_runtime`,
`parse_hdr_videoframe`) remain for one release as deprecated wrappers;
see the CHANGELOG for the mapping.

## Framework use

```toml
oxideav-hdr = "0.0"    # default `registry` feature: pulls oxideav-core
```

`oxideav_hdr::register(&mut RuntimeContext)` installs the `hdr` codec
(decoder + encoder, `hdr_sw`) and the container (`.hdr` / `.pic`
extensions, `#?RADIANCE` / `#?RGBE` probe, one-packet demuxer /
muxer); `register_codecs` / `register_containers` /
`register_registries` take the individual registries, `make_decoder` /
`make_encoder` are the factories. `oxideav_meta::register_all` calls
`register` for you.

The framework boundary is the **native float layout**: the `Decoder`
emits `RgbF32Le` frames (one packed plane, scene-referred linear light,
colour signal attached) — no tone mapping — so `oxideav-pixfmt` /
`oxideav-image` decide how to display them. The `Encoder` accepts
`RgbF32Le` natively and `Rgb24` / `Rgba` through the raw-path rule
(`b / 255`, alpha dropped; `input_gamma` option to linearise); its
options schema is `rle`, `line_ending`, `exposure`, `software`,
`gamma`, `input_gamma`. The frame bridge is `From<HdrImage> for
VideoFrame` and `HdrImage::from_video_frame(&VideoFrame,
&CodecParameters) -> Result<HdrImage, HdrError>` (also
`TryFrom<(&VideoFrame, &CodecParameters)>`).

## Supported layouts

| Decode (native) | Encode |
|---|---|
| `RgbF32Le` — one packed plane, 12 bytes/pixel, R, G, B little-endian `f32`, display order (every on-disk orientation normalised) | `RgbF32Le` as given (padded planes repacked). `encode_rgb8` / `encode_rgba8` / `Rgb24` / `Rgba` frames: `b / 255` → float (or `(b / 255) ^ input_gamma`), alpha dropped |

Both `FORMAT=32-bit_rle_rgbe` (RGB radiance) and `FORMAT=32-bit_rle_xyze`
(CIE XYZ) decode to the same layout; `header.format` /
`ImageInfo::rgbe_format` says which triple the floats are. `to_rgb8` /
`to_rgba8` clamp each float to `[0, 1]` and scale `× 255` (nearest; `NaN`
→ 0), converting XYZE triples to RGB in the picture's effective
primaries first; `EXPOSURE=` / `COLORCORR=` / `GAMMA=` are **not**
applied (see `HdrImage::to_rgb8_with_exposure(stops)`, `tone_map`, and
the `apply_*` / `recover_*` / `linearize_gamma` helpers). The float →
RGBE quantisation on encode is lossy at ~1 % per channel; the quad layer
(`from_rgbe_quads` / `to_rgbe_quads`) is bit-exact.

## Options

`EncodeOptions` (`#[non_exhaustive]`, `Default`, `with_*`): `rle:
RleMode` (`Auto` default — new-RLE for on-disk scanline widths in
`8..=32767`, old-RLE otherwise; `New`, `Old`, `Uncompressed`,
`Smallest`), `line_ending: LineEnding` (`Lf` / `Crlf`), `magic:
Option<MagicLine>` (`None` preserves the decoded `#?…` identifier,
`#?RADIANCE` for a fresh image), header overrides `exposure`,
`software`, `primaries`, `gamma` (each `Option`, overriding the image's
header for the one encode), and `input_gamma` for the 8-bit paths.

`DecodeOptions` (`#[non_exhaustive]`, `Default`, `with_*`): `max_width`
/ `max_height` (`Some(32_767)`), `max_pixels` (`None`), `max_bytes`
(`Some(1 GiB)`), `strict` (`false`), `fallback: FallbackMode` (`OldRle`
— how a scanline without the new-RLE marker is read; `Uncompressed` for
files written with `RleMode::Uncompressed` / `Smallest`). `unlimited()`
lifts every limit.

## Metadata and colour

Radiance headers carry no ICC / Exif / XMP, so `Metadata { icc, exif,
xmp }` are always `None`. `Metadata::gamma` is the encoding exponent
derived from the de-facto `GAMMA=g` record (`stored = linear^(1/g)` per
the staged spec, so `gamma = 1 / g`, e.g. `GAMMA=2.2` → `0.4545`); `None`
when absent, `1`, or degenerate.

`ColorInfo { range, primaries, transfer, matrix }` (H.273 code points)
is derived from the header: `Full` range, `matrix` 0 (RGB), `transfer`
8 (linear) unless `GAMMA≠1` (then 2, unspecified, with the exponent in
`metadata.gamma`), `primaries` 1 / 9 / 12 when the `PRIMARIES=` record
matches BT.709 / BT.2020 / Display P3 chromaticities (within 1e-3), else
2. Radiance's own default primaries (`0.640 0.330 0.290 0.600 0.150
0.060 0.333 0.333`, no `PRIMARIES=` record) have no H.273 code point, so
such a file reports `primaries = 2`; the exact chromaticities are always
available through `HdrImage::effective_primaries()`. The complete
parsed header (every typed record, free-form records, comments, command
lines, axis flags) is `HdrImage::header` / `ImageInfo::header`, and
the encoder writes it back; `with_header` / `sync_color_from_header`
re-derive `color` / `metadata` after editing it.

## Limits

Every `DecodeOptions` limit is checked against the resolution line
**before** the plane is allocated (`HdrError::LimitExceeded`); the
defaults admit any picture up to 32 767 on a side and ~89 megapixels
(1 GiB of `f32` RGB). `info` applies no limit and allocates only the
header. `probe` is total and allocation-free; the fuzz targets cover
`probe` / `info` / `decode` (lenient and strict) on hostile input.
`strict` additionally rejects a magic identifier other than `RADIANCE` /
`RGBE`, a header without `FORMAT=`, and trailing bytes after the last
scanline.

## Format specifics

### Format coverage

| Feature                      | Read | Write |
|------------------------------|:----:|:-----:|
| `#?RADIANCE` / `#?RGBE` magic|  Y   |   Y (default `#?RADIANCE`; `EncodeOptions::default().with_magic(MagicLine::Rgbe)` for the legacy spelling) |
| `#?<identifier>` general magic line (the staged note's `HDRSTR = "#?"` + caller-supplied identifier — `#?RADIANCE` / `#?RGBE` are just the common spellings; any non-empty token after `#?` is a valid header-id line and is preserved verbatim in `HdrHeader::magic_id`) | Y (parsed into `magic_id`; empty `#?` rejected) | Y (`MagicLine::Custom(String)` emits an arbitrary identifier; the default `EncodeOptions::magic = None` reproduces the decoded `magic_id` so a decode→encode round-trip keeps the original `#?…` line instead of rewriting it to `#?RADIANCE`) |
| `KEY=VALUE` header records   |  Y   |   Y   |
| Header program/command lines (the format note's "`#?…` identifier followed by one or more lines giving the programs used to produce the picture, interspersed with variable assignments" — a non-comment header line carrying no `=`, e.g. `rpict -vp 0 0 0 scene.oct`, is kept verbatim in `HdrHeader::commands` and re-emitted right after the magic line, rather than rejecting renderer-produced files) | Y (preserved in read order) | Y (emitted ahead of `FORMAT=`) |
| `FORMAT` declared **at most once** (a second `FORMAT=` record is rejected as invalid per the staged spec's "at most one FORMAT line is allowed", rather than last-wins overwriting an ambiguous pixel-format declaration; the value is trimmed of surrounding whitespace before matching the two valid pixel formats — `FORMAT= 32-bit_rle_rgbe ` parses — consistent with the spec's "value up until the end of line" and every sibling typed field, while interior non-format tokens stay rejected as unsupported) | Y (enforced) | Y (single) |
| `EXPOSURE` / `GAMMA` / `PIXASPECT` / `SOFTWARE` | Y | Y |
| `VIEW=` renderer view-parameter record | Y | Y |
| Multiple `VIEW=` records merged cumulatively (a later `-v<x>` option group overrides the same flag in the accumulated view, genuinely-new flags are appended, the later command prefix wins — per the format note's "cumulative inasmuch as new view options add to or override old ones" rule, not whole-string last-wins) | Y | n/a |
| Multiple `EXPOSURE` / `COLORCORR` / `PIXASPECT` records stacked multiplicatively | Y | n/a |
| `COLORCORR` (3-float)        |  Y   |   Y   |
| `HdrImage::effective_pixaspect` (header value or reference-manual default `1.0`) | helper | n/a |
| `HdrImage::square_pixel_dimensions` / `display_aspect_ratio` (display geometry corrected for non-square pixels per spec §1's "PIXASPECT = pixel height / pixel width … **not** the image aspect ratio": stretches the height axis by the cumulative `PIXASPECT` factor so a viewer draws the picture undistorted — `(width, height·p)` square-pixel size and the `width/(height·p)` displayed width:height ratio; degenerate `0`/non-finite factor and zero height fall back to the `1.0` identity) | helper | n/a |
| `HdrImage::effective_exposure` (header value or staged-spec default `1.0` per "no `EXPOSURE` ⇒ none applied") | helper | n/a |
| `HdrImage::effective_colorcorr` (header value or staged-spec default `[1.0, 1.0, 1.0]` per "should have unit brightness") | helper | n/a |
| `PRIMARIES` (8-float chromaticity) | Y |   Y   |
| `HdrImage::effective_primaries` (header value or reference-manual default `0.640 0.330 0.290 0.600 0.150 0.060 1/3 1/3` (default origin primaries) with equal-energy white) | helper | n/a |
| All 8 axis-flag combinations |  Y   |  Y (Y-first + X-first transpose) |
| `Orientation` enum naming all 8 resolution-string forms (`Standard` = `-Y N +X M`, `FlipX`, `Rotate180`, `FlipY`, `Rotate90Cw`, `Rotate90CwFlipY`, `Rotate90Ccw`, `Rotate90CcwFlipY` per the format note's §2 table) with `from_axis_fields` / `to_axis_fields` (a total mutual inverse over the `(y_sign, x_sign, x_first)` triple), `is_x_first`, `resolution_template`, plus `HdrHeader::orientation` / `set_orientation` to read or set the on-disk scanline layout by geometric name | helper | helper |
| `GeometricOp` (the §2 orientation matrix as the rectangle's dihedral group `D₄`: `Identity` / `FlipHorizontal` / `FlipVertical` / `Rotate180` / `Rotate90Cw` / `Rotate90Ccw` / `Transpose` / `AntiTranspose`) with `ALL`, `swaps_dimensions` (the four 90°-class ops swap W/H), `inverse`, and `then` (group composition — proven closed + associative); `Orientation::display_transform` / `from_display_transform` bijection from each resolution-string form to the op carrying the standard displayed picture onto it | helper | helper |
| `HdrImage::{apply_geometric, to_orientation, normalize_from, reorient}` — geometric reorientation of the **decoded** float buffer (lossless pure pixel permutation; 90°-class ops swap `width`/`height`; header/`format` untouched; zero-dimension guard). `reorient(from, to)` collapses `from⁻¹ ∘ to` into one op across the full 8×8 orientation matrix; verified end-to-end through `encode` → `decode` in `tests/reorient.rs` | helper | helper |
| 32-bit_rle_rgbe pixels       |  Y   |   Y   |
| `HdrImage::from_rgbe_quads` / `to_rgbe_quads` (byte-level view of the picture: build a float image from exact on-disk `[R, G, B, E]` quads, or re-derive the quad stream the encoder commits to the wire — a **bit-exact** RGBE round-trip surface alongside the lossy float-in/float-out path, since `rgb_to_rgbe` is idempotent on the normalised quads the encoder produces) | helper | helper |
| `rgbe_unbiased_exponent([u8; 4]) -> Option<i32>` (returns the spec-§3 `byte - 128` shared exponent, or `None` for the all-zero sentinel pixel — `Some(1)` for the spec-canonical worked example `(128, 64, 32, 129)`) | inspector | n/a |
| `rgbe_is_zero_pixel([u8; 4]) -> bool` (`bool`-returning sentinel inspector keying off `rgbe[3] == 0` per spec §3's "no valid pixel with exponent byte 0" rule — the boolean counterpart to `rgbe_unbiased_exponent` for call sites that don't need the exponent value) | inspector | n/a |
| `rgbe_channel_scale([u8; 4]) -> Option<f32>` (the spec-§3 decode-formula factor `f = ldexp(1.0, byte − (128 + 8))` such that each channel equals `mantissa * f`, or `None` for the all-zero sentinel — `Some(2⁻⁷)` for the spec-canonical worked example `(128, 64, 32, 129)`; completes the quad-inspector trio) | inspector | n/a |
| `rgbe_shift_exponent([u8; 4], stops) -> Option<[u8; 4]>` (exact `×2^stops` on the wire quad — a pure exponent-byte add per the §3 shared-exponent layout, mantissas/chromaticity untouched, no `f32` quantisation round-trip; sentinel passes through verbatim (black × 2ⁿ = black), a shifted exponent outside `1..=255` is unrepresentable → `None`; agrees byte-for-byte with `adjust_exposure_stops` + `to_rgbe_quads` on normalised quads) | quad helper | quad helper |
| 32-bit_rle_xyze pixels       |  Y   |   Y (with helpers in `xyz`) |
| New RLE (`0x02 0x02 hi lo`)  |  Y   |   Y   |
| Old RLE (sentinel pixels) — including runs that **span the scanline boundary** (a sentinel as the first quad of a later scanline repeats the carried-over previous pixel), and a spec-conformant rejection of a **leading sentinel with no previous pixel** (a sentinel as the very first quad of the picture is malformed per the format note's "the first scanline cannot be a sentinel run", surfaced as an error rather than silently decoding a black run) |  Y   |   Y (`RleMode::Old`) |
| Auto-RLE (width heuristic)   |  -   |   Y (`RleMode::Auto`) |
| Content-adaptive smallest-output encode (`RleMode::Smallest`: per scanline, emit whichever of new-RLE and flat is smaller — valid because the spec's reader probe is per-scanline; noisy rows genuinely cost more under new-RLE (`width + ceil(width/128)` per channel + marker) so mixed-content files beat both pure modes; out-of-range widths always flat, never errors; flat rows provably can't alias the `0x02 0x02` marker since the quantiser keeps the dominant mantissa ≥ 128 while the probe needs the first three bytes < 0x80; pair with `FallbackMode::Uncompressed`) | - | Y (`RleMode::Smallest`) |
| Uncompressed (flat `4 * W` byte) scanlines | Y (`DecodeOptions::default().with_fallback(FallbackMode::Uncompressed)`) | Y (`RleMode::Uncompressed`) |
| CRLF line endings            |  Y   |   Y (`LineEnding::Crlf`) |
| Decoder resource limits (`DecodeOptions`) | Y (default 32 767 × 32 767, ≤ 1 GiB plane; `decode_with` for custom) | n/a |
| `HdrImage::apply_exposure`   |  decode helper |  n/a |
| `HdrImage::adjust_exposure_factor` / `adjust_exposure_stops` (writer-side exposure adjustment that keeps the picture physically meaningful: multiplies the stored channels **and** folds the same factor into the `EXPOSURE=` slot per spec §1's cumulative already-applied-multiplier rule, so `scene_referred_radiance_buffer` / the `recover_*` helpers are invariant; the `2^stops` form matches the format note's integer-stop (`-e +/-stops`) brightness adjustment and is bit-exact reversible — `+n` then `-n` restores every `f32` sample; degenerate `0`/negative/non-finite factors rejected with the picture untouched) | helper | helper |
| `HdrImage::apply_colorcorr`  |  decode helper |  n/a |
| `HdrImage::recover_original_radiance` (spec-canonical undo of `EXPOSURE=` — divides the buffer by the cumulative factor to reconstruct scene-referred radiance, per the staged spec's "divide file values by the product of all EXPOSURE settings" rule) | decode helper | n/a |
| `HdrImage::recover_original_colorcorr` (per-channel reciprocal of `apply_colorcorr` — reconstructs pre-correction radiance for files that carry `COLORCORR=`) | decode helper | n/a |
| `HdrImage::recover_scene_referred_radiance` (one-shot in-place composition of `recover_original_radiance` + `recover_original_colorcorr` — divides *both* the cumulative `EXPOSURE=` multiplier and the `COLORCORR=` triple out of the stored channels and clears both header slots, the full §1 "recover original radiances" operation in one call; degenerate / unit / absent factors are no-op divisions that still clear the slot) | decode helper | n/a |
| `HdrImage::scene_referred_radiance_buffer` (non-mutating RGB-buffer counterpart to `scene_referred_luminance_buffer`: a fresh `width·height·3` float buffer of the recovered scene-referred radiance — stored channels with the baked-in `EXPOSURE` + `COLORCORR` divided back out — leaving the image's pixels and typed slots untouched so the records survive a re-encode; equals `pixels` exactly when neither record is present; degenerate factors treated as identity) | decode helper | n/a |
| `HdrImage::effective_gamma` (header `GAMMA=` value or the staged-spec default `1.0` per the "The `GAMMA=` header variable" section's "when no `GAMMA=` line is present, the value is taken to be `1.0`, meaning no gamma correction has been applied and the stored pixels are already linear" — `GAMMA=` is a de-facto extension outside the canonical seven header variables, so native-Radiance pictures omit it and read as the linear identity) | helper | n/a |
| `HdrImage::linearize_gamma` / `linear_radiance_buffer` (apply the staged-spec `linear_channel = stored_channel ^ g` linearisation — a file carrying `GAMMA=g` stores gamma-encoded, display-oriented channels a honouring reader must undo before radiometric use — to the decoded float buffer; the in-place form clears the slot so a re-encode neither re-declares nor double-applies, the buffer form is non-mutating and preserves the slot; absent / `1.0` / degenerate exponents are identity no-ops, negative channels pass through verbatim since a fractional power of a negative base is NaN) | decode helper | decode helper |
| `HdrImage::recover_linear_scene_referred_radiance` / `linear_scene_referred_radiance_buffer` (the staged spec's fully-specified decode order from "The `GAMMA=` header variable": linearise **first** (`stored^g`), **then** divide out `COLORCORR` and `EXPOSURE` — because the linear `EXPOSURE` / `COLORCORR` recovery is only meaningful once the pixels are linear; the gamma-aware extension of the `scene_referred_radiance_buffer` / `recover_scene_referred_radiance` pair, reducing to them when `GAMMA` is absent; mutating form clears all three slots, buffer form is non-mutating) | decode helper | decode helper |
| `HdrImage::linear_scene_referred_luminance_buffer` (gamma-aware extension of `scene_referred_luminance_buffer`: linearises the stored channels through `GAMMA=` (`stored^g`) and divides out `EXPOSURE` + `COLORCORR` before the §"Physical interpretation" luminance projection — honouring the staged spec's rule that the radiance recovery is only meaningful once the pixels are linear; reduces to `scene_referred_luminance_buffer` when `GAMMA` is absent; non-mutating) | decode helper | n/a |
| `HdrImage::apply_gamma_encoding(g)` (writer-side inverse of `linearize_gamma`: assumes a linear buffer, encodes `stored = linear ^ (1/g)` and records `GAMMA=g`; the gamma analogue of `adjust_exposure_factor`, a paired pixel-and-header write that keeps the file self-consistent; rejects degenerate `g` (`0` / negative / non-finite) as `false` with the picture untouched, an exact `1.0` records `GAMMA=1.0` as an explicit linear marker — the staged spec notes canonical readers ignore `GAMMA` so interop-maximising writers should emit linear pixels and omit it) | n/a | helper |
| XYZE ↔ RGB (sRGB / Radiance) |  -   | helpers |
| `rgb_to_xyz_matrix_from_primaries` / `xyz_to_rgb_matrix_from_primaries` (derive a linear `RGB ↔ XYZ` matrix from any `Primaries` record's eight CIE xy floats per BT.709 §3 / IEC 61966-2-1 Annex C — works for `P3_D65`, `REC2020`, and arbitrary 8-float `PRIMARIES=` records the named `RgbColorSpace` enum doesn't cover) | n/a | helpers |
| `convert_image_xyz_to_rgb_with_primaries` / `convert_image_rgb_to_xyz_with_primaries` + `_with_effective_primaries` wide-gamut whole-image converters (in-place; pick the chromaticity record explicitly or thread the file's own `PRIMARIES=` via `effective_primaries`; return `bool` and leave the buffer / format tag untouched on degenerate input) | helpers | helpers |
| `convert_image_rgb_to_xyz_photometric` / `convert_image_xyz_to_rgb_photometric` (+ `_with_primaries` / `_with_effective_primaries`) **file-faithful** format converters — per spec §"Physical interpretation" the two `FORMAT`s sit on different physical scales (RGBE = spectral radiance in W/sr/m², XYZE's "Y primary is already lumens/steradian/m²"), so these compose the colorimetric matrix with the uniform `WHTEFFICACY = 179` lm/W scale in one pass; `luminance_buffer` of the converted picture agrees with the original (the plain converters stay purely colorimetric) | helpers | helpers |
| `Primaries::SRGB` / `RADIANCE` / `P3_D65` / `REC2020` constants | n/a | constants |
| Tone-mapping (Linear / Gamma / Reinhard / ReinhardExtended / ReinhardLuminance / Hable / Drago / ACES) | - | helpers |
| Radiance photometric luminance (`179 * (0.265 R + 0.670 G + 0.065 B)` for RGBE; stored `Y` verbatim for XYZE per spec §"Physical interpretation" — "the Y primary is already lumens/steradian/m², so the 179× luminance conversion is unnecessary"; the pre-r383 `179 * Y` XYZE branch overstated luminance by the efficacy factor) | helper (`luminance_lm_per_sr_per_m2`, `HdrImage::luminance_buffer`) | n/a |
| `HdrImage::scene_referred_luminance_buffer` — *physical* per-pixel luminance (lm/sr/m²) computed after dividing out the cumulative `EXPOSURE` product and per-channel `COLORCORR` triple the writer baked in, composing the staged spec's §1 recovery rules with the §"Physical interpretation" luminance formula (non-mutating; degenerate `0`/non-finite factors treated as identity; agrees with `luminance_buffer` when neither record is present) | helper | n/a |

### Bit-exact RGBE-quad round-trip matrix

The float-in / float-out API (`encode` / `decode`) is inherently
lossy — the 8-bit shared-exponent mantissa quantises each pixel. The
**byte** layer is not: every scanline flavour (new-RLE, old-RLE,
uncompressed) is a lossless re-packing of the exact `[R, G, B, E]`
quads, and `rgb_to_rgbe` is idempotent on the normalised quad subset the
encoder produces (dominant mantissa `≥ 128`, decoded magnitude above the
`1e-32` black floor). `tests/rgbe_roundtrip_matrix.rs` proves the
resulting contract end-to-end: a picture built from normalised quads via
`HdrImage::from_rgbe_quads`, encoded, decoded, and read back with
`to_rgbe_quads`, reproduces the original quad stream **byte-for-byte**
across the cross-product of resolution variants × all eight
resolution-string orientations × every RLE flavour, for both RGBE and
XYZE pictures, under LF and CRLF line endings, and alongside the full
typed-header round-trip. The quad streams come from a small
deterministic in-tree LCG (no external property-test crate).

An opt-in, env-gated test suite cross-validates encode/decode against
an external Radiance-capable image tool when one is present on `PATH`
(black-box validator only; it skips cleanly when absent).

Committed on-disk regression fixtures live under
[`tests/fixtures/`](tests/fixtures/) (`gradient_32x16_newrle.hdr`,
`solid_16x8_oldrle.hdr`, `gradient_32x16_crlf_plusY.hdr`,
`flat_4x2_uncompressed.hdr`, `xyze_24x10_newrle.hdr`). Between them
they exercise every typed `KEY=VALUE` slot the decoder recognises plus
an untyped extra record, both `\n` and `\r\n` line endings, the
canonical `-Y H +X W` and the non-default `+Y H +X W` axis orders,
both `FORMAT`s (the XYZE fixture also anchors the photometric
luminance semantics — stored `Y` verbatim, scene-referred `Y ÷
EXPOSURE` — against committed bytes), and all three pixel-section
encodings the spec enumerates (new-RLE, old-RLE, uncompressed). The
matching `tests/fixture_decode.rs` integration test decodes each one,
asserts the recovered structure, and re-encodes it with byte-identity
against the committed file. Regenerate after an intentional
wire-format change with `cargo run --example gen_fixtures`.

## Performance

The full Criterion suite, measured numbers and the ranked hotspot table
live in [`BENCHMARKS.md`](BENCHMARKS.md). Three bench targets cover the
crate's hot surface end-to-end:

* [`benches/encode.rs`](benches/encode.rs) — `encode` in
  all four modes (`New`, `Old`, `Auto`, `Uncompressed`) on three
  inline-synthesised inputs.
* [`benches/decode.rs`](benches/decode.rs) — `decode` on the same
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

A `cargo-fuzz` harness under [`fuzz/`](fuzz/) ships six libFuzzer
targets covering the public decode + encode + colour-conversion surface
end-to-end. The
harness uses the standalone (`default-features = false`) build so it
never links `oxideav-core` — the targets exercise only the
framework-free `decode` / `encode` path that downstream
image-library consumers actually call.

| Target       | What it stresses                                                                                 |
|--------------|--------------------------------------------------------------------------------------------------|
| `decode`     | `probe` / `info` / `decode` (lenient + strict) / `decode_rgb8` on arbitrary bytes — every code path the reader can take on hostile input; a successful decode must agree with `info` and re-`encode`. The `DecodeOptions` default (max 32 767 × 32 767, ≤ 1 GiB plane) caps the worst-case allocation so libFuzzer doesn't OOM. |
| `roundtrip`  | Synthesise a fuzz-driven small picture, run `encode` → `decode`, assert structure survives end-to-end. Catches encoder/decoder asymmetries. |
| `headers`    | Prepend a valid `#?RADIANCE\n` magic and a minimal `-Y 1 +X 8\n` resolution line so libFuzzer's coverage gradient focuses the corpus on the text `KEY=VALUE` parse (EXPOSURE / COLORCORR / PRIMARIES floats, comment lines, mid-line `=`). |
| `pixels`     | Wrap a *fuzz-controlled pixel section* in a valid container envelope (magic + blank line + a fuzz-chosen `-Y H +X W` resolution line with small, bounded dimensions), then decode it under **both** `FallbackMode` branches. Forces the corpus straight into the new-RLE / old-RLE / uncompressed inner loops. |
| `colorconv`  | Drive the **float-domain colour pipeline** — XYZE↔RGB conversion (named-space, arbitrary-`PRIMARIES`, `_with_effective_`, and the six r383 photometric `WHTEFFICACY`-folded forms), `rgb_to_xyz_matrix_from_primaries` / `xyz_to_rgb_matrix_from_primaries` (the `3×3` inversion), `luminance_lm_per_sr_per_m2`, `adjust_exposure_factor`/`_stops` (hostile factors must be rejected without touching the buffer; a recorded `EXPOSURE=` stays finite-positive), `rgbe_shift_exponent` (i32-extreme stops), and all eight tone-mapping operators — on raw fuzz bytes reinterpreted **verbatim** as `f32`. The four byte-surface targets only feed this code floats laundered through the RGBE quantiser (finite, non-negative, bounded), so NaN / ±inf / negative / subnormal samples and degenerate chromaticity records reach the matrix inversion and per-operator transcendentals only here. Asserts every call returns without panicking and that buffer-length / `Rgb24` byte-count invariants hold. |
| `encode`     | Drive the **encoder** across the full cross-product of its on-wire options — the four `RleMode` flavours × `Lf`/`Crlf` × `#?RADIANCE`/`#?RGBE` × all eight resolution-string orientations × RGBE/XYZE `FORMAT` — each carrying a fuzz-built header with every typed record (`EXPOSURE` / `GAMMA` / `PIXASPECT` / `COLORCORR` / `PRIMARIES` / `SOFTWARE` / `VIEW` + a command line). Encodes via `encode` with every `EncodeOptions` combination, decodes with the matching `FallbackMode`, and asserts dimensions, `FORMAT`, orientation and every typed record survive. Where `roundtrip` only ever exercises the `encode` default path (RADIANCE + default header + New-RLE + `Lf`), this one reaches the header writer and the three other RLE flavours. The deterministic 256-case version of the same matrix is pinned in `tests/roundtrip.rs`. |

Run any target with:

```sh
cd fuzz
cargo +nightly fuzz run decode      # or roundtrip / headers / pixels / colorconv / encode
```

The harness is `cargo-fuzz` standard layout — `fuzz/Cargo.toml` declares
its own `[workspace]` block so the umbrella workspace never tries to
build the `nightly`-only libfuzzer dependency.

## License

MIT — see [`LICENSE`](LICENSE).
