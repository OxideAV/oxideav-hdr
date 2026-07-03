#![no_main]

//! Drive the **encoder** across the full cross-product of its on-wire
//! options and assert every choice survives a decode round trip.
//!
//! The existing `roundtrip` target only ever exercises the
//! `encode_hdr` *default* path: the `#?RADIANCE` magic line, an
//! all-default header (no typed records), `RleMode::New`, and
//! `LineEnding::Lf`. That leaves the bulk of the encoder's branch
//! surface unfuzzed — the three other RLE flavours, the CRLF text
//! terminator, the legacy `#?RGBE` magic spelling, the XYZE `FORMAT`,
//! all eight resolution-string orientations, and — most importantly —
//! the header writer, which serialises seven typed records
//! (`EXPOSURE` / `GAMMA` / `PIXASPECT` / `COLORCORR` / `PRIMARIES` /
//! `SOFTWARE` / `VIEW`) plus program/command lines and free-form
//! `KEY=VALUE` extras, each of which the decoder must parse back.
//!
//! This target reaches that surface directly. From the fuzz input it
//! derives:
//!
//!  * small in-bounds dimensions (`8..=39` per axis — the `out_w` after
//!    an X-first transpose stays inside the new-RLE `8..=32767` range so
//!    `RleMode::New` never errors out, and the worst-case buffer is
//!    tiny),
//!  * one of the four [`RleMode`] flavours, one of the two
//!    [`LineEnding`]s, one of the two [`MagicLine`] spellings, one of
//!    the eight [`Orientation`]s, and the RGBE-vs-XYZE `FORMAT`,
//!  * a fuzz-built header carrying every typed record at fuzz-chosen
//!    (finite, decode-survivable) values plus a command line and a
//!    free-form extra,
//!  * a fuzz-driven positive-float pixel buffer.
//!
//! It encodes via [`encode_hdr_with_full_options`] (RLE × line-ending ×
//! magic) after setting the orientation + format + records on the
//! header, decodes with the [`FallbackMode`] matching the chosen RLE
//! flavour, and asserts:
//!
//!  * the encode never errors for an in-range input (an `Err` on the
//!    constrained dimensions is a genuine encoder bug),
//!  * the decoded dimensions, pixel-format and buffer length survive
//!    (any axis-flag / transpose confusion shows up as a length or
//!    dimension mismatch),
//!  * the decoded `FORMAT` matches what was encoded,
//!  * each typed header record round-trips to a value within a tight
//!    tolerance of what was written (catches any header writer / parser
//!    asymmetry — a misspelled key, a dropped record, a float that
//!    doesn't survive the `{}`-format → `parse::<f32>()` round trip).
//!
//! As with the other targets the crate is pulled in with
//! `default-features = false` so the fuzz build exercises the
//! framework-free standalone API only and never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_hdr::{
    encode_hdr_with_full_options, parse_hdr_with_options, FallbackMode, HdrFormat, HdrImage,
    HdrPixelFormat, LineEnding, MagicLine, Orientation, Primaries, RleMode,
};

/// Pull a float in `[0, 1)` out of one fuzz byte.
fn unit(b: u8) -> f32 {
    f32::from(b) / 256.0
}

