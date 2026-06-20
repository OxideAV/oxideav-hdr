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

/// The eight named geometric orientations a Radiance resolution line
/// can encode, relative to the format's fixed standard coordinate
/// system (origin at the lower-left, X increasing right, Y increasing
/// up).
///
/// The resolution line lists two axis flags; the *first* axis listed is
/// the major / outer sort and a `-` sign means that axis is *decreasing*
/// through the file. The format note (`docs/image/hdr/`, §2 "Resolution
/// string") enumerates eight legal forms and names the geometric
/// transform each one applies to the standard orientation:
///
/// | Resolution string | Variant                         |
/// |-------------------|---------------------------------|
/// | `-Y N +X M`       | [`Orientation::Standard`]        |
/// | `-Y N -X M`       | [`Orientation::FlipX`]           |
/// | `+Y N -X M`       | [`Orientation::Rotate180`]       |
/// | `+Y N +X M`       | [`Orientation::FlipY`]           |
/// | `+X M +Y N`       | [`Orientation::Rotate90Cw`]      |
/// | `-X M +Y N`       | [`Orientation::Rotate90CwFlipY`] |
/// | `-X M -Y N`       | [`Orientation::Rotate90Ccw`]     |
/// | `+X M -Y N`       | [`Orientation::Rotate90CcwFlipY`]|
///
/// The variant captures exactly the same information as the
/// [`HdrHeader`]'s three low-level fields (`y_sign`, `x_sign`,
/// `x_first`); [`Orientation::from_axis_fields`] and
/// [`Orientation::to_axis_fields`] convert losslessly between the two
/// representations, so callers can reason about a decoded picture's
/// scanline layout by name rather than by re-deriving it from the raw
/// flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// `-Y N +X M` — the standard orientation produced by the
    /// renderers: scanlines run from the upper-left across to the
    /// upper-right, then down the picture. Y-major, Y decreasing,
    /// X increasing.
    Standard,
    /// `-Y N -X M` — X reversed; the image is flipped left↔right from
    /// the standard orientation.
    FlipX,
    /// `+Y N -X M` — flipped left↔right *and* top↔bottom, i.e. rotated
    /// 180° from the standard orientation.
    Rotate180,
    /// `+Y N +X M` — flipped top↔bottom from the standard orientation.
    FlipY,
    /// `+X M +Y N` — rotated 90° clockwise from the standard
    /// orientation. X-major (each on-disk scanline is one column).
    Rotate90Cw,
    /// `-X M +Y N` — rotated 90° clockwise, then flipped top↔bottom.
    Rotate90CwFlipY,
    /// `-X M -Y N` — rotated 90° counter-clockwise from the standard
    /// orientation.
    Rotate90Ccw,
    /// `+X M -Y N` — rotated 90° counter-clockwise, then flipped
    /// top↔bottom.
    Rotate90CcwFlipY,
}

impl Orientation {
    /// Map the [`HdrHeader`]'s three low-level axis fields onto the named
    /// orientation they encode. Total over all `2 × 2 × 2` combinations
    /// — every `(y_sign, x_sign, x_first)` triple corresponds to exactly
    /// one of the eight legal resolution strings.
    pub fn from_axis_fields(y_sign: AxisSign, x_sign: AxisSign, x_first: bool) -> Self {
        use AxisSign::{Decreasing, Increasing};
        match (x_first, y_sign, x_sign) {
            // Y-first forms (`±Y H ±X W`).
            (false, Decreasing, Increasing) => Self::Standard,
            (false, Decreasing, Decreasing) => Self::FlipX,
            (false, Increasing, Decreasing) => Self::Rotate180,
            (false, Increasing, Increasing) => Self::FlipY,
            // X-first forms (`±X W ±Y H`).
            (true, Increasing, Increasing) => Self::Rotate90Cw,
            (true, Increasing, Decreasing) => Self::Rotate90CwFlipY,
            (true, Decreasing, Decreasing) => Self::Rotate90Ccw,
            (true, Decreasing, Increasing) => Self::Rotate90CcwFlipY,
        }
    }

