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

use crate::header::{GeometricOp, HdrHeader, Orientation, Primaries};

// ---------------------------------------------------------------------------
// Geometric reorientation primitives (the §2 resolution-string orientation
// matrix, applied to the *displayed* float buffer)
// ---------------------------------------------------------------------------
//
// A decoded `HdrImage` always carries its `pixels` buffer in canonical
// standard display order — top-down, left-to-right, the picture seen
// right-side-up (`docs/image/hdr/radiance-hdr-rgbe-format.md` §2: the
// `-Y N +X M` standard form "scanlines run from the upper-left across to
// upper-right, then down the picture"). The on-disk axis flags only choose
// the file byte order; the decoder normalises every flavour to this one
// display layout, and the encoder reorients back out.
//
// These helpers transform the *picture content* itself — rotate the image
// 90°, mirror it, etc. — so callers holding a decoded buffer can apply or
// undo the eight geometric symmetries the format note's §2 table enumerates.
// Each operates on packed RGB f32 triples (3 components per pixel) and is a
// pure pixel permutation: no value is altered, only its (x, y) position.

/// Mirror the picture left↔right (reflect across the vertical centre line).
/// Dimensions are unchanged. Pixel `(x, y)` moves to `(w-1-x, y)`.
fn buf_flip_horizontal(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let dst = (y * width + (width - 1 - x)) * 3;
            out[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
        }
    }
    out
}

/// Mirror the picture top↔bottom (reflect across the horizontal centre
/// line). Dimensions are unchanged. Pixel `(x, y)` moves to `(x, h-1-y)`.
fn buf_flip_vertical(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let row = width * 3;
    let mut out = vec![0.0f32; pixels.len()];
    for y in 0..height {
        let src = y * row;
        let dst = (height - 1 - y) * row;
        out[dst..dst + row].copy_from_slice(&pixels[src..src + row]);
    }
    out
}

/// Rotate the picture 180°. Dimensions are unchanged. Pixel `(x, y)` moves
/// to `(w-1-x, h-1-y)` — the composition of a horizontal and a vertical
/// flip, done in one pass.
fn buf_rotate_180(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let n = width * height;
    let mut out = vec![0.0f32; pixels.len()];
    for i in 0..n {
        let src = i * 3;
        let dst = (n - 1 - i) * 3;
        out[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
    }
    out
}

/// Transpose across the main diagonal (`(x, y) -> (y, x)`). The output
/// dimensions are swapped: a `w × h` picture becomes `h × w`. This is the
/// reflection that turns the four 90°-rotation orientations into the four
/// axis-aligned ones.
fn buf_transpose(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; pixels.len()];
    // Source `(x, y)` at `(y*width + x)*3`; dest (dims now height×width)
    // `(y, x)` at `(x*height + y)*3`.
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let dst = (x * height + y) * 3;
            out[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
        }
    }
    out
}

/// Rotate the picture 90° clockwise. Output dimensions are swapped
/// (`w × h` → `h × w`). A pixel at `(x, y)` in the source lands at
/// `(h-1-y, x)` in the rotated picture.
fn buf_rotate_90_cw(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    // Output is `out_w = height` wide, `out_h = width` tall.
    let out_w = height;
    let mut out = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let (ox, oy) = (height - 1 - y, x);
            let dst = (oy * out_w + ox) * 3;
            out[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
        }
    }
    out
}