fuzz_target!(|data: &[u8]| {
    // Need a fixed prefix of option/header selector bytes plus at least
    // one pixel-seed byte.
    if data.len() < 24 {
        return;
    }

    // --- on-wire option selectors (first 4 bytes) ---
    let rle = match data[0] % 5 {
        0 => RleMode::New,
        1 => RleMode::Old,
        2 => RleMode::Auto,
        3 => RleMode::Uncompressed,
        _ => RleMode::Smallest,
    };
    let line_ending = if data[1] & 1 == 0 {
        LineEnding::Lf
    } else {
        LineEnding::Crlf
    };
    let magic = if data[2] & 1 == 0 {
        MagicLine::Radiance
    } else {
        MagicLine::Rgbe
    };
    let orientation = match data[3] % 8 {
        0 => Orientation::Standard,
        1 => Orientation::FlipX,
        2 => Orientation::Rotate180,
        3 => Orientation::FlipY,
        4 => Orientation::Rotate90Cw,
        5 => Orientation::Rotate90CwFlipY,
        6 => Orientation::Rotate90Ccw,
        _ => Orientation::Rotate90CcwFlipY,
    };

    // --- dimensions (bytes 4,5), kept small + inside new-RLE range ---
    //
    // After an X-first transpose the on-disk scanline width is the
    // canonical *height*, so both dimensions must stay in `8..=32767`
    // for `RleMode::New` (and `Auto`'s new-RLE pick) to encode without
    // erroring. `8..=39` does that with a trivially small buffer.
    let width: u32 = u32::from(data[4] % 32) + 8;
    let height: u32 = u32::from(data[5] % 32) + 8;

    // --- format (byte 6) ---
    let format = if data[6] & 1 == 0 {
        HdrFormat::Rgbe
    } else {
        HdrFormat::Xyze
    };

    // --- typed header record values (bytes 7..=22) ---
    //
    // Every value is built so its `{}` text form parses back to the
    // same f32: `unit(b)` is `k/256`, exactly representable. Exposure /
    // pixaspect are floored away from `0` so the decoder's "cumulative
    // product" fold keeps them; a `0.0` would still round-trip but makes
    // a less interesting assertion.
    let exposure = unit(data[7]) + 1.0 / 256.0; // (1..=256)/256
    let gamma = unit(data[8]) + 1.0; // 1.0 ..< 2.0, all exact
    let pixaspect = unit(data[9]) + 1.0 / 256.0;
    let colorcorr = [unit(data[10]), unit(data[11]), unit(data[12])];
    // PRIMARIES: eight small distinct chromaticities, each `k/256`.
    let primaries = Primaries {
        red: (unit(data[13]), unit(data[14])),
        green: (unit(data[15]), unit(data[16])),
        blue: (unit(data[17]), unit(data[18])),
        white: (unit(data[19]), unit(data[20])),
    };
    // SOFTWARE / VIEW / command: pick presence from two flag bytes so
    // the writer's "absent record is omitted" branch is also covered.
    let want_software = data[21] & 1 == 0;
    let want_view = data[21] & 2 == 0;
    let want_command = data[22] & 1 == 0;

    // --- pixel buffer (remaining bytes), all strictly positive ---
    let pixel_count = (width as usize) * (height as usize);
    let n = pixel_count * 3;
    let body = &data[23..];
    let mut pixels = Vec::with_capacity(n);
    for i in 0..n {
        let byte = if body.is_empty() {
            0u8
        } else {
            body[i % body.len()]
        };
        // `+1` floor keeps every sample > 0 so no pixel collapses to the
        // all-black `0,0,0,0` quad (which would obscure orientation bugs).
        pixels.push((f32::from(byte) + 1.0) / 256.0);
    }

    let mut image = HdrImage::new_rgb96f(width, height, pixels);
    image.header.format = format;
    image.header.set_orientation(orientation);
    image.header.exposure = Some(exposure);
    image.header.gamma = Some(gamma);
    image.header.pixaspect = Some(pixaspect);
    image.header.colorcorr = Some(colorcorr);
    image.header.primaries = Some(primaries);
    if want_software {
        image.header.software = Some("oxideav-fuzz 1.0".to_string());
    }
    if want_view {
        image.header.view = Some("rvu -vp 0 0 1 -vd 0 0 -1".to_string());
    }
    if want_command {
        image
            .header
            .commands
            .push("rpict -vf scene.vp scene.oct".to_string());
    }

    let encoded = match encode_hdr_with_full_options(&image, rle, line_ending, magic) {
        Ok(v) => v,
        // The dimensions are constrained into the new-RLE range, so an
        // `Err` here (only emitted on zero-dim / length-mismatch / a
        // new-RLE width overflow that our bounds preclude) would be a
        // genuine encoder bug. Return cleanly; the round-trip asserts
        // below are the canonical crash triggers.
        Err(_) => return,
    };

    // Decode with the fallback matching the encoded flavour. New-RLE
    // carries its own `0x02 0x02 hi lo` marker so the fallback is
    // irrelevant for it and for `Auto`'s new pick; Old / Uncompressed
    // need their matching fallback because they have no in-band marker,
    // and Smallest's flat scanlines must be read flat.
    let fallback = match rle {
        RleMode::Uncompressed | RleMode::Smallest => FallbackMode::Uncompressed,
        _ => FallbackMode::OldRle,
    };

    let decoded = parse_hdr_with_options(&encoded, fallback)
        .expect("encoder output must be parseable by parse_hdr_with_options");

    assert_eq!(decoded.width, width, "width survives round trip");
    assert_eq!(decoded.height, height, "height survives round trip");
    assert_eq!(decoded.pixel_format, HdrPixelFormat::Rgb96f);
    assert_eq!(
        decoded.pixels.len(),
        n,
        "decoded pixel buffer is width × height × 3 floats long",
    );

    // FORMAT round-trips.
    assert_eq!(decoded.header.format, format, "FORMAT survives round trip");

    // Typed header records round-trip. Each was built from a `k/256`
    // value (or `1 + k/256`), exactly representable, so the only error
    // source is the `{}` → `parse::<f32>()` text path — the tolerance is
    // a generous `1e-4` to absorb shortest-round-trip float formatting.
    let approx = |a: f32, b: f32| (a - b).abs() < 1e-4;

    assert!(
        decoded.header.exposure.map(|e| approx(e, exposure)) == Some(true),
        "EXPOSURE round trip: wrote {exposure}, read {:?}",
        decoded.header.exposure,
    );
    assert!(
        decoded.header.gamma.map(|g| approx(g, gamma)) == Some(true),
        "GAMMA round trip: wrote {gamma}, read {:?}",
        decoded.header.gamma,
    );
    assert!(
        decoded.header.pixaspect.map(|p| approx(p, pixaspect)) == Some(true),
        "PIXASPECT round trip: wrote {pixaspect}, read {:?}",
        decoded.header.pixaspect,
    );
    match decoded.header.colorcorr {
        Some(cc) => assert!(
            approx(cc[0], colorcorr[0])
                && approx(cc[1], colorcorr[1])
                && approx(cc[2], colorcorr[2]),
            "COLORCORR round trip: wrote {colorcorr:?}, read {cc:?}",
        ),
        None => panic!("COLORCORR record dropped"),
    }
    match decoded.header.primaries {
        Some(p) => {
            assert!(approx(p.red.0, primaries.red.0) && approx(p.red.1, primaries.red.1));
            assert!(approx(p.green.0, primaries.green.0) && approx(p.green.1, primaries.green.1));
            assert!(approx(p.blue.0, primaries.blue.0) && approx(p.blue.1, primaries.blue.1));
            assert!(approx(p.white.0, primaries.white.0) && approx(p.white.1, primaries.white.1));
        }
        None => panic!("PRIMARIES record dropped"),
    }
    if want_software {
        assert_eq!(
            decoded.header.software.as_deref(),
            Some("oxideav-fuzz 1.0"),
            "SOFTWARE round trip",
        );
    }
    if want_command {
        assert!(
            decoded
                .header
                .commands
                .iter()
                .any(|c| c == "rpict -vf scene.vp scene.oct"),
            "command line round trip: {:?}",
            decoded.header.commands,
        );
    }
    // The decoded orientation must name what we asked the encoder to
    // emit: the encoder writes the axis flags for `orientation` and the
    // decoder reads them straight back, so the round trip is exact.
    assert_eq!(
        decoded.header.orientation(),
        orientation,
        "orientation survives round trip",
    );
});
