//! Standalone image container returned by `oxideav-hdr`'s framework-free
//! decode API and accepted by the standalone encode API.
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate can be built with the default `registry` feature off — i.e.
//! without depending on `oxideav-core` at all. When the `registry`
//! feature is on the [`crate::registry`] module wires this shape into
//! the framework `VideoFrame` representation by tone-mapping each f32
//! channel into Rgb24 (clamped, gamma-corrected) at the boundary so the
//! float dynamic range stays available to native callers and the LDR
//! framework path stays simple.

use crate::header::{HdrHeader, Primaries};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_exposure_scales_pixels_and_clears_header() {
        let mut img = HdrImage::new_rgb96f(1, 2, vec![1.0, 0.5, 0.25, 2.0, 1.0, 0.5]);
        img.header.exposure = Some(0.5);
        img.apply_exposure();
        assert!(img.header.exposure.is_none(), "exposure slot not cleared");
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
        assert!((img.pixels[5] - 0.25).abs() < 1e-6);
        // Second call must be a no-op (slot is None).
        img.apply_exposure();
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn apply_exposure_with_none_does_nothing() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![1.0, 0.5, 0.25]);
        assert!(img.header.exposure.is_none());
        img.apply_exposure();
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn apply_exposure_unit_factor_is_a_no_op() {
        // EXPOSURE=1.0 should not perturb the float pixels and still
        // clears the header slot.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.125]);
        img.header.exposure = Some(1.0);
        img.apply_exposure();
        assert!(img.header.exposure.is_none());
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
        assert!((img.pixels[1] - 0.25).abs() < 1e-6);
        assert!((img.pixels[2] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn apply_colorcorr_scales_each_channel_independently() {
        let mut img = HdrImage::new_rgb96f(2, 1, vec![1.0, 1.0, 1.0, 0.5, 0.25, 0.10]);
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.apply_colorcorr();
        assert!(img.header.colorcorr.is_none(), "colorcorr slot not cleared");
        // Pixel 0
        assert!((img.pixels[0] - 2.0).abs() < 1e-6);
        assert!((img.pixels[1] - 4.0).abs() < 1e-6);
        assert!((img.pixels[2] - 8.0).abs() < 1e-6);
        // Pixel 1
        assert!((img.pixels[3] - 1.0).abs() < 1e-6);
        assert!((img.pixels[4] - 1.0).abs() < 1e-6);
        assert!((img.pixels[5] - 0.80).abs() < 1e-6);
    }

    #[test]
    fn luminance_buffer_rgbe_matches_per_pixel_formula() {
        // Three pixels at known radiance values; the buffer should be
        // 179 * (0.265*R + 0.670*G + 0.065*B) for each.
        let pixels = vec![1.0, 1.0, 1.0, 0.5, 0.25, 0.10, 0.0, 1.0, 0.0];
        let img = HdrImage::new_rgb96f(3, 1, pixels);
        let lum = img.luminance_buffer();
        assert_eq!(lum.len(), 3);
        // Pixel 0: 179 * 1.0 = 179.
        assert!((lum[0] - 179.0).abs() < 1e-3);
        // Pixel 1: 179 * (0.265*0.5 + 0.670*0.25 + 0.065*0.10)
        let p1 = 179.0 * (0.265 * 0.5 + 0.670 * 0.25 + 0.065 * 0.10);
        assert!((lum[1] - p1).abs() < 1e-2);
        // Pixel 2: pure green, 179 * 0.670 = 119.93.
        assert!((lum[2] - 179.0 * 0.670).abs() < 1e-2);
    }

    #[test]
    fn luminance_buffer_xyze_skips_per_primary_projection() {
        use crate::HdrFormat;
        let pixels = vec![0.1, 0.5, 0.2, 0.3, 1.0, 0.4];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.format = HdrFormat::Xyze;
        let lum = img.luminance_buffer();
        // XYZE: luminance is 179 * Y exactly.
        assert!((lum[0] - 179.0 * 0.5).abs() < 1e-2);
        assert!((lum[1] - 179.0 * 1.0).abs() < 1e-2);
    }

    #[test]
    fn effective_pixaspect_defaults_to_one_when_absent() {
        // No PIXASPECT record → reference-manual default of 1.0.
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        assert!(img.header.pixaspect.is_none());
        assert!((img.effective_pixaspect() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn effective_pixaspect_returns_header_value_when_set() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.pixaspect = Some(0.5);
        assert!((img.effective_pixaspect() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn apply_colorcorr_unit_vector_is_a_no_op() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.7, 0.5, 0.3]);
        img.header.colorcorr = Some([1.0, 1.0, 1.0]);
        img.apply_colorcorr();
        assert!(img.header.colorcorr.is_none());
        assert!((img.pixels[0] - 0.7).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn effective_primaries_defaults_to_radiance_when_absent() {
        // No PRIMARIES record → reference-manual default: Greg Ward's
        // original Radiance primaries with an equal-energy white
        // (`0.640 0.330 0.290 0.600 0.150 0.060 0.333 0.333`).
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        assert!(img.header.primaries.is_none());
        let p = img.effective_primaries();
        assert!((p.red.0 - 0.640).abs() < 1e-5);
        assert!((p.red.1 - 0.330).abs() < 1e-5);
        assert!((p.green.0 - 0.290).abs() < 1e-5);
        assert!((p.green.1 - 0.600).abs() < 1e-5);
        assert!((p.blue.0 - 0.150).abs() < 1e-5);
        assert!((p.blue.1 - 0.060).abs() < 1e-5);
        // Equal-energy white: x = y = 1/3.
        assert!((p.white.0 - 1.0 / 3.0).abs() < 1e-5);
        assert!((p.white.1 - 1.0 / 3.0).abs() < 1e-5);
        // Exact match against the `Primaries::RADIANCE` constant: the
        // helper is a substitution, not a reconstruction.
        assert_eq!(p, Primaries::RADIANCE);
    }

    #[test]
    fn effective_primaries_returns_header_value_when_set() {
        // When the file declared a PRIMARIES record the helper must
        // return that value verbatim, NOT the reference-manual default.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.primaries = Some(Primaries::SRGB);
        let p = img.effective_primaries();
        assert_eq!(p, Primaries::SRGB);
        // Pin the sRGB-specific value (D65 white at 0.3127, 0.3290)
        // so a future swap of the constants is caught by this test.
        assert!((p.white.0 - 0.3127).abs() < 1e-5);
        assert!((p.white.1 - 0.3290).abs() < 1e-5);
    }

    #[test]
    fn effective_primaries_is_idempotent_with_record_roundtrip() {
        // The default Radiance primaries must survive the on-disk
        // PRIMARIES record round-trip without drift, so a caller that
        // re-encodes with `header.primaries = Some(effective)` and
        // re-decodes recovers the same chromaticities.
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        let p = img.effective_primaries();
        let s = p.to_record_string();
        let back = Primaries::from_record_str(&s).expect("PRIMARIES round-trip parse");
        assert!((back.red.0 - p.red.0).abs() < 1e-5);
        assert!((back.green.0 - p.green.0).abs() < 1e-5);
        assert!((back.blue.0 - p.blue.0).abs() < 1e-5);
        assert!((back.white.0 - p.white.0).abs() < 1e-5);
        assert!((back.white.1 - p.white.1).abs() < 1e-5);
    }
}

/// Pixel layout used by [`HdrImage`].
///
/// Always packed RGB f32 in linear scene-referred space (after
/// shared-exponent decode). Alpha is not part of the Radiance
/// container, so there's no Rgba variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrPixelFormat {
    /// Packed 32-bit float RGB, 12 bytes per pixel, channel order R, G, B.
    Rgb96f,
}

/// One decoded HDR frame, framework-free shape.
///
/// `pixels` is `width * height * 3` long, packed row-major top-down
/// regardless of the on-disk axis flags.
#[derive(Debug, Clone)]
pub struct HdrImage {
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Pixel layout the float buffer carries. Always
    /// [`HdrPixelFormat::Rgb96f`] today.
    pub pixel_format: HdrPixelFormat,
    /// `width * height * 3` packed f32 components, row-major, top-down,
    /// channel order R, G, B. Each value is the linear scene-referred
    /// radiance reconstructed from the on-disk shared-exponent
    /// representation.
    pub pixels: Vec<f32>,
    /// Header metadata that survived the decode (everything between the
    /// magic line and the resolution line, plus the resolution line's
    /// axis flags). Encoders accept this as a hint; decoders always
    /// populate it with whatever the file declared.
    pub header: HdrHeader,
}

impl HdrImage {
    /// Convenience: construct a top-down RGB f32 image with a default
    /// [`HdrHeader`].
    pub fn new_rgb96f(width: u32, height: u32, pixels: Vec<f32>) -> Self {
        debug_assert_eq!(pixels.len(), (width as usize) * (height as usize) * 3);
        Self {
            width,
            height,
            pixel_format: HdrPixelFormat::Rgb96f,
            pixels,
            header: HdrHeader::default(),
        }
    }

    /// Apply the header's `EXPOSURE` factor to every float channel and
    /// clear the header slot.
    ///
    /// The Radiance reference manual defines `EXPOSURE=` as the
    /// cumulative multiplicative scale applied to the radiance values on
    /// the way out of decode — i.e. the on-disk float `f` represents a
    /// real-world radiance of `f * EXPOSURE`. The decoder stores the
    /// value but does not multiply it into the buffer (so callers that
    /// want the raw shared-exponent samples still have them); this
    /// helper performs the multiplication in-place and clears
    /// `header.exposure` to signal that the pixels are now in
    /// post-exposure space. A second call is a no-op.
    pub fn apply_exposure(&mut self) {
        if let Some(e) = self.header.exposure.take() {
            if (e - 1.0).abs() > f32::EPSILON {
                for v in &mut self.pixels {
                    *v *= e;
                }
            }
        }
    }

    /// Allocate a fresh `width * height` buffer of per-pixel photometric
    /// luminance values, in lumens per steradian per m², computed from
    /// the picture's float channels per the Radiance reference-manual
    /// formula:
    ///
    /// ```text
    /// FORMAT=32-bit_rle_rgbe -> 179 * (0.265*R + 0.670*G + 0.065*B)
    /// FORMAT=32-bit_rle_xyze -> 179 * Y
    /// ```
    ///
    /// The reduction is the same one Radiance's `luminance(col)` macro
    /// produces. `header.format` selects which branch is applied so the
    /// caller doesn't have to track it explicitly. Out-of-gamut samples
    /// (`R<0`, `G<0`, `B<0`) are passed through linearly — the formula
    /// is `Σ a_i * c_i` and inherits whatever sign the input has.
    pub fn luminance_buffer(&self) -> Vec<f32> {
        let n = (self.width as usize) * (self.height as usize);
        let mut out = Vec::with_capacity(n);
        for px in self.pixels.chunks_exact(3) {
            out.push(crate::xyz::luminance_lm_per_sr_per_m2(
                [px[0], px[1], px[2]],
                self.header.format,
            ));
        }
        out
    }

    /// Pixel aspect ratio (`pixel height / pixel width`) the picture
    /// declared via one or more `PIXASPECT=` records, with the
    /// reference-manual default of `1.0` applied when no record was
    /// present.
    ///
    /// The Radiance reference manual defines `PIXASPECT=` as a
    /// multiplicative-cumulative scalar: the *effective* aspect ratio
    /// is the product of every `PIXASPECT=` record in the header, and
    /// `1.0` when none appears. The decoder folds the multiplication
    /// into [`HdrHeader::pixaspect`] at parse time so this helper just
    /// substitutes the `None` → `1.0` default for the caller without
    /// any extra arithmetic.
    pub fn effective_pixaspect(&self) -> f32 {
        self.header.pixaspect.unwrap_or(1.0)
    }

    /// CIE chromaticity coordinates of the three RGB primaries and the
    /// reference white the picture should be interpreted against, with
    /// the Radiance reference-manual default applied when no
    /// `PRIMARIES=` record was present.
    ///
    /// Per the staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`
    /// §1 PRIMARIES row), when a Radiance picture omits `PRIMARIES=` the
    /// consumer is expected to assume Greg Ward's original Radiance
    /// primaries with an equal-energy reference white —
    /// `0.640 0.330 0.290 0.600 0.150 0.060 0.333 0.333` (R, G, B, W).
    /// Those are the values [`Primaries::RADIANCE`] holds (the white
    /// point exact at `(1/3, 1/3)` so the round-trip through
    /// [`Primaries::from_record_str`] / [`Primaries::to_record_string`]
    /// is non-lossy at f32 precision).
    ///
    /// Mirrors [`Self::effective_pixaspect`] in shape: callers that need
    /// "what the file said, or the spec default" can take this in one
    /// call without re-implementing the fallback. Consumers that need to
    /// distinguish "file declared default-equal primaries explicitly"
    /// from "no record was present" should match on
    /// [`HdrHeader::primaries`] directly.
    pub fn effective_primaries(&self) -> Primaries {
        self.header.primaries.unwrap_or(Primaries::RADIANCE)
    }

    /// Apply the header's `COLORCORR` per-channel multiplier to every
    /// float channel and clear the header slot.
    ///
    /// `COLORCORR=R G B` is documented in the Radiance reference manual
    /// as a per-channel multiplier complementary to `EXPOSURE`: the
    /// on-disk channel `c_i` represents a scene-referred radiance of
    /// `c_i * COLORCORR_i`. Decoder behaviour mirrors `apply_exposure`:
    /// the value is parsed and round-tripped but not folded into the
    /// pixel buffer until the consumer asks. Clears `header.colorcorr`
    /// after applying so a re-encode doesn't double-apply.
    pub fn apply_colorcorr(&mut self) {
        if let Some([r, g, b]) = self.header.colorcorr.take() {
            // Skip the trivial 1,1,1 case so we don't burn N float muls.
            if (r - 1.0).abs() > f32::EPSILON
                || (g - 1.0).abs() > f32::EPSILON
                || (b - 1.0).abs() > f32::EPSILON
            {
                for px in self.pixels.chunks_exact_mut(3) {
                    px[0] *= r;
                    px[1] *= g;
                    px[2] *= b;
                }
            }
        }
    }
}
