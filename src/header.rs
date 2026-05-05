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

/// Header carried inside [`crate::HdrImage`]. Everything optional has
/// `Option<…>` so `Default` produces something writable.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrHeader {
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
            format: HdrFormat::Rgbe,
            exposure: None,
            gamma: None,
            software: None,
            pixaspect: None,
            other: Vec::new(),
            comments: Vec::new(),
            y_sign: AxisSign::Decreasing,
            x_sign: AxisSign::Increasing,
            x_first: false,
        }
    }
}
