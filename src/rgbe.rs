//! Shared-exponent RGBE pixel encoding — the four-byte representation
//! Greg Ward described in "Real Pixels" (Graphics Gems II, 1991).
//!
//! On disk each pixel is four bytes:
//! ```text
//! byte 0: red mantissa     (0..=255)
//! byte 1: green mantissa   (0..=255)
//! byte 2: blue mantissa    (0..=255)
//! byte 3: shared exponent  (0..=255, biased by 128)
//! ```
//!
//! Decode (per channel):
//! ```text
//! channel_f32 = (mantissa as f32 / 256.0) * 2 ^ (exponent as i32 - 128)
//! ```
//! When the exponent byte is `0` the four bytes denote a pure-zero
//! pixel and every channel decodes to `0.0`.
//!
//! Encode picks the channel of largest magnitude, derives the shared
//! exponent so the 8-bit mantissa of that channel sits in the
//! `[128, 256)` range (i.e. uses every available bit of dynamic range),
//! then scales the other two channels by the same exponent.

/// Returns `true` when an RGBE pixel is the all-zero sentinel the
/// staged spec (`docs/image/hdr/radiance-hdr-rgbe-format.md` §3)
/// documents as "exactly black; the zero exponent is the sentinel
/// for 'no value', so there is no valid pixel with exponent byte 0".
///
/// The sentinel test keys off the exponent byte alone — the mantissa
/// bytes (`rgbe[0..=2]`) are intentionally not inspected, mirroring
/// the rule embedded in [`rgbe_unbiased_exponent`] and [`rgbe_to_rgb`]
/// (both treat any `rgbe[3] == 0` as black regardless of the
/// mantissa values). The spec is explicit on this point: exponent
/// byte `0` is the "no value" marker regardless of what the
/// mantissas hold, so `[255, 255, 255, 0]` is just as much the
/// sentinel as `[0, 0, 0, 0]`.
///
/// This is the `bool`-returning counterpart to
/// [`rgbe_unbiased_exponent`] (which returns `Option<i32>` for the
/// same branch). Pick this inspector when only the boolean
/// "is this pixel the sentinel?" question matters — e.g. a scanline
/// walk that wants to skip the sentinel pixels before a luminance
/// reduction, or a fuzz oracle that counts how many sentinel
/// pixels a decoder encountered. Picking up only the boolean avoids
/// the `Option::is_none()` unwrap that the existing
/// `rgbe_unbiased_exponent` path requires for the same use-case,
/// and means the call site does not have to mentally substitute
/// "exponent value `None`" for "the pixel is the sentinel".
///
/// Contract is the spec's verbatim "no valid pixel with exponent
/// byte 0" rule: the function returns `true` if and only if
/// `rgbe[3] == 0`. Composes with the existing inspectors —
/// `rgbe_is_zero_pixel(p)` is exactly
/// `rgbe_unbiased_exponent(p).is_none()` and exactly
/// `rgbe_to_rgb(p) == [0.0, 0.0, 0.0]`, and any of the three
/// formulations may be picked at the call site for whichever
/// reads most naturally.
#[inline]
pub fn rgbe_is_zero_pixel(rgbe: [u8; 4]) -> bool {
    rgbe[3] == 0
}

