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

## License

MIT — see [`LICENSE`](LICENSE).
