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
/// C `TPL_DEP_COST_SCALE_LOG2` (pcs.h or definitions) — fixed-point scale of
/// the stored stats.
pub const TPL_DEP_COST_SCALE_LOG2: u32 = 7;
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

/// C `get_neighbor_samples_dc` (src_ops_process.c:359-376) — copy the
/// block's real above/left neighbours from an already-reconstructed buffer.
///
/// `above`/`left` carry the C convention: index 0 is the corner
/// (`above0_row[-1]` / `left0_col[-1]`).
pub fn get_neighbor_samples_dc(
    src: &[u8],
    src_stride: usize,
    origin: usize,
    above: &mut [u8],
    left: &mut [u8],
    bsize: usize,
) {
    // top left
    above[0] = src[origin - src_stride - 1];
    left[0] = above[0];
    // top
    above[1..1 + bsize].copy_from_slice(&src[origin - src_stride..origin - src_stride + bsize]);
    // left
    let mut read = origin - 1;
    for l in left.iter_mut().take(1 + bsize).skip(1) {
        *l = src[read];
        read += src_stride;
    }
}

/// C `svt_aom_update_neighbor_samples_array_open_loop_mb`
/// (enc_intra_prediction.c:731-816) on a flat buffer: the C version reads
/// `input_ptr->y_buffer`; passing `(buf, stride, width, height)` makes this
/// one signature serve both the source and the `_recon` variant
/// (enc_intra_prediction.c:818-927 — byte-identical except it takes the
/// buffer + dims directly rather than through `EbPictureBufferDesc`).
#[allow(clippy::too_many_arguments)]
pub fn update_neighbor_samples_open_loop(
    use_top_right_bottom_left: bool,
    update_top_neighbor: bool,
    buf: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    src_origin_x: u32,
    src_origin_y: u32,
    bwidth: usize,
    bheight: usize,
    above_ref: &mut [u8],
    left_ref: &mut [u8],
) {
    let block_width_neigh = if use_top_right_bottom_left {
        bwidth << 1
    } else {
        bwidth
    };
    let block_height_neigh = if use_top_right_bottom_left {
        bheight << 1
    } else {
        bheight
    };
    let origin = src_origin_y as usize * stride + src_origin_x as usize;

    // C initialises above to 127 and left to 129 INCLUDING the corner slot.
    for v in above_ref.iter_mut().take(block_width_neigh + 1) {
        *v = 127;
    }
    for v in left_ref.iter_mut().take(block_height_neigh + 1) {
        *v = 129;
    }

    // Upper left sample. C writes it into above_ref[0]/left_ref[0] then
    // bumps both pointers — same as the [0]=corner, [1..]=row convention.
    if src_origin_x != 0 && src_origin_y != 0 {
        let t = buf[origin - stride - 1];
        above_ref[0] = t;
        left_ref[0] = t;
    } else {
        above_ref[0] = 128;
        left_ref[0] = 128;
    }

    // Left column.
    let mut count = block_width_neigh;
    if src_origin_x != 0 {
        let mut read = origin - 1;
        if src_origin_y == 0 {
            // C writes the top-row pixel into left_ref[-1]... i.e. [0].
            left_ref[0] = buf[read];
        }
        count = if src_origin_y + count as u32 > height {
            count - ((src_origin_y + count as u32) - height) as usize
        } else {
            count
        };
        // C writes left_ref[1..=count].
        for l in left_ref.iter_mut().take(1 + count).skip(1) {
            *l = buf[read];
            read += stride;
        }
        if use_top_right_bottom_left {
            // pad unknown left-bottom pixels with value at (-1, -15):
            // left_ref[bheight+1 ..= 2*bheight] = left_ref[bheight].
            let v = left_ref[bheight];
            for l in left_ref.iter_mut().take(1 + block_height_neigh).skip(1 + bheight) {
                *l = v;
            }
        }
    } else if src_origin_y != 0 {
        count = if src_origin_y + count as u32 > height {
            count - ((src_origin_y + count as u32) - height) as usize
        } else {
            count
        };
        // C: EB_MEMSET(left_ref - 1, src_ptr[-stride], count + 1) — the corner
        // AND the first `count` left entries all take the above row's pixel;
        // *(above_ref - 1) = same. With [0]=corner that is:
        let t = buf[origin - stride];
        for l in left_ref.iter_mut().take(1 + count) {
            *l = t;
        }
        above_ref[0] = t;
    }

    // Top row.
    let mut count = block_width_neigh;
    if src_origin_y != 0 {
        if update_top_neighbor {
            let read = origin - stride;
            count = if src_origin_x + count as u32 > width {
                count - ((src_origin_x + count as u32) - width) as usize
            } else {
                count
            };
            above_ref[1..1 + count].copy_from_slice(&buf[read..read + count]);
        }
        // pad unknown top-right pixels with value at (15, -1)
        if use_top_right_bottom_left && src_origin_x != 0 {
            let v = above_ref[bwidth];
            for a in above_ref.iter_mut().take(1 + block_width_neigh).skip(1 + bwidth) {
                *a = v;
            }
        }
    } else if src_origin_x != 0 {
        count = if src_origin_x + count as u32 > width {
            count - ((src_origin_x + count as u32) - width) as usize
        } else {
            count
        };
        // C: EB_MEMSET(above_ref - 1, *(left_ref - count), count + 1) —
        // `left_ref` has already advanced to base+1+block_width_neigh (the
        // left column fill ran first, src_origin_x != 0), so the value is
        // `left[1 + block_width_neigh - count]`: the LEFT sample `count`
        // rows up from the fill's end — the corner plus `count` above
        // entries all take it.
        let v = left_ref[1 + block_width_neigh - count];
        for a in above_ref.iter_mut().take(1 + count) {
            *a = v;
        }
    }
}

// =============================================================================
// init_xd_tpl + MV cost params (src_ops_process.c:384-419)
// =============================================================================

/// The `MacroBlockD` fields TPL uses (`init_xd_tpl`, src_ops_process.c:404).
#[derive(Debug, Clone, Copy)]
pub struct TplXd {
    /// `xd->mb_to_top_edge`.
    pub to_top: i32,
    /// `xd->mb_to_bottom_edge`.
    pub to_bottom: i32,
    /// `xd->mb_to_left_edge`.
    pub to_left: i32,
    /// `xd->mb_to_right_edge`.
    pub to_right: i32,
    /// `xd->mi_row`.
    pub mi_row: i32,
    /// `xd->mi_col`.
    pub mi_col: i32,
}

impl TplXd {
    /// `init_xd_tpl` — `mi_row`/`mi_col` are the block's position, the edges
    /// are `(0,0)`-relative distance-to-frame-edge in pixels.
    pub fn new(mi_rows: i32, mi_cols: i32, block_size: BlockSize, mb_origin_x: u32, mb_origin_y: u32) -> Self {
        const MI_SIZE: i32 = 4;
        let bw = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[block_size as usize] as i32;
        let bh = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[block_size as usize] as i32;
        let mi_row = (mb_origin_y >> 2) as i32;
        let mi_col = (mb_origin_x >> 2) as i32;
        Self {
            to_top: -((mi_row * MI_SIZE) * 8),
            to_bottom: ((mi_rows - bh - mi_row) * MI_SIZE) * 8,
            to_left: -((mi_col * MI_SIZE) * 8),
            to_right: ((mi_cols - bw - mi_col) * MI_SIZE) * 8,
            mi_row,
            mi_col,
        }
    }

    /// The [`svtav1_dsp::port_subpel_params::MbEdges`] view.
    pub fn edges(&self) -> svtav1_dsp::port_subpel_params::MbEdges {
        svtav1_dsp::port_subpel_params::MbEdges {
            to_left: self.to_left,
            to_right: self.to_right,
            to_top: self.to_top,
            to_bottom: self.to_bottom,
        }
    }
}

/// C `svt_tpl_init_mv_cost_params` (src_ops_process.c:384-404) — TPL always
/// uses `MV_COST_NONE` (no rate update inside the dispenser). C also writes
/// `full_ref_mv` and `sad_per_bit`, which no `mcomp.c` consumer reads —
/// the port's [`crate::md_subpel::MvCostParams`] omits both on purpose.
pub fn tpl_init_mv_cost_params(
    ref_mv: Mv,
    rdmult: u32,
) -> crate::md_subpel::MvCostParams<'static> {
    crate::md_subpel::MvCostParams {
        ref_mv,
        mv_cost_type: crate::md_subpel::MvCostType::None,
        tables: None,
        // `AOMMAX(rdmult >> RD_EPB_SHIFT, 1)` — RD_EPB_SHIFT = 6.
        error_per_bit: (rdmult >> 6).max(1) as i32,
        early_exit_th: 0,
    }
}