    /// Decompose the named orientation into the `(y_sign, x_sign,
    /// x_first)` triple the [`HdrHeader`] stores. The exact inverse of
    /// [`Orientation::from_axis_fields`].
    pub fn to_axis_fields(self) -> (AxisSign, AxisSign, bool) {
        use AxisSign::{Decreasing, Increasing};
        match self {
            Self::Standard => (Decreasing, Increasing, false),
            Self::FlipX => (Decreasing, Decreasing, false),
            Self::Rotate180 => (Increasing, Decreasing, false),
            Self::FlipY => (Increasing, Increasing, false),
            Self::Rotate90Cw => (Increasing, Increasing, true),
            Self::Rotate90CwFlipY => (Increasing, Decreasing, true),
            Self::Rotate90Ccw => (Decreasing, Decreasing, true),
            Self::Rotate90CcwFlipY => (Decreasing, Increasing, true),
        }
    }

    /// `true` when this orientation lists the X axis before the Y axis
    /// (`±X W ±Y H`), i.e. each on-disk scanline holds one column's
    /// worth of samples rather than one row's. The four 90°-rotation
    /// variants are X-first; the four 0°/180°/mirror variants are
    /// Y-first.
    pub fn is_x_first(self) -> bool {
        self.to_axis_fields().2
    }

    /// Render the resolution-line template for this orientation as the
    /// printf-style string the format note uses, with `%d` placeholders
    /// for the two dimension values in on-disk order. The standard form
    /// is `"-Y %d +X %d"`.
    ///
    /// The placeholders are in *resolution-line* order: for the Y-first
    /// variants that's `<Y-flag> H <X-flag> W`; for the X-first variants
    /// it's `<X-flag> W <Y-flag> H`. See
    /// [`HdrHeader::resolution_line`](crate::HdrHeader) — the encoder's
    /// `write_resolution` substitutes the real dimensions into exactly
    /// this layout.
    pub fn resolution_template(self) -> &'static str {
        match self {
            Self::Standard => "-Y %d +X %d",
            Self::FlipX => "-Y %d -X %d",
            Self::Rotate180 => "+Y %d -X %d",
            Self::FlipY => "+Y %d +X %d",
            Self::Rotate90Cw => "+X %d +Y %d",
            Self::Rotate90CwFlipY => "-X %d +Y %d",
            Self::Rotate90Ccw => "-X %d -Y %d",
            Self::Rotate90CcwFlipY => "+X %d -Y %d",
        }
    }

    /// The geometric symmetry that carries the **standard** picture
    /// (`Self::Standard`, the right-side-up displayed image) onto the
    /// picture this orientation describes, per the format note's §2 table
    /// (`docs/image/hdr/radiance-hdr-rgbe-format.md`). For example
    /// [`Orientation::Rotate90Cw`] describes a picture that is the standard
    /// image rotated 90° clockwise, so its display transform is
    /// [`GeometricOp::Rotate90Cw`].
    ///
    /// Composing this transform with [`GeometricOp::inverse`] is exactly
    /// what [`crate::HdrImage::reorient`] uses to move a decoded buffer
    /// between any two of the eight orientations: go from the source
    /// orientation back to standard (the inverse), then forward to the
    /// target orientation. Because the eight transforms form the dihedral
    /// group of the rectangle (the order-8 symmetry group `D₄`), every
    /// such move is itself one of the eight ops.
    pub fn display_transform(self) -> GeometricOp {
        match self {
            Self::Standard => GeometricOp::Identity,
            Self::FlipX => GeometricOp::FlipHorizontal,
            Self::Rotate180 => GeometricOp::Rotate180,
            Self::FlipY => GeometricOp::FlipVertical,
            Self::Rotate90Cw => GeometricOp::Rotate90Cw,
            Self::Rotate90CwFlipY => GeometricOp::Transpose,
            Self::Rotate90Ccw => GeometricOp::Rotate90Ccw,
            Self::Rotate90CcwFlipY => GeometricOp::AntiTranspose,
        }
    }

    /// The named orientation whose [`Orientation::display_transform`] is
    /// `op` — the inverse lookup of [`Orientation::display_transform`].
    /// Total: each of the eight [`GeometricOp`] variants names exactly one
    /// orientation.
    pub fn from_display_transform(op: GeometricOp) -> Self {
        match op {
            GeometricOp::Identity => Self::Standard,
            GeometricOp::FlipHorizontal => Self::FlipX,
            GeometricOp::Rotate180 => Self::Rotate180,
            GeometricOp::FlipVertical => Self::FlipY,
            GeometricOp::Rotate90Cw => Self::Rotate90Cw,
            GeometricOp::Transpose => Self::Rotate90CwFlipY,
            GeometricOp::Rotate90Ccw => Self::Rotate90Ccw,
            GeometricOp::AntiTranspose => Self::Rotate90CcwFlipY,
        }
    }
}

