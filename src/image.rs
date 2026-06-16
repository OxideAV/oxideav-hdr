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
    fn scene_referred_luminance_equals_file_luminance_without_records() {
        // No EXPOSURE / COLORCORR: scene-referred recovery is the
        // identity, so the physical-luminance buffer must agree with the
        // file-referred `luminance_buffer` exactly.
        let pixels = vec![1.0, 0.5, 0.25, 0.1, 0.8, 0.3];
        let img = HdrImage::new_rgb96f(2, 1, pixels);
        assert!(img.header.exposure.is_none());
        assert!(img.header.colorcorr.is_none());
        let file = img.luminance_buffer();
        let scene = img.scene_referred_luminance_buffer();
        assert_eq!(file.len(), scene.len());
        for (f, s) in file.iter().zip(scene.iter()) {
            assert!((f - s).abs() < 1e-3, "{f} vs {s}");
        }
    }

    #[test]
    fn scene_referred_luminance_divides_out_exposure() {
        // EXPOSURE=4 was baked in; stored pixel = radiance * 4. The
        // physical luminance must be computed on radiance = stored / 4,
        // i.e. exactly 1/4 of the file-referred luminance.
        let pixels = vec![1.0, 1.0, 1.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(4.0);
        let scene = img.scene_referred_luminance_buffer();
        // recovered = (0.25,0.25,0.25) ⇒ 179 * 0.25 = 44.75.
        assert!((scene[0] - 179.0 * 0.25).abs() < 1e-2, "{}", scene[0]);
        // Non-mutating: the header slot and pixels are untouched.
        assert_eq!(img.header.exposure, Some(4.0));
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn scene_referred_luminance_divides_out_colorcorr_per_channel() {
        // COLORCORR=(2,4,5) was baked in; recover by the per-channel
        // reciprocal before the 179*(0.265R+0.670G+0.065B) projection.
        let pixels = vec![2.0, 4.0, 5.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let scene = img.scene_referred_luminance_buffer();
        // recovered = (1,1,1) ⇒ 179 * (0.265+0.670+0.065) = 179.
        assert!((scene[0] - 179.0).abs() < 1e-2, "{}", scene[0]);
        assert_eq!(img.header.colorcorr, Some([2.0, 4.0, 5.0]));
    }

    #[test]
    fn scene_referred_luminance_composes_exposure_and_colorcorr() {
        // Both records present: divide by the EXPOSURE product *and* the
        // per-channel COLORCORR triple before projecting.
        let pixels = vec![6.0, 12.0, 15.0]; // = radiance(1,1,1) * 3 * (2,4,5)
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let scene = img.scene_referred_luminance_buffer();
        assert!((scene[0] - 179.0).abs() < 1e-2, "{}", scene[0]);
    }

    #[test]
    fn scene_referred_luminance_xyze_uses_y_after_recovery() {
        use crate::HdrFormat;
        // XYZE: luminance is 179 * Y of the recovered channels. COLORCORR
        // applies to the three stored channels (X,Y,Z) in order, matching
        // `recover_original_colorcorr`.
        let pixels = vec![0.2, 2.0, 0.6]; // Y stored = 2.0
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.format = HdrFormat::Xyze;
        img.header.exposure = Some(2.0);
        img.header.colorcorr = Some([1.0, 4.0, 1.0]);
        let scene = img.scene_referred_luminance_buffer();
        // recovered Y = 2.0 / 2.0 / 4.0 = 0.25 ⇒ 179 * 0.25 = 44.75.
        assert!((scene[0] - 179.0 * 0.25).abs() < 1e-2, "{}", scene[0]);
    }

    #[test]
    fn scene_referred_luminance_treats_degenerate_records_as_identity() {
        // A zero / non-finite cumulative factor must not poison the
        // buffer with NaN / ∞ — it is treated as "no recovery applied",
        // matching recover_original_radiance / recover_original_colorcorr.
        let pixels = vec![1.0, 1.0, 1.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(0.0);
        img.header.colorcorr = Some([f32::NAN, 1.0, 1.0]);
        let scene = img.scene_referred_luminance_buffer();
        assert!(scene[0].is_finite(), "{}", scene[0]);
        // Identity recovery ⇒ same as the file-referred luminance.
        assert!((scene[0] - 179.0).abs() < 1e-2, "{}", scene[0]);
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
    fn square_pixel_dimensions_identity_for_square_pixels() {
        // No PIXASPECT record → square pixels → the displayed shape is
        // exactly the sample-grid dimensions.
        let img = HdrImage::new_rgb96f(64, 48, vec![0.0; 64 * 48 * 3]);
        let (w, h) = img.square_pixel_dimensions();
        assert!((w - 64.0).abs() < 1e-4);
        assert!((h - 48.0).abs() < 1e-4);
        // Naive sample-grid ratio and display ratio coincide for square
        // pixels.
        assert!((img.display_aspect_ratio() - 64.0 / 48.0).abs() < 1e-5);
    }

    #[test]
    fn square_pixel_dimensions_stretches_height_by_pixaspect() {
        // Spec §1: PIXASPECT = pixel height / pixel width. A factor of 2
        // means each pixel is twice as tall as wide, so the displayed
        // picture is twice as tall as the sample grid: width unchanged,
        // height doubled.
        let mut img = HdrImage::new_rgb96f(100, 50, vec![0.0; 100 * 50 * 3]);
        img.header.pixaspect = Some(2.0);
        let (w, h) = img.square_pixel_dimensions();
        assert!((w - 100.0).abs() < 1e-4, "{w}");
        assert!((h - 100.0).abs() < 1e-4, "{h}");
    }

    #[test]
    fn square_pixel_dimensions_compresses_height_for_subunit_pixaspect() {
        // PIXASPECT < 1 → pixels wider than tall → displayed height is a
        // fraction of the sample-grid height.
        let mut img = HdrImage::new_rgb96f(80, 80, vec![0.0; 80 * 80 * 3]);
        img.header.pixaspect = Some(0.5);
        let (w, h) = img.square_pixel_dimensions();
        assert!((w - 80.0).abs() < 1e-4, "{w}");
        assert!((h - 40.0).abs() < 1e-4, "{h}");
    }

    #[test]
    fn display_aspect_ratio_differs_from_sample_grid_for_nonsquare_pixels() {
        // Spec §1 warns PIXASPECT is "Not the image aspect ratio". A
        // square 512×512 sample grid stored with PIXASPECT=2 should be
        // shown at a 1:2 (wide:tall) display ratio = 0.5, even though the
        // naive grid ratio is 1.0.
        let mut img = HdrImage::new_rgb96f(512, 512, vec![0.0; 512 * 512 * 3]);
        img.header.pixaspect = Some(2.0);
        assert!((img.display_aspect_ratio() - 0.5).abs() < 1e-5);
        // The sample grid itself stays square.
        assert_eq!((img.width, img.height), (512, 512));
    }

    #[test]
    fn square_pixel_dimensions_folds_cumulative_pixaspect_product() {
        // The decoder folds multiple PIXASPECT= records into the running
        // product in header.pixaspect, so the helper sees the combined
        // factor (here 0.5 * 4.0 = 2.0).
        let mut img = HdrImage::new_rgb96f(10, 30, vec![0.0; 10 * 30 * 3]);
        img.header.pixaspect = Some(0.5 * 4.0);
        let (w, h) = img.square_pixel_dimensions();
        assert!((w - 10.0).abs() < 1e-4, "{w}");
        assert!((h - 60.0).abs() < 1e-4, "{h}");
    }

    #[test]
    fn square_pixel_dimensions_degenerate_pixaspect_is_identity() {
        // A 0.0 or non-finite cumulative factor is treated as the 1.0
        // identity (the permissive handling the recover_* helpers use), so
        // a malformed PIXASPECT can never produce a 0 / non-finite display
        // size.
        for bad in [0.0_f32, f32::NAN, f32::INFINITY, -1.0] {
            let mut img = HdrImage::new_rgb96f(20, 10, vec![0.0; 20 * 10 * 3]);
            img.header.pixaspect = Some(bad);
            let (w, h) = img.square_pixel_dimensions();
            assert!(w.is_finite() && h.is_finite(), "bad={bad}");
            assert!((w - 20.0).abs() < 1e-4, "bad={bad} w={w}");
            assert!((h - 10.0).abs() < 1e-4, "bad={bad} h={h}");
            assert!(img.display_aspect_ratio().is_finite());
        }
    }

    #[test]
    fn display_aspect_ratio_zero_height_returns_one() {
        // Degenerate zero-height picture: no sensible ratio exists, so the
        // helper returns 1.0 rather than a non-finite value.
        let img = HdrImage::new_rgb96f(8, 0, vec![]);
        assert_eq!(img.display_aspect_ratio(), 1.0);
        let (w, h) = img.square_pixel_dimensions();
        assert!((w - 8.0).abs() < 1e-4);
        assert_eq!(h, 0.0);
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
    fn recover_original_radiance_divides_pixels_and_clears_header() {
        // Per the staged spec: stored = original × EXPOSURE; recover
        // original by dividing. With EXPOSURE=0.5 the stored 0.5 maps
        // back to a scene-referred 1.0; the stored 0.25 maps back to
        // 0.5. The slot is cleared after recovery.
        let mut img = HdrImage::new_rgb96f(1, 2, vec![0.5, 0.25, 0.125, 1.0, 0.5, 0.25]);
        img.header.exposure = Some(0.5);
        img.recover_original_radiance();
        assert!(
            img.header.exposure.is_none(),
            "exposure slot not cleared after recovery"
        );
        // Pixel 0: 0.5 / 0.5 = 1.0
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.25).abs() < 1e-6);
        // Pixel 1: 1.0 / 0.5 = 2.0
        assert!((img.pixels[3] - 2.0).abs() < 1e-6);
        assert!((img.pixels[4] - 1.0).abs() < 1e-6);
        assert!((img.pixels[5] - 0.5).abs() < 1e-6);
        // Second call is a no-op (slot already None).
        img.recover_original_radiance();
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recover_original_radiance_with_none_does_nothing() {
        // Spec: "No EXPOSURE ⇒ none applied." Method is a no-op when the
        // slot is absent.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![1.0, 0.5, 0.25]);
        assert!(img.header.exposure.is_none());
        img.recover_original_radiance();
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn recover_original_radiance_unit_factor_is_a_no_op() {
        // EXPOSURE=1.0: division by 1.0 is the identity. Pixels stay
        // untouched, the slot is still cleared.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.125]);
        img.header.exposure = Some(1.0);
        img.recover_original_radiance();
        assert!(img.header.exposure.is_none());
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
        assert!((img.pixels[1] - 0.25).abs() < 1e-6);
        assert!((img.pixels[2] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn recover_original_radiance_zero_factor_does_not_blow_up() {
        // A literal EXPOSURE=0 record is degenerate (division would
        // produce non-finite values). The method clears the slot but
        // leaves the pixels untouched rather than emitting NaN/inf.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.125]);
        img.header.exposure = Some(0.0);
        img.recover_original_radiance();
        assert!(img.header.exposure.is_none());
        assert!(img.pixels[0].is_finite());
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
        assert!((img.pixels[1] - 0.25).abs() < 1e-6);
        assert!((img.pixels[2] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn recover_original_radiance_inverts_apply_exposure() {
        // apply_exposure multiplies, recover_original_radiance divides.
        // Round-trip through both should land back at the original
        // float buffer within f32 precision.
        let original = vec![0.7_f32, 0.5, 0.3, 0.4, 0.25, 0.15];
        let mut img = HdrImage::new_rgb96f(2, 1, original.clone());
        img.header.exposure = Some(0.5);
        img.apply_exposure();
        // After apply: header None, pixels multiplied.
        img.header.exposure = Some(0.5);
        img.recover_original_radiance();
        for (i, (&a, &b)) in original.iter().zip(img.pixels.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "pixel {i}: {a} vs {b}");
        }
    }

    #[test]
    fn recover_original_radiance_undoes_stacked_exposures() {
        // The decoder folds multiple EXPOSURE records into the running
        // product, so a single division by that product undoes the
        // whole stack — this matches the spec wording "divide file
        // values by the product of all EXPOSURE settings". Stored
        // values came from original × (0.5 × 0.25) = original × 0.125;
        // dividing by 0.125 should recover original.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.125, 0.0625, 0.03125]);
        img.header.exposure = Some(0.5 * 0.25);
        img.recover_original_radiance();
        assert!(img.header.exposure.is_none());
        assert!((img.pixels[0] - 1.0).abs() < 1e-5);
        assert!((img.pixels[1] - 0.5).abs() < 1e-5);
        assert!((img.pixels[2] - 0.25).abs() < 1e-5);
    }

    #[test]
    fn recover_original_colorcorr_divides_per_channel_and_clears_header() {
        // Stored channels = original × COLORCORR. With COLORCORR=2,4,8,
        // the stored (2.0, 4.0, 8.0) maps back to (1.0, 1.0, 1.0).
        let mut img = HdrImage::new_rgb96f(2, 1, vec![2.0, 4.0, 8.0, 1.0, 2.0, 4.0]);
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.recover_original_colorcorr();
        assert!(
            img.header.colorcorr.is_none(),
            "colorcorr slot not cleared after recovery"
        );
        // Pixel 0: (2/2, 4/4, 8/8) = (1, 1, 1)
        assert!((img.pixels[0] - 1.0).abs() < 1e-6);
        assert!((img.pixels[1] - 1.0).abs() < 1e-6);
        assert!((img.pixels[2] - 1.0).abs() < 1e-6);
        // Pixel 1: (1/2, 2/4, 4/8) = (0.5, 0.5, 0.5)
        assert!((img.pixels[3] - 0.5).abs() < 1e-6);
        assert!((img.pixels[4] - 0.5).abs() < 1e-6);
        assert!((img.pixels[5] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn recover_original_colorcorr_with_none_does_nothing() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.7, 0.5, 0.3]);
        assert!(img.header.colorcorr.is_none());
        img.recover_original_colorcorr();
        assert!((img.pixels[0] - 0.7).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn recover_original_colorcorr_unit_vector_is_a_no_op() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.7, 0.5, 0.3]);
        img.header.colorcorr = Some([1.0, 1.0, 1.0]);
        img.recover_original_colorcorr();
        assert!(img.header.colorcorr.is_none());
        assert!((img.pixels[0] - 0.7).abs() < 1e-6);
        assert!((img.pixels[1] - 0.5).abs() < 1e-6);
        assert!((img.pixels[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn recover_original_colorcorr_zero_component_does_not_blow_up() {
        // Any zero component is degenerate (division produces non-finite
        // values). Clear the slot, leave pixels untouched.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.125]);
        img.header.colorcorr = Some([2.0, 0.0, 4.0]);
        img.recover_original_colorcorr();
        assert!(img.header.colorcorr.is_none());
        for &v in &img.pixels {
            assert!(v.is_finite());
        }
        assert!((img.pixels[0] - 0.5).abs() < 1e-6);
        assert!((img.pixels[1] - 0.25).abs() < 1e-6);
        assert!((img.pixels[2] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn recover_original_colorcorr_inverts_apply_colorcorr() {
        let original = vec![0.6_f32, 0.4, 0.2];
        let mut img = HdrImage::new_rgb96f(1, 1, original.clone());
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.apply_colorcorr();
        // Restore the slot and recover.
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.recover_original_colorcorr();
        for (a, b) in original.iter().zip(img.pixels.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn effective_exposure_defaults_to_one_when_absent() {
        // Spec: "No EXPOSURE ⇒ none applied." Helper returns 1.0 (the
        // identity multiplier) when the slot is None.
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        assert!(img.header.exposure.is_none());
        assert!((img.effective_exposure() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn effective_exposure_returns_header_value_when_set() {
        // When the file declared (or the decoder folded multiple records
        // into) an EXPOSURE= value, the helper returns it verbatim.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.exposure = Some(0.5);
        assert!((img.effective_exposure() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn effective_exposure_returns_explicit_one_when_set() {
        // An explicit `EXPOSURE=1.0` and the no-record case both produce
        // 1.0 — the helper intentionally collapses both to the identity
        // factor because the multiplicative semantics are identical. The
        // caller that needs to distinguish "file declared it" from "file
        // omitted it" matches on header.exposure directly.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.exposure = Some(1.0);
        assert!((img.effective_exposure() - 1.0).abs() < 1e-6);
        assert_eq!(img.header.exposure, Some(1.0));
    }

    #[test]
    fn effective_exposure_does_not_perturb_header_slot() {
        // The helper reads the slot — it must not clear it (the
        // typed-slot inspector contract). Verified by re-reading the
        // header field after the call.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.exposure = Some(2.5);
        let _ = img.effective_exposure();
        assert_eq!(img.header.exposure, Some(2.5));
    }

    #[test]
    fn effective_colorcorr_defaults_to_unit_triple_when_absent() {
        // Spec: COLORCORR "should have unit brightness so it does not
        // change overall brightness"; absent record ⇒ the per-channel
        // identity triple [1.0, 1.0, 1.0].
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        assert!(img.header.colorcorr.is_none());
        let c = img.effective_colorcorr();
        assert!((c[0] - 1.0).abs() < 1e-6);
        assert!((c[1] - 1.0).abs() < 1e-6);
        assert!((c[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn effective_colorcorr_returns_header_value_when_set() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        let c = img.effective_colorcorr();
        assert!((c[0] - 2.0).abs() < 1e-6);
        assert!((c[1] - 4.0).abs() < 1e-6);
        assert!((c[2] - 8.0).abs() < 1e-6);
    }

    #[test]
    fn effective_colorcorr_returns_explicit_unit_triple_when_set() {
        // Explicit `COLORCORR=1 1 1` and absent-record both produce
        // [1, 1, 1]. The helper folds them; callers needing the
        // distinction match the typed slot.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.colorcorr = Some([1.0, 1.0, 1.0]);
        let c = img.effective_colorcorr();
        assert!((c[0] - 1.0).abs() < 1e-6);
        assert!((c[1] - 1.0).abs() < 1e-6);
        assert!((c[2] - 1.0).abs() < 1e-6);
        assert_eq!(img.header.colorcorr, Some([1.0, 1.0, 1.0]));
    }

    #[test]
    fn effective_colorcorr_does_not_perturb_header_slot() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.colorcorr = Some([0.7, 0.5, 0.3]);
        let _ = img.effective_colorcorr();
        assert_eq!(img.header.colorcorr, Some([0.7, 0.5, 0.3]));
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

    /// The picture's dimensions corrected for non-square pixels, as a
    /// floating-point `(width, height)` pair in *square-pixel* units —
    /// i.e. the shape the image must be drawn at so it isn't displayed
    /// distorted.
    ///
    /// The staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1
    /// PIXASPECT row) defines `PIXASPECT=` as the *pixel* aspect ratio,
    /// "pixel height / pixel width", and warns it is explicitly **not**
    /// the image aspect ratio. A `PIXASPECT` of `p` therefore means each
    /// stored pixel is `p` times as tall as it is wide; a consumer that
    /// ignores the record and draws the `width × height` sample grid on a
    /// square-pixel display squashes the picture vertically by the factor
    /// `p`. Restoring the intended geometry stretches the height axis by
    /// `p`, leaving the width axis unchanged: the displayed shape is
    /// `(width, height * p)`. The cumulative `PIXASPECT` product the
    /// decoder folds into [`HdrHeader::pixaspect`] is used via
    /// [`Self::effective_pixaspect`], so multiple `PIXASPECT=` records and
    /// the absent-record default of `1.0` (square pixels, returns the
    /// stored dimensions unchanged) are both handled.
    ///
    /// Returns floats because the corrected height is generally
    /// fractional; the stored integer [`Self::width`] / [`Self::height`]
    /// are the sample-grid dimensions and are untouched. The picture's
    /// *sample count* and on-disk layout do not change — only the
    /// proportions a viewer should present. A degenerate cumulative factor
    /// (`0.0` or non-finite) is treated as the `1.0` identity, matching
    /// the permissive handling the `recover_*` helpers use, so a malformed
    /// `PIXASPECT=` can never yield a `0` or non-finite display size.
    pub fn square_pixel_dimensions(&self) -> (f32, f32) {
        let p = self.effective_pixaspect();
        let p = if p > 0.0 && p.is_finite() { p } else { 1.0 };
        (self.width as f32, self.height as f32 * p)
    }

    /// The aspect ratio (width ÷ height) the picture should be *displayed*
    /// at once its non-square pixels are accounted for — the proportions
    /// the spec's PIXASPECT record exists to communicate.
    ///
    /// Derived from [`Self::square_pixel_dimensions`]: with `PIXASPECT=p`
    /// meaning each pixel is `p` times as tall as wide (staged spec §1
    /// PIXASPECT row, "pixel height / pixel width"), the displayed shape
    /// is `(width, height * p)` square-pixel units, so the displayed
    /// width-to-height ratio is `width / (height * p)`. The spec is
    /// explicit that this is *not* the same as the naive sample-grid ratio
    /// `width / height` — that equality holds only for square pixels
    /// (`PIXASPECT` absent or `1.0`). For example a `512 × 512` picture
    /// stored with `PIXASPECT=2` is meant to be shown at a 1:2
    /// (wide:tall) display ratio, i.e. `display_aspect_ratio() == 0.5`,
    /// even though its sample grid is square.
    ///
    /// Returns `1.0` for a zero-height picture (no sensible ratio exists)
    /// rather than producing a non-finite value, and folds the same
    /// degenerate-`PIXASPECT` guard as [`Self::square_pixel_dimensions`].
    pub fn display_aspect_ratio(&self) -> f32 {
        let (w, h) = self.square_pixel_dimensions();
        if h > 0.0 {
            w / h
        } else {
            1.0
        }
    }

    /// Cumulative `EXPOSURE=` multiplier the picture declared, with the
    /// staged spec's "no `EXPOSURE` ⇒ none applied" default of `1.0`
    /// substituted when no record was present.
    ///
    /// Per the staged spec
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 EXPOSURE row),
    /// `EXPOSURE=` is a single-float, cumulative-multiplicative scalar
    /// that the writer has already folded into the stored pixels. The
    /// decoder collapses multiple records into the running product in
    /// [`HdrHeader::exposure`], and the spec is explicit that the
    /// absence of any record means no multiplier was applied (the
    /// identity factor `1.0`). Through round 251 the only way to read
    /// the cumulative factor with the spec-documented default applied
    /// was to write the `header.exposure.unwrap_or(1.0)` two-token
    /// boilerplate at every call site; this helper does the
    /// substitution in one call without perturbing
    /// [`HdrHeader::exposure`], so callers that need to distinguish
    /// "file declared `EXPOSURE=1.0` explicitly" from "no record was
    /// present" can still match on the typed slot directly.
    ///
    /// Mirrors [`Self::effective_pixaspect`] / [`Self::effective_primaries`]
    /// in shape: a single `f32`-returning method that pre-applies the
    /// spec-documented default for the most common
    /// "what did the file say, or what should I assume" case.
    pub fn effective_exposure(&self) -> f32 {
        self.header.exposure.unwrap_or(1.0)
    }

    /// Cumulative `COLORCORR=` per-channel multiplier the picture
    /// declared, with the staged spec's "should have unit brightness"
    /// default of `[1.0, 1.0, 1.0]` substituted when no record was
    /// present.
    ///
    /// Per the staged spec
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 COLORCORR row),
    /// `COLORCORR=` is a 3-float cumulative-multiplicative
    /// per-primary correction the writer has already folded into the
    /// stored channels. The decoder collapses multiple records into the
    /// element-wise product in [`HdrHeader::colorcorr`]; the spec
    /// treats the absence of any record as the per-channel identity
    /// triple `[1.0, 1.0, 1.0]` (the "should have unit brightness"
    /// invariant the spec spells out, applied to the absent-record
    /// case). Through round 251 the only way to read the cumulative
    /// triple with the spec-documented default applied was to write
    /// the `header.colorcorr.unwrap_or([1.0; 3])` boilerplate at every
    /// call site; this helper does the substitution in one call
    /// without perturbing [`HdrHeader::colorcorr`], so callers that
    /// need to distinguish "file declared `COLORCORR=1 1 1` explicitly"
    /// from "no record was present" can still match on the typed slot
    /// directly.
    ///
    /// Mirrors [`Self::effective_exposure`] /
    /// [`Self::effective_pixaspect`] / [`Self::effective_primaries`]
    /// in shape.
    pub fn effective_colorcorr(&self) -> [f32; 3] {
        self.header.colorcorr.unwrap_or([1.0, 1.0, 1.0])
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

    /// Divide each float channel by the header's cumulative `EXPOSURE`
    /// factor to reconstruct the original scene-referred radiance, then
    /// clear the header slot.
    ///
    /// The staged spec
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1 EXPOSURE row)
    /// documents `EXPOSURE=` as a multiplier *already applied to* the
    /// stored pixels: the on-disk channel `c_i` equals `original_i *
    /// EXPOSURE`, and recovering the original radiance in physical units
    /// (watts/sr/m²) is the divide-by-the-product operation the spec
    /// describes verbatim — "to recover original radiances divide file
    /// values by the product of all `EXPOSURE` settings". When the
    /// header carries multiple `EXPOSURE=` records the decoder already
    /// folds them into the running product stored in
    /// [`HdrHeader::exposure`], so a single division by that field
    /// undoes the entire stack.
    ///
    /// This is the spec-canonical recovery operation, complementary to
    /// [`Self::apply_exposure`]: where `apply_exposure` post-multiplies
    /// the buffer by the recorded factor (useful for re-applying an
    /// exposure adjustment to the float samples on the way to a
    /// display-side tone-mapper), this method removes the factor that
    /// the writer already baked in. Pick the one that matches the
    /// numerical contract your downstream pipeline expects.
    ///
    /// A `None` slot is treated as the spec-documented absence of an
    /// `EXPOSURE=` record ("No `EXPOSURE` ⇒ none applied") and the
    /// method is a no-op. An exact-`1.0` factor is also a no-op since
    /// division would be the identity and the slot is cleared anyway.
    /// A second call is a no-op (the slot is `None` after the first).
    /// A `0.0` factor is rejected as a no-op (division would produce
    /// non-finite values; the spec treats a literal zero exposure as a
    /// malformed-but-permissive record). The slot is still cleared so
    /// callers don't see the offending value on a re-encode.
    pub fn recover_original_radiance(&mut self) {
        if let Some(e) = self.header.exposure.take() {
            if e == 0.0 || !e.is_finite() {
                return;
            }
            if (e - 1.0).abs() > f32::EPSILON {
                let inv = 1.0 / e;
                for v in &mut self.pixels {
                    *v *= inv;
                }
            }
        }
    }

    /// Divide each float channel by its corresponding component of the
    /// header's cumulative `COLORCORR` triple to reconstruct the
    /// original per-primary radiance, then clear the header slot.
    ///
    /// The staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md` §1
    /// COLORCORR row) describes the record as a per-primary multiplier
    /// "already applied" to the stored channels, complementary to
    /// `EXPOSURE` but tracking per-primary colour correction rather than
    /// overall brightness. Recovering the pre-correction channels is
    /// therefore the per-component reciprocal of [`Self::apply_colorcorr`]
    /// — the same divide-to-undo idiom the EXPOSURE counterpart uses.
    ///
    /// When the header carries multiple `COLORCORR=` records the decoder
    /// already folds them into the element-wise product stored in
    /// [`HdrHeader::colorcorr`], so a single per-channel division undoes
    /// the entire stack. A `None` slot, the trivial `1.0, 1.0, 1.0`
    /// triple, and any component that is `0.0` or non-finite are all
    /// treated as no-ops (the spec considers a zero per-channel
    /// correction degenerate; division would produce non-finite values).
    /// The slot is still cleared so callers don't see the offending
    /// values on a re-encode. A second call is a no-op.
    pub fn recover_original_colorcorr(&mut self) {
        if let Some([r, g, b]) = self.header.colorcorr.take() {
            if r == 0.0
                || g == 0.0
                || b == 0.0
                || !r.is_finite()
                || !g.is_finite()
                || !b.is_finite()
            {
                return;
            }
            if (r - 1.0).abs() > f32::EPSILON
                || (g - 1.0).abs() > f32::EPSILON
                || (b - 1.0).abs() > f32::EPSILON
            {
                let (ir, ig, ib) = (1.0 / r, 1.0 / g, 1.0 / b);
                for px in self.pixels.chunks_exact_mut(3) {
                    px[0] *= ir;
                    px[1] *= ig;
                    px[2] *= ib;
                }
            }
        }
    }

    /// Allocate a fresh `width * height` buffer of per-pixel *physical*
    /// photometric luminance values, in lumens per steradian per m²,
    /// computed from the picture's **scene-referred** radiance — i.e.
    /// after dividing out the multipliers the writer baked into the
    /// stored channels — rather than from the stored float samples
    /// directly.
    ///
    /// This is the spec-canonical composition of two rules in the staged
    /// spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`):
    ///
    /// 1. §1 (EXPOSURE / COLORCORR header rows): the stored channel
    ///    `c_i` equals `radiance_i * EXPOSURE * COLORCORR_i` because both
    ///    records are "already applied to" the pixels; "to recover
    ///    original radiances (watts/sr/m²) divide file values by the
    ///    product of all EXPOSURE settings", and COLORCORR is the
    ///    complementary per-primary multiplier removed the same way. The
    ///    decoder folds multiple records of each kind into the running
    ///    product stored in [`HdrHeader::exposure`] /
    ///    [`HdrHeader::colorcorr`], so a single division by each undoes
    ///    the entire stack.
    /// 2. §"Physical interpretation": the photometric luminance of a
    ///    scene-referred radiance pixel is `179 * (0.265 R + 0.670 G +
    ///    0.065 B)` for `FORMAT=32-bit_rle_rgbe` and `179 * Y` for
    ///    `FORMAT=32-bit_rle_xyze`.
    ///
    /// Where [`Self::luminance_buffer`] applies the §"Physical
    /// interpretation" formula to the stored float samples verbatim (the
    /// right answer when no `EXPOSURE=` / `COLORCORR=` was baked in, or
    /// when the caller wants file-referred luminance), this method first
    /// reconstructs the original radiance the spec describes, so the
    /// returned luminance is the genuine physical quantity for files that
    /// do carry those records. When neither record is present (or both
    /// are the identity) the two methods agree exactly.
    ///
    /// Non-mutating: the image's pixels and header are left untouched, so
    /// it composes the
    /// [`Self::recover_original_radiance`] / [`Self::recover_original_colorcorr`]
    /// divide-to-undo rules without the in-place clear those methods
    /// perform. A degenerate cumulative factor (`EXPOSURE` that is `0.0`
    /// or non-finite, or any `COLORCORR` component that is `0.0` or
    /// non-finite) is treated as "no recovery applied" for that record —
    /// the same permissive handling [`Self::recover_original_radiance`] /
    /// [`Self::recover_original_colorcorr`] use, so a malformed header can
    /// never turn the luminance buffer into NaN / ∞.
    pub fn scene_referred_luminance_buffer(&self) -> Vec<f32> {
        // Reciprocal of the cumulative EXPOSURE multiplier (identity for
        // absent / degenerate records), per spec §1 + the recovery
        // contract of `recover_original_radiance`.
        let inv_exposure = match self.header.exposure {
            Some(e) if e != 0.0 && e.is_finite() => 1.0 / e,
            _ => 1.0,
        };
        // Per-channel reciprocal of the cumulative COLORCORR triple
        // (identity for absent / degenerate components), per spec §1 +
        // the recovery contract of `recover_original_colorcorr`. The
        // triple multiplies the three stored channels in order, matching
        // `recover_original_colorcorr` for both RGBE (R,G,B) and XYZE
        // (X,Y,Z) storage.
        let inv_cc = match self.header.colorcorr {
            Some([r, g, b]) => [
                if r != 0.0 && r.is_finite() {
                    1.0 / r
                } else {
                    1.0
                },
                if g != 0.0 && g.is_finite() {
                    1.0 / g
                } else {
                    1.0
                },
                if b != 0.0 && b.is_finite() {
                    1.0 / b
                } else {
                    1.0
                },
            ],
            None => [1.0, 1.0, 1.0],
        };
        let n = (self.width as usize) * (self.height as usize);
        let mut out = Vec::with_capacity(n);
        for px in self.pixels.chunks_exact(3) {
            let recovered = [
                px[0] * inv_exposure * inv_cc[0],
                px[1] * inv_exposure * inv_cc[1],
                px[2] * inv_exposure * inv_cc[2],
            ];
            out.push(crate::xyz::luminance_lm_per_sr_per_m2(
                recovered,
                self.header.format,
            ));
        }
        out
    }
}
