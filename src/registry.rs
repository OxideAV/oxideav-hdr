//! `oxideav-core` integration layer for `oxideav-hdr`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-hdr` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] — the unified `RuntimeContext` entry point
//!   `oxideav_meta::register_all` calls (via the `register!` macro).
//!   Internally calls [`register_codecs`] and [`register_containers`].
//! * [`register_codecs`] — the HDR codec (decoder + encoder) into a
//!   [`CodecRegistry`]; [`register_containers`] — the demuxer / muxer /
//!   `.hdr` + `.pic` extensions / probe into a [`ContainerRegistry`];
//!   [`register_registries`] — both at once.
//! * `From<HdrImage> for VideoFrame` and [`HdrImage::from_video_frame`]
//!   — the frame bridge: one packed `RgbF32Le` plane plus the
//!   colour-signal side-channel, the [`HdrPixelFormat`] ↔ `PixelFormat`
//!   mapping and [`ColorInfo`] ↔ `ColorSignal`.
//! * `From<HdrError> for oxideav_core::Error` and the
//!   `CodecOptionsStruct` schema for [`EncodeOptions`].
//!
//! The framework boundary is the **native float layout**: the registry
//! `Decoder` emits `RgbF32Le` frames (scene-referred linear light, no
//! tone mapping), so `oxideav-pixfmt` / `oxideav-image` decide how to
//! display them. The `Encoder` accepts `RgbF32Le` natively and `Rgb24`
//! / `Rgba` through the raw-path rule (`b / 255`, alpha dropped;
//! `input_gamma` option to linearise).

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecOptionsStruct, CodecParameters, CodecRegistry,
    ColorPrimaries, ColorSignal, ContainerRegistry, MatrixCoefficients, OptionField, OptionKind,
    OptionValue, PixelFormat, RuntimeContext, TransferCharacteristics, VideoFrame, VideoPlane,
};

use crate::container;
use crate::encoder::{LineEnding, RleMode};
use crate::error::HdrError;
use crate::header::Primaries;
use crate::image::{ColorInfo, ColorRange, HdrImage, HdrPixelFormat};
use crate::options::EncodeOptions;

/// Convert an [`HdrError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free decode/encode functions.
impl From<HdrError> for oxideav_core::Error {
    fn from(e: HdrError) -> Self {
        match e {
            HdrError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            HdrError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            HdrError::LimitExceeded(s) => oxideav_core::Error::ResourceExhausted(s),
            HdrError::Io(e) => oxideav_core::Error::Io(e),
        }
    }
}

// ---- pixel formats --------------------------------------------------------

/// The 1:1 name mapping from [`HdrPixelFormat`] to the framework enum.
pub fn to_core_pixel_format(pf: HdrPixelFormat) -> PixelFormat {
    match pf {
        HdrPixelFormat::RgbF32Le => PixelFormat::RgbF32Le,
    }
}

/// Map a framework pixel format to [`HdrPixelFormat`]; `Err` for every
/// layout but `RgbF32Le`.
pub fn from_core_pixel_format(pf: PixelFormat) -> Result<HdrPixelFormat, HdrError> {
    match pf {
        PixelFormat::RgbF32Le => Ok(HdrPixelFormat::RgbF32Le),
        other => Err(HdrError::unsupported(format!(
            "HDR: pixel format {other:?} not supported (RgbF32Le only)"
        ))),
    }
}

impl From<HdrPixelFormat> for PixelFormat {
    fn from(pf: HdrPixelFormat) -> Self {
        to_core_pixel_format(pf)
    }
}

impl TryFrom<PixelFormat> for HdrPixelFormat {
    type Error = HdrError;
    fn try_from(pf: PixelFormat) -> Result<Self, HdrError> {
        from_core_pixel_format(pf)
    }
}

// ---- colour signalling ----------------------------------------------------

