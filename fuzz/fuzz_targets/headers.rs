#![no_main]

//! Focus the corpus on the text-header parse path.
//!
//! `decode.rs` exercises the whole decoder against arbitrary fuzz
//! input, which is the right shape for catching panics in the RLE
//! inner loops — but its corpus tends to drift towards inputs that
//! fail the magic check early and never reach the header `KEY=VALUE`
//! split. This target prepends a valid `#?RADIANCE\n` magic line, a
//! fuzz-supplied header body, and a minimal `-Y 1 +X 8\n` resolution
//! line plus 32 bytes of synthetic pixel payload (32 = 4 bytes/pixel
//! × 8 px, the smallest legal new-RLE width), so libFuzzer's coverage
//! gradient pulls the corpus towards interesting `KEY=VALUE` shapes:
//! malformed `EXPOSURE=` / `COLORCORR=` / `PRIMARIES=` records,
//! mid-line `=` placement, comment lines (`#…`), UTF-8 boundary
//! splits, etc.
//!
//! `parse_hdr` applies the default `HdrLimits` so the trailing
//! pixel-section read is bounded regardless of what the header parse
//! does.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::parse_hdr;

fuzz_target!(|data: &[u8]| {
    // Strip any embedded NUL bytes — `parse_hdr` reads the header as
    // line-delimited UTF-8 and a stray NUL would just look like one
    // more byte in a value, which doesn't add coverage. Skip the
    // input rather than smuggle them in.
    if data.iter().any(|&b| b == 0) {
        return;
    }
    // Cap the header body so libFuzzer's corpus doesn't bloat with
    // multi-megabyte text inputs. 4 KiB is more than any real Radiance
    // header.
    let header_body = if data.len() <= 4096 {
        data
    } else {
        &data[..4096]
    };

    // Build:
    //   #?RADIANCE\n
    //   <fuzz body>
    //   \n          (blank-line terminator)
    //   -Y 1 +X 8\n (smallest legal new-RLE resolution)
    //   <32 B pixel data — 8 px × 4 B = uncompressed flat scanline>
    let mut buf = Vec::with_capacity(64 + header_body.len() + 32);
    buf.extend_from_slice(b"#?RADIANCE\n");
    buf.extend_from_slice(header_body);
    // Ensure the body ends in a newline so the blank-line terminator
    // sits on its own line; if the fuzz body already ends with `\n`
    // the extra one is the terminator, otherwise the body's final
    // record gets one and the next iteration adds the terminator.
    buf.push(b'\n');
    // Blank-line terminator.
    buf.push(b'\n');
    // Minimum legal resolution + 32 B of flat pixel payload.
    buf.extend_from_slice(b"-Y 1 +X 8\n");
    buf.extend_from_slice(&[0x10, 0x20, 0x30, 0x80].repeat(8));

    let _ = parse_hdr(&buf);
});
