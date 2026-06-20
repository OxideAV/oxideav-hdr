//! End-to-end exercise of the geometric reorientation subsystem through
//! the shipped public re-exports.
//!
//! Lives outside `src/` so it pins the public `GeometricOp` /
//! `HdrImage::{apply_geometric, to_orientation, normalize_from, reorient}`
//! surface and catches visibility regressions. The geometry is asserted
//! against an independent coordinate model, and a final arm threads a
//! reoriented picture through the real `encode_hdr` -> `parse_hdr` wire
//! path to prove the transform survives a byte-level round-trip.

use oxideav_hdr::{encode_hdr, parse_hdr, GeometricOp, HdrImage, HdrPixelFormat, Orientation};

const ALL_ORIENT: [Orientation; 8] = [
    Orientation::Standard,
    Orientation::FlipX,
    Orientation::Rotate180,
    Orientation::FlipY,
    Orientation::Rotate90Cw,
    Orientation::Rotate90CwFlipY,
    Orientation::Rotate90Ccw,
    Orientation::Rotate90CcwFlipY,
];

/// Build a `w x h` picture whose every pixel encodes its own `(x, y)` in
/// the R/G channels so a geometric permutation is directly auditable. Used
/// for the in-memory arms; the encode/decode arm uses normalised RGBE
/// quads instead so its assertions can be bit-exact.
fn coord_image(w: u32, h: u32) -> HdrImage {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            pixels.push(x as f32);
            pixels.push(y as f32);
            pixels.push(1.0);
        }
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

/// Coordinate ground truth: (dst_x, dst_y, out_w, out_h) for a source
/// `(x, y)` of a `w x h` picture under `op`.
fn model(op: GeometricOp, x: i64, y: i64, w: i64, h: i64) -> (i64, i64, i64, i64) {
    match op {
        GeometricOp::Identity => (x, y, w, h),
        GeometricOp::FlipHorizontal => (w - 1 - x, y, w, h),
        GeometricOp::FlipVertical => (x, h - 1 - y, w, h),
        GeometricOp::Rotate180 => (w - 1 - x, h - 1 - y, w, h),
        GeometricOp::Rotate90Cw => (h - 1 - y, x, h, w),
        GeometricOp::Rotate90Ccw => (y, w - 1 - x, h, w),
        GeometricOp::Transpose => (y, x, h, w),
        GeometricOp::AntiTranspose => (h - 1 - y, w - 1 - x, h, w),
    }
}

fn px(img: &HdrImage, x: u32, y: u32) -> [f32; 3] {
    let off = ((y * img.width + x) * 3) as usize;
    [img.pixels[off], img.pixels[off + 1], img.pixels[off + 2]]
}

#[test]
fn public_apply_geometric_matches_model_for_every_op() {
    let (w, h) = (5u32, 3u32);
    for op in GeometricOp::ALL {
        let mut img = coord_image(w, h);
        img.apply_geometric(op);
        let (_, _, mw, mh) = model(op, 0, 0, w as i64, h as i64);
        assert_eq!((img.width as i64, img.height as i64), (mw, mh), "{op:?}");
        assert_eq!(img.pixel_format, HdrPixelFormat::Rgb96f);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy, _, _) = model(op, x as i64, y as i64, w as i64, h as i64);
                let got = px(&img, dx as u32, dy as u32);
                assert_eq!([got[0] as u32, got[1] as u32], [x, y], "{op:?} ({x},{y})");
            }
        }
    }
}

#[test]
fn public_reorient_covers_full_orientation_matrix() {
    let original = coord_image(4, 6);
    for from in ALL_ORIENT {
        for to in ALL_ORIENT {
            // reorient must equal normalize-then-render.
            let mut a = original.clone();
            a.reorient(from, to);
            let mut b = original.clone();
            b.normalize_from(from);
            b.to_orientation(to);
            assert_eq!((a.width, a.height), (b.width, b.height), "{from:?}->{to:?}");
            assert_eq!(a.pixels, b.pixels, "{from:?}->{to:?}");
        }
    }
}

#[test]
fn public_op_then_inverse_restores_picture() {
    let original = coord_image(7, 2);
    for op in GeometricOp::ALL {
        let mut img = original.clone();
        img.apply_geometric(op);
        img.apply_geometric(op.inverse());
        assert_eq!(
            (img.width, img.height),
            (original.width, original.height),
            "{op:?}"
        );
        assert_eq!(img.pixels, original.pixels, "{op:?}");
    }
}

#[test]
fn reorient_survives_encode_decode_round_trip() {
    // Bit-exact arm: build a picture from normalised RGBE quads (so the
    // shared-exponent codec is idempotent on it), reorient it, encode to
    // the Radiance wire format, decode, and assert the decoded buffer
    // matches the geometrically-transformed quads the model predicts.
    //
    // Quads use a dominant mantissa >= 128 and a mid-range exponent so
    // `rgb_to_rgbe(rgbe_to_rgb(q)) == q` for each one (the normalised
    // subset documented on the crate's bit-exact round-trip surface).
    // Both dimensions stay >= 8 after a transpose so the default new-RLE
    // encoder's minimum-scanline-width constraint holds in every
    // orientation; the off-square shape still makes a stray transpose
    // visible.
    let (w, h) = (11u32, 9u32);
    let mut quads = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            // Encode (x, y) into two mantissas (both >= 128) so each
            // recovered pixel is unique and the permutation is auditable.
            let r = 128u8 + (x as u8);
            let g = 128u8 + (y as u8);
            quads.push([r, g, 200u8, 130u8]);
        }
    }
    let header = oxideav_hdr::HdrHeader::default();
    let base = HdrImage::from_rgbe_quads(w, h, &quads, header);

    for op in GeometricOp::ALL {
        let mut img = base.clone();
        img.apply_geometric(op);

        let bytes = encode_hdr(&img).expect("encode reoriented picture");
        let back = parse_hdr(&bytes).expect("decode reoriented picture");

        let (_, _, mw, mh) = model(op, 0, 0, w as i64, h as i64);
        assert_eq!(
            (back.width as i64, back.height as i64),
            (mw, mh),
            "{op:?}: dims"
        );

        // The decoded quads must equal the model-permuted source quads.
        let got = back.to_rgbe_quads();
        let owu = mw as u32;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy, _, _) = model(op, x as i64, y as i64, w as i64, h as i64);
                let src_idx = (y * w + x) as usize;
                let dst_idx = (dy as u32 * owu + dx as u32) as usize;
                assert_eq!(
                    got[dst_idx], quads[src_idx],
                    "{op:?}: source quad ({x},{y}) expected at ({dx},{dy})",
                );
            }
        }
    }
}