/// [`ColorInfo`] as the framework's [`ColorSignal`] (code points map
/// 1:1).
pub fn to_color_signal(c: &ColorInfo) -> ColorSignal {
    let range = match c.range {
        ColorRange::Unspecified => oxideav_core::ColorRange::Unspecified,
        ColorRange::Limited => oxideav_core::ColorRange::Limited,
        ColorRange::Full => oxideav_core::ColorRange::Full,
    };
    ColorSignal::new(
        range,
        ColorPrimaries(c.primaries),
        TransferCharacteristics(c.transfer),
        MatrixCoefficients(c.matrix),
    )
}

/// The inverse of [`to_color_signal`].
pub fn from_color_signal(s: &ColorSignal) -> ColorInfo {
    let range = match s.range {
        oxideav_core::ColorRange::Limited => ColorRange::Limited,
        oxideav_core::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, s.primaries.0, s.transfer.0, s.matrix.0)
}

// ---- frame bridge ---------------------------------------------------------

/// [`HdrImage`] → `VideoFrame`, moving the plane out of the image: one
/// packed `RgbF32Le` plane plus the colour-signal side-channel. The
/// signal is always attached — a Radiance picture always carries colour
/// information (linear light by definition, plus `PRIMARIES=` /
/// `GAMMA=` when present).
pub(crate) fn image_into_video_frame(mut image: HdrImage, pts: Option<i64>) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    let mut frame = VideoFrame {
        pts,
        planes: vec![VideoPlane { stride, data }],
    };
    frame.set_color_signal(to_color_signal(&image.color));
    frame
}

impl From<HdrImage> for VideoFrame {
    /// The pixel plane (`pts` `None`) plus the colour-signal
    /// side-channel.
    fn from(image: HdrImage) -> Self {
        image_into_video_frame(image, None)
    }
}

impl From<&HdrImage> for VideoFrame {
    fn from(image: &HdrImage) -> Self {
        image_into_video_frame(image.clone(), None)
    }
}

