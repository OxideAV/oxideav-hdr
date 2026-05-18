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
//! * **ReinhardExtended** — Reinhard's modified operator
//!   `v * (1 + v/Lwhite^2) / (1 + v)`, which lets very-bright samples
//!   actually reach 1.0 instead of asymptoting to it. From Reinhard,
//!   Stark, Shirley, Ferwerda, "Photographic Tone Reproduction for
//!   Digital Images" (ACM ToG 2002) §3.1.
//! * **Hable** — John Hable's "Uncharted 2" filmic curve, derivation
//!   published at GDC 2010 ("Uncharted 2: HDR Lighting"). Five-knot
//!   rational function with a `linear_white` normalisation. Designed
//!   for game-style filmic response with crisp shadows and rolled-off
//!   highlights.
//! * **Drago** — Drago, Myszkowski, Annen, Chiba, "Adaptive Logarithmic
//!   Mapping For Displaying High Contrast Scenes" (EUROGRAPHICS 2003).
//!   Bias-controlled `log_{base(Lw_max)}` mapping that adapts the
//!   compression to the scene's maximum luminance for a perceptually
//!   uniform response across orders of magnitude.
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
    /// Modified Reinhard with a `white_point` saturation — `(v *
    /// (1 + v/white²)) / (1 + v)`. Lets very-bright samples actually
    /// reach 1.0 (where the unmodified Reinhard asymptotes from below).
    ReinhardExtended {
        exposure: f32,
        /// Luminance value that maps to 1.0 (display white).
        white_point: f32,
    },
    /// John Hable's "Uncharted 2" filmic curve, GDC 2010. Five-knot
    /// rational function with a `linear_white` normalisation.
    Hable {
        exposure: f32,
        /// Brightness of the white point used to normalise the curve.
        /// Hable's GDC 2010 default is `11.2`.
        linear_white: f32,
    },
    /// Drago, Myszkowski, Annen, Chiba (EUROGRAPHICS 2003) adaptive
    /// logarithmic operator. The `bias` parameter (0..=1) controls
    /// shadow/highlight balance; Drago's recommended default is `0.85`.
    Drago {
        exposure: f32,
        /// Maximum luminance in the scene (used as the log base);
        /// callers usually estimate this from a per-image pass.
        scene_max: f32,
        /// Bias parameter `0..=1`. Higher values brighten shadows;
        /// lower values protect highlights. `0.85` is Drago's default.
        bias: f32,
    },
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
        ToneMap::ReinhardExtended {
            exposure,
            white_point,
        } => {
            let w2 = (white_point * white_point).max(1e-6);
            let r = reinhard_extended(rgb[0] * exposure, w2);
            let g = reinhard_extended(rgb[1] * exposure, w2);
            let b = reinhard_extended(rgb[2] * exposure, w2);
            [srgb_oetf(r), srgb_oetf(g), srgb_oetf(b)]
        }
        ToneMap::Hable {
            exposure,
            linear_white,
        } => {
            // Apply curve to each channel, then normalise by the curve
            // value at `linear_white` so display white maps to 1.0.
            let denom = hable_curve(linear_white).max(1e-6);
            let r = hable_curve(rgb[0] * exposure) / denom;
            let g = hable_curve(rgb[1] * exposure) / denom;
            let b = hable_curve(rgb[2] * exposure) / denom;
            [srgb_oetf(r), srgb_oetf(g), srgb_oetf(b)]
        }
        ToneMap::Drago {
            exposure,
            scene_max,
            bias,
        } => {
            let r = drago(rgb[0] * exposure, scene_max * exposure, bias);
            let g = drago(rgb[1] * exposure, scene_max * exposure, bias);
            let b = drago(rgb[2] * exposure, scene_max * exposure, bias);
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

/// Reinhard's modified ("extended") operator with an explicit white
/// point: `v * (1 + v/W²) / (1 + v)` with `W² = white_point²` already
/// squared by the caller. Maps `v = white_point` exactly onto 1.0
/// instead of asymptoting toward it.
#[inline]
fn reinhard_extended(v: f32, white_sq: f32) -> f32 {
    let v = if v.is_finite() && v > 0.0 { v } else { 0.0 };
    let num = v * (1.0 + v / white_sq);
    let den = 1.0 + v;
    clamp01(num / den)
}

/// John Hable's "Uncharted 2" filmic curve, GDC 2010. The five-knot
/// rational fit:
/// ```text
/// f(x) = ((x * (A*x + C*B) + D*E) / (x * (A*x + B) + D*F)) - E/F
/// ```
/// with `A=0.15, B=0.50, C=0.10, D=0.20, E=0.02, F=0.30`. Apply once
/// to the linear scene value, once to the `linear_white` reference,
/// then divide so display white lands at 1.0 (the caller does the
/// normalisation).
#[inline]
fn hable_curve(x: f32) -> f32 {
    let x = if x.is_finite() && x > 0.0 { x } else { 0.0 };
    let a = 0.15_f32;
    let b = 0.50;
    let c = 0.10;
    let d = 0.20;
    let e = 0.02;
    let f = 0.30;
    ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f
}

/// Drago, Myszkowski, Annen, Chiba (EUROGRAPHICS 2003) adaptive
/// logarithmic operator §3:
/// ```text
/// Ld = (Ldmax * 0.01 / log10(1 + Lwmax))
///      * log(1 + Lw)
///      / log(2 + 8 * ((Lw/Lwmax)^(log(bias)/log(0.5))))
/// ```
/// We normalise out the `(Ldmax * 0.01)` part so the operator's output
/// already sits in `[0, 1]` (display-referred). `scene_max` is `Lwmax`;
/// `bias` defaults to Drago's recommended `0.85`. The paper's `log`s
/// are natural log; we keep them so.
#[inline]
fn drago(v: f32, scene_max: f32, bias: f32) -> f32 {
    let v = if v.is_finite() && v > 0.0 { v } else { 0.0 };
    let lwmax = scene_max.max(1e-6);
    // Clamp bias into the open (0, 1) interval — log(0) and log(1) both
    // degenerate the curve.
    let bias = bias.clamp(0.001, 0.999);
    let log_bias = bias.ln() / 0.5_f32.ln();
    let log_v = (1.0 + v).ln();
    let ratio = (v / lwmax).clamp(0.0, 1.0);
    let log_denom = (2.0 + 8.0 * ratio.powf(log_bias)).ln();
    // Normalisation: the `0.01 * Ldmax / log10(1 + Lwmax)` prefactor in
    // the paper sets the absolute display luminance; for an LDR
    // tone-mapper we want `Ld(Lwmax) ≈ 1`. With the prefactor folded
    // into a per-image scale we end up with:
    //   Ld(v) / Ld(Lwmax) = (log_v / log_denom) / (log_max / log_max_denom)
    // where the *_max variants are evaluated at `v = Lwmax`.
    let log_max = (1.0 + lwmax).ln();
    let log_max_denom = (2.0 + 8.0_f32 * 1.0_f32.powf(log_bias)).ln();
    let raw = log_v / log_denom;
    let raw_max = log_max / log_max_denom;
    clamp01(raw / raw_max.max(1e-6))
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
            ToneMap::ReinhardExtended {
                exposure: 1.0,
                white_point: 4.0,
            },
            ToneMap::Hable {
                exposure: 1.0,
                linear_white: 11.2,
            },
            ToneMap::Drago {
                exposure: 1.0,
                scene_max: 1.0,
                bias: 0.85,
            },
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
    fn reinhard_extended_reaches_white_at_white_point() {
        // With white_point = 4.0, a sample at 4.0 should land at 1.0
        // (display white) — that's the whole point of the extended
        // variant relative to the unmodified one.
        let img = one_pixel([4.0, 4.0, 4.0]);
        let out = tone_map(
            &img,
            ToneMap::ReinhardExtended {
                exposure: 1.0,
                white_point: 4.0,
            },
        );
        for &v in &out {
            assert_eq!(v, 255);
        }
    }

    #[test]
    fn hable_compresses_highlights_monotonically() {
        // Curve should be monotonically increasing in x and never NaN.
        let xs = [0.0_f32, 0.1, 0.5, 1.0, 2.0, 5.0, 20.0, 200.0];
        let mut last = -1.0;
        for &x in &xs {
            let img = one_pixel([x, x, x]);
            let out = tone_map(
                &img,
                ToneMap::Hable {
                    exposure: 1.0,
                    linear_white: 11.2,
                },
            );
            let v = out[0] as f32;
            assert!(v >= last - 1e-3, "non-monotonic at x={x}: {last} → {v}");
            last = v;
        }
    }

    #[test]
    fn drago_handles_wide_range_and_normalises_to_white() {
        // scene_max should map (approximately) to display white.
        let img = one_pixel([100.0, 100.0, 100.0]);
        let out = tone_map(
            &img,
            ToneMap::Drago {
                exposure: 1.0,
                scene_max: 100.0,
                bias: 0.85,
            },
        );
        for &v in &out {
            assert!(v > 220, "scene_max should land near white, got {v}");
        }
        // A mid-range sample should sit comfortably between 0 and 255.
        let img = one_pixel([1.0, 1.0, 1.0]);
        let out = tone_map(
            &img,
            ToneMap::Drago {
                exposure: 1.0,
                scene_max: 100.0,
                bias: 0.85,
            },
        );
        for &v in &out {
            assert!(v > 20 && v < 230, "mid-range Drago out of band: {v}");
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