/// One of the eight rigid symmetries of a rectangle — the dihedral group
/// `D₄`. These are exactly the geometric operations the Radiance §2
/// resolution-string orientations describe, named as picture transforms
/// rather than as on-disk axis flags.
///
/// Four of them (`Identity`, `FlipHorizontal`, `FlipVertical`,
/// `Rotate180`) preserve the `width × height` aspect; the other four
/// (`Rotate90Cw`, `Rotate90Ccw`, `Transpose`, `AntiTranspose`) swap the
/// two dimensions. [`GeometricOp::swaps_dimensions`] reports which.
///
/// The group is closed under composition ([`GeometricOp::then`]) and every
/// element has an inverse ([`GeometricOp::inverse`]); [`GeometricOp::ALL`]
/// enumerates all eight. [`crate::HdrImage`] applies any of them to its
/// float pixel buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeometricOp {
    /// Leave the picture unchanged.
    Identity,
    /// Mirror left↔right (reflect across the vertical centre line).
    FlipHorizontal,
    /// Mirror top↔bottom (reflect across the horizontal centre line).
    FlipVertical,
    /// Rotate 180°.
    Rotate180,
    /// Rotate 90° clockwise. Swaps width and height.
    Rotate90Cw,
    /// Rotate 90° counter-clockwise. Swaps width and height.
    Rotate90Ccw,
    /// Reflect across the main diagonal (`(x, y) -> (y, x)`). Swaps width
    /// and height. Equivalent to a 90° CW rotation followed by a vertical
    /// flip.
    Transpose,
    /// Reflect across the anti-diagonal. Swaps width and height.
    /// Equivalent to a 90° CCW rotation followed by a vertical flip (or a
    /// transpose followed by a 180° rotation).
    AntiTranspose,
}

impl GeometricOp {
    /// All eight symmetries, in a stable order.
    pub const ALL: [GeometricOp; 8] = [
        GeometricOp::Identity,
        GeometricOp::FlipHorizontal,
        GeometricOp::FlipVertical,
        GeometricOp::Rotate180,
        GeometricOp::Rotate90Cw,
        GeometricOp::Rotate90Ccw,
        GeometricOp::Transpose,
        GeometricOp::AntiTranspose,
    ];

    /// `true` when applying this op swaps the picture's width and height
    /// (the four 90°-class operations). The aspect-preserving four return
    /// `false`.
    pub fn swaps_dimensions(self) -> bool {
        matches!(
            self,
            Self::Rotate90Cw | Self::Rotate90Ccw | Self::Transpose | Self::AntiTranspose
        )
    }

