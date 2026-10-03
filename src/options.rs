//! Decode-side limits and strictness ([`DecodeOptions`]) and the
//! encode-side knobs ([`EncodeOptions`]) of the standalone API.

use crate::encoder::{LineEnding, MagicLine, RleMode};
use crate::error::{HdrError, Result};
use crate::header::Primaries;
use crate::rle::FallbackMode;

/// Limits and strictness for [`crate::decode_with`].
///
/// Every limit is checked against the resolution line **before** the
/// pixel plane is allocated, so a hostile header fails with
/// [`HdrError::LimitExceeded`] instead of committing memory. The
/// defaults: `max_width` / `max_height` of
/// [`DecodeOptions::DEFAULT_MAX_DIMENSION`] (32 767 — the widest
/// scanline the new-RLE marker can address), no pixel-count limit, the
/// decoded plane capped at [`DecodeOptions::DEFAULT_MAX_BYTES`] (1 GiB
/// of `f32` RGB, i.e. ~89 megapixels), `strict = false`, and
/// [`FallbackMode::OldRle`] for scanlines without the new-RLE marker.
///
/// `strict` rejects what the lenient decoder tolerates per the staged
/// spec's "must" clauses: a magic identifier other than `RADIANCE` /
/// `RGBE`, a header without a `FORMAT=` record (the spec says a valid
/// picture carries exactly one), and trailing bytes after the last
/// scanline. Per-scanline detection of a missing new-RLE marker is
/// normative behaviour, not leniency, so it is unaffected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Reject pictures wider than this (pixels, display orientation).
    pub max_width: Option<u32>,
    /// Reject pictures taller than this (pixels, display orientation).
    pub max_height: Option<u32>,
    /// Reject pictures with more than this many pixels (`width ×
    /// height`).
    pub max_pixels: Option<u64>,
    /// Reject pictures whose decoded plane would exceed this many
    /// bytes (`width × height × 12`).
    pub max_bytes: Option<u64>,
    /// Reject the recoverable irregularities listed in the type docs.
    pub strict: bool,
    /// How a scanline *without* the new-RLE marker is read: as the
    /// pre-1991 sentinel-run old-RLE flavour (the default) or as flat
    /// uncompressed quads. See [`FallbackMode`].
    pub fallback: FallbackMode,
}

impl DecodeOptions {
    /// Default [`Self::max_width`] / [`Self::max_height`]: 32 767.
    pub const DEFAULT_MAX_DIMENSION: u32 = 32_767;
    /// Default [`Self::max_bytes`]: 1 GiB of decoded plane.
    pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;

    /// The defaults (see the type docs).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or lift with `None`) the width limit.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set (or lift with `None`) the height limit.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set (or lift with `None`) the pixel-count limit.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set (or lift with `None`) the decoded-bytes limit.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set strict mode (see the type docs).
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Set how marker-less scanlines are read.
    pub fn with_fallback(mut self, fallback: FallbackMode) -> Self {
        self.fallback = fallback;
        self
    }

    /// Lift every limit (`max_*` all `None`). Trusted local input
    /// only — a hostile header is then bounded by the allocator alone.
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self
    }

    /// Check a resolution line's geometry against the limits. `bytes`
    /// is the decoded plane size the native layout implies.
    pub(crate) fn check(&self, width: u32, height: u32, bytes: u64) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(HdrError::limit(format!(
                    "HDR: resolution width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(HdrError::limit(format!(
                    "HDR: resolution height {height} exceeds max_height {m}"
                )));
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        if let Some(m) = self.max_pixels {
            if pixels > m {
                return Err(HdrError::limit(format!(
                    "HDR: {pixels} pixels exceed max_pixels {m}"
                )));
            }
        }
        if let Some(m) = self.max_bytes {
            if bytes > m {
                return Err(HdrError::limit(format!(
                    "HDR: decoded plane of {bytes} bytes exceeds max_bytes {m}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: Some(Self::DEFAULT_MAX_DIMENSION),
            max_height: Some(Self::DEFAULT_MAX_DIMENSION),
            max_pixels: None,
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
            fallback: FallbackMode::OldRle,
        }
    }
}

/// Encoder knobs for [`crate::encode`] / [`crate::encode_rgb8`] /
/// [`crate::encode_rgba8`].
///
/// The on-wire choices are fields: the scanline [`RleMode`] (default
/// [`RleMode::Auto`] — new-RLE for on-disk scanline widths in
/// `8..=32767`, old-RLE otherwise, so every geometry encodes), the
/// text-section [`LineEnding`] (default LF) and
/// the magic line (`None` = the identifier the image's header carries,
/// `#?RADIANCE` for a freshly built image). The header records that a
/// raw 8-bit caller cannot otherwise reach are fields too: `exposure`,
/// `software`, `primaries` and `gamma`; when `Some` they **override**
/// the matching slot of [`crate::HdrImage::header`] for the one encode
/// (the image is not modified), when `None` the image's header is
/// written as is. `input_gamma` only affects the raw 8-bit paths
/// (`encode_rgb8` / `encode_rgba8`): `None` treats the bytes as linear
/// (`b / 255`), `Some(g)` linearises them as `(b / 255) ^ g`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Scanline RLE flavour.
    pub rle: RleMode,
    /// Text-section line terminator.
    pub line_ending: LineEnding,
    /// Magic-line identifier; `None` preserves the image's.
    pub magic: Option<MagicLine>,
    /// `EXPOSURE=` override.
    pub exposure: Option<f32>,
    /// `SOFTWARE=` override.
    pub software: Option<String>,
    /// `PRIMARIES=` override.
    pub primaries: Option<Primaries>,
    /// `GAMMA=` override (records that the stored values are
    /// gamma-encoded with this exponent; it does not transform them).
    pub gamma: Option<f32>,
    /// Linearisation exponent for 8-bit input (raw paths only).
    pub input_gamma: Option<f32>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            rle: RleMode::Auto,
            line_ending: LineEnding::Lf,
            magic: None,
            exposure: None,
            software: None,
            primaries: None,
            gamma: None,
            input_gamma: None,
        }
    }
}

