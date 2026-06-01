#![no_main]

//! Decode arbitrary fuzz-supplied bytes through `parse_hdr`. The
//! decoder must always return a `Result` and never panic / abort /
//! OOM, regardless of how malformed the input is.
//!
//! The contract under test is purely that the call *returns*: a
//! malformed stream yields `Err(HdrError::…)`, a well-formed one
//! yields `Ok(HdrImage)`, and neither path may panic, integer-overflow
//! (in a debug build), index out of bounds, or try to allocate an
//! attacker-controlled pixel buffer the size of the claimed
//! `width * height * 3 * sizeof(f32)`. `parse_hdr` applies the default
//! `HdrLimits` (max 32_767 × 32_767, ≤ 256 MiB pixel buffer), so a
//! hostile resolution line is rejected before the decoder touches its
//! allocator. The return value is intentionally discarded.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::parse_hdr;

fuzz_target!(|data: &[u8]| {
    let _ = parse_hdr(data);
});
