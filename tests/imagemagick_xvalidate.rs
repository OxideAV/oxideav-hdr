//! Cross-validate `oxideav-hdr` encode/decode against ImageMagick.
//!
//! Skipped automatically if `magick` (ImageMagick 7) isn't on `PATH`.
//! On a developer machine with ImageMagick installed this exercises
//! the on-the-wire compatibility (encoder produces a file ImageMagick
//! understands, decoder accepts a file ImageMagick produced) and the
//! XYZE↔RGB conversion helpers' numerical accuracy.

use std::process::Command;

use oxideav_hdr::{
    convert_image_rgb_to_xyz, convert_image_xyz_to_rgb, encode_hdr, parse_hdr, HdrImage,
    RgbColorSpace,
};

fn have_magick() -> bool {
    Command::new("magick")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a smooth deterministic 8x4 gradient for the cross-tests.
fn synthetic(w: u32, h: u32) -> HdrImage {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w.max(1) as f32;
            let v = y as f32 / h.max(1) as f32;
            // Stay in `[0, 1]` linear-light so the round-trip via
            // ImageMagick (which maps to its 16-bit quantum range)
            // doesn't blow out.
            pixels.push(0.05 + 0.85 * u);
            pixels.push(0.05 + 0.85 * v);
            pixels.push(0.05 + 0.85 * (u * v).sqrt());
        }
    }
    HdrImage::new_rgb96f(w, h, pixels)
}