    /// The inverse symmetry: `op.then(op.inverse()) == Identity` for every
    /// op. Six of the eight are involutions (their own inverse); the two
    /// 90° rotations invert to each other.
    pub fn inverse(self) -> Self {
        match self {
            Self::Rotate90Cw => Self::Rotate90Ccw,
            Self::Rotate90Ccw => Self::Rotate90Cw,
            // Identity, the two flips, the 180° rotation, and both
            // diagonal reflections are their own inverse.
            other => other,
        }
    }

    /// Compose two symmetries: `a.then(b)` is the single op equal to
    /// applying `a` first, then `b`. Closed over the group — the result is
    /// always one of the eight variants. This is the group multiplication
    /// that lets [`crate::HdrImage::reorient`] collapse a "back to
    /// standard, then out to the target" pair into a single transform.
    pub fn then(self, next: Self) -> Self {
        // Represent each element of D₄ as a (rotation k·90° CCW, mirror?)
        // pair and multiply in that normal form, then read the product
        // back out. `r` counts 90° CCW steps (0..4); `m` is a post-rotation
        // horizontal mirror flag.
        let (r0, m0) = self.to_rot_mirror();
        let (r1, m1) = next.to_rot_mirror();
        // Composition in the semidirect-product normal form
        // (apply self, then next): rotations add, but a mirror in `self`
        // negates the rotation contributed afterwards by `next`.
        let r = if m0 { (r0 + 4 - r1) % 4 } else { (r0 + r1) % 4 };
        let m = m0 ^ m1;
        Self::from_rot_mirror(r, m)
    }

    // Normal form: number of 90°-CCW rotations (0..4) and whether a
    // horizontal mirror is applied *after* the rotation.
    fn to_rot_mirror(self) -> (u8, bool) {
        match self {
            Self::Identity => (0, false),
            Self::Rotate90Ccw => (1, false),
            Self::Rotate180 => (2, false),
            Self::Rotate90Cw => (3, false),
            Self::FlipHorizontal => (0, true),
            Self::AntiTranspose => (1, true),
            Self::FlipVertical => (2, true),
            Self::Transpose => (3, true),
        }
    }

    fn from_rot_mirror(r: u8, m: bool) -> Self {
        match (r % 4, m) {
            (0, false) => Self::Identity,
            (1, false) => Self::Rotate90Ccw,
            (2, false) => Self::Rotate180,
            (3, false) => Self::Rotate90Cw,
            (0, true) => Self::FlipHorizontal,
            (1, true) => Self::AntiTranspose,
            (2, true) => Self::FlipVertical,
            (3, true) => Self::Transpose,
            _ => unreachable!("r is reduced mod 4"),
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
    /// Program / command lines carried in the header. The staged format
    /// note documents the header as the `#?…` identifier line "followed
    /// by one or more lines giving the programs used to produce the
    /// picture, interspersed with variable assignments". Such a line is
    /// neither a comment (`#…`) nor a `KEY=VALUE` assignment — it is the
    /// verbatim command (e.g. `rpict -vp 0 0 0 scene.oct`) that created
    /// the file. Every line in the header that contains no `=` and does
    /// not start with `#` is preserved here, in read order, so a
    /// decode→encode round-trip reproduces it instead of rejecting the
    /// file outright. Stored verbatim (no trailing newline).
    pub commands: Vec<String>,
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
            commands: Vec::new(),
            y_sign: AxisSign::Decreasing,
            x_sign: AxisSign::Increasing,
            x_first: false,
        }
    }
}

impl HdrHeader {
    /// The named [`Orientation`] this header's resolution-line axis
    /// fields encode. A convenience over reading `y_sign` / `x_sign` /
    /// `x_first` directly; lets a caller branch on the geometric meaning
    /// (`Orientation::Standard`, `Orientation::Rotate90Cw`, …) without
    /// re-deriving it from the raw flags.
    pub fn orientation(&self) -> Orientation {
        Orientation::from_axis_fields(self.y_sign, self.x_sign, self.x_first)
    }