/// C `tpl_subpel_search` (src_ops_process.c:419-516) — refine the ME MV to
/// sub-pel via `find_best_sub_pixel_tree_pruned`, always `USE_4_TAPS`.
#[allow(clippy::too_many_arguments)]
pub fn tpl_subpel_search(
    tpl_ctrls: &crate::port_picstruct::TplControls,
    update_type: crate::port_picstruct::FrameUpdateType,
    qp_qindex: u8,
    extended_crf_qindex_offset: i32,
    mi_rows: i32,
    mi_cols: i32,
    ref_pic: &TplPic<'_>,
    input_pic: &TplPic<'_>,
    mb_origin_x: u32,
    mb_origin_y: u32,
    bsize: u8,
    best_mv: &mut Mv,
) {
    let block_size = match bsize {
        8 => BlockSize::Block8x8,
        16 => BlockSize::Block16x16,
        32 => BlockSize::Block32x32,
        _ => BlockSize::Block64x64,
    };
    let xd = TplXd::new(mi_rows, mi_cols, block_size, mb_origin_x, mb_origin_y);
    let ref_mv = Mv::ZERO;

    // Full-pel limits, then sub-pel.
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[block_size as usize] as i32;
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[block_size as usize] as i32;
    const AOM_INTERP_EXTEND: i32 = 4;
    const MI_SIZE: i32 = 4;
    let mut full_limits = svtav1_types::motion::FullMvLimits {
        col_min: -(((xd.mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND),
        col_max: (mi_cols - xd.mi_col) * MI_SIZE + AOM_INTERP_EXTEND,
        row_min: -(((xd.mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND),
        row_max: (mi_rows - xd.mi_row) * MI_SIZE + AOM_INTERP_EXTEND,
    };
    crate::intrabc::set_mv_search_range(&mut full_limits, ref_mv);
    let mv_limits = crate::md_subpel::set_subpel_mv_search_range(
        (
            full_limits.col_min,
            full_limits.col_max,
            full_limits.row_min,
            full_limits.row_max,
        ),
        ref_mv,
    );

    // MV cost params — C keys the qindex off static_config.qp through
    // quantizer_to_qindex + the extended CRF offset, clamps to MAXQ, and
    // scales rdmult by TPL_RDMULT_SCALING_FACTOR.
    let qindex = i32::from(qp_qindex) + extended_crf_qindex_offset;
    let qindex = qindex.min(crate::port_rc_vbr_cbr_qpick::MAXQ);
    let rc_update_type = match update_type {
        crate::port_picstruct::FrameUpdateType::Kf => crate::port_rc_process::FrameUpdateType::KfUpdate,
        crate::port_picstruct::FrameUpdateType::Lf => crate::port_rc_process::FrameUpdateType::LfUpdate,
        crate::port_picstruct::FrameUpdateType::Gf => crate::port_rc_process::FrameUpdateType::GfUpdate,
        crate::port_picstruct::FrameUpdateType::Arf => crate::port_rc_process::FrameUpdateType::ArfUpdate,
        crate::port_picstruct::FrameUpdateType::Overlay => crate::port_rc_process::FrameUpdateType::OverlayUpdate,
        crate::port_picstruct::FrameUpdateType::IntnlOverlay => crate::port_rc_process::FrameUpdateType::IntnlOverlayUpdate,
        crate::port_picstruct::FrameUpdateType::IntnlArf => crate::port_rc_process::FrameUpdateType::IntnlArfUpdate,
    };
    let rdmult = crate::port_rc_process::compute_rd_mult_based_on_qindex(
        8, rc_update_type, qindex,
    ) / TPL_RDMULT_SCALING_FACTOR;
    let mv_cost_params = tpl_init_mv_cost_params(ref_mv, rdmult.max(0) as u32);

    let (w, h) = (
        svtav1_dsp::port_obmc_data::block_size_wide(block_size),
        svtav1_dsp::port_obmc_data::block_size_high(block_size),
    );
    let ref_origin = ref_pic.origin + mb_origin_x as usize + mb_origin_y as usize * ref_pic.y_stride;
    let src_origin = input_pic.origin + mb_origin_x as usize + mb_origin_y as usize * input_pic.y_stride;

    let var_params = crate::md_subpel::SubpelSearchVarParams {
        src: input_pic.y,
        src_base: src_origin,
        src_stride: input_pic.y_stride,
        ref_alloc: ref_pic.y,
        ref_base: ref_origin as i64,
        ref_stride: ref_pic.y_stride,
        w,
        h,
        bias_fp: 0,
        subpel_search_type: crate::inter_me::obmc_search::USE_4_TAPS,
    };
    let ms_params = crate::md_subpel::SubpelSearchParams {
        allow_hp: false, // pcs->frm_hdr.allow_high_precision_mv is not set yet
        forced_stop: i32::from(tpl_ctrls.subpel_depth),
        iters_per_step: 2,
        pred_variance_th: 0,
        abs_th_mult: 0,
        round_dev_th: i32::MAX,
        skip_diag_refinement: tpl_ctrls.subpel_diag_refinement,
        search_stage: crate::md_subpel::SPEL_ME,
        list_idx: 0,
        ref_idx: 0,
        mv_limits,
    };

    // C takes best_mv >> 3 as the full-pel start (TODO in C says this should
    // be get_fullmv_from_mv), then get_mv_from_fullmv shifts it back — a
    // floor, not the rounding `to_fullpel` does, so keep the shift.
    let start_mv = Mv {
        x: best_mv.x >> 3,
        y: best_mv.y >> 3,
    }
    .to_subpel();
    let (_besterr, st) = crate::md_subpel::find_best_sub_pixel_tree_pruned(
        None,
        &ms_params,
        &var_params,
        &mv_cost_params,
        start_mv,
        block_size,
    );
    *best_mv = st.best_mv;
}

// =============================================================================
// get_quantize_error + rate_estimator (src_ops_process.c:232-273)
// =============================================================================

/// C `svt_av1_block_error` (`aom_dsp_rtcd`): `error = Σ(coeff-dqcoeff)²`,
/// `ssz = Σcoeff²` over `pix_num` raster entries.
fn av1_block_error(coeff: &[i32], dqcoeff: &[i32], pix_num: usize) -> (i64, i64) {
    let mut error = 0i64;
    let mut sqcoeff = 0i64;
    for i in 0..pix_num {
        let diff = i64::from(coeff[i]) - i64::from(dqcoeff[i]);
        error += diff * diff;
        sqcoeff += i64::from(coeff[i]) * i64::from(coeff[i]);
    }
    (error, sqcoeff)
}

/// C `get_quantize_error` (src_ops_process.c:232-257): FP-quantize `coeff`
/// with the TPL quant table, then `recon_error`/`sse` are the block errors
/// shifted down 2 (0 at TX_32X32) and floored at 1.
#[allow(clippy::too_many_arguments)]
pub fn get_quantize_error(
    quant: &crate::quant::QuantTable,
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    tx_size: TxSize,
    eob: &mut u16,
    recon_error: &mut i64,
    sse: &mut i64,
) {
    let scan = crate::entropy::scan_tables::scan(tx_size as usize, 0);
    // `1 << num_pels_log2_lookup[txsize_to_bsize[tx_size]]` — every TPL tx
    // is `size x (size >> subsample)` so the pixel count is w*h directly.
    let (txw, txh) = tx_dims(tx_size);
    let pix_num = txw * txh;
    let shift: i64 = if tx_size == TxSize::Tx32x32 { 0 } else { 2 };

    // C `svt_av1_quantize_fp` — `quantize_fp_helper_c` `(void)`-ignores its
    // `zbin_ptr` and `quant_shift_ptr`, so the `mb_plane.zbin_qtx`/
    // `quant_shift_qtx` rows C loads are inert here.
    *eob = crate::quant::quantize_fp(coeff, scan, quant, 0, qcoeff, dqcoeff);

    let (err, sq) = av1_block_error(coeff, dqcoeff, pix_num);
    *recon_error = (err >> shift).max(1);
    *sse = (sq >> shift).max(1);
}

/// C `rate_estimator` (src_ops_process.c:258-272): `eob + 1` plus
/// `floor(log2(level+1)) + (level > 0)` per coded coeff, in
/// `AV1_PROB_COST_SHIFT` fixed point.
pub fn rate_estimator(qcoeff: &[i32], eob: usize, tx_size: TxSize) -> i32 {
    let scan = crate::entropy::scan_tables::scan(tx_size as usize, 0);
    const AV1_PROB_COST_SHIFT: i32 = 9;
    let mut rate_cost = eob as i32 + 1;
    for idx in 0..eob {
        let abs_level = qcoeff[scan[idx] as usize].abs();
        // `svt_log2f` = get_msb = floor(log2(x)).
        rate_cost += msb(abs_level as u32 + 1) + i32::from(abs_level > 0);
    }
    rate_cost << AV1_PROB_COST_SHIFT
}

/// C `svt_log2f` / `get_msb` (definitions.h:613): position of the top bit.
fn msb(x: u32) -> i32 {
    31 - x.leading_zeros() as i32
}

// =============================================================================
// result_model_store (src_ops_process.c:273-352)
// =============================================================================

/// C `result_model_store`: clamp stats to >= 1 and write them into the
/// `tpl_stats` grid at `synth_blk_size` granularity (32/16/8 — the 8x8 arm
/// also normalizes a 32- or 16-sized result down).
pub fn result_model_store(
    tpl_stats: &mut TplStats,
    tpl_stats_grid: &mut [TplStats],
    synth_blk_size: u8,
    aligned_width: u32,
    mb_origin_x: u32,
    mb_origin_y: u32,
    size: u32,
) {
    tpl_stats.srcrf_dist = tpl_stats.srcrf_dist.max(1);
    tpl_stats.recrf_dist = tpl_stats.recrf_dist.max(1);
    tpl_stats.srcrf_rate = tpl_stats.srcrf_rate.max(1);
    tpl_stats.recrf_rate = tpl_stats.recrf_rate.max(1);

    if synth_blk_size == 32 {
        let stride = (aligned_width as usize + 31) / 32;
        let idx = (mb_origin_y as usize >> 5) * stride + (mb_origin_x as usize >> 5);
        tpl_stats_grid[idx] = *tpl_stats;
    } else if synth_blk_size == 16 {
        let stride = (aligned_width as usize + 15) / 16;
        let idx = (mb_origin_y as usize >> 4) * stride + (mb_origin_x as usize >> 4);
        if size == 32 {
            // normalize based on the block size 16x16
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 4).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 4).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 4).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 4).max(1);
            tpl_stats_grid[idx] = *tpl_stats;
            tpl_stats_grid[idx + 1] = *tpl_stats;
            tpl_stats_grid[idx + stride] = *tpl_stats;
            tpl_stats_grid[idx + stride + 1] = *tpl_stats;
        } else if size == 16 {
            tpl_stats_grid[idx] = *tpl_stats;
        }
    } else {
        // small resolution: 16x16 data duplicated on an 8x8 grid
        let stride = ((aligned_width as usize + 15) / 16) << 1;
        let idx = (mb_origin_y as usize >> 3) * stride + (mb_origin_x as usize >> 3);
        if size == 32 {
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 16).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 16).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 16).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 16).max(1);
            for r in 0..4usize {
                for c in 0..4usize {
                    tpl_stats_grid[idx + r * stride + c] = *tpl_stats;
                }
            }
        } else if size == 16 {
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 4).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 4).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 4).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 4).max(1);
            for r in 0..2usize {
                for c in 0..2usize {
                    tpl_stats_grid[idx + r * stride + c] = *tpl_stats;
                }
            }
        }
    }
}

// =============================================================================
// tpl_mc_flow_dispenser_sb_generic (src_ops_process.c:517-1202)
// =============================================================================

/// Per-SB inputs to [`tpl_mc_flow_dispenser_sb_generic`], bundling the C
/// `pcs` / `enc_ctx` / `scs` fields the function reads.
pub struct TplSbCtx<'a> {
    /// C `pcs->enhanced_pic` — the input source plane.
    pub input_pic: TplPic<'a>,
    /// C `pcs->tpl_ctrls`.
    pub tpl_ctrls: crate::port_picstruct::TplControls,
    /// C `pcs->tpl_data`.
    pub tpl_data: &'a TplData,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->slice_type`.
    pub slice_type: crate::port_picstruct::SliceType,
    /// C `pcs->update_type` — for the subpel rdmult.
    pub update_type: crate::port_picstruct::FrameUpdateType,
    /// C `scs->static_config.qp` already mapped through
    /// `quantizer_to_qindex` — the qindex the subpel rdmult uses.
    pub qp_qindex: u8,
    /// C `scs->static_config.extended_crf_qindex_offset`.
    pub extended_crf_qindex_offset: i32,
    /// C `pcs->tpl_src_data_ready` — reuse cached src stats (tpl_lad_mg>0
    /// re-runs a frame inside a later window).
    pub tpl_src_data_ready: bool,
    /// C `pcs->enable_me_16x16` — scales `me_mb_offset`.
    pub enable_me_16x16: bool,
    /// C `scs->tpl_lad_mg` — whether `tpl_src_stats_buffer` is live.
    pub tpl_lad_mg: u8,
    /// C `scs->b64_geom[sb_index]` reduced to the SB origin.
    pub sb_origin: (u32, u32),
    /// C `pcs->aligned_width`.
    pub aligned_width: u32,
    /// C `(pcs->aligned_width + 15) >> 4` — the `tpl_src_stats_buffer` stride.
    pub aligned16_width: usize,
    /// C `cm->mi_rows` / `cm->mi_cols`.
    pub mi_rows: i32,
    /// See [`TplSbCtx::mi_rows`].
    pub mi_cols: i32,
    /// C `scs->max_input_luma_width` / `max_input_luma_height` — the
    /// `svt_aom_filter_intra_edge` bounds args.
    pub max_input_luma_width: u32,
    /// See [`TplSbCtx::max_input_luma_width`].
    pub max_input_luma_height: u32,
    /// C `scs->super_block_size` — for [`svtav1_dsp::port_subpel_params::RefGeometry`].
    pub super_block_size: i32,
    /// C `scs->sf_identity`.
    pub sf_identity: svtav1_dsp::port_scale_factors::ScaleFactors,
    /// The `quants_8bit` row for this picture's TPL qindex — C indexes
    /// `scs->enc_ctx->quants_8bit.y_*[qIndex]` per SB call.
    pub quant: crate::quant::QuantTable,
    /// C `enc_ctx->poc_map_idx` — TPL window index -> POC.
    pub poc_map_idx: &'a [u64],
    /// C `pcs->tpl_data.base_pcs->tpl_valid_pic` — which earlier window
    /// frames have a usable TPL recon.
    pub base_tpl_valid_pic: &'a [u8],
}

/// The ME results for one superblock — the `pa_me_data->me_results[sb_index]`
/// subset the dispenser iterates.
pub struct TplMeResults<'a> {
    /// C `me_results->total_me_candidate_index` (indexed by `me_mb_offset`).
    pub total_me_candidate_index: &'a [u8],
    /// C `me_results->me_candidate_array`
    /// (`me_mb_offset * max_cand + cand`).
    pub me_candidate_array: &'a [crate::inter_me::context::MeCandidate],
    /// C `me_results->me_mv_array`
    /// (`me_mb_offset * max_refs + (list ? max_l0 : 0) + ref_idx`), full-pel.
    pub me_mv_array: &'a [Mv],
    /// C `pa_me_data->max_cand`.
    pub max_cand: usize,
    /// C `pa_me_data->max_refs`.
    pub max_refs: usize,
    /// C `pa_me_data->max_l0`.
    pub max_l0: usize,
}

/// C `svt_aom_filter_intra_edge` (intra_prediction.c:2597) restricted to the
/// directional arm — the only mode class TPL ever calls it with, so the
/// `extend_modes` flags are always overridden by the `p_angle` chain. Fixed
/// TX_16X16 geometry. `above`/`left` are the C `above_row`/`left_col`
/// pointers: index `origin` is row sample 0, `origin - 1` the corner.
#[allow(clippy::too_many_arguments)]
fn tpl_filter_intra_edge(
    p_angle: i32,
    max_frame_width: u32,
    max_frame_height: u32,
    cu_origin_x: u32,
    cu_origin_y: u32,
    above: &mut [u8],
    left: &mut [u8],
    origin: usize,
) {
    use svtav1_dsp::intra_pred as ip;
    let mb_stride = (max_frame_width + 15) / 16;
    let mb_height = (max_frame_height + 15) / 16;
    let txwpx = 16i32; // TX_16X16
    let txhpx = 16i32;
    let need_right = p_angle < 90;
    let need_bottom = p_angle > 180;
    // C reads extend_modes[mode] first, then the directional arm overrides
    // all three flags — the initial values are dead here, so we keep only
    // the override (the caller only reaches us for directional modes).
    let (need_above, need_left) = if p_angle <= 90 {
        (true, false)
    } else if p_angle < 180 {
        (true, true)
    } else {
        (false, true)
    };
    let need_above_left = true;
    let n_top_px = if cu_origin_y > 0 {
        txwpx.min((mb_stride * 16) as i32 - cu_origin_x as i32 + txwpx)
    } else {
        0
    };
    let n_left_px = if cu_origin_x > 0 {
        txhpx.min((mb_height * 16) as i32 - cu_origin_y as i32 + txhpx)
    } else {
        0
    };

    if p_angle != 90 && p_angle != 180 {
        let ab_le = if need_above_left { 1usize } else { 0 };
        if need_above && need_left && (txwpx + txhpx >= 24) {
            ip::filter_intra_edge_corner(above, left, origin);
        }
        if need_above && n_top_px > 0 {
            let strength =
                ip::intra_edge_filter_strength(txwpx, txhpx, p_angle - 90, 0);
            let n_px =
                n_top_px as usize + ab_le + if need_right { txhpx as usize } else { 0 };
            ip::filter_intra_edge(above, origin - ab_le, n_px, strength);
        }
        if need_left && n_left_px > 0 {
            let strength =
                ip::intra_edge_filter_strength(txhpx, txwpx, p_angle - 180, 0);
            let n_px =
                n_left_px as usize + ab_le + if need_bottom { txwpx as usize } else { 0 };
            ip::filter_intra_edge(left, origin - ab_le, n_px, strength);
        }
    }
    if need_above
        && ip::use_intra_edge_upsample(txwpx, txhpx, p_angle - 90, 0)
    {
        let n_px = txwpx as usize + if need_right { txhpx as usize } else { 0 };
        ip::upsample_intra_edge(above, origin, n_px);
    }
    if need_left
        && ip::use_intra_edge_upsample(txhpx, txwpx, p_angle - 180, 0)
    {
        let n_px = txhpx as usize + if need_bottom { txwpx as usize } else { 0 };
        ip::upsample_intra_edge(left, origin, n_px);
    }
}