impl EncodeOptions {
    /// The defaults (auto RLE, LF, header as carried by the image).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the scanline RLE flavour.
    pub fn with_rle(mut self, rle: RleMode) -> Self {
        self.rle = rle;
        self
    }

    /// Set the text-section line terminator.
    pub fn with_line_ending(mut self, line_ending: LineEnding) -> Self {
        self.line_ending = line_ending;
        self
    }

    /// Force (or with `None`, preserve) the magic-line identifier.
    pub fn with_magic(mut self, magic: impl Into<Option<MagicLine>>) -> Self {
        self.magic = magic.into();
        self
    }

    /// Set (or clear) the `EXPOSURE=` override.
    pub fn with_exposure(mut self, exposure: impl Into<Option<f32>>) -> Self {
        self.exposure = exposure.into();
        self
    }

    /// Set (or clear) the `SOFTWARE=` override.
    pub fn with_software(mut self, software: impl Into<Option<String>>) -> Self {
        self.software = software.into();
        self
    }

    /// Set (or clear) the `PRIMARIES=` override.
    pub fn with_primaries(mut self, primaries: impl Into<Option<Primaries>>) -> Self {
        self.primaries = primaries.into();
        self
    }

    /// Set (or clear) the `GAMMA=` override.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// Set (or clear) the 8-bit input linearisation exponent.
    pub fn with_input_gamma(mut self, input_gamma: impl Into<Option<f32>>) -> Self {
        self.input_gamma = input_gamma.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_fire_in_order() {
        let o = DecodeOptions::default()
            .with_max_width(10u32)
            .with_max_height(10u32)
            .with_max_pixels(50u64)
            .with_max_bytes(100u64);
        assert!(o.check(5, 5, 75).is_ok());
        assert!(matches!(o.check(11, 1, 1), Err(HdrError::LimitExceeded(_))));
        assert!(matches!(o.check(1, 11, 1), Err(HdrError::LimitExceeded(_))));
        assert!(matches!(o.check(8, 8, 1), Err(HdrError::LimitExceeded(_))));
        assert!(matches!(
            o.check(5, 5, 101),
            Err(HdrError::LimitExceeded(_))
        ));
        assert!(o.unlimited().check(u32::MAX, u32::MAX, u64::MAX).is_ok());
    }

    #[test]
    fn defaults_are_finite_and_lenient() {
        let d = DecodeOptions::default();
        assert_eq!(d.max_width, Some(32_767));
        assert_eq!(d.max_height, Some(32_767));
        assert_eq!(d.max_pixels, None);
        assert_eq!(d.max_bytes, Some(1 << 30));
        assert!(!d.strict);
        assert_eq!(d.fallback, FallbackMode::OldRle);
        // A 4K render (3840 × 2160 × 12 ≈ 95 MiB) fits the default cap;
        // the worst-case max-dimension square does not.
        assert!(d.check(3840, 2160, 3840 * 2160 * 12).is_ok());
        assert!(d.check(32_767, 32_767, 32_767 * 32_767 * 12).is_err());
    }

    #[test]
    fn encode_defaults_preserve_the_header() {
        let e = EncodeOptions::default();
        assert_eq!(e.rle, RleMode::Auto);
        assert_eq!(e.line_ending, LineEnding::Lf);
        assert!(e.magic.is_none());
        assert!(e.exposure.is_none() && e.software.is_none());
        assert!(e.primaries.is_none() && e.gamma.is_none());
        assert!(e.input_gamma.is_none());
        let e = e
            .with_rle(RleMode::Uncompressed)
            .with_line_ending(LineEnding::Crlf)
            .with_magic(MagicLine::Rgbe)
            .with_exposure(2.0)
            .with_software("test".to_string())
            .with_primaries(Primaries::SRGB)
            .with_gamma(2.2)
            .with_input_gamma(2.2);
        assert_eq!(e.rle, RleMode::Uncompressed);
        assert_eq!(e.magic, Some(MagicLine::Rgbe));
        assert_eq!(e.exposure, Some(2.0));
        assert_eq!(e.software.as_deref(), Some("test"));
        assert_eq!(e.primaries, Some(Primaries::SRGB));
        assert_eq!(e.gamma, Some(2.2));
        assert_eq!(e.input_gamma, Some(2.2));
    }
}
