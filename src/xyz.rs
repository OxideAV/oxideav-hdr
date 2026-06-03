//! CIE XYZ ↔ linear RGB conversion helpers for the
//! `32-bit_rle_xyze` Radiance variant.
//!
//! XYZE files store CIE 1931 X, Y, Z tristimulus values in the same
//! shared-exponent four-byte representation as RGBE. The decoder in
//! [`crate::decoder`] returns the raw float channels untouched —
//! downstream code that wants linear RGB needs to pick a primaries
//! matrix and apply it. These helpers expose the two most commonly
//! requested matrices:
//!
//! * **sRGB / Rec. 709 primaries with a D65 white point** — the
//!   matrix every modern display targets. Standardised in
//!   IEC 61966-2-1 (sRGB) and ITU-R BT.709.
//! * **Radiance reference primaries** — the historical RGB primaries
//!   Greg Ward chose for the original RADIANCE renderer (close to but
//!   not exactly Rec. 709). Used by Radiance's own `ra_xyze` round-trip
//!   tools and a handful of older HDR datasets.
//!
//! All matrices here operate on **linear** scene-referred values; no
//! gamma is applied. Apply the gamma / OETF of your choice (sRGB
//! transfer function, simple `^(1/2.2)`, ACES output transform, …) on
//! top via the helpers in [`crate::tonemap`] if you need an LDR result.
//!
//! Numerical references:
//! * IEC 61966-2-1:1999 Annex C (sRGB → CIE XYZ matrix).
//! * BT.709-6 §3 (Rec. 709 primaries / RGB-XYZ derivation).
//! * Greg Ward, "The RADIANCE Picture File Format", radsite.lbl.gov
//!   (the `WHTEFFICACY = 179.0` and `RGB ↔ XYZ` constants used by the
//!   reference encoder/decoder pair).
//!
//! ## Photometric luminance
//!
//! The Radiance reference manual section "Physical interpretation" defines
//! a fixed photometric conversion that turns a `32-bit_rle_rgbe` pixel
//! into lumens / steradian / m²:
//!
//! ```text
//! luminance = 179 * (0.265*R + 0.670*G + 0.065*B)        (for FORMAT=32-bit_rle_rgbe)
//! luminance = 179 * Y                                    (for FORMAT=32-bit_rle_xyze)
//! ```
//!
//! 179 lumens/watt is Radiance's `WHTEFFICACY` — the standard luminous
//! efficacy of equal-energy white. The three RGBE coefficients are the
//! photopic weights of Greg Ward's reference RGB primaries onto CIE Y.
//! XYZE files don't need the 0.265/0.670/0.065 step because their Y
//! channel is already CIE Y; the 179× factor is the only remaining
//! radiance → photometric conversion. See [`luminance_lm_per_sr_per_m2`]
//! for the per-pixel helper and [`crate::HdrImage::luminance_buffer`]
//! for the whole-image variant.

/// Identifier for one of the supported RGB working spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbColorSpace {
    /// sRGB / Rec. 709 primaries with a D65 reference white. The
    /// matrix matches IEC 61966-2-1 Annex C and is what mainstream
    /// LDR consumers expect after tone mapping.
    Srgb,
    /// Greg Ward's original Radiance RGB primaries with an E
    /// (equal-energy) reference white. Reproduces the reference
    /// `ra_xyze` round-trip when both encoder and decoder use it.
    Radiance,
}

/// Forward matrix `M` such that `[X Y Z]^T = M * [R G B]^T`. All
/// values linear-light, no gamma applied. Row-major.
pub fn rgb_to_xyz_matrix(space: RgbColorSpace) -> [[f32; 3]; 3] {
    match space {
        // IEC 61966-2-1 Annex C (sRGB / Rec. 709 with D65 white).
        // Standardised values, four-decimal rounding from the
        // recommendation.
        RgbColorSpace::Srgb => [
            [0.4124564, 0.3575761, 0.1804375],
            [0.2126729, 0.7151522, 0.072175],
            [0.0193339, 0.119192, 0.9503041],
        ],
        // Greg Ward's Radiance primaries (xr=0.640, yr=0.330; xg=0.290,
        // yg=0.600; xb=0.150, yb=0.060) with the E (equal-energy) white
        // (xw=1/3, yw=1/3). Matrix derived analytically — kept here as
        // pre-computed constants to avoid pulling in a linear-algebra
        // crate.
        RgbColorSpace::Radiance => [
            [0.5141446, 0.3238845, 0.1619709],
            [0.2651058, 0.6701058, 0.0647884],
            [0.0241005, 0.1228527, 0.8530467],
        ],
    }
}

