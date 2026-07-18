//! `oxideav-core` integration layer for `oxideav-hdr`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-hdr` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The framework boundary tone-maps the f32 RGB channels to 8-bit
//! `Rgb24` (gamma + exposure applied) so the generic `VideoFrame`
//! representation stays usable; the float dynamic range is preserved
//! on the standalone API.

use oxideav_core::ContainerRegistry;
use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};
use oxideav_core::{CodecInfo, CodecRegistry};

use crate::container;
use crate::error::HdrError;

/// Convert an [`HdrError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free decode/encode functions.
impl From<HdrError> for oxideav_core::Error {
    fn from(e: HdrError) -> Self {
        match e {
            HdrError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            HdrError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            // The framework `Error` enum has no dedicated TooLarge
            // variant — fold it back into `InvalidData` so framework
            // callers see a single `Decoder` error class without losing
            // the diagnostic string. Standalone callers that care about
            // the distinction (e.g. distinguishing "this picture is too
            // big for our policy" from "this header is malformed") match
            // on the crate-local `HdrError::TooLarge` directly.
            HdrError::TooLarge(s) => oxideav_core::Error::InvalidData(s),
        }
    }
}

/// Register the HDR codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("hdr_sw")
        .with_intra_only(true)
        .with_lossless(false)
        .with_max_size(32767, 32767)
        .with_pixel_formats(vec![PixelFormat::Rgb24, PixelFormat::Rgba]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::decoder::make_decoder)
            .encoder(crate::encoder::make_encoder),
    );
}

/// Register the HDR container demuxer + muxer + extension + probe
/// into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Combined registration for callers that just want everything wired up
/// in one call.
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

/// Register codecs + containers into an `oxideav_core::RuntimeContext`
/// — the form `oxideav_meta::register_all` dispatches via the
/// [`oxideav_core::register!`] macro below. The two-registry
/// [`register`] above remains the direct API.
pub fn register_runtime(ctx: &mut oxideav_core::RuntimeContext) {
    register(&mut ctx.codecs, &mut ctx.containers);
}

oxideav_core::register!("hdr", register_runtime);

#[cfg(test)]
mod runtime_entry_tests {
    use super::*;

    #[test]
    fn oxideav_entry_installs_codec_and_container() {
        let mut ctx = oxideav_core::RuntimeContext::new();
        __oxideav_entry(&mut ctx);
        assert!(
            ctx.codecs.decoder_ids().next().is_some(),
            "__oxideav_entry should install codec decoder factories"
        );
        assert_eq!(
            ctx.containers.container_for_extension("hdr"),
            Some("hdr"),
            "__oxideav_entry should install the .hdr extension hint"
        );
    }
}
