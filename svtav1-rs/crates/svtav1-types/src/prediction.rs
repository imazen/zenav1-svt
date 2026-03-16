//! Prediction mode definitions.
//!
//! Ported from `definitions.h` lines 1127-1262.

/// AV1 prediction modes (13 intra + 4 single-ref inter + 8 compound inter = 25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PredictionMode {
    // Intra modes (0-12)
    DcPred = 0,
    VPred = 1,
    HPred = 2,
    D45Pred = 3,
    D135Pred = 4,
    D113Pred = 5,
    D157Pred = 6,
    D203Pred = 7,
    D67Pred = 8,
    SmoothPred = 9,
    SmoothVPred = 10,
    SmoothHPred = 11,
    PaethPred = 12,
    // Single-ref inter modes (13-16)
    NearestMv = 13,
    NearMv = 14,
    GlobalMv = 15,
    NewMv = 16,
    // Compound inter modes (17-24)
    NearestNearestMv = 17,
    NearNearMv = 18,
    NearestNewMv = 19,
    NewNearestMv = 20,
    NearNewMv = 21,
    NewNearMv = 22,
    GlobalGlobalMv = 23,
    NewNewMv = 24,
}

impl PredictionMode {
    /// Total number of modes (MB_MODE_COUNT).
    pub const COUNT: usize = 25;

    /// Number of intra modes (INTRA_MODES = PAETH_PRED + 1).
    pub const INTRA_MODES: usize = 13;

    /// Range constants matching C enum.
    pub const INTRA_MODE_START: u8 = Self::DcPred as u8;
    pub const INTRA_MODE_END: u8 = Self::NearestMv as u8;
    pub const SINGLE_INTER_MODE_START: u8 = Self::NearestMv as u8;
    pub const SINGLE_INTER_MODE_END: u8 = Self::NearestNearestMv as u8;
    pub const COMP_INTER_MODE_START: u8 = Self::NearestNearestMv as u8;
    pub const COMP_INTER_MODE_END: u8 = Self::COUNT as u8;

    /// Invalid sentinel for uv_mode in inter blocks.
    pub const INTRA_INVALID_RAW: u8 = Self::COUNT as u8;

    #[inline]
    pub const fn is_intra(self) -> bool {
        (self as u8) < Self::INTRA_MODE_END
    }

    #[inline]
    pub const fn is_inter(self) -> bool {
        (self as u8) >= Self::SINGLE_INTER_MODE_START
    }

    #[inline]
    pub const fn is_compound(self) -> bool {
        (self as u8) >= Self::COMP_INTER_MODE_START
    }

    #[inline]
    pub const fn is_single_ref(self) -> bool {
        let v = self as u8;
        v >= Self::SINGLE_INTER_MODE_START && v < Self::SINGLE_INTER_MODE_END
    }
}

/// UV (chroma) prediction modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum UvPredictionMode {
    UvDcPred = 0,
    UvVPred = 1,
    UvHPred = 2,
    UvD45Pred = 3,
    UvD135Pred = 4,
    UvD113Pred = 5,
    UvD157Pred = 6,
    UvD203Pred = 7,
    UvD67Pred = 8,
    UvSmoothPred = 9,
    UvSmoothVPred = 10,
    UvSmoothHPred = 11,
    UvPaethPred = 12,
    /// Chroma-from-Luma prediction.
    UvCflPred = 13,
}

impl UvPredictionMode {
    pub const COUNT: usize = 14;
    pub const INVALID_RAW: u8 = 14;
}

/// Motion mode for inter prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MotionMode {
    SimpleTranslation = 0,
    /// 2-sided OBMC.
    ObmcCausal = 1,
    /// 2-sided warped motion.
    WarpedCausal = 2,
}

impl MotionMode {
    pub const COUNT: usize = 3;
}

/// Inter-intra prediction modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InterIntraMode {
    IiDcPred = 0,
    IiVPred = 1,
    IiHPred = 2,
    IiSmoothPred = 3,
}

impl InterIntraMode {
    pub const COUNT: usize = 4;
}

