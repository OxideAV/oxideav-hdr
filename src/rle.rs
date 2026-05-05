//! Adaptive RLE for Radiance scanlines.
//!
//! Two flavours exist; this module knows about both on the read path
//! and emits only the new format on the write path.
//!
//! ## New RLE (post-1991, what every modern writer emits)
//!
//! Each scanline begins with the four-byte marker
//! ```text
//! 0x02 0x02 (width >> 8) (width & 0xFF)
//! ```
//! and the width must satisfy `8 <= W <= 32767` (the high bit being
//! clear is what disambiguates the marker from an old-RLE first
//! pixel — see below).
//!
//! After the marker the four channels (R, G, B, exponent) are stored
//! one after another, each as its own RLE stream covering exactly
//! `width` bytes. The codes inside each channel are:
//! * **Literal run**: a leading byte `n` with `1 <= n <= 128`. The
//!   next `n` bytes are copied verbatim into the output.
//! * **Repeat run**: a leading byte `n` with `129 <= n <= 255`. The
//!   following single byte is repeated `n - 128` times (so 1..=127
//!   repeats).
//!
//! ## Old RLE (pre-1991, read-only here)
//!
//! Each scanline is a sequence of 4-byte pixels. A pixel whose
//! mantissa is `(1, 1, 1)` is a *sentinel*: its exponent byte is the
//! low 8 bits of a run length, and consecutive sentinels chain higher
//! bytes shifted by 8 each. The run repeats the previous decoded pixel.
//! The first scanline cannot be a sentinel run (there's no previous
//! pixel), and a sentinel chain of 16 or more bytes is malformed.

use crate::error::{HdrError as Error, Result};

/// Decode one scanline. `width` is the number of pixels expected.
/// Returns four channel buffers (R, G, B, E) of `width` bytes each.
/// `pos` is advanced past the bytes consumed.
///
/// Picks new-RLE vs old-RLE based on the first four bytes: if they
/// match `0x02 0x02 hi lo` with `hi << 8 | lo == width` and `width >= 8`
/// the new format is used; otherwise we fall through to the old one.
pub fn decode_scanline(
    src: &[u8],
    pos: &mut usize,
    width: usize,
    prev_pixel: &mut Option<[u8; 4]>,
) -> Result<[Vec<u8>; 4]> {
    if width == 0 {
        return Err(Error::invalid("HDR: zero-width scanline"));
    }
    // Try new-RLE marker first.
    let p = *pos;
    if (8..32768).contains(&width)
        && src.len() >= p + 4
        && src[p] == 0x02
        && src[p + 1] == 0x02
        && (((src[p + 2] as usize) << 8) | src[p + 3] as usize) == width
    {
        *pos = p + 4;
        decode_new_rle(src, pos, width, prev_pixel)
    } else {
        decode_old_rle(src, pos, width, prev_pixel)
    }
}

fn decode_new_rle(
    src: &[u8],
    pos: &mut usize,
    width: usize,
    prev_pixel: &mut Option<[u8; 4]>,
) -> Result<[Vec<u8>; 4]> {
    let mut channels: [Vec<u8>; 4] = [
        Vec::with_capacity(width),
        Vec::with_capacity(width),
        Vec::with_capacity(width),
        Vec::with_capacity(width),
    ];
    for ch in &mut channels {
        let mut written = 0usize;
        while written < width {
            if *pos >= src.len() {
                return Err(Error::invalid("HDR: new-RLE truncated"));
            }
            let code = src[*pos];
            *pos += 1;
            if code > 128 {
                let run = (code & 0x7F) as usize;
                if run == 0 {
                    return Err(Error::invalid("HDR: new-RLE zero-length repeat"));
                }
                if written + run > width {
                    return Err(Error::invalid("HDR: new-RLE repeat overruns scanline"));
                }
                if *pos >= src.len() {
                    return Err(Error::invalid("HDR: new-RLE missing repeat byte"));
                }
                let value = src[*pos];
                *pos += 1;
                for _ in 0..run {
                    ch.push(value);
                }
                written += run;
            } else {
                let run = code as usize;
                if run == 0 {
                    return Err(Error::invalid("HDR: new-RLE zero-length literal"));
                }
                if written + run > width {
                    return Err(Error::invalid("HDR: new-RLE literal overruns scanline"));
                }
                if *pos + run > src.len() {
                    return Err(Error::invalid("HDR: new-RLE literal truncated"));
                }
                ch.extend_from_slice(&src[*pos..*pos + run]);
                *pos += run;
                written += run;
            }
        }
    }
    if let Some(slot) = prev_pixel {
        *slot = [
            channels[0][width - 1],
            channels[1][width - 1],
            channels[2][width - 1],
            channels[3][width - 1],
        ];
    } else {
        *prev_pixel = Some([
            channels[0][width - 1],
            channels[1][width - 1],
            channels[2][width - 1],
            channels[3][width - 1],
        ]);
    }
    Ok(channels)
}

