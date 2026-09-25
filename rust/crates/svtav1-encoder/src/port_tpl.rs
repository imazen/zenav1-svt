//! `src_ops_process.c` — the TPL (temporal-dependency model) lookahead engine.
//!
//! C reference: v4.2.0 `Source/Lib/Codec/src_ops_process.c`. TPL runs a
//! reduced-cost "fake encode" over each picture in the lookahead window:
//! for every block it scores intra prediction against the SOURCE, motion
//! search against the reference SOURCE pictures, then re-predicts from the
//! reference TPL *reconstructions* (the `mc_flow_rec_picture_buffer` planes
//! this module itself fills) so later frames inherit a propagation cost.
//! The per-block [`TplStats`] grid feeds `tpl_model_update` /
//! `svt_aom_generate_r0beta`, whose `r0` drives
//! `svt_aom_sb_qp_derivation_tpl_la` (rc_aq.c) and the mfmv>=2 arm of
//! `use_ref_frame_mvs` (adaptive_mv_pred.c).
//!
//! | C | here |
//! |---|------|
//! | `tpl_regular_setup_me_refs` (117-155) | [`tpl_regular_setup_me_refs`] |
//! | `tpl_prep_info` (155-184) | [`tpl_prep_info`] |
//! | `generate_lambda_scaling_factor` (184-232) | [`generate_lambda_scaling_factor`] |
//! | `get_quantize_error` (232-258) | [`get_quantize_error`] |
//! | `rate_estimator` (258-273) | [`rate_estimator`] |
//! | `result_model_store` (273-352) | [`result_model_store`] |
//! | `tpl_blk_idx_tab` + mode arrays (352-384) | [`TPL_BLK_IDX_TAB`] etc. |
//! | `svt_tpl_init_mv_cost_params` (384-404) | [`tpl_init_mv_cost_params`] |
//! | `init_xd_tpl` (404-419) | [`TplXd::new`] |
//! | `tpl_subpel_search` (419-517) | [`tpl_subpel_search`] |
//! | `tpl_mc_flow_dispenser_sb_generic` (517-1202) | [`tpl_mc_flow_dispenser_sb_generic`] |
//! | `tpl_mc_flow_dispenser` (1338-1411) | [`tpl_mc_flow_dispenser`] |
//!
//! C's `assign_tpl_segments` / `tpl_dispenser_st` are the resource-manager's
//! segmented-thread plumbing; the port drives the same SB loop sequentially
//! in [`tpl_mc_flow_dispenser`], so there is nothing to translate there.

use alloc::vec::Vec;
use svtav1_dsp::hadamard::aom_satd;
use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;
use svtav1_types::transform::{TxSize, TxType};

/// C `MAX_TPL_SIZE` (src_ops_process.c:374).
pub const MAX_TPL_SIZE: usize = 32;
/// C `MAX_TPL_SAMPLES_PER_BLOCK`.
pub const MAX_TPL_SAMPLES_PER_BLOCK: usize = MAX_TPL_SIZE * MAX_TPL_SIZE;
/// C `TPL_RDMULT_SCALING_FACTOR` (src_ops_process.c:375).
pub const TPL_RDMULT_SCALING_FACTOR: i32 = 6;
/// C `TPL_PAD` (encode_context.h:35).
pub const TPL_PAD: i32 = 32;
/// C `REF_LIST_MAX_DEPTH` — the per-list candidate ceiling TPL indexes by.
pub const REF_LIST_MAX_DEPTH: usize = 8;
/// C `MAX_TPL_LA_SW` == `MAX_TPL_GROUP_SIZE` (definitions.h:57/67).
pub const MAX_TPL_LA_SW: usize = 512;
/// C `TPL_DEP_COST_SCALE_LOG2` (definitions.h:65) — fixed-point scale of
/// the stored stats.
pub const TPL_DEP_COST_SCALE_LOG2: u32 = 4;
/// C `NEWMV` as a `PredictionMode` value (definitions.h:1205).
pub const NEWMV: u8 = svtav1_types::prediction::PredictionMode::NewMv as u8;
/// C `DC_PRED` re-export for stat fields.
pub const DC_PRED: u8 = crate::port_picstruct::DC_PRED;

/// C `tpl_blk_idx_tab` (src_ops_process.c:352-357): row 0 maps a dispenser
/// `blk_index` to the z-order coded-unit index, row 1 to the ME `pu_index`.
#[rustfmt::skip]
pub const TPL_BLK_IDX_TAB: [[u8; 21]; 2] = [
    /*CU index*/ [0, 1, 22, 43, 64, 2, 7, 12, 17, 23, 28, 33, 38, 44, 49, 54, 59, 65, 70, 75, 80],
    /*ME index*/ [0, 1, 2, 3, 4, 5, 6, 9, 10, 7, 8, 11, 12, 13, 14, 17, 18, 15, 16, 19, 20],
];