impl HdrImage {
    /// Rebuild an image from a framework frame and the stream
    /// parameters that describe it: `width`, `height` and
    /// `pixel_format` are required. `RgbF32Le` frames become the plane
    /// as is (geometry validated by [`HdrImage::packed`]); `Rgb24` /
    /// `Rgba` frames are converted by the raw-path rule (`b / 255`,
    /// alpha dropped — see [`HdrImage::from_rgb8`]); any other layout is
    /// [`HdrError::Unsupported`]. The frame's colour-signal
    /// side-channel, refined over `params.color_signal`, becomes `color`
    /// when it specifies anything, and a BT.709 / BT.2020 / Display P3
    /// primaries code point is also written into `header.primaries` so
    /// the encoder emits the matching `PRIMARIES=` record.
    pub fn from_video_frame(
        frame: &VideoFrame,
        params: &CodecParameters,
    ) -> Result<Self, HdrError> {
        let width = params
            .width
            .ok_or_else(|| HdrError::invalid("HDR: width missing in CodecParameters"))?;
        let height = params
            .height
            .ok_or_else(|| HdrError::invalid("HDR: height missing in CodecParameters"))?;
        let format = params
            .pixel_format
            .ok_or_else(|| HdrError::invalid("HDR: pixel_format missing in CodecParameters"))?;
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| HdrError::invalid("HDR: frame has no planes"))?;
        let mut img = match format {
            PixelFormat::RgbF32Le => HdrImage::packed(
                width,
                height,
                HdrPixelFormat::RgbF32Le,
                plane.stride,
                plane.data.clone(),
            )?,
            PixelFormat::Rgb24 | PixelFormat::Rgba => {
                let bpp = if format == PixelFormat::Rgb24 { 3 } else { 4 };
                let row = width as usize * bpp;
                if plane.stride < row {
                    return Err(HdrError::invalid("HDR: frame stride below row size"));
                }
                let mut tight = Vec::with_capacity(row * height as usize);
                for y in 0..height as usize {
                    let start = y * plane.stride;
                    let src = plane
                        .data
                        .get(start..start + row)
                        .ok_or_else(|| HdrError::invalid("HDR: frame plane too short"))?;
                    tight.extend_from_slice(src);
                }
                HdrImage::from_8bit(width, height, &tight, bpp, None)?
            }
            other => {
                return Err(HdrError::unsupported(format!(
                    "HDR: pixel format {other:?} not supported (RgbF32Le / Rgb24 / Rgba)"
                )))
            }
        };
        // Per-frame record refines the stream-level description; an
        // entirely unspecified result keeps the header-derived default.
        let sig = frame
            .color_signal()
            .unwrap_or_default()
            .or(params.color_signal);
        if !sig.is_unspecified() {
            img.color = from_color_signal(&sig);
            img.header.primaries = match sig.primaries.0 {
                ColorInfo::PRIMARIES_BT709 => Some(Primaries::SRGB),
                ColorInfo::PRIMARIES_BT2020 => Some(Primaries::REC2020),
                ColorInfo::PRIMARIES_P3_D65 => Some(Primaries::P3_D65),
                _ => img.header.primaries,
            };
        }
        Ok(img)
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for HdrImage {
    type Error = HdrError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> Result<Self, HdrError> {
        HdrImage::from_video_frame(frame, params)
    }
}

// ---- CodecOptionsStruct (registry-only schema for EncodeOptions) ----------

/// The framework's options schema for the HDR encoder — what makes the
/// knobs discoverable to `oxideav list`, validatable by the pipeline's
/// JSON-options checker, and parsed with uniform error messages.
///
/// Keys: `rle` (`new` / `old` / `auto` / `uncompressed` / `smallest`),
/// `line_ending` (`lf` / `crlf`), `exposure` (f32, `EXPOSURE=`
/// override), `software` (string, `SOFTWARE=` override), `gamma` (f32,
/// `GAMMA=` override) and `input_gamma` (f32, linearisation exponent
/// for `Rgb24` / `Rgba` input frames).
impl CodecOptionsStruct for EncodeOptions {
    const SCHEMA: &'static [OptionField] = &[
        OptionField {
            name: "rle",
            kind: OptionKind::Enum(&["new", "old", "auto", "uncompressed", "smallest"]),
            default: OptionValue::String(String::new()),
            help: "Scanline RLE flavour: auto (default: new when the width is \
                   in 8..=32767, else old), new, old (pre-1991 sentinel \
                   runs), uncompressed (flat quads), smallest (per-scanline \
                   new vs flat).",
        },
        OptionField {
            name: "line_ending",
            kind: OptionKind::Enum(&["lf", "crlf"]),
            default: OptionValue::String(String::new()),
            help: "Text-header line terminator: lf (default) or crlf.",
        },
        OptionField {
            name: "exposure",
            kind: OptionKind::F32,
            default: OptionValue::F32(1.0),
            help: "EXPOSURE= record to write (overrides the image header).",
        },
        OptionField {
            name: "software",
            kind: OptionKind::String,
            default: OptionValue::String(String::new()),
            help: "SOFTWARE= record to write (overrides the image header).",
        },
        OptionField {
            name: "gamma",
            kind: OptionKind::F32,
            default: OptionValue::F32(1.0),
            help: "GAMMA= record to write (declares the stored values \
                   gamma-encoded; does not transform them).",
        },
        OptionField {
            name: "input_gamma",
            kind: OptionKind::F32,
            default: OptionValue::F32(1.0),
            help: "Linearisation exponent for 8-bit Rgb24 / Rgba input \
                   frames: value = (byte / 255) ^ input_gamma. Absent: \
                   bytes are treated as linear.",
        },
    ];