fn decode_old_rle(
    src: &[u8],
    pos: &mut usize,
    width: usize,
    prev_pixel: &mut Option<[u8; 4]>,
) -> Result<[Vec<u8>; 4]> {
    let mut out: [Vec<u8>; 4] = [
        Vec::with_capacity(width),
        Vec::with_capacity(width),
        Vec::with_capacity(width),
        Vec::with_capacity(width),
    ];
    let mut last = prev_pixel.unwrap_or([0, 0, 0, 0]);
    let mut shift = 0u32;
    let mut written = 0usize;
    while written < width {
        if *pos + 4 > src.len() {
            return Err(Error::invalid("HDR: old-RLE truncated pixel"));
        }
        let pixel = [src[*pos], src[*pos + 1], src[*pos + 2], src[*pos + 3]];
        *pos += 4;
        if pixel[0] == 1 && pixel[1] == 1 && pixel[2] == 1 {
            // Sentinel: low 8 bits of a run-length stored in the
            // exponent byte; chained sentinels accumulate higher bytes.
            let chunk = (pixel[3] as u32) << shift;
            shift += 8;
            if shift > 24 {
                return Err(Error::invalid("HDR: old-RLE run length overflow"));
            }
            let run = chunk as usize;
            if written + run > width {
                return Err(Error::invalid("HDR: old-RLE repeat overruns scanline"));
            }
            for _ in 0..run {
                out[0].push(last[0]);
                out[1].push(last[1]);
                out[2].push(last[2]);
                out[3].push(last[3]);
            }
            written += run;
            // Note: we do NOT reset `shift` here. The Radiance grammar
            // chains sentinels until the next non-sentinel pixel; the
            // runs accumulate until then.
        } else {
            // Plain literal pixel. Resets the chained-shift accumulator.
            out[0].push(pixel[0]);
            out[1].push(pixel[1]);
            out[2].push(pixel[2]);
            out[3].push(pixel[3]);
            last = pixel;
            shift = 0;
            written += 1;
        }
    }
    *prev_pixel = Some(last);
    Ok(out)
}

/// Encode one scanline using the new-RLE format. Each of the four
/// channels is RLE-coded independently and the four resulting streams
/// are concatenated after the `0x02 0x02 hi lo` marker.
pub fn encode_scanline(channels: &[Vec<u8>; 4], width: usize, out: &mut Vec<u8>) -> Result<()> {
    if !(8..32768).contains(&width) {
        return Err(Error::unsupported(
            "HDR encoder: new-RLE width must be 8..=32767",
        ));
    }
    out.push(0x02);
    out.push(0x02);
    out.push((width >> 8) as u8);
    out.push((width & 0xFF) as u8);
    for ch in channels {
        debug_assert_eq!(ch.len(), width);
        encode_channel(ch, out);
    }
    Ok(())
}

