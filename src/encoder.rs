//! Radiance HDR top-level encode: emit the magic line, the
//! `KEY=VALUE` header (FORMAT first, then anything the caller put in
//! [`HdrHeader::other`]), the resolution line, and the new-RLE pixel
//! rows.
//!
//! The encoder honours [`HdrHeader::y_sign`] / [`HdrHeader::x_sign`] /
//! [`HdrHeader::x_first`]. The pixel buffer is always interpreted as
//! top-down `(y, x)` row-major; when the requested axis flags differ
//! from that canonical orientation the encoder mirrors / transposes the
//! buffer on its way out so the on-disk file matches the requested
//! resolution-line orientation. Defaults are
//! `y_sign = Decreasing, x_sign = Increasing, x_first = false`, i.e.
//! the canonical `-Y H +X W` form. All eight axis-flag combinations are
//! supported on both read and write; round-tripping any of them through
//! [`encode_hdr`] + [`crate::parse_hdr`] reproduces the original
//! buffer.
//!
//! For [`RleMode::New`] the width must be in the range `8..=32767`
//! (the new-RLE marker can't address rows outside that range);
//! [`RleMode::Auto`] picks new-RLE for widths in that range and falls
//! back to [`RleMode::Old`] for narrower / wider images.

use std::borrow::Cow;

use crate::error::{HdrError as Error, Result};
use crate::header::{AxisSign, HdrHeader};
use crate::image::{HdrImage, HdrPixelFormat};
use crate::rgbe::rgb_to_rgbe;
use crate::rle::{encode_scanline, encode_scanline_old_rle};

/// Choice of line terminator used by the encoder's text section
/// (magic line, `KEY=VALUE` records, blank-line separator, resolution
/// line). The pixel payload that follows is always pure binary RLE.
///
/// The Radiance reader treats both `\n` and `\r\n` as a line break
/// (current decoder strips `\r` via [`crate::decoder::parse_hdr`]); the
/// encoder defaults to bare `\n` for the smaller wire image but
/// [`LineEnding::Crlf`] can be requested when the on-disk file needs to
/// match the CRLF convention some Windows-era Radiance tools used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// Single `\n` (the default, matches every shipped fixture in the
    /// Radiance reference distribution).
    Lf,
    /// `\r\n` for every text line. Pure-binary pixel payload unchanged.
    Crlf,
}

impl LineEnding {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::Crlf => b"\r\n",
        }
    }
}

/// Choice of RLE flavour for the encoded scanlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RleMode {
    /// Greg Ward's adaptive new-RLE (`0x02 0x02 hi lo` marker per
    /// scanline). Width must be in `8..=32767`.
    New,
    /// Pre-1991 old-RLE: per-pixel literals interleaved with chained
    /// `(1, 1, 1, n)` sentinel runs. No width restriction.
    Old,
    /// Heuristic: pick [`RleMode::New`] when the image width falls in
    /// `8..=32767` (the new-RLE marker's addressable range), otherwise
    /// fall back to [`RleMode::Old`]. The encoder never errors on
    /// out-of-range widths in `Auto` mode.
    Auto,
}

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
    encode_hdr_with_rle(image, RleMode::New)
}

/// Like [`encode_hdr`] but with an explicit choice of RLE flavour.
///
/// Use [`RleMode::Old`] for outputs targeting consumers that don't
/// recognise the post-1991 `0x02 0x02 hi lo` scanline marker (very
/// narrow images that fall outside the new-RLE width range
/// `8..=32767`, or when matching a legacy fixture exactly).
pub fn encode_hdr_with_rle(image: &HdrImage, rle: RleMode) -> Result<Vec<u8>> {
    encode_hdr_with_options(image, rle, LineEnding::Lf)
}