    fn apply(&mut self, key: &str, value: &OptionValue) -> oxideav_core::Result<()> {
        match key {
            "rle" => {
                self.rle = match value.as_str()? {
                    "new" => RleMode::New,
                    "old" => RleMode::Old,
                    "auto" => RleMode::Auto,
                    "uncompressed" => RleMode::Uncompressed,
                    "smallest" => RleMode::Smallest,
                    other => {
                        return Err(oxideav_core::Error::invalid(format!(
                            "HDR encoder: invalid rle {other:?}"
                        )))
                    }
                }
            }
            "line_ending" => {
                self.line_ending = match value.as_str()? {
                    "lf" => LineEnding::Lf,
                    "crlf" => LineEnding::Crlf,
                    other => {
                        return Err(oxideav_core::Error::invalid(format!(
                            "HDR encoder: invalid line_ending {other:?}"
                        )))
                    }
                }
            }
            "exposure" => self.exposure = Some(value.as_f32()?),
            "software" => self.software = Some(value.as_str()?.to_owned()),
            "gamma" => self.gamma = Some(value.as_f32()?),
            "input_gamma" => self.input_gamma = Some(value.as_f32()?),
            // Unreachable: parse_options rejects unknown keys against
            // SCHEMA before apply runs.
            other => {
                return Err(oxideav_core::Error::invalid(format!(
                    "HDR encoder: unknown option {other:?}"
                )))
            }
        }
        Ok(())
    }
}

// ---- registration ---------------------------------------------------------

/// Register the HDR codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("hdr_sw")
        .with_intra_only(true)
        .with_lossless(false)
        .with_max_size(32767, 32767)
        .with_pixel_formats(vec![
            PixelFormat::RgbF32Le,
            PixelFormat::Rgb24,
            PixelFormat::Rgba,
        ]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::decoder::make_decoder)
            .encoder(crate::encoder::make_encoder)
            .encoder_options::<EncodeOptions>(),
    );
}

