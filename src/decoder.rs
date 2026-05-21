//! Radiance HDR top-level decode: read the magic line, the
//! `KEY=VALUE` header, the resolution line, and the pixel rows.
//!
//! Output is always packed `Rgb96f` in top-down memory order
//! (`width * height * 3` floats). The on-disk axis flags are honoured
//! by reordering the rows / mirroring within each row at the end of
//! decode so the consumer doesn't have to know about them.
//!
//! With the default `registry` feature on, the gated `HdrDecoder`
//! trait impl wraps [`parse_hdr`] for the `oxideav_core::Decoder`
//! surface and tone-maps each pixel into Rgb24 at the boundary.

use crate::error::{HdrError as Error, Result};
use crate::header::{AxisSign, HdrFormat, HdrHeader, Primaries};
use crate::image::{HdrImage, HdrPixelFormat};
use crate::rgbe::rgbe_to_rgb;
use crate::rle::decode_scanline;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. Consumes one packet per
/// whole HDR file and produces one float-RGB frame.
#[cfg(feature = "registry")]
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(HdrDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct HdrDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Decoder for HdrDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_hdr(&packet.data)?;
        self.pending = Some(image_to_video_frame(image));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
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

#[cfg(feature = "registry")]
fn image_to_video_frame(image: HdrImage) -> VideoFrame {
    // Tone-map to 8-bit Rgb24 at the framework boundary so the
    // generic VideoFrame stays representable. The standalone API
    // keeps the f32 channels.
    let n = (image.width as usize) * (image.height as usize);
    let mut data = Vec::with_capacity(n * 3);
    let gamma = image.header.gamma.unwrap_or(2.2);
    let exposure = image.header.exposure.unwrap_or(1.0);
    for i in 0..n {
        for c in 0..3 {
            let v = image.pixels[i * 3 + c] * exposure;
            let g = if v <= 0.0 { 0.0 } else { v.powf(1.0 / gamma) };
            data.push((g.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    VideoFrame {
        pts: None,
        planes: vec![VideoPlane {
            stride: image.width as usize * 3,
            data,
        }],
    }
}

// ---------------------------------------------------------------------------
// Public standalone API
// ---------------------------------------------------------------------------

/// Decode a complete HDR file (magic line + `KEY=VALUE` header +
/// resolution line + pixel rows) into an [`HdrImage`] tagged
/// [`HdrPixelFormat::Rgb96f`], top-down.
pub fn parse_hdr(input: &[u8]) -> Result<HdrImage> {
    let mut cursor = 0usize;
    let mut header = parse_header(input, &mut cursor)?;
    let (width, height) = parse_resolution(input, &mut cursor, &mut header)?;
    // Resolution line lists the *outer* axis first, then the inner axis.
    // For Y-first files (`±Y H ±X W`) that's H scanlines of W pixels.
    // For X-first files (`±X W ±Y H`) it's W scanlines of H pixels —
    // each scanline is one column's worth of Y samples.
    let (scanline_count, scanline_len) = if header.x_first {
        (width, height)
    } else {
        (height, width)
    };
    let pixels = decode_pixel_rows(input, &mut cursor, scanline_len, scanline_count)?;
    let pixels = reorder_for_axis_flags(pixels, width, height, &header);
    Ok(HdrImage {
        width: width as u32,
        height: height as u32,
        pixel_format: HdrPixelFormat::Rgb96f,
        pixels,
        header,
    })
}

fn parse_header(input: &[u8], cursor: &mut usize) -> Result<HdrHeader> {
    // First line: magic.
    let magic = read_line(input, cursor).ok_or_else(|| Error::invalid("HDR: missing magic"))?;
    let trimmed = trim_cr(magic);
    if trimmed != b"#?RADIANCE" && trimmed != b"#?RGBE" {
        return Err(Error::invalid(
            "HDR: missing #?RADIANCE / #?RGBE magic line",
        ));
    }
    let mut header = HdrHeader::default();
    loop {
        let line =
            read_line(input, cursor).ok_or_else(|| Error::invalid("HDR: header truncated"))?;
        let line = trim_cr(line);
        if line.is_empty() {
            break;
        }
        if line.starts_with(b"#") {
            // Comment line — keep without the leading '#'.
            let comment = std::str::from_utf8(&line[1..])
                .map_err(|_| Error::invalid("HDR: non-UTF8 comment line"))?;
            header.comments.push(comment.to_owned());
            continue;
        }
        // KEY=VALUE record.
        let eq = line
            .iter()
            .position(|&b| b == b'=')
            .ok_or_else(|| Error::invalid("HDR: header line without '='"))?;
        let key = std::str::from_utf8(&line[..eq])
            .map_err(|_| Error::invalid("HDR: non-UTF8 header key"))?;
        let value = std::str::from_utf8(&line[eq + 1..])
            .map_err(|_| Error::invalid("HDR: non-UTF8 header value"))?;
        match key {
            "FORMAT" => {
                header.format = match value {
                    "32-bit_rle_rgbe" => HdrFormat::Rgbe,
                    "32-bit_rle_xyze" => HdrFormat::Xyze,
                    other => {
                        return Err(Error::unsupported(format!("HDR: unknown FORMAT '{other}'")))
                    }
                };
            }
            "EXPOSURE" => {
                // Per the Radiance reference manual, multiple EXPOSURE
                // records stack multiplicatively (each one represents an
                // additional exposure-adjustment pass applied to the
                // already-encoded radiance values). Accumulate the
                // product across all occurrences.
                let v = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Error::invalid("HDR: invalid EXPOSURE"))?;
                header.exposure = Some(match header.exposure {
                    Some(prev) => prev * v,
                    None => v,
                });
            }
            "GAMMA" => {
                header.gamma = Some(
                    value
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| Error::invalid("HDR: invalid GAMMA"))?,
                );
            }
            "PIXASPECT" => {
                header.pixaspect = Some(
                    value
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| Error::invalid("HDR: invalid PIXASPECT"))?,
                );
            }
            "SOFTWARE" => {
                header.software = Some(value.to_owned());
            }
            "VIEW" => {
                // Per the Radiance reference manual, the VIEW record is
                // free-form text containing the renderer's view
                // parameters. We preserve the literal value. When more
                // than one VIEW record appears (renderers can stack
                // them across rerender passes), each subsequent one
                // wins — the reference convention is "the last VIEW=
                // record on the page describes the present picture".
                header.view = Some(value.to_owned());
            }
            "COLORCORR" => {
                let parts: Vec<&str> = value.split_whitespace().collect();
                if parts.len() != 3 {
                    return Err(Error::invalid("HDR: COLORCORR must have 3 floats"));
                }
                let r: f32 = parts[0]
                    .parse()
                    .map_err(|_| Error::invalid("HDR: invalid COLORCORR red"))?;
                let g: f32 = parts[1]
                    .parse()
                    .map_err(|_| Error::invalid("HDR: invalid COLORCORR green"))?;
                let b: f32 = parts[2]
                    .parse()
                    .map_err(|_| Error::invalid("HDR: invalid COLORCORR blue"))?;
                // Per the Radiance reference manual, COLORCORR records
                // stack multiplicatively in the same way as EXPOSURE.
                header.colorcorr = Some(match header.colorcorr {
                    Some([pr, pg, pb]) => [pr * r, pg * g, pb * b],
                    None => [r, g, b],
                });
            }
            "PRIMARIES" => {
                header.primaries = Some(
                    Primaries::from_record_str(value)
                        .ok_or_else(|| Error::invalid("HDR: PRIMARIES must have 8 floats"))?,
                );
            }
            _ => {
                header.other.push((key.to_owned(), value.to_owned()));
            }
        }
    }
    Ok(header)
}

fn parse_resolution(
    input: &[u8],
    cursor: &mut usize,
    header: &mut HdrHeader,
) -> Result<(usize, usize)> {
    let line =
        read_line(input, cursor).ok_or_else(|| Error::invalid("HDR: missing resolution line"))?;
    let line = trim_cr(line);
    let s =
        std::str::from_utf8(line).map_err(|_| Error::invalid("HDR: non-UTF8 resolution line"))?;
    // Eight legal forms — split, take the first axis flag + value pair
    // and the second.
    let toks: Vec<&str> = s.split_whitespace().collect();
    if toks.len() != 4 {
        return Err(Error::invalid("HDR: resolution line must have 4 tokens"));
    }
    let (a_flag, a_val, b_flag, b_val) = (toks[0], toks[1], toks[2], toks[3]);
    let (a_axis, a_sign) = parse_axis_flag(a_flag)?;
    let (b_axis, b_sign) = parse_axis_flag(b_flag)?;
    if a_axis == b_axis {
        return Err(Error::invalid(
            "HDR: resolution line must have one X and one Y flag",
        ));
    }
    let a_n: usize = a_val
        .parse()
        .map_err(|_| Error::invalid("HDR: invalid resolution value"))?;
    let b_n: usize = b_val
        .parse()
        .map_err(|_| Error::invalid("HDR: invalid resolution value"))?;
    let (width, height, x_first, y_sign, x_sign);
    if a_axis == 'Y' {
        height = a_n;
        y_sign = a_sign;
        width = b_n;
        x_sign = b_sign;
        x_first = false;
    } else {
        width = a_n;
        x_sign = a_sign;
        height = b_n;
        y_sign = b_sign;
        x_first = true;
    }
    if width == 0 || height == 0 {
        return Err(Error::invalid("HDR: zero dimension in resolution line"));
    }
    header.x_sign = x_sign;
    header.y_sign = y_sign;
    header.x_first = x_first;
    Ok((width, height))
}

fn parse_axis_flag(flag: &str) -> Result<(char, AxisSign)> {
    let bytes = flag.as_bytes();
    if bytes.len() != 2 {
        return Err(Error::invalid("HDR: axis flag must be 2 chars"));
    }
    let sign = match bytes[0] {
        b'+' => AxisSign::Increasing,
        b'-' => AxisSign::Decreasing,
        _ => return Err(Error::invalid("HDR: axis flag sign must be + or -")),
    };
    let axis = match bytes[1] {
        b'X' => 'X',
        b'Y' => 'Y',
        _ => return Err(Error::invalid("HDR: axis flag axis must be X or Y")),
    };
    Ok((axis, sign))
}

fn decode_pixel_rows(
    input: &[u8],
    cursor: &mut usize,
    width: usize,
    height: usize,
) -> Result<Vec<f32>> {
    let mut pixels = vec![0.0f32; width * height * 3];
    let mut prev_pixel: Option<[u8; 4]> = None;
    for y in 0..height {
        let chans = decode_scanline(input, cursor, width, &mut prev_pixel)?;
        for (x, ch_r) in chans[0].iter().enumerate() {
            let rgbe = [*ch_r, chans[1][x], chans[2][x], chans[3][x]];
            let rgb = rgbe_to_rgb(rgbe);
            let off = (y * width + x) * 3;
            pixels[off] = rgb[0];
            pixels[off + 1] = rgb[1];
            pixels[off + 2] = rgb[2];
        }
    }
    Ok(pixels)
}

/// Reorder rows / mirror within rows so the output is always top-down,
/// left-to-right regardless of the on-disk axis flags.
///
/// `width` / `height` are the *image* dimensions (matching the caller-
/// visible `HdrImage`). For Y-first files the decoded buffer is already
/// in `(y, x)` row-major layout. For X-first files the decoded buffer
/// is in `(x, y)` order — each on-disk scanline holds one column's
/// worth of Y samples — so we transpose first to land in the canonical
/// `(y, x)` layout, then apply the axis-sign flips.
fn reorder_for_axis_flags(
    pixels: Vec<f32>,
    width: usize,
    height: usize,
    header: &HdrHeader,
) -> Vec<f32> {
    let mut out = pixels;
    if header.x_first {
        // Source layout: (x, y) row-major with `width` outer rows and
        // `height` inner cols. Transpose flips that to (y, x) row-major
        // with `height` outer rows and `width` inner cols.
        out = transpose(&out, height, width);
    }
    // After the optional transpose the in-memory layout is canonical
    // (y, x) with the caller-visible (width, height) dimensions.
    out = apply_axis_flips(out, width, height, header.x_sign, header.y_sign);
    out
}

fn transpose(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; pixels.len()];
    // Source layout: (y, x) at offset (y*width + x)*3.
    // Dest layout (transposed): (x, y) at offset (x*height + y)*3.
    // After transpose, dimensions become (new_w=height, new_h=width).
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let dst = (x * height + y) * 3;
            out[dst] = pixels[src];
            out[dst + 1] = pixels[src + 1];
            out[dst + 2] = pixels[src + 2];
        }
    }
    out
}

