#![no_main]

//! Drive the float-domain colour pipeline — XYZE↔RGB conversion,
//! primaries-matrix derivation, photometric luminance, and the eight
//! tone-mapping operators — on hostile float input.
//!
//! The other four targets all enter through the *byte* surface
//! (`parse_hdr` / `encode_hdr`), which means every float that reaches
//! the conversion and tone-mapping code has already been laundered
//! through the RGBE shared-exponent quantiser: it is finite,
//! non-negative, and bounded by `mantissa * 2^(exp-136)`. That makes
//! the genuinely numeric surface — the `3×3` matrix inversion in
//! [`rgb_to_xyz_matrix_from_primaries`], the per-operator transcendental
//! evaluations (`powf`, `ln`, the Drago `log` base, the Reinhard
//! divisions), and the `apply_matrix` accumulation — effectively
//! unreachable with NaN / ±inf / negative / denormal samples through
//! those targets.
//!
//! This target closes that gap. It synthesises an `HdrImage` whose
//! pixel buffer is taken *verbatim* from raw fuzz bytes reinterpreted
//! as `f32` (so NaN, ±inf, negatives, subnormals, and ±0 all occur),
//! plus a fuzz-controlled [`Primaries`] record whose eight chromaticity
//! floats are likewise unconstrained (degenerate / collinear / zero-`y`
//! primaries exercise the singular-matrix `None` branch of the matrix
//! derivation). It then runs, asserting only that each call *returns*
//! without panicking, integer-overflowing, or producing a buffer of the
//! wrong length:
//!
//!  * both named-space whole-image conversions (`Srgb`, `Radiance`),
//!  * the arbitrary-primaries conversions and their `_with_effective_`
//!    wrappers (the `bool` return distinguishes the degenerate branch),
//!  * the single-pixel `xyz_to_rgb` / `rgb_to_xyz` and their inverse,
//!  * `rgb_to_xyz_matrix_from_primaries` / `xyz_to_rgb_matrix_from_primaries`
//!    directly (the `Option` return is the degeneracy oracle),
//!  * `luminance_lm_per_sr_per_m2` under both `HdrFormat` branches,
//!  * the six photometric (`WHTEFFICACY`-folded) file-faithful
//!    converters added in round 383, over the same hostile buffer and
//!    the same unconstrained `Primaries` record,
//!  * `HdrImage::adjust_exposure_factor` / `adjust_exposure_stops` with
//!    verbatim fuzz factors (NaN / ±inf / negative / zero must be
//!    rejected without touching the buffer; a recorded `EXPOSURE=`
//!    stays finite-positive) and full-`i8`-range stop counts,
//!  * `rgbe_shift_exponent` with i32-extreme stop counts on
//!    fuzz-shaped quads (must shift in range or report `None`, never
//!    panic / overflow / land a non-sentinel quad on exponent byte 0),
//!  * `tone_map` with all eight `ToneMap` operators, each with
//!    fuzz-controlled exposure / white-point / bias / scene-max params,
//!    asserting the returned `Rgb24` buffer is exactly `width*height*3`
//!    bytes and every byte is a real `u8` (the quantiser must clamp
//!    NaN / inf to the `0..=255` range rather than wrap or panic).
//!
//! As with the other targets, the crate is pulled in with
//! `default-features = false`, so the fuzz build never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::{
    convert_image_rgb_to_xyz, convert_image_rgb_to_xyz_photometric,
    convert_image_rgb_to_xyz_photometric_with_effective_primaries,
    convert_image_rgb_to_xyz_photometric_with_primaries,
    convert_image_rgb_to_xyz_with_effective_primaries, convert_image_rgb_to_xyz_with_primaries,
    convert_image_xyz_to_rgb, convert_image_xyz_to_rgb_photometric,
    convert_image_xyz_to_rgb_photometric_with_effective_primaries,
    convert_image_xyz_to_rgb_photometric_with_primaries,
    convert_image_xyz_to_rgb_with_effective_primaries, convert_image_xyz_to_rgb_with_primaries,
    luminance_lm_per_sr_per_m2, rgb_to_xyz, rgb_to_xyz_matrix_from_primaries, rgbe_shift_exponent,
    tone_map, xyz_to_rgb, xyz_to_rgb_matrix_from_primaries, HdrFormat, HdrImage, Primaries,
    RgbColorSpace, ToneMap,
};