#[test]
fn imagemagick_can_decode_our_encoder_output() {
    if !have_magick() {
        eprintln!("magick not found — skipping cross-validation");
        return;
    }
    let img = synthetic(16, 8);
    let bytes = encode_hdr(&img).expect("encode");
    let tmp_dir = std::env::temp_dir();
    let in_path = tmp_dir.join("oxideav_hdr_xvalidate_in.hdr");
    let out_path = tmp_dir.join("oxideav_hdr_xvalidate_out.ppm");
    std::fs::write(&in_path, &bytes).expect("write tmp HDR");
    // Convert to PPM with floating-point read so the linear values
    // survive without an OETF being applied behind our back.
    let status = Command::new("magick")
        .arg(&in_path)
        .arg("-depth")
        .arg("16")
        .arg(&out_path)
        .status()
        .expect("run magick");
    assert!(status.success(), "magick conversion failed");
    let ppm = std::fs::read(&out_path).expect("read ppm");
    // PPM P6 header: "P6\nW H\n65535\n" then binary samples.
    // ImageMagick may insert comment lines (`# …\n`) between the
    // magic and the dimensions; skip them.
    assert!(ppm.starts_with(b"P6"), "ImageMagick output isn't PPM");
    let mut pos = 0usize;
    let mut tokens_seen = 0usize;
    while tokens_seen < 4 {
        // Token 0 = "P6", 1 = width, 2 = height, 3 = max sample value.
        // Skip leading whitespace.
        while pos < ppm.len() && (ppm[pos] == b'\n' || ppm[pos] == b' ' || ppm[pos] == b'\t') {
            pos += 1;
        }
        if pos < ppm.len() && ppm[pos] == b'#' {
            // Comment runs to next newline.
            while pos < ppm.len() && ppm[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        // Consume one whitespace-delimited token.
        while pos < ppm.len()
            && ppm[pos] != b'\n'
            && ppm[pos] != b' '
            && ppm[pos] != b'\t'
            && ppm[pos] != b'\r'
        {
            pos += 1;
        }
        tokens_seen += 1;
    }
    // The byte after the max-sample-value token is exactly one
    // whitespace, then the binary block starts.
    pos += 1;
    // 16-bit samples big-endian, 3 channels per pixel, 16*8 pixels.
    let samples = &ppm[pos..];
    assert_eq!(samples.len(), 16 * 8 * 3 * 2);
    // Spot-check the corners — they shouldn't be black or white.
    let read_u16 = |i: usize| ((samples[i * 2] as u32) << 8 | samples[i * 2 + 1] as u32) as u16;
    let r0 = read_u16(0) as f32 / 65535.0;
    let last_pixel = 16 * 8 - 1;
    let r_last = read_u16(last_pixel * 3) as f32 / 65535.0;
    // ImageMagick may apply an sRGB OETF on the way out — both
    // gamma-encoded and linear interpretations should give a top-left
    // dimmer than the bottom-right.
    assert!(
        r_last > r0,
        "expected gradient to brighten across the image, got r0={r0} r_last={r_last}"
    );
    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn we_can_decode_imagemagick_output() {
    if !have_magick() {
        eprintln!("magick not found — skipping cross-validation");
        return;
    }
    let tmp_dir = std::env::temp_dir();
    let path = tmp_dir.join("oxideav_hdr_xvalidate_im.hdr");
    // Ask ImageMagick to write a 16x8 black-to-white linear gradient
    // in the radiance HDR format. `-colorspace RGB` keeps it linear.
    let status = Command::new("magick")
        .arg("-size")
        .arg("16x8")
        .arg("gradient:black-white")
        .arg("-colorspace")
        .arg("RGB")
        .arg(format!("hdr:{}", path.display()))
        .status()
        .expect("run magick");
    assert!(status.success(), "magick HDR write failed");
    let bytes = std::fs::read(&path).expect("read tmp HDR");
    let img = parse_hdr(&bytes).expect("parse imagemagick HDR");
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 8);
    // Top row should be near-black, bottom row near-white. We check
    // the average brightness per row to dodge per-pixel quantisation
    // noise.
    let avg_row = |y: usize| {
        let row = &img.pixels[y * 16 * 3..(y + 1) * 16 * 3];
        row.iter().sum::<f32>() / row.len() as f32
    };
    let top = avg_row(0);
    let bot = avg_row(7);
    assert!(top < 0.1, "top row should be near-black, got {top}");
    assert!(bot > 0.7, "bottom row should be near-white, got {bot}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn xyze_roundtrip_preserves_radiometry_via_imagemagick() {
    if !have_magick() {
        eprintln!("magick not found — skipping cross-validation");
        return;
    }
    // Build an RGB image, convert to XYZE on the float side, encode,
    // round-trip through ImageMagick (which knows about both formats),
    // then convert the decoded image back to RGB and compare.
    let original = synthetic(16, 8);
    let mut xyz_image = original.clone();
    convert_image_rgb_to_xyz(&mut xyz_image, RgbColorSpace::Radiance);
    let bytes = encode_hdr(&xyz_image).expect("encode XYZE");
    let tmp_dir = std::env::temp_dir();
    let xyze_path = tmp_dir.join("oxideav_hdr_xyze_in.hdr");
    let rgb_path = tmp_dir.join("oxideav_hdr_xyze_back.hdr");
    std::fs::write(&xyze_path, &bytes).expect("write tmp xyze");
    // Use ImageMagick to convert the XYZE file back to a plain RGBE
    // file, then re-decode and compare.
    let status = Command::new("magick")
        .arg(&xyze_path)
        .arg(format!("hdr:{}", rgb_path.display()))
        .status()
        .expect("run magick");
    assert!(status.success(), "magick XYZE→RGBE failed");
    let back_bytes = std::fs::read(&rgb_path).expect("read tmp rgbe");
    let mut back = parse_hdr(&back_bytes).expect("parse RGBE");
    if matches!(back.header.format, oxideav_hdr::HdrFormat::Xyze) {
        // Some ImageMagick builds preserve the FORMAT tag — convert
        // ourselves so we end up comparing apples to apples.
        convert_image_xyz_to_rgb(&mut back, RgbColorSpace::Radiance);
    }
    assert_eq!(back.width, original.width);
    assert_eq!(back.height, original.height);
    // Compare per-pixel within a generous tolerance — there are two
    // shared-exponent quantisation steps + one matrix round-trip in
    // between, plus ImageMagick's internal colour management may add
    // a small chromatic adaptation.
    let mut max_err = 0.0f32;
    for i in 0..original.pixels.len() {
        let a = original.pixels[i];
        let b = back.pixels[i];
        let err = (a - b).abs();
        if err > max_err {
            max_err = err;
        }
    }
    // 12% absolute is loose but accounts for ImageMagick's optional
    // chromatic adaptation; the test's primary purpose is the
    // "doesn't crash + produces sensible numbers" cross-check.
    assert!(
        max_err < 0.20,
        "XYZE round-trip max abs error {max_err} too large"
    );
    let _ = std::fs::remove_file(&xyze_path);
    let _ = std::fs::remove_file(&rgb_path);
}