/// Rotate the picture 90° counter-clockwise. Output dimensions are swapped
/// (`w × h` → `h × w`). A pixel at `(x, y)` in the source lands at
/// `(y, w-1-x)` in the rotated picture.
fn buf_rotate_90_ccw(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    // Output is `out_w = height` wide, `out_h = width` tall.
    let out_w = height;
    let mut out = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let (ox, oy) = (y, width - 1 - x);
            let dst = (oy * out_w + ox) * 3;
            out[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_rgbe_quads_matches_per_pixel_encoder() {
        // The quad stream must be bit-identical to running rgb_to_rgbe on
        // each pixel in top-down order — the same quads the encoder writes.
        use crate::rgbe::rgb_to_rgbe;
        let pixels = vec![1.0_f32, 0.5, 0.25, 4.0, 2.0, 1.0, 0.0, 0.0, 0.0];
        let img = HdrImage::new_rgb96f(3, 1, pixels.clone());
        let quads = img.to_rgbe_quads();
        assert_eq!(quads.len(), 3);
        for (i, px) in pixels.chunks_exact(3).enumerate() {
            assert_eq!(quads[i], rgb_to_rgbe([px[0], px[1], px[2]]), "pixel {i}");
        }
        // Spec §3 worked example: (1.0, 0.5, 0.25) -> (128, 64, 32, 129).
        assert_eq!(quads[0], [128, 64, 32, 129]);
        // Black pixel -> all-zero sentinel.
        assert_eq!(quads[2], [0, 0, 0, 0]);
    }

    #[test]
    fn from_rgbe_quads_decodes_each_quad() {
        // Building from quads must decode each with rgbe_to_rgb into the
        // float buffer, top-down.
        use crate::rgbe::rgbe_to_rgb;
        let quads = [[128, 64, 32, 129], [200, 100, 50, 130], [0, 0, 0, 0]];
        let img = HdrImage::from_rgbe_quads(3, 1, &quads, HdrHeader::default());
        assert_eq!(img.width, 3);
        assert_eq!(img.height, 1);
        assert_eq!(img.pixels.len(), 9);
        for (i, &q) in quads.iter().enumerate() {
            let rgb = rgbe_to_rgb(q);
            assert_eq!(&img.pixels[i * 3..i * 3 + 3], &rgb[..], "pixel {i}");
        }
        // The worked-example quad decodes to (1.0, 0.5, 0.25).
        assert_eq!(&img.pixels[0..3], &[1.0, 0.5, 0.25]);
    }

    #[test]
    fn rgbe_quads_round_trip_bit_exactly_for_normalised_quads() {
        // The core bit-exact contract: a picture built from normalised
        // RGBE quads (dominant mantissa >= 128, magnitude above the 1e-32
        // black floor) re-encodes to *exactly* the same quads. Walk a
        // representative spread of exponents and mantissa shapes. The
        // smallest exponent byte sampled is 64: byte 1 (unbiased -127)
        // decodes to ~2^-128, below the 1e-32 floor rgb_to_rgbe flushes
        // to black, so it is intentionally *not* a member of the
        // bit-exact subset (the floor boundary is pinned by a dedicated
        // test below).
        let mut quads = Vec::new();
        for e in [64u8, 128, 129, 200, 255] {
            for &dom in &[128u8, 200, 255] {
                quads.push([dom, dom / 2, dom / 4, e]);
                quads.push([dom / 4, dom, dom / 2, e]);
                quads.push([dom / 2, dom / 4, dom, e]);
            }
        }
        // Include the black sentinel — it round-trips to itself too.
        quads.push([0, 0, 0, 0]);
        let n = quads.len() as u32;
        let img = HdrImage::from_rgbe_quads(n, 1, &quads, HdrHeader::default());
        let back = img.to_rgbe_quads();
        assert_eq!(back, quads, "normalised-quad round-trip drifted");
    }

    #[test]
    fn rgbe_quads_below_black_floor_flush_to_sentinel() {
        // A normalised-mantissa quad whose *decoded* magnitude sits below
        // the 1e-32 floor rgb_to_rgbe enforces does NOT round-trip to
        // itself — it collapses to the all-zero sentinel. Exponent byte 1
        // (unbiased -127) with the maximal mantissa 255 decodes to
        // ~255/256 * 2^-127 ≈ 1.5e-38, comfortably below 1e-32, so the
        // re-encode flushes it to black. This pins the lower boundary of
        // the bit-exact subset.
        let quads = [[255u8, 255, 255, 1]];
        let img = HdrImage::from_rgbe_quads(1, 1, &quads, HdrHeader::default());
        let back = img.to_rgbe_quads();
        assert_eq!(
            back[0],
            [0, 0, 0, 0],
            "sub-floor quad should flush to sentinel"
        );
    }

    #[test]
    fn from_rgbe_quads_preserves_supplied_header() {
        let header = HdrHeader {
            format: crate::HdrFormat::Xyze,
            exposure: Some(2.5),
            ..HdrHeader::default()
        };
        let quads = [[128, 64, 32, 129]];
        let img = HdrImage::from_rgbe_quads(1, 1, &quads, header.clone());
        assert_eq!(img.header.format, crate::HdrFormat::Xyze);
        assert_eq!(img.header.exposure, Some(2.5));
    }

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
    fn adjust_exposure_factor_scales_pixels_and_records_multiplier() {
        // No prior record: the slot seeds from the spec default 1.0 and
        // becomes Some(factor); pixels are multiplied by the factor.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![1.0, 0.5, 0.25]);
        assert!(img.adjust_exposure_factor(4.0));
        assert_eq!(img.header.exposure, Some(4.0));
        assert!((img.pixels[0] - 4.0).abs() < 1e-6);
        assert!((img.pixels[1] - 2.0).abs() < 1e-6);
        assert!((img.pixels[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn adjust_exposure_factor_stacks_with_existing_record() {
        // EXPOSURE is cumulative per spec §1: an existing record folds
        // multiplicatively with the new factor.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![1.0, 1.0, 1.0]);
        img.header.exposure = Some(3.0);
        assert!(img.adjust_exposure_factor(2.0));
        assert_eq!(img.header.exposure, Some(6.0));
        assert!((img.pixels[0] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn adjust_exposure_preserves_scene_referred_radiance() {
        // The whole point of recording the multiplier: the recovered
        // scene radiance is invariant across the adjustment.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.125]);
        img.header.exposure = Some(2.0);
        let before = img.scene_referred_radiance_buffer();
        assert!(img.adjust_exposure_stops(3));
        let after = img.scene_referred_radiance_buffer();
        for (b, a) in before.iter().zip(after.iter()) {
            assert!((b - a).abs() < 1e-6, "{b} vs {a}");
        }
        // And recover_original_radiance lands on the same values.
        img.recover_original_radiance();
        for (b, a) in before.iter().zip(img.pixels.iter()) {
            assert!((b - a).abs() < 1e-6, "{b} vs {a}");
        }
    }

    #[test]
    fn adjust_exposure_stops_round_trip_is_bit_exact() {
        // 2^n multiplication only moves the f32 exponent field, so
        // +n then -n restores every sample bit-for-bit.
        let original = vec![0.3_f32, 1.7, 0.001, 250.0, 5e-20, 3e18];
        let mut img = HdrImage::new_rgb96f(2, 1, original.clone());
        assert!(img.adjust_exposure_stops(5));
        assert!(img.adjust_exposure_stops(-5));
        for (o, p) in original.iter().zip(img.pixels.iter()) {
            assert_eq!(o.to_bits(), p.to_bits(), "{o} vs {p}");
        }
        // The two stop records fold to exactly 1.0 (2^5 * 2^-5).
        assert_eq!(img.header.exposure, Some(1.0));
    }

    #[test]
    fn adjust_exposure_rejects_degenerate_factors() {
        let original = vec![1.0_f32, 0.5, 0.25];
        let mut img = HdrImage::new_rgb96f(1, 1, original.clone());
        for bad in [0.0_f32, -2.0, f32::NAN, f32::INFINITY] {
            assert!(!img.adjust_exposure_factor(bad), "{bad} must be rejected");
        }
        // 2^stops overflows / underflows f32 beyond ~±126 stops.
        assert!(!img.adjust_exposure_stops(1000));
        assert!(!img.adjust_exposure_stops(-1000));
        assert_eq!(img.pixels, original);
        assert!(img.header.exposure.is_none());
    }

    #[test]
    fn adjust_exposure_unit_factor_and_zero_stops_are_no_ops() {
        // factor 1.0 / stops 0 succeed but do not materialise an
        // explicit EXPOSURE=1 record or touch the pixels.
        let original = vec![1.0_f32, 0.5, 0.25];
        let mut img = HdrImage::new_rgb96f(1, 1, original.clone());
        assert!(img.adjust_exposure_factor(1.0));
        assert!(img.adjust_exposure_stops(0));
        assert_eq!(img.pixels, original);
        assert!(img.header.exposure.is_none());
        // With an existing record the slot is equally untouched.
        img.header.exposure = Some(3.0);
        assert!(img.adjust_exposure_stops(0));
        assert_eq!(img.header.exposure, Some(3.0));
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
        // XYZE: luminance is the stored Y verbatim — per the staged
        // spec's §"Physical interpretation" the Y primary is already
        // lumens/sr/m², so no 179× efficacy factor is applied.
        assert!((lum[0] - 0.5).abs() < 1e-6);
        assert!((lum[1] - 1.0).abs() < 1e-6);
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
        // XYZE: luminance is the recovered Y verbatim (no 179× — the
        // stored Y is already photometric per the staged spec). COLORCORR
        // applies to the three stored channels (X,Y,Z) in order, matching
        // `recover_original_colorcorr`.
        let pixels = vec![0.2, 2.0, 0.6]; // Y stored = 2.0
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.format = HdrFormat::Xyze;
        img.header.exposure = Some(2.0);
        img.header.colorcorr = Some([1.0, 4.0, 1.0]);
        let scene = img.scene_referred_luminance_buffer();
        // recovered Y = 2.0 / 2.0 / 4.0 = 0.25.
        assert!((scene[0] - 0.25).abs() < 1e-6, "{}", scene[0]);
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
    fn scene_referred_radiance_equals_pixels_without_records() {
        // No EXPOSURE / COLORCORR: recovery is the identity, so the
        // recovered RGB buffer equals the stored pixels exactly.
        let pixels = vec![1.0, 0.5, 0.25, 0.1, 0.8, 0.3];
        let img = HdrImage::new_rgb96f(2, 1, pixels.clone());
        assert!(img.header.exposure.is_none());
        assert!(img.header.colorcorr.is_none());
        let scene = img.scene_referred_radiance_buffer();
        assert_eq!(scene.len(), pixels.len());
        for (p, s) in pixels.iter().zip(scene.iter()) {
            assert!((p - s).abs() < 1e-6, "{p} vs {s}");
        }
    }

    #[test]
    fn scene_referred_radiance_divides_out_exposure() {
        // EXPOSURE=4 baked in: stored = radiance * 4. Recovered RGB must
        // be exactly stored / 4 on every channel.
        let pixels = vec![4.0, 2.0, 1.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(4.0);
        let scene = img.scene_referred_radiance_buffer();
        assert!((scene[0] - 1.0).abs() < 1e-6, "{}", scene[0]);
        assert!((scene[1] - 0.5).abs() < 1e-6, "{}", scene[1]);
        assert!((scene[2] - 0.25).abs() < 1e-6, "{}", scene[2]);
        // Non-mutating: header slot + pixels untouched.
        assert_eq!(img.header.exposure, Some(4.0));
        assert!((img.pixels[0] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn scene_referred_radiance_divides_out_colorcorr_per_channel() {
        // COLORCORR=(2,4,5): recover by the per-channel reciprocal.
        let pixels = vec![2.0, 4.0, 5.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let scene = img.scene_referred_radiance_buffer();
        assert!((scene[0] - 1.0).abs() < 1e-6, "{}", scene[0]);
        assert!((scene[1] - 1.0).abs() < 1e-6, "{}", scene[1]);
        assert!((scene[2] - 1.0).abs() < 1e-6, "{}", scene[2]);
        assert_eq!(img.header.colorcorr, Some([2.0, 4.0, 5.0]));
    }

    #[test]
    fn scene_referred_radiance_composes_exposure_and_colorcorr() {
        // Both present: divide by the EXPOSURE product *and* the
        // per-channel COLORCORR triple. stored = (1,1,1) * 3 * (2,4,5).
        let pixels = vec![6.0, 12.0, 15.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let scene = img.scene_referred_radiance_buffer();
        for c in &scene {
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
    }

    #[test]
    fn scene_referred_radiance_luminance_matches_luminance_buffer() {
        // The luminance of the recovered RGB buffer (computed here via the
        // public luminance helper) must equal `scene_referred_luminance_buffer`
        // exactly — the two scene-referred views are derived from the same
        // recovery factors.
        let pixels = vec![6.0, 12.0, 15.0, 1.0, 2.0, 4.0];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let rgb = img.scene_referred_radiance_buffer();
        let lum = img.scene_referred_luminance_buffer();
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let l =
                crate::xyz::luminance_lm_per_sr_per_m2([px[0], px[1], px[2]], img.header.format);
            assert!((l - lum[i]).abs() < 1e-2, "pixel {i}: {l} vs {}", lum[i]);
        }
    }

    #[test]
    fn scene_referred_radiance_treats_degenerate_records_as_identity() {
        // Zero / non-finite factors must not poison the buffer.
        let pixels = vec![1.0, 1.0, 1.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(0.0);
        img.header.colorcorr = Some([f32::NAN, 1.0, 1.0]);
        let scene = img.scene_referred_radiance_buffer();
        for c in &scene {
            assert!(c.is_finite(), "{c}");
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
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
    fn recover_scene_referred_radiance_divides_both_and_clears_slots() {
        // stored = radiance(1,1,1) * EXPOSURE(3) * COLORCORR(2,4,5).
        let pixels = vec![6.0, 12.0, 15.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        img.recover_scene_referred_radiance();
        for c in &img.pixels {
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
        assert!(img.header.exposure.is_none(), "exposure slot not cleared");
        assert!(img.header.colorcorr.is_none(), "colorcorr slot not cleared");
    }

    #[test]
    fn recover_scene_referred_radiance_matches_buffer_view() {
        // The in-place mutator leaves the buffer holding the same values
        // the non-mutating `scene_referred_radiance_buffer` returns.
        let pixels = vec![6.0, 12.0, 15.0, 1.0, 2.0, 4.0];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let expect = img.scene_referred_radiance_buffer();
        img.recover_scene_referred_radiance();
        assert_eq!(img.pixels.len(), expect.len());
        for (a, b) in img.pixels.iter().zip(expect.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn recover_scene_referred_radiance_with_no_records_is_a_noop() {
        let pixels = vec![1.0, 0.5, 0.25];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels.clone());
        img.recover_scene_referred_radiance();
        for (a, b) in pixels.iter().zip(img.pixels.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn recover_scene_referred_radiance_degenerate_factor_clears_without_poisoning() {
        // A zero exposure / NaN colorcorr component is a no-op division
        // but still clears the slot — the buffer must stay finite.
        let pixels = vec![1.0, 1.0, 1.0];
        let mut img = HdrImage::new_rgb96f(1, 1, pixels);
        img.header.exposure = Some(0.0);
        img.header.colorcorr = Some([f32::NAN, 1.0, 1.0]);
        img.recover_scene_referred_radiance();
        for c in &img.pixels {
            assert!(c.is_finite() && (c - 1.0).abs() < 1e-6, "{c}");
        }
        assert!(img.header.exposure.is_none());
        assert!(img.header.colorcorr.is_none());
        // Idempotent: a second call does nothing.
        img.recover_scene_referred_radiance();
        for c in &img.pixels {
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
    }

    #[test]
    fn recover_scene_referred_radiance_inverts_apply_chain() {
        // apply_exposure + apply_colorcorr fold the factors in;
        // recover_scene_referred_radiance is their composed inverse.
        let original = vec![0.6_f32, 0.4, 0.2];
        let mut img = HdrImage::new_rgb96f(1, 1, original.clone());
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.apply_exposure();
        img.apply_colorcorr();
        // Restore the slots so the recover step has factors to undo.
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 8.0]);
        img.recover_scene_referred_radiance();
        for (a, b) in original.iter().zip(img.pixels.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn effective_gamma_defaults_to_one_when_absent() {
        // Staged spec: "when no GAMMA= line is present, the value is taken
        // to be 1.0" — the linear identity.
        let img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        assert!(img.header.gamma.is_none());
        assert!((img.effective_gamma() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn effective_gamma_returns_header_value_and_does_not_perturb_slot() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.0, 0.0, 0.0]);
        img.header.gamma = Some(2.2);
        assert!((img.effective_gamma() - 2.2).abs() < 1e-6);
        // Inspector contract: reading must not clear the slot.
        assert_eq!(img.header.gamma, Some(2.2));
    }

    #[test]
    fn linearize_gamma_applies_power_and_clears_slot() {
        // stored^g per channel; g=2.0 squares each channel.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.25, 0.1]);
        img.header.gamma = Some(2.0);
        img.linearize_gamma();
        let expect = [0.25_f32, 0.0625, 0.01];
        for (a, b) in img.pixels.iter().zip(expect.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
        assert!(img.header.gamma.is_none(), "gamma slot not cleared");
        // Idempotent: a second call is a no-op.
        img.linearize_gamma();
        for (a, b) in img.pixels.clone().iter().zip(expect.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn linearize_gamma_identity_and_degenerate_are_noop_but_clear() {
        // g == 1.0 and any degenerate exponent leave the pixels untouched;
        // the slot is still cleared (matching the recover_* contract) and a
        // negative channel is passed through verbatim rather than NaN'd.
        for g in [1.0_f32, 0.0, -2.0, f32::NAN, f32::INFINITY] {
            let pixels = vec![0.5_f32, -0.25, 0.0];
            let mut img = HdrImage::new_rgb96f(1, 1, pixels.clone());
            img.header.gamma = Some(g);
            img.linearize_gamma();
            for (a, b) in img.pixels.iter().zip(pixels.iter()) {
                assert!((a - b).abs() < 1e-6, "g={g}: {a} vs {b}");
            }
            assert!(img.header.gamma.is_none(), "g={g}: slot not cleared");
        }
    }

    #[test]
    fn linear_radiance_buffer_matches_mutator_and_preserves_slot() {
        let pixels = vec![0.5_f32, 0.25, 0.1, 0.8, 0.4, 0.2];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.gamma = Some(2.4);
        let buf = img.linear_radiance_buffer();
        // Non-mutating: slot and pixels untouched.
        assert_eq!(img.header.gamma, Some(2.4));
        let mut mutated = img.clone();
        mutated.linearize_gamma();
        assert_eq!(buf.len(), mutated.pixels.len());
        for (a, b) in buf.iter().zip(mutated.pixels.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn linear_radiance_buffer_absent_gamma_equals_pixels() {
        let img = HdrImage::new_rgb96f(1, 1, vec![0.6, 0.4, 0.2]);
        assert_eq!(img.linear_radiance_buffer(), img.pixels);
    }

    #[test]
    fn recover_linear_scene_referred_radiance_linearises_then_divides() {
        // stored = (radiance^(1/g)) * EXPOSURE * COLORCORR. With radiance
        // (1,1,1), g=2 ⇒ radiance^(1/2)=1, so stored = EXPOSURE*COLORCORR.
        // Recovery must return (1,1,1) and clear all three slots.
        let mut img = HdrImage::new_rgb96f(1, 1, vec![6.0, 12.0, 15.0]);
        img.header.gamma = Some(2.0);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        // Pre-image: linearise (square) then divide. 6^2=36 /(3*2)=6...
        // Use a cleaner construction: pick stored so stored^2 / (E*CC)=1.
        // stored = sqrt(E*CC): sqrt(6)=2.449.., sqrt(12)=3.464.., sqrt(15)=3.873..
        img.pixels = vec![6.0_f32.sqrt(), 12.0_f32.sqrt(), 15.0_f32.sqrt()];
        img.recover_linear_scene_referred_radiance();
        for c in &img.pixels {
            assert!((c - 1.0).abs() < 1e-5, "{c}");
        }
        assert!(img.header.gamma.is_none());
        assert!(img.header.exposure.is_none());
        assert!(img.header.colorcorr.is_none());
    }

    #[test]
    fn recover_linear_scene_referred_radiance_matches_buffer_view() {
        let pixels = vec![0.7_f32, 0.5, 0.3, 0.9, 0.6, 0.2];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.gamma = Some(2.2);
        img.header.exposure = Some(1.5);
        img.header.colorcorr = Some([1.1, 0.9, 1.05]);
        let expect = img.linear_scene_referred_radiance_buffer();
        img.recover_linear_scene_referred_radiance();
        assert_eq!(img.pixels.len(), expect.len());
        for (a, b) in img.pixels.iter().zip(expect.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn linear_scene_referred_buffer_no_gamma_equals_scene_referred() {
        // Without GAMMA the gamma-aware buffer must equal the plain
        // EXPOSURE/COLORCORR recovery buffer.
        let pixels = vec![6.0_f32, 12.0, 15.0, 1.0, 2.0, 4.0];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.exposure = Some(3.0);
        img.header.colorcorr = Some([2.0, 4.0, 5.0]);
        let plain = img.scene_referred_radiance_buffer();
        let gamma_aware = img.linear_scene_referred_radiance_buffer();
        assert_eq!(plain.len(), gamma_aware.len());
        for (a, b) in plain.iter().zip(gamma_aware.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn linear_scene_referred_luminance_no_gamma_equals_plain() {
        // Without GAMMA the gamma-aware luminance buffer must match the
        // plain scene-referred luminance buffer.
        let pixels = vec![0.6_f32, 0.4, 0.2, 0.9, 0.5, 0.1];
        let mut img = HdrImage::new_rgb96f(2, 1, pixels);
        img.header.exposure = Some(2.0);
        img.header.colorcorr = Some([1.2, 0.8, 1.0]);
        let plain = img.scene_referred_luminance_buffer();
        let gamma_aware = img.linear_scene_referred_luminance_buffer();
        assert_eq!(plain.len(), gamma_aware.len());
        for (a, b) in plain.iter().zip(gamma_aware.iter()) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn linear_scene_referred_luminance_linearises_before_projecting() {
        // A GAMMA-carrying picture: the luminance must be computed from
        // stored^g, not from the raw stored channels. g=2 squares each
        // channel before the 179*(0.265R+0.670G+0.065B) projection.
        let stored = vec![0.5_f32, 0.5, 0.5];
        let mut img = HdrImage::new_rgb96f(1, 1, stored);
        img.header.gamma = Some(2.0);
        let lum = img.linear_scene_referred_luminance_buffer();
        // linear channel = 0.25 each ⇒ 179 * 0.25 * (0.265+0.670+0.065)=179*0.25.
        let expect = 179.0 * 0.25 * (0.265 + 0.670 + 0.065);
        assert!((lum[0] - expect).abs() < 1e-2, "{} vs {expect}", lum[0]);
        // Non-mutating: slot survives.
        assert_eq!(img.header.gamma, Some(2.0));
    }

    #[test]
    fn apply_gamma_encoding_inverts_linearize_gamma() {
        // Round-trip: encode a linear buffer with g then linearise back.
        let original = vec![0.6_f32, 0.4, 0.2, 0.9, 0.1, 0.05];
        let mut img = HdrImage::new_rgb96f(2, 1, original.clone());
        assert!(img.apply_gamma_encoding(2.2));
        assert_eq!(img.header.gamma, Some(2.2));
        // Encoded buffer differs from the linear original.
        assert!(img
            .pixels
            .iter()
            .zip(original.iter())
            .any(|(a, b)| (a - b).abs() > 1e-3));
        img.linearize_gamma();
        for (a, b) in img.pixels.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
        assert!(img.header.gamma.is_none());
    }

    #[test]
    fn apply_gamma_encoding_rejects_degenerate_and_records_unit() {
        let mut img = HdrImage::new_rgb96f(1, 1, vec![0.5, 0.4, 0.3]);
        for g in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            let before = img.pixels.clone();
            assert!(!img.apply_gamma_encoding(g), "g={g} should reject");
            assert_eq!(img.pixels, before, "g={g} must leave pixels untouched");
            assert!(img.header.gamma.is_none(), "g={g} must not record");
        }
        // Exact 1.0 is the identity but still records GAMMA=1.
        let before = img.pixels.clone();
        assert!(img.apply_gamma_encoding(1.0));
        assert_eq!(img.pixels, before);
        assert_eq!(img.header.gamma, Some(1.0));
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

    // -- Geometric reorientation of the displayed buffer (§2 matrix) --

    /// Build a `w × h` picture whose every pixel carries its own `(x, y)`
    /// in the R and G channels (B held at a constant marker). This makes a
    /// geometric permutation directly auditable: after a transform the
    /// pixel at output `(ox, oy)` must carry the source `(x, y)` the
    /// coordinate model predicts.
    fn coord_image(w: u32, h: u32) -> HdrImage {
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push(x as f32);
                pixels.push(y as f32);
                pixels.push(0.5);
            }
        }
        HdrImage::new_rgb96f(w, h, pixels)
    }

    /// Coordinate ground-truth model, mirrored from the header-module test
    /// (kept independent of the buffer permutation under test).
    fn model_dst(op: GeometricOp, x: i64, y: i64, w: i64, h: i64) -> (i64, i64, i64, i64) {
        match op {
            GeometricOp::Identity => (x, y, w, h),
            GeometricOp::FlipHorizontal => (w - 1 - x, y, w, h),
            GeometricOp::FlipVertical => (x, h - 1 - y, w, h),
            GeometricOp::Rotate180 => (w - 1 - x, h - 1 - y, w, h),
            GeometricOp::Rotate90Cw => (h - 1 - y, x, h, w),
            GeometricOp::Rotate90Ccw => (y, w - 1 - x, h, w),
            GeometricOp::Transpose => (y, x, h, w),
            GeometricOp::AntiTranspose => (h - 1 - y, w - 1 - x, h, w),
        }
    }

    fn pixel_at(img: &HdrImage, x: u32, y: u32) -> [f32; 3] {
        let off = ((y * img.width + x) * 3) as usize;
        [img.pixels[off], img.pixels[off + 1], img.pixels[off + 2]]
    }

    #[test]
    fn apply_geometric_matches_coordinate_model_for_every_op() {
        // A non-square, content-asymmetric picture so a stray transpose,
        // mirror, or dimension swap is observable.
        let (w, h) = (3u32, 5u32);
        for op in GeometricOp::ALL {
            let mut img = coord_image(w, h);
            img.apply_geometric(op);
            // Dimensions follow the model.
            let (_, _, mw, mh) = model_dst(op, 0, 0, w as i64, h as i64);
            assert_eq!(
                (img.width as i64, img.height as i64),
                (mw, mh),
                "{op:?}: output dimensions",
            );
            // Every source pixel landed where the model says.
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy, _, _) = model_dst(op, x as i64, y as i64, w as i64, h as i64);
                    let got = pixel_at(&img, dx as u32, dy as u32);
                    assert_eq!(
                        [got[0] as u32, got[1] as u32],
                        [x, y],
                        "{op:?}: source ({x},{y}) expected at ({dx},{dy})",
                    );
                    assert_eq!(got[2], 0.5, "{op:?}: B marker disturbed");
                }
            }
        }
    }

    #[test]
    fn apply_geometric_inverse_restores_buffer_bit_for_bit() {
        let original = coord_image(4, 7);
        for op in GeometricOp::ALL {
            let mut img = original.clone();
            img.apply_geometric(op);
            img.apply_geometric(op.inverse());
            assert_eq!(img.width, original.width, "{op:?}: width restored");
            assert_eq!(img.height, original.height, "{op:?}: height restored");
            assert_eq!(img.pixels, original.pixels, "{op:?}: pixels restored");
        }
    }

    #[test]
    fn apply_geometric_composition_equals_single_then_op() {
        // Applying `a` then `b` must equal applying `a.then(b)` once — the
        // group law realised on the actual pixel buffer.
        let original = coord_image(3, 5);
        for a in GeometricOp::ALL {
            for b in GeometricOp::ALL {
                let mut seq = original.clone();
                seq.apply_geometric(a);
                seq.apply_geometric(b);

                let mut one = original.clone();
                one.apply_geometric(a.then(b));

                assert_eq!(
                    (seq.width, seq.height),
                    (one.width, one.height),
                    "{a:?}.then({b:?}): dims",
                );
                assert_eq!(seq.pixels, one.pixels, "{a:?}.then({b:?}): pixels");
            }
        }
    }

    #[test]
    fn to_orientation_then_normalize_from_is_identity() {
        let original = coord_image(5, 3);
        for o in [
            Orientation::Standard,
            Orientation::FlipX,
            Orientation::Rotate180,
            Orientation::FlipY,
            Orientation::Rotate90Cw,
            Orientation::Rotate90CwFlipY,
            Orientation::Rotate90Ccw,
            Orientation::Rotate90CcwFlipY,
        ] {
            let mut img = original.clone();
            img.to_orientation(o);
            img.normalize_from(o);
            assert_eq!(img.width, original.width, "{o:?}");
            assert_eq!(img.height, original.height, "{o:?}");
            assert_eq!(img.pixels, original.pixels, "{o:?}: round-trip");
        }
    }

    #[test]
    fn reorient_equals_normalize_then_to_orientation() {
        // reorient(from, to) must equal: normalize_from(from) then
        // to_orientation(to) — done across the full 8×8 orientation matrix.
        let all = [
            Orientation::Standard,
            Orientation::FlipX,
            Orientation::Rotate180,
            Orientation::FlipY,
            Orientation::Rotate90Cw,
            Orientation::Rotate90CwFlipY,
            Orientation::Rotate90Ccw,
            Orientation::Rotate90CcwFlipY,
        ];
        let original = coord_image(4, 6);
        for from in all {
            for to in all {
                let mut a = original.clone();
                a.reorient(from, to);

                let mut b = original.clone();
                b.normalize_from(from);
                b.to_orientation(to);

                assert_eq!((a.width, a.height), (b.width, b.height), "{from:?}->{to:?}");
                assert_eq!(a.pixels, b.pixels, "{from:?}->{to:?}");
            }
        }
    }

    #[test]
    fn reorient_same_orientation_is_identity() {
        let all = [
            Orientation::Standard,
            Orientation::FlipX,
            Orientation::Rotate180,
            Orientation::FlipY,
            Orientation::Rotate90Cw,
            Orientation::Rotate90CwFlipY,
            Orientation::Rotate90Ccw,
            Orientation::Rotate90CcwFlipY,
        ];
        let original = coord_image(3, 7);
        for o in all {
            let mut img = original.clone();
            img.reorient(o, o);
            assert_eq!(img.pixels, original.pixels, "{o:?}: reorient(o,o)");
            assert_eq!((img.width, img.height), (original.width, original.height));
        }
    }

    #[test]
    fn apply_geometric_on_zero_dimension_swaps_extents_only() {
        // A degenerate 0×4 picture: a dimension-swapping op must still
        // report swapped extents, and nothing panics.
        let mut img = HdrImage::new_rgb96f(0, 4, Vec::new());
        img.apply_geometric(GeometricOp::Rotate90Cw);
        assert_eq!((img.width, img.height), (4, 0));
        assert!(img.pixels.is_empty());
        // Aspect-preserving op leaves the (still empty) shape alone.
        img.apply_geometric(GeometricOp::FlipVertical);
        assert_eq!((img.width, img.height), (4, 0));
    }

    #[test]
    fn rotate_90_cw_is_visually_a_quarter_turn() {
        // Concrete 2×1 sanity check independent of the model helper: a row
        // [A, B] rotated 90° CW becomes a column with A on top, B below.
        let mut img = HdrImage::new_rgb96f(2, 1, vec![1.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        img.apply_geometric(GeometricOp::Rotate90Cw);
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(pixel_at(&img, 0, 0)[0], 1.0, "A on top");
        assert_eq!(pixel_at(&img, 0, 1)[0], 2.0, "B below");
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

    /// Construct a top-down RGB f32 image directly from a slice of
    /// on-disk RGBE quads, decoding each `[R, G, B, E]` byte tuple through
    /// the shared-exponent decode formula
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §3) into the float
    /// pixel buffer.
    ///
    /// `quads` is `width * height` long, in canonical top-down,
    /// left-to-right pixel order (the same order
    /// [`Self::to_rgbe_quads`] returns). Each quad is decoded with
    /// [`crate::rgbe::rgbe_to_rgb`], so an all-zero / zero-exponent quad
    /// becomes the black pixel `[0.0, 0.0, 0.0]` per the spec's "exponent
    /// byte 0 ⇒ no value" sentinel rule.
    ///
    /// This is the byte-faithful inverse of [`Self::to_rgbe_quads`]:
    /// because the shared-exponent codec is idempotent on normalised
    /// quads (the encoder always emits a dominant mantissa in
    /// `[128, 256)` and a decoded magnitude above the `1e-32` black
    /// floor — see [`crate::rgbe::rgb_to_rgbe`]), a picture built from
    /// such quads re-encodes to *exactly* the same quads. Callers that
    /// need a bit-exact RGBE round-trip (rather than the inherently lossy
    /// float-in / float-out path) build with this constructor and verify
    /// with [`Self::to_rgbe_quads`].
    ///
    /// The header is taken verbatim; the caller is responsible for any
    /// `FORMAT` / orientation flags it wants on the re-encode.
    ///
    /// # Panics
    ///
    /// Debug-asserts that `quads.len() == width * height`.
    pub fn from_rgbe_quads(width: u32, height: u32, quads: &[[u8; 4]], header: HdrHeader) -> Self {
        debug_assert_eq!(quads.len(), (width as usize) * (height as usize));
        let mut pixels = Vec::with_capacity(quads.len() * 3);
        for &q in quads {
            let rgb = crate::rgbe::rgbe_to_rgb(q);
            pixels.push(rgb[0]);
            pixels.push(rgb[1]);
            pixels.push(rgb[2]);
        }
        Self {
            width,
            height,
            pixel_format: HdrPixelFormat::Rgb96f,
            pixels,
            header,
        }
    }

    /// Re-derive the on-disk RGBE quad for every pixel, in canonical
    /// top-down, left-to-right order — the byte-level view of the picture
    /// the encoder will commit to the wire (modulo the chosen scanline
    /// RLE flavour, which is a lossless re-packing of these exact quads).
    ///
    /// Each float triple is run through [`crate::rgbe::rgb_to_rgbe`], the
    /// same shared-exponent quantiser [`crate::encode_hdr`] applies per
    /// pixel, so the returned `Vec<[u8; 4]>` is bit-identical to the quad
    /// stream a fresh encode would produce. Pairs with
    /// [`Self::from_rgbe_quads`] to give callers a bit-exact RGBE
    /// round-trip surface: `from_rgbe_quads(.., q, ..).to_rgbe_quads()`
    /// returns `q` unchanged for every normalised quad (the property the
    /// `tests/rgbe_roundtrip_matrix.rs` matrix pins across resolution /
    /// orientation / RLE-flavour variants).
    ///
    /// Returns `width * height` quads. A black pixel maps to the all-zero
    /// sentinel `[0, 0, 0, 0]`.
    pub fn to_rgbe_quads(&self) -> Vec<[u8; 4]> {
        let n = (self.width as usize) * (self.height as usize);
        let mut out = Vec::with_capacity(n);
        for px in self.pixels.chunks_exact(3) {
            out.push(crate::rgbe::rgb_to_rgbe([px[0], px[1], px[2]]));
        }
        out
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
    /// the picture's float channels per the staged spec's §"Physical
    /// interpretation" (`docs/image/hdr/radiance-hdr-rgbe-format.md`):
    ///
    /// ```text
    /// FORMAT=32-bit_rle_rgbe -> 179 * (0.265*R + 0.670*G + 0.065*B)
    /// FORMAT=32-bit_rle_xyze -> Y
    /// ```
    ///
    /// RGBE primaries carry spectral radiance in watts/sr/m², so the
    /// `WHTEFFICACY = 179` lm/W factor performs the radiometric →
    /// photometric conversion; for XYZE the spec is explicit that "the Y
    /// primary is already lumens/steradian/m², so the 179× luminance
    /// conversion is unnecessary" — the Y channel is returned verbatim.
    /// `header.format` selects which branch is applied so the
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

    /// The `GAMMA=` transfer exponent the picture declared, with the
    /// staged-spec default of `1.0` (linear pixels, no correction)
    /// substituted when no record is present.
    ///
    /// Per the staged spec's "The `GAMMA=` header variable" section
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`), `GAMMA=g` records
    /// that the stored channels "have already been gamma-corrected with
    /// exponent `g`" — a display-oriented, non-linear quantity rather than
    /// linear radiance — and "when no `GAMMA=` line is present, the value
    /// is taken to be `1.0`, meaning no gamma correction has been applied
    /// and the stored pixels are already linear". `GAMMA=` is a de-facto
    /// extension outside the canonical seven header variables, so pictures
    /// written by native Radiance tools omit it and this helper returns the
    /// linear identity for them. Consumers that need to distinguish "file
    /// declared `GAMMA=1` explicitly" from "no record was present" should
    /// match on [`HdrHeader::gamma`] directly.
    ///
    /// Mirrors [`Self::effective_exposure`] / [`Self::effective_pixaspect`]
    /// / [`Self::effective_primaries`] in shape.
    pub fn effective_gamma(&self) -> f32 {
        self.header.gamma.unwrap_or(1.0)
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

    /// Multiply every float channel by `factor` **and record the
    /// multiplication in the `EXPOSURE=` slot**, keeping the picture's
    /// scene-referred radiance unchanged.
    ///
    /// This is the writer-side counterpart to the §1 recovery rule in
    /// the staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`):
    /// `EXPOSURE=` is a "single float multiplier already applied to all
    /// pixels", cumulative, and "to recover original radiances
    /// (watts/sr/m²) divide file values by the product of all `EXPOSURE`
    /// settings". Brightening or dimming a picture while *keeping it
    /// physically meaningful* therefore requires two writes — scale the
    /// stored channels, and fold the same factor into the header slot so
    /// the recovery division still lands on the original radiance. This
    /// helper performs both atomically: `pixels *= factor` and
    /// `header.exposure = Some(effective_exposure() * factor)` (a `None`
    /// slot seeds from the spec default `1.0`, becoming
    /// `Some(factor)`). [`Self::scene_referred_radiance_buffer`] and the
    /// `recover_*` helpers are invariant across a call.
    ///
    /// Contrast with the neighbouring exposure helpers, which move in
    /// other directions: [`Self::apply_exposure`] multiplies the
    /// *already-recorded* factor into the pixels (clearing the slot),
    /// and [`Self::recover_original_radiance`] divides it out. Neither
    /// changes the displayed brightness *and* keeps the record
    /// consistent the way this one does.
    ///
    /// Returns `true` when the adjustment was applied. A degenerate
    /// `factor` (`0.0`, negative, or non-finite) is rejected as `false`
    /// with the picture untouched — a zero or negative multiplier would
    /// make the recovery division degenerate / flip radiance signs, and
    /// the permissive `recover_*` handling would then silently discard
    /// it. An exact `1.0` factor is a full no-op that still returns
    /// `true` (the slot is left as-is rather than materialising an
    /// explicit `EXPOSURE=1` record).
    pub fn adjust_exposure_factor(&mut self, factor: f32) -> bool {
        if !(factor.is_finite() && factor > 0.0) {
            return false;
        }
        if factor == 1.0 {
            return true;
        }
        for v in &mut self.pixels {
            *v *= factor;
        }
        self.header.exposure = Some(self.effective_exposure() * factor);
        true
    }

    /// Adjust the picture's exposure by a whole number of photographic
    /// *stops* — the `2^stops` power-of-two form of
    /// [`Self::adjust_exposure_factor`].
    ///
    /// The staged format note's skeletal converter documents exposure
    /// adjustment in exactly this shape (a `-e +/-stops` integer-stop
    /// brightness option applied to the decoded scanlines), and the
    /// power-of-two factor has a pleasant numerical property: an `f32`
    /// multiplication by `2^n` is *exact* (it only moves the exponent
    /// field), so `adjust_exposure_stops(n)` followed by
    /// `adjust_exposure_stops(-n)` restores every sample bit-for-bit
    /// (absent overflow to `∞` / underflow to subnormal-zero at the
    /// extremes of the `f32` range).
    ///
    /// Returns `true` when applied; `false` (picture untouched) when
    /// `2^stops` is not a finite positive `f32` (|stops| beyond ~±126).
    /// `stops = 0` is a no-op that returns `true`.
    pub fn adjust_exposure_stops(&mut self, stops: i32) -> bool {
        self.adjust_exposure_factor(2.0_f32.powi(stops))
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

    /// Reconstruct the picture's scene-referred radiance **in place** by
    /// dividing out *both* the cumulative `EXPOSURE=` multiplier and the
    /// `COLORCORR=` triple the writer baked into the stored channels, and
    /// clear both header slots.
    ///
    /// This is the one-shot composition of
    /// [`Self::recover_original_radiance`] and
    /// [`Self::recover_original_colorcorr`] — the full §1 recovery the
    /// staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md`)
    /// describes ("to recover original radiances (watts/sr/m²) divide
    /// file values by the product of all EXPOSURE settings", with the
    /// complementary per-primary `COLORCORR` division). After this call
    /// the buffer holds scene-referred radiance and the typed
    /// [`HdrHeader::exposure`] / [`HdrHeader::colorcorr`] slots are
    /// cleared, so a subsequent re-encode does not re-bake the factors.
    ///
    /// It is the mutating counterpart to the non-mutating
    /// [`Self::scene_referred_radiance_buffer`]: the two leave the buffer
    /// holding the same recovered values (the radiance buffer just returns
    /// a fresh copy and preserves the slots). The two component mutators'
    /// edge-case handling carries over verbatim — a `None` slot, the
    /// trivial `1.0` exposure / `[1, 1, 1]` triple, and any `0.0` or
    /// non-finite factor are no-op divisions (the slot is still cleared),
    /// so a malformed header can never write NaN / ∞ into the buffer. A
    /// second call is a no-op (both slots are `None` after the first).
    pub fn recover_scene_referred_radiance(&mut self) {
        self.recover_original_radiance();
        self.recover_original_colorcorr();
    }

    /// Linearise the stored float channels through the header's `GAMMA=`
    /// transfer exponent **in place** and clear the header slot.
    ///
    /// Per the staged spec's "The `GAMMA=` header variable" section
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`), a picture that
    /// carries `GAMMA=g` stores channels that "have already been
    /// gamma-corrected with exponent `g`" — the mantissas are a
    /// gamma-encoded, display-oriented quantity, not linear radiance — and
    /// "a reader that honours `GAMMA=g` must apply the inverse of the
    /// recorded encoding to each channel to obtain a linear value", namely
    /// `linear_channel = stored_channel ^ g` per channel, so that `g = 1.0`
    /// (or an absent header) is the identity.
    ///
    /// This raises every channel of [`Self::pixels`] to the power `g` and
    /// clears [`HdrHeader::gamma`], so the buffer is now linear and a
    /// re-encode neither re-declares nor double-applies the correction. It
    /// is the gamma counterpart to [`Self::apply_exposure`] /
    /// [`Self::apply_colorcorr`] (a stored-into-pixels operation that
    /// clears the slot). A `None` slot, an exact `1.0`, and any degenerate
    /// exponent (`0.0`, negative, or non-finite) are treated as the
    /// identity no-op — the same permissive handling the `recover_*` /
    /// `apply_*` helpers use — so a malformed `GAMMA=` can never turn the
    /// buffer into NaN / ∞. A `0.0` channel maps to `0.0`; a negative
    /// channel (out of gamut, never produced by an RGBE decode) is passed
    /// through verbatim rather than raised to a fractional power, which
    /// would be NaN. A second call is a no-op (the slot is `None`).
    pub fn linearize_gamma(&mut self) {
        if let Some(g) = self.header.gamma.take() {
            if g > 0.0 && g.is_finite() && (g - 1.0).abs() > f32::EPSILON {
                for v in &mut self.pixels {
                    if *v > 0.0 {
                        *v = v.powf(g);
                    }
                }
            }
        }
    }

    /// Allocate a fresh `width * height * 3` float buffer of the picture's
    /// **linearised** channels — the stored channels put through the
    /// `GAMMA=` transfer exponent (`stored ^ g`) — without mutating the
    /// image.
    ///
    /// This is the non-mutating counterpart to [`Self::linearize_gamma`]:
    /// where that method rewrites [`Self::pixels`] in place and clears the
    /// slot, this returns a fresh copy in the same packed top-down layout
    /// and leaves the picture's pixels and [`HdrHeader::gamma`] slot
    /// untouched, so the record survives a re-encode and the buffer can be
    /// requested repeatedly. It applies the staged spec's
    /// `linear_channel = stored_channel ^ g` linearisation rule (see
    /// [`Self::linearize_gamma`]); when no `GAMMA=` record is present, or
    /// the exponent is exactly `1.0` or degenerate (`0.0` / negative /
    /// non-finite), the returned buffer equals [`Self::pixels`] exactly.
    /// Negative channels are passed through verbatim (a fractional power of
    /// a negative base is NaN) and `0.0` maps to `0.0`.
    pub fn linear_radiance_buffer(&self) -> Vec<f32> {
        let g = self.effective_gamma();
        let apply = g > 0.0 && g.is_finite() && (g - 1.0).abs() > f32::EPSILON;
        let mut out = Vec::with_capacity(self.pixels.len());
        for &v in &self.pixels {
            out.push(if apply && v > 0.0 { v.powf(g) } else { v });
        }
        out
    }

    /// Reconstruct the picture's **linear scene-referred radiance** in
    /// place: first linearise the stored channels through `GAMMA=`
    /// (`stored ^ g`), then divide out the cumulative `EXPOSURE=`
    /// multiplier and the `COLORCORR=` triple, clearing all three header
    /// slots.
    ///
    /// This is the fully-specified decode the staged spec's "The `GAMMA=`
    /// header variable" section describes
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`): "A fully-specified
    /// decode therefore linearises first (`stored^g`), then divides out
    /// `COLORCORR` and `EXPOSURE`" — because once `GAMMA` has been applied
    /// the stored numbers are no longer linear, so the physical-radiance
    /// recovery for `EXPOSURE` / `COLORCORR` (both *linear* scale factors)
    /// is only meaningful after linearisation. It is the one-shot
    /// composition of [`Self::linearize_gamma`] followed by
    /// [`Self::recover_scene_referred_radiance`], in that spec-mandated
    /// order.
    ///
    /// After this call the buffer holds linear scene-referred radiance and
    /// the [`HdrHeader::gamma`] / [`HdrHeader::exposure`] /
    /// [`HdrHeader::colorcorr`] slots are cleared, so a re-encode does not
    /// re-bake or re-declare any of them. In native Radiance pictures
    /// `GAMMA` is absent and the data are already linear, so this reduces
    /// to [`Self::recover_scene_referred_radiance`]. All three components'
    /// permissive edge-case handling (identity / `None` / degenerate → no
    /// op, slot still cleared) carries over, and a second call is a no-op.
    /// It is the mutating counterpart to
    /// [`Self::linear_scene_referred_radiance_buffer`].
    pub fn recover_linear_scene_referred_radiance(&mut self) {
        self.linearize_gamma();
        self.recover_scene_referred_radiance();
    }

    /// Allocate a fresh `width * height * 3` float buffer of the picture's
    /// **linear scene-referred radiance** — the stored channels
    /// linearised through `GAMMA=` (`stored ^ g`) and then divided by the
    /// cumulative `EXPOSURE=` multiplier and `COLORCORR=` triple — without
    /// mutating the image.
    ///
    /// This is the non-mutating counterpart to
    /// [`Self::recover_linear_scene_referred_radiance`] and the
    /// gamma-aware extension of [`Self::scene_referred_radiance_buffer`]:
    /// it applies the staged spec's fully-specified decode order
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`, "linearises first
    /// (`stored^g`), then divides out `COLORCORR` and `EXPOSURE`") in one
    /// allocation, leaving [`Self::pixels`] and every typed header slot
    /// untouched so the records survive a re-encode. When no `GAMMA=`
    /// record is present (or the exponent is the `1.0` identity) it agrees
    /// exactly with [`Self::scene_referred_radiance_buffer`]; when none of
    /// the three records is present it equals [`Self::pixels`]. The same
    /// permissive degenerate-factor handling the component operations use
    /// carries over, so a malformed header can never turn the buffer into
    /// NaN / ∞ (negative channels are passed through the linearisation
    /// verbatim).
    pub fn linear_scene_referred_radiance_buffer(&self) -> Vec<f32> {
        let g = self.effective_gamma();
        let apply_g = g > 0.0 && g.is_finite() && (g - 1.0).abs() > f32::EPSILON;
        let (inv_exposure, inv_cc) = self.scene_referred_recovery_factors();
        let lin = |v: f32| if apply_g && v > 0.0 { v.powf(g) } else { v };
        let mut out = Vec::with_capacity(self.pixels.len());
        for px in self.pixels.chunks_exact(3) {
            out.push(lin(px[0]) * inv_exposure * inv_cc[0]);
            out.push(lin(px[1]) * inv_exposure * inv_cc[1]);
            out.push(lin(px[2]) * inv_exposure * inv_cc[2]);
        }
        out
    }

    /// Allocate a fresh `width * height` buffer of per-pixel *physical*
    /// photometric luminance (lm/sr/m²) computed from the picture's
    /// **linear scene-referred** radiance — the stored channels
    /// linearised through `GAMMA=` (`stored ^ g`) and then divided by the
    /// cumulative `EXPOSURE=` multiplier and `COLORCORR=` triple — before
    /// the §"Physical interpretation" luminance formula is applied.
    ///
    /// This is the gamma-aware extension of
    /// [`Self::scene_referred_luminance_buffer`]: where that method
    /// assumes the stored channels are already linear (correct for native
    /// Radiance pictures, which never carry `GAMMA=`), this one first
    /// applies the staged spec's `linear_channel = stored_channel ^ g`
    /// linearisation, honouring the "The `GAMMA=` header variable" section's
    /// rule that the physical-radiance recovery is "only meaningful after
    /// the pixels have been linearised with `GAMMA`"
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`). The luminance itself
    /// is the §"Physical interpretation" projection — `179 * (0.265 R +
    /// 0.670 G + 0.065 B)` for `FORMAT=32-bit_rle_rgbe`, the recovered `Y`
    /// verbatim for `FORMAT=32-bit_rle_xyze`.
    ///
    /// Non-mutating: the image's pixels and every typed header slot are
    /// left untouched. When no `GAMMA=` record is present (or the exponent
    /// is the `1.0` identity) it agrees exactly with
    /// [`Self::scene_referred_luminance_buffer`]. The same permissive
    /// degenerate-factor handling carries over, so a malformed header can
    /// never turn the luminance buffer into NaN / ∞; negative channels are
    /// passed through the linearisation verbatim.
    pub fn linear_scene_referred_luminance_buffer(&self) -> Vec<f32> {
        let g = self.effective_gamma();
        let apply_g = g > 0.0 && g.is_finite() && (g - 1.0).abs() > f32::EPSILON;
        let (inv_exposure, inv_cc) = self.scene_referred_recovery_factors();
        let lin = |v: f32| if apply_g && v > 0.0 { v.powf(g) } else { v };
        let n = (self.width as usize) * (self.height as usize);
        let mut out = Vec::with_capacity(n);
        for px in self.pixels.chunks_exact(3) {
            let recovered = [
                lin(px[0]) * inv_exposure * inv_cc[0],
                lin(px[1]) * inv_exposure * inv_cc[1],
                lin(px[2]) * inv_exposure * inv_cc[2],
            ];
            out.push(crate::xyz::luminance_lm_per_sr_per_m2(
                recovered,
                self.header.format,
            ));
        }
        out
    }

    /// Gamma-encode the stored (linear) float channels with transfer
    /// exponent `gamma` **in place** and record `GAMMA=gamma` in the
    /// header — the writer-side inverse of [`Self::linearize_gamma`].
    ///
    /// Per the staged spec's "The `GAMMA=` header variable" section
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`), a file that carries
    /// `GAMMA=g` stores channels encoded as `stored = linear ^ (1/g)` (so
    /// that the honouring reader recovers `linear = stored ^ g`). This
    /// helper assumes the current buffer holds linear channels, raises each
    /// to the power `1/gamma`, and sets [`HdrHeader::gamma`] to `gamma` so
    /// the encoding is recorded for the reader to undo. It is the gamma
    /// analogue of [`Self::adjust_exposure_factor`] — a paired
    /// pixel-and-header write that keeps the file self-consistent.
    ///
    /// The staged spec notes `GAMMA=` is a de-facto extension outside the
    /// canonical specification, that "canonical Radiance readers ignore it
    /// and assume linear pixels", and that "writers targeting maximum
    /// interoperability with native Radiance should emit linear pixels and
    /// omit `GAMMA` (equivalently `GAMMA=1`)" — so prefer leaving pixels
    /// linear unless you specifically need a gamma-encoded file.
    ///
    /// Returns `true` when the encoding was applied. A degenerate `gamma`
    /// (`0.0`, negative, or non-finite) is rejected as `false` with the
    /// picture untouched. An exact `1.0` is the identity: pixels are left
    /// unchanged, but `GAMMA=1.0` is still recorded (an explicit
    /// linear-marker), and `true` is returned. The round-trip
    /// `apply_gamma_encoding(g)` then [`Self::linearize_gamma`] restores
    /// the linear channels to within `f32` power-function precision.
    /// Negative channels are passed through verbatim (a fractional power of
    /// a negative base is NaN); `0.0` maps to `0.0`.
    pub fn apply_gamma_encoding(&mut self, gamma: f32) -> bool {
        if !(gamma.is_finite() && gamma > 0.0) {
            return false;
        }
        if (gamma - 1.0).abs() > f32::EPSILON {
            let inv = 1.0 / gamma;
            for v in &mut self.pixels {
                if *v > 0.0 {
                    *v = v.powf(inv);
                }
            }
        }
        self.header.gamma = Some(gamma);
        true
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
    ///    0.065 B)` for `FORMAT=32-bit_rle_rgbe`, and the stored `Y`
    ///    verbatim for `FORMAT=32-bit_rle_xyze` (the spec's "the Y
    ///    primary is already lumens/steradian/m², so the 179× luminance
    ///    conversion is unnecessary").
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
        let (inv_exposure, inv_cc) = self.scene_referred_recovery_factors();
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

    /// The per-channel reciprocal factors that undo the cumulative
    /// `EXPOSURE=` multiplier and `COLORCORR=` triple the writer baked
    /// into the stored channels — the scalar reciprocal of the exposure
    /// product, and the per-channel reciprocal of the colour-correction
    /// triple.
    ///
    /// Shared by [`Self::scene_referred_luminance_buffer`],
    /// [`Self::scene_referred_radiance_buffer`], and
    /// [`Self::recover_scene_referred_radiance`] so the three expose
    /// numerically identical recovery semantics. A degenerate cumulative
    /// factor (`EXPOSURE` that is `0.0` or non-finite, or any `COLORCORR`
    /// component that is `0.0` or non-finite) maps to the `1.0` identity
    /// for that factor — the same permissive handling
    /// [`Self::recover_original_radiance`] /
    /// [`Self::recover_original_colorcorr`] use, so a malformed header can
    /// never inject NaN / ∞ into the recovered buffer.
    fn scene_referred_recovery_factors(&self) -> (f32, [f32; 3]) {
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
        (inv_exposure, inv_cc)
    }

    /// Allocate a fresh `width * height * 3` float buffer of the
    /// picture's **scene-referred** radiance — the stored channels with
    /// the cumulative `EXPOSURE=` multiplier and `COLORCORR=` triple the
    /// writer baked in divided back out — without mutating the image.
    ///
    /// This is the RGB-buffer counterpart to
    /// [`Self::scene_referred_luminance_buffer`]: where that method
    /// projects the recovered radiance through the §"Physical
    /// interpretation" luminance formula to a single scalar per pixel,
    /// this returns the recovered three-channel radiance itself, in the
    /// same packed `[R, G, B, R, G, B, …]` (or `[X, Y, Z, …]` for XYZE)
    /// top-down layout as [`Self::pixels`]. It composes the §1 recovery
    /// rules — "to recover original radiances (watts/sr/m²) divide file
    /// values by the product of all EXPOSURE settings", with the
    /// complementary per-primary `COLORCORR` division — into one
    /// allocation, so a consumer that needs scene-referred RGB for
    /// downstream radiometric work (a custom luminance weighting, a
    /// physically-based relight, a different colour-space projection)
    /// no longer has to mutate the image and clobber its header slots
    /// via two separate [`Self::recover_original_radiance`] /
    /// [`Self::recover_original_colorcorr`] calls.
    ///
    /// Non-mutating: the image's pixels and header are left untouched, so
    /// unlike the in-place `recover_*` mutators this can be called
    /// repeatedly and the typed `EXPOSURE=` / `COLORCORR=` slots survive
    /// for a re-encode. When neither record is present (or both are the
    /// identity) the returned buffer equals [`Self::pixels`] exactly. A
    /// degenerate cumulative factor (`EXPOSURE` that is `0.0` or
    /// non-finite, or any `COLORCORR` component that is `0.0` or
    /// non-finite) is treated as "no recovery applied" for that factor —
    /// the same permissive handling the `recover_*` helpers use — so a
    /// malformed header can never turn the buffer into NaN / ∞.
    pub fn scene_referred_radiance_buffer(&self) -> Vec<f32> {
        let (inv_exposure, inv_cc) = self.scene_referred_recovery_factors();
        let mut out = Vec::with_capacity(self.pixels.len());
        for px in self.pixels.chunks_exact(3) {
            out.push(px[0] * inv_exposure * inv_cc[0]);
            out.push(px[1] * inv_exposure * inv_cc[1]);
            out.push(px[2] * inv_exposure * inv_cc[2]);
        }
        out
    }

    /// Apply one of the eight rigid picture symmetries
    /// ([`GeometricOp`], the §2 resolution-string orientation matrix of
    /// `docs/image/hdr/radiance-hdr-rgbe-format.md`) to the decoded float
    /// buffer **in place**, rewriting the picture content rather than the
    /// on-disk axis flags.
    ///
    /// The decoded buffer is always in canonical standard display order
    /// (top-down, left-to-right). This rotates / mirrors the actual image:
    /// e.g. [`GeometricOp::Rotate90Cw`] turns a `w × h` picture into the
    /// `h × w` picture you would see after turning it a quarter-turn
    /// clockwise. The four 90°-class ops swap [`Self::width`] and
    /// [`Self::height`] (see [`GeometricOp::swaps_dimensions`]); the header
    /// metadata and `pixel_format` are left untouched, so the result
    /// re-encodes with whatever orientation flags the header carries.
    ///
    /// Every operation is a pure pixel permutation — no radiance value is
    /// altered, so this is lossless and composes exactly: applying `op`
    /// then `op.inverse()` restores the original buffer bit-for-bit, and
    /// `a` then `b` equals the single op `a.then(b)`.
    pub fn apply_geometric(&mut self, op: GeometricOp) {
        let w = self.width as usize;
        let h = self.height as usize;
        if w == 0 || h == 0 {
            // Degenerate picture: nothing to permute, but a dimension-
            // swapping op must still swap the (zero) extents so callers
            // see a consistent shape.
            if op.swaps_dimensions() {
                core::mem::swap(&mut self.width, &mut self.height);
            }
            return;
        }
        let (new_pixels, swap) = match op {
            GeometricOp::Identity => return,
            GeometricOp::FlipHorizontal => (buf_flip_horizontal(&self.pixels, w, h), false),
            GeometricOp::FlipVertical => (buf_flip_vertical(&self.pixels, w, h), false),
            GeometricOp::Rotate180 => (buf_rotate_180(&self.pixels, w, h), false),
            GeometricOp::Rotate90Cw => (buf_rotate_90_cw(&self.pixels, w, h), true),
            GeometricOp::Rotate90Ccw => (buf_rotate_90_ccw(&self.pixels, w, h), true),
            GeometricOp::Transpose => (buf_transpose(&self.pixels, w, h), true),
            // Anti-diagonal reflection = transpose, then 180° rotation of
            // the (now h×w) result. Both passes are pure permutations.
            GeometricOp::AntiTranspose => {
                let t = buf_transpose(&self.pixels, w, h);
                (buf_rotate_180(&t, h, w), true)
            }
        };
        self.pixels = new_pixels;
        if swap {
            core::mem::swap(&mut self.width, &mut self.height);
        }
    }

    /// Reinterpret the decoded standard-display buffer as the picture
    /// described by `target` and rewrite the pixels to match — i.e. apply
    /// `target.display_transform()`.
    ///
    /// Use this to *render* a decoded picture the way a given orientation
    /// would display it. The inverse is [`Self::normalize_from`].
    pub fn to_orientation(&mut self, target: Orientation) {
        self.apply_geometric(target.display_transform());
    }

    /// Undo `source`'s display transform, mapping a buffer that is laid out
    /// as `source` describes back to canonical standard display order —
    /// i.e. apply `source.display_transform().inverse()`.
    ///
    /// This is the exact inverse of [`Self::to_orientation`]:
    /// `img.to_orientation(o); img.normalize_from(o)` restores `img`.
    pub fn normalize_from(&mut self, source: Orientation) {
        self.apply_geometric(source.display_transform().inverse());
    }

    /// Move the picture content from the `from` orientation to the `to`
    /// orientation in a single pass: undo `from`'s display transform (back
    /// to standard), then apply `to`'s. Because the eight transforms form
    /// the dihedral group `D₄`, the round trip collapses to one
    /// [`GeometricOp`] — `from.display_transform().inverse().then(
    /// to.display_transform())` — applied once, so no intermediate buffer
    /// is built.
    ///
    /// `reorient(o, o)` is the identity for every `o`.
    pub fn reorient(&mut self, from: Orientation, to: Orientation) {
        let op = from
            .display_transform()
            .inverse()
            .then(to.display_transform());
        self.apply_geometric(op);
    }
}
