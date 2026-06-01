//! Crate-local error type used by `oxideav-hdr`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`HdrError`] gains a
//! `From<HdrError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! underlying parse/encode functions stay framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-hdr`. Standalone (no `oxideav-core`)
/// callers see this; framework callers convert via the gated
/// `From<HdrError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, HdrError>;

/// Error variants returned by `oxideav-hdr`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit. The crate intentionally avoids surfacing transport (`Io`)
/// or framework-specific (`FormatNotFound`, `CodecNotFound`) errors —
/// those originate in callers that are already linking `oxideav-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HdrError {
    /// The byte stream is malformed (bad magic, malformed header line,
    /// resolution line missing, RLE run runs past the end of the row,
    /// etc.).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (unknown FORMAT, encoder asked to write old-RLE, etc.).
    Unsupported(String),
    /// The resolution line declares a picture larger than the caller-
    /// configured [`crate::HdrLimits`]. Raised before any pixel buffer
    /// is allocated. Round 202 added this variant so attacker-crafted
    /// gigantic-dimension headers (e.g. `-Y u32::MAX +X u32::MAX`) are
    /// rejected at the door rather than triggering an unbounded
    /// allocation. The string carries the dimension that tripped the
    /// limit and the limit value for diagnosability.
    TooLarge(String),
}

impl HdrError {
    /// Construct an [`HdrError::InvalidData`] from a stringy message.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct an [`HdrError::Unsupported`] from a stringy message.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    /// Construct an [`HdrError::TooLarge`] from a stringy message.
    pub fn too_large(msg: impl Into<String>) -> Self {
        Self::TooLarge(msg.into())
    }
}

impl fmt::Display for HdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::TooLarge(s) => write!(f, "too large: {s}"),
        }
    }
}

impl std::error::Error for HdrError {}
