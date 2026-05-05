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

/// Encode one scanline using the *old* (pre-1991) RLE format —
/// per-pixel literal bytes interleaved with `(1, 1, 1, n_low)`
/// "sentinel" pixels that repeat the previous literal pixel. Useful for
/// callers that need to write the legacy format (very narrow images, or
/// fixtures targeting old viewers that don't grok the new-RLE marker).
///
/// The first pixel cannot be a sentinel — there is no previous pixel
/// for it to repeat — so the encoder always emits at least one literal
/// at the start of the scanline. Run lengths above 255 are split into
/// chained sentinels (each carries one byte of the run length, low to
/// high, shifted by 8 each). The chained-sentinel grammar caps the run
/// at 24 bits (`0xFF_FFFF`); longer runs are split into multiple
/// literal+sentinel pairs.
///
/// A literal pixel whose mantissa happens to be `(1, 1, 1)` would be
/// indistinguishable from a sentinel on the read side, so we promote
/// it to `(2, 1, 1)` (least-significant-bit nudge on the red channel)
/// before writing. This matches what Greg Ward's reference writer did
/// to sidestep the same ambiguity.
pub fn encode_scanline_old_rle(
    channels: &[Vec<u8>; 4],
    width: usize,
    out: &mut Vec<u8>,
) -> Result<()> {
    if width == 0 {
        return Err(Error::invalid("HDR encoder: zero-width old-RLE scanline"));
    }
    if channels[0].len() != width
        || channels[1].len() != width
        || channels[2].len() != width
        || channels[3].len() != width
    {
        return Err(Error::invalid(
            "HDR encoder: old-RLE channel length mismatch",
        ));
    }
    let mut i = 0usize;
    let mut prev: Option<[u8; 4]> = None;
    while i < width {
        // Sanitise the current pixel so it can't collide with the
        // sentinel marker `(1, 1, 1, *)`. We bump the red mantissa to
        // 2 — the on-disk format quantises in 1/256 units so this is a
        // ~0.4% perturbation, well below the shared-exponent noise.
        let mut pixel = [
            channels[0][i],
            channels[1][i],
            channels[2][i],
            channels[3][i],
        ];
        if pixel[0] == 1 && pixel[1] == 1 && pixel[2] == 1 {
            pixel[0] = 2;
        }
        // First pixel of the scanline must be a literal.
        if prev != Some(pixel) || i == 0 {
            out.extend_from_slice(&pixel);
            prev = Some(pixel);
            i += 1;
            continue;
        }
        // We already emitted a literal matching `pixel`. Count further
        // identical pixels and emit a chained-sentinel run.
        let mut run = 0usize;
        while i < width && run < 0x00FF_FFFF {
            let cur = [
                channels[0][i],
                channels[1][i],
                channels[2][i],
                channels[3][i],
            ];
            let cur = if cur[0] == 1 && cur[1] == 1 && cur[2] == 1 {
                [2, cur[1], cur[2], cur[3]]
            } else {
                cur
            };
            if cur != pixel {
                break;
            }
            run += 1;
            i += 1;
        }
        // `run` is the number of *additional* identical pixels we want
        // to emit. The decoder treats each chained-sentinel byte as
        // shifted by 8 bits, so we split the run length into 8-bit
        // chunks low-to-high and emit them as a contiguous chain.
        emit_run_chain(run, out);
    }
    Ok(())
}

