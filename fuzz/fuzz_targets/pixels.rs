#![no_main]

//! Drive the corpus straight into the pixel-section (RLE) decoders.
//!
//! `decode.rs` feeds wholly arbitrary bytes to `parse_hdr`, which is
//! the right shape for catching panics anywhere in the pipeline — but
//! libFuzzer has to stumble onto a valid `#?RADIANCE` magic, a blank
//! line, *and* a well-formed `-Y H +X W` resolution line all by chance
//! before a single byte ever reaches the new-RLE / old-RLE inner
//! loops. That structural prefix is statistically rare, so the
//! coverage gradient rarely pushes the corpus deep into the run-code
//! handling. `roundtrip.rs`, conversely, only ever decodes the
//! encoder's *own* well-formed output, so it never sees hostile run
//! codes at all.
//!
//! This target closes that gap: it synthesises a valid container
//! envelope (magic + blank line + a fuzz-chosen resolution line) and
//! then appends the *rest* of the fuzz input verbatim as the pixel
//! section. The width/height are derived from two fuzz bytes and
//! capped small so the decoder's per-scanline buffers stay bounded
//! (the default `HdrLimits` also gate the resolution integers, but
//! keeping the chosen dims tiny means the fuzzer spends its effort on
//! the run-code grammar rather than on allocating large buffers).
//!
//! Both `FallbackMode` branches are exercised on every input:
//!
//! * `OldRle` — the `(1,1,1,n)` sentinel-pixel grammar, the
//!   chained-shift run accumulator, the previous-pixel repeat logic.
//! * `Uncompressed` — the flat `4 * width` quad reader.
//!
//! The new-RLE marker (`0x02 0x02 hi lo`) is recognised under *either*
//! fallback whenever the fuzz pixel bytes happen to start with it, so
//! the per-channel literal/repeat inner loop is reachable from both
//! calls too.
//!
//! The contract under test is identical to `decode.rs`: the call must
//! always *return* a `Result` and never panic, integer-overflow (debug
//! build), index out of bounds, or over-allocate. The return value is
//! intentionally discarded.
//!
//! As with the other targets, the crate is pulled in with
//! `default-features = false`, so the fuzz build never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::{parse_hdr_with_options, FallbackMode};

fuzz_target!(|data: &[u8]| {
    // Need two bytes for the dimensions; the rest is the pixel section.
    if data.len() < 2 {
        return;
    }

    // Derive small, in-bounds dimensions from the first two fuzz bytes.
    // Range `1..=256` for each: spanning below the new-RLE floor (8) so
    // the old-RLE / uncompressed fallbacks fire, across the floor so
    // the new-RLE marker path is reachable, and small enough that the
    // worst-case per-scanline buffer (256 px × 4 channels) stays tiny.
    let width: u32 = u32::from(data[0]) + 1;
    let height: u32 = u32::from(data[1]) + 1;
    let pixel_section = &data[2..];

    // Build the container envelope:
    //   #?RADIANCE\n
    //   FORMAT=32-bit_rle_rgbe\n
    //   \n                         (blank-line header terminator)
    //   -Y <height> +X <width>\n   (standard axis order)
    //   <fuzz pixel section>
    let mut buf = Vec::with_capacity(48 + pixel_section.len());
    buf.extend_from_slice(b"#?RADIANCE\n");
    buf.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    buf.push(b'\n');
    buf.extend_from_slice(format!("-Y {height} +X {width}\n").as_bytes());
    buf.extend_from_slice(pixel_section);

    // Exercise both fallback grammars on the identical envelope. Each
    // must return a Result without panicking; the new-RLE marker path
    // is reachable from either when the pixel bytes start with the
    // `0x02 0x02 hi lo` marker for the chosen width.
    let _ = parse_hdr_with_options(&buf, FallbackMode::OldRle);
    let _ = parse_hdr_with_options(&buf, FallbackMode::Uncompressed);
});