/// Full-control encode: pick the RLE flavour and the text-line
/// terminator independently.
///
/// The pixel payload following the resolution line is identical to
/// what [`encode_hdr_with_rle`] would produce — only the bytes of the
/// magic line, KEY=VALUE records, header terminator and resolution
/// line change between `LineEnding::Lf` and `LineEnding::Crlf`.
pub fn encode_hdr_with_options(
    image: &HdrImage,
    rle: RleMode,
    line_ending: LineEnding,
) -> Result<Vec<u8>> {
    let w = image.width as usize;
    let h = image.height as usize;
    if image.pixels.len() != w * h * 3 {
        return Err(Error::invalid(
            "HDR encoder: pixels length doesn't match width*height*3",
        ));
    }
    if w == 0 || h == 0 {
        return Err(Error::invalid("HDR encoder: zero dimension"));
    }
    // Reorder the canonical top-down (y, x) buffer into the layout
    // implied by the header's axis-sign flags before encoding. The
    // decoder applies the inverse on the way back, so any of the eight
    // axis-flag combinations round-trips losslessly.
    //
    // For Y-first headers (`±Y H ±X W`) the on-disk scanline is one row
    // of the canonical buffer and the only reordering is a vertical /
    // horizontal mirror per the sign flags. For X-first headers
    // (`±X W ±Y H`) each on-disk "scanline" is actually a column of the
    // canonical buffer, so we transpose into (x, y) order first; the
    // on-disk width then becomes `height` (the height-many original
    // rows, now laid out one per output sample) and the on-disk height
    // becomes `width`. The axis-sign flips apply after the transpose.
    //
    // Fast path: on the canonical `-Y H +X W` header (the overwhelmingly
    // common case — encoder default) `reorient_for_axis_flags` returns
    // a `Cow::Borrowed(&image.pixels)` so the previous round-131
    // unconditional `pixels.to_vec()` (~12 MiB alloc/memcpy per
    // 1024×1024 default-axis encode) is gone. Mirrored / transposed
    // headers still pay the allocation since the on-disk layout
    // genuinely differs from the canonical buffer.
    let (out_w, out_h, oriented) = reorient_for_axis_flags(&image.pixels, w, h, &image.header);
    // The new-RLE marker addresses the *on-disk* scanline width, which
    // differs from the canonical image width for X-first headers — apply
    // the auto/strict check against `out_w` rather than `w`.
    let effective_rle = match rle {
        RleMode::Auto => {
            if (8..=32767).contains(&out_w) {
                RleMode::New
            } else {
                RleMode::Old
            }
        }
        other => other,
    };
    if effective_rle == RleMode::New && !(8..=32767).contains(&out_w) {
        return Err(Error::unsupported(format!(
            "HDR encoder: on-disk scanline width {out_w} outside supported new-RLE range 8..=32767 (try RleMode::Old or RleMode::Auto)"
        )));
    }
    let mut out = Vec::with_capacity(32 + out_w * out_h * 4);
    write_header(&mut out, &image.header, line_ending);
    write_resolution(&mut out, out_w, out_h, &image.header, line_ending);
    write_pixel_rows(&mut out, out_w, out_h, &oriented, effective_rle)?;
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

fn write_header(out: &mut Vec<u8>, header: &HdrHeader, eol: LineEnding) {
    let nl = eol.as_bytes();
    out.extend_from_slice(b"#?RADIANCE");
    out.extend_from_slice(nl);
    out.extend_from_slice(format!("FORMAT={}", header.format.as_str()).as_bytes());
    out.extend_from_slice(nl);
    if let Some(g) = header.gamma {
        out.extend_from_slice(format!("GAMMA={g}").as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some(e) = header.exposure {
        out.extend_from_slice(format!("EXPOSURE={e}").as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some(p) = header.pixaspect {
        out.extend_from_slice(format!("PIXASPECT={p}").as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some([r, g, b]) = header.colorcorr {
        out.extend_from_slice(format!("COLORCORR={r} {g} {b}").as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some(p) = header.primaries {
        out.extend_from_slice(format!("PRIMARIES={}", p.to_record_string()).as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some(s) = &header.software {
        out.extend_from_slice(format!("SOFTWARE={s}").as_bytes());
        out.extend_from_slice(nl);
    }
    if let Some(v) = &header.view {
        out.extend_from_slice(format!("VIEW={v}").as_bytes());
        out.extend_from_slice(nl);
    }
    for (k, v) in &header.other {
        // Keep arbitrary records the caller stashed earlier.
        out.extend_from_slice(format!("{k}={v}").as_bytes());
        out.extend_from_slice(nl);
    }
    for c in &header.comments {
        out.extend_from_slice(format!("#{c}").as_bytes());
        out.extend_from_slice(nl);
    }
    // Empty line terminates the header.
    out.extend_from_slice(nl);
}

fn write_resolution(
    out: &mut Vec<u8>,
    out_width: usize,
    out_height: usize,
    header: &HdrHeader,
    eol: LineEnding,
) {
    let y_flag = match header.y_sign {
        AxisSign::Decreasing => "-Y",
        AxisSign::Increasing => "+Y",
    };
    let x_flag = match header.x_sign {
        AxisSign::Decreasing => "-X",
        AxisSign::Increasing => "+X",
    };
    // The resolution line lists either `<Y_flag> H <X_flag> W` (when
    // the on-disk scanline holds one row's worth of Y-pixels) or
    // `<X_flag> W <Y_flag> H` (when the on-disk scanline holds one
    // column's worth of X-pixels). In the X-first layout the `out_*`
    // dimensions are already swapped by `reorient_for_axis_flags`, so
    // `out_height` is the *image* width and vice versa.
    if header.x_first {
        // out_height = canonical width, out_width = canonical height.
        out.extend_from_slice(format!("{x_flag} {out_height} {y_flag} {out_width}").as_bytes());
    } else {
        out.extend_from_slice(format!("{y_flag} {out_height} {x_flag} {out_width}").as_bytes());
    }
    out.extend_from_slice(eol.as_bytes());
}

/// Reorder a canonical top-down `(y, x)` row-major float buffer into
/// the on-disk layout implied by `header.y_sign` / `header.x_sign` /
/// `header.x_first`.
///
/// Returns `(out_width, out_height, oriented_pixels)`. For Y-first
/// headers the returned width/height match the input; for X-first
/// headers they are swapped (each on-disk scanline is one column of
/// the canonical buffer).
///
/// The fast path — the canonical `-Y H +X W` default (no flip, no
/// transpose) — returns the caller's buffer as a `Cow::Borrowed`,
/// skipping the ~12 MiB alloc/memcpy that would otherwise dominate a
/// 1024×1024 default-axis encode. Mirrored / transposed cases still
/// produce an owned reordering since the on-disk layout genuinely
/// differs from the canonical buffer.
fn reorient_for_axis_flags<'a>(
    pixels: &'a [f32],
    width: usize,
    height: usize,
    header: &HdrHeader,
) -> (usize, usize, Cow<'a, [f32]>) {
    let flip_y = header.y_sign == AxisSign::Increasing;
    let flip_x = header.x_sign == AxisSign::Decreasing;

    if header.x_first {
        // Transpose into (x, y) row-major: each output row is a column
        // of the canonical buffer. After transpose the on-disk width is
        // the canonical `height` (one sample per original row) and the
        // on-disk height is the canonical `width`.
        let out_w = height;
        let out_h = width;
        let mut m = vec![0.0_f32; pixels.len()];
        for ox in 0..width {
            // `ox` is the canonical X column which becomes output row.
            // Apply the X sign flip on the source-X side: if `flip_x`
            // is set, the first on-disk "scanline" (output row index 0)
            // should hold the canonical right-most column.
            let src_x = if flip_x { width - 1 - ox } else { ox };
            for oy in 0..height {
                // `oy` is the canonical Y row which becomes output col.
                let src_y = if flip_y { height - 1 - oy } else { oy };
                let src = (src_y * width + src_x) * 3;
                let dst = (ox * out_w + oy) * 3;
                m[dst] = pixels[src];
                m[dst + 1] = pixels[src + 1];
                m[dst + 2] = pixels[src + 2];
            }
        }
        return (out_w, out_h, Cow::Owned(m));
    }

    if !flip_x && !flip_y {
        // Canonical orientation — no reordering needed, hand the
        // caller's buffer back unmodified.
        return (width, height, Cow::Borrowed(pixels));
    }
    let mut m = vec![0.0_f32; pixels.len()];
    for y in 0..height {
        let src_y = if flip_y { height - 1 - y } else { y };
        for x in 0..width {
            let src_x = if flip_x { width - 1 - x } else { x };
            let src = (src_y * width + src_x) * 3;
            let dst = (y * width + x) * 3;
            m[dst] = pixels[src];
            m[dst + 1] = pixels[src + 1];
            m[dst + 2] = pixels[src + 2];
        }
    }
    (width, height, Cow::Owned(m))
}

fn write_pixel_rows(
    out: &mut Vec<u8>,
    width: usize,
    height: usize,
    pixels: &[f32],
    rle: RleMode,
) -> Result<()> {
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
        match rle {
            RleMode::New => encode_scanline(&channels, width, out)?,
            RleMode::Old | RleMode::Auto => encode_scanline_old_rle(&channels, width, out)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::parse_hdr;

    fn pattern(w: u32, h: u32) -> HdrImage {
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..(w * h) as usize {
            pixels.push((i as f32 + 1.0) * 0.01);
            pixels.push((i as f32 + 1.0) * 0.005);
            pixels.push((i as f32 + 1.0) * 0.002);
        }
        HdrImage::new_rgb96f(w, h, pixels)
    }

    #[test]
    fn crlf_encoder_terminates_every_text_line_with_crlf() {
        // 16-wide so the new-RLE path fires.
        let img = pattern(16, 4);
        let bytes = encode_hdr_with_options(&img, RleMode::New, LineEnding::Crlf).unwrap();
        // The magic, FORMAT line, blank-line terminator and resolution
        // line must all end in `\r\n`.
        assert!(bytes.starts_with(b"#?RADIANCE\r\n"));
        // The first six bytes after the magic begin "FORMAT".
        let after_magic = &bytes[b"#?RADIANCE\r\n".len()..];
        assert!(after_magic.starts_with(b"FORMAT="));
        // No bare `\n` should precede the pixel payload start.
        // Locate the blank-line terminator (a `\r\n\r\n` quartet) — the
        // pixel data starts after the following resolution line.
        let blank_pos = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("CRLF blank terminator missing");
        // Confirm the resolution line that follows is CRLF too.
        let resline_start = blank_pos + 4;
        let resline_end = resline_start
            + bytes[resline_start..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("CRLF resolution-line terminator missing");
        let resline = std::str::from_utf8(&bytes[resline_start..resline_end]).unwrap();
        assert!(
            resline.starts_with("-Y ") || resline.starts_with("+Y "),
            "unexpected resolution-line orientation: {resline:?}",
        );
        // Roundtrip through the decoder (which already strips `\r`).
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(back.width, 16);
        assert_eq!(back.height, 4);
    }

    #[test]
    fn lf_encoder_produces_no_carriage_returns_in_text_section() {
        // Confirm the default LF path doesn't accidentally pick up a
        // `\r` anywhere in the text section.
        let img = pattern(16, 2);
        let bytes = encode_hdr_with_options(&img, RleMode::New, LineEnding::Lf).unwrap();
        // Locate the LF blank-line terminator.
        let blank_pos = bytes
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("LF blank terminator missing");
        // Then the LF after the resolution line.
        let resline_end = blank_pos
            + 2
            + bytes[blank_pos + 2..]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap();
        // No `\r` should appear in the text section ([0..resline_end+1]).
        assert!(
            !bytes[..=resline_end].contains(&b'\r'),
            "LF encoder leaked a carriage return into the text section",
        );
    }

    #[test]
    fn view_record_round_trips_through_encoder_and_decoder() {
        let mut img = pattern(16, 2);
        img.header.view = Some("rvu -vp 0 0 10 -vd 0 0 -1 -vu 0 1 0".to_owned());
        let bytes = encode_hdr(&img).unwrap();
        let head_end = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
        let head = std::str::from_utf8(&bytes[..head_end]).unwrap();
        assert!(
            head.contains("VIEW=rvu -vp 0 0 10 -vd 0 0 -1 -vu 0 1 0"),
            "VIEW record missing from header: {head:?}",
        );
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(
            back.header.view.as_deref(),
            Some("rvu -vp 0 0 10 -vd 0 0 -1 -vu 0 1 0")
        );
    }

    #[test]
    fn reorient_canonical_axis_borrows_input_buffer() {
        // The default `-Y H +X W` axis must not allocate a new pixel
        // buffer — `reorient_for_axis_flags` should hand the caller's
        // slice back as `Cow::Borrowed`. The pointer + length equality
        // check below is what the round-179 zero-copy refactor is
        // about; if a future change reintroduces an unconditional
        // `to_vec()` this test catches it.
        let img = pattern(16, 4);
        let (out_w, out_h, oriented) = reorient_for_axis_flags(
            &img.pixels,
            img.width as usize,
            img.height as usize,
            &img.header,
        );
        assert_eq!(out_w, img.width as usize);
        assert_eq!(out_h, img.height as usize);
        assert!(matches!(oriented, Cow::Borrowed(_)));
        // Identity of the borrow: same ptr + same length means the
        // encoder will read straight out of the caller's `Vec<f32>`.
        let canon_ptr = img.pixels.as_ptr();
        let canon_len = img.pixels.len();
        let oriented_ptr = oriented.as_ptr();
        let oriented_len = oriented.len();
        assert_eq!(canon_ptr, oriented_ptr);
        assert_eq!(canon_len, oriented_len);
    }

    #[test]
    fn reorient_flipped_axis_returns_owned_reordering() {
        // A non-canonical axis (here `+Y H +X W` — vertical mirror)
        // genuinely needs to reorder the buffer, so the slow path
        // continues to return an owned `Vec<f32>` wrapped in
        // `Cow::Owned`. Roundtrip the mirror through the decoder so the
        // ownership change is observably correct, not just a tag check.
        let mut img = pattern(16, 4);
        img.header.y_sign = AxisSign::Increasing;
        let (_out_w, _out_h, oriented) = reorient_for_axis_flags(
            &img.pixels,
            img.width as usize,
            img.height as usize,
            &img.header,
        );
        assert!(matches!(oriented, Cow::Owned(_)));
        let bytes = encode_hdr(&img).unwrap();
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(back.width, img.width);
        assert_eq!(back.height, img.height);
        assert_eq!(back.header.y_sign, AxisSign::Increasing);
        // Sample a handful of pixels to confirm the mirror round-trips.
        for y in 0..img.height as usize {
            for x in 0..img.width as usize {
                let i = (y * img.width as usize + x) * 3;
                let a = img.pixels[i];
                let b = back.pixels[i];
                let err = (a - b).abs();
                assert!(err < 0.02, "mirror y={y} x={x}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn crlf_round_trips_all_typed_header_records() {
        // Ensure the CRLF encoder doesn't accidentally drop typed
        // records by relying on `\n` being a single byte.
        let mut img = pattern(16, 2);
        img.header.exposure = Some(2.0);
        img.header.gamma = Some(1.0);
        img.header.software = Some("oxideav-hdr/crlf".to_owned());
        img.header.view = Some("rvu -vp 0 0 5".to_owned());
        img.header.colorcorr = Some([1.1, 1.0, 0.9]);
        img.header.pixaspect = Some(1.0);
        let bytes = encode_hdr_with_options(&img, RleMode::New, LineEnding::Crlf).unwrap();
        let back = parse_hdr(&bytes).unwrap();
        assert_eq!(back.header.exposure, Some(2.0));
        assert_eq!(back.header.gamma, Some(1.0));
        assert_eq!(back.header.software.as_deref(), Some("oxideav-hdr/crlf"));
        assert_eq!(back.header.view.as_deref(), Some("rvu -vp 0 0 5"));
        assert_eq!(back.header.colorcorr, Some([1.1, 1.0, 0.9]));
        assert_eq!(back.header.pixaspect, Some(1.0));
    }
}