/// Inverse matrix `M^-1` such that `[R G B]^T = M^-1 * [X Y Z]^T`.
pub fn xyz_to_rgb_matrix(space: RgbColorSpace) -> [[f32; 3]; 3] {
    match space {
        // IEC 61966-2-1 Annex C inverse.
        RgbColorSpace::Srgb => [
            [3.2404542, -1.5371385, -0.4985314],
            [-0.969266, 1.8760108, 0.041556],
            [0.0556434, -0.2040259, 1.0572252],
        ],
        // Inverse of the Radiance forward matrix above. Computed once
        // and pinned here.
        RgbColorSpace::Radiance => [
            [2.5653128, -1.1668496, -0.3984632],
            [-1.0221082, 1.9782866, 0.0438216],
            [0.0747244, -0.2519396, 1.1772152],
        ],
    }
}

/// Convert one CIE XYZ triple to linear RGB in the chosen working
/// space. Negative results (out-of-gamut samples) are returned as-is —
/// callers that need an in-gamut display value should clamp or
/// gamut-map after the conversion.
#[inline]
pub fn xyz_to_rgb(xyz: [f32; 3], space: RgbColorSpace) -> [f32; 3] {
    let m = xyz_to_rgb_matrix(space);
    apply_matrix(m, xyz)
}

/// Convert one linear RGB triple into CIE XYZ.
#[inline]
pub fn rgb_to_xyz(rgb: [f32; 3], space: RgbColorSpace) -> [f32; 3] {
    let m = rgb_to_xyz_matrix(space);
    apply_matrix(m, rgb)
}

/// Convert the float channels carried by an [`crate::HdrImage`] from
/// CIE XYZ into linear RGB in `space`, in-place. Use this when the
/// decoded image's `header.format` is [`crate::HdrFormat::Xyze`] and
/// you want to consume it as RGB. The header tag is updated to
/// [`crate::HdrFormat::Rgbe`] so re-encoding writes the standard
/// `32-bit_rle_rgbe` variant.
pub fn convert_image_xyz_to_rgb(image: &mut crate::HdrImage, space: RgbColorSpace) {
    let m = xyz_to_rgb_matrix(space);
    for px in image.pixels.chunks_exact_mut(3) {
        let v = apply_matrix(m, [px[0], px[1], px[2]]);
        px[0] = v[0];
        px[1] = v[1];
        px[2] = v[2];
    }
    image.header.format = crate::HdrFormat::Rgbe;
}

/// Inverse of [`convert_image_xyz_to_rgb`]: convert linear RGB to CIE
/// XYZ in-place and update the header tag so re-encoding writes
/// `32-bit_rle_xyze`.
pub fn convert_image_rgb_to_xyz(image: &mut crate::HdrImage, space: RgbColorSpace) {
    let m = rgb_to_xyz_matrix(space);
    for px in image.pixels.chunks_exact_mut(3) {
        let v = apply_matrix(m, [px[0], px[1], px[2]]);
        px[0] = v[0];
        px[1] = v[1];
        px[2] = v[2];
    }
    image.header.format = crate::HdrFormat::Xyze;
}

/// Radiance's standard luminous efficacy of equal-energy white,
/// `WHTEFFICACY` in `src/common/color.h`. The constant that turns
/// reference-encoder watts/sr/m² into lumens/sr/m².
pub const WHTEFFICACY: f32 = 179.0;

