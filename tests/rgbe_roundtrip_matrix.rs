//! Bit-exact RGBE-quad round-trip matrix.
//!
//! The float-in / float-out path through [`encode_hdr`] / [`parse_hdr`] is
//! inherently lossy (8-bit shared-exponent mantissas). The *byte* layer —
//! the RGBE quads the scanline RLE packs and unpacks — is not: every
//! scanline flavour (new-RLE, old-RLE, uncompressed) is a lossless
//! re-packing of the exact `[R, G, B, E]` quads, and the shared-exponent
//! quantiser is idempotent on the subset of quads the encoder produces
//! (dominant mantissa `>= 128`, decoded magnitude above the `1e-32` black
//! floor — see `rgb_to_rgbe`'s idempotence note).
//!
//! This matrix proves the resulting bit-exact contract end-to-end: a
//! picture built from normalised RGBE quads via
//! [`HdrImage::from_rgbe_quads`], encoded, decoded, and read back with
//! [`HdrImage::to_rgbe_quads`], reproduces the original quad stream
//! **byte-for-byte** across the cross-product of:
//!
//! * resolution variants (square, wide, tall, single-row, single-column,
//!   minimum new-RLE width, just-under new-RLE width),
//! * all eight resolution-string orientations (the `Orientation` enum),
//! * every encoder RLE flavour (New / Old / Auto / Uncompressed), each
//!   decoded with the matching [`FallbackMode`].
//!
//! No external property-test crate is used (clean-room + dependency
//! discipline); the quad streams come from a small deterministic LCG so
//! the matrix is reproducible and the wall stays spec-only.

use oxideav_hdr::{
    encode_hdr_with_options, parse_hdr_with_options, AxisSign, FallbackMode, HdrHeader, HdrImage,
    LineEnding, Orientation, RleMode,
};

/// Deterministic 64-bit linear-congruential generator (Numerical Recipes
/// constants). Reproducible across runs; no external dependency.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn byte(&mut self) -> u8 {
        (self.next_u32() >> 24) as u8
    }
}

/// Generate `count` *normalised* RGBE quads — quads that lie in the
/// idempotent subset of the shared-exponent codec, so they re-encode to
/// themselves bit-exactly:
///
/// * the dominant mantissa is forced to `>= 128` (the encoder always
///   normalises the largest channel into `[128, 256)`),
/// * the exponent byte is kept in `64..=255` so the decoded magnitude
///   stays above the `1e-32` black floor,
/// * occasional black sentinels (`[0, 0, 0, 0]`) are sprinkled in — they
///   are also fixed points of the round-trip.
fn normalised_quads(rng: &mut Lcg, count: usize) -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        // ~1 in 16 pixels is the black sentinel.
        if rng.byte() < 16 {
            out.push([0, 0, 0, 0]);
            continue;
        }
        // Three mantissas, then force at least one to be the dominant
        // >= 128 channel so the quad is normalised.
        let mut m = [rng.byte(), rng.byte(), rng.byte()];
        let dom = (rng.next_u32() % 3) as usize;
        if m[dom] < 128 {
            m[dom] |= 0x80; // lift into [128, 255].
        }
        // Exponent byte in 64..=255 keeps every channel above 1e-32.
        let e = 64 + (rng.byte() % 192);
        out.push([m[0], m[1], m[2], e]);
    }
    out
}

/// The decoder fallback that matches a given encoder RLE flavour for the
/// non-new-RLE scanline path.
fn fallback_for(rle: RleMode) -> FallbackMode {
    match rle {
        // New-RLE carries its own marker; the fallback is never reached,
        // but OldRle is the historical default.
        RleMode::New | RleMode::Auto | RleMode::Old => FallbackMode::OldRle,
        RleMode::Uncompressed => FallbackMode::Uncompressed,
    }
}

/// Resolutions chosen to stress the encoder's width-dependent branches:
/// the new-RLE width window (`8..=32767`), single-row / single-column
/// degenerate shapes, and non-square aspect ratios that expose transpose
/// bugs in the X-first orientations.
const RESOLUTIONS: &[(u32, u32)] = &[
    (8, 8),  // minimum new-RLE width, square.
    (16, 4), // wide.
    (4, 16), // tall (width below new-RLE min — exercises Auto fallback).
    (1, 12), // single column.
    (12, 1), // single row.
    (32, 9), // larger, odd height.
    (7, 7),  // just under the new-RLE min on both axes.
    (9, 13), // coprime non-square.
];

const ORIENTATIONS: [Orientation; 8] = [
    Orientation::Standard,
    Orientation::FlipX,
    Orientation::Rotate180,
    Orientation::FlipY,
    Orientation::Rotate90Cw,
    Orientation::Rotate90CwFlipY,
    Orientation::Rotate90Ccw,
    Orientation::Rotate90CcwFlipY,
];

/// Encoder RLE flavours that round-trip every quad losslessly.
///
/// `RleMode::Old` perturbs a literal `(1, 1, 1, *)` quad's red mantissa
/// to dodge the run sentinel, so it is *not* byte-exact in general; the
/// generator never emits `(1, 1, 1)` mantissas (the dominant channel is
/// always `>= 128`), so Old is byte-exact here too. Auto picks New within
/// the width window and Old outside it.
const RLE_MODES: [RleMode; 4] = [
    RleMode::New,
    RleMode::Old,
    RleMode::Auto,
    RleMode::Uncompressed,
];