fn apply_axis_flips(
    pixels: Vec<f32>,
    width: usize,
    height: usize,
    x_sign: AxisSign,
    y_sign: AxisSign,
) -> Vec<f32> {
    let flip_y = matches!(y_sign, AxisSign::Increasing); // -Y is the standard top-down
    let flip_x = matches!(x_sign, AxisSign::Decreasing);
    if !flip_y && !flip_x {
        return pixels;
    }
    let mut out = vec![0.0f32; pixels.len()];
    for y in 0..height {
        let src_y = if flip_y { height - 1 - y } else { y };
        for x in 0..width {
            let src_x = if flip_x { width - 1 - x } else { x };
            let src = (src_y * width + src_x) * 3;
            let dst = (y * width + x) * 3;
            out[dst] = pixels[src];
            out[dst + 1] = pixels[src + 1];
            out[dst + 2] = pixels[src + 2];
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Line reader for the text header
// ---------------------------------------------------------------------------

/// Read up to (but not including) the next `\n`, advancing `cursor`
/// past the terminator. Returns `None` at EOF without a trailing
/// newline (header truncated).
fn read_line<'a>(input: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let start = *cursor;
    let rest = input.get(start..)?;
    let nl = rest.iter().position(|&b| b == b'\n')?;
    *cursor = start + nl + 1;
    Some(&input[start..start + nl])
}

/// Strip a single trailing `\r` if present (some writers use CRLF).
fn trim_cr(line: &[u8]) -> &[u8] {
    if let Some((&b'\r', rest)) = line.split_last() {
        rest
    } else {
        line
    }
}

/// Compatibility wrapper around [`parse_hdr`] returning an
/// `oxideav_core::VideoFrame`. Available with the default `registry`
/// feature.
#[cfg(feature = "registry")]
pub fn parse_hdr_videoframe(input: &[u8]) -> oxideav_core::Result<VideoFrame> {
    Ok(image_to_video_frame(parse_hdr(input)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_line_handles_lf_and_crlf() {
        let buf = b"foo\nbar\r\nbaz";
        let mut c = 0;
        assert_eq!(read_line(buf, &mut c), Some(&b"foo"[..]));
        let line2 = read_line(buf, &mut c).unwrap();
        assert_eq!(trim_cr(line2), b"bar");
        // No trailing newline on `baz` ⇒ None.
        assert_eq!(read_line(buf, &mut c), None);
    }

    #[test]
    fn parse_axis_flag_rejects_garbage() {
        assert!(parse_axis_flag("Y").is_err());
        assert!(parse_axis_flag("+Z").is_err());
        assert!(parse_axis_flag("/Y").is_err());
        assert_eq!(parse_axis_flag("-Y").unwrap(), ('Y', AxisSign::Decreasing));
    }

    #[test]
    fn multiple_exposure_records_stack_multiplicatively() {
        // Per the Radiance reference manual, EXPOSURE records stack
        // multiplicatively. Two records (0.5 and 0.25) should land at
        // 0.125 in the decoded header.
        let bytes = b"#?RADIANCE\nEXPOSURE=0.5\nEXPOSURE=0.25\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        let e = header.exposure.expect("EXPOSURE missing");
        assert!((e - 0.125).abs() < 1e-6, "expected 0.125, got {e}");
    }

    #[test]
    fn multiple_colorcorr_records_stack_multiplicatively() {
        // Same rule as EXPOSURE — element-wise product across records.
        let bytes = b"#?RADIANCE\nCOLORCORR=2.0 0.5 1.0\nCOLORCORR=0.5 0.5 2.0\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        let cc = header.colorcorr.expect("COLORCORR missing");
        assert!((cc[0] - 1.0).abs() < 1e-6, "R: expected 1.0, got {}", cc[0]);
        assert!(
            (cc[1] - 0.25).abs() < 1e-6,
            "G: expected 0.25, got {}",
            cc[1]
        );
        assert!((cc[2] - 2.0).abs() < 1e-6, "B: expected 2.0, got {}", cc[2]);
    }

    #[test]
    fn view_record_is_parsed_into_typed_slot() {
        let bytes =
            b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nVIEW=rvu -vp 0 0 10 -vd 0 0 -1\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.view.as_deref(), Some("rvu -vp 0 0 10 -vd 0 0 -1"));
        // The typed VIEW slot should NOT also leak into `other`.
        assert!(header.other.iter().all(|(k, _)| k != "VIEW"));
    }

    #[test]
    fn last_view_record_wins_when_stacked() {
        // Renderers can write multiple VIEW= records across rerender
        // passes. The reference convention is "the last VIEW record on
        // the page describes the present picture".
        let bytes = b"#?RADIANCE\nVIEW=rvu -vp 0 0 5\nVIEW=rvu -vp 0 0 10\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.view.as_deref(), Some("rvu -vp 0 0 10"));
    }

    #[test]
    fn single_exposure_record_is_passed_through() {
        // The stacking shouldn't break the single-record case.
        let bytes = b"#?RADIANCE\nEXPOSURE=1.5\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.exposure, Some(1.5));
    }
}
