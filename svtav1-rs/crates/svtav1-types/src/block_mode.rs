//! Block mode information structures.
//!
//! Ported from `block_structures.h` lines 36-104.

use crate::block::BlockSize;
use crate::frame::PLANE_TYPES;
use crate::motion::Mv;
use crate::partition::PartitionType;
use crate::prediction::{
    InterInterCompoundData, InterIntraMode, MotionMode, PredictionMode,
    UvPredictionMode, PALETTE_MAX_SIZE,
};

/// Block mode information — core per-block state in the MI grid.
///
/// This is the fundamental unit stored per 4x4 MI position. It contains
/// all information needed for prediction, transform, and entropy coding.
#[derive(Debug, Clone, Copy)]
pub struct BlockModeInfo {
    /// The prediction mode used.
    pub mode: PredictionMode,
    /// The UV mode when intra is used.
    pub uv_mode: UvPredictionMode,

    // --- Inter mode info ---
    /// Motion vectors (unipred in [0], bipred in [0] and [1]).
    pub mv: [Mv; 2],
    /// Reference frames for the MV.
    pub ref_frame: [i8; 2],
    /// Interpolation filter (packed u32).
    pub interp_filters: u32,
    /// Compound blend data.
    pub interinter_comp: InterInterCompoundData,
    /// Motion mode (simple, OBMC, warped).
    pub motion_mode: MotionMode,
    /// Number of samples used by warped causal.
    pub num_proj_ref: u8,
    /// Inter-intra prediction mode.
    pub interintra_mode: InterIntraMode,
    /// Wedge index for interintra.
    pub interintra_wedge_index: i8,

    // --- Intra mode info ---
    /// Directional mode angle delta per plane type.
    pub angle_delta: [i8; PLANE_TYPES],
    /// Filter intra mode.
    pub filter_intra_mode: u8,
    /// CFL: joint sign of alpha Cb and alpha Cr.
    pub cfl_alpha_signs: u8,
    /// CFL: index of alpha Cb/Cr combination.
    pub cfl_alpha_idx: u8,

    // --- Transform ---
    pub tx_depth: u8,

    // --- Flags (bitfields in C) ---
    pub is_interintra_used: bool,
    pub use_wedge_interintra: bool,
    /// Indicates if masked compound is used.
    pub comp_group_idx: bool,
    /// 0 = distance-weighted blending, 1 = averaging.
    pub compound_idx: bool,
    /// Skip coefficients only.
    pub skip: bool,
    /// Skip mode info + coefficients.
    pub skip_mode: bool,
    /// Whether intrabc is used.
    pub use_intrabc: bool,
}

impl Default for BlockModeInfo {
    fn default() -> Self {
        Self {
            mode: PredictionMode::DcPred,
            uv_mode: UvPredictionMode::UvDcPred,
            mv: [Mv::ZERO; 2],
            ref_frame: [0; 2],
            interp_filters: 0,
            interinter_comp: InterInterCompoundData::default(),
            motion_mode: MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            interintra_mode: InterIntraMode::IiDcPred,
            interintra_wedge_index: 0,
            angle_delta: [0; PLANE_TYPES],
            filter_intra_mode: 0,
            cfl_alpha_signs: 0,
            cfl_alpha_idx: 0,
            tx_depth: 0,
            is_interintra_used: false,
            use_wedge_interintra: false,
            comp_group_idx: false,
            compound_idx: false,
            skip: false,
            skip_mode: false,
            use_intrabc: false,
        }
    }
}

impl BlockModeInfo {
    /// Returns true if this block has a second reference.
    #[inline]
    pub const fn has_second_ref(&self) -> bool {
        self.ref_frame[1] > 0 // > INTRA_FRAME
    }

    /// Returns true if this block uses inter prediction (including intrabc).
    #[inline]
    pub const fn is_inter_block(&self) -> bool {
        self.use_intrabc || self.ref_frame[0] > 0
    }
}

/// Palette luma mode info.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaletteLumaModeInfo {
    /// Base colors for Y only.
    pub palette_colors: [u16; PALETTE_MAX_SIZE],
    /// Number of base colors for Y.
    pub palette_size: u8,
}

/// Macroblock mode info — extended block info including partition and segment.
///
/// Ported from `block_structures.h` lines 97-104.
#[derive(Debug, Clone, Copy)]
pub struct MbModeInfo {
    pub block_mi: BlockModeInfo,
    pub bsize: BlockSize,
    pub partition: PartitionType,
    pub segment_id: u8,
    pub palette_mode_info: PaletteLumaModeInfo,
    pub cdef_strength: i8,
}

impl Default for MbModeInfo {
    fn default() -> Self {
        Self {
            block_mi: BlockModeInfo::default(),
            bsize: BlockSize::Block4x4,
            partition: PartitionType::None,
            segment_id: 0,
            palette_mode_info: PaletteLumaModeInfo::default(),
            cdef_strength: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_mode_info_default() {
        let bmi = BlockModeInfo::default();
        assert!(bmi.mode.is_intra());
        assert!(!bmi.is_inter_block());
        assert!(!bmi.has_second_ref());
        assert!(!bmi.skip);
    }

    #[test]
    fn mb_mode_info_default() {
        let mmi = MbModeInfo::default();
        assert_eq!(mmi.bsize, BlockSize::Block4x4);
        assert_eq!(mmi.cdef_strength, 0);
    }
}
