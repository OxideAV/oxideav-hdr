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
use crate::limits::HdrLimits;
use crate::rgbe::rgbe_to_rgb;
use crate::rle::{decode_scanline_with_fallback, FallbackMode};

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
/// [`HdrPixelFormat::Rgb96f`], top-down. Applies the default
/// [`HdrLimits`] (max 32 767 × 32 767, ≤ 256 MiB pixel buffer); for
/// trusted input that needs larger pictures use
/// [`parse_hdr_with_limits`] / [`parse_hdr_with_options_and_limits`].
pub fn parse_hdr(input: &[u8]) -> Result<HdrImage> {
    parse_hdr_with_options_and_limits(input, FallbackMode::OldRle, &HdrLimits::default())
}

/// Like [`parse_hdr`] but with a caller-chosen [`HdrLimits`].
///
/// Pass [`HdrLimits::unbounded`] for trusted local input that needs to
/// decode legitimately-huge pictures (the encoder's `Vec` capacity
/// still bounds the worst case via the allocator); see [`HdrLimits`]
/// for the field-by-field rationale of the defaults.
pub fn parse_hdr_with_limits(input: &[u8], limits: &HdrLimits) -> Result<HdrImage> {
    parse_hdr_with_options_and_limits(input, FallbackMode::OldRle, limits)
}

/// Decode a complete HDR file picking the non-new-RLE fallback per
/// `fallback`. See [`FallbackMode`] for the trade-off. Applies the
/// default [`HdrLimits`].
///
/// Use [`FallbackMode::Uncompressed`] for files written with
/// [`crate::encoder::RleMode::Uncompressed`] or any other flat-scanline
/// writer; use [`FallbackMode::OldRle`] (the default of [`parse_hdr`])
/// for pre-1991 sentinel-run files. The two modes diverge only when
/// the new-RLE marker is absent: with `OldRle`, `(1, 1, 1, *)` quads
/// are interpreted as run sentinels; with `Uncompressed`, every quad
/// is a literal RGBE pixel.
pub fn parse_hdr_with_options(input: &[u8], fallback: FallbackMode) -> Result<HdrImage> {
    parse_hdr_with_options_and_limits(input, fallback, &HdrLimits::default())
}