/// C `size_array[MAX_TPL_MODE]` — dispenser block size per search level.
pub const SIZE_ARRAY: [u32; 3] = [16, 32, 64];
/// C `blk_start_array[MAX_TPL_MODE]`.
pub const BLK_START_ARRAY: [u8; 3] = [5, 1, 0];
/// C `blk_end_array[MAX_TPL_MODE]` (INCLUSIVE — C loops `<= blk_end`).
pub const BLK_END_ARRAY: [u8; 3] = [20, 4, 0];
/// C `tx_size_array[MAX_TPL_MODE]`.
pub const TX_SIZE_ARRAY: [TxSize; 3] = [TxSize::Tx16x16, TxSize::Tx32x32, TxSize::Tx64x64];
/// C `sub2_tx_size_array[MAX_TPL_MODE]` (`subsample_tx == 1`).
pub const SUB2_TX_SIZE_ARRAY: [TxSize; 3] = [TxSize::Tx16x8, TxSize::Tx32x16, TxSize::Tx64x32];
/// C `sub4_tx_size_array[MAX_TPL_MODE]` (`subsample_tx == 2`).
pub const SUB4_TX_SIZE_ARRAY: [TxSize; 3] = [TxSize::Tx16x4, TxSize::Tx32x8, TxSize::Tx64x16];

/// C `TplStats` (coding_unit.h:239-248).
#[derive(Debug, Clone, Copy, Default)]
pub struct TplStats {
    /// C `srcrf_dist` — distortion, source-reference prediction.
    pub srcrf_dist: i64,
    /// C `recrf_dist` — distortion, reconstruction-reference prediction.
    pub recrf_dist: i64,
    /// C `srcrf_rate`.
    pub srcrf_rate: i64,
    /// C `recrf_rate`.
    pub recrf_rate: i64,
    /// C `mc_dep_rate` — filled by `tpl_model_update`, not the dispenser.
    pub mc_dep_rate: i64,
    /// C `mc_dep_dist`.
    pub mc_dep_dist: i64,
    /// C `mv` — the winning inter MV, eighth-pel.
    pub mv: Mv,
    /// C `ref_frame_poc`.
    pub ref_frame_poc: u64,
}

/// C `TplSrcStats` (coding_unit.h:250-258) — the src-search result cached per
/// 16x16 block when `tpl_lad_mg > 0` reuses a window.
#[derive(Debug, Clone, Copy, Default)]
pub struct TplSrcStats {
    /// C `srcrf_dist`.
    pub srcrf_dist: i64,
    /// C `srcrf_rate`.
    pub srcrf_rate: i64,
    /// C `ref_frame_poc`.
    pub ref_frame_poc: u64,
    /// C `mv`.
    pub mv: Mv,
    /// C `best_mode` — `DC_PRED` or `NEWMV`.
    pub best_mode: u8,
    /// C `best_rf_idx` — `svt_get_ref_frame_type - 1`, -1 when intra won.
    pub best_rf_idx: i32,
    /// C `best_intra_mode` (`PredictionMode`).
    pub best_intra_mode: u8,
}

/// C `EbDownScaledBufDescPtrArray` reduced to what TPL reads: the reference's
/// POC plus an index into the caller's ref-picture table (C stores a raw
/// `picture_ptr`; the port borrows by index).
#[derive(Debug, Clone, Copy, Default)]
pub struct TplRefDs {
    /// C `picture_number`.
    pub picture_number: u64,
    /// Index into [`TplFrameCtx::ref_pics`]; `usize::MAX` mirrors C's NULL.
    pub pic_index: usize,
}

/// The `pcs->tpl_data` fields the dispenser reads (pcs.h:490-510 subset).
#[derive(Debug, Clone)]
pub struct TplData {
    /// C `tpl_ref0_count` / `tpl_ref1_count`.
    pub tpl_ref0_count: u8,
    /// See [`TplData::tpl_ref0_count`].
    pub tpl_ref1_count: u8,
    /// C `ref_in_slide_window` — ref lives inside the TPL window, so its
    /// predictor is the TPL RECON buffer rather than the source.
    pub ref_in_slide_window: [[bool; REF_LIST_MAX_DEPTH]; 2],
    /// C `ref_tpl_group_idx` — window index of an in-window ref, -1 outside.
    pub ref_tpl_group_idx: [[i32; REF_LIST_MAX_DEPTH]; 2],
    /// C `tpl_ref_ds_ptr_array`.
    pub tpl_ref_ds: [[TplRefDs; REF_LIST_MAX_DEPTH]; 2],
    /// C `tpl_slice_type`.
    pub tpl_slice_type: crate::port_picstruct::SliceType,
    /// C `tpl_temporal_layer_index`.
    pub tpl_temporal_layer_index: u8,
    /// C `is_ref`.
    pub is_ref: bool,
    /// C `tpl_decode_order`.
    pub tpl_decode_order: i32,
}

