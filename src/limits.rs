//! Pre-contract decoder resource limits.
//!
//! [`HdrLimits`] is superseded by [`crate::DecodeOptions`] (the
//! image-crate API contract's `max_width` / `max_height` / `max_pixels`
//! / `max_bytes` fields, `None` = unlimited). It is kept for one release
//! as a deprecated type with a lossless `From<HdrLimits> for
//! DecodeOptions` conversion so the deprecated `parse_hdr_with_limits`
//! wrappers keep compiling.

#![allow(deprecated)]

use crate::options::DecodeOptions;

/// Decoder resource limits applied during resolution-line validation.
///
/// All limits are **inclusive** — `max_width = 32_767` accepts a
/// 32 767-pixel-wide picture and rejects one of 32 768.
///
/// The defaults ([`HdrLimits::default`]):
///
/// | Field             | Default        | Why                                                                |
/// |-------------------|---------------:|--------------------------------------------------------------------|
/// | `max_width`       | 32 767         | Same ceiling the new-RLE scanline marker (`0x02 0x02 hi lo`) can address |
/// | `max_height`      | 32 767         | Symmetric to `max_width` — covers every renderer output the project has seen |
/// | `max_pixel_bytes` | 268 435 456 (256 MiB) | Hard ceiling on the `width * height * 3 * sizeof(f32)` float buffer the decoder allocates |
///
/// At the defaults a malicious header asking for a 32 767 × 32 767
/// image would resolve to `32 767 × 32 767 × 12 ≈ 12.9 GiB` and is
/// rejected before the decoder touches its allocator. A real
/// 4 096 × 4 096 picture only weighs 192 MiB so this leaves headroom
/// for typical use. Hand-rolled checks elsewhere in the decoder still
/// apply `checked_mul` on the same arithmetic so a custom limit set
/// that bumps `max_pixel_bytes` past `usize::MAX / 12` continues to
/// error gracefully rather than wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(note = "use oxideav_hdr::DecodeOptions (IMAGE_CRATE_API)")]
pub struct HdrLimits {
    /// Maximum picture width in pixels (inclusive). Zero is still
    /// rejected by [`crate::decode`] independent of this
    /// field.
    pub max_width: u32,
    /// Maximum picture height in pixels (inclusive).
    pub max_height: u32,
    /// Maximum decoded-pixel-buffer size in bytes (inclusive). The
    /// decoder pre-computes `width * height * 3 * 4` from the
    /// resolution line and compares against this before allocating.
    pub max_pixel_bytes: usize,
}

impl Default for HdrLimits {
    fn default() -> Self {
        Self {
            max_width: 32_767,
            max_height: 32_767,
            max_pixel_bytes: 256 * 1024 * 1024,
        }
    }
}

impl HdrLimits {
    /// Permissive limits suitable for trusted local input only — every
    /// dimension capped at `u32::MAX` and the pixel-buffer ceiling at
    /// `usize::MAX`. The decoder's internal `checked_mul` guards still
    /// trap pure arithmetic overflow so a hostile header doesn't wrap
    /// silently, but no early reject fires. **Do not** use on
    /// untrusted input — a header advertising `-Y u32::MAX +X u32::MAX`
    /// will be accepted only insofar as the multiplication itself
    /// returns `None`; the rejection message is "pixel-buffer size
    /// overflows usize" rather than a friendly "image too large".
    pub fn unbounded() -> Self {
        Self {
            max_width: u32::MAX,
            max_height: u32::MAX,
            max_pixel_bytes: usize::MAX,
        }
    }
}

impl From<HdrLimits> for DecodeOptions {
    /// Lossless mapping: the three inclusive caps become `Some(..)`
    /// limits; `u32::MAX` / `usize::MAX` ([`HdrLimits::unbounded`])
    /// become `None`.
    fn from(l: HdrLimits) -> Self {
        DecodeOptions::default()
            .with_max_width((l.max_width != u32::MAX).then_some(l.max_width))
            .with_max_height((l.max_height != u32::MAX).then_some(l.max_height))
            .with_max_pixels(None)
            .with_max_bytes((l.max_pixel_bytes != usize::MAX).then_some(l.max_pixel_bytes as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_admit_every_size_the_existing_test_corpus_uses() {
        let lim = HdrLimits::default();
        // 1×1 — smallest possible.
        assert!(1 <= lim.max_width && 1 <= lim.max_height);
        // The committed `tests/fixtures/` fixtures are at most 32×16.
        assert!(32 <= lim.max_width && 16 <= lim.max_height);
        // A 4K render output (3 840 × 2 160) — within the default cap.
        assert!(3_840 <= lim.max_width && 2_160 <= lim.max_height);
        // 4K × f32 RGB = 3 840 × 2 160 × 12 ≈ 95 MiB — fits in the
        // default pixel-byte cap of 256 MiB.
        let bytes = 3_840_usize * 2_160 * 12;
        assert!(bytes <= lim.max_pixel_bytes);
    }

    #[test]
    fn defaults_match_new_rle_marker_addressability() {
        // The new-RLE marker is `0x02 0x02 (W>>8) (W&0xFF)` which can
        // only address widths in `8..=32_767`. Mirroring that as the
        // max-width default keeps the two ceilings in sync — any file
        // whose resolution line passes the limit check is automatically
        // a candidate for the new-RLE write path.
        let lim = HdrLimits::default();
        assert_eq!(lim.max_width, 32_767);
        assert_eq!(lim.max_height, 32_767);
    }

    #[test]
    fn converts_losslessly_into_decode_options() {
        let d = DecodeOptions::from(HdrLimits::default());
        assert_eq!(d.max_width, Some(32_767));
        assert_eq!(d.max_height, Some(32_767));
        assert_eq!(d.max_pixels, None);
        assert_eq!(d.max_bytes, Some(256 * 1024 * 1024));
        let u = DecodeOptions::from(HdrLimits::unbounded());
        assert_eq!(u.max_width, None);
        assert_eq!(u.max_height, None);
        assert_eq!(u.max_bytes, None);
    }

    #[test]
    fn unbounded_actually_unbounded() {
        let lim = HdrLimits::unbounded();
        assert_eq!(lim.max_width, u32::MAX);
        assert_eq!(lim.max_height, u32::MAX);
        assert_eq!(lim.max_pixel_bytes, usize::MAX);
    }

    #[test]
    fn default_pixel_byte_cap_rejects_the_worst_case_max_dimension() {
        // `max_width` × `max_height` × 12 bytes/pixel ≈ 12.9 GiB —
        // far past the 256 MiB pixel-byte cap. The pixel-byte
        // check is therefore the operative gate at the defaults; the
        // dimension caps are a coarse pre-filter.
        let lim = HdrLimits::default();
        let worst_case = (lim.max_width as usize) * (lim.max_height as usize) * 12;
        assert!(worst_case > lim.max_pixel_bytes);
    }
}
