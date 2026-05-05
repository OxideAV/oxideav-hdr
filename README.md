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

## Coverage (round 1)

| Feature                      | Read | Write |
|------------------------------|:----:|:-----:|
| `#?RADIANCE` / `#?RGBE` magic|  Y   |   Y   |
| `KEY=VALUE` header records   |  Y   |   Y   |
| All 8 axis-flag combinations |  Y   | `-Y H +X W` only |
| 32-bit_rle_rgbe pixels       |  Y   |   Y   |
| 32-bit_rle_xyze pixels       |  Y   |   Y (no colour conversion) |
| New RLE (`0x02 0x02 hi lo`)  |  Y   |   Y   |
| Old RLE (sentinel pixels)    |  Y   |   N   |
| CRLF line endings            |  Y   |   N   |

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
