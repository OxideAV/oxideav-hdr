//! The root vocabulary of the image-crate API contract
//! (`IMAGE_CRATE_API`): `probe` / `info` / `decode*` / `encode*`.
//!
//! Every function here is framework-free (builds with
//! `default-features = false`) and is the single implementation the
//! registry `Decoder` / `Encoder` adapters call.

use std::io::{Read, Write};

use crate::decoder;
use crate::encoder;
use crate::error::Result;
use crate::image::{HdrImage, ImageInfo, RgbImage, RgbaImage};
use crate::options::{DecodeOptions, EncodeOptions};

/// `true` when `bytes` starts with one of the two canonical magic
/// lines, `#?RADIANCE` or `#?RGBE`. Total, allocation-free, `false` on
/// short input. Says nothing about the rest of the header — see
/// [`info`]. ([`decode`] itself accepts the wider `#?<identifier>`
/// class the staged spec documents; `probe` answers for the two
/// spellings the format is recognised by.)
pub fn probe(bytes: &[u8]) -> bool {
    decoder::probe(bytes)
}

/// Header only: the parsed [`crate::HdrHeader`], display dimensions
/// from the resolution line, native [`crate::PixelFormat`]
/// (`RgbF32Le`), `frames` (always 1), alpha (never), colour derived
/// from `PRIMARIES=` / `GAMMA=`, and the (always absent) ICC / Exif /
/// XMP flags. Reads the text header and the resolution line and
/// nothing else; the pixel section may be missing or truncated.
///
/// Errors: [`crate::HdrError::InvalidData`] for a missing `#?` magic,
/// a malformed record, a missing or malformed resolution line or a
/// zero dimension; [`crate::HdrError::Unsupported`] for an unknown
/// `FORMAT=` value.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    decoder::info(bytes)
}

/// Decode a complete Radiance picture into its native layout (one
/// tightly packed `RgbF32Le` plane in display order) with
/// [`DecodeOptions::default`] (32 767 × 32 767, 1 GiB plane cap,
/// old-RLE fallback, lenient).
pub fn decode(bytes: &[u8]) -> Result<HdrImage> {
    decode_with(bytes, &DecodeOptions::default())
}

/// [`decode`] with explicit limits, strictness and marker-less
/// scanline fallback. Every limit is checked against the resolution
/// line before the plane is allocated
/// ([`crate::HdrError::LimitExceeded`]).
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<HdrImage> {
    decoder::decode_with(bytes, opts)
}

/// Decode straight to tightly packed 8-bit RGB
/// ([`HdrImage::to_rgb8`]: clamp `[0, 1]` × 255, no exposure / gamma /
/// tone curve), default limits.
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    Ok(RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// Decode straight to tightly packed 8-bit RGBA (alpha `255`), default
/// limits.
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    let img = decode(bytes)?;
    Ok(RgbaImage::new(img.width, img.height, img.to_rgba8()))
}

/// Read `r` to end and [`decode`] it. Read failures surface as
/// [`crate::HdrError::Io`].
pub fn decode_from<R: Read>(mut r: R) -> Result<HdrImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Encode `image` as a complete Radiance picture: the header
/// ([`HdrImage::header`] with the [`EncodeOptions`] record overrides),
/// the resolution line for the header's orientation, and the scanlines
/// in the chosen RLE flavour. The one native layout always encodes as
/// given (a plane with row padding is repacked). ICC / Exif / XMP
/// metadata cannot be carried and is ignored. Every geometry is
/// representable (the default `RleMode::Auto` falls back to old-RLE
/// where the new-RLE marker cannot address the scanline width);
/// [`crate::HdrError::Unsupported`] is returned only when `RleMode::New`
/// is forced at an on-disk scanline width outside `8..=32767`.
pub fn encode(image: &HdrImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    encoder::encode_image(image, opts)
}

/// Encode tightly packed 8-bit RGB (`3 × width × height` bytes) as a
/// Radiance picture. Each byte `b` becomes the linear float `b / 255`
/// — or `(b / 255) ^ g` with [`EncodeOptions::input_gamma`] = `Some(g)`
/// — and is then RGBE-quantised like any float image; the header is
/// the default (`#?RADIANCE`, `FORMAT=32-bit_rle_rgbe`, `-Y H +X W`)
/// plus the option overrides. A short buffer is
/// [`crate::HdrError::InvalidData`].
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let img = HdrImage::from_8bit(width, height, rgb, 3, opts.input_gamma)?;
    encode(&img, opts)
}

