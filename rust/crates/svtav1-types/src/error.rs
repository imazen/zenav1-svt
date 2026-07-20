//! Shared encoder error type + location-traced result alias.
//!
//! Additive production-hardening surface (Feature 2). Existing infallible
//! entry points are untouched; the new fallible `try_*` methods on
//! `EncodePipeline` return [`EncodeResult`], whose error carries a
//! [`whereat`] source-location trace ([`At`]) around an [`EncodeError`].

use whereat::At;

/// Result alias for fallible encoder entry points. The error is wrapped in
/// [`At`] so it records where it was raised (crate + `file:line`) without
/// any heap allocation on the `Ok` path.
pub type EncodeResult<T> = core::result::Result<T, At<EncodeError>>;

/// Errors an additive fallible encode entry point can surface instead of
/// panicking.
///
/// `#[non_exhaustive]` so new variants can be added without a breaking
/// change — match arms in downstream code must keep a wildcard.
#[derive(Debug)]
#[non_exhaustive]
pub enum EncodeError {
    /// Cooperative cancellation fired. Carries the [`enough::StopReason`]
    /// reported by the caller-supplied stop token.
    Cancelled(enough::StopReason),
    /// A fallible allocation could not be satisfied (only produced when the
    /// `fallible-alloc` feature is enabled).
    AllocFailed {
        /// Number of bytes the allocation requested (saturating product of
        /// element count and element size).
        requested_bytes: u64,
        /// Static label identifying the allocation site.
        context: &'static str,
    },
    /// The requested frame dimensions are unsupported or invalid for the
    /// current configuration.
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
        /// Human-readable reason the dimensions were rejected.
        reason: &'static str,
    },
    /// A configuration combination the port cannot (yet) encode.
    UnsupportedConfig(&'static str),
}

impl From<enough::StopReason> for EncodeError {
    fn from(r: enough::StopReason) -> Self {
        EncodeError::Cancelled(r)
    }
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EncodeError::Cancelled(r) => write!(f, "encode cancelled: {r}"),
            EncodeError::AllocFailed {
                requested_bytes,
                context,
            } => {
                if context.is_empty() {
                    write!(f, "allocation of {requested_bytes} bytes failed")
                } else {
                    write!(f, "allocation of {requested_bytes} bytes failed for {context}")
                }
            }
            EncodeError::InvalidDimensions {
                width,
                height,
                reason,
            } => write!(f, "invalid dimensions {width}x{height}: {reason}"),
            EncodeError::UnsupportedConfig(what) => write!(f, "unsupported config: {what}"),
        }
    }
}

// Stable in `core` since Rust 1.81; the workspace MSRV is 1.85.
impl core::error::Error for EncodeError {}
