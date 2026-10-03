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

/// Contract alias for [`HdrError`].
pub type Error = HdrError;

/// Error variants returned by `oxideav-hdr`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit. Framework-specific errors (`FormatNotFound`,
/// `CodecNotFound`, …) originate in callers that are already linking
/// `oxideav-core`.
#[derive(Debug)]
#[non_exhaustive]
pub enum HdrError {
    /// The byte stream is malformed (bad magic, malformed header line,
    /// resolution line missing, RLE run runs past the end of the row,
    /// etc.), or a caller-assembled image has inconsistent geometry.
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (unknown FORMAT, encoder asked to write new-RLE at an
    /// unaddressable width, etc.).
    Unsupported(String),
    /// The resolution line declares a picture larger than the caller-
    /// configured [`crate::DecodeOptions`] limits. Raised before any
    /// pixel buffer is allocated so attacker-crafted gigantic-dimension
    /// headers (e.g. `-Y u32::MAX +X u32::MAX`) are rejected at the door
    /// rather than triggering an unbounded allocation. The string
    /// carries the dimension that tripped the limit and the limit value.
    LimitExceeded(String),
    /// A read ([`crate::decode_from`]) or write ([`crate::encode_to`])
    /// failed.
    Io(std::io::Error),
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

    /// Construct an [`HdrError::LimitExceeded`] from a stringy message.
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }

    /// The pre-contract name of [`HdrError::limit`].
    #[deprecated(note = "use HdrError::limit (IMAGE_CRATE_API)")]
    pub fn too_large(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl fmt::Display for HdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for HdrError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for HdrError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