/// The intra-prediction neighbor layout TPL uses: the C arrays have the
/// corner at `data[MAX_TPL_SIZE - 1]` and `row = data + MAX_TPL_SIZE`.
const TPL_NEIGH_SZ: usize = MAX_TPL_SIZE * 4 + 1;

/// C `pcs->tpl_ctrls.pf_shape` (`u8`) -> [`TxCoeffShape`]; C's field can
/// only ever hold DEFAULT/N2/N4 — `svt_aom_set_tpl_params` assigns literals.
fn tpl_pf_shape(t: &crate::port_picstruct::TplControls) -> svtav1_dsp::fwd_txfm_pf::TxCoeffShape {
    use svtav1_dsp::fwd_txfm_pf::TxCoeffShape as S;
    match t.pf_shape {
        crate::port_picstruct::N2_SHAPE => S::N2,
        crate::port_picstruct::N4_SHAPE => S::N4,
        _ => S::Default,
    }
}

/// `(tx_size_wide, tx_size_high)` for the square-subsample TX family TPL
/// uses — the only shapes `TX_SIZE_ARRAY`/`SUB2`/`SUB4` can produce.
fn tx_dims(tx: TxSize) -> (usize, usize) {
    match tx {
        TxSize::Tx16x16 => (16, 16),
        TxSize::Tx32x32 => (32, 32),
        TxSize::Tx64x64 => (64, 64),
        TxSize::Tx16x8 => (16, 8),
        TxSize::Tx32x16 => (32, 16),
        TxSize::Tx64x32 => (64, 32),
        TxSize::Tx16x4 => (16, 4),
        TxSize::Tx32x8 => (32, 8),
        TxSize::Tx64x16 => (64, 16),
        other => unreachable!("tpl tx shape {other:?}"),
    }
}

/// u8 -> [`svtav1_types::prediction::PredictionMode`] for the intra modes
/// TPL loops over (`DC_PRED..=PAETH_PRED`). `None` for out-of-range — the
/// callers keep C's behaviour of never asking for one.
fn tpl_mode(v: u8) -> Option<svtav1_types::prediction::PredictionMode> {
    use svtav1_types::prediction::PredictionMode as M;
    Some(match v {
        0 => M::DcPred,
        1 => M::VPred,
        2 => M::HPred,
        3 => M::D45Pred,
        4 => M::D135Pred,
        5 => M::D113Pred,
        6 => M::D157Pred,
        7 => M::D203Pred,
        8 => M::D67Pred,
        9 => M::SmoothPred,
        10 => M::SmoothVPred,
        11 => M::SmoothHPred,
        12 => M::PaethPred,
        _ => return None,
    })
}

/// Fill one mode's neighbors + edge filter, predict into `pred`, and score —
/// the shared body of the src-path and recon-path intra searches.
/// `above0`/`left0` carry the [0]=corner convention from
/// [`update_neighbor_samples_open_loop`].
#[allow(clippy::too_many_arguments)]
fn tpl_predict_intra(
    mode_u8: u8,
    p_angle: i32,
    max_w: u32,
    max_h: u32,
    mb_origin_x: u32,
    mb_origin_y: u32,
    above0: &[u8],
    left0: &[u8],
    scratch_above: &mut [u8],
    scratch_left: &mut [u8],
    size: usize,
    pred: &mut [u8],
    pred_stride: usize,
) {
    let mode = tpl_mode(mode_u8).expect("TPL iterates DC..=PAETH");
    let (above_row, left_row, corner): (&[u8], &[u8], u8) = if crate::intra_open_loop::is_directional_mode(mode) {
        scratch_above.copy_from_slice(above0);
        scratch_left.copy_from_slice(left0);
        // above0 is a `TPL_NEIGH_SZ` slice with the corner at index 0 and
        // the row at 1.. — but C's arrays put the corner at
        // `data[MAX_TPL_SIZE-1]`, row at `data+MAX_TPL_SIZE`. The scratch
        // copies keep the same layout, and `tpl_filter_intra_edge` takes
        // `origin` = the row start index inside the slice.
        tpl_filter_intra_edge(
            p_angle,
            max_w,
            max_h,
            mb_origin_x,
            mb_origin_y,
            scratch_above,
            scratch_left,
            1, // corner at [0], row at [1..]
        );
        (&scratch_above[1..], &scratch_left[1..], scratch_above[0])
    } else {
        (&above0[1..], &left0[1..], above0[0])
    };
    let n = crate::intra_open_loop::Neighbours {
        above: &above_row[..size],
        left: &left_row[..size],
        top_left: corner,
        has_left: mb_origin_x > 0,
        has_above: mb_origin_y > 0,
    };
    // C returns EB_ErrorNone even when the dr leaf falls through on a bad
    // angle — unreachable here because `p_angle` comes from
    // `mode_to_angle_map` exactly as in C.
    let _ = crate::intra_open_loop::intra_prediction_open_loop_mb(
        mode, p_angle, n, size, size, pred, pred_stride,
    );
}

