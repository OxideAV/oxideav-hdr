//! Radiance HDR header — the magic line, the `KEY=VALUE` metadata
//! list (terminated by an empty line), and the resolution line that
//! follows it.
//!
//! Format references (clean-room, public spec only):
//! * Greg Ward's "Real Pixels" (Graphics Gems II, 1991) for the
//!   shared-exponent pixel rationale.
//! * The radsite.lbl.gov "RADIANCE Reference Manual" appendix on the
//!   `.pic` / `.hdr` file format for the header grammar.
//!
//! Magic line is one of:
//! ```text
//! #?RADIANCE
//! #?RGBE
//! ```
//! followed by `\n`.
//!
//! Then 0+ records of the form `KEY=VALUE\n`, plus optional comment
//! lines beginning with `#`. The list is terminated by a single empty
//! line (`\n` on its own).
//!
//! Then the resolution line — exactly one of the eight axis-flag
//! orderings:
//! ```text
//! -Y H +X W       (most common — top-down rows, left-to-right cols)
//! +Y H +X W
//! -Y H -X W
//! +Y H -X W
//! +X W -Y H
//! +X W +Y H
//! -X W -Y H
//! -X W +Y H
//! ```
//! The "Y" axis value is the row count, the "X" axis value is the
//! column count. Sign `+` means "increases as the index advances"; `-`
//! means "decreases". So `-Y` means the first scanline corresponds to
//! the largest Y value (top of image), which matches the standard
//! top-down memory order.

/// Recognised values of the `FORMAT=` header record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrFormat {
    /// `32-bit_rle_rgbe` — the standard RGB shared-exponent encoding.
    Rgbe,
    /// `32-bit_rle_xyze` — CIE XYZ shared-exponent encoding. Decoded
    /// to f32 channels just like RGBE; downstream code is responsible
    /// for any colour conversion it might want.
    Xyze,
}

impl HdrFormat {
    /// String form as it appears on disk after `FORMAT=`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rgbe => "32-bit_rle_rgbe",
            Self::Xyze => "32-bit_rle_xyze",
        }
    }
}

/// Sign on a resolution-line axis flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisSign {
    /// `+` — index increases with array position.
    Increasing,
    /// `-` — index decreases with array position.
    Decreasing,
}

impl AxisSign {
    /// Render as the literal `+` or `-` byte the resolution line uses.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Increasing => "+",
            Self::Decreasing => "-",
        }
    }
}

/// CIE chromaticity coordinates carried in a `PRIMARIES=` record.
///
/// Radiance's `PRIMARIES` header tag is eight space-separated floats:
/// `Rx Ry Gx Gy Bx By Wx Wy`. Each `(x, y)` is the CIE 1931 xy
/// chromaticity for one of the three primaries or the reference white.
/// The two missing components are `Rz = 1 - Rx - Ry`, …; full XYZ
/// values follow by post-scaling the primaries onto the white point
/// (the construction in BT.709 §3 / IEC 61966-2-1 Annex C).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primaries {
    /// `(x, y)` chromaticity of the red primary.
    pub red: (f32, f32),
    /// `(x, y)` chromaticity of the green primary.
    pub green: (f32, f32),
    /// `(x, y)` chromaticity of the blue primary.
    pub blue: (f32, f32),
    /// `(x, y)` chromaticity of the reference white point.
    pub white: (f32, f32),
}

impl Primaries {
    /// sRGB / Rec. 709 primaries with a D65 reference white, as
    /// standardised in IEC 61966-2-1 Annex C / BT.709-6 §3.
    pub const SRGB: Self = Self {
        red: (0.640, 0.330),
        green: (0.300, 0.600),
        blue: (0.150, 0.060),
        white: (0.3127, 0.3290),
    };

    /// Greg Ward's original Radiance RGB primaries with an equal-energy
    /// (E) reference white. These are the values the reference
    /// `ra_xyze` tool uses when a `PRIMARIES=` record is absent.
    pub const RADIANCE: Self = Self {
        red: (0.640, 0.330),
        green: (0.290, 0.600),
        blue: (0.150, 0.060),
        white: (1.0 / 3.0, 1.0 / 3.0),
    };

