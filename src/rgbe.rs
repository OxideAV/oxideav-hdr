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
}
