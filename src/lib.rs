//! Pure-Rust Radiance RGBE (`.hdr` / `.pic`) reader and writer.
//!
//! Greg Ward's shared-exponent floating-point image format from "Real
//! Pixels" (Graphics Gems II, 1991), as documented in the
//! `radsite.lbl.gov` Radiance reference manual.
//!
//! The on-disk file is:
//! 1. The magic line `#?RADIANCE` (or the older `#?RGBE`).
//! 2. Zero or more `KEY=VALUE` text records (FORMAT, EXPOSURE, GAMMA,
//!    SOFTWARE, PIXASPECT, plus any caller-stashed extras), terminated
//!    by an empty line.
//! 3. A resolution line listing the row count and column count with
//!    axis-direction flags, e.g. `-Y 1024 +X 1280`.
//! 4. `height` scanlines of new-RLE-coded RGBE pixels (or, for very
//!    old files, individual 4-byte pixels with sentinel-pixel old-RLE
//!    runs — which we still read).
//!
//! Each pixel decodes to four bytes (R mantissa, G mantissa, B
//! mantissa, shared exponent biased by 128) and reconstructs into
//! three `f32` channels via `(mantissa / 256) * 2^(exponent - 128)`.
//!
//! ## Standalone use (the image-crate API contract)
//!
//! The crate root follows the OxideAV image-crate API contract
//! (`IMAGE_CRATE_API`): [`probe`], [`info`], [`decode`] /
//! [`decode_with`] → [`HdrImage`] (one packed `RgbF32Le` plane),
//! [`decode_rgb8`] / [`decode_rgba8`] → [`RgbImage`] / [`RgbaImage`],
//! [`decode_from`], [`encode`] / [`encode_rgb8`] / [`encode_rgba8`] /
//! [`encode_to`] with [`EncodeOptions`] and [`DecodeOptions`].
//!
//! ```no_run
//! let bytes = std::fs::read("in.hdr")?;
//! if oxideav_hdr::probe(&bytes) {
//!     let info = oxideav_hdr::info(&bytes)?;          // header only
//!     let img = oxideav_hdr::decode(&bytes)?;         // HdrImage, RgbF32Le
//!     let rgb8: Vec<u8> = img.to_rgb8();              // clamp [0, 1] × 255
//!     let floats: Vec<f32> = img.pixels();            // linear radiance
//!     let (w, h) = (img.width(), img.height());
//!     assert_eq!((w, h), (info.width, info.height));
//!     let out = oxideav_hdr::encode(&img, &oxideav_hdr::EncodeOptions::default())?;
//!     std::fs::write("out.hdr", out)?;
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` / `Encoder` trait surface plus
//! the [`register`] entry point and the `HdrImage` ⇄ `VideoFrame`
//! bridge. Disable the feature (`default-features = false`) for an
//! `oxideav-core`-free build that still exposes the whole standalone
//! API above plus the depth helpers ([`tone_map`], the `xyz` module,
//! the header / orientation / exposure helpers on [`HdrImage`]).

pub mod api;
#[cfg(feature = "registry")]
pub mod container;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod header;
pub mod image;
pub mod limits;
pub mod options;
#[cfg(feature = "registry")]
pub mod registry;
pub mod rgbe;
pub mod rle;
pub mod tonemap;
pub mod xyz;

/// Codec id for HDR image frames.
pub const CODEC_ID_STR: &str = "hdr";

// --- the contract vocabulary -------------------------------------------------
pub use api::{
    decode, decode_from, decode_rgb8, decode_rgba8, decode_with, encode, encode_rgb8, encode_rgba8,
    encode_to, info, probe,
};
pub use error::{Error, HdrError, Result};
pub use image::{
    ColorInfo, ColorRange, HdrImage, HdrPixelFormat, ImageInfo, Metadata, PixelFormat, Plane,
    RgbImage, RgbaImage,
};
pub use options::{DecodeOptions, EncodeOptions};

// --- pre-contract entry points (deprecated wrappers, one release) -----------
#[cfg(feature = "registry")]
#[allow(deprecated)]
pub use decoder::parse_hdr_videoframe;
#[allow(deprecated)]
pub use decoder::{
    parse_hdr, parse_hdr_with_limits, parse_hdr_with_options, parse_hdr_with_options_and_limits,
};
#[allow(deprecated)]
pub use encoder::{
    encode_hdr, encode_hdr_preserving_magic, encode_hdr_rgb96f, encode_hdr_with_full_options,
    encode_hdr_with_options, encode_hdr_with_rle,
};
#[allow(deprecated)]
pub use limits::HdrLimits;

// --- format depth ------------------------------------------------------------
pub use encoder::{LineEnding, MagicLine, RleMode};
pub use header::{AxisSign, GeometricOp, HdrFormat, HdrHeader, Orientation, Primaries};
pub use rgbe::{
    rgb_to_rgbe, rgbe_channel_scale, rgbe_is_zero_pixel, rgbe_shift_exponent, rgbe_to_rgb,
    rgbe_unbiased_exponent,
};
pub use rle::FallbackMode;
pub use tonemap::{tone_map, ToneMap};
pub use xyz::{
    convert_image_rgb_to_xyz, convert_image_rgb_to_xyz_photometric,
    convert_image_rgb_to_xyz_photometric_with_effective_primaries,
    convert_image_rgb_to_xyz_photometric_with_primaries,
    convert_image_rgb_to_xyz_with_effective_primaries, convert_image_rgb_to_xyz_with_primaries,
    convert_image_xyz_to_rgb, convert_image_xyz_to_rgb_photometric,
    convert_image_xyz_to_rgb_photometric_with_effective_primaries,
    convert_image_xyz_to_rgb_photometric_with_primaries,
    convert_image_xyz_to_rgb_with_effective_primaries, convert_image_xyz_to_rgb_with_primaries,
    luminance_lm_per_sr_per_m2, rgb_to_xyz, rgb_to_xyz_matrix, rgb_to_xyz_matrix_from_primaries,
    xyz_to_rgb, xyz_to_rgb_matrix, xyz_to_rgb_matrix_from_primaries, RgbColorSpace,
    RGBE_BRIGHT_COEFFS, WHTEFFICACY,
};

// --- framework integration -----------------------------------------------------
#[cfg(feature = "registry")]
pub use decoder::make_decoder;
#[cfg(feature = "registry")]
pub use encoder::make_encoder;
#[cfg(feature = "registry")]
#[doc(hidden)]
pub use registry::__oxideav_entry;
#[cfg(feature = "registry")]
#[allow(deprecated)]
pub use registry::register_runtime;
#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers, register_registries};

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a smooth gradient HDR image — radiance ramps from `1e-3`
    /// in the top-left corner up to `1e3` in the bottom-right with a
    /// soft per-channel weighting so each row exercises the
    /// shared-exponent encoder at a different magnitude.
    fn synthetic_gradient(w: u32, h: u32) -> HdrImage {
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                // Magnitude spans ~6 decades.
                let mag = 1e-3_f32 * 10.0_f32.powf(6.0 * (u + v) * 0.5);
                pixels.push(mag);
                pixels.push(mag * 0.5);
                pixels.push(mag * 0.25);
            }
        }
        HdrImage::from_f32(w, h, pixels).unwrap()
    }

    #[test]
    fn gradient_self_roundtrip() {
        // Width must be in 8..=32767 for the new-RLE path.
        let src = synthetic_gradient(32, 16);
        let bytes = encode(&src, &EncodeOptions::default()).unwrap();
        // Magic line should be the first thing on the wire.
        assert!(bytes.starts_with(b"#?RADIANCE\n"));
        let back = decode(&bytes).unwrap();
        assert_eq!(back.width, src.width);
        assert_eq!(back.height, src.height);
        for i in 0..src.pixels().len() {
            let a = src.pixels()[i];
            let b = back.pixels()[i];
            // Shared-mantissa quantisation: ~1/128 of the channel of
            // largest magnitude in the same pixel. We allow either
            // 1.5% relative error OR an absolute error within one
            // mantissa step of the dominant channel — a small channel
            // sharing the exponent of a large neighbour can be off by
            // up to ~max/256 in absolute terms.
            let pixel = i / 3;
            let pmax = src.pixels()[pixel * 3..pixel * 3 + 3]
                .iter()
                .fold(0.0_f32, |m, v| m.max(v.abs()));
            let abs_err = (a - b).abs();
            let rel_err = abs_err / a.max(1e-30);
            assert!(
                rel_err < 0.015 || abs_err < pmax / 128.0,
                "pixel {i}: {a} vs {b} (rel={rel_err}, abs={abs_err}, pmax={pmax})"
            );
        }
    }

    #[test]
    fn rejects_missing_magic() {
        let bytes = b"NOT A RADIANCE FILE\n\n-Y 10 +X 10\n";
        assert!(decode(bytes).is_err());
    }

    #[test]
    fn rejects_zero_dimensions() {
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 0 +X 8\n";
        assert!(decode(bytes).is_err());
    }

    #[test]
    fn parses_extra_header_records() {
        // Build an 8×1 image, encode, then re-decode and assert the
        // extra record survives.
        let mut img = synthetic_gradient(8, 1);
        img.header.exposure = Some(0.7);
        img.header.gamma = Some(2.2);
        img.header.other.push(("OXIDEAV".into(), "round1".into()));
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.header.exposure, Some(0.7));
        assert_eq!(back.header.gamma, Some(2.2));
        assert!(back
            .header
            .other
            .iter()
            .any(|(k, v)| k == "OXIDEAV" && v == "round1"));
    }

    #[test]
    fn solid_colour_roundtrips_via_repeat_run() {
        // A solid colour is the worst-case for the literal path — make
        // sure the repeat path actually fires (we should encode each
        // channel as one repeat run + tail).
        let w = 64;
        let h = 4;
        let mut pixels = vec![0.0_f32; w * h * 3];
        for i in 0..w * h {
            pixels[i * 3] = 0.50;
            pixels[i * 3 + 1] = 0.25;
            pixels[i * 3 + 2] = 0.10;
        }
        let img = HdrImage::from_f32(w as u32, h as u32, pixels.clone()).unwrap();
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        // Crude size sanity check — 4 channels × 64 px × 4 rows in
        // literals would be > 1024 bytes; with repeats it should be
        // far less.
        let approx_payload = bytes.len() as i64 - 64; // header is ~30-50 bytes
        assert!(
            approx_payload < 200,
            "solid-colour payload is {approx_payload} bytes — repeat-run path likely broken"
        );
        let back = decode(&bytes).unwrap();
        for (i, (a, b)) in pixels.iter().zip(back.pixels().iter()).enumerate() {
            let err = (a - b).abs();
            assert!(err < 0.01, "pixel {i}: {a} vs {b}");
        }
    }

    #[test]
    fn axis_flag_roundtrip_through_default_writer() {
        // Encoder always emits `-Y H +X W`; decoder should see
        // y_sign=Decreasing, x_sign=Increasing on the way back.
        let img = synthetic_gradient(8, 4);
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.header.y_sign, header::AxisSign::Decreasing);
        assert_eq!(back.header.x_sign, header::AxisSign::Increasing);
        assert!(!back.header.x_first);
    }
}

/// The pre-contract entry points must keep working for the one release
/// they are kept; exercised here so a regression shows up before a
/// consumer hits it.
#[cfg(test)]
#[allow(deprecated)]
mod deprecated_wrappers {
    use super::*;

    fn img() -> HdrImage {
        let quads: Vec<[u8; 4]> = (0..32).map(|i| [128 + i as u8, 64, 32, 129]).collect();
        HdrImage::from_rgbe_quads(16, 2, &quads, HdrHeader::default()).unwrap()
    }

    #[test]
    fn parse_and_encode_wrappers_match_the_contract_functions() {
        let i = img();
        let bytes = encode_hdr(&i).unwrap();
        assert_eq!(
            bytes,
            encode(&i, &EncodeOptions::default().with_rle(RleMode::New)).unwrap()
        );
        assert_eq!(parse_hdr(&bytes).unwrap(), decode(&bytes).unwrap());
        assert_eq!(
            parse_hdr_with_limits(&bytes, &HdrLimits::default()).unwrap(),
            decode(&bytes).unwrap()
        );
        assert_eq!(
            parse_hdr_with_options(&bytes, FallbackMode::Uncompressed).unwrap(),
            decode(&bytes).unwrap()
        );
        assert_eq!(
            parse_hdr_with_options_and_limits(
                &bytes,
                FallbackMode::OldRle,
                &HdrLimits::unbounded()
            )
            .unwrap(),
            decode(&bytes).unwrap()
        );
        assert_eq!(
            encode_hdr_with_rle(&i, RleMode::Uncompressed).unwrap(),
            encode(
                &i,
                &EncodeOptions::default().with_rle(RleMode::Uncompressed)
            )
            .unwrap()
        );
        assert_eq!(
            encode_hdr_with_options(&i, RleMode::New, LineEnding::Crlf).unwrap(),
            encode(
                &i,
                &EncodeOptions::default()
                    .with_rle(RleMode::New)
                    .with_line_ending(LineEnding::Crlf)
            )
            .unwrap()
        );
        let rgbe = encode_hdr_with_full_options(&i, RleMode::New, LineEnding::Lf, MagicLine::Rgbe)
            .unwrap();
        assert!(rgbe.starts_with(b"#?RGBE\n"));
        let back = parse_hdr(&rgbe).unwrap();
        // The preserving wrapper keeps `#?RGBE`; the plain one rewrites.
        assert!(
            encode_hdr_preserving_magic(&back, RleMode::New, LineEnding::Lf)
                .unwrap()
                .starts_with(b"#?RGBE\n")
        );
        assert!(encode_hdr(&back).unwrap().starts_with(b"#?RADIANCE\n"));
        let raw = encode_hdr_rgb96f(16, 2, i.pixels(), HdrHeader::default()).unwrap();
        assert_eq!(raw, bytes);
        let legacy = HdrImage::new_rgb96f(16, 2, i.pixels());
        assert_eq!(legacy.as_bytes(), i.as_bytes());
        assert_eq!(legacy.pixel_format(), HdrPixelFormat::Rgb96f);
        assert_eq!(HdrPixelFormat::Rgb96f, HdrPixelFormat::RgbF32Le);
        assert!(matches!(
            HdrError::too_large("x"),
            HdrError::LimitExceeded(_)
        ));
    }

    #[cfg(feature = "registry")]
    #[test]
    fn registry_wrappers_still_install() {
        let mut ctx = oxideav_core::RuntimeContext::new();
        register_runtime(&mut ctx);
        assert!(ctx.codecs.decoder_ids().next().is_some());
        let frame = parse_hdr_videoframe(&encode_hdr(&img()).unwrap()).unwrap();
        assert_eq!(frame.image_planes()[0].stride, 16 * 12);
    }
}
