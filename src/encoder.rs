//! Radiance HDR top-level encode: emit the magic line, the
//! `KEY=VALUE` header (FORMAT first, then anything the caller put in
//! [`HdrHeader::other`]), the resolution line, and the new-RLE pixel
//! rows.
//!
//! Always writes top-down rows (`-Y H +X W`) regardless of the
//! `HdrHeader::y_sign` / `HdrHeader::x_sign` / `HdrHeader::x_first`
//! fields. Those exist on the round-trip side only — the encoder
//! ignores them so we never accidentally emit one of the rare
//! axis-flag combinations a careless decoder might mishandle.
//!
//! Width must be in the range `8..=32767` (the new-RLE marker can't
//! address rows outside that range). Encoding a wider image is
//! reported as `HdrError::Unsupported`; oxideav-hdr round 2+ will
//! grow a per-row plain-pixel fallback for the few hundred pixels
//! per side wide enough to need it.

use crate::error::{HdrError as Error, Result};
use crate::header::HdrHeader;
use crate::image::{HdrImage, HdrPixelFormat};
use crate::rgbe::rgb_to_rgbe;
use crate::rle::encode_scanline;

#[cfg(feature = "registry")]
use oxideav_core::Encoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, TimeBase};

/// Factory registered with the codec registry.
#[cfg(feature = "registry")]
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    Ok(Box::new(HdrEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct HdrEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Encoder for HdrEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "HDR encoder: expected video frame",
                ))
            }
        };
        let format = self.out_params.pixel_format.ok_or_else(|| {
            oxideav_core::Error::invalid("HDR encoder: pixel_format missing in CodecParameters")
        })?;
        let width = self.out_params.width.ok_or_else(|| {
            oxideav_core::Error::invalid("HDR encoder: width missing in CodecParameters")
        })?;
        let height = self.out_params.height.ok_or_else(|| {
            oxideav_core::Error::invalid("HDR encoder: height missing in CodecParameters")
        })?;
        if vf.planes.is_empty() {
            return Err(oxideav_core::Error::invalid(
                "HDR encoder: empty frame plane",
            ));
        }
        let bytes_per_pixel = match format {
            PixelFormat::Rgb24 => 3usize,
            PixelFormat::Rgba => 4,
            other => {
                return Err(oxideav_core::Error::invalid(format!(
                    "HDR encoder: unsupported pixel format {other:?}"
                )))
            }
        };
        // Convert the LDR plane to f32 in the [0, 1] range so the
        // shared-exponent encoder has something sensible to compress.
        let n = (width as usize) * (height as usize);
        let mut pixels = Vec::with_capacity(n * 3);
        let stride = vf.planes[0].stride;
        for y in 0..height as usize {
            let row = &vf.planes[0].data[y * stride..y * stride + width as usize * bytes_per_pixel];
            for x in 0..width as usize {
                let off = x * bytes_per_pixel;
                pixels.push(row[off] as f32 / 255.0);
                pixels.push(row[off + 1] as f32 / 255.0);
                pixels.push(row[off + 2] as f32 / 255.0);
            }
        }
        let img = HdrImage {
            width,
            height,
            pixel_format: HdrPixelFormat::Rgb96f,
            pixels,
            header: HdrHeader::default(),
        };
        let bytes = encode_hdr(&img)?;
        self.pending = Some(bytes);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public standalone API
// ---------------------------------------------------------------------------

/// Encode an [`HdrImage`] into a complete HDR file (magic line +
/// `KEY=VALUE` header + resolution line + new-RLE pixel rows).
pub fn encode_hdr(image: &HdrImage) -> Result<Vec<u8>> {
    let w = image.width as usize;
    let h = image.height as usize;
    if image.pixels.len() != w * h * 3 {
        return Err(Error::invalid(
            "HDR encoder: pixels length doesn't match width*height*3",
        ));
    }
    if !(8..=32767).contains(&w) {
        return Err(Error::unsupported(format!(
            "HDR encoder: width {w} outside supported new-RLE range 8..=32767"
        )));
    }
    let mut out = Vec::with_capacity(32 + w * h * 4);
    write_header(&mut out, &image.header);
    write_resolution(&mut out, w, h);
    write_pixel_rows(&mut out, w, h, &image.pixels)?;
    Ok(out)
}

/// Convenience wrapper that builds an [`HdrImage`] from raw float
/// data and the supplied header, then defers to [`encode_hdr`].
pub fn encode_hdr_rgb96f(
    width: u32,
    height: u32,
    pixels: Vec<f32>,
    header: HdrHeader,
) -> Result<Vec<u8>> {
    let img = HdrImage {
        width,
        height,
        pixel_format: HdrPixelFormat::Rgb96f,
        pixels,
        header,
    };
    encode_hdr(&img)
}

fn write_header(out: &mut Vec<u8>, header: &HdrHeader) {
    out.extend_from_slice(b"#?RADIANCE\n");
    out.extend_from_slice(format!("FORMAT={}\n", header.format.as_str()).as_bytes());
    if let Some(g) = header.gamma {
        out.extend_from_slice(format!("GAMMA={g}\n").as_bytes());
    }
    if let Some(e) = header.exposure {
        out.extend_from_slice(format!("EXPOSURE={e}\n").as_bytes());
    }
    if let Some(p) = header.pixaspect {
        out.extend_from_slice(format!("PIXASPECT={p}\n").as_bytes());
    }
    if let Some(s) = &header.software {
        out.extend_from_slice(format!("SOFTWARE={s}\n").as_bytes());
    }
    for (k, v) in &header.other {
        // Keep arbitrary records the caller stashed earlier.
        out.extend_from_slice(format!("{k}={v}\n").as_bytes());
    }
    for c in &header.comments {
        out.extend_from_slice(format!("#{c}\n").as_bytes());
    }
    // Empty line terminates the header.
    out.push(b'\n');
}

fn write_resolution(out: &mut Vec<u8>, width: usize, height: usize) {
    // Always emit the canonical `-Y H +X W` form.
    out.extend_from_slice(format!("-Y {height} +X {width}\n").as_bytes());
}

fn write_pixel_rows(out: &mut Vec<u8>, width: usize, height: usize, pixels: &[f32]) -> Result<()> {
    // For each scanline, build the four channel buffers from the
    // shared-exponent pixel encoding then RLE-code them.
    let mut channels: [Vec<u8>; 4] = [
        vec![0u8; width],
        vec![0u8; width],
        vec![0u8; width],
        vec![0u8; width],
    ];
    for y in 0..height {
        let row = &pixels[y * width * 3..(y + 1) * width * 3];
        for (x, px) in row.chunks_exact(3).enumerate() {
            let rgbe = rgb_to_rgbe([px[0], px[1], px[2]]);
            channels[0][x] = rgbe[0];
            channels[1][x] = rgbe[1];
            channels[2][x] = rgbe[2];
            channels[3][x] = rgbe[3];
        }
        encode_scanline(&channels, width, out)?;
    }
    Ok(())
}