/// C `tpl_mc_flow_dispenser_sb_generic` — one SB of the TPL fake-encode.
///
/// `recon` is `enc_ctx->mc_flow_rec_picture_buffer[frame_idx]`; `rec_refs`
/// is the window's recon list. `ref_pics` is the two-list view of
/// `pcs->tpl_data.tpl_ref_ds_ptr_array` (downscaled PA sources) and
/// `me_results` the SB's PA ME output.
#[allow(clippy::too_many_arguments)]
pub fn tpl_mc_flow_dispenser_sb_generic(
    ctx: &TplSbCtx<'_>,
    dispenser_search_level: usize,
    recon: &mut TplPicMut<'_>,
    rec_refs: &[TplPic<'_>],
    ref_pics: &[TplPic<'_>],
    me_results: &TplMeResults<'_>,
    tpl_src_stats_buffer: &mut [TplSrcStats],
    tpl_stats_grid: &mut [TplStats],
) {
    use svtav1_dsp::port_masked_compound::subtract_block;

    let size = SIZE_ARRAY[dispenser_search_level] as usize;
    let blk_start = BLK_START_ARRAY[dispenser_search_level];
    let blk_end = BLK_END_ARRAY[dispenser_search_level];

    let tpl_ctrls = &ctx.tpl_ctrls;
    let tx_size = if tpl_ctrls.subsample_tx == 2 {
        SUB4_TX_SIZE_ARRAY[dispenser_search_level]
    } else if tpl_ctrls.subsample_tx == 1 {
        SUB2_TX_SIZE_ARRAY[dispenser_search_level]
    } else {
        TX_SIZE_ARRAY[dispenser_search_level]
    };
    let subsample_tx = tpl_ctrls.subsample_tx as usize;

    let input_pic = ctx.input_pic;
    let src_stride = input_pic.y_stride;
    let dst_buffer_stride = recon.y_stride;
    let (sb_org_x, sb_org_y) = ctx.sb_origin;

    let disable_intra_pred = tpl_ctrls.disable_intra_pred_nref != 0
        && ctx.temporal_layer_index == ctx.hierarchical_levels;
    let intra_dc_sad_path = tpl_ctrls.use_sad_in_src_search != 0
        && tpl_ctrls.intra_mode_end == crate::port_picstruct::DC_PRED;

    let geom = svtav1_dsp::port_subpel_params::RefGeometry {
        super_block_size: ctx.super_block_size,
        frame_width: input_pic.width as i32,
        frame_height: input_pic.height as i32,
    };

    for blk_index in blk_start..=blk_end {
        let z_blk_index = TPL_BLK_IDX_TAB[0][blk_index as usize] as usize;
        let blk_stats = &CODED_UNIT_STATS[z_blk_index];
        let bsize = blk_stats.size as usize;
        let block_size = match bsize {
            8 => BlockSize::Block8x8,
            16 => BlockSize::Block16x16,
            32 => BlockSize::Block32x32,
            _ => BlockSize::Block64x64,
        };
        let mb_origin_x = sb_org_x + u32::from(blk_stats.org_x);
        let mb_origin_y = sb_org_y + u32::from(blk_stats.org_y);

        // at least half of the block inside
        if mb_origin_x as usize + (size >> 1) > input_pic.width as usize
            || mb_origin_y as usize + (size >> 1) > input_pic.height as usize
        {
            continue;
        }

        let xd = TplXd::new(ctx.mi_rows, ctx.mi_cols, block_size, mb_origin_x, mb_origin_y);
        let edges = xd.edges();

        let dst_mb_offset =
            recon.origin + mb_origin_y as usize * dst_buffer_stride + mb_origin_x as usize;
        let src_mb = input_pic.origin + mb_origin_x as usize + mb_origin_y as usize * src_stride;

        let mut recon_error: i64 = 1;
        let mut sse: i64 = 1;
        let mut best_ref_poc: u64 = 0;
        let mut best_rf_idx: i32 = -1;
        let mut final_best_mv = Mv::ZERO;
        let mut best_mode: u8 = DC_PRED;
        let mut tpl_stats = TplStats::default();
        let mut best_intra_mode: u8 = DC_PRED;

        let src_stats_idx = ((mb_origin_y as usize) >> 4) * ctx.aligned16_width
            + ((mb_origin_x as usize) >> 4);

        let mut predictor = [0u8; MAX_TPL_SAMPLES_PER_BLOCK * 2];
        let mut src_diff = [0i16; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut coeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut qcoeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut dqcoeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut best_coeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut compensated_blk = [0u8; MAX_TPL_SAMPLES_PER_BLOCK];

        if !ctx.tpl_src_data_ready {
            let mut best_inter_cost: i64 = i64::MAX;
            let mut best_intra_cost: i64 = i64::MAX;
            if !disable_intra_pred {
                if intra_dc_sad_path {
                    // Fast DC-only + SAD path. C uses the real source
                    // neighbours when the whole block is interior, else the
                    // open-loop edge builder. [0]=corner layout in
                    // `above0`/`left0`.
                    let mut above0 = [0u8; TPL_NEIGH_SZ];
                    let mut left0 = [0u8; TPL_NEIGH_SZ];
                    let mb_inside = mb_origin_x as usize + size <= input_pic.width as usize
                        && mb_origin_y as usize + size <= input_pic.height as usize;
                    if mb_origin_x > 0 && mb_origin_y > 0 && mb_inside {
                        get_neighbor_samples_dc(
                            input_pic.y,
                            src_stride,
                            src_mb,
                            &mut above0,
                            &mut left0,
                            bsize,
                        );
                    } else {
                        update_neighbor_samples_open_loop(
                            true,
                            true,
                            input_pic.y,
                            src_stride,
                            input_pic.width,
                            input_pic.height,
                            mb_origin_x,
                            mb_origin_y,
                            bsize,
                            bsize,
                            &mut above0,
                            &mut left0,
                        );
                    }
                    let n = crate::intra_open_loop::Neighbours {
                        above: &above0[1..1 + size],
                        left: &left0[1..1 + size],
                        top_left: above0[0],
                        has_left: mb_origin_x > 0,
                        has_above: mb_origin_y > 0,
                    };
                    // tx_size_array[search_level] — the FULL (non-subsampled)
                    // block for prediction, C passes `size` as dst_stride.
                    let _ = crate::intra_open_loop::intra_prediction_open_loop_mb(
                        svtav1_types::prediction::PredictionMode::DcPred,
                        0,
                        n,
                        size,
                        size,
                        &mut predictor,
                        size,
                    );
                    best_intra_cost = i64::from(crate::inter_me::sad::nxm_sad_kernel(
                        &input_pic.y[src_mb..],
                        src_stride,
                        &predictor,
                        size,
                        size,
                        size,
                    ));
                } else {
                    let mut above0 = [0u8; TPL_NEIGH_SZ];
                    let mut left0 = [0u8; TPL_NEIGH_SZ];
                    let mut above = [0u8; TPL_NEIGH_SZ];
                    let mut left = [0u8; TPL_NEIGH_SZ];

                    update_neighbor_samples_open_loop(
                        true,
                        true,
                        input_pic.y,
                        src_stride,
                        input_pic.width,
                        input_pic.height,
                        mb_origin_x,
                        mb_origin_y,
                        bsize,
                        bsize,
                        &mut above0,
                        &mut left0,
                    );

                    let intra_mode_end = tpl_ctrls.intra_mode_end;
                    for ois_intra_mode in DC_PRED..=intra_mode_end {
                        let mode = tpl_mode(ois_intra_mode).expect("intra_mode_end <= PAETH");
                        let p_angle = if crate::intra_open_loop::is_directional_mode(mode) {
                            crate::intra_edge::MODE_TO_ANGLE_MAP[ois_intra_mode as usize]
                        } else {
                            0
                        };
                        tpl_predict_intra(
                            ois_intra_mode,
                            p_angle,
                            ctx.max_input_luma_width,
                            ctx.max_input_luma_height,
                            mb_origin_x,
                            mb_origin_y,
                            &above0,
                            &left0,
                            &mut above,
                            &mut left,
                            size,
                            &mut predictor,
                            size,
                        );
                        let intra_cost: i64 = if tpl_ctrls.use_sad_in_src_search != 0 {
                            i64::from(crate::inter_me::sad::nxm_sad_kernel(
                                &input_pic.y[src_mb..],
                                src_stride,
                                &predictor,
                                size,
                                size,
                                size,
                            ))
                        } else {
                            subtract_block(
                                size >> subsample_tx,
                                size,
                                &mut src_diff,
                                size << subsample_tx,
                                &input_pic.y[src_mb..],
                                src_stride << subsample_tx,
                                &predictor,
                                size << subsample_tx,
                            );
                            svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                                &src_diff,
                                size << subsample_tx,
                                &mut coeff,
                                tx_size,
                                tpl_pf_shape(tpl_ctrls),
                                8,
                                false,
                            );
                            i64::from(aom_satd(
                                &coeff[..(size * size) >> subsample_tx],
                            )) << subsample_tx
                        };
                        if intra_cost < best_intra_cost {
                            best_mode = ois_intra_mode;
                            best_intra_cost = intra_cost;
                            best_intra_mode = ois_intra_mode;
                        }
                    }
                }
            }

            // Inter Src path — score each ME candidate against the
            // reference SOURCE pictures.
            let mut me_mb_offset = TPL_BLK_IDX_TAB[1][blk_index as usize] as usize;
            if !ctx.enable_me_16x16 {
                me_mb_offset = (me_mb_offset - 1) / 4;
            }
            let total_me_cnt = if ctx.slice_type == crate::port_picstruct::SliceType::I {
                0
            } else {
                me_results.total_me_candidate_index[me_mb_offset] as usize
            };
            let me_block_results =
                &me_results.me_candidate_array[me_mb_offset * me_results.max_cand..];

            for me_cand in me_block_results.iter().take(total_me_cnt) {
                // consider only single refs
                if me_cand.direction() > 1 {
                    continue;
                }
                let list_index = me_cand.direction() as usize;
                let ref_pic_index = if list_index == 0 {
                    me_cand.ref_idx_l0() as usize
                } else {
                    me_cand.ref_idx_l1() as usize
                };
                // exclude this cand if the reference is within the sliding
                // window and does not have valid TPL recon data
                let ref_grp_idx =
                    ctx.tpl_data.ref_tpl_group_idx[list_index][ref_pic_index] as usize;
                if ref_grp_idx > 0
                    && ctx.base_tpl_valid_pic[ref_grp_idx] == 0
                {
                    continue;
                }
                let rf_idx =
                    crate::port_picstruct::get_ref_frame_type(list_index as u8, ref_pic_index as u8)
                        as i32
                        - 1;
                let me_offset = me_mb_offset * me_results.max_refs
                    + if list_index != 0 { me_results.max_l0 } else { 0 }
                    + ref_pic_index;
                let ref_pic = ref_pics
                    [ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index];

                let mut x_curr_mv = i32::from(me_results.me_mv_array[me_offset].x) * 8;
                let mut y_curr_mv = i32::from(me_results.me_mv_array[me_offset].y) * 8;

                if mb_origin_x as i32 + (x_curr_mv >> 3) < -TPL_PAD {
                    x_curr_mv = (-TPL_PAD - mb_origin_x as i32) * 8;
                }
                if mb_origin_x as i32 + bsize as i32 + (x_curr_mv >> 3)
                    > TPL_PAD + ref_pic.max_width as i32 - 1
                {
                    x_curr_mv = (TPL_PAD + ref_pic.max_width as i32 - 1
                        - (mb_origin_x as i32 + bsize as i32))
                        * 8;
                }
                if mb_origin_y as i32 + (y_curr_mv >> 3) < -TPL_PAD {
                    y_curr_mv = (-TPL_PAD - mb_origin_y as i32) * 8;
                }
                if mb_origin_y as i32 + bsize as i32 + (y_curr_mv >> 3)
                    > TPL_PAD + ref_pic.max_height as i32 - 1
                {
                    y_curr_mv = (TPL_PAD + ref_pic.max_height as i32 - 1
                        - (mb_origin_y as i32 + bsize as i32))
                        * 8;
                }

                let mut best_mv = Mv {
                    x: x_curr_mv as i16,
                    y: y_curr_mv as i16,
                };
                if tpl_ctrls.subpel_depth != crate::port_picstruct::FULL_PEL {
                    tpl_subpel_search(
                        tpl_ctrls,
                        ctx.update_type,
                        ctx.qp_qindex,
                        ctx.extended_crf_qindex_offset,
                        ctx.mi_rows,
                        ctx.mi_cols,
                        &ref_pic,
                        &input_pic,
                        mb_origin_x,
                        mb_origin_y,
                        bsize as u8,
                        &mut best_mv,
                    );
                }
                let ref_origin_index = (ref_pic.origin as i32
                    + mb_origin_x as i32
                    + (best_mv.x as i32 / 8)
                    + (mb_origin_y as i32 + (best_mv.y as i32 / 8)) * ref_pic.y_stride as i32)
                    as usize;

                let subpel_mv = (best_mv.x & 0x7) != 0 || (best_mv.y & 0x7) != 0;
                if subpel_mv {
                    let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                    let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                        false,
                        MAX_TPL_SIZE,
                        false,
                        8,
                    );
                    let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                        svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                        ref_pic.origin,
                        ref_pic.y_stride,
                        svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(&mut compensated_blk),
                        size,
                        &mut conv_buf,
                        mb_origin_y as i32,
                        mb_origin_x as i32,
                        svtav1_dsp::port_subpel_params::Mv { x: best_mv.x, y: best_mv.y },
                        &ctx.sf_identity,
                        &cp,
                        0, // interp_filters
                        None,
                        None,
                        geom,
                        bsize,
                        bsize,
                        &edges,
                        0,
                        0,
                        0,
                        8,
                        false,
                        false,
                    );
                }

                let inter_cost: i64 = if tpl_ctrls.use_sad_in_src_search != 0 {
                    i64::from(crate::inter_me::sad::nxm_sad_kernel(
                        &input_pic.y[src_mb..],
                        src_stride,
                        if subpel_mv { &compensated_blk } else { &ref_pic.y[ref_origin_index..] },
                        if subpel_mv { size } else { ref_pic.y_stride },
                        size,
                        size,
                    ))
                } else {
                    subtract_block(
                        size >> subsample_tx,
                        size,
                        &mut src_diff,
                        size << subsample_tx,
                        &input_pic.y[src_mb..],
                        src_stride << subsample_tx,
                        if subpel_mv { &compensated_blk } else { &ref_pic.y[ref_origin_index..] },
                        (if subpel_mv { size } else { ref_pic.y_stride }) << subsample_tx,
                    );
                    svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                        &src_diff,
                        size << subsample_tx,
                        &mut coeff,
                        tx_size,
                        tpl_pf_shape(tpl_ctrls),
                        8,
                        false,
                    );
                    i64::from(aom_satd(&coeff[..(size * size) >> subsample_tx]))
                        << subsample_tx
                };

                if inter_cost < best_inter_cost {
                    if tpl_ctrls.use_sad_in_src_search == 0 {
                        best_coeff.copy_from_slice(&coeff);
                    }
                    best_ref_poc = ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].picture_number;
                    best_rf_idx = rf_idx;
                    best_inter_cost = inter_cost;
                    final_best_mv = best_mv;
                }
            } // rf_idx

            if best_inter_cost < best_intra_cost {
                best_mode = NEWMV;
            }

            if best_mode == NEWMV {
                let mut eob = 0u16;
                if tpl_ctrls.use_sad_in_src_search != 0 {
                    // Re-derive the winning prediction (the SAD path did not
                    // keep its coeff block).
                    let list_index = if best_rf_idx < 4 { 0 } else { 1 };
                    let ref_pic_index =
                        if best_rf_idx >= 4 { best_rf_idx - 4 } else { best_rf_idx } as usize;
                    let ref_pic = ref_pics
                        [ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index];
                    let ref_origin_index = (ref_pic.origin as i32
                        + mb_origin_x as i32
                        + (final_best_mv.x as i32 >> 3)
                        + (mb_origin_y as i32 + (final_best_mv.y as i32 >> 3))
                            * ref_pic.y_stride as i32)
                        as usize;
                    let subpel_mv =
                        (final_best_mv.x & 0x7) != 0 || (final_best_mv.y & 0x7) != 0;
                    if subpel_mv {
                        let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                        let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                            false,
                            MAX_TPL_SIZE,
                            false,
                            8,
                        );
                        let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                            svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                            ref_pic.origin,
                            ref_pic.y_stride,
                            svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(
                                &mut compensated_blk,
                            ),
                            size,
                            &mut conv_buf,
                            mb_origin_y as i32,
                            mb_origin_x as i32,
                            svtav1_dsp::port_subpel_params::Mv { x: final_best_mv.x, y: final_best_mv.y },
                            &ctx.sf_identity,
                            &cp,
                            0, // interp_filters
                            None,
                            None,
                            geom,
                            bsize,
                            bsize,
                            &edges,
                            0,
                            0,
                            0,
                            8,
                            false,
                            false,
                        );
                    }
                    subtract_block(
                        size >> subsample_tx,
                        size,
                        &mut src_diff,
                        size << subsample_tx,
                        &input_pic.y[src_mb..],
                        src_stride << subsample_tx,
                        if subpel_mv { &compensated_blk } else { &ref_pic.y[ref_origin_index..] },
                        (if subpel_mv { size } else { ref_pic.y_stride }) << subsample_tx,
                    );
                    svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                        &src_diff,
                        size << subsample_tx,
                        &mut best_coeff,
                        tx_size,
                        tpl_pf_shape(tpl_ctrls),
                        8,
                        false,
                    );
                }

                get_quantize_error(
                    &ctx.quant,
                    &best_coeff,
                    &mut qcoeff,
                    &mut dqcoeff,
                    tx_size,
                    &mut eob,
                    &mut recon_error,
                    &mut sse,
                );
                let rate_cost: i64 = if tpl_ctrls.compute_rate != 0 {
                    i64::from(rate_estimator(&qcoeff, eob as usize, tx_size))
                } else {
                    0
                };
                tpl_stats.srcrf_rate =
                    (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
                tpl_stats.srcrf_dist =
                    (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
            }
            if ctx.tpl_lad_mg > 0 {
                // store src based stats
                tpl_src_stats_buffer[src_stats_idx] = TplSrcStats {
                    srcrf_dist: tpl_stats.srcrf_dist,
                    srcrf_rate: tpl_stats.srcrf_rate,
                    mv: final_best_mv,
                    best_rf_idx,
                    ref_frame_poc: best_ref_poc,
                    best_mode,
                    best_intra_mode,
                };
            }
        } else {
            // get src based stats from previously computed data
            let s = tpl_src_stats_buffer[src_stats_idx];
            tpl_stats.srcrf_dist = s.srcrf_dist;
            tpl_stats.srcrf_rate = s.srcrf_rate;
            final_best_mv = s.mv;
            best_rf_idx = s.best_rf_idx;
            best_ref_poc = s.ref_frame_poc;
            best_mode = s.best_mode;
            best_intra_mode = s.best_intra_mode;
        }

        // Recon path — predict from the TPL RECONSTRUCTIONS.
        if best_mode == NEWMV {
            // inter recon with rec_picture as reference pic
            let ref_poc = best_ref_poc;
            let list_index = if best_rf_idx < 4 { 0usize } else { 1 };
            let ref_pic_index = if best_rf_idx >= 4 {
                (best_rf_idx - 4) as usize
            } else {
                best_rf_idx as usize
            };

            let ref_pic: TplPic<'_> = if ctx.tpl_data.ref_in_slide_window[list_index]
                [ref_pic_index]
            {
                let mut ref_frame_idx = 0usize;
                while ref_frame_idx < MAX_TPL_LA_SW
                    && ctx.poc_map_idx[ref_frame_idx] != ref_poc
                {
                    ref_frame_idx += 1;
                }
                assert!(
                    ref_frame_idx != MAX_TPL_LA_SW,
                    "tpl: in-window ref poc {ref_poc} not in poc_map_idx"
                );
                rec_refs[ref_frame_idx]
            } else {
                ref_pics[ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index]
            };

            let ref_origin_index = (ref_pic.origin as i32
                + mb_origin_x as i32
                + (final_best_mv.x as i32 >> 3)
                + (mb_origin_y as i32 + (final_best_mv.y as i32 >> 3)) * ref_pic.y_stride as i32)
                as usize;
            // REDO COMPENSATION WITH REF PIC (INSTEAD OF REF BEING THE SRC PIC)
            let subpel_mv = (final_best_mv.x & 0x7) != 0 || (final_best_mv.y & 0x7) != 0;
            if subpel_mv {
                let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                    false,
                    MAX_TPL_SIZE,
                    false,
                    8,
                );
                let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                    svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                    ref_pic.origin,
                    ref_pic.y_stride,
                    svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(
                        &mut recon.y[dst_mb_offset..],
                    ),
                    dst_buffer_stride,
                    &mut conv_buf,
                    mb_origin_y as i32,
                    mb_origin_x as i32,
                    svtav1_dsp::port_subpel_params::Mv { x: final_best_mv.x, y: final_best_mv.y },
                    &ctx.sf_identity,
                    &cp,
                    0, // interp_filters
                    None,
                    None,
                    geom,
                    bsize,
                    bsize,
                    &edges,
                    0,
                    0,
                    0,
                    8,
                    false,
                    false,
                );
            } else {
                for i in 0..size {
                    let src_row = ref_origin_index + i * ref_pic.y_stride;
                    let dst_row = dst_mb_offset + i * dst_buffer_stride;
                    recon.y[dst_row..dst_row + size]
                        .copy_from_slice(&ref_pic.y[src_row..src_row + size]);
                }
            }
        } else {
            // intra recon — neighbours come from the TPL RECON buffer.
            let mut above0 = [0u8; TPL_NEIGH_SZ];
            let mut left0 = [0u8; TPL_NEIGH_SZ];
            let mut above = [0u8; TPL_NEIGH_SZ];
            let mut left = [0u8; TPL_NEIGH_SZ];

            if intra_dc_sad_path {
                let mb_inside = mb_origin_x as usize + size <= input_pic.width as usize
                    && mb_origin_y as usize + size <= input_pic.height as usize;
                if mb_origin_x > 0 && mb_origin_y > 0 && mb_inside {
                    get_neighbor_samples_dc(
                        recon.y,
                        dst_buffer_stride,
                        dst_mb_offset,
                        &mut above0,
                        &mut left0,
                        bsize,
                    );
                } else {
                    update_neighbor_samples_open_loop(
                        true,
                        true,
                        recon.y,
                        dst_buffer_stride,
                        input_pic.width,
                        input_pic.height,
                        mb_origin_x,
                        mb_origin_y,
                        size,
                        size,
                        &mut above0,
                        &mut left0,
                    );
                }
                let n = crate::intra_open_loop::Neighbours {
                    above: &above0[1..1 + size],
                    left: &left0[1..1 + size],
                    top_left: above0[0],
                    has_left: mb_origin_x > 0,
                    has_above: mb_origin_y > 0,
                };
                let _ = crate::intra_open_loop::intra_prediction_open_loop_mb(
                    svtav1_types::prediction::PredictionMode::DcPred,
                    0,
                    n,
                    size,
                    size,
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride,
                );
            } else {
                update_neighbor_samples_open_loop(
                    true,
                    true,
                    recon.y,
                    dst_buffer_stride,
                    input_pic.width,
                    input_pic.height,
                    mb_origin_x,
                    mb_origin_y,
                    size,
                    size,
                    &mut above0,
                    &mut left0,
                );
                let p_angle = if crate::intra_open_loop::is_directional_mode(
                    tpl_mode(best_intra_mode).expect("stored intra mode"),
                ) {
                    crate::intra_edge::MODE_TO_ANGLE_MAP[best_intra_mode as usize]
                } else {
                    0
                };
                tpl_predict_intra(
                    best_intra_mode,
                    p_angle,
                    ctx.max_input_luma_width,
                    ctx.max_input_luma_height,
                    mb_origin_x,
                    mb_origin_y,
                    &above0,
                    &left0,
                    &mut above,
                    &mut left,
                    size,
                    // predict straight into the recon buffer — C's dst_buffer
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride,
                );
            }
        }

        subtract_block(
            size >> subsample_tx,
            size,
            &mut src_diff,
            size << subsample_tx,
            &input_pic.y[src_mb..],
            src_stride << subsample_tx,
            &recon.y[dst_mb_offset..],
            dst_buffer_stride << subsample_tx,
        );
        svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
            &src_diff,
            size << subsample_tx,
            &mut coeff,
            tx_size,
            tpl_pf_shape(tpl_ctrls),
            8,
            false,
        );

        let mut eob = 0u16;
        get_quantize_error(
            &ctx.quant,
            &coeff,
            &mut qcoeff,
            &mut dqcoeff,
            tx_size,
            &mut eob,
            &mut recon_error,
            &mut sse,
        );
        let rate_cost: i64 = if tpl_ctrls.compute_rate != 0 {
            i64::from(rate_estimator(&qcoeff, eob as usize, tx_size))
        } else {
            0
        };

        if !disable_intra_pred || ctx.tpl_data.is_ref {
            if eob != 0 {
                let _ = svtav1_dsp::txfm_dispatch::inv_transform_recon8bit_in_place(
                    &dqcoeff,
                    size,
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride << subsample_tx,
                    tx_size,
                    TxType::DctDct,
                    u32::from(eob),
                    false,
                );
                // populate the subsampled rows with a copy of the neighbour
                if subsample_tx == 2 {
                    for i in (0..size).step_by(4) {
                        for k in 1..=3usize {
                            let src_row = dst_mb_offset + i * dst_buffer_stride;
                            let dst_row = dst_mb_offset + (i + k) * dst_buffer_stride;
                            let row: [u8; MAX_TPL_SIZE] = recon.y
                                [src_row..src_row + size]
                                .try_into()
                                .unwrap();
                            recon.y[dst_row..dst_row + size].copy_from_slice(&row[..size]);
                        }
                    }
                } else if subsample_tx == 1 {
                    for i in (0..size).step_by(2) {
                        let src_row = dst_mb_offset + i * dst_buffer_stride;
                        let dst_row = dst_mb_offset + (i + 1) * dst_buffer_stride;
                        let row: [u8; MAX_TPL_SIZE] = recon.y[src_row..src_row + size]
                            .try_into()
                            .unwrap();
                        recon.y[dst_row..dst_row + size].copy_from_slice(&row[..size]);
                    }
                }
            }
        }

        tpl_stats.recrf_dist = (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        tpl_stats.recrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        if best_mode != NEWMV {
            tpl_stats.srcrf_dist = (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
            tpl_stats.srcrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        }

        tpl_stats.recrf_dist = tpl_stats.srcrf_dist.max(tpl_stats.recrf_dist);
        tpl_stats.recrf_rate = tpl_stats.srcrf_rate.max(tpl_stats.recrf_rate);
        if ctx.tpl_data.tpl_slice_type != crate::port_picstruct::SliceType::I && best_rf_idx != -1
        {
            tpl_stats.mv = final_best_mv;
            tpl_stats.ref_frame_poc = best_ref_poc;
        }

        // Motion flow dependency dispenser.
        result_model_store(
            &mut tpl_stats,
            tpl_stats_grid,
            tpl_ctrls.synth_blk_size,
            ctx.aligned_width,
            mb_origin_x,
            mb_origin_y,
            size as u32,
        );
    }
}