fn encode_channel(buf: &[u8], out: &mut Vec<u8>) {
    let n = buf.len();
    let mut i = 0usize;
    while i < n {
        // Look for a run of identical bytes starting at i.
        let mut run_end = i + 1;
        while run_end < n && buf[run_end] == buf[i] && (run_end - i) < 127 {
            run_end += 1;
        }
        let run_len = run_end - i;
        if run_len >= 4 {
            // Worth a repeat code.
            out.push(128 + run_len as u8);
            out.push(buf[i]);
            i = run_end;
        } else {
            // Emit a literal run up to the next length-3+ identical
            // run (or end-of-buffer / 128-byte cap).
            let mut lit_end = i + 1;
            let mut max_end = (i + 128).min(n);
            while lit_end < max_end {
                // Peek one byte ahead — if we're about to enter a
                // length-3 run, stop the literal here so the run can
                // become its own repeat code.
                let mut look = lit_end;
                let mut same = 1;
                while look + 1 < n && buf[look + 1] == buf[look] && same < 4 {
                    look += 1;
                    same += 1;
                }
                if same >= 4 {
                    break;
                }
                lit_end += 1;
                // Re-cap so we never write more than 128 literals.
                max_end = (i + 128).min(n);
            }
            let lit_len = lit_end - i;
            out.push(lit_len as u8);
            out.extend_from_slice(&buf[i..lit_end]);
            i = lit_end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_roundtrip_literals_only() {
        let mut encoded = Vec::new();
        let chans = [
            (0..16u8).collect::<Vec<_>>(),
            (16..32u8).collect::<Vec<_>>(),
            (32..48u8).collect::<Vec<_>>(),
            (48..64u8).collect::<Vec<_>>(),
        ];
        encode_scanline(&chans, 16, &mut encoded).unwrap();
        let mut pos = 0;
        let mut prev = None;
        let back = decode_scanline(&encoded, &mut pos, 16, &mut prev).unwrap();
        assert_eq!(back, chans);
        assert_eq!(pos, encoded.len());
    }

    #[test]
    fn channel_roundtrip_repeats() {
        let mut encoded = Vec::new();
        let chans = [
            vec![0xAAu8; 64],
            vec![0xBBu8; 64],
            vec![0xCCu8; 64],
            vec![0xDDu8; 64],
        ];
        encode_scanline(&chans, 64, &mut encoded).unwrap();
        // Repeat-only payload should be very short.
        assert!(encoded.len() < 64);
        let mut pos = 0;
        let mut prev = None;
        let back = decode_scanline(&encoded, &mut pos, 64, &mut prev).unwrap();
        assert_eq!(back, chans);
    }

    #[test]
    fn channel_roundtrip_mixed() {
        let mut encoded = Vec::new();
        let mut data = (0..50u8).collect::<Vec<_>>();
        data.extend(std::iter::repeat(0x77).take(40));
        data.extend(50..60u8);
        let chans = [data.clone(), data.clone(), data.clone(), data.clone()];
        let w = data.len();
        encode_scanline(&chans, w, &mut encoded).unwrap();
        let mut pos = 0;
        let mut prev = None;
        let back = decode_scanline(&encoded, &mut pos, w, &mut prev).unwrap();
        assert_eq!(back, chans);
    }

    #[test]
    fn old_rle_decodes_literal_pixels() {
        // A scanline with three independent pixels (no sentinels).
        let pixels = [
            0x10, 0x20, 0x30, 0x80, // literal pixel A
            0x40, 0x50, 0x60, 0x80, // literal pixel B
            0x70, 0x80, 0x90, 0x80, // literal pixel C
        ];
        let mut pos = 0;
        let mut prev = None;
        let chans = decode_scanline(&pixels, &mut pos, 3, &mut prev).unwrap();
        assert_eq!(chans[0], vec![0x10, 0x40, 0x70]);
        assert_eq!(chans[1], vec![0x20, 0x50, 0x80]);
        assert_eq!(chans[2], vec![0x30, 0x60, 0x90]);
        assert_eq!(chans[3], vec![0x80, 0x80, 0x80]);
        assert_eq!(pos, pixels.len());
    }

    #[test]
    fn old_rle_decodes_sentinel_run() {
        // First a literal pixel, then a sentinel saying "repeat 5×".
        let pixels = [
            0x55, 0x66, 0x77, 0x80, // literal pixel
            0x01, 0x01, 0x01, 0x05, // sentinel: repeat last pixel 5 times
        ];
        let mut pos = 0;
        let mut prev = None;
        let chans = decode_scanline(&pixels, &mut pos, 6, &mut prev).unwrap();
        assert_eq!(chans[0], vec![0x55; 6]);
        assert_eq!(chans[1], vec![0x66; 6]);
        assert_eq!(chans[2], vec![0x77; 6]);
        assert_eq!(chans[3], vec![0x80; 6]);
    }
}
