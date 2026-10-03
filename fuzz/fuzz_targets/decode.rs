#![no_main]

//! Feed arbitrary fuzz-supplied bytes through the contract's three
//! read entry points — `probe`, `info` and `decode` (plus the strict
//! variant and the 8-bit convenience path). Every call must *return*:
//! a malformed stream yields `Err(HdrError::…)`, a well-formed one
//! yields `Ok(..)`, and neither path may panic, integer-overflow (in a
//! debug build), index out of bounds, or try to allocate an
//! attacker-controlled pixel plane the size of the claimed
//! `width * height * 12`. `decode` applies `DecodeOptions::default()`
//! (max 32 767 × 32 767, ≤ 1 GiB plane), so a hostile resolution line
//! is rejected before the decoder touches its allocator; `info` applies
//! no limit and allocates nothing but the header. Return values are
//! intentionally discarded, except that a successful `decode` must
//! agree with `info` on the geometry and must re-encode.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::{decode, decode_rgb8, decode_with, encode, info, probe, DecodeOptions, EncodeOptions};

fuzz_target!(|data: &[u8]| {
    let _ = probe(data);
    let header = info(data);
    let strict = DecodeOptions::default().with_strict(true);
    let _ = decode_with(data, &strict);
    if let Ok(img) = decode(data) {
        let i = header.expect("decode succeeded, so info must too");
        assert_eq!((i.width, i.height), (img.width(), img.height()));
        assert_eq!(i.color, img.color);
        let rgb = img.to_rgb8();
        assert_eq!(rgb.len(), img.width() as usize * img.height() as usize * 3);
        assert_eq!(decode_rgb8(data).expect("decode_rgb8 agrees").data, rgb);
        encode(&img, &EncodeOptions::default()).expect("a decoded picture always re-encodes");
    }
});