/// Register the HDR container demuxer + muxer + extensions + probe
/// into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Register codecs and containers into two separate registries (the
/// pre-contract two-argument `register`).
pub fn register_registries(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

/// Unified entry point: install every codec and container provided by
/// `oxideav-hdr` into a [`RuntimeContext`]. Also wired into
/// `oxideav_meta::register_all` via the [`oxideav_core::register!`]
/// macro below.
pub fn register(ctx: &mut RuntimeContext) {
    register_registries(&mut ctx.codecs, &mut ctx.containers);
}

/// The pre-contract name of [`register`].
#[deprecated(note = "use oxideav_hdr::register (IMAGE_CRATE_API)")]
pub fn register_runtime(ctx: &mut RuntimeContext) {
    register(ctx);
}

oxideav_core::register!("hdr", register);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecodeOptions, HdrHeader, PixelFormat as HdrPf};
    use oxideav_core::{Frame, Packet, TimeBase};

    fn gridded(w: u32, h: u32) -> HdrImage {
        let quads: Vec<[u8; 4]> = (0..w * h)
            .map(|i| [128 + (i % 128) as u8, 64, 32, 120 + (i % 10) as u8])
            .collect();
        HdrImage::from_rgbe_quads(w, h, &quads, HdrHeader::default()).unwrap()
    }

    #[test]
    fn oxideav_entry_installs_codec_and_container() {
        let mut ctx = RuntimeContext::new();
        __oxideav_entry(&mut ctx);
        assert!(
            ctx.codecs.decoder_ids().next().is_some(),
            "__oxideav_entry should install codec decoder factories"
        );
        assert_eq!(ctx.containers.container_for_extension("hdr"), Some("hdr"));
        assert_eq!(ctx.containers.container_for_extension("pic"), Some("hdr"));
        let mut ctx2 = RuntimeContext::new();
        register(&mut ctx2);
        assert!(ctx2.codecs.decoder_ids().next().is_some());
    }

    #[test]
    fn frame_bridge_round_trips_native_float() {
        let mut img = gridded(8, 2);
        img.header.primaries = Some(Primaries::SRGB);
        img.sync_color_from_header();
        let frame = VideoFrame::from(img.clone());
        assert_eq!(frame.image_planes().len(), 1);
        assert_eq!(frame.image_planes()[0].stride, 8 * 12);
        assert_eq!(frame.image_planes()[0].data, img.as_bytes().unwrap());
        let sig = frame.color_signal().expect("signal always stamped");
        assert_eq!(sig.primaries.0, ColorInfo::PRIMARIES_BT709);
        assert_eq!(sig.transfer.0, ColorInfo::TRANSFER_LINEAR);
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(8);
        params.height = Some(2);
        params.pixel_format = Some(PixelFormat::RgbF32Le);
        let back = HdrImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(back.as_bytes(), img.as_bytes());
        assert_eq!(back.color, img.color);
        assert_eq!(back.header.primaries, Some(Primaries::SRGB));
        let back2 = HdrImage::try_from((&frame, &params)).unwrap();
        assert_eq!(back2, back);
        // Unsupported layouts and missing params are crate errors.
        params.pixel_format = Some(PixelFormat::Yuv420P);
        assert!(matches!(
            HdrImage::from_video_frame(&frame, &params),
            Err(HdrError::Unsupported(_))
        ));
        params.pixel_format = None;
        assert!(matches!(
            HdrImage::from_video_frame(&frame, &params),
            Err(HdrError::InvalidData(_))
        ));
        assert_eq!(PixelFormat::from(HdrPf::RgbF32Le), PixelFormat::RgbF32Le);
        assert!(HdrPf::try_from(PixelFormat::Rgb24).is_err());
    }

    #[test]
    fn frame_bridge_converts_8bit_frames() {
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 8,
                data: vec![0, 128, 255, 9, 255, 0, 64, 9],
            }],
        };
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(2);
        params.height = Some(1);
        params.pixel_format = Some(PixelFormat::Rgba);
        let img = HdrImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(img.to_rgb8(), vec![0, 128, 255, 255, 0, 64]);
        // Padded stride on Rgb24.
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 8,
                data: vec![1, 2, 3, 4, 5, 6, 0, 0],
            }],
        };
        params.pixel_format = Some(PixelFormat::Rgb24);
        let img = HdrImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(img.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn framework_decoder_emits_native_float_and_encoder_accepts_it() {
        let img = gridded(16, 4);
        let bytes = crate::encode(&img, &EncodeOptions::default()).unwrap();
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(16);
        params.height = Some(4);
        params.pixel_format = Some(PixelFormat::RgbF32Le);
        let mut dec = crate::decoder::make_decoder(&params).unwrap();
        let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes.clone());
        pkt.pts = Some(7);
        dec.send_packet(&pkt).unwrap();
        let Frame::Video(vf) = dec.receive_frame().unwrap() else {
            panic!("expected a video frame");
        };
        assert_eq!(vf.pts, Some(7));
        assert_eq!(vf.image_planes()[0].data, img.as_bytes().unwrap());
        assert!(vf.color_signal().is_some());
        dec.flush().unwrap();
        assert!(matches!(dec.receive_frame(), Err(oxideav_core::Error::Eof)));

        params.options.insert("rle", "uncompressed");
        params.options.insert("software", "bridge-test");
        let mut enc = crate::encoder::make_encoder(&params).unwrap();
        enc.send_frame(&Frame::Video(vf)).unwrap();
        let out = enc.receive_packet().unwrap();
        let back = crate::decode_with(
            &out.data,
            &DecodeOptions::default().with_fallback(crate::FallbackMode::Uncompressed),
        )
        .unwrap();
        assert_eq!(back.as_bytes(), img.as_bytes());
        assert_eq!(back.header.software.as_deref(), Some("bridge-test"));
        // Unknown option keys are rejected at construction.
        params.options.insert("bogus", "1");
        assert!(crate::encoder::make_encoder(&params).is_err());
    }

    #[test]
    fn error_mapping_keeps_variants() {
        let e: oxideav_core::Error = HdrError::limit("x").into();
        assert!(matches!(e, oxideav_core::Error::ResourceExhausted(_)));
        let e: oxideav_core::Error = HdrError::Io(std::io::Error::other("y")).into();
        assert!(matches!(e, oxideav_core::Error::Io(_)));
        let e: oxideav_core::Error = HdrError::unsupported("z").into();
        assert!(matches!(e, oxideav_core::Error::Unsupported(_)));
    }
}
