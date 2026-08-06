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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// Every variant's rendered message must name the VALUE that caused it, not
    /// merely the category. This is the one place a caller reads when an encode
    /// refuses, and "invalid dimensions" with no dimensions in it costs them a
    /// debugging session.
    ///
    /// Written as an exhaustive `match` over a constructed sample of each
    /// variant so that adding a variant without a message that mentions its
    /// payload fails to compile here rather than shipping.
    #[test]
    fn every_error_message_names_its_payload() {
        let samples = [
            EncodeError::Cancelled(enough::StopReason::Cancelled),
            EncodeError::AllocFailed {
                requested_bytes: 123_456,
                context: "tile recon canvas",
            },
            EncodeError::AllocFailed {
                requested_bytes: 7,
                context: "",
            },
            EncodeError::InvalidDimensions {
                width: 63,
                height: 65,
                reason: "monochrome encode requires 8-aligned dims",
            },
            EncodeError::UnsupportedConfig("inter frames are not implemented"),
        ];

        for e in &samples {
            let s = e.to_string();
            // The compiler enforces coverage: `#[non_exhaustive]` binds only
            // DOWNSTREAM crates, so in-crate this match must list every
            // variant — adding one without a payload assertion here is a
            // compile error, not a silently untested message.
            match e {
                EncodeError::Cancelled(_) => {
                    assert!(s.contains("cancel"), "{s:?}");
                }
                EncodeError::AllocFailed {
                    requested_bytes,
                    context,
                } => {
                    assert!(
                        s.contains(&requested_bytes.to_string()),
                        "the byte count must be in the message: {s:?}"
                    );
                    if !context.is_empty() {
                        assert!(
                            s.contains(context),
                            "a non-empty context must be in the message: {s:?}"
                        );
                    }
                }
                EncodeError::InvalidDimensions {
                    width,
                    height,
                    reason,
                } => {
                    assert!(
                        s.contains(&alloc::format!("{width}x{height}")),
                        "the geometry must be in the message: {s:?}"
                    );
                    assert!(
                        s.contains(reason),
                        "the reason must be in the message: {s:?}"
                    );
                }
                EncodeError::UnsupportedConfig(what) => {
                    assert!(s.contains(what), "the reason must be in the message: {s:?}");
                }
            }
            assert!(
                s.len() > 15,
                "an error message this terse cannot be acted on: {s:?}"
            );
        }
    }

    /// A stop token's reason must survive the conversion — a cancelled encode
    /// that reports a generic failure is indistinguishable from a real fault.
    #[test]
    fn stop_reason_converts_into_cancelled() {
        let e: EncodeError = enough::StopReason::Cancelled.into();
        assert!(matches!(
            e,
            EncodeError::Cancelled(enough::StopReason::Cancelled)
        ));
    }

    /// The error type is usable in `Box<dyn Error>` / `?` chains. This is a
    /// contract of the public API, not an implementation detail.
    #[test]
    fn encode_error_is_a_core_error() {
        fn assert_error<E: core::error::Error>(_: &E) {}
        assert_error(&EncodeError::UnsupportedConfig("x"));
    }
}
