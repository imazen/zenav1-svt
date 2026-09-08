//! Explicit still-encoding policy and deterministic effort resolution.
//!
//! Version 1 resolves effort to native search buckets. It does not claim
//! fractional adaptive search or calibrated runtime/quality estimates.
use crate::avif::{NativePreset, SvtReference, ZenEnhancements};

/// Checked ordinal work preference. Higher permits more work, not more seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effort(f32);
impl Effort {
    pub fn new(value: f32) -> Result<Self, &'static str> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("effort must be finite and in 0..=1");
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
    pub const fn value(self) -> f32 {
        self.0
    }
    /// Native buckets, not interpolation of C settings. Research -1 is the tip.
    pub fn native_preset(self) -> NativePreset {
        const PRESETS: [i8; 11] = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, -1];
        NativePreset::new(PRESETS[(self.0 * 10.0).floor() as usize]).unwrap()
    }
}

/// A decision constraint, not certification that every input is byte-exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodingPolicy {
    /// Reference-supported decisions only; no Zen extensions or backend routing.
    SvtParity(SvtReference),
    /// Existing extensions may be selected explicitly; no unmeasured auto-bundle.
    Zen,
}
impl EncodingPolicy {
    /// New parity requests default to the explicitly pinned pristine source.
    pub const fn svt_parity() -> Self {
        Self::SvtParity(SvtReference::Mainline420)
    }
}

/// Backend-owned statement about measurement availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StillSuitability {
    /// No representative calibrated RD/time estimate is available for routing.
    Uncalibrated,
}

/// Resolved search policy. Full pixel/config identity is the wrapper's concern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStillPolicy {
    pub version: u32,
    /// None preserves the legacy constructor contract.
    pub policy: Option<EncodingPolicy>,
    pub reference: SvtReference,
    pub requested_effort: Option<Effort>,
    pub native_preset: NativePreset,
    pub enhancements: ZenEnhancements,
    pub suitability: StillSuitability,
}
