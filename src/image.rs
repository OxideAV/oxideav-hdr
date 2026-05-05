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
}
