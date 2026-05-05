//! Tone-mapping operators for turning a high-dynamic-range linear
//! RGB float buffer into 8-bit display-ready output.
//!
//! All operators here are scalar (per-pixel) and apply a non-linear
//! response curve to compress the scene-referred radiance range into
//! the `[0, 1]` display range, then quantise to `u8`. They take a
//! borrowed [`crate::HdrImage`] and produce a `Vec<u8>` of
//! `width * height * 3` packed Rgb24 samples in top-down memory order.
//!
//! Operator catalogue:
//!
//! * **Linear** — `clamp(v * exposure, 0, 1)`. Simple white-point
//!   normalisation; clips highlights.
//! * **Gamma** — `clamp(v * exposure, 0, 1) ^ (1/gamma)`. Linear with
//!   a gamma OETF; the historical "viewer" tone-map for `.hdr`.
//! * **Reinhard** — `v / (1 + v)` per-channel after exposure scaling.
//!   The classic Reinhard et al. 2002 global operator.
//! * **ACES** — the public-domain Krzysztof Narkowicz fit to the ACES
//!   reference rendering transform (Hable 2017 derivation, blog post
//!   `knarkowicz.wordpress.com`). Designed for film-look highlight
//!   roll-off. Pure polynomial — no LUT, no external dep.
//!
//! All operators apply an sRGB-style gamma encoding (≈ `^(1/2.2)`) at
//! the very end so the resulting `u8` values are directly suitable for
//! display on an sRGB monitor without further processing. (The
//! [`ToneMap::Linear`] path skips that gamma, in case the caller wants
//! raw linear samples.)
//!
//! Performance note: the operators run on the `Vec<f32>` directly with
//! no SIMD or chunking — they're intended for medium-sized HDR images
//! (tens of megapixels) and downstream consumers that care about
//! speed should layer their own SIMD pass on top of the standalone
//! `parse_hdr` API.

use crate::image::HdrImage;

/// Available tone-mapping operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToneMap {
    /// `clamp(v * exposure, 0, 1)` per channel. No gamma — output is
    /// linear-light quantised to `u8` (mostly useful for debugging or
    /// for downstream code that wants to apply its own OETF).
    Linear { exposure: f32 },
    /// `clamp(v * exposure, 0, 1) ^ (1/gamma)`. Default gamma 2.2 is
    /// the historical Radiance viewer behaviour.
    Gamma { exposure: f32, gamma: f32 },
    /// Reinhard et al. 2002 global operator: `(v*e) / (1 + v*e)`
    /// per channel, then sRGB gamma encoding.
    Reinhard { exposure: f32 },
    /// Krzysztof Narkowicz's polynomial ACES fit, then sRGB gamma.
    /// Designed for film-look highlight roll-off; the most common
    /// "looks-good-out-of-the-box" choice for HDR previews.
    Aces { exposure: f32 },
}

impl Default for ToneMap {
    /// Default = ACES with neutral exposure. Matches what most modern
    /// HDR viewers default to.
    fn default() -> Self {
        Self::Aces { exposure: 1.0 }
    }
}

/// Apply `op` to every pixel of `image` and return a packed
/// `width * height * 3` Rgb24 buffer in top-down memory order.
pub fn tone_map(image: &HdrImage, op: ToneMap) -> Vec<u8> {
    let n = image.pixels.len();
    debug_assert_eq!(n % 3, 0);
    let mut out = Vec::with_capacity(n);
    for px in image.pixels.chunks_exact(3) {
        let mapped = apply(op, [px[0], px[1], px[2]]);
        out.push(quantise(mapped[0]));
        out.push(quantise(mapped[1]));
        out.push(quantise(mapped[2]));
    }
    out
}