/// Photopic weights that project Greg Ward's reference RGB primaries
/// onto CIE Y. They appear in the Radiance reference manual section
/// "Physical interpretation" as the per-primary multipliers used by the
/// canonical `luminance(col)` macro: `Y = 0.265*R + 0.670*G + 0.065*B`.
/// Order is `(R, G, B)`.
pub const RGBE_BRIGHT_COEFFS: [f32; 3] = [0.265, 0.670, 0.065];

/// Photometric luminance, in lumens per steradian per m², of a single
/// scene-referred pixel from a Radiance picture.
///
/// `format` selects which of the two photometric reductions documented
/// in the Radiance reference manual is applied:
///
/// * [`crate::HdrFormat::Rgbe`] — the per-primary projection
///   `179 * (0.265*R + 0.670*G + 0.065*B)`.
/// * [`crate::HdrFormat::Xyze`] — pass-through `179 * Y` because the
///   Y channel of an XYZE file is already CIE Y.
///
/// Negative inputs (out-of-gamut samples) are returned as-is; callers
/// that need a clamped photometric value should `max(0.0)` after the
/// call.
#[inline]
pub fn luminance_lm_per_sr_per_m2(pixel: [f32; 3], format: crate::HdrFormat) -> f32 {
    match format {
        crate::HdrFormat::Rgbe => {
            WHTEFFICACY
                * (RGBE_BRIGHT_COEFFS[0] * pixel[0]
                    + RGBE_BRIGHT_COEFFS[1] * pixel[1]
                    + RGBE_BRIGHT_COEFFS[2] * pixel[2])
        }
        crate::HdrFormat::Xyze => WHTEFFICACY * pixel[1],
    }
}