impl Default for TplData {
    /// C zeroes the struct (`EB_MEMSET` in `tpl_prep_info`); `B_SLICE` is
    /// enum 0.
    fn default() -> Self {
        Self {
            tpl_ref0_count: 0,
            tpl_ref1_count: 0,
            ref_in_slide_window: [[false; REF_LIST_MAX_DEPTH]; 2],
            ref_tpl_group_idx: [[0; REF_LIST_MAX_DEPTH]; 2],
            tpl_ref_ds: [[TplRefDs::default(); REF_LIST_MAX_DEPTH]; 2],
            tpl_slice_type: crate::port_picstruct::SliceType::B,
            tpl_temporal_layer_index: 0,
            is_ref: false,
            tpl_decode_order: 0,
        }
    }
}

/// A luma plane TPL reads — input source, PA-source reference, or a
/// completed TPL recon.
#[derive(Debug, Clone, Copy)]
pub struct TplPic<'a> {
    /// `y_buffer`.
    pub y: &'a [u8],
    /// `y_stride`.
    pub y_stride: usize,
    /// `width` (coded).
    pub width: u32,
    /// `height` (coded).
    pub height: u32,
    /// `max_width` — the ALLOCATED extent the TPL_PAD MV clamps read.
    pub max_width: u32,
    /// `max_height`.
    pub max_height: u32,
    /// Index of frame position (0, 0) inside `y` — nonzero for padded planes.
    pub origin: usize,
}

/// The mutable twin of [`TplPic`] for the recon being written.
#[derive(Debug)]
pub struct TplPicMut<'a> {
    /// `y_buffer`.
    pub y: &'a mut [u8],
    /// `y_stride`.
    pub y_stride: usize,
    /// `width`.
    pub width: u32,
    /// `height` .
    pub height: u32,
    /// `border` — the pad width `svt_aom_generate_padding` re-extends.
    pub border: usize,
    /// Index of frame position (0, 0) inside `y`.
    pub origin: usize,
}

/// `CodedBlockStats` (utility.h:71-79) — the z-order 64x64-block table the
/// dispenser indexes through [`TPL_BLK_IDX_TAB`].
#[derive(Debug, Clone, Copy)]
pub struct CodedBlockStats {
    /// C `depth`.
    pub depth: u8,
    /// C `size`.
    pub size: u8,
    /// C `size_log2`.
    pub size_log2: u8,
    /// C `org_x`.
    pub org_x: u8,
    /// C `org_y`.
    pub org_y: u8,
    /// C `cu_num_in_depth`.
    pub cu_num_in_depth: u8,
    /// C `parent32x32_index`.
    pub parent32x32_index: u8,
}