/// Pull the next `f32` out of the byte stream, advancing the cursor by
/// four. Returns `0.0` once the stream is exhausted so the remaining
/// derivations stay deterministic on short inputs. The bytes are
/// reinterpreted *verbatim* — every bit pattern, including the NaN /
/// inf / subnormal encodings, is reachable.
fn next_f32(bytes: &[u8], cursor: &mut usize) -> f32 {
    let i = *cursor;
    *cursor += 4;
    if i + 4 > bytes.len() {
        return 0.0;
    }
    f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
}

fuzz_target!(|data: &[u8]| {
    // Layout: [w_byte][h_byte] then a run of verbatim f32 lanes used
    // first for the eight chromaticity floats, then for the pixel
    // buffer. Need at least the two dimension bytes.
    if data.len() < 2 {
        return;
    }

    // Small, bounded dimensions: 1..=32 px each side keeps the worst-case
    // buffer (32×32×3 = 3072 floats) tiny so the fuzzer spends its budget
    // on the numeric grammar, not on allocation.
    let width: u32 = (u32::from(data[0]) % 32) + 1;
    let height: u32 = (u32::from(data[1]) % 32) + 1;
    let pixel_count = (width as usize) * (height as usize);
    let n = pixel_count * 3;

    let mut cursor = 2usize;

    // Eight verbatim chromaticity floats → a fully unconstrained
    // Primaries record. Degenerate (zero-`y`, collinear, NaN) records
    // drive the singular-matrix `None` branch of the derivations.
    let primaries = Primaries {
        red: (next_f32(data, &mut cursor), next_f32(data, &mut cursor)),
        green: (next_f32(data, &mut cursor), next_f32(data, &mut cursor)),
        blue: (next_f32(data, &mut cursor), next_f32(data, &mut cursor)),
        white: (next_f32(data, &mut cursor), next_f32(data, &mut cursor)),
    };

    // Matrix derivations in isolation — the `Option` return is the
    // degeneracy oracle. Must never panic on a singular / non-finite
    // record.
    let _ = rgb_to_xyz_matrix_from_primaries(primaries);
    let _ = xyz_to_rgb_matrix_from_primaries(primaries);

    // Tone-mapping operator parameters, also verbatim from the stream so
    // negative / NaN / inf exposures and white points are reachable.
    let exposure = next_f32(data, &mut cursor);
    let gamma = next_f32(data, &mut cursor);
    let white_point = next_f32(data, &mut cursor);
    let linear_white = next_f32(data, &mut cursor);
    let scene_max = next_f32(data, &mut cursor);
    let bias = next_f32(data, &mut cursor);

    // Single-pixel conversions — feed the first three pixel-buffer lanes
    // (or zeros) through each named-space helper. Cheap, and reaches the
    // scalar `apply_matrix` path independent of the whole-image loop.
    let probe = [
        next_f32(data, &mut cursor),
        next_f32(data, &mut cursor),
        next_f32(data, &mut cursor),
    ];
    for space in [RgbColorSpace::Srgb, RgbColorSpace::Radiance] {
        let _ = xyz_to_rgb(probe, space);
        let _ = rgb_to_xyz(probe, space);
    }
    for fmt in [HdrFormat::Rgbe, HdrFormat::Xyze] {
        let _ = luminance_lm_per_sr_per_m2(probe, fmt);
    }

    // Build the pixel buffer from the *remaining* stream, reinterpreted
    // verbatim so NaN / ±inf / subnormal / negative samples all occur.
    let mut pixels = Vec::with_capacity(n);
    for _ in 0..n {
        pixels.push(next_f32(data, &mut cursor));
    }
    let base = HdrImage::new_rgb96f(width, height, pixels);

    // Named-space whole-image conversions: each must rewrite exactly
    // `n` floats in place and leave the buffer length unchanged.
    for space in [RgbColorSpace::Srgb, RgbColorSpace::Radiance] {
        let mut img = base.clone();
        convert_image_xyz_to_rgb(&mut img, space);
        assert_eq!(img.pixels.len(), n, "xyz_to_rgb preserves buffer length");
        let mut img = base.clone();
        convert_image_rgb_to_xyz(&mut img, space);
        assert_eq!(img.pixels.len(), n, "rgb_to_xyz preserves buffer length");
    }

    // Arbitrary-primaries conversions. The `bool` return is the
    // degenerate-record signal; on `false` the buffer + format tag must
    // be left untouched (length unchanged either way).
    {
        let mut img = base.clone();
        let _ran = convert_image_xyz_to_rgb_with_primaries(&mut img, primaries);
        assert_eq!(img.pixels.len(), n, "with_primaries preserves length");
        let mut img = base.clone();
        let _ran = convert_image_rgb_to_xyz_with_primaries(&mut img, primaries);
        assert_eq!(img.pixels.len(), n, "with_primaries preserves length");
    }

    // The `_with_effective_primaries` wrappers thread the picture's own
    // (here: default Radiance) primaries — a separate code path from the
    // explicit-record form.
    {
        let mut img = base.clone();
        let _ran = convert_image_xyz_to_rgb_with_effective_primaries(&mut img);
        assert_eq!(img.pixels.len(), n, "effective wrapper preserves length");
        let mut img = base.clone();
        let _ran = convert_image_rgb_to_xyz_with_effective_primaries(&mut img);
        assert_eq!(img.pixels.len(), n, "effective wrapper preserves length");
    }

    // The photometric (file-faithful, WHTEFFICACY-folded) converter
    // family — the same hostile buffer through the scaled matrices,
    // covering all six round-383 entry points.
    for space in [RgbColorSpace::Srgb, RgbColorSpace::Radiance] {
        let mut img = base.clone();
        convert_image_rgb_to_xyz_photometric(&mut img, space);
        assert_eq!(img.pixels.len(), n, "photometric preserves length");
        let mut img = base.clone();
        convert_image_xyz_to_rgb_photometric(&mut img, space);
        assert_eq!(img.pixels.len(), n, "photometric preserves length");
    }
    {
        let mut img = base.clone();
        let _ran = convert_image_rgb_to_xyz_photometric_with_primaries(&mut img, primaries);
        assert_eq!(img.pixels.len(), n, "photometric primaries len");
        let mut img = base.clone();
        let _ran = convert_image_xyz_to_rgb_photometric_with_primaries(&mut img, primaries);
        assert_eq!(img.pixels.len(), n, "photometric primaries len");
        let mut img = base.clone();
        let _ran = convert_image_rgb_to_xyz_photometric_with_effective_primaries(&mut img);
        assert_eq!(img.pixels.len(), n, "photometric effective len");
        let mut img = base.clone();
        let _ran = convert_image_xyz_to_rgb_photometric_with_effective_primaries(&mut img);
        assert_eq!(img.pixels.len(), n, "photometric effective len");
    }

    // Record-consistent exposure adjustment: an arbitrary fuzz factor
    // (NaN / inf / negative / zero are all reachable and must be
    // rejected without touching the buffer), an arbitrary stop count,
    // and the invariant that a successful adjustment keeps the buffer
    // length and never poisons the EXPOSURE slot with a non-finite
    // value.
    {
        let mut img = base.clone();
        let factor = exposure; // reuse a verbatim fuzz float
        let ran = img.adjust_exposure_factor(factor);
        assert_eq!(img.pixels.len(), n, "adjust_exposure preserves length");
        if ran {
            if let Some(e) = img.header.exposure {
                assert!(e.is_finite() && e > 0.0, "recorded EXPOSURE stays sane");
            }
        }
        let stops = i32::from(data[0] as i8); // full i8 range incl. negatives
        let _ = img.adjust_exposure_stops(stops);
        assert_eq!(img.pixels.len(), n, "stops adjustment preserves length");
    }

    // GAMMA= linearisation surface. The transfer exponent is a verbatim
    // fuzz float, so 0 / negative / NaN / inf (all treated as the identity
    // no-op) and huge finite exponents (whose power blows up to inf on the
    // hostile buffer) both reach the per-channel `stored^g`. None of the
    // helpers may panic, and every one must preserve the buffer length.
    {
        // In-place linearisation from a header slot: the slot must always
        // end cleared, and effective_gamma must report the identity 1.0
        // once it is.
        let mut img = base.clone();
        img.header.gamma = Some(gamma);
        img.linearize_gamma();
        assert_eq!(img.pixels.len(), n, "linearize_gamma preserves length");
        assert!(img.header.gamma.is_none(), "gamma slot cleared");
        assert!((img.effective_gamma() - 1.0).abs() < f32::EPSILON);

        // Non-mutating buffers: fixed lengths, slot preserved.
        let mut img = base.clone();
        img.header.gamma = Some(gamma);
        assert_eq!(img.linear_radiance_buffer().len(), n, "linear buffer len");
        assert_eq!(img.header.gamma, Some(gamma), "buffer view preserves slot");

        // Full linearise-then-divide recovery, mutating + buffer form,
        // stacked with arbitrary EXPOSURE / COLORCORR fuzz factors.
        let mut img = base.clone();
        img.header.gamma = Some(gamma);
        img.header.exposure = Some(exposure);
        img.header.colorcorr = Some([white_point, linear_white, scene_max]);
        assert_eq!(
            img.linear_scene_referred_radiance_buffer().len(),
            n,
            "linear scene-referred buffer len"
        );
        assert_eq!(
            img.linear_scene_referred_luminance_buffer().len(),
            pixel_count,
            "linear scene-referred luminance buffer len"
        );
        img.recover_linear_scene_referred_radiance();
        assert_eq!(img.pixels.len(), n, "recover preserves length");
        assert!(img.header.gamma.is_none() && img.header.exposure.is_none());

        // Writer-side encoding: degenerate exponents rejected without
        // touching the buffer; a successful call records a finite-positive
        // GAMMA slot.
        let mut img = base.clone();
        let ran = img.apply_gamma_encoding(gamma);
        assert_eq!(img.pixels.len(), n, "apply_gamma_encoding preserves length");
        if ran {
            let g = img.header.gamma.expect("successful encode records GAMMA");
            assert!(g.is_finite() && g > 0.0, "recorded GAMMA stays sane");
        } else {
            assert!(
                img.header.gamma.is_none(),
                "rejected encode records nothing"
            );
        }
    }

    // Wire-level exponent shift: every mantissa/exponent/stop shape must
    // either shift in range or report None — never panic or overflow.
    {
        let quad = [data[0], data[1], data[data.len() - 1], data[data.len() / 2]];
        for stops in [i32::MIN, -256, -1, 0, 1, 256, i32::MAX] {
            if let Some(s) = rgbe_shift_exponent(quad, stops) {
                if quad[3] != 0 {
                    assert!(s[3] != 0, "shift never lands on the sentinel byte");
                }
            }
        }
    }

    // Every tone-mapping operator, each fed the hostile float buffer and
    // fuzz-controlled parameters. The returned `Rgb24` buffer must be
    // exactly `n` bytes — the quantiser is responsible for clamping
    // NaN / inf samples into `0..=255` rather than panicking or wrapping.
    let ops = [
        ToneMap::Linear { exposure },
        ToneMap::Gamma { exposure, gamma },
        ToneMap::Reinhard { exposure },
        ToneMap::ReinhardExtended {
            exposure,
            white_point,
        },
        ToneMap::ReinhardLuminance {
            exposure,
            white_point,
        },
        ToneMap::Hable {
            exposure,
            linear_white,
        },
        ToneMap::Drago {
            exposure,
            scene_max,
            bias,
        },
        ToneMap::Aces { exposure },
    ];
    for op in ops {
        let out = tone_map(&base, op);
        assert_eq!(out.len(), n, "tone_map emits width*height*3 Rgb24 bytes");
    }
});