/// Encode tightly packed 8-bit RGBA (`4 × width × height` bytes) as a
/// Radiance picture. Alpha is **dropped** — Radiance has no alpha
/// mechanism; colour bytes convert as in [`encode_rgb8`].
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let img = HdrImage::from_8bit(width, height, rgba, 4, opts.input_gamma)?;
    encode(&img, opts)
}

/// [`encode`] into a writer. Write failures surface as
/// [`crate::HdrError::Io`].
pub fn encode_to<W: Write>(image: &HdrImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{ColorInfo, PixelFormat, Plane};
    use crate::{
        HdrError, HdrFormat, HdrHeader, LineEnding, MagicLine, Orientation, Primaries, RleMode,
    };

    /// A 16×4 ramp spanning several exponents, with every pixel on the
    /// RGBE grid so `decode(encode(img)) == img` holds exactly.
    fn gridded(w: u32, h: u32) -> HdrImage {
        let mut quads = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let e = 120 + ((x + y) % 20) as u8;
                let dom = 128 + ((x * 7 + y * 3) % 128) as u8;
                quads.push([dom, dom / 2 + 1, dom / 3 + 1, e]);
            }
        }
        HdrImage::from_rgbe_quads(w, h, &quads, HdrHeader::default()).unwrap()
    }

    #[test]
    fn probe_is_total_and_magic_only() {
        assert!(!probe(b""));
        assert!(!probe(b"#?"));
        assert!(!probe(b"#?RADIAN"));
        assert!(probe(b"#?RADIANCE"));
        assert!(probe(b"#?RGBE\n"));
        assert!(!probe(b"#?MYTOOL\n"));
        assert!(!probe(b"\x89PNG\r\n\x1a\n"));
        let bytes = encode(&gridded(16, 4), &EncodeOptions::default()).unwrap();
        assert!(probe(&bytes));
    }

    #[test]
    fn info_reads_header_only() {
        let mut img = gridded(16, 4);
        img.header.exposure = Some(0.5);
        img.header.primaries = Some(Primaries::SRGB);
        img.header.set_orientation(Orientation::Rotate90Cw);
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        // Header + resolution line only: cut the pixel section away.
        let res_end = {
            let blank = bytes.windows(2).position(|w| w == b"\n\n").unwrap() + 2;
            blank + bytes[blank..].iter().position(|&b| b == b'\n').unwrap() + 1
        };
        let i = info(&bytes[..res_end]).unwrap();
        assert_eq!((i.width, i.height), (16, 4));
        assert_eq!(i.format, PixelFormat::RgbF32Le);
        assert_eq!(i.frames, 1);
        assert!(!i.has_alpha);
        assert!(!i.has_icc && !i.has_exif && !i.has_xmp);
        assert_eq!(i.rgbe_format, HdrFormat::Rgbe);
        assert_eq!(i.orientation, Orientation::Rotate90Cw);
        assert_eq!(i.header.exposure, Some(0.5));
        assert_eq!(i.color, ColorInfo::linear_srgb());
        assert_eq!(i.header.magic_id.as_deref(), Some("RADIANCE"));
        // Gigantic dimensions are described, not rejected.
        let i = info(b"#?RADIANCE\n\n-Y 40000 +X 40000\n").unwrap();
        assert_eq!((i.width, i.height), (40000, 40000));
        assert!(matches!(
            info(b"#?RADIANCE\n\n-Y 1 +X 5000000000\n"),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            info(b"#?RADIANCE\n"),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(info(b"PNG"), Err(HdrError::InvalidData(_))));
    }

    #[test]
    fn decode_fills_native_layout_colour_and_metadata() {
        let img = gridded(16, 4);
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (16, 4));
        assert_eq!(back.format(), PixelFormat::RgbF32Le);
        assert_eq!(back.planes.len(), 1);
        assert_eq!(back.planes[0].stride, 16 * 12);
        assert!(back.is_tightly_packed());
        assert_eq!(back.as_bytes().unwrap(), img.as_bytes().unwrap());
        assert_eq!(back.color, ColorInfo::hdr_default());
        assert!(back.metadata.is_empty());
        assert_eq!(back.header.magic_id.as_deref(), Some("RADIANCE"));
    }

    #[test]
    fn lossless_round_trip_is_exact() {
        // decode(encode(img)) == img once the image carries the magic
        // identifier the writer emits (a decoder-produced image always
        // does).
        let mut img = gridded(24, 6);
        img.header.magic_id = Some("RADIANCE".into());
        img.header.software = Some("oxideav-hdr test".into());
        img.header.exposure = Some(2.0);
        img.header.gamma = Some(2.2);
        img.header.primaries = Some(Primaries::REC2020);
        img.header.set_orientation(Orientation::FlipX);
        img.sync_color_from_header();
        for rle in [
            RleMode::New,
            RleMode::Old,
            RleMode::Uncompressed,
            RleMode::Smallest,
        ] {
            let opts = EncodeOptions::default().with_rle(rle);
            let bytes = encode(&img, &opts).unwrap();
            let fallback = match rle {
                RleMode::Old => crate::FallbackMode::OldRle,
                _ => crate::FallbackMode::Uncompressed,
            };
            let back =
                decode_with(&bytes, &DecodeOptions::default().with_fallback(fallback)).unwrap();
            assert_eq!(back, img, "round trip drifted under {rle:?}");
        }
        // The decoded image re-encodes byte-identically (magic preserved).
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(encode(&back, &EncodeOptions::default()).unwrap(), bytes);
    }

    #[test]
    fn colour_and_gamma_derive_from_the_header() {
        let mut img = gridded(8, 1);
        img.header.gamma = Some(2.2);
        img.header.primaries = Some(Primaries::P3_D65);
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.color.primaries, ColorInfo::PRIMARIES_P3_D65);
        assert_eq!(back.color.transfer, ColorInfo::UNSPECIFIED);
        assert!((back.metadata.gamma.unwrap() - 1.0 / 2.2).abs() < 1e-6);
        // Radiance's own primaries have no code point; GAMMA=1 is linear.
        let mut img = gridded(8, 1);
        img.header.gamma = Some(1.0);
        img.header.primaries = Some(Primaries::RADIANCE);
        let back = decode(&encode(&img, &EncodeOptions::default()).unwrap()).unwrap();
        assert_eq!(back.color, ColorInfo::hdr_default());
        assert_eq!(back.metadata.gamma, None);
        // Record overrides on the encode side.
        let opts = EncodeOptions::default()
            .with_exposure(4.0)
            .with_software("sw".to_string())
            .with_primaries(Primaries::SRGB)
            .with_gamma(1.8);
        let back = decode(&encode(&gridded(8, 1), &opts).unwrap()).unwrap();
        assert_eq!(back.header.exposure, Some(4.0));
        assert_eq!(back.header.software.as_deref(), Some("sw"));
        assert_eq!(back.header.primaries, Some(Primaries::SRGB));
        assert_eq!(back.header.gamma, Some(1.8));
        assert_eq!(back.color.primaries, ColorInfo::PRIMARIES_BT709);
    }

    #[test]
    fn rgb8_and_rgba8_raw_paths() {
        let w = 8u32;
        let h = 2u32;
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 13 % 256) as u8).collect();
        let bytes = encode_rgb8(w, h, &rgb, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        // b / 255 round-trips through the RGBE grid to within one
        // mantissa step of the dominant channel.
        for (px, want) in back.pixels().chunks_exact(3).zip(rgb.chunks_exact(3)) {
            let max = want.iter().copied().max().unwrap() as f32 / 255.0;
            for c in 0..3 {
                let w8 = want[c] as f32 / 255.0;
                assert!(
                    (px[c] - w8).abs() <= max / 128.0 + 1e-6,
                    "{px:?} vs {want:?}"
                );
            }
        }
        // The 8-bit output path reproduces the input exactly for
        // values on the grid (black / white / mid).
        let flat = [0u8, 255, 128, 0, 255, 128];
        let bytes = encode_rgb8(2, 1, &flat, &EncodeOptions::default()).unwrap();
        let out = decode_rgb8(&bytes).unwrap();
        assert_eq!((out.width, out.height), (2, 1));
        assert_eq!(out.as_bytes(), &[0, 255, 128, 0, 255, 128]);
        let rgba = decode_rgba8(&bytes).unwrap();
        assert_eq!(rgba.into_raw(), vec![0, 255, 128, 255, 0, 255, 128, 255]);
        // RGBA input: alpha dropped.
        let rgba_in = [10u8, 20, 30, 7, 40, 50, 60, 0];
        let bytes = encode_rgba8(2, 1, &rgba_in, &EncodeOptions::default()).unwrap();
        let out = decode_rgb8(&bytes).unwrap();
        assert_eq!(out.as_bytes(), &[10, 20, 30, 40, 50, 60]);
        // Short buffers are InvalidData.
        assert!(matches!(
            encode_rgb8(2, 2, &flat, &EncodeOptions::default()),
            Err(HdrError::InvalidData(_))
        ));
        // input_gamma linearises: 128 → (128/255)^2.2 ≈ 0.2195.
        let bytes = encode_rgb8(
            1,
            1,
            &[128, 128, 128],
            &EncodeOptions::default().with_input_gamma(2.2),
        )
        .unwrap();
        let px = decode(&bytes).unwrap().pixel(0, 0);
        assert!(
            (px[0] - (128.0f32 / 255.0).powf(2.2)).abs() < 0.003,
            "{px:?}"
        );
    }

    #[test]
    fn to_rgb8_clamps_and_scales() {
        let img = HdrImage::from_f32(
            3,
            1,
            vec![0.0, 0.5, 1.0, 2.0, -1.0, f32::NAN, 0.25, 0.75, 0.999],
        )
        .unwrap();
        assert_eq!(img.to_rgb8(), vec![0, 128, 255, 255, 0, 0, 64, 191, 255]);
        assert_eq!(img.to_rgba8()[3], 255);
        assert_eq!(img.to_rgba8().len(), 12);
        // +1 stop doubles everything before the clamp.
        assert_eq!(&img.to_rgb8_with_exposure(1.0)[..3], &[0, 255, 255]);
        assert_eq!(&img.to_rgb8_with_exposure(-1.0)[..3], &[0, 64, 128]);
    }

    #[test]
    fn to_rgb8_converts_xyze_to_rgb_first() {
        // Equal-energy white in XYZ under Radiance primaries (E white)
        // is RGB white.
        let mut img = HdrImage::from_f32(1, 1, vec![1.0, 1.0, 1.0]).unwrap();
        img.header.format = HdrFormat::Xyze;
        let rgb = img.to_rgb8();
        for c in rgb {
            assert!(c >= 253, "{c}");
        }
    }

    #[test]
    fn constructors_validate_geometry() {
        assert!(matches!(
            HdrImage::new(0, 1, PixelFormat::RgbF32Le, vec![Plane::new(0, vec![])]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::new(1, 1, PixelFormat::RgbF32Le, vec![]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::packed(2, 1, PixelFormat::RgbF32Le, 12, vec![0; 24]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::packed(2, 2, PixelFormat::RgbF32Le, 24, vec![0; 36]),
            Err(HdrError::InvalidData(_))
        ));
        // stride*(h-1)+row is enough; padding is tolerated.
        let img = HdrImage::packed(2, 2, PixelFormat::RgbF32Le, 32, vec![0; 56]).unwrap();
        assert!(!img.is_tightly_packed());
        assert_eq!(img.pixels().len(), 12);
        assert!(matches!(
            HdrImage::from_f32(2, 2, vec![0.0; 11]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::from_rgb8(2, 2, vec![0; 11]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::from_rgba8(2, 2, vec![0; 15]),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::from_rgbe_quads(2, 2, &[[0; 4]; 3], HdrHeader::default()),
            Err(HdrError::InvalidData(_))
        ));
        assert!(matches!(
            HdrImage::from_rgb8_with_gamma(1, 1, vec![0; 3], 0.0),
            Err(HdrError::InvalidData(_))
        ));
        // A padded plane encodes like its tight repack.
        let tight = HdrImage::from_f32(2, 2, (0..12).map(|i| i as f32).collect()).unwrap();
        let mut padded = Vec::new();
        for row in tight.as_bytes().unwrap().chunks_exact(24) {
            padded.extend_from_slice(row);
            padded.extend_from_slice(&[0xAA; 8]);
        }
        let padded = HdrImage::packed(2, 2, PixelFormat::RgbF32Le, 32, padded).unwrap();
        assert_eq!(padded.pixels(), tight.pixels());
        assert_eq!(
            encode(&padded, &EncodeOptions::default()).unwrap(),
            encode(&tight, &EncodeOptions::default()).unwrap()
        );
    }

    #[test]
    fn decode_options_limits_fire_before_allocation() {
        let big = b"#?RADIANCE\n\n-Y 40000 +X 40000\n";
        assert!(matches!(decode(big), Err(HdrError::LimitExceeded(_))));
        let wide = b"#?RADIANCE\n\n-Y 1 +X 2000000000\n";
        assert!(matches!(decode(wide), Err(HdrError::LimitExceeded(_))));
        let small = b"#?RADIANCE\n\n-Y 4 +X 32\n";
        let tight = DecodeOptions::default().with_max_bytes(1024u64);
        assert!(matches!(
            decode_with(small, &tight),
            Err(HdrError::LimitExceeded(_))
        ));
        let px = DecodeOptions::default().with_max_pixels(100u64);
        assert!(matches!(
            decode_with(small, &px),
            Err(HdrError::LimitExceeded(_))
        ));
        // Past the gate the missing pixel section is InvalidData, not a
        // limit error.
        let err = decode_with(big, &DecodeOptions::default().unlimited()).unwrap_err();
        assert!(matches!(err, HdrError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn strict_rejects_what_lenient_tolerates() {
        let img = gridded(8, 1);
        let strict = DecodeOptions::default().with_strict(true);
        // Canonical output passes strict.
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        assert!(decode_with(&bytes, &strict).is_ok());
        // Custom magic identifier.
        let custom = encode(
            &img,
            &EncodeOptions::default().with_magic(MagicLine::Custom("MYTOOL".into())),
        )
        .unwrap();
        assert!(decode(&custom).is_ok());
        assert!(matches!(
            decode_with(&custom, &strict),
            Err(HdrError::InvalidData(_))
        ));
        // Trailing bytes.
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b"junk");
        assert!(decode(&trailing).is_ok());
        assert!(matches!(
            decode_with(&trailing, &strict),
            Err(HdrError::InvalidData(_))
        ));
        // Missing FORMAT record: drop the line from the text header.
        let text_end = bytes.windows(2).position(|w| w == b"\n\n").unwrap() + 2;
        let text = std::str::from_utf8(&bytes[..text_end]).unwrap();
        let stripped: String = text
            .split_inclusive('\n')
            .filter(|l| !l.starts_with("FORMAT="))
            .collect();
        let mut no_format = stripped.into_bytes();
        no_format.extend_from_slice(&bytes[text_end..]);
        assert!(decode(&no_format).is_ok());
        assert!(matches!(
            decode_with(&no_format, &strict),
            Err(HdrError::InvalidData(_))
        ));
    }

    #[test]
    fn io_paths_wrap_std_io() {
        let img = gridded(8, 1);
        let mut buf = Vec::new();
        encode_to(
            &img,
            &EncodeOptions::default().with_line_ending(LineEnding::Crlf),
            &mut buf,
        )
        .unwrap();
        assert!(buf.starts_with(b"#?RADIANCE\r\n"));
        let back = decode_from(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(back.as_bytes(), img.as_bytes());
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        assert!(matches!(decode_from(Failing), Err(HdrError::Io(_))));
        struct Sink;
        impl Write for Sink {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            encode_to(&img, &EncodeOptions::default(), Sink),
            Err(HdrError::Io(_))
        ));
        let e: HdrError = std::io::Error::other("x").into();
        assert!(e.to_string().contains("io error"));
    }

    #[test]
    fn hostile_inputs_never_panic() {
        let probes: [&[u8]; 8] = [
            b"",
            b"#?",
            b"#?RADIANCE",
            b"#?RADIANCE\n",
            b"#?RADIANCE\n\n",
            b"#?RADIANCE\n\n-Y 1 +X 1\n",
            b"#?RADIANCE\n\n-Y 1 +X 8\n\x02\x02\x00\x08",
            b"#?RADIANCE\nFORMAT=bogus\n\n-Y 1 +X 1\n\x80\x80\x80\x81",
        ];
        for p in probes {
            let _ = probe(p);
            let _ = info(p);
            let _ = decode(p);
            let _ = decode_rgb8(p);
        }
    }
}