/// Compound prediction blend types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompoundType {
    Average = 0,
    DistWtd = 1,
    Wedge = 2,
    DiffWtd = 3,
}

impl CompoundType {
    pub const COUNT: usize = 4;
    /// Number of masked compound types (wedge + diffwtd).
    pub const MASKED_TYPES: usize = 2;
}

/// Filter intra modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FilterIntraMode {
    FilterDcPred = 0,
    FilterVPred = 1,
    FilterHPred = 2,
    FilterD157Pred = 3,
    FilterPaethPred = 4,
}

impl FilterIntraMode {
    pub const COUNT: usize = 5;
}

/// Mapping from FilterIntraMode to PredictionMode.
pub const FIMODE_TO_INTRAMODE: [PredictionMode; FilterIntraMode::COUNT] = [
    PredictionMode::DcPred,
    PredictionMode::VPred,
    PredictionMode::HPred,
    PredictionMode::D157Pred,
    PredictionMode::PaethPred,
];

/// Number of directional modes.
pub const DIRECTIONAL_MODES: usize = 8;
/// Maximum angle delta.
pub const MAX_ANGLE_DELTA: i8 = 3;
/// Angle step in degrees.
pub const ANGLE_STEP: u8 = 3;

/// Number of single-ref inter modes.
pub const INTER_MODES: usize = 4; // NEWMV - NEARESTMV + 1
/// Number of compound inter modes.
pub const INTER_COMPOUND_MODES: usize = 8; // NEW_NEWMV - NEAREST_NEARESTMV + 1

/// Inter-inter compound data.
#[derive(Debug, Clone, Copy, Default)]
pub struct InterInterCompoundData {
    pub compound_type: u8,
    pub wedge_index: u8,
    pub wedge_sign: u8,
    pub mask_type: u8,
}

/// Palette size enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PaletteSize {
    TwoColors = 0,
    ThreeColors = 1,
    FourColors = 2,
    FiveColors = 3,
    SixColors = 4,
    SevenColors = 5,
    EightColors = 6,
}

impl PaletteSize {
    pub const COUNT: usize = 7;
}

/// Maximum palette size.
pub const PALETTE_MAX_SIZE: usize = 8;

/// Maximum wedge types.
pub const MAX_WEDGE_TYPES: usize = 1 << 4;
pub const MAX_WEDGE_SIZE_LOG2: usize = 5;
pub const MAX_WEDGE_SIZE: usize = 1 << MAX_WEDGE_SIZE_LOG2;

/// Blend constants.
pub const AOM_BLEND_A64_ROUND_BITS: u32 = 6;
pub const AOM_BLEND_A64_MAX_ALPHA: u32 = 1 << AOM_BLEND_A64_ROUND_BITS;

/// CFL (chroma-from-luma) constants.
pub const CFL_ALPHABET_SIZE_LOG2: usize = 4;
pub const CFL_ALPHABET_SIZE: usize = 1 << CFL_ALPHABET_SIZE_LOG2;
pub const CFL_JOINT_SIGNS: usize = 8; // CFL_SIGNS * CFL_SIGNS - 1

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prediction_mode_discriminants() {
        assert_eq!(PredictionMode::DcPred as u8, 0);
        assert_eq!(PredictionMode::PaethPred as u8, 12);
        assert_eq!(PredictionMode::NearestMv as u8, 13);
        assert_eq!(PredictionMode::NewNewMv as u8, 24);
    }

    #[test]
    fn prediction_mode_classification() {
        assert!(PredictionMode::DcPred.is_intra());
        assert!(!PredictionMode::DcPred.is_inter());
        assert!(PredictionMode::NearestMv.is_inter());
        assert!(PredictionMode::NearestMv.is_single_ref());
        assert!(!PredictionMode::NearestMv.is_compound());
        assert!(PredictionMode::NearestNearestMv.is_compound());
    }

    #[test]
    fn uv_mode_discriminants() {
        assert_eq!(UvPredictionMode::UvCflPred as u8, 13);
    }
}
