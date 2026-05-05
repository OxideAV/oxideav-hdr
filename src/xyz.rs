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

#[inline]
fn apply_matrix(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
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
}