// =============================================================================
// tpl_prep_info + tpl_regular_setup_me_refs (src_ops_process.c:117-184)
// =============================================================================

/// C `RDDIV_BITS` (rd_cost.h:34).
const RDDIV_BITS: u32 = 7;
/// C `AV1_PROB_COST_SHIFT`.
const AV1_PROB_COST_SHIFT: u32 = 9;

/// C `RDCOST(RM, R, D)` (rd_cost.h:36).
pub fn rdcost_tpl(rm: i64, r: i64, d: i64) -> i64 {
    ((r * rm + (1 << (AV1_PROB_COST_SHIFT - 1))) >> AV1_PROB_COST_SHIFT) + (d << RDDIV_BITS)
}

/// The fields one `pcs->tpl_group[i]` picture contributes to
/// [`tpl_prep_info`]/[`tpl_regular_setup_me_refs`].
#[derive(Debug, Clone)]
pub struct TplPrepPic {
    /// C `pcs_tpl->slice_type`.
    pub slice_type: crate::port_picstruct::SliceType,
    /// C `pcs_tpl->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs_tpl->is_ref`.
    pub is_ref: bool,
    /// C `pcs_tpl->decode_order`.
    pub decode_order: i32,
    /// C `pcs_tpl->picture_number` (POC).
    pub picture_number: u64,
    /// C `ref_list0_count_try` / `ref_list1_count_try`.
    pub ref_list_count_try: [u8; 2],
    /// C `ref_pic_poc_array[list][idx]`.
    pub ref_pic_poc: [[u64; REF_LIST_MAX_DEPTH]; 2],
    /// For each ref, the index of its `EbPaReferenceObject->input_padded_pic`
    /// in the caller's `ref_pics` table (C stores the pointer; the port
    /// carries the index).
    pub ref_pic_index: [[usize; REF_LIST_MAX_DEPTH]; 2],
    /// For each ref, `EbPaReferenceObject->picture_number`.
    pub ref_poc_ds: [[u64; REF_LIST_MAX_DEPTH]; 2],
}

/// C `tpl_regular_setup_me_refs`: for each non-I picture's two ref lists,
/// record the ref count, mark window membership, and fill `tpl_ref_ds` with
/// (poc, ref-picture index).
pub fn tpl_regular_setup_me_refs(
    group_pocs: &[u64],
    pic: &TplPrepPic,
    tpl_data: &mut TplData,
) {
    for list_index in 0..2usize {
        let ref_list_count = pic.ref_list_count_try[list_index];
        if list_index == 0 {
            tpl_data.tpl_ref0_count = ref_list_count;
        } else {
            tpl_data.tpl_ref1_count = ref_list_count;
        }

        for ref_idx in 0..ref_list_count as usize {
            let ref_poc = pic.ref_pic_poc[list_index][ref_idx];

            tpl_data.ref_tpl_group_idx[list_index][ref_idx] = -1;
            for (j, &poc) in group_pocs.iter().enumerate() {
                if ref_poc == poc {
                    tpl_data.ref_in_slide_window[list_index][ref_idx] = true;
                    tpl_data.ref_tpl_group_idx[list_index][ref_idx] = j as i32;
                    break;
                }
            }

            tpl_data.tpl_ref_ds[list_index][ref_idx] = TplRefDs {
                picture_number: pic.ref_poc_ds[list_index][ref_idx],
                pic_index: pic.ref_pic_index[list_index][ref_idx],
            };
        }
    }
}

/// C `tpl_prep_info`: zero each picture's ref tables, copy its
/// slice/layer/ref/decode-order fields, then set up ME refs for non-I
/// pictures. `group_pocs` is the window's `tpl_group[i]->picture_number`
/// list; `pics` is the same group in the same order, and the returned
/// `tpl_data` vector is 1:1 with it.
pub fn tpl_prep_info(group_pocs: &[u64], pics: &[TplPrepPic]) -> Vec<TplData> {
    assert_eq!(group_pocs.len(), pics.len());
    pics.iter()
        .map(|pic| {
            let mut tpl_data = TplData {
                tpl_slice_type: pic.slice_type,
                tpl_temporal_layer_index: pic.temporal_layer_index,
                is_ref: pic.is_ref,
                tpl_decode_order: pic.decode_order,
                ..Default::default()
            };
            if tpl_data.tpl_slice_type != crate::port_picstruct::SliceType::I {
                tpl_regular_setup_me_refs(group_pocs, pic, &mut tpl_data);
            }
            tpl_data
        })
        .collect()
}

// =============================================================================
// generate_lambda_scaling_factor (src_ops_process.c:184-232)
// =============================================================================