fn emit_run_chain(run: usize, out: &mut Vec<u8>) {
    if run == 0 {
        return;
    }
    // The decoder accumulates `(byte << shift)` across consecutive
    // chained sentinels, with `shift` advancing by 8 per sentinel.
    // Each sentinel therefore contributes one *byte* of the binary
    // expansion of the desired run length. We emit those bytes low to
    // high. A zero byte in the middle of the chain still has to be
    // written out — the next sentinel's shift depends on its position
    // in the chain, not on the bytes around it — but any trailing zero
    // bytes can simply be dropped (they'd contribute 0 each).
    let mut bytes: [u8; 4] = [
        (run & 0xFF) as u8,
        ((run >> 8) & 0xFF) as u8,
        ((run >> 16) & 0xFF) as u8,
        ((run >> 24) & 0xFF) as u8,
    ];
    // Cap at 24 bits (caller guarantees `run < 0x100_0000`) and drop
    // trailing zero chunks.
    debug_assert_eq!(bytes[3], 0, "old-RLE run length capped to 24 bits");
    bytes[3] = 0;
    let mut last = 3;
    while last > 0 && bytes[last] == 0 {
        last -= 1;
    }
    for &chunk in &bytes[..=last] {
        out.extend_from_slice(&[1, 1, 1, chunk]);
    }
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

    fn decode_old_rle_only(src: &[u8], width: usize) -> [Vec<u8>; 4] {
        // Force the old-RLE path by skipping the new-RLE marker probe.
        let mut pos = 0;
        let mut prev = None;
        super::decode_old_rle(src, &mut pos, width, &mut prev).unwrap()
    }

    #[test]
    fn old_rle_encode_roundtrip_literals_and_runs() {
        // Mixed: 3 distinct literal pixels + 7 repeats + 2 distinct.
        let chans = [
            vec![
                0x10, 0x40, 0x70, 0x70, 0x70, 0x70, 0x70, 0x70, 0x70, 0x80, 0x90,
            ],
            vec![
                0x20, 0x50, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x10, 0x20,
            ],
            vec![
                0x30, 0x60, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x30, 0x40,
            ],
            vec![
                0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80,
            ],
        ];
        let w = chans[0].len();
        let mut encoded = Vec::new();
        encode_scanline_old_rle(&chans, w, &mut encoded).unwrap();
        let back = decode_old_rle_only(&encoded, w);
        assert_eq!(back, chans);
    }

    #[test]
    fn old_rle_encode_chained_sentinel_for_long_run() {
        // 300 identical pixels (after a literal) needs a 2-byte chain
        // (300 = 0x12C → chunks 0x2C low + 0x01 high → 44 + 256 = 300).
        let mut chans: [Vec<u8>; 4] = [
            vec![0x42; 301],
            vec![0x55; 301],
            vec![0x66; 301],
            vec![0x80; 301],
        ];
        // Make the very first pixel different so we exercise the
        // literal-then-run path rather than degenerate first-pixel-must-
        // be-literal logic.
        chans[0][0] = 0x10;
        chans[1][0] = 0x20;
        chans[2][0] = 0x30;
        let w = chans[0].len();
        let mut encoded = Vec::new();
        encode_scanline_old_rle(&chans, w, &mut encoded).unwrap();
        let back = decode_old_rle_only(&encoded, w);
        assert_eq!(back, chans);
    }

    #[test]
    fn old_rle_encode_avoids_sentinel_collision() {
        // A literal pixel whose mantissa is (1, 1, 1) collides with
        // the sentinel marker — encoder should perturb it.
        let chans = [
            vec![0x01, 0x40, 0x70],
            vec![0x01, 0x50, 0x80],
            vec![0x01, 0x60, 0x90],
            vec![0x80, 0x80, 0x80],
        ];
        let mut encoded = Vec::new();
        encode_scanline_old_rle(&chans, 3, &mut encoded).unwrap();
        // First pixel on disk should NOT be (1, 1, 1, *).
        assert_ne!(&encoded[..3], [1, 1, 1].as_slice());
        let back = decode_old_rle_only(&encoded, 3);
        // Round-trip preserves everything except the first pixel's red
        // mantissa, which got nudged from 1 to 2.
        assert_eq!(back[0][0], 2);
        assert_eq!(back[1][0], 1);
        assert_eq!(back[2][0], 1);
        assert_eq!(back[3][0], 0x80);
    }

    #[test]
    fn old_rle_encode_first_pixel_always_literal() {
        // Even if all pixels are identical, the first one must be a
        // literal — there's no previous pixel for it to repeat.
        let chans = [vec![0x55; 4], vec![0x66; 4], vec![0x77; 4], vec![0x80; 4]];
        let mut encoded = Vec::new();
        encode_scanline_old_rle(&chans, 4, &mut encoded).unwrap();
        // First 4 bytes should be the literal pixel itself.
        assert_eq!(&encoded[..4], &[0x55, 0x66, 0x77, 0x80]);
        let back = decode_old_rle_only(&encoded, 4);
        assert_eq!(back, chans);
    }
}