    /// DCI-P3 with a D65 reference white — the wide-gamut RGB space
    /// most consumer HDR displays (Apple "Display P3", Android Display
    /// P3) target. Primaries per SMPTE RP 431-2 (D-Cinema reference
    /// projector) with the white point swapped from DCI to D65 per the
    /// Display P3 specification used by sRGB-replacement HDR pipelines.
    pub const P3_D65: Self = Self {
        red: (0.680, 0.320),
        green: (0.265, 0.690),
        blue: (0.150, 0.060),
        white: (0.3127, 0.3290),
    };

    /// ITU-R BT.2020 / Rec.2020 ultra-wide-gamut primaries with a D65
    /// reference white. The colour space used by HDR10 / HLG TV
    /// production. Values per ITU-R BT.2020-2 §2 Table 4.
    pub const REC2020: Self = Self {
        red: (0.708, 0.292),
        green: (0.170, 0.797),
        blue: (0.131, 0.046),
        white: (0.3127, 0.3290),
    };

    /// Format as the eight-float space-separated string the on-disk
    /// `PRIMARIES=` record uses.
    pub fn to_record_string(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {}",
            self.red.0,
            self.red.1,
            self.green.0,
            self.green.1,
            self.blue.0,
            self.blue.1,
            self.white.0,
            self.white.1,
        )
    }

    /// Parse the eight-float value of a `PRIMARIES=` record. Returns
    /// `None` if the record doesn't have exactly eight floats.
    pub fn from_record_str(value: &str) -> Option<Self> {
        let parts: Vec<f32> = value
            .split_whitespace()
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        if parts.len() != 8 {
            return None;
        }
        Some(Self {
            red: (parts[0], parts[1]),
            green: (parts[2], parts[3]),
            blue: (parts[4], parts[5]),
            white: (parts[6], parts[7]),
        })
    }
}

/// Header carried inside [`crate::HdrImage`]. Everything optional has
/// `Option<…>` so `Default` produces something writable.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrHeader {
    /// The identifier carried on the `#?…` magic line, with the leading
    /// `#?` stripped — e.g. `"RADIANCE"`, `"RGBE"`, or the name of the
    /// program that wrote the file. The staged format note documents the
    /// header magic as the two-byte string `#?` followed by a
    /// caller-supplied identifier (`newheader(s)` writes `#?` then `s`),
    /// so any non-empty token after `#?` is a valid magic line — not just
    /// the two canonical spellings. The decoder preserves whatever it
    /// read here so a re-encode can reproduce the original identifier
    /// verbatim; `None` on a `Default` header means "let the encoder pick
    /// its default identifier".
    pub magic_id: Option<String>,
    /// `FORMAT=` value. Defaults to [`HdrFormat::Rgbe`].
    pub format: HdrFormat,
    /// `EXPOSURE=` (cumulative; see Radiance docs for the multiplicative
    /// stacking rule when multiple records appear).
    pub exposure: Option<f32>,
    /// `GAMMA=` value.
    pub gamma: Option<f32>,
    /// `SOFTWARE=` line.
    pub software: Option<String>,
    /// `PIXASPECT=` value.
    pub pixaspect: Option<f32>,
    /// `VIEW=` record. Free-form camera / view-parameter string written
    /// by the Radiance renderer (`-vp`, `-vd`, `-vu`, `-vh`, `-vv`, …
    /// flags concatenated). The reference manual documents the record
    /// as caller-defined text — we preserve the value verbatim and
    /// leave any tokenisation to the consumer.
    pub view: Option<String>,
    /// `COLORCORR=` three-float per-channel correction. The Radiance
    /// reference manual defines it as a multiplicative scale applied to
    /// the float channels on the way out of decode (separately from
    /// EXPOSURE, which it does not stack into); we parse and round-trip
    /// it but leave honouring it to the tone-mapper / display path.
    pub colorcorr: Option<[f32; 3]>,
    /// `PRIMARIES=` chromaticity coordinates. Defaults to `None`, in
    /// which case consumers should assume Radiance's default RGB
    /// primaries.
    pub primaries: Option<Primaries>,
    /// Free-form `KEY=VALUE` records that didn't match a typed slot
    /// above. Preserved in the order they were read.
    pub other: Vec<(String, String)>,
    /// Comment lines (starting with `#`) sandwiched in the header,
    /// excluding the leading `#?…` magic line. Stored without the
    /// leading `#`.
    pub comments: Vec<String>,
    /// Sign on the Y (row-direction) axis flag in the resolution line.
    /// Defaults to [`AxisSign::Decreasing`] which gives the standard
    /// top-down `-Y H +X W` layout.
    pub y_sign: AxisSign,
    /// Sign on the X (column-direction) axis flag.
    pub x_sign: AxisSign,
    /// True when the resolution line lists the X axis before the Y
    /// axis (`+X W -Y H` etc.). Defaults to false (Y-first).
    pub x_first: bool,
}