    /// Set the resolution-line axis fields from a named [`Orientation`].
    /// Writes `y_sign`, `x_sign` and `x_first` to the triple the
    /// orientation decomposes into; the encoder then emits the matching
    /// resolution line. The inverse of [`HdrHeader::orientation`].
    ///
    /// Note this changes only the on-disk scanline *ordering* the
    /// encoder will write — it does not reorder the canonical top-down
    /// `(y, x)` pixel buffer in [`crate::HdrImage`]. The encoder applies
    /// the geometric transform implied by these fields on its way out,
    /// and the decoder undoes it on the way back, so a buffer encoded
    /// under any orientation round-trips to the same canonical layout.
    pub fn set_orientation(&mut self, orientation: Orientation) {
        let (y_sign, x_sign, x_first) = orientation.to_axis_fields();
        self.y_sign = y_sign;
        self.x_sign = x_sign;
        self.x_first = x_first;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_axis_fields_round_trip_over_all_eight_forms() {
        // Every (y_sign, x_sign, x_first) triple maps to exactly one
        // Orientation and back. Walk all 2×2×2 combinations and assert
        // `to_axis_fields(from_axis_fields(t)) == t` — i.e. the two
        // conversions are mutual inverses and the mapping is total.
        use AxisSign::{Decreasing, Increasing};
        for &y in &[Decreasing, Increasing] {
            for &x in &[Decreasing, Increasing] {
                for &xf in &[false, true] {
                    let o = Orientation::from_axis_fields(y, x, xf);
                    assert_eq!(
                        o.to_axis_fields(),
                        (y, x, xf),
                        "round-trip failed for y={y:?} x={x:?} x_first={xf}",
                    );
                }
            }
        }
    }

    #[test]
    fn orientation_all_eight_variants_are_distinct() {
        // The eight legal resolution strings must each name a different
        // orientation — no two triples collapse to the same variant.
        use std::collections::HashSet;
        use AxisSign::{Decreasing, Increasing};
        let mut seen = HashSet::new();
        for &y in &[Decreasing, Increasing] {
            for &x in &[Decreasing, Increasing] {
                for &xf in &[false, true] {
                    seen.insert(Orientation::from_axis_fields(y, x, xf));
                }
            }
        }
        assert_eq!(
            seen.len(),
            8,
            "expected 8 distinct orientations, got {}",
            seen.len()
        );
    }

    #[test]
    fn orientation_named_forms_match_spec_table() {
        // Pin each named variant to the exact axis-field triple the
        // format note's §2 resolution-string table assigns it.
        use AxisSign::{Decreasing, Increasing};
        // -Y N +X M  — Standard.
        assert_eq!(
            Orientation::Standard.to_axis_fields(),
            (Decreasing, Increasing, false)
        );
        // -Y N -X M  — flipped left↔right.
        assert_eq!(
            Orientation::FlipX.to_axis_fields(),
            (Decreasing, Decreasing, false)
        );
        // +Y N -X M  — rotated 180°.
        assert_eq!(
            Orientation::Rotate180.to_axis_fields(),
            (Increasing, Decreasing, false)
        );
        // +Y N +X M  — flipped top↔bottom.
        assert_eq!(
            Orientation::FlipY.to_axis_fields(),
            (Increasing, Increasing, false)
        );
        // +X M +Y N  — rotated 90° clockwise.
        assert_eq!(
            Orientation::Rotate90Cw.to_axis_fields(),
            (Increasing, Increasing, true)
        );
        // -X M +Y N  — rotated 90° CW then flipped top↔bottom.
        assert_eq!(
            Orientation::Rotate90CwFlipY.to_axis_fields(),
            (Increasing, Decreasing, true)
        );
        // -X M -Y N  — rotated 90° counter-clockwise.
        assert_eq!(
            Orientation::Rotate90Ccw.to_axis_fields(),
            (Decreasing, Decreasing, true)
        );
        // +X M -Y N  — rotated 90° CCW then flipped top↔bottom.
        assert_eq!(
            Orientation::Rotate90CcwFlipY.to_axis_fields(),
            (Decreasing, Increasing, true)
        );
    }

    #[test]
    fn orientation_resolution_templates_match_spec_strings() {
        assert_eq!(Orientation::Standard.resolution_template(), "-Y %d +X %d");
        assert_eq!(Orientation::FlipX.resolution_template(), "-Y %d -X %d");
        assert_eq!(Orientation::Rotate180.resolution_template(), "+Y %d -X %d");
        assert_eq!(Orientation::FlipY.resolution_template(), "+Y %d +X %d");
        assert_eq!(Orientation::Rotate90Cw.resolution_template(), "+X %d +Y %d");
        assert_eq!(
            Orientation::Rotate90CwFlipY.resolution_template(),
            "-X %d +Y %d"
        );
        assert_eq!(
            Orientation::Rotate90Ccw.resolution_template(),
            "-X %d -Y %d"
        );
        assert_eq!(
            Orientation::Rotate90CcwFlipY.resolution_template(),
            "+X %d -Y %d"
        );
    }

    #[test]
    fn orientation_is_x_first_flags_rotation_variants() {
        // The four 90°-rotation variants are X-first; the four
        // mirror/180° variants are Y-first.
        assert!(!Orientation::Standard.is_x_first());
        assert!(!Orientation::FlipX.is_x_first());
        assert!(!Orientation::Rotate180.is_x_first());
        assert!(!Orientation::FlipY.is_x_first());
        assert!(Orientation::Rotate90Cw.is_x_first());
        assert!(Orientation::Rotate90CwFlipY.is_x_first());
        assert!(Orientation::Rotate90Ccw.is_x_first());
        assert!(Orientation::Rotate90CcwFlipY.is_x_first());
    }

    #[test]
    fn header_orientation_round_trips_via_setter() {
        // `set_orientation` then `orientation` must reproduce the named
        // variant for every one of the eight forms.
        let all = [
            Orientation::Standard,
            Orientation::FlipX,
            Orientation::Rotate180,
            Orientation::FlipY,
            Orientation::Rotate90Cw,
            Orientation::Rotate90CwFlipY,
            Orientation::Rotate90Ccw,
            Orientation::Rotate90CcwFlipY,
        ];
        for o in all {
            let mut h = HdrHeader::default();
            h.set_orientation(o);
            assert_eq!(h.orientation(), o);
        }
    }

    #[test]
    fn default_header_orientation_is_standard() {
        // The `Default` header is the canonical `-Y H +X W` layout, which
        // names `Orientation::Standard`.
        assert_eq!(HdrHeader::default().orientation(), Orientation::Standard);
    }

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

    // -- GeometricOp group algebra (the §2 orientation matrix as D₄) --

    /// An independent coordinate-level model of a [`GeometricOp`]: where a
    /// source pixel `(x, y)` of a `w × h` picture lands, and the output
    /// dimensions. This is the geometric ground truth the abstract
    /// `then` / `inverse` group ops are validated against — it never calls
    /// the normal-form arithmetic under test.
    fn model(op: GeometricOp, x: i64, y: i64, w: i64, h: i64) -> (i64, i64, i64, i64) {
        // Returns (dst_x, dst_y, out_w, out_h).
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

    #[test]
    fn geometric_op_all_lists_eight_distinct_variants() {
        use std::collections::HashSet;
        let set: HashSet<_> = GeometricOp::ALL.iter().copied().collect();
        assert_eq!(set.len(), 8, "ALL must enumerate eight distinct ops");
    }

    #[test]
    fn geometric_op_swaps_dimensions_matches_model() {
        // The (3,7) asymmetric source makes a dimension swap observable.
        for op in GeometricOp::ALL {
            let (_, _, ow, oh) = model(op, 0, 0, 3, 7);
            assert_eq!(
                op.swaps_dimensions(),
                (ow, oh) == (7, 3),
                "{op:?}: swaps_dimensions disagrees with the coordinate model",
            );
        }
    }

    #[test]
    fn geometric_op_inverse_undoes_every_op() {
        // For each op, mapping a pixel forward then through the inverse must
        // land back on the original coordinate for every pixel of a 3×7 grid.
        let (w, h) = (3i64, 7i64);
        for op in GeometricOp::ALL {
            let inv = op.inverse();
            for y in 0..h {
                for x in 0..w {
                    let (mx, my, mw, mh) = model(op, x, y, w, h);
                    let (bx, by, bw, bh) = model(inv, mx, my, mw, mh);
                    assert_eq!(
                        (bx, by, bw, bh),
                        (x, y, w, h),
                        "{op:?}.inverse() failed to restore ({x},{y})",
                    );
                }
            }
            // Algebraic statement of the same fact.
            assert_eq!(op.then(inv), GeometricOp::Identity, "{op:?}.then(inverse)");
        }
    }

    #[test]
    fn geometric_op_then_matches_sequential_application() {
        // `a.then(b)` must equal "apply a, then b" as a coordinate map, for
        // every ordered pair across a 3×7 grid. This pins the normal-form
        // group multiplication against the independent geometric model.
        let (w, h) = (3i64, 7i64);
        for a in GeometricOp::ALL {
            for b in GeometricOp::ALL {
                let composed = a.then(b);
                for y in 0..h {
                    for x in 0..w {
                        let (ax, ay, aw, ah) = model(a, x, y, w, h);
                        let (bx, by, bw, bh) = model(b, ax, ay, aw, ah);
                        let (cx, cy, cw, ch) = model(composed, x, y, w, h);
                        assert_eq!(
                            (bx, by, bw, bh),
                            (cx, cy, cw, ch),
                            "{a:?}.then({b:?}) != sequential at ({x},{y})",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn geometric_op_then_is_closed_and_associative() {
        // Closure: every product is one of the eight. Associativity:
        // (a·b)·c == a·(b·c) for all triples — the group axioms.
        for a in GeometricOp::ALL {
            for b in GeometricOp::ALL {
                assert!(GeometricOp::ALL.contains(&a.then(b)), "{a:?}·{b:?} escaped");
                for c in GeometricOp::ALL {
                    assert_eq!(
                        a.then(b).then(c),
                        a.then(b.then(c)),
                        "associativity failed for {a:?},{b:?},{c:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn orientation_display_transform_round_trips() {
        // display_transform / from_display_transform are mutual inverses
        // over all eight orientations, and the mapping is a bijection onto
        // the eight GeometricOp variants.
        use std::collections::HashSet;
        let orientations = [
            Orientation::Standard,
            Orientation::FlipX,
            Orientation::Rotate180,
            Orientation::FlipY,
            Orientation::Rotate90Cw,
            Orientation::Rotate90CwFlipY,
            Orientation::Rotate90Ccw,
            Orientation::Rotate90CcwFlipY,
        ];
        let mut ops = HashSet::new();
        for o in orientations {
            let op = o.display_transform();
            ops.insert(op);
            assert_eq!(
                Orientation::from_display_transform(op),
                o,
                "{o:?} did not round-trip through its display transform",
            );
        }
        assert_eq!(ops.len(), 8, "display_transform is not a bijection");
    }

    #[test]
    fn orientation_x_first_iff_display_transform_swaps_dimensions() {
        // The four X-first orientations are exactly the four dimension-
        // swapping (90°-class) display transforms — a cross-check tying the
        // axis-field view to the geometric view.
        for op in GeometricOp::ALL {
            let o = Orientation::from_display_transform(op);
            assert_eq!(
                o.is_x_first(),
                op.swaps_dimensions(),
                "{o:?} / {op:?}: x_first vs dimension-swap mismatch",
            );
        }
    }
}
