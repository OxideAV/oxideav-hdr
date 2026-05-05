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

pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("hdr", open_demuxer);
    reg.register_muxer("hdr", open_muxer);
    reg.register_extension("hdr", "hdr");
    reg.register_extension("pic", "hdr"); // Radiance's original extension
    reg.register_probe("hdr", probe);
}

fn probe(data: &ProbeData) -> ProbeScore {
    if data.buf.starts_with(b"#?RADIANCE") || data.buf.starts_with(b"#?RGBE") {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("hdr") | Some("pic")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    if !(buf.starts_with(b"#?RADIANCE") || buf.starts_with(b"#?RGBE")) {
        return Err(Error::invalid("HDR: missing #?RADIANCE / #?RGBE magic"));
    }
    // Pull width/height out of the header so the StreamInfo carries
    // accurate metadata without having to fully decode the pixel
    // array.
    let (width, height) = peek_dimensions(&buf).unwrap_or((0, 0));
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(width);
    params.height = Some(height);
    params.pixel_format = Some(PixelFormat::Rgb24);
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

/// Best-effort width/height read from the header without invoking the
/// full decoder. Returns `None` for malformed inputs — the caller
/// falls back to `(0, 0)` and the actual decode reports the error.
fn peek_dimensions(buf: &[u8]) -> Option<(u32, u32)> {
    // Find the empty line that ends the KEY=VALUE block, then parse
    // the next line as the resolution line.
    let mut i = 0;
    let mut prev_was_lf = false;
    while i < buf.len() {
        if buf[i] == b'\n' {
            if prev_was_lf {
                // Empty line — i is the position of the second \n.
                let res_start = i + 1;
                let nl = buf[res_start..].iter().position(|&b| b == b'\n')?;
                let line = &buf[res_start..res_start + nl];
                let s = std::str::from_utf8(line).ok()?;
                let toks: Vec<&str> = s.split_whitespace().collect();
                if toks.len() != 4 {
                    return None;
                }
                // Find which token is X and which is Y.
                let mut x = None;
                let mut y = None;
                for pair in toks.chunks(2) {
                    let flag = pair.first()?;
                    let val: u32 = pair.get(1)?.parse().ok()?;
                    if flag.ends_with('X') {
                        x = Some(val);
                    } else if flag.ends_with('Y') {
                        y = Some(val);
                    }
                }
                return Some((x?, y?));
            }
            prev_was_lf = true;
        } else if buf[i] == b'\r' {
            // ignore — CRLF tolerated
        } else {
            prev_was_lf = false;
        }
        i += 1;
    }
    None
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