#[test]
fn rgbe_quads_round_trip_bit_exactly_across_resolution_orientation_rle_matrix() {
    let mut rng = Lcg::new(0x0344_0344_u64);
    let mut cases = 0usize;
    for &(w, h) in RESOLUTIONS {
        let quads = normalised_quads(&mut rng, (w * h) as usize);
        for &orientation in &ORIENTATIONS {
            for &rle in &RLE_MODES {
                // New-RLE rejects on-disk scanline widths outside
                // 8..=32767. The on-disk scanline width is the canonical
                // width for Y-first orientations and the canonical height
                // for X-first ones. Skip the combinations New can't
                // represent (Auto + Old + Uncompressed cover those).
                let on_disk_scanline_w = if orientation.is_x_first() { h } else { w };
                if rle == RleMode::New && !(8..=32767).contains(&on_disk_scanline_w) {
                    continue;
                }

                let mut header = HdrHeader::default();
                header.set_orientation(orientation);
                let img = HdrImage::from_rgbe_quads(w, h, &quads, header);

                let bytes = encode_hdr_with_options(&img, rle, LineEnding::Lf)
                    .unwrap_or_else(|e| panic!("encode {w}x{h} {orientation:?} {rle:?}: {e}"));
                let back = parse_hdr_with_options(&bytes, fallback_for(rle))
                    .unwrap_or_else(|e| panic!("decode {w}x{h} {orientation:?} {rle:?}: {e}"));

                assert_eq!(back.width, w, "{w}x{h} {orientation:?} {rle:?}: width");
                assert_eq!(back.height, h, "{w}x{h} {orientation:?} {rle:?}: height");
                assert_eq!(
                    back.header.orientation(),
                    orientation,
                    "{w}x{h} {orientation:?} {rle:?}: orientation slot lost"
                );

                let back_quads = back.to_rgbe_quads();
                assert_eq!(
                    back_quads, quads,
                    "{w}x{h} {orientation:?} {rle:?}: RGBE quads drifted across round-trip"
                );
                cases += 1;
            }
        }
    }
    // Sanity: the matrix actually exercised a meaningful number of cases.
    assert!(cases >= 200, "matrix only ran {cases} cases");
}

#[test]
fn rgbe_quads_round_trip_under_crlf_line_endings() {
    // The CRLF text section must not perturb the binary quad payload.
    let mut rng = Lcg::new(7);
    let (w, h) = (16u32, 6u32);
    let quads = normalised_quads(&mut rng, (w * h) as usize);
    for &orientation in &ORIENTATIONS {
        let mut header = HdrHeader::default();
        header.set_orientation(orientation);
        let img = HdrImage::from_rgbe_quads(w, h, &quads, header);
        let bytes = encode_hdr_with_options(&img, RleMode::Auto, LineEnding::Crlf).unwrap();
        let back = parse_hdr_with_options(&bytes, FallbackMode::OldRle).unwrap();
        assert_eq!(back.to_rgbe_quads(), quads, "{orientation:?}: CRLF drift");
    }
}

#[test]
fn rgbe_quads_round_trip_preserves_typed_header_records() {
    // The byte-exact quad contract must coexist with the typed-header
    // round-trip: build a picture from quads, stamp every typed slot, and
    // confirm both the quads and the records survive.
    use oxideav_hdr::Primaries;
    let mut rng = Lcg::new(99);
    let (w, h) = (24u32, 3u32);
    let quads = normalised_quads(&mut rng, (w * h) as usize);
    let header = HdrHeader {
        exposure: Some(1.5),
        gamma: Some(2.2),
        pixaspect: Some(0.75),
        colorcorr: Some([1.1, 1.0, 0.9]),
        primaries: Some(Primaries::REC2020),
        software: Some("oxideav-hdr/matrix".to_owned()),
        view: Some("rpict -vp 0 0 5 -vd 0 0 -1".to_owned()),
        y_sign: AxisSign::Decreasing,
        ..HdrHeader::default()
    };
    let img = HdrImage::from_rgbe_quads(w, h, &quads, header);
    let bytes = encode_hdr_with_options(&img, RleMode::New, LineEnding::Lf).unwrap();
    let back = parse_hdr_with_options(&bytes, FallbackMode::OldRle).unwrap();

    assert_eq!(back.to_rgbe_quads(), quads, "quad drift with typed records");
    assert_eq!(back.header.exposure, Some(1.5));
    assert_eq!(back.header.gamma, Some(2.2));
    assert_eq!(back.header.pixaspect, Some(0.75));
    assert_eq!(back.header.colorcorr, Some([1.1, 1.0, 0.9]));
    assert_eq!(back.header.software.as_deref(), Some("oxideav-hdr/matrix"));
    assert_eq!(
        back.header.view.as_deref(),
        Some("rpict -vp 0 0 5 -vd 0 0 -1")
    );
    let p = back.header.primaries.expect("PRIMARIES lost");
    assert!((p.red.0 - 0.708).abs() < 1e-4);
}