/// C `coded_unit_stats_array` (utility.c:34-124) — `svt_aom_get_coded_blk_stats`'s
/// table. The dispenser only reads `size`/`org_x`/`org_y`; the full row is
/// kept because the table is a faithful copy, not a projection.
#[rustfmt::skip]
pub const CODED_UNIT_STATS: [CodedBlockStats; 85] = {
    const fn s(depth: u8, size: u8, size_log2: u8, org_x: u8, org_y: u8, num: u8, p32: u8) -> CodedBlockStats {
        CodedBlockStats { depth, size, size_log2, org_x, org_y, cu_num_in_depth: num, parent32x32_index: p32 }
    }
    [
        s(0, 64, 6, 0, 0, 0, 0), // 0
        s(1, 32, 5, 0, 0, 0, 1), // 1
        s(2, 16, 4, 0, 0, 0, 1), // 2
        s(3, 8, 3, 0, 0, 0, 1), // 3
        s(3, 8, 3, 8, 0, 1, 1), // 4
        s(3, 8, 3, 0, 8, 8, 1), // 5
        s(3, 8, 3, 8, 8, 9, 1), // 6
        s(2, 16, 4, 16, 0, 1, 1), // 7
        s(3, 8, 3, 16, 0, 2, 1), // 8
        s(3, 8, 3, 24, 0, 3, 1), // 9
        s(3, 8, 3, 16, 8, 10, 1), // 10
        s(3, 8, 3, 24, 8, 11, 1), // 11
        s(2, 16, 4, 0, 16, 4, 1), // 12
        s(3, 8, 3, 0, 16, 16, 1), // 13
        s(3, 8, 3, 8, 16, 17, 1), // 14
        s(3, 8, 3, 0, 24, 24, 1), // 15
        s(3, 8, 3, 8, 24, 25, 1), // 16
        s(2, 16, 4, 16, 16, 5, 1), // 17
        s(3, 8, 3, 16, 16, 18, 1), // 18
        s(3, 8, 3, 24, 16, 19, 1), // 19
        s(3, 8, 3, 16, 24, 26, 1), // 20
        s(3, 8, 3, 24, 24, 27, 1), // 21
        s(1, 32, 5, 32, 0, 1, 2), // 22
        s(2, 16, 4, 32, 0, 2, 2), // 23
        s(3, 8, 3, 32, 0, 4, 2), // 24
        s(3, 8, 3, 40, 0, 5, 2), // 25
        s(3, 8, 3, 32, 8, 12, 2), // 26
        s(3, 8, 3, 40, 8, 13, 2), // 27
        s(2, 16, 4, 48, 0, 3, 2), // 28
        s(3, 8, 3, 48, 0, 6, 2), // 29
        s(3, 8, 3, 56, 0, 7, 2), // 30
        s(3, 8, 3, 48, 8, 14, 2), // 31
        s(3, 8, 3, 56, 8, 15, 2), // 32
        s(2, 16, 4, 32, 16, 6, 2), // 33
        s(3, 8, 3, 32, 16, 20, 2), // 34
        s(3, 8, 3, 40, 16, 21, 2), // 35
        s(3, 8, 3, 32, 24, 28, 2), // 36
        s(3, 8, 3, 40, 24, 29, 2), // 37
        s(2, 16, 4, 48, 16, 7, 2), // 38
        s(3, 8, 3, 48, 16, 22, 2), // 39
        s(3, 8, 3, 56, 16, 23, 2), // 40
        s(3, 8, 3, 48, 24, 30, 2), // 41
        s(3, 8, 3, 56, 24, 31, 2), // 42
        s(1, 32, 5, 0, 32, 2, 3), // 43
        s(2, 16, 4, 0, 32, 8, 3), // 44
        s(3, 8, 3, 0, 32, 32, 3), // 45
        s(3, 8, 3, 8, 32, 33, 3), // 46
        s(3, 8, 3, 0, 40, 40, 3), // 47
        s(3, 8, 3, 8, 40, 41, 3), // 48
        s(2, 16, 4, 16, 32, 9, 3), // 49
        s(3, 8, 3, 16, 32, 34, 3), // 50
        s(3, 8, 3, 24, 32, 35, 3), // 51
        s(3, 8, 3, 16, 40, 42, 3), // 52
        s(3, 8, 3, 24, 40, 43, 3), // 53
        s(2, 16, 4, 0, 48, 12, 3), // 54
        s(3, 8, 3, 0, 48, 48, 3), // 55
        s(3, 8, 3, 8, 48, 49, 3), // 56
        s(3, 8, 3, 0, 56, 56, 3), // 57
        s(3, 8, 3, 8, 56, 57, 3), // 58
        s(2, 16, 4, 16, 48, 13, 3), // 59
        s(3, 8, 3, 16, 48, 50, 3), // 60
        s(3, 8, 3, 24, 48, 51, 3), // 61
        s(3, 8, 3, 16, 56, 58, 3), // 62
        s(3, 8, 3, 24, 56, 59, 3), // 63
        s(1, 32, 5, 32, 32, 3, 4), // 64
        s(2, 16, 4, 32, 32, 10, 4), // 65
        s(3, 8, 3, 32, 32, 36, 4), // 66
        s(3, 8, 3, 40, 32, 37, 4), // 67
        s(3, 8, 3, 32, 40, 44, 4), // 68
        s(3, 8, 3, 40, 40, 45, 4), // 69
        s(2, 16, 4, 48, 32, 11, 4), // 70
        s(3, 8, 3, 48, 32, 38, 4), // 71
        s(3, 8, 3, 56, 32, 39, 4), // 72
        s(3, 8, 3, 48, 40, 46, 4), // 73
        s(3, 8, 3, 56, 40, 47, 4), // 74
        s(2, 16, 4, 32, 48, 14, 4), // 75
        s(3, 8, 3, 32, 48, 52, 4), // 76
        s(3, 8, 3, 40, 48, 53, 4), // 77
        s(3, 8, 3, 32, 56, 60, 4), // 78
        s(3, 8, 3, 40, 56, 61, 4), // 79
        s(2, 16, 4, 48, 48, 15, 4), // 80
        s(3, 8, 3, 48, 48, 54, 4), // 81
        s(3, 8, 3, 56, 48, 55, 4), // 82
        s(3, 8, 3, 48, 56, 62, 4), // 83
        s(3, 8, 3, 56, 56, 63, 4), // 84
    ]
};

// =============================================================================
// Neighbor-sample staging (enc_intra_prediction.c:731-927)
// =============================================================================

pub use svtav1_types::math::rd::rdcost_i64 as rdcost_tpl;
#[cfg(test)]
mod tests;
mod search;
pub use search::*;
mod dispenser;
pub use dispenser::*;

mod model;
pub use model::*;