#[inline]
fn apply_matrix(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Derive the linear `RGB → CIE XYZ` matrix from an arbitrary
/// [`crate::Primaries`] record using the standard primary-construction
/// procedure (BT.709 §3 / IEC 61966-2-1 Annex C).
///
/// Given primary chromaticities `(xR, yR)`, `(xG, yG)`, `(xB, yB)` and a
/// reference white `(xW, yW)`, each primary's XYZ tristimulus values are
/// `Xi = xi / yi`, `Yi = 1`, `Zi = (1 - xi - yi) / yi`. The unscaled
/// matrix `[X_R X_G X_B; Y_R Y_G Y_B; Z_R Z_G Z_B]` is then post-scaled
/// by per-primary luminance scalars `(SR, SG, SB)` chosen so that
/// `[1 1 1]^T` maps to the white-point XYZ, i.e.
/// `[SR SG SB]^T = M_unscaled^{-1} * [Xw Yw Zw]^T`.
///
/// The derivation is purely algebraic and pulls in no `PRIMARIES`
/// matrix beyond the eight CIE xy floats the [`crate::Primaries`]
/// record already carries. Returns `None` when the primaries are
/// degenerate (any `y == 0`, or a singular unscaled matrix); the
/// reference manual treats such records as malformed and leaves their
/// interpretation undefined, so callers can treat `None` as "fall back
/// to a known-good matrix" (e.g. `rgb_to_xyz_matrix(RgbColorSpace::Srgb)`).
///
/// This is the matrix that turns the `0.640 0.330 0.290 0.600 0.150
/// 0.060 1/3 1/3` Radiance default into the
/// [`RgbColorSpace::Radiance`] entry of [`rgb_to_xyz_matrix`] within
/// `f32` precision, and the sRGB chromaticities (`Primaries::SRGB`)
/// into the IEC 61966-2-1 Annex C entry within `f32` precision —
/// callers that need a matrix for a primaries record the named
/// `RgbColorSpace` enum doesn't cover (e.g. `Primaries::P3_D65`,
/// `Primaries::REC2020`, or a custom 8-float `PRIMARIES=` record from a
/// niche renderer) get the right linear matrix without hard-coding new
/// constants into the crate.
pub fn rgb_to_xyz_matrix_from_primaries(p: crate::Primaries) -> Option<[[f32; 3]; 3]> {
    // Reject any zero-Y chromaticity (X = x/y, Z = (1-x-y)/y blow up).
    if p.red.1 == 0.0 || p.green.1 == 0.0 || p.blue.1 == 0.0 || p.white.1 == 0.0 {
        return None;
    }
    // Per-primary XYZ tristimulus values.
    let xy_to_xyz = |(x, y): (f32, f32)| -> [f32; 3] {
        let z = 1.0 - x - y;
        [x / y, 1.0, z / y]
    };
    let r = xy_to_xyz(p.red);
    let g = xy_to_xyz(p.green);
    let b = xy_to_xyz(p.blue);
    let w = xy_to_xyz(p.white);
    // Unscaled matrix (columns = primary tristimulus values).
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    // Invert m by cofactor expansion (we already have a 3×3; no need
    // for a general LU). det = r dot (g × b).
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < f32::EPSILON {
        return None;
    }
    let inv = invert3x3(m, det);
    // Per-primary luminance scalars: m^-1 * w.
    let s = apply_matrix(inv, w);
    // Final matrix: each column of `m` scaled by `s_i`.
    Some([
        [s[0] * m[0][0], s[1] * m[0][1], s[2] * m[0][2]],
        [s[0] * m[1][0], s[1] * m[1][1], s[2] * m[1][2]],
        [s[0] * m[2][0], s[1] * m[2][1], s[2] * m[2][2]],
    ])
}

/// Inverse of [`rgb_to_xyz_matrix_from_primaries`]: derive the
/// `CIE XYZ → RGB` matrix for an arbitrary [`crate::Primaries`] record.
///
/// Implementation: derive the forward matrix and invert it (3×3
/// cofactor expansion). Returns `None` for the same degenerate cases
/// as the forward helper, plus any non-invertible scaled matrix.
pub fn xyz_to_rgb_matrix_from_primaries(p: crate::Primaries) -> Option<[[f32; 3]; 3]> {
    let m = rgb_to_xyz_matrix_from_primaries(p)?;
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < f32::EPSILON {
        return None;
    }
    Some(invert3x3(m, det))
}

/// 3×3 cofactor inverse. `det` is passed in so callers that already
/// computed it don't redo the work.
#[inline]
fn invert3x3(m: [[f32; 3]; 3], det: f32) -> [[f32; 3]; 3] {
    let inv_det = 1.0 / det;
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn srgb_white_maps_to_d65_xyz() {
        // Pure white in sRGB has nominal CIE XYZ ≈ (0.9505, 1.0000,
        // 1.0890) (the D65 reference white). The matrix rows sum to
        // exactly those values to within rounding.
        let xyz = rgb_to_xyz([1.0, 1.0, 1.0], RgbColorSpace::Srgb);
        assert!(approx(xyz[0], 0.9505, 1e-3));
        assert!(approx(xyz[1], 1.0000, 1e-3));
        assert!(approx(xyz[2], 1.0890, 1e-3));
    }

    #[test]
    fn srgb_roundtrips_arbitrary_colour() {
        for &rgb in &[
            [0.5_f32, 0.25, 0.10],
            [0.0, 1.0, 0.0],
            [1.0, 0.5, 0.0],
            [0.123, 0.456, 0.789],
        ] {
            let xyz = rgb_to_xyz(rgb, RgbColorSpace::Srgb);
            let back = xyz_to_rgb(xyz, RgbColorSpace::Srgb);
            for i in 0..3 {
                assert!(
                    approx(back[i], rgb[i], 1e-4),
                    "ch {i}: {} vs {}",
                    rgb[i],
                    back[i]
                );
            }
        }
    }

    #[test]
    fn radiance_roundtrips_arbitrary_colour() {
        for &rgb in &[[0.5_f32, 0.25, 0.10], [1.0, 1.0, 1.0], [0.7, 0.3, 0.9]] {
            let xyz = rgb_to_xyz(rgb, RgbColorSpace::Radiance);
            let back = xyz_to_rgb(xyz, RgbColorSpace::Radiance);
            for i in 0..3 {
                assert!(
                    approx(back[i], rgb[i], 1e-3),
                    "ch {i}: {} vs {}",
                    rgb[i],
                    back[i]
                );
            }
        }
    }

    #[test]
    fn srgb_red_primary_has_zero_blue_y() {
        // A pure-red sRGB sample should hit the red row of the matrix
        // exactly: X = 0.4124, Y = 0.2127, Z = 0.0193.
        let xyz = rgb_to_xyz([1.0, 0.0, 0.0], RgbColorSpace::Srgb);
        assert!(approx(xyz[0], 0.4124564, 1e-5));
        assert!(approx(xyz[1], 0.2126729, 1e-5));
        assert!(approx(xyz[2], 0.0193339, 1e-5));
    }

    #[test]
    fn luminance_rgbe_uses_radiance_coefficients() {
        // Worked example from the picture-file-format reference:
        // luminance = 179 * (0.265*R + 0.670*G + 0.065*B).
        // Punching (1, 1, 1) through that formula yields
        // 179 * (0.265 + 0.670 + 0.065) = 179 * 1.0 = 179.
        let y = luminance_lm_per_sr_per_m2([1.0, 1.0, 1.0], crate::HdrFormat::Rgbe);
        assert!((y - 179.0).abs() < 1e-3, "expected 179, got {y}");
    }

    #[test]
    fn luminance_rgbe_isolates_each_channel() {
        // Each primary in isolation should yield its weighted contribution.
        let lr = luminance_lm_per_sr_per_m2([1.0, 0.0, 0.0], crate::HdrFormat::Rgbe);
        let lg = luminance_lm_per_sr_per_m2([0.0, 1.0, 0.0], crate::HdrFormat::Rgbe);
        let lb = luminance_lm_per_sr_per_m2([0.0, 0.0, 1.0], crate::HdrFormat::Rgbe);
        assert!((lr - 179.0 * 0.265).abs() < 1e-3, "R: {lr}");
        assert!((lg - 179.0 * 0.670).abs() < 1e-3, "G: {lg}");
        assert!((lb - 179.0 * 0.065).abs() < 1e-3, "B: {lb}");
        // G dominates: 0.670 > 0.265 > 0.065.
        assert!(lg > lr);
        assert!(lr > lb);
    }

    #[test]
    fn luminance_xyze_is_179_times_y() {
        // XYZE files: the Y primary is already lumens/sr/m²; the only
        // remaining conversion is the 179× scale.
        let y = luminance_lm_per_sr_per_m2([0.1, 1.0, 0.2], crate::HdrFormat::Xyze);
        assert!((y - 179.0).abs() < 1e-3, "expected 179, got {y}");
        // Doubling Y doubles the luminance; X and Z are ignored.
        let y2 = luminance_lm_per_sr_per_m2([5.0, 2.0, 5.0], crate::HdrFormat::Xyze);
        assert!((y2 - 358.0).abs() < 1e-3, "expected 358, got {y2}");
    }

    #[test]
    fn luminance_scales_linearly_with_input() {
        // Doubling every channel doubles the photometric luminance — the
        // formula is linear in the float radiance values.
        let base = luminance_lm_per_sr_per_m2([0.50, 0.25, 0.10], crate::HdrFormat::Rgbe);
        let dbl = luminance_lm_per_sr_per_m2([1.00, 0.50, 0.20], crate::HdrFormat::Rgbe);
        assert!((dbl - 2.0 * base).abs() < 1e-3);
    }

    #[test]
    fn whtefficacy_constant_matches_reference() {
        // The constant is fixed at 179 lm/W in the reference manual; lock
        // it down so an accidental edit shows up in CI.
        assert!((WHTEFFICACY - 179.0).abs() < 1e-6);
        // Coefficients sum to 1 by construction (they project onto the
        // E white point of Ward's primaries).
        let sum = RGBE_BRIGHT_COEFFS[0] + RGBE_BRIGHT_COEFFS[1] + RGBE_BRIGHT_COEFFS[2];
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "coeffs sum to {sum}, expected 1.0"
        );
    }

    #[test]
    fn convert_image_helpers_flip_format_tag() {
        use crate::{HdrFormat, HdrImage};
        let pixels = vec![1.0, 0.5, 0.25, 0.7, 0.6, 0.5];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels.clone());
        convert_image_rgb_to_xyz(&mut img, RgbColorSpace::Srgb);
        assert_eq!(img.header.format, HdrFormat::Xyze);
        // Round-trip back to RGB.
        convert_image_xyz_to_rgb(&mut img, RgbColorSpace::Srgb);
        assert_eq!(img.header.format, HdrFormat::Rgbe);
        for (i, (got, want)) in img.pixels.iter().zip(pixels.iter()).enumerate() {
            assert!((got - want).abs() < 1e-4, "pixel {i}: {want} vs {got}");
        }
    }

    #[test]
    fn derived_matrix_for_srgb_matches_named_constant() {
        // Feeding `Primaries::SRGB` (the IEC 61966-2-1 Annex C
        // chromaticities) into the chromaticity-derived helper must
        // recover the hard-coded `RgbColorSpace::Srgb` matrix within
        // f32 precision. The named constant is just an algebraic
        // simplification of the same derivation.
        let derived = rgb_to_xyz_matrix_from_primaries(crate::Primaries::SRGB)
            .expect("sRGB primaries derive to a valid matrix");
        let named = rgb_to_xyz_matrix(RgbColorSpace::Srgb);
        for (r, (drow, nrow)) in derived.iter().zip(named.iter()).enumerate() {
            for (c, (d, n)) in drow.iter().zip(nrow.iter()).enumerate() {
                assert!(
                    (d - n).abs() < 1e-3,
                    "row {r} col {c}: derived={d} named={n}"
                );
            }
        }
    }

    #[test]
    fn derived_matrix_for_radiance_matches_named_constant() {
        // Same check against Greg Ward's E-white Radiance primaries.
        let derived = rgb_to_xyz_matrix_from_primaries(crate::Primaries::RADIANCE)
            .expect("Radiance primaries derive to a valid matrix");
        let named = rgb_to_xyz_matrix(RgbColorSpace::Radiance);
        for (r, (drow, nrow)) in derived.iter().zip(named.iter()).enumerate() {
            for (c, (d, n)) in drow.iter().zip(nrow.iter()).enumerate() {
                assert!(
                    (d - n).abs() < 1e-3,
                    "row {r} col {c}: derived={d} named={n}"
                );
            }
        }
    }

    #[test]
    fn derived_matrix_maps_unit_rgb_to_white_xyz() {
        // The whole point of the per-primary luminance scaling step is
        // that `[1 1 1]^T` maps to the reference white's CIE XYZ. With
        // a D65 white (Y normalised to 1), Y must come out at exactly
        // 1.0 within f32 precision.
        for primaries in [
            crate::Primaries::SRGB,
            crate::Primaries::P3_D65,
            crate::Primaries::REC2020,
            crate::Primaries::RADIANCE,
        ] {
            let m = rgb_to_xyz_matrix_from_primaries(primaries).unwrap();
            let xyz = apply_matrix(m, [1.0, 1.0, 1.0]);
            assert!(
                (xyz[1] - 1.0).abs() < 1e-4,
                "Y for {primaries:?}: {} (expected 1.0)",
                xyz[1]
            );
            // The X/Z components match the white-point chromaticity
            // `(xw/yw, 1, (1-xw-yw)/yw)`.
            let xw = primaries.white.0 / primaries.white.1;
            let zw = (1.0 - primaries.white.0 - primaries.white.1) / primaries.white.1;
            assert!(
                (xyz[0] - xw).abs() < 1e-4,
                "X for {primaries:?}: {} expected {xw}",
                xyz[0]
            );
            assert!(
                (xyz[2] - zw).abs() < 1e-4,
                "Z for {primaries:?}: {} expected {zw}",
                xyz[2]
            );
        }
    }

    #[test]
    fn derived_matrix_round_trips_through_inverse() {
        // The two derived matrices are mutual inverses; their product
        // is the identity within f32 precision. Tested for each named
        // primaries constant the crate ships.
        for primaries in [
            crate::Primaries::SRGB,
            crate::Primaries::P3_D65,
            crate::Primaries::REC2020,
            crate::Primaries::RADIANCE,
        ] {
            let m = rgb_to_xyz_matrix_from_primaries(primaries).unwrap();
            let inv = xyz_to_rgb_matrix_from_primaries(primaries).unwrap();
            // Verify m * inv ≈ identity.
            for (r, mrow) in m.iter().enumerate() {
                for c in 0..3 {
                    let v: f32 = mrow
                        .iter()
                        .zip(inv.iter())
                        .map(|(mv, irow)| mv * irow[c])
                        .sum();
                    let expected = if r == c { 1.0 } else { 0.0 };
                    assert!(
                        (v - expected).abs() < 1e-3,
                        "{primaries:?} row {r} col {c}: {v} vs {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn derived_matrix_rejects_degenerate_white_point() {
        // A `yW = 0` chromaticity (X = xw/0 is non-finite). The helper
        // must refuse to silently emit `inf`s.
        let bad = crate::Primaries {
            red: (0.640, 0.330),
            green: (0.290, 0.600),
            blue: (0.150, 0.060),
            white: (0.5, 0.0),
        };
        assert!(rgb_to_xyz_matrix_from_primaries(bad).is_none());
        assert!(xyz_to_rgb_matrix_from_primaries(bad).is_none());
    }

    #[test]
    fn derived_matrix_rejects_zero_y_primary() {
        // Any `yi = 0` primary makes the per-primary Y = 1/y blow up.
        // Rejected with `None`.
        let bad = crate::Primaries {
            red: (0.640, 0.000),
            green: (0.290, 0.600),
            blue: (0.150, 0.060),
            white: (1.0 / 3.0, 1.0 / 3.0),
        };
        assert!(rgb_to_xyz_matrix_from_primaries(bad).is_none());
    }

    #[test]
    fn derived_matrix_for_p3_d65_maps_white_to_d65_xyz() {
        // Per the Display P3 spec: the white is D65 with nominal XYZ
        // (0.9505, 1.0000, 1.0890). The derived matrix's column sum
        // (i.e. `M * [1 1 1]^T`) must match those values.
        let m = rgb_to_xyz_matrix_from_primaries(crate::Primaries::P3_D65).unwrap();
        let xyz = apply_matrix(m, [1.0, 1.0, 1.0]);
        assert!(
            (xyz[0] - 0.9505).abs() < 1e-3,
            "X for P3-D65: {} (expected 0.9505)",
            xyz[0]
        );
        assert!(
            (xyz[1] - 1.0000).abs() < 1e-3,
            "Y for P3-D65: {} (expected 1.0)",
            xyz[1]
        );
        assert!(
            (xyz[2] - 1.0890).abs() < 1e-3,
            "Z for P3-D65: {} (expected 1.0890)",
            xyz[2]
        );
    }

    #[test]
    fn derived_matrix_for_rec2020_maps_white_to_d65_xyz() {
        // Rec.2020 also uses a D65 white. Same expected XYZ as sRGB and
        // P3 — only the per-primary gamut changes.
        let m = rgb_to_xyz_matrix_from_primaries(crate::Primaries::REC2020).unwrap();
        let xyz = apply_matrix(m, [1.0, 1.0, 1.0]);
        assert!(
            (xyz[0] - 0.9505).abs() < 1e-3,
            "X for Rec.2020: {} (expected 0.9505)",
            xyz[0]
        );
        assert!(
            (xyz[1] - 1.0000).abs() < 1e-3,
            "Y for Rec.2020: {} (expected 1.0)",
            xyz[1]
        );
        assert!(
            (xyz[2] - 1.0890).abs() < 1e-3,
            "Z for Rec.2020: {} (expected 1.0890)",
            xyz[2]
        );
    }
}
