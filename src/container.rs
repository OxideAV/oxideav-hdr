//! HDR container: one Radiance file = one [`Packet`] on stream `0`.
//! Matches how the other single-image codecs in the workspace
//! (`oxideav-bmp`, `oxideav-png` for non-APNG, `oxideav-pbm`) plug
//! into the container pipeline.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, Error, MediaType, Packet, PixelFormat, Result,
    StreamInfo, TimeBase,
};
use oxideav_core::{
    ContainerRegistry, Demuxer, Muxer, ProbeData, ProbeScore, ReadSeek, WriteSeek, MAX_PROBE_SCORE,
};

/// Register the demuxer, muxer, `.hdr` / `.pic` extensions and the
/// magic-line probe under the container name `"hdr"`.
pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("hdr", open_demuxer);
    reg.register_muxer("hdr", open_muxer);
    reg.register_extension("hdr", "hdr");
    reg.register_extension("pic", "hdr"); // Radiance's original extension
    reg.register_probe("hdr", probe);
}

fn probe(data: &ProbeData) -> ProbeScore {
    if crate::probe(data.buf) {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("hdr") | Some("pic")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

/// Open a Radiance picture as a one-packet video stream. The stream's
/// `CodecParameters` carry the display dimensions from the resolution
/// line and the native `RgbF32Le` layout; the colour signal derived
/// from the header is attached when it specifies primaries.
pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    if !buf.starts_with(b"#?") {
        return Err(Error::invalid("HDR: missing #? magic line"));
    }
    // Pull width/height (and the colour signal) out of the header so
    // the StreamInfo carries accurate metadata without decoding the
    // pixel array. A malformed header is reported by the decoder.
    let info = crate::info(&buf).ok();
    let (width, height) = info.as_ref().map(|i| (i.width, i.height)).unwrap_or((0, 0));
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(width);
    params.height = Some(height);
    params.pixel_format = Some(PixelFormat::RgbF32Le);
    if let Some(i) = &info {
        params.color_signal = crate::registry::to_color_signal(&i.color);
    }
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: TimeBase::new(1, 1),
        start_time: Some(0),
        duration: None,
    };
    Ok(Box::new(HdrDemuxer {
        streams: vec![stream],
        data: Some(buf),
    }))
}

struct HdrDemuxer {
    streams: Vec<StreamInfo>,
    /// `None` once the sole packet has been emitted.
    data: Option<Vec<u8>>,
}

impl Demuxer for HdrDemuxer {
    fn format_name(&self) -> &str {
        "hdr"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.data.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => Err(Error::Eof),
        }
    }
}

/// Open a muxer that writes the single encoded packet verbatim.
pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid(
            "HDR muxer: expected exactly one video stream",
        ));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("HDR muxer: stream must be video"));
    }
    Ok(Box::new(HdrMuxer { output }))
}

struct HdrMuxer {
    output: Box<dyn WriteSeek>,
}

impl Muxer for HdrMuxer {
    fn format_name(&self) -> &str {
        "hdr"
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        self.output.write_all(&packet.data)?;
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        Ok(())
    }
}