/// Returns the unbiased shared exponent of an RGBE pixel — the
/// integer `n` such that each decoded channel equals
/// `(mantissa / 256) * 2^n` — or `None` when the pixel's exponent byte
/// is the all-zero sentinel that the staged spec
/// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §3) documents as
/// "exactly black; the zero exponent is the sentinel for 'no value',
/// so there is no valid pixel with exponent byte 0".
///
/// The on-disk exponent byte carries an **excess-128 bias** per spec
/// §3 ("The exponent byte carries an excess-128 bias"), so the
/// returned `i32` is `rgbe[3] as i32 - 128`. For the canonical worked
/// example `(R,G,B)=(1.0, 0.5, 0.25) -> bytes (128, 64, 32, 129)`
/// (spec §3) this returns `Some(1)` — the channels are `mantissa/256
/// * 2^1`, matching `128/256*2 = 1.0`, `64/256*2 = 0.5`,
/// `32/256*2 = 0.25`.
///
/// The mantissas (`rgbe[0..=2]`) are intentionally not inspected —
/// the sentinel rule keys off the exponent byte alone, and the spec
/// is explicit that exponent byte `0` is the "no value" marker
/// regardless of the mantissa values. Callers that need the full
/// channel triple should reach for [`rgbe_to_rgb`] instead; this
/// inspector is for the common "what magnitude does this pixel sit
/// at?" use-case where building the three `f32` channels would be
/// wasted work (e.g. picking a per-pixel auto-exposure factor without
/// fully decoding the picture, or filtering out the sentinel pixels
/// before a luminance scan).
///
/// Returning `Option<i32>` lets the sentinel case be matched
/// explicitly without the caller re-deriving the "exponent == 0
/// means black" rule at every call site. The shape mirrors the
/// `effective_*` family on [`crate::HdrImage`]: a single inspector
/// that embeds one spec-documented quirk and returns the
/// straightforward value otherwise.
#[inline]
pub fn rgbe_unbiased_exponent(rgbe: [u8; 4]) -> Option<i32> {
    let e = rgbe[3];
    if e == 0 {
        None
    } else {
        Some(e as i32 - 128)
    }
}

/// Returns the shared per-channel scale factor of an RGBE pixel — the
/// `f32` value `f` such that each decoded channel equals
/// `mantissa_byte as f32 * f` — or `None` when the pixel's exponent
/// byte is the all-zero sentinel the staged spec
/// (`docs/image/hdr/radiance-hdr-rgbe-format.md` §3) documents as
/// "exactly black; the zero exponent is the sentinel for 'no value',
/// so there is no valid pixel with exponent byte 0".
///
/// The returned factor is the staged spec's decode formula verbatim:
/// `f = ldexp(1.0, rgbe[3] - (128 + 8))` ("remove bias + 8-bit
/// scale" — the exponent byte carries an excess-128 bias per spec §3
/// and the 256-mantissa scale contributes the extra `-8`). For the
/// spec-canonical worked example `(R,G,B)=(1.0, 0.5, 0.25) -> bytes
/// (128, 64, 32, 129)` this returns `Some(2^-7)` = `Some(0.0078125)`,
/// and `128 * 2^-7 = 1.0`, `64 * 2^-7 = 0.5`, `32 * 2^-7 = 0.25`
/// recover the three channels exactly.
///
/// This completes the quad-inspector trio: [`rgbe_is_zero_pixel`]
/// answers "is this pixel the sentinel?" (`bool`),
/// [`rgbe_unbiased_exponent`] answers "what magnitude does it sit at?"
/// (the `n` in `(mantissa / 256) * 2^n`), and this inspector answers
/// "what do I multiply the mantissa bytes by?" (the ready-to-use
/// `f32` factor, `f = 2^n / 256`). Pick this one when the call site
/// actually multiplies — e.g. a luminance reduction that folds the
/// three mantissa bytes through their weights and applies the shared
/// scale once (`lum_scale = f * (w_r*m_r + w_g*m_g + w_b*m_b)`), or a
/// single-channel probe that wants `rgbe[1] as f32 * f` without
/// paying for the other two channels the way [`rgbe_to_rgb`] does.
///
/// The mantissas (`rgbe[0..=2]`) are intentionally not inspected —
/// the sentinel rule keys off the exponent byte alone, matching
/// [`rgbe_is_zero_pixel`] / [`rgbe_unbiased_exponent`] /
/// [`rgbe_to_rgb`]. The factor is computed with the same `ldexp`
/// helper [`rgbe_to_rgb`] uses internally, so
/// `rgbe_to_rgb(p)[i] == p[i] as f32 * rgbe_channel_scale(p).unwrap()`
/// holds bit-exactly for every non-sentinel quad; the full exponent
/// range stays finite and nonzero in `f32` (byte `1` ⇒ `2^-135`, a
/// subnormal; byte `255` ⇒ `2^119`).
#[inline]
pub fn rgbe_channel_scale(rgbe: [u8; 4]) -> Option<f32> {
    let e = rgbe[3];
    if e == 0 {
        None
    } else {
        // Spec §3 decode formula: f = ldexp(1.0, rgbe[3] - (128 + 8)).
        Some(ldexp(1.0, e as i32 - 128 - 8))
    }
}