/// Full-control decode: pick the non-new-RLE fallback per `fallback`
/// AND the resource ceilings per `limits` independently.
///
/// The limits apply at the resolution-line stage — before the decoder
/// allocates the `width * height * 3` float pixel buffer — so a
/// malicious header is rejected at the door with
/// [`HdrError::TooLarge`](crate::HdrError::TooLarge) rather than
/// triggering an unbounded allocation.
pub fn parse_hdr_with_options_and_limits(
    input: &[u8],
    fallback: FallbackMode,
    limits: &HdrLimits,
) -> Result<HdrImage> {
    let mut cursor = 0usize;
    let mut header = parse_header(input, &mut cursor)?;
    let (width, height) = parse_resolution(input, &mut cursor, &mut header, limits)?;
    // Resolution line lists the *outer* axis first, then the inner axis.
    // For Y-first files (`±Y H ±X W`) that's H scanlines of W pixels.
    // For X-first files (`±X W ±Y H`) it's W scanlines of H pixels —
    // each scanline is one column's worth of Y samples.
    let (scanline_count, scanline_len) = if header.x_first {
        (width, height)
    } else {
        (height, width)
    };
    let pixels = decode_pixel_rows(input, &mut cursor, scanline_len, scanline_count, fallback)?;
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
    // Per the format spec, "at most one FORMAT line is allowed". A
    // second FORMAT record makes the picture invalid rather than the
    // last-wins overwrite a permissive parser would do — two distinct
    // pixel-format declarations leave the scanline section ambiguous.
    let mut format_seen = false;
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
                if format_seen {
                    return Err(Error::invalid("HDR: at most one FORMAT line is allowed"));
                }
                format_seen = true;
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
                // Per the Radiance reference manual, PIXASPECT is
                // cumulative — when multiple records appear the
                // pixel aspect ratio is the product of all of them.
                // The default when no PIXASPECT record is present is
                // 1.0 (square pixels); see HdrImage::effective_pixaspect.
                let v = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Error::invalid("HDR: invalid PIXASPECT"))?;
                header.pixaspect = Some(match header.pixaspect {
                    Some(prev) => prev * v,
                    None => v,
                });
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
    limits: &HdrLimits,
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
    // Apply the caller-configured resource limits BEFORE returning the
    // dimensions. The downstream pixel-buffer allocation is
    // `width * height * 3 * sizeof(f32)`; without these checks a
    // malicious header could either OOM the host or trigger a usize
    // overflow that wraps the allocation request to a tiny value and
    // sets up out-of-bounds writes later in the decode loop.
    if width > limits.max_width as usize {
        return Err(Error::too_large(format!(
            "HDR: resolution width {width} exceeds HdrLimits::max_width ({})",
            limits.max_width
        )));
    }
    if height > limits.max_height as usize {
        return Err(Error::too_large(format!(
            "HDR: resolution height {height} exceeds HdrLimits::max_height ({})",
            limits.max_height
        )));
    }
    // Pixel-buffer size in bytes: `width * height * 3 * 4`. Use
    // `checked_mul` so a hostile combination that still slips past the
    // per-axis caps (e.g. when the caller relaxes the dimension caps
    // but keeps `max_pixel_bytes` tight) is rejected at the arithmetic
    // level rather than wrapping.
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| Error::too_large("HDR: width × height overflows usize"))?;
    let buf_bytes = pixel_count
        .checked_mul(12)
        .ok_or_else(|| Error::too_large("HDR: pixel-buffer size overflows usize"))?;
    if buf_bytes > limits.max_pixel_bytes {
        return Err(Error::too_large(format!(
            "HDR: pixel-buffer size {buf_bytes} bytes exceeds HdrLimits::max_pixel_bytes ({})",
            limits.max_pixel_bytes
        )));
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
    fallback: FallbackMode,
) -> Result<Vec<f32>> {
    // Defensive guard: `parse_resolution` already applies HdrLimits +
    // checked_mul on the same product, but `decode_pixel_rows` is
    // pub(crate) and would be reachable from a future test / helper
    // that bypasses the resolution-line gate. Keep the overflow check
    // here so the float-count multiplication never wraps.
    let float_count = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| Error::too_large("HDR: width × height × 3 overflows usize"))?;
    let mut pixels = vec![0.0f32; float_count];
    let mut prev_pixel: Option<[u8; 4]> = None;
    for y in 0..height {
        let chans = decode_scanline_with_fallback(input, cursor, width, &mut prev_pixel, fallback)?;
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
    fn multiple_pixaspect_records_stack_multiplicatively() {
        // Per the Radiance reference manual, PIXASPECT is cumulative:
        // when multiple records appear the *effective* aspect ratio is
        // their product. Three records (2.0, 0.5, 1.25) should land at
        // 1.25 in the decoded header.
        let bytes = b"#?RADIANCE\nPIXASPECT=2.0\nPIXASPECT=0.5\nPIXASPECT=1.25\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        let p = header.pixaspect.expect("PIXASPECT missing");
        assert!((p - 1.25).abs() < 1e-6, "expected 1.25, got {p}");
    }

    #[test]
    fn single_pixaspect_record_is_passed_through() {
        // The cumulative stacking must not perturb the single-record
        // case (the round 1..207 happy path).
        let bytes = b"#?RADIANCE\nPIXASPECT=0.75\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.pixaspect, Some(0.75));
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
    fn duplicate_format_record_is_rejected() {
        // The format spec mandates "at most one FORMAT line". A second
        // FORMAT record (even with the same value) makes the picture
        // invalid — the parser must not silently last-wins it.
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nFORMAT=32-bit_rle_xyze\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let err = parse_header(bytes, &mut cursor).expect_err("two FORMAT lines must error");
        assert!(
            err.to_string().contains("at most one FORMAT"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_format_record_rejected_even_when_identical() {
        // Two FORMAT lines with the *same* value are still invalid per
        // spec — the "at most one" rule is structural, not value-based.
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        assert!(parse_header(bytes, &mut cursor).is_err());
    }

    #[test]
    fn single_format_record_is_accepted() {
        // The duplicate-FORMAT guard must not perturb the happy path.
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.format, HdrFormat::Xyze);
    }

    #[test]
    fn single_exposure_record_is_passed_through() {
        // The stacking shouldn't break the single-record case.
        let bytes = b"#?RADIANCE\nEXPOSURE=1.5\n\n-Y 1 +X 8\n";
        let mut cursor = 0usize;
        let header = parse_header(bytes, &mut cursor).unwrap();
        assert_eq!(header.exposure, Some(1.5));
    }

    #[test]
    fn limits_reject_oversize_width_before_allocation() {
        // A resolution line declaring width 2 000 000 000 would, with
        // the round 1..201 decoder, attempt to allocate ~24 GiB of
        // float pixel buffer. The default HdrLimits cap (max_width =
        // 32_767) rejects the file at parse time.
        let bytes = b"#?RADIANCE\n\n-Y 1 +X 2000000000\n";
        let err = parse_hdr(bytes).unwrap_err();
        assert!(matches!(err, Error::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn limits_reject_oversize_height_before_allocation() {
        let bytes = b"#?RADIANCE\n\n-Y 2000000000 +X 8\n";
        let err = parse_hdr(bytes).unwrap_err();
        assert!(matches!(err, Error::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn limits_reject_pixel_byte_overrun() {
        // 32 767 × 32 767 = ~1 G pixels, ×12 B = ~12 GiB — well past
        // the 256 MiB default pixel-buffer cap. Both per-axis dimensions
        // sit at the very edge of `max_width` / `max_height`, so the
        // pixel-byte cap is what fires.
        let bytes = b"#?RADIANCE\n\n-Y 32767 +X 32767\n";
        let err = parse_hdr(bytes).unwrap_err();
        assert!(matches!(err, Error::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn limits_unbounded_admits_larger_dimensions_via_with_limits() {
        // The same 32_767 × 32_767 header above must be accepted past
        // the dimension check with `HdrLimits::unbounded`; the request
        // still fails downstream (the pixel section isn't present), but
        // with `InvalidData` rather than `TooLarge`.
        let bytes = b"#?RADIANCE\n\n-Y 32767 +X 32767\n";
        let err = parse_hdr_with_limits(bytes, &HdrLimits::unbounded()).unwrap_err();
        // Past the resolution-line gate, the absent pixel section will
        // either trip the new-RLE truncation guard (InvalidData) or
        // the per-row decode error — but NOT `TooLarge`. We assert the
        // negative so the test remains correct if the downstream error
        // shape evolves.
        assert!(
            !matches!(err, Error::TooLarge(_)),
            "unbounded limits should not raise TooLarge, got {err:?}",
        );
    }

    #[test]
    fn limits_custom_pixel_byte_cap_rejects_borderline_input() {
        // Tighten the pixel-byte cap to 1 KiB so even a 16×4 header
        // (16 × 4 × 12 = 768 B fits, but 32 × 4 × 12 = 1 536 B does
        // not) is rejected. The dimension caps are kept at their
        // default to confirm the pixel-byte axis fires independently.
        let bytes = b"#?RADIANCE\n\n-Y 4 +X 32\n";
        let custom = HdrLimits {
            max_pixel_bytes: 1024,
            ..HdrLimits::default()
        };
        let err =
            parse_hdr_with_options_and_limits(bytes, FallbackMode::OldRle, &custom).unwrap_err();
        assert!(matches!(err, Error::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn limits_accept_typical_image_dimensions() {
        // Round 1..201 corpus dimensions (32×16 etc.) must continue to
        // parse at the defaults. This is the "no regression" anchor:
        // the limit additions must not change the happy-path verdict
        // for any image the existing test suite decodes.
        let bytes = b"#?RADIANCE\n\n-Y 16 +X 32\n";
        // Header is parseable to the point of the resolution line; the
        // pixel section is missing so the decode fails downstream, but
        // not with TooLarge.
        let err = parse_hdr(bytes).unwrap_err();
        assert!(
            !matches!(err, Error::TooLarge(_)),
            "32×16 must clear the default limits, got {err:?}",
        );
    }
}
