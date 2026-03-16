//! Interpolation filter definitions.
//!
//! Ported from `definitions.h` lines 764-801.

/// Interpolation filter types for sub-pixel motion compensation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InterpFilter {
    EightTapRegular = 0,
    EightTapSmooth = 1,
    MultiTapSharp = 2,
    Bilinear = 3,
}

impl InterpFilter {
    /// Total number of filter types (INTERP_FILTERS_ALL).
    pub const ALL_COUNT: usize = 4;

    /// Number of switchable filters (SWITCHABLE_FILTERS = BILINEAR).
    pub const SWITCHABLE_FILTERS: usize = 3;

    /// Switchable mode sentinel (SWITCHABLE = SWITCHABLE_FILTERS + 1).
    pub const SWITCHABLE: u8 = Self::SWITCHABLE_FILTERS as u8 + 1;
}

/// Sub-pixel search stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SubpelStage {
    /// Motion estimation.
    Me = 0,
    /// Predictive motion estimation.
    Pme = 1,
}

/// Sub-pixel search type (number of filter taps to use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SubpelSearchType {
    Use2TapsOrig = 0,
    Use2Taps = 1,
    Use4Taps = 2,
    Use8Taps = 3,
}

/// Sub-pixel search method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SubpelSearchMethod {
    Tree = 0,
    TreePruned = 1,
}

/// Sub-pixel force stop precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SubpelForceStop {
    EighthPel = 0,
    QuarterPel = 1,
    HalfPel = 2,
    FullPel = 3,
}

/// Interpolation extend for border handling.
pub const AOM_INTERP_EXTEND: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interp_filter_discriminants() {
        assert_eq!(InterpFilter::EightTapRegular as u8, 0);
        assert_eq!(InterpFilter::Bilinear as u8, 3);
    }
}