impl Default for HdrHeader {
    fn default() -> Self {
        Self {
            magic_id: None,
            format: HdrFormat::Rgbe,
            exposure: None,
            gamma: None,
            software: None,
            pixaspect: None,
            view: None,
            colorcorr: None,
            primaries: None,
            other: Vec::new(),
            comments: Vec::new(),
            y_sign: AxisSign::Decreasing,
            x_sign: AxisSign::Increasing,
            x_first: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_record_string_roundtrips() {
        let p = Primaries::SRGB;
        let s = p.to_record_string();
        let back = Primaries::from_record_str(&s).unwrap();
        assert!((back.red.0 - p.red.0).abs() < 1e-5);
        assert!((back.green.1 - p.green.1).abs() < 1e-5);
        assert!((back.white.0 - p.white.0).abs() < 1e-5);
    }

    #[test]
    fn primaries_rejects_short_record() {
        assert!(Primaries::from_record_str("0.64 0.33 0.30 0.60").is_none());
    }

    #[test]
    fn p3_d65_constants_match_spec() {
        // SMPTE RP 431-2 primaries with white point swapped to D65 per
        // the Display P3 spec.
        let p = Primaries::P3_D65;
        assert!((p.red.0 - 0.680).abs() < 1e-4);
        assert!((p.red.1 - 0.320).abs() < 1e-4);
        assert!((p.green.0 - 0.265).abs() < 1e-4);
        assert!((p.green.1 - 0.690).abs() < 1e-4);
        assert!((p.blue.0 - 0.150).abs() < 1e-4);
        assert!((p.blue.1 - 0.060).abs() < 1e-4);
        assert!((p.white.0 - 0.3127).abs() < 1e-4);
        assert!((p.white.1 - 0.3290).abs() < 1e-4);
    }

    #[test]
    fn rec2020_constants_match_spec() {
        // ITU-R BT.2020-2 Table 4.
        let p = Primaries::REC2020;
        assert!((p.red.0 - 0.708).abs() < 1e-4);
        assert!((p.red.1 - 0.292).abs() < 1e-4);
        assert!((p.green.0 - 0.170).abs() < 1e-4);
        assert!((p.green.1 - 0.797).abs() < 1e-4);
        assert!((p.blue.0 - 0.131).abs() < 1e-4);
        assert!((p.blue.1 - 0.046).abs() < 1e-4);
        assert!((p.white.0 - 0.3127).abs() < 1e-4);
        assert!((p.white.1 - 0.3290).abs() < 1e-4);
    }

    #[test]
    fn p3_d65_roundtrips_via_record_string() {
        let p = Primaries::P3_D65;
        let s = p.to_record_string();
        let back = Primaries::from_record_str(&s).unwrap();
        assert!((back.red.0 - p.red.0).abs() < 1e-5);
        assert!((back.green.0 - p.green.0).abs() < 1e-5);
        assert!((back.blue.0 - p.blue.0).abs() < 1e-5);
        assert!((back.white.0 - p.white.0).abs() < 1e-5);
    }
}
