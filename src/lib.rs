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
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` / `Encoder` trait surface plus
//! a [`registry::register`] entry point. Disable the feature
//! (`default-features = false`) for an `oxideav-core`-free build that
//! still exposes the standalone [`parse_hdr`] / [`encode_hdr`] API
//! plus crate-local [`HdrImage`] / [`HdrPixelFormat`] / [`HdrError`]
//! types.

#[cfg(feature = "registry")]
pub mod container;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod header;
pub mod image;
pub mod limits;
#[cfg(feature = "registry")]
pub mod registry;
pub mod rgbe;
pub mod rle;
pub mod tonemap;
pub mod xyz;

/// Codec id for HDR image frames.
pub const CODEC_ID_STR: &str = "hdr";

#[cfg(feature = "registry")]
pub use decoder::parse_hdr_videoframe;
pub use decoder::{
    parse_hdr, parse_hdr_with_limits, parse_hdr_with_options, parse_hdr_with_options_and_limits,
};
pub use encoder::{
    encode_hdr, encode_hdr_rgb96f, encode_hdr_with_options, encode_hdr_with_rle, LineEnding,
    RleMode,
};
pub use error::{HdrError, Result};
pub use header::{AxisSign, HdrFormat, HdrHeader, Primaries};
pub use image::{HdrImage, HdrPixelFormat};
pub use limits::HdrLimits;
pub use rgbe::{rgb_to_rgbe, rgbe_to_rgb};
pub use rle::FallbackMode;
pub use tonemap::{tone_map, ToneMap};
pub use xyz::{
    convert_image_rgb_to_xyz, convert_image_xyz_to_rgb, luminance_lm_per_sr_per_m2, rgb_to_xyz,
    rgb_to_xyz_matrix, xyz_to_rgb, xyz_to_rgb_matrix, RgbColorSpace, RGBE_BRIGHT_COEFFS,
    WHTEFFICACY,
};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};

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
        HdrImage::new_rgb96f(w, h, pixels)
    }

    #[test]
    fn gradient_self_roundtrip() {
        // Width must be in 8..=32767 for the new-RLE path.
        let src = synthetic_gradient(32, 16);
        let bytes = encode_hdr(&src).unwrap();
        // Magic line should be the first thing on the wire.
        assert!(bytes.starts_with(b"#?RADIANCE\n"));
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(back.width, src.width);
        assert_eq!(back.height, src.height);
        for i in 0..src.pixels.len() {
            let a = src.pixels[i];
            let b = back.pixels[i];
            // Shared-mantissa quantisation: ~1/128 of the channel of
            // largest magnitude in the same pixel. We allow either
            // 1.5% relative error OR an absolute error within one
            // mantissa step of the dominant channel — a small channel
            // sharing the exponent of a large neighbour can be off by
            // up to ~max/256 in absolute terms.
            let pixel = i / 3;
            let pmax = src.pixels[pixel * 3..pixel * 3 + 3]
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
        assert!(parse_hdr(bytes).is_err());
    }

    #[test]
    fn rejects_zero_dimensions() {
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 0 +X 8\n";
        assert!(parse_hdr(bytes).is_err());
    }

    #[test]
    fn parses_extra_header_records() {
        // Build an 8×1 image, encode, then re-decode and assert the
        // extra record survives.
        let mut img = synthetic_gradient(8, 1);
        img.header.exposure = Some(0.7);
        img.header.gamma = Some(2.2);
        img.header.other.push(("OXIDEAV".into(), "round1".into()));
        let bytes = encode_hdr(&img).unwrap();
        let back = parse_hdr(&bytes).unwrap();
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
        let img = HdrImage::new_rgb96f(w as u32, h as u32, pixels.clone());
        let bytes = encode_hdr(&img).unwrap();
        // Crude size sanity check — 4 channels × 64 px × 4 rows in
        // literals would be > 1024 bytes; with repeats it should be
        // far less.
        let approx_payload = bytes.len() as i64 - 64; // header is ~30-50 bytes
        assert!(
            approx_payload < 200,
            "solid-colour payload is {approx_payload} bytes — repeat-run path likely broken"
        );
        let back = parse_hdr(&bytes).unwrap();
        for (i, (a, b)) in pixels.iter().zip(back.pixels.iter()).enumerate() {
            let err = (a - b).abs();
            assert!(err < 0.01, "pixel {i}: {a} vs {b}");
        }
    }

    #[test]
    fn axis_flag_roundtrip_through_default_writer() {
        // Encoder always emits `-Y H +X W`; decoder should see
        // y_sign=Decreasing, x_sign=Increasing on the way back.
        let img = synthetic_gradient(8, 4);
        let bytes = encode_hdr(&img).unwrap();
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(back.header.y_sign, header::AxisSign::Decreasing);
        assert_eq!(back.header.x_sign, header::AxisSign::Increasing);
        assert!(!back.header.x_first);
    }
}