/// Decode one shared-exponent pixel. The output array is `[R, G, B]`
/// in the source colour space (RGB for `32-bit_rle_rgbe` files, CIE
/// XYZ for `32-bit_rle_xyze`).
#[inline]
pub fn rgbe_to_rgb(rgbe: [u8; 4]) -> [f32; 3] {
    let e = rgbe[3];
    if e == 0 {
        return [0.0, 0.0, 0.0];
    }
    // 2^(e - 128) split into a mantissa scale of 1/256 (the `256.0`
    // in the denominator below) and the integer-power-of-two factor.
    let factor = ldexp(1.0, e as i32 - 128 - 8);
    [
        rgbe[0] as f32 * factor,
        rgbe[1] as f32 * factor,
        rgbe[2] as f32 * factor,
    ]
}

/// Encode one linear RGB triple into the four-byte shared-exponent
/// representation. Negative or non-finite inputs are clamped to zero
/// before encoding (the format has no representation for either).
///
/// ## Idempotence on normalised quads
///
/// The quantiser is **idempotent over the subset of RGBE quads it
/// produces**: `frexp` always normalises the dominant channel's mantissa
/// into `[128, 256)`, so for any quad whose dominant mantissa is `>= 128`
/// and whose decoded magnitude is above the `1e-32` black floor,
/// `rgb_to_rgbe(rgbe_to_rgb(q)) == q` holds bit-exactly. This is what
/// makes the byte-level round-trip surface
/// ([`crate::HdrImage::from_rgbe_quads`] /
/// [`crate::HdrImage::to_rgbe_quads`]) lossless: a picture built from
/// such quads re-encodes to exactly the same bytes. Quads outside the
/// subset (a denormalised mantissa with no channel at `>= 128`, or a
/// decoded magnitude below `1e-32`) re-encode to a renormalised /
/// black-flushed quad instead.
#[inline]
pub fn rgb_to_rgbe(rgb: [f32; 3]) -> [u8; 4] {
    let r = sanitize(rgb[0]);
    let g = sanitize(rgb[1]);
    let b = sanitize(rgb[2]);
    let max = r.max(g).max(b);
    if max < 1.0e-32 {
        return [0, 0, 0, 0];
    }
    // frexp-style split: max = significand * 2^exp, with significand in
    // [0.5, 1.0). Scaling by 256/max then puts each channel's mantissa
    // into [0, 256).
    let (significand, exp) = frexp(max);
    let scale = significand * 256.0 / max;
    let er = (r * scale) as u32;
    let eg = (g * scale) as u32;
    let eb = (b * scale) as u32;
    let exponent_byte = exp + 128;
    // Defensive clamp — for inputs outside the representable range
    // we clamp the exponent byte rather than panic.
    let exponent_byte = exponent_byte.clamp(1, 255) as u8;
    [
        er.min(255) as u8,
        eg.min(255) as u8,
        eb.min(255) as u8,
        exponent_byte,
    ]
}

/// `f * 2^n`. Avoids pulling in `libm`.
#[inline]
fn ldexp(f: f32, n: i32) -> f32 {
    // Build `2^n` directly via `f64::powi` so we cover the full 8-bit
    // exponent range without relying on `f32::powi` precision.
    f * (2.0f64.powi(n)) as f32
}

/// Decompose `f` into `(significand, exponent)` such that
/// `f = significand * 2^exponent` with `significand` in `[0.5, 1.0)`.
/// Mirrors C's `frexpf`. Pre-condition: `f` is finite and `> 0`.
#[inline]
fn frexp(f: f32) -> (f32, i32) {
    let bits = f.to_bits();
    let raw_exp = ((bits >> 23) & 0xFF) as i32;
    if raw_exp == 0 {
        // Subnormal — normalise by hand.
        let scaled = f * (1u64 << 32) as f32;
        let (s, e) = frexp(scaled);
        return (s, e - 32);
    }
    let exponent = raw_exp - 126; // ieee bias 127, then -1 to land in [0.5, 1.0)
    let mantissa_bits = (bits & 0x807F_FFFF) | (126u32 << 23);
    (f32::from_bits(mantissa_bits), exponent)
}