/// C `generate_lambda_scaling_factor` — per-16x16 (or 32x32) grid, combine
/// `recrf_dist` against the MC-dependency delta into `c = 1.2 + rk/r0`.
///
/// `tpl_stats` is the flat `pa_me_data->tpl_stats` grid at
/// `mi_cols_sr >> tpl_synth_size_offset` stride; `tpl_rdmult_scaling_factors`
/// is the `num_cols x num_rows` output.
#[allow(clippy::too_many_arguments)]
pub fn generate_lambda_scaling_factor(
    synth_blk_size: u8,
    mi_rows: i32,
    enhanced_unscaled_width: u32,
    base_rdmult: i64,
    mc_dep_cost_base: i64,
    r0: f64,
    tpl_stats: &[TplStats],
    tpl_rdmult_scaling_factors: &mut [f64],
) {
    let tpl_synth_size_offset: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << tpl_synth_size_offset;
    let mi_cols_sr = ((enhanced_unscaled_width as i32 + 15) / 16) << 2;
    // BLOCK_32X32 at synth 32 else BLOCK_16X16 — 8x8 shares the 16 grid.
    let (num_mi_w, num_mi_h) = if synth_blk_size == 32 { (8, 8) } else { (4, 4) };
    let num_cols = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    let num_rows = (mi_rows + num_mi_h - 1) / num_mi_h;
    let stride = mi_cols_sr >> tpl_synth_size_offset;
    let c = 1.2f64;

    for row in 0..num_rows {
        for col in 0..num_cols {
            let mut recrf_dist_sum: i64 = 0;
            let mut mc_dep_delta_sum: i64 = 0;
            let index = (row * num_cols + col) as usize;
            let mut mi_row = row * num_mi_h;
            while mi_row < (row + 1) * num_mi_h {
                let mut mi_col = col * num_mi_w;
                while mi_col < (col + 1) * num_mi_w {
                    if mi_row < mi_rows && mi_col < mi_cols_sr {
                        let index1 = ((mi_row >> tpl_synth_size_offset) * stride
                            + (mi_col >> tpl_synth_size_offset))
                            as usize;
                        let st = &tpl_stats[index1];
                        let mc_dep_delta =
                            rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
                        recrf_dist_sum += st.recrf_dist;
                        mc_dep_delta_sum += mc_dep_delta;
                    }
                    mi_col += step;
                }
                mi_row += step;
            }
            let mut scaling_factors = c;
            if mc_dep_cost_base != 0 && recrf_dist_sum > 0 {
                let rk = ((recrf_dist_sum << RDDIV_BITS) as f64)
                    / (((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum) as f64);
                scaling_factors += rk / r0;
            }
            tpl_rdmult_scaling_factors[index] = scaling_factors;
        }
    }
}

// =============================================================================
// tpl_mc_flow_dispenser (src_ops_process.c:1338-1411)
// =============================================================================

/// C's `quantizer_to_qindex[static_config.qp] + extended_crf_qindex_offset`,
/// clamped to MAXQ, then the optional `enable_tpl_qps` per-temporal-layer
/// delta — the TPL picture's `qIndex`.
pub fn tpl_qindex(
    qp: u8,
    extended_crf_qindex_offset: i32,
    enable_tpl_qps: bool,
    tpl_slice_type: crate::port_picstruct::SliceType,
    hierarchical_levels: u8,
    tpl_temporal_layer_index: u8,
) -> i32 {
    let mut qindex = i32::from(crate::rate_control::QUANTIZER_TO_QINDEX[qp.min(63) as usize])
        + extended_crf_qindex_offset;
    qindex = qindex.min(crate::port_rc_vbr_cbr_qpick::MAXQ);

    if enable_tpl_qps {
        #[rustfmt::skip]
        const DELTA_RATE_NEW: [[f64; 6]; 7] = [
            [1.0, 1.0, 1.0, 1.0, 1.0, 1.0], // 1L
            [0.6, 1.0, 1.0, 1.0, 1.0, 1.0], // 2L
            [0.6, 0.8, 1.0, 1.0, 1.0, 1.0], // 3L
            [0.6, 0.8, 0.9, 1.0, 1.0, 1.0], // 4L
            [0.35, 0.6, 0.8, 0.9, 1.0, 1.0], //5L
            [0.35, 0.6, 0.8, 0.9, 0.95, 1.0], //6L
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0], // C indexes [hier][tl]; row 6 never read
        ];
        let q_val = crate::rate_control::convert_qindex_to_q(qindex, 8);
        let delta_qindex = if tpl_slice_type == crate::port_picstruct::SliceType::I {
            crate::rate_control::compute_qdelta(q_val, q_val * 0.25, 8)
        } else {
            crate::rate_control::compute_qdelta(
                q_val,
                q_val * DELTA_RATE_NEW[hierarchical_levels as usize]
                    [tpl_temporal_layer_index as usize],
                8,
            )
        };
        qindex += delta_qindex;
    }
    qindex
}

/// C `tpl_mc_flow_dispenser`: derive this picture's TPL `qIndex`, iterate
/// all SBs (full 64x64 SBs run at `tpl_ctrls.dispenser_search_level`, edge
/// SBs at level 0), then pad the completed recon picture.
///
/// `ctx` is a fn(&sb_index) -> &TplSbCtx adapter the caller uses to bind the
/// per-SB `b64_geom` origin; `sb_geom` gives each SB's (origin, is_full_64)
/// — the port keeps the geometry outside the ctx so the same ctx serves
/// every SB.
#[allow(clippy::too_many_arguments)]
pub fn tpl_mc_flow_dispenser<'a>(
    qp: u8,
    extended_crf_qindex_offset: i32,
    sb_geom: &[(u32, u32, bool)], // (org_x, org_y, is_full_64x64)
    make_ctx: impl Fn(usize) -> TplSbCtx<'a>,
    recon: &mut TplPicMut<'_>,
    rec_refs: &[TplPic<'a>],
    ref_pics: &[TplPic<'a>],
    me_results: impl Fn(usize) -> TplMeResults<'a>,
    tpl_src_stats_buffer: &mut [TplSrcStats],
    tpl_stats_grid: &mut [TplStats],
) -> i32 {
    // The ctx for sb0 carries the frame-level fields; qIndex derivation is
    // per-PICTURE in C, so read them once here.
    let ctx0 = make_ctx(0);
    let qindex = tpl_qindex(
        qp,
        extended_crf_qindex_offset,
        ctx0.tpl_ctrls.enable_tpl_qps != 0,
        ctx0.tpl_data.tpl_slice_type,
        ctx0.hierarchical_levels,
        ctx0.tpl_data.tpl_temporal_layer_index,
    );
    let rc_update_type = match ctx0.update_type {
        crate::port_picstruct::FrameUpdateType::Kf => crate::port_rc_process::FrameUpdateType::KfUpdate,
        crate::port_picstruct::FrameUpdateType::Lf => crate::port_rc_process::FrameUpdateType::LfUpdate,
        crate::port_picstruct::FrameUpdateType::Gf => crate::port_rc_process::FrameUpdateType::GfUpdate,
        crate::port_picstruct::FrameUpdateType::Arf => crate::port_rc_process::FrameUpdateType::ArfUpdate,
        crate::port_picstruct::FrameUpdateType::Overlay => crate::port_rc_process::FrameUpdateType::OverlayUpdate,
        crate::port_picstruct::FrameUpdateType::IntnlOverlay => crate::port_rc_process::FrameUpdateType::IntnlOverlayUpdate,
        crate::port_picstruct::FrameUpdateType::IntnlArf => crate::port_rc_process::FrameUpdateType::IntnlArfUpdate,
    };
    let base_rdmult = crate::port_rc_process::compute_rd_mult_based_on_qindex(
        8, rc_update_type, qindex,
    ) / TPL_RDMULT_SCALING_FACTOR;

    for (sb_index, &(org_x, org_y, is_full)) in sb_geom.iter().enumerate() {
        let mut ctx = make_ctx(sb_index);
        ctx.sb_origin = (org_x, org_y);
        // C re-derives the quant table per SB from the same qIndex — the
        // caller pre-binds it in ctx.quant.
        let level = if is_full {
            ctx.tpl_ctrls.dispenser_search_level as usize
        } else {
            0
        };
        tpl_mc_flow_dispenser_sb_generic(
            &ctx,
            level,
            recon,
            rec_refs,
            ref_pics,
            &me_results(sb_index),
            tpl_src_stats_buffer,
            tpl_stats_grid,
        );
    }

    // padding current recon picture
    crate::port_preanalysis::generate_padding(
        recon.y,
        recon.origin,
        recon.y_stride,
        recon.width as usize,
        recon.height as usize,
        recon.border,
        recon.border,
    );
    base_rdmult
}


// =============================================================================
// tpl_mc_flow_synthesizer + generate_r0beta (src_ops_process.c:1409-1690)
// =============================================================================

/// Per-picture view the synthesizer mutates — C's
/// `pcs_array[i]->{picture_number, aligned_width, av1_cm, pa_me_data->tpl_stats}`.
pub struct TplSynthPic<'a> {
    /// C `pcs->picture_number` — matched against `TplStats::ref_frame_poc`.
    pub picture_number: u64,
    /// C `pcs->aligned_width` (superres-scaled width, like everywhere in TPL).
    pub aligned_width: u32,
    /// C `av1_cm->mi_rows` / `mi_cols` — bounds for the dependency walk.
    pub mi_rows: i32,
    pub mi_cols: i32,
    /// C `pa_me_data->tpl_stats` — flat grid at `(aligned_width+15)/16 << (2-shift)`
    /// stride.
    pub stats: &'a mut [TplStats],
}

/// C `round_floor` (src_ops_process.c:1441) — floor division that rounds
/// negatives DOWN (away from zero toward -inf), unlike `>>`.
fn round_floor(ref_pos: i32, bsize_pix: i32) -> i32 {
    if ref_pos < 0 {
        -(1 + (-ref_pos - 1) / bsize_pix)
    } else {
        ref_pos / bsize_pix
    }
}

/// C `get_overlap_area` (src_ops_process.c:1410) — overlap of the `block`-th
/// quadrant of the grid cell at (`grid_pos_*`) with the predicted region at
/// (`ref_pos_*`), `bw`x`bh` pixels each.
fn get_overlap_area(
    grid_pos_row: i32,
    grid_pos_col: i32,
    ref_pos_row: i32,
    ref_pos_col: i32,
    block: i32,
    bw: i32,
    bh: i32,
) -> i32 {
    let (width, height) = match block {
        0 => (grid_pos_col + bw - ref_pos_col, grid_pos_row + bh - ref_pos_row),
        1 => (ref_pos_col + bw - grid_pos_col, grid_pos_row + bh - ref_pos_row),
        2 => (grid_pos_col + bw - ref_pos_col, ref_pos_row + bh - grid_pos_row),
        _ => (ref_pos_col + bw - grid_pos_col, ref_pos_row + bh - grid_pos_row),
    };
    width * height
}

/// C `delta_rate_cost` (src_ops_process.c:1452) — dependency-rate cost in
/// `TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT` fixed point.
fn delta_rate_cost(delta_rate: i64, recrf_dist: i64, srcrf_dist: i64, pix_num: i64) -> i64 {
    let beta = srcrf_dist as f64 / recrf_dist as f64;
    if srcrf_dist <= 128 {
        return delta_rate;
    }
    let dr = (delta_rate >> (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)) as f64
        / pix_num as f64;
    let log_den = beta.ln() / 2.0f64.ln() + 2.0 * dr;
    let rate_cost = if log_den > 10.0f64.ln() / 2.0f64.ln() {
        let rc = ((1.0f64 / beta).ln() * pix_num as f64) / 2.0f64.ln() / 2.0;
        (rc as i64) << (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)
    } else {
        let num = 2.0f64.powf(log_den);
        let den = num * beta + (1.0 - beta) * beta;
        let rc = (pix_num as f64 * (num / den).ln()) / 2.0f64.ln() / 2.0;
        (rc as i64) << (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)
    };
    rate_cost
}

/// C `tpl_model_update_b` (src_ops_process.c:1484) — propagate one
/// synth-block's `(cur_dep + mc_dep)` into the 4 quadrants of the reference
/// picture's stats grid it overlaps.
fn tpl_model_update_b(
    ref_pic: &mut TplSynthPic<'_>,
    synth_blk_size: u8,
    compute_rate: bool,
    tpl_stats: &TplStats,
    mi_row: i32,
    mi_col: i32,
    bsize: BlockSize,
) {
    // C `get_fullmv_from_mv` — subpel to fullpel, arithmetic shift.
    let ref_pos_row = mi_row * 4 + i32::from(tpl_stats.mv.y >> 3);
    let ref_pos_col = mi_col * 4 + i32::from(tpl_stats.mv.x >> 3);

    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let (bw, bh) = (mi_width * 4, mi_height * 4);
    let pix_num = (bw * bh) as i64;
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let mi_cols_sr = ((ref_pic.aligned_width as i32 + 15) / 16) << 2;

    let grid_pos_row_base = round_floor(ref_pos_row, bh) * bh;
    let grid_pos_col_base = round_floor(ref_pos_col, bw) * bw;

    let cur_dep_dist = tpl_stats.recrf_dist - tpl_stats.srcrf_dist;
    // C divides by `recrf_dist` unconditionally — stats are floored at 1 by
    // `result_model_store`, so it is never zero on this path.
    let mc_dep_dist = tpl_stats.mc_dep_dist * (tpl_stats.recrf_dist - tpl_stats.srcrf_dist)
        / tpl_stats.recrf_dist;
    let delta_rate = tpl_stats.recrf_rate - tpl_stats.srcrf_rate;
    let mc_dep_rate = if compute_rate {
        delta_rate_cost(
            tpl_stats.mc_dep_rate,
            tpl_stats.recrf_dist,
            tpl_stats.srcrf_dist,
            pix_num,
        )
    } else {
        0
    };

    for block in 0..4 {
        let grid_pos_row = grid_pos_row_base + bh * (block >> 1);
        let grid_pos_col = grid_pos_col_base + bw * (block & 0x01);
        if grid_pos_row >= 0
            && grid_pos_row < ref_pic.mi_rows * 4
            && grid_pos_col >= 0
            && grid_pos_col < ref_pic.mi_cols * 4
        {
            let overlap_area = get_overlap_area(
                grid_pos_row,
                grid_pos_col,
                ref_pos_row,
                ref_pos_col,
                block,
                bw,
                bh,
            );
            let ref_mi_row = round_floor(grid_pos_row, bh) * mi_height;
            let ref_mi_col = round_floor(grid_pos_col, bw) * mi_width;
            let step = 1i32 << shift;
            let mut idy = 0;
            while idy < mi_height {
                let mut idx = 0;
                while idx < mi_width {
                    let cell = (((ref_mi_row + idy) >> shift) * (mi_cols_sr >> shift)
                        + ((ref_mi_col + idx) >> shift))
                        as usize;
                    let st = &mut ref_pic.stats[cell];
                    st.mc_dep_dist += ((cur_dep_dist + mc_dep_dist) * overlap_area as i64) / pix_num;
                    st.mc_dep_rate += ((delta_rate + mc_dep_rate) * overlap_area as i64) / pix_num;
                    idx += step;
                }
                idy += step;
            }
        }
    }
}