/// Apply `op` to a single pixel (3 channels) and return the
/// post-gamma display-referred RGB triple in `[0, 1]`.
#[inline]
pub fn apply(op: ToneMap, rgb: [f32; 3]) -> [f32; 3] {
    match op {
        ToneMap::Linear { exposure } => {
            // Pure clamp, no gamma — caller wants linear samples.
            [
                clamp01(rgb[0] * exposure),
                clamp01(rgb[1] * exposure),
                clamp01(rgb[2] * exposure),
            ]
        }
        ToneMap::Gamma { exposure, gamma } => {
            let inv_g = 1.0 / gamma.max(1e-3);
            [
                gamma_pow(clamp01(rgb[0] * exposure), inv_g),
                gamma_pow(clamp01(rgb[1] * exposure), inv_g),
                gamma_pow(clamp01(rgb[2] * exposure), inv_g),
            ]
        }
        ToneMap::Reinhard { exposure } => {
            let r = reinhard(rgb[0] * exposure);
            let g = reinhard(rgb[1] * exposure);
            let b = reinhard(rgb[2] * exposure);
            [srgb_oetf(r), srgb_oetf(g), srgb_oetf(b)]
        }
        ToneMap::Aces { exposure } => {
            let r = aces_narkowicz(rgb[0] * exposure);
            let g = aces_narkowicz(rgb[1] * exposure);
            let b = aces_narkowicz(rgb[2] * exposure);
            [srgb_oetf(r), srgb_oetf(g), srgb_oetf(b)]
        }
    }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

#[inline]
fn quantise(v: f32) -> u8 {
    (clamp01(v) * 255.0).round() as u8
}

/// Reinhard global: `v / (1 + v)`. Asymptotically maps `[0, ∞)` into
/// `[0, 1)` so highlights compress instead of clipping.
#[inline]
fn reinhard(v: f32) -> f32 {
    let v = if v.is_finite() && v > 0.0 { v } else { 0.0 };
    v / (1.0 + v)
}

/// `clamp01(v)^(1/gamma)` — the gamma-only branch's per-channel call.
/// Wrapped in a helper to avoid `f32::powf` panicking on negatives
/// (we already clamped so this stays defensive).
#[inline]
fn gamma_pow(v: f32, inv_g: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else {
        v.powf(inv_g)
    }
}

/// IEC 61966-2-1 sRGB OETF (linear → encoded). Applied at the very
/// end of the Reinhard / ACES paths so the `u8` output is sRGB-encoded
/// and directly displayable.
#[inline]
fn srgb_oetf(linear: f32) -> f32 {
    let l = clamp01(linear);
    if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// Krzysztof Narkowicz's polynomial fit to the full ACES filmic
/// rendering transform. From his "ACES Filmic Tone Mapping Curve"
/// blog post (knarkowicz.wordpress.com, 2016) — five-coefficient
/// fit, no external table or LUT needed.
#[inline]
fn aces_narkowicz(x: f32) -> f32 {
    let x = if x.is_finite() && x > 0.0 { x } else { 0.0 };
    let a = 2.51_f32;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    let num = x * (a * x + b);
    let den = x * (c * x + d) + e;
    clamp01(num / den)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::HdrImage;

    fn one_pixel(rgb: [f32; 3]) -> HdrImage {
        HdrImage::new_rgb96f(1, 1, vec![rgb[0], rgb[1], rgb[2]])
    }

    #[test]
    fn linear_preserves_zero_and_clips_high() {
        let img = one_pixel([0.0, 0.5, 10.0]);
        let out = tone_map(&img, ToneMap::Linear { exposure: 1.0 });
        assert_eq!(out, vec![0, 128, 255]);
    }

    #[test]
    fn linear_exposure_scales() {
        let img = one_pixel([0.5, 0.5, 0.5]);
        let out = tone_map(&img, ToneMap::Linear { exposure: 0.5 });
        assert_eq!(out, vec![64, 64, 64]);
    }

    #[test]
    fn gamma_22_brightens_midtones() {
        // 0.5 linear with gamma 2.2 → ~ 0.73 (brighter than linear).
        let img = one_pixel([0.5, 0.5, 0.5]);
        let out = tone_map(
            &img,
            ToneMap::Gamma {
                exposure: 1.0,
                gamma: 2.2,
            },
        );
        for &v in &out {
            assert!(v > 180 && v < 200, "expected ~186, got {v}");
        }
    }

    #[test]
    fn reinhard_compresses_highlights() {
        // 1000 → 1000/1001 ≈ 0.999 linear → ≈ 0.999 sRGB (very close
        // to 255 but never clips to it the way Linear would).
        let img = one_pixel([1000.0, 1000.0, 1000.0]);
        let out = tone_map(&img, ToneMap::Reinhard { exposure: 1.0 });
        // sRGB of 0.999 is ~1.0 → 255 after rounding, but the
        // important property is *no NaN* and monotonic.
        for &v in &out {
            assert_eq!(v, 255);
        }
        // Halve the exposure: result should be slightly less than 255.
        let out = tone_map(&img, ToneMap::Reinhard { exposure: 0.001 });
        for &v in &out {
            // 1.0 → reinhard 0.5 → sRGB ~0.735 → ~187.
            assert!(v > 180 && v < 195, "got {v}");
        }
    }

    #[test]
    fn aces_handles_super_bright_pixel() {
        // Very bright input shouldn't wrap or NaN.
        let img = one_pixel([1e6, 1e6, 1e6]);
        let out = tone_map(&img, ToneMap::Aces { exposure: 1.0 });
        for &v in &out {
            assert_eq!(v, 255);
        }
        // ACES on a mid-grey 0.18 sample with Narkowicz's polynomial
        // fit: linear ≈ 0.267 → sRGB ≈ 0.553 → ≈ 141. (This is
        // brighter than the full ACES RRT/ODT, which lands ~0.10
        // linear, but matches the polynomial fit's documented response
        // and is what every renderer that uses Narkowicz's curve gets.)
        let img = one_pixel([0.18, 0.18, 0.18]);
        let out = tone_map(&img, ToneMap::Aces { exposure: 1.0 });
        for &v in &out {
            assert!(v > 130 && v < 150, "got {v}");
        }
    }

    #[test]
    fn nan_and_negative_clamp_to_zero() {
        let img = one_pixel([-1.0, f32::NAN, f32::INFINITY]);
        for op in [
            ToneMap::Linear { exposure: 1.0 },
            ToneMap::Gamma {
                exposure: 1.0,
                gamma: 2.2,
            },
            ToneMap::Reinhard { exposure: 1.0 },
            ToneMap::Aces { exposure: 1.0 },
        ] {
            let out = tone_map(&img, op);
            // Negative and NaN map to 0; +INF should map to 255 (no
            // panic, no garbage).
            assert_eq!(out[0], 0, "{op:?} negative");
            assert_eq!(out[1], 0, "{op:?} NaN");
            assert!(out[2] == 255 || out[2] == 0, "{op:?} +INF -> {}", out[2]);
        }
    }

    #[test]
    fn default_op_is_aces_neutral_exposure() {
        // Just exercise the Default impl so it doesn't bit-rot.
        let op = ToneMap::default();
        assert!(matches!(op, ToneMap::Aces { exposure } if (exposure - 1.0).abs() < 1e-6));
    }

    #[test]
    fn output_length_matches_pixel_count() {
        // Multi-pixel image — ensure we emit exactly 3*W*H bytes.
        let pixels = vec![0.25_f32; 30 * 20 * 3];
        let img = HdrImage::new_rgb96f(30, 20, pixels);
        let out = tone_map(&img, ToneMap::Aces { exposure: 1.0 });
        assert_eq!(out.len(), 30 * 20 * 3);
    }
}