/// Replace negatives, NaNs and infinities with zero — the on-disk
/// format has no representation for any of them.
#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_pixel_roundtrips() {
        assert_eq!(rgb_to_rgbe([0.0, 0.0, 0.0]), [0, 0, 0, 0]);
        assert_eq!(rgbe_to_rgb([0, 0, 0, 0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn unit_pixel_roundtrips_within_quantisation() {
        let original = [1.0_f32, 0.5, 0.25];
        let rgbe = rgb_to_rgbe(original);
        let back = rgbe_to_rgb(rgbe);
        for i in 0..3 {
            // Allow up to ~1/256 relative error from the 8-bit mantissa.
            let err = (back[i] - original[i]).abs() / original[i].max(1e-30);
            assert!(err < 1.0 / 200.0, "ch {i}: {} vs {}", original[i], back[i]);
        }
    }

    #[test]
    fn high_dynamic_range_roundtrips() {
        for &x in &[1.0e-4_f32, 1.0e-2, 1.0, 1.0e2, 1.0e4, 1.0e6] {
            let rgbe = rgb_to_rgbe([x, x * 0.7, x * 0.3]);
            let back = rgbe_to_rgb(rgbe);
            // Each channel should be within ~0.5% relative error.
            assert!(
                (back[0] / x - 1.0).abs() < 0.01,
                "x={x}: back[0]={} vs {x}",
                back[0]
            );
        }
    }

    #[test]
    fn negative_and_nan_clamp_to_zero() {
        let rgbe = rgb_to_rgbe([-1.0, f32::NAN, f32::INFINITY]);
        assert_eq!(rgbe, [0, 0, 0, 0]);
    }

    #[test]
    fn unbiased_exponent_zero_pixel_is_sentinel() {
        // Spec §3: the all-zero RGBE quad means "exactly black; the
        // zero exponent is the sentinel for 'no value'". Returning
        // None pins that branch.
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 0]), None);
    }

    #[test]
    fn unbiased_exponent_sentinel_keys_off_exponent_only() {
        // Mantissas must not influence the sentinel test — only the
        // exponent byte == 0 marks a no-value pixel per spec §3.
        assert_eq!(rgbe_unbiased_exponent([255, 255, 255, 0]), None);
        assert_eq!(rgbe_unbiased_exponent([7, 11, 200, 0]), None);
    }

    #[test]
    fn unbiased_exponent_spec_worked_example() {
        // Spec §3: (R,G,B)=(1.0, 0.5, 0.25) -> bytes (128, 64, 32, 129).
        // Channels equal `mantissa/256 * 2^n`; with mantissa 128
        // giving 1.0 the unbiased exponent must be 1
        // (128/256 * 2^1 = 1.0).
        assert_eq!(rgbe_unbiased_exponent([128, 64, 32, 129]), Some(1));
    }

    #[test]
    fn unbiased_exponent_byte_128_is_zero() {
        // The excess-128 bias means a stored byte of 128 decodes to
        // an unbiased exponent of 0 — the channel-scale boundary
        // where mantissa/256 IS the channel value.
        assert_eq!(rgbe_unbiased_exponent([200, 100, 50, 128]), Some(0));
    }

    #[test]
    fn unbiased_exponent_full_range_byte_values_pin_bias() {
        // Pin every non-sentinel exponent byte to the
        // `byte - 128` formula across the boundary cases the spec
        // documents: 1 -> -127, 127 -> -1, 128 -> 0, 129 -> 1,
        // 255 -> 127.
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 1]), Some(-127));
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 127]), Some(-1));
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 128]), Some(0));
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 129]), Some(1));
        assert_eq!(rgbe_unbiased_exponent([0, 0, 0, 255]), Some(127));
    }

    #[test]
    fn unbiased_exponent_agrees_with_rgbe_to_rgb_magnitude() {
        // Cross-check: for a non-sentinel pixel the returned exponent
        // `n` must satisfy `decoded[i] == mantissa[i] / 256 * 2^n`
        // exactly (the channel-decode formula the inspector summarises).
        let rgbe = [200_u8, 100, 50, 130];
        let n = rgbe_unbiased_exponent(rgbe).expect("non-sentinel");
        let decoded = rgbe_to_rgb(rgbe);
        let scale = (2.0_f32).powi(n) / 256.0;
        for (i, &m) in rgbe[..3].iter().enumerate() {
            let expected = m as f32 * scale;
            assert!(
                (decoded[i] - expected).abs() < 1e-6,
                "ch {i}: decoded {} vs formula {}",
                decoded[i],
                expected
            );
        }
    }

    #[test]
    fn is_zero_pixel_matches_sentinel_byte() {
        // Spec §3: the all-zero quad is "exactly black; the zero
        // exponent is the sentinel for 'no value'". The canonical
        // sentinel shape returns true.
        assert!(rgbe_is_zero_pixel([0, 0, 0, 0]));
    }

    #[test]
    fn is_zero_pixel_keys_off_exponent_only() {
        // Mantissas must not influence the test — only `rgbe[3] == 0`
        // marks a no-value pixel per spec §3.
        assert!(rgbe_is_zero_pixel([255, 255, 255, 0]));
        assert!(rgbe_is_zero_pixel([7, 11, 200, 0]));
        assert!(rgbe_is_zero_pixel([1, 1, 1, 0]));
    }

    #[test]
    fn is_zero_pixel_false_for_every_nonzero_exponent() {
        // Boundary bytes the spec §3 worked example + bias
        // documentation imply: 1 (most negative unbiased), 127, 128
        // (exponent-zero boundary), 129 (the worked example), 255
        // (most positive unbiased). All must be reported as
        // non-sentinel regardless of the mantissa bytes.
        for &e in &[1u8, 127, 128, 129, 255] {
            assert!(!rgbe_is_zero_pixel([0, 0, 0, e]));
            assert!(!rgbe_is_zero_pixel([200, 100, 50, e]));
        }
        // Spec §3 worked example pixel.
        assert!(!rgbe_is_zero_pixel([128, 64, 32, 129]));
    }

    #[test]
    fn is_zero_pixel_agrees_with_unbiased_exponent_none_branch() {
        // The boolean inspector composes with the existing
        // `rgbe_unbiased_exponent` inspector: `is_zero_pixel(p)` ==
        // `unbiased_exponent(p).is_none()` for every possible quad.
        // Exhaustively walk every exponent byte (the only byte the
        // sentinel rule keys off) with two mantissa shapes to pin
        // the cross-formulation invariant.
        for e in 0u8..=255 {
            for mantissas in &[[0u8, 0, 0], [200, 100, 50]] {
                let p = [mantissas[0], mantissas[1], mantissas[2], e];
                assert_eq!(
                    rgbe_is_zero_pixel(p),
                    rgbe_unbiased_exponent(p).is_none(),
                    "disagreement on quad {p:?}"
                );
            }
        }
    }

    #[test]
    fn is_zero_pixel_agrees_with_rgbe_to_rgb_black_branch() {
        // Cross-check against the decode formula: a sentinel pixel
        // decodes to `[0.0, 0.0, 0.0]`, and `rgbe_to_rgb` returns a
        // strictly-positive triple for any non-sentinel pixel with
        // at least one nonzero mantissa.
        for e in 0u8..=255 {
            let p = [128_u8, 64, 32, e];
            let decoded = rgbe_to_rgb(p);
            let is_black = decoded == [0.0, 0.0, 0.0];
            assert_eq!(
                rgbe_is_zero_pixel(p),
                is_black,
                "quad {p:?} decoded to {decoded:?}"
            );
        }
    }

    #[test]
    fn is_zero_pixel_round_trips_through_encoder() {
        // After encoding a black RGB triple the encoder produces the
        // all-zero quad; the inspector reports that as the sentinel.
        // A non-zero RGB triple produces a quad with a nonzero
        // exponent byte and the inspector reports false.
        assert!(rgbe_is_zero_pixel(rgb_to_rgbe([0.0, 0.0, 0.0])));
        assert!(!rgbe_is_zero_pixel(rgb_to_rgbe([4.0, 2.0, 1.0])));
        // The defensive "negative / non-finite clamps to zero" branch
        // also produces the sentinel (matches the rgb_to_rgbe
        // documented behaviour: "Negative or non-finite inputs are
        // clamped to zero before encoding").
        assert!(rgbe_is_zero_pixel(rgb_to_rgbe([
            -1.0,
            f32::NAN,
            f32::INFINITY,
        ])));
    }

    #[test]
    fn channel_scale_zero_pixel_is_sentinel() {
        // Spec §3: exponent byte 0 is the "no value" sentinel — there
        // is no scale factor for a sentinel pixel.
        assert_eq!(rgbe_channel_scale([0, 0, 0, 0]), None);
    }

    #[test]
    fn channel_scale_sentinel_keys_off_exponent_only() {
        // Mantissas must not influence the sentinel test — only the
        // exponent byte == 0 marks a no-value pixel per spec §3.
        assert_eq!(rgbe_channel_scale([255, 255, 255, 0]), None);
        assert_eq!(rgbe_channel_scale([7, 11, 200, 0]), None);
    }

    #[test]
    fn channel_scale_spec_worked_example() {
        // Spec §3: (R,G,B)=(1.0, 0.5, 0.25) -> bytes (128, 64, 32, 129).
        // The decode formula f = ldexp(1.0, 129 - (128 + 8)) = 2^-7,
        // and mantissa * f recovers each channel exactly.
        let scale = rgbe_channel_scale([128, 64, 32, 129]).expect("non-sentinel");
        assert_eq!(scale, 0.0078125); // 2^-7, exact in f32
        assert_eq!(128.0 * scale, 1.0);
        assert_eq!(64.0 * scale, 0.5);
        assert_eq!(32.0 * scale, 0.25);
    }

    #[test]
    fn channel_scale_boundary_bytes_pin_formula() {
        // Pin the f = 2^(byte - 136) formula across the byte range:
        // byte 136 is the unit-scale boundary (mantissa byte == channel
        // value), byte 1 lands on a subnormal-but-exact power of two,
        // byte 255 stays finite at 2^119.
        assert_eq!(rgbe_channel_scale([0, 0, 0, 136]), Some(1.0));
        assert_eq!(rgbe_channel_scale([0, 0, 0, 135]), Some(0.5));
        assert_eq!(rgbe_channel_scale([0, 0, 0, 137]), Some(2.0));
        // Expected powers computed in f64 then narrowed — `f32::powi`
        // can't reach 2^-135 directly (the 2^135 reciprocal step
        // overflows f32), but the f64 value narrows to the exact
        // subnormal.
        assert_eq!(
            rgbe_channel_scale([0, 0, 0, 1]),
            Some((2.0_f64).powi(-135) as f32)
        );
        assert_eq!(
            rgbe_channel_scale([0, 0, 0, 255]),
            Some((2.0_f64).powi(119) as f32)
        );
        // Every non-sentinel scale is finite and strictly positive.
        for e in 1u8..=255 {
            let f = rgbe_channel_scale([0, 0, 0, e]).expect("non-sentinel");
            assert!(f.is_finite() && f > 0.0, "byte {e}: scale {f}");
        }
    }

    #[test]
    fn channel_scale_agrees_with_rgbe_to_rgb_bit_exactly() {
        // Cross-check against the decode path: for every exponent byte
        // the factor must reproduce rgbe_to_rgb's channels bit-exactly
        // (both paths run the same spec-§3 ldexp formula).
        for e in 0u8..=255 {
            let p = [200_u8, 100, 50, e];
            let decoded = rgbe_to_rgb(p);
            match rgbe_channel_scale(p) {
                None => assert_eq!(decoded, [0.0, 0.0, 0.0], "sentinel byte {e}"),
                Some(f) => {
                    for (i, &m) in p[..3].iter().enumerate() {
                        assert_eq!(
                            decoded[i],
                            m as f32 * f,
                            "ch {i} byte {e}: decode disagrees with mantissa * scale"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn channel_scale_composes_with_sibling_inspectors() {
        // Trio invariants for every exponent byte: the sentinel branch
        // matches rgbe_is_zero_pixel, and on the non-sentinel branch
        // f == 2^n / 256 with n from rgbe_unbiased_exponent.
        for e in 0u8..=255 {
            let p = [128_u8, 64, 32, e];
            let scale = rgbe_channel_scale(p);
            assert_eq!(scale.is_none(), rgbe_is_zero_pixel(p), "byte {e}");
            if let Some(f) = scale {
                let n = rgbe_unbiased_exponent(p).expect("non-sentinel");
                // 2^n / 256 computed in f64 then narrowed, mirroring
                // the ldexp helper — exact for the whole byte range.
                let expected = (2.0_f64.powi(n) / 256.0) as f32;
                assert_eq!(f, expected, "byte {e}: f {f} vs 2^{n}/256 {expected}");
            }
        }
    }

    #[test]
    fn channel_scale_roundtrips_through_encoder() {
        // After encoding a non-zero RGB triple, mantissa * scale of the
        // produced quad recovers values within the format's mantissa
        // quantisation; a black-pixel encode produces the sentinel.
        let rgbe = rgb_to_rgbe([4.0, 2.0, 1.0]);
        // 4.0 = 0.5 * 2^3 ⇒ exponent byte 131 ⇒ f = 2^(131-136) = 2^-5.
        let f = rgbe_channel_scale(rgbe).expect("non-sentinel");
        assert_eq!(f, 0.03125);
        assert_eq!(rgbe[0] as f32 * f, 4.0);
        assert_eq!(rgbe[1] as f32 * f, 2.0);
        assert_eq!(rgbe[2] as f32 * f, 1.0);
        assert_eq!(rgbe_channel_scale(rgb_to_rgbe([0.0, 0.0, 0.0])), None);
    }

    #[test]
    fn normalised_quad_encode_decode_is_idempotent() {
        // The bit-exact contract that `HdrImage::{from,to}_rgbe_quads`
        // and the round-trip matrix rest on: for any *normalised* quad
        // (dominant mantissa >= 128, decoded magnitude above the 1e-32
        // black floor) `rgb_to_rgbe(rgbe_to_rgb(q)) == q` exactly. The
        // encoder always produces quads in this subset (frexp normalises
        // the dominant channel into [128, 256)), so a picture built from
        // such quads re-encodes byte-for-byte. Walk a dense grid of
        // exponents and mantissa shapes that all sit in the subset.
        for e in 23u8..=255 {
            // e >= 23 keeps the dominant mantissa-128 magnitude
            // (2^(e-129)) at or above the 1e-32 floor.
            for &dom in &[128u8, 150, 200, 255] {
                let shapes = [
                    [dom, dom / 2, dom / 4, e],
                    [dom / 4, dom, dom / 2, e],
                    [dom / 2, dom / 4, dom, e],
                    [dom, dom, dom, e],
                    [dom, 0, 0, e],
                    [0, dom, 0, e],
                    [0, 0, dom, e],
                ];
                for q in shapes {
                    let back = rgb_to_rgbe(rgbe_to_rgb(q));
                    assert_eq!(back, q, "idempotence drifted for quad {q:?}");
                }
            }
        }
    }

    #[test]
    fn sub_floor_quad_flushes_to_black_on_re_encode() {
        // The lower boundary of the idempotent subset: a quad whose
        // decoded magnitude is below the 1e-32 black floor re-encodes to
        // the all-zero sentinel rather than to itself. Byte 1 (unbiased
        // -127) with mantissa 255 decodes to ~1.5e-38 < 1e-32.
        let q = [255u8, 255, 255, 1];
        assert_eq!(rgb_to_rgbe(rgbe_to_rgb(q)), [0, 0, 0, 0]);
    }

    #[test]
    fn unbiased_exponent_roundtrips_through_encoder() {
        // After encoding a non-zero RGB triple the exponent the
        // encoder selected is exactly the value the inspector reports;
        // a black-pixel encode produces the sentinel byte and the
        // inspector reflects that as None.
        let rgbe = rgb_to_rgbe([4.0, 2.0, 1.0]);
        // 4.0 = 0.5 * 2^3 ⇒ encoder picks exponent 3 (frexp), stored
        // as byte 131 (excess-128).
        assert_eq!(rgbe_unbiased_exponent(rgbe), Some(3));
        // Black encodes to the all-zero quad, which the inspector
        // reports as the sentinel.
        let black = rgb_to_rgbe([0.0, 0.0, 0.0]);
        assert_eq!(black, [0, 0, 0, 0]);
        assert_eq!(rgbe_unbiased_exponent(black), None);
    }
}