/// C `tpl_model_update` (src_ops_process.c:1543) — walk one outer block's
/// synth cells, find each cell's reference picture in the window by
/// `ref_frame_poc`, and propagate. `i` is deliberately NOT reset between
/// cells — C's forward-only scan.
fn tpl_model_update(
    pics: &mut [TplSynthPic<'_>],
    frame_idx: usize,
    mi_row: i32,
    mi_col: i32,
    bsize: BlockSize,
    frames_in_sw: usize,
    synth_blk_size: u8,
    compute_rate: bool,
) {
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let block_size = if synth_blk_size == 8 {
        BlockSize::Block8x8
    } else if synth_blk_size == 16 {
        BlockSize::Block16x16
    } else {
        BlockSize::Block32x32
    };
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << shift;
    let mi_cols_sr = ((pics[frame_idx].aligned_width as i32 + 15) / 16) << 2;

    // Copy the cells out first so the per-ref mutable borrows don't alias the
    // current picture's grid.
    let mut cells: alloc::vec::Vec<TplStats> = alloc::vec::Vec::new();
    let mut idy = 0;
    while idy < mi_height {
        let mut idx = 0;
        while idx < mi_width {
            let cell = ((((mi_row + idy) >> shift) * (mi_cols_sr >> shift))
                + ((mi_col + idx) >> shift)) as usize;
            cells.push(pics[frame_idx].stats[cell]);
            idx += step;
        }
        idy += step;
    }

    let mut i = 0usize;
    let mut cell_iter = cells.iter();
    idy = 0;
    while idy < mi_height {
        let mut idx = 0;
        while idx < mi_width {
            let tpl_stats = *cell_iter.next().unwrap();
            while i < frames_in_sw && pics[i].picture_number != tpl_stats.ref_frame_poc {
                i += 1;
            }
            if i < frames_in_sw {
                tpl_model_update_b(
                    &mut pics[i],
                    synth_blk_size,
                    compute_rate,
                    &tpl_stats,
                    mi_row + idy,
                    mi_col + idx,
                    block_size,
                );
            }
            idx += step;
        }
        idy += step;
    }
}

/// C `tpl_mc_flow_synthesizer` (src_ops_process.c:1575) — sweep the whole
/// frame at the OUTER block size (32x32 for synth 32, else 16x16).
pub fn tpl_mc_flow_synthesizer(
    pics: &mut [TplSynthPic<'_>],
    frame_idx: usize,
    frames_in_sw: usize,
    synth_blk_size: u8,
    compute_rate: bool,
) {
    let bsize = if synth_blk_size == 32 {
        BlockSize::Block32x32
    } else {
        BlockSize::Block16x16
    };
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let mi_rows = pics[frame_idx].mi_rows;
    let mi_cols = pics[frame_idx].mi_cols;
    let mut mi_row = 0;
    while mi_row < mi_rows {
        let mut mi_col = 0;
        while mi_col < mi_cols {
            tpl_model_update(
                pics,
                frame_idx,
                mi_row,
                mi_col,
                bsize,
                frames_in_sw,
                synth_blk_size,
                compute_rate,
            );
            mi_col += mi_width;
        }
        mi_row += mi_height;
    }
}

/// C `coded_to_superres_mi` (resize.h:71).
fn coded_to_superres_mi(mi_col: i32, denom: u8) -> i32 {
    (mi_col * denom as i32 + 4) / 8
}

/// Output of C `svt_aom_generate_r0beta` (src_ops_process.c:1592-1687).
pub struct R0Beta {
    /// C `pcs->r0` — valid only when `tpl_is_valid`.
    pub r0: f64,
    /// C `pcs->tpl_is_valid`.
    pub tpl_is_valid: bool,
}

/// C `svt_aom_generate_r0beta` — frame-level `r0`, then
/// `generate_lambda_scaling_factor`, then per-SB `tpl_beta`.
///
/// `tpl_stats` is the synth-grid for THIS picture; `sb_geom` is each SB's
/// `(org_x, org_y)` in pixels; `tpl_beta` is the per-SB output (indexed
/// `sb_y * picture_sb_width + sb_x`).
#[allow(clippy::too_many_arguments)]
pub fn generate_r0beta(
    synth_blk_size: u8,
    sb_size: u32,
    aligned_width: u32,
    aligned_height: u32,
    enhanced_unscaled_width: u32,
    enhanced_unscaled_height: u32,
    superres_denom: u8,
    cm_mi_rows: i32,
    base_rdmult: i64,
    tpl_stats: &[TplStats],
    sb_geom: &[(u32, u32)],
    tpl_rdmult_scaling_factors: &mut [f64],
    tpl_beta: &mut [f64],
) -> R0Beta {
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << shift;
    let col_step_sr = coded_to_superres_mi(step, superres_denom);
    // Super-res UPSCALED size.
    let mi_cols_sr = ((enhanced_unscaled_width as i32 + 15) / 16) << 2;
    let mi_rows = ((enhanced_unscaled_height as i32 + 15) / 16) << 2;

    let mut recrf_dist_base_sum: i64 = 0;
    let mut mc_dep_delta_base_sum: i64 = 0;
    let mut count: i64 = 0;
    let mut max_dist: i64 = 0;

    let mut row = 0;
    while row < cm_mi_rows {
        let mut col = 0;
        while col < mi_cols_sr {
            let st =
                &tpl_stats[((row >> shift) * (mi_cols_sr >> shift) + (col >> shift)) as usize];
            let mc_dep_delta = rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
            recrf_dist_base_sum += st.recrf_dist;
            mc_dep_delta_base_sum += mc_dep_delta;
            count += 1;
            if mc_dep_delta > max_dist {
                max_dist = mc_dep_delta;
            }
            col += col_step_sr;
        }
        row += step;
    }

    let mc_dep_cost_base = (recrf_dist_base_sum << RDDIV_BITS) + mc_dep_delta_base_sum;
    let (r0, tpl_is_valid) = if mc_dep_cost_base != 0 {
        let mut r0 =
            ((recrf_dist_base_sum << RDDIV_BITS) as f64) / (mc_dep_cost_base as f64);
        if max_dist > (mc_dep_delta_base_sum / count) * 100
            && max_dist > (mc_dep_delta_base_sum * 9 / 10)
        {
            r0 = 1.0;
        }
        (r0, true)
    } else {
        (0.0, false)
    };

    generate_lambda_scaling_factor(
        synth_blk_size,
        cm_mi_rows,
        enhanced_unscaled_width,
        base_rdmult,
        mc_dep_cost_base,
        r0,
        tpl_stats,
        tpl_rdmult_scaling_factors,
    );

    // Per-SB `tpl_beta` — C src_ops_process.c:1644-1684. Superres scale-down
    // uses the scaled width (`aligned_width`), as in C.
    let sb_mi_sz = (sb_size >> 2) as i32;
    let picture_sb_width = aligned_width.div_ceil(sb_size);
    let picture_sb_height = aligned_height.div_ceil(sb_size);
    let mi_high = sb_mi_sz;
    let mi_wide = sb_mi_sz;
    for sb_y in 0..picture_sb_height {
        for sb_x in 0..picture_sb_width {
            let (org_x, org_y) = sb_geom[(sb_y * picture_sb_width + sb_x) as usize];
            let mi_row = (org_y >> 2) as i32;
            let mi_col = (org_x >> 2) as i32;
            let mut recrf_dist_sum: i64 = 0;
            let mut mc_dep_delta_sum: i64 = 0;
            let mi_col_sr = coded_to_superres_mi(mi_col, superres_denom);
            let mi_col_end_sr = coded_to_superres_mi(mi_col + mi_wide, superres_denom);
            let row_step = step;

            let mut row = mi_row;
            while row < mi_row + mi_high {
                let mut col = mi_col_sr;
                while col < mi_col_end_sr {
                    if row < mi_rows && col < mi_cols_sr {
                        let index = ((row >> shift) * (mi_cols_sr >> shift) + (col >> shift))
                            as usize;
                        let st = &tpl_stats[index];
                        let mc_dep_delta =
                            rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
                        recrf_dist_sum += st.recrf_dist;
                        mc_dep_delta_sum += mc_dep_delta;
                    }
                    col += col_step_sr;
                }
                row += row_step;
            }
            let mut beta = 1.0f64;
            if recrf_dist_sum > 0 {
                let rk = ((recrf_dist_sum << RDDIV_BITS) as f64)
                    / (((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum) as f64);
                beta = r0 / rk;
            }
            tpl_beta[(sb_y * picture_sb_width + sb_x) as usize] = beta;
        }
    }

    R0Beta { r0, tpl_is_valid }
}


// =============================================================================
// tpl_mc_flow (src_ops_process.c:1793-1975)
// =============================================================================

/// Per-window-frame state the flow owns — the `tpl_group` member's
/// `pa_me_data` plus the `tpl_valid_pic`/`tpl_src_data_ready` flags.
///
/// The recon plane is deliberately NOT here: it lives in the parallel
/// `recons` array so the driver can hand the dispenser a mutable view of the
/// current frame's recon AND immutable views of the completed in-window
/// references (`enc_ctx->mc_flow_rec_picture_buffer`) at the same time.
pub struct TplWindowFrame<'a> {
    /// C `pcs->picture_number` — matched by the synthesizer's poc search and
    /// recorded into `poc_map_idx`.
    pub picture_number: u64,
    /// C `pcs->tpl_valid_pic[frame_idx]` — set by `store_extended_group`.
    pub valid: bool,
    /// C `pcs->tpl_data` — the per-frame ref mapping the dispenser reads.
    pub tpl_data: TplData,
    /// C `pa_me_data->tpl_stats` — the synth grid (zeroed each pass).
    pub stats: &'a mut [TplStats],
    /// C `pa_me_data->tpl_src_stats_buffer` — the 16-px src-stats grid.
    pub src_stats: &'a mut [TplSrcStats],
    /// C `pcs->aligned_width` (superres-scaled).
    pub aligned_width: u32,
    /// C `av1_cm->mi_rows` / `mi_cols`.
    pub mi_rows: i32,
    /// See [`TplWindowFrame::mi_rows`].
    pub mi_cols: i32,
    /// C `pcs->tpl_src_data_ready` — out flag set when `tpl_lad_mg > 0` and
    /// the frame was dispensed.
    pub tpl_src_data_ready: bool,
}

/// Arguments handed to the caller's per-frame dispenser closure — everything
/// C's `tpl_mc_flow` loop body set up before calling
/// `tpl_mc_flow_dispenser`.
pub struct TplDispenseArgs<'a, 'b> {
    /// `frame_idx`.
    pub frame_idx: usize,
    /// The frame being dispensed.
    pub frame: &'a mut TplWindowFrame<'b>,
    /// Its recon buffer (`mc_flow_rec_picture_buffer[frame_idx]`).
    pub recon: &'a mut TplPicMut<'b>,
    /// `mc_flow_rec_picture_buffer` for the WHOLE window, indexed by
    /// `frame_idx` exactly like `tpl_ref_ds[..].pic_index` — slot
    /// `frame_idx` itself is a placeholder that is never read (a frame can
    /// never reference itself).
    pub ref_pics: &'a [TplPic<'a>],
    /// `enc_ctx->poc_map_idx` — window index to POC, filled through
    /// `frame_idx` inclusive (C writes each entry just before dispensing).
    pub poc_map_idx: &'a [u64],
}

/// C `tpl_mc_flow` (src_ops_process.c:1793-1975), minus the resource-pool
/// plumbing that has no safe-Rust analogue:
///
/// * `svt_get_empty_object`/`tpl_ref_list`/`svt_release_object` manage C's
///   recon-buffer pool; the port's caller owns `recons` directly.
/// * `init_tpl_segments` + `assign_tpl_segments` are the multi-threaded SB
///   scheduler; the port's dispenser walks all SBs serially.
/// * `svt_wait_cond_var(&tpl_group[i]->me_ready)` is the PA-ME barrier — the
///   caller runs after ME results exist by construction.
/// * `EB_DELETE(non_tf_input)` / `release_pa_reference_objects` are lifetime
///   management.
///
/// What IS preserved, in C order: `frames_in_sw` clamp, per-frame stats-grid
/// zero, `poc_map_idx` fill, valid-gated dispenser calls, `tpl_src_data_ready`
/// marking, and the reverse-order synthesizer sweep.
///
/// The caller must only invoke this when C would:
/// `tpl_ctrls.enable && !frame_superres_enabled && temporal_layer_index == 0`
/// (`svt_aom_source_based_operations_kernel_iter`) and
/// `tpl_group[0].tpl_data.tpl_temporal_layer_index == 0` (the `tpl_mc_flow`
/// gate). Returns `frames_in_sw`.
pub fn tpl_mc_flow<'a>(
    window: &mut [TplWindowFrame<'a>],
    recons: &mut [TplPicMut<'a>],
    synth_blk_size: u8,
    compute_rate: bool,
    tpl_lad_mg: u8,
    mut dispense: impl FnMut(TplDispenseArgs<'_, '_>) -> i32,
) -> usize {
    let frames_in_sw = MAX_TPL_LA_SW.min(window.len());
    debug_assert_eq!(recons.len(), window.len());

    // Grid geometry — C derives it from `enhanced_pic` dims + synth size.
    // `picture_width_in_mb` is the tpl_stats row stride.
    let picture_width_in_mb = |w: u32| -> usize {
        let mb = w.div_ceil(16) as usize;
        match synth_blk_size {
            8 => mb << 1,
            32 => w.div_ceil(32) as usize,
            _ => mb,
        }
    };

    let mut poc_map_idx: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(frames_in_sw);

    // TPL main frame loop — forward so in-window ref recons are complete.
    for frame_idx in 0..frames_in_sw {
        poc_map_idx.push(window[frame_idx].picture_number);

        // Zero this frame's tpl_stats grid — C memsets `picture_height_in_mb
        // * picture_width_in_mb` cells, which is the whole grid.
        let row_len = picture_width_in_mb(window[frame_idx].aligned_width);
        debug_assert!(window[frame_idx].stats.len() >= row_len);
        for st in window[frame_idx].stats.iter_mut() {
            *st = TplStats::default();
        }

        let tpl_on = window[frame_idx].valid;
        if tpl_on {
            // Split `recons` so slot frame_idx is mutable while the rest are
            // shared views — the C code aliases these freely via
            // `mc_flow_rec_picture_buffer`.
            let n_recons = recons.len();
            let (left, right) = recons.split_at_mut(frame_idx);
            let (cur, right) = right.split_at_mut(1);
            let mut ref_pics: alloc::vec::Vec<TplPic<'_>> =
                alloc::vec::Vec::with_capacity(n_recons);
            for r in left.iter() {
                ref_pics.push(TplPic {
                    y: r.y,
                    y_stride: r.y_stride,
                    width: r.width,
                    height: r.height,
                    max_width: r.width,
                    max_height: r.height,
                    origin: r.origin,
                });
            }
            // Placeholder for the self slot — `pic_index == frame_idx` can
            // never be produced by `tpl_prep_info`.
            ref_pics.push(TplPic {
                y: &[],
                y_stride: 0,
                width: 0,
                height: 0,
                max_width: 0,
                max_height: 0,
                origin: 0,
            });
            for r in right.iter() {
                ref_pics.push(TplPic {
                    y: r.y,
                    y_stride: r.y_stride,
                    width: r.width,
                    height: r.height,
                    max_width: r.width,
                    max_height: r.height,
                    origin: r.origin,
                });
            }
            let (_, right) = window.split_at_mut(frame_idx);
            let frame = &mut right[0];
            dispense(TplDispenseArgs {
                frame_idx,
                frame,
                recon: &mut cur[0],
                ref_pics: &ref_pics,
                poc_map_idx: &poc_map_idx,
            });
        }

        if tpl_lad_mg > 0 && tpl_on {
            window[frame_idx].tpl_src_data_ready = true;
        }
    }

    // Synthesizer — REVERSE order over valid pics, propagating each frame's
    // dependency deltas onto its references' grids.
    {
        let valid: alloc::vec::Vec<bool> =
            window[..frames_in_sw].iter().map(|w| w.valid).collect();
        let mut synth: alloc::vec::Vec<TplSynthPic<'_>> = window[..frames_in_sw]
            .iter_mut()
            .map(|w| TplSynthPic {
                picture_number: w.picture_number,
                aligned_width: w.aligned_width,
                mi_rows: w.mi_rows,
                mi_cols: w.mi_cols,
                stats: &mut *w.stats,
            })
            .collect();
        for frame_idx in (0..frames_in_sw).rev() {
            if valid[frame_idx] {
                tpl_mc_flow_synthesizer(
                    &mut synth,
                    frame_idx,
                    frames_in_sw,
                    synth_blk_size,
                    compute_rate,
                );
            }
        }
    }

    frames_in_sw
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `result_model_store` — synth_blk_size 32 writes one cell at (x>>5,
    /// y>>5) on the (w+31)/32 grid.
    #[test]
    fn result_model_store_32() {
        let mut st = TplStats {
            srcrf_dist: 7,
            recrf_dist: 9,
            srcrf_rate: 3,
            recrf_rate: 5,
            ..Default::default()
        };
        let mut grid = vec![TplStats::default(); 4 * 4];
        // aligned_width 128 -> stride 4; block at (64,32) -> (y>>5)*4+(x>>5)
        // = 1*4+2 = 6
        result_model_store(&mut st, &mut grid, 32, 128, 64, 32, 32);
        assert_eq!(grid[6].srcrf_dist, 7);
        assert_eq!(grid[6].recrf_dist, 9);
        assert!(grid[5].srcrf_dist == 0 && grid[7].srcrf_dist == 0);
    }

    /// synth 16: a size-32 result is quartered and written to the four 16x16
    /// cells it covers; a size-16 result writes one cell.
    #[test]
    fn result_model_store_16() {
        let mut st = TplStats {
            srcrf_dist: 64,
            recrf_dist: 64,
            srcrf_rate: 32,
            recrf_rate: 16,
            ..Default::default()
        };
        let mut grid = vec![TplStats::default(); 8 * 8];
        // aligned_width 128 -> stride 8; block at (32,16) -> idx (1,2)=10
        result_model_store(&mut st, &mut grid, 16, 128, 32, 16, 32);
        for &idx in &[10usize, 11, 18, 19] {
            assert_eq!(grid[idx].srcrf_dist, 16, "idx {idx}");
            assert_eq!(grid[idx].recrf_rate, 4);
        }
        assert_eq!(grid[9].srcrf_dist, 0);

        let mut st2 = TplStats {
            srcrf_dist: 5,
            recrf_dist: 5,
            srcrf_rate: 5,
            recrf_rate: 5,
            ..Default::default()
        };
        let mut grid2 = vec![TplStats::default(); 8 * 8];
        result_model_store(&mut st2, &mut grid2, 16, 128, 32, 16, 16);
        assert_eq!(grid2[10].srcrf_dist, 5);
        assert_eq!(grid2[11].srcrf_dist, 0);
    }

    /// synth 8: a size-16 result is quartered over a 2x2 patch of the
    /// doubled-resolution grid.
    #[test]
    fn result_model_store_8() {
        let mut st = TplStats {
            srcrf_dist: 40,
            recrf_dist: 40,
            srcrf_rate: 8,
            recrf_rate: 4,
            ..Default::default()
        };
        // aligned_width 128 -> stride ((128+15)/16)<<1 = 16; block at (16,16)
        // -> idx (2,2) = 34
        let mut grid = vec![TplStats::default(); 16 * 8];
        result_model_store(&mut st, &mut grid, 8, 128, 16, 16, 16);
        for &idx in &[34usize, 35, 50, 51] {
            assert_eq!(grid[idx].srcrf_dist, 10, "idx {idx}");
        }
        assert_eq!(grid[33].srcrf_dist, 0);
    }

    /// `rate_estimator`: eob+1 plus per-coeff floor(log2(level+1)) + (level>0),
    /// shifted by AV1_PROB_COST_SHIFT=9.
    #[test]
    fn rate_estimator_counts() {
        // Tx16x16 scan — coeff[0] is the DC. Put levels at scan positions
        // 0..2 (raster 0,1,16 for the default scan).
        let mut q = [0i32; 256];
        let scan = crate::entropy::scan_tables::scan(TxSize::Tx16x16 as usize, 0);
        q[scan[0] as usize] = -3; // msb(4)=2, +1 -> 3
        q[scan[1] as usize] = 0; // msb(1)=0, +0 -> 0
        q[scan[2] as usize] = 1; // msb(2)=1, +1 -> 2
        let eob = 3;
        let rate = rate_estimator(&q, eob, TxSize::Tx16x16);
        assert_eq!(rate, (3 + 1 + 3 + 0 + 2) << 9);
    }

    /// `tpl_qindex` without enable_tpl_qps: quantizer_to_qindex[qp] + offset,
    /// clamped to MAXQ.
    #[test]
    fn tpl_qindex_plain() {
        let base = crate::rate_control::QUANTIZER_TO_QINDEX[30] as i32;
        assert_eq!(
            tpl_qindex(30, 0, false, crate::port_picstruct::SliceType::B, 4, 2),
            base
        );
        // clamps at 255
        assert!(tpl_qindex(63, 200, false, crate::port_picstruct::SliceType::B, 4, 2) <= 255);
    }

    /// `rdcost_tpl` = ((r*rm + half) >> 9) + (d << 7).
    #[test]
    fn rdcost_shape() {
        assert_eq!(rdcost_tpl(0, 0, 5), 5 << 7);
        assert_eq!(rdcost_tpl(64, 512, 0), (512 * 64 + 256) >> 9);
    }

    /// `round_floor` rounds -1..-bsize down to -1 (unlike `>>` on positives).
    #[test]
    fn round_floor_negatives() {
        assert_eq!(round_floor(0, 16), 0);
        assert_eq!(round_floor(15, 16), 0);
        assert_eq!(round_floor(16, 16), 1);
        assert_eq!(round_floor(-1, 16), -1);
        assert_eq!(round_floor(-16, 16), -1);
        assert_eq!(round_floor(-17, 16), -2);
    }

    /// `get_overlap_area`: a ref pos offset by (-4,-4) px inside a 16x16 cell
    /// spreads over the four quadrants 16/48/48/144.
    #[test]
    fn overlap_area_quadrants() {
        // ref_pos (12,12) inside grid cells of 16 px at base (0,0).
        let a0 = get_overlap_area(0, 0, 12, 12, 0, 16, 16);
        let a1 = get_overlap_area(0, 16, 12, 12, 1, 16, 16);
        let a2 = get_overlap_area(16, 0, 12, 12, 2, 16, 16);
        let a3 = get_overlap_area(16, 16, 12, 12, 3, 16, 16);
        assert_eq!((a0, a1, a2, a3), (16, 48, 48, 144));
        assert_eq!(a0 + a1 + a2 + a3, 256);
    }

    /// `tpl_model_update` — a cur-frame cell with mv (0,0) and poc pointing at
    /// pics[0] deposits (cur_dep_dist, delta_rate) into exactly one ref cell.
    #[test]
    fn model_update_propagates() {
        // 64x64, synth 16 -> mi_cols_sr = 16, grid stride 4.
        let mut ref_stats = vec![TplStats::default(); 16];
        let mut cur_stats = vec![TplStats::default(); 16];
        // cur cell (mi_row=4, mi_col=4) -> grid idx 5.
        cur_stats[5] = TplStats {
            ref_frame_poc: 0,
            mv: Mv { x: 0, y: 0 },
            recrf_dist: 1000,
            srcrf_dist: 500,
            recrf_rate: 100,
            srcrf_rate: 50,
            ..Default::default()
        };
        {
            let mut pics = [
                TplSynthPic {
                    picture_number: 0,
                    aligned_width: 64,
                    mi_rows: 16,
                    mi_cols: 16,
                    stats: &mut ref_stats,
                },
                TplSynthPic {
                    picture_number: 8,
                    aligned_width: 64,
                    mi_rows: 16,
                    mi_cols: 16,
                    stats: &mut cur_stats,
                },
            ];
            // Outer bsize for synth 16 is BLOCK_16X16; walk only block (4,4).
            tpl_model_update(
                &mut pics,
                1,
                4,
                4,
                BlockSize::Block16x16,
                2,
                16,
                false,
            );
            // mv (0,0): ref_pos (16,16) sits exactly on grid cell (16,16) ->
            // only quadrant 0 overlaps, area 256 = pix_num.
            let ref_cell = &pics[0].stats[5];
            assert_eq!(ref_cell.mc_dep_dist, 500);
            assert_eq!(ref_cell.mc_dep_rate, 50);
            // neighbours untouched
            assert_eq!(pics[0].stats[4].mc_dep_dist, 0);
            assert_eq!(pics[0].stats[6].mc_dep_dist, 0);
        }
    }

    /// `generate_r0beta`: zero-dep stats -> mc_dep_cost_base > 0 only if
    /// recrf_dist > 0; r0 = recrf/(recrf+dep).
    #[test]
    fn r0beta_no_dep() {
        // 64x64, synth 16, sb_size 64 -> 1 sb. tpl_stats all recrf=100, dep=0.
        let stats = vec![
            TplStats {
                recrf_dist: 100,
                srcrf_dist: 50,
                recrf_rate: 10,
                srcrf_rate: 5,
                ..Default::default()
            };
            16
        ];
        let mut scale = vec![0.0f64; 16];
        let mut beta = vec![0.0f64; 1];
        let out = generate_r0beta(
            16,
            64,
            64,
            64,
            64,
            64,
            8, // SCALE_NUMERATOR — no superres
            16,
            64,
            &stats,
            &[(0, 0)],
            &mut scale,
            &mut beta,
        );
        assert!(out.tpl_is_valid);
        // dep deltas are all 0 -> r0 = 1.0, beta = r0/rk = 1.0.
        assert_eq!(out.r0, 1.0);
        assert_eq!(beta[0], 1.0);
        // scaling factor = 1.2 + rk/r0 with rk = 1 -> 2.2 per cell.
        assert!((scale[0] - 2.2).abs() < 1e-9);
    }

    /// `TplXd` — C `init_xd_tpl`: edges in subpel units at (mi*4)*8.
    #[test]
    fn xd_edges() {
        let xd = TplXd::new(68, 120, BlockSize::Block16x16, 64, 32);
        // mi_row = 8, mi_col = 16; bw=bh=4 mi.
        assert_eq!(xd.to_top, -(8 * 4 * 8));
        assert_eq!(xd.to_bottom, (68 - 4 - 8) * 4 * 8);
        assert_eq!(xd.to_left, -(16 * 4 * 8));
        assert_eq!(xd.to_right, (120 - 4 - 16) * 4 * 8);
        assert_eq!(xd.mi_row, 8);
        assert_eq!(xd.mi_col, 16);
    }
}
