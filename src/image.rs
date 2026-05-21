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

use crate::header::HdrHeader;

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
    fn apply_colorcorr_unit_vector_is_a_no_op() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.7, 0.5, 0.3]);
        img.header.colorcorr = Some([1.0, 1.0, 1.0]);
        img.apply_colorcorr();
        assert!(img.header.colorcorr.is_none());
        assert!((img.pixels[0] - 0.7).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.3).abs() < 1e-6);
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
