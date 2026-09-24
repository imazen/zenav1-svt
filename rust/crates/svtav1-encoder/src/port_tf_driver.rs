//! The serial RANDOM_ACCESS MCTF driver — `mctf_frame`'s filter dispatch
//! (`mctf_frame_st` + `svt_av1_init_temporal_filtering`,
//! `pd_process.c:4175-4245` / `temporal_filtering.c:3951-4116`) and the whole
//! of `produce_temporally_filtered_pic` (`temporal_filtering.c:2594-3548`)
//! behind it, transcribed as one sequential pass.
//!
//! # What C threads and this module serialises
//!
//! C runs `mctf_frame` per picture in DISPLAY order inside
//! `svt_aom_picture_decision_process`'s third pass; each picture's
//! `tf_segments_total_count` segments are farmed out to ME processes and the
//! per-segment `me_ctx` is independent. The segment split is scheduling only:
//! every `(blk_row, blk_col)` decision reads per-block state that is fully
//! re-initialised for each reference, and `tf_tot_{horz,vert}_blks` is a
//! commutative accumulation. The single-threaded kernel path
//! (`CONFIG_SINGLE_THREAD_KERNEL`, `mctf_frame_st`) is byte-identical to the
//! threaded one, so this module implements THAT control flow.
//!
//! # The buffer model
//!
//! C keeps two objects per picture: `enhanced_pic` (the padded input the
//! filter reads and eventually OVERWRITES) and `pa_ref_pic_wrapper`'s
//! `input_padded_pic` + quarter + sixteenth decimated pyramid (what ME and
//! the later encode search). Crucially, `input_padded_pic->y_buffer` ALIASES
//! `enhanced_pic->y_buffer` (`reference_object.c:302`) — one allocation. The
//! chroma lives only on `enhanced_pic`. [`TfPicBufs`] reproduces exactly
//! that: `pa.full` is both the PA pyramid's top level AND the enhanced luma,
//! `u`/`v` are the enhanced chroma.
//!
//! # Reachability
//!
//! 8-bit only today — the RA entry point (`try_encode_frame_420_ra`) stores
//! `Vec<u8>` planes, matching C's `!is_highbd` arm end to end. The
//! `use_zz_based_filter` arm of `apply_filtering_block_plane_wise` is dead
//! (`derive_tf_params` never selects an LD level); the `use_8bit_subpel` and
//! hbd accumulators are ported but unreachable here.

use alloc::boxed::Box;
use alloc::vec::Vec;

use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind};
use svtav1_dsp::port_enc_make_pred::{enc_make_inter_predictor, DstPlane, SrcPlanes};
use svtav1_dsp::port_inter_predictor::{broadcast_interp_filter, make_interp_filters};
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges as DspEdges, Mv, RefGeometry};
use svtav1_dsp::port_tf_pred::{simple_luma_unipred, TfDst, TfSrc};
use svtav1_dsp::variance::variance_diff;

use crate::inter_me::context::{
    MeB64Output, MeContext, MePicParams, MeRefs, MeSrcBufs, MeType, FULL_SAD_SEARCH,
};
use crate::inter_me::motion_estimation_b64;
use crate::inter_me::sad::{mvxt, mvyt};
use crate::inter_me::tables::TAB8X8;
use crate::inter_me_arm::{PaPicture, PaPlane, PA_BORDER};
use crate::port_enc_mode_config::leaf::get_enable_me_8x8;
use crate::port_enc_mode_config::me::{apply_me_tf_signals, sig_deriv_me_tf};
use crate::port_enc_mode_config::ResolutionRange;
use crate::port_picstruct::{
    tf_motion_direction, FrameUpdateType, PicParams, SliceType, TfMemberPool,
};
use crate::port_preanalysis as pre;
use crate::port_temporal_filtering as tf;
use crate::port_temporal_filtering::{
    apply_filtering_block_plane_wise_offsets, apply_filtering_central,
    apply_temporal_filter_planewise_medium, convert_64x64_info_to_32x32_info_mvs,
    derive_tf_32x32_block_split_flag, derive_tf_shift, get_final_filtered_pixels,
    init_tf_chroma, init_tf_mv_dist_th, pad_and_decimate_filtered_pic,
    subpel_idx_16x16, subpel_idx_32x32, subpel_idx_8x8, subpel_params_for_block,
    subpel_search,
    tf_32x32_inter_prediction_requests, tf_64x64_inter_prediction_request, tf_block_grid,
    tf_use_64x64_pred, TfKernelCtx, TfPredictionRequest, TfSearchCtrls, TfSplitCtx,
    IDX_32X32_TO_IDX_16X16, TF_BW, TF_BH,
};

/// `BLK_PELS` — one 64x64 TF block.
const BLK_PELS: usize = TF_BW * TF_BH;

/// `PU_64X64` / `PU_32X32_0` / `PU_16X16_0` / `PU_8X8_0` (`motion_estimation.h`)
/// — the square-PU slot bases inside `p_sb_best_{sad,mv}[0][0]`.
const PU_64X64: usize = 0;
const PU_32X32_0: usize = 1;
const PU_16X16_0: usize = 5;
const PU_8X8_0: usize = 21;

/// `tab16x16` (`motion_estimation.h`) — 16x16 raster order to ME z-order,
/// the twin of [`TAB8X8`]. `tf_16x16_sub_pel_search` reads
/// `p_best_mv16x16[tab16x16[pu_index]]`.
#[rustfmt::skip]
const TAB16X16: [usize; 16] = [
    0, 1, 4, 5,
    2, 3, 6, 7,
    8, 9, 12, 13,
    10, 11, 14, 15,
];

/// `INT_MAX` as C assigns it to the `uint64_t` block-error fields.
const BLOCK_ERROR_INT_MAX: u64 = i32::MAX as u64;

// ---------------------------------------------------------------------------
// Per-picture buffered planes
// ---------------------------------------------------------------------------

/// One buffered picture's TF-side storage — C's `enhanced_pic` (luma +
/// chroma, PA border) fused with `pa_ref_pic_wrapper`'s decimated pyramid,
/// plus the `saved_src_pic` an I slice's `filt_unfilt_dist` diffs against.
///
/// `pa.full` is BOTH `input_padded_pic->y_buffer` and `enhanced_pic`'s luma
/// (the two alias in C). `u`/`v` are `enhanced_pic`'s chroma, padded with
/// `PA_BORDER >> ss` border like C's `eb_picture_buffer_desc` (the stride is
/// `(y_stride + ss_x) >> ss_x`, which for ss=1 is the same
/// `width + 2*border` formula [`PaPlane`] already uses at half dims).
pub struct TfPicBufs {
    /// The PA reference pyramid (`input_padded_pic` + `quarter` +
    /// `sixteenth`), luma only — `pa.full` is the enhanced luma.
    pub pa: PaPicture,
    /// `enhanced_pic` chroma U at `PA_BORDER >> 1`.
    pub u: PaPlane,
    /// `enhanced_pic` chroma V.
    pub v: PaPlane,
    /// `pcs->saved_src_pic` — the unfiltered ALIGNED-dims luma copy
    /// `filt_unfilt_dist` diffs the filtered picture against. Only ever
    /// allocated for an I slice (or the psnr/ssim/superres-recode arm, which
    /// the RA path does not reach — see [`tf::choose_src_save`]).
    pub saved_y: Option<Vec<u8>>,
    /// `pcs->do_tf` — whether this picture's own filter ran. The encode side
    /// swaps in the filtered planes only when set.
    pub do_tf: bool,
}

impl TfPicBufs {
    /// `svt_aom_pad_input_pictures` + the PA object build for one buffered
    /// frame. `y`/`u`/`v` are the tightly packed TRUE-dims planes; the active
    /// regions are the ALIGNED dims (luma `aw x ah`, chroma `aw>>1 x ah>>1`,
    /// the `div_ceil` equivalent — `aw`/`ah` are multiples of 8, hence even).
    ///
    /// The true->aligned pad is C `pad_input_picture`'s replicate-last-
    /// column-then-last-row, done by the caller's `pad_plane_replicate` and
    /// passed in already-expanded, so the active region is byte-identical to
    /// C's `enhanced_pic` content.
    pub fn from_aligned_planes(
        y_aligned: &[u8],
        u_aligned: &[u8],
        v_aligned: &[u8],
        aligned_w: usize,
        aligned_h: usize,
        picture_number: u64,
    ) -> Self {
        let pa = PaPicture::from_source(y_aligned, aligned_w, aligned_w, aligned_h, picture_number);
        let (cw, ch) = (aligned_w >> 1, aligned_h >> 1);
        let cb = PA_BORDER >> 1;
        // The monochrome arm hands in empty chroma — `tf_chroma` is false
        // there, so empty planes are never read.
        let mut u = PaPlane::empty();
        if !u_aligned.is_empty() {
            u.refill_from_plane(u_aligned, cw, cw, ch, cb);
        }
        let mut v = PaPlane::empty();
        if !v_aligned.is_empty() {
            v.refill_from_plane(v_aligned, cw, cw, ch, cb);
        }
        Self {
            pa,
            u,
            v,
            saved_y: None,
            do_tf: false,
        }
    }

    /// The luma active region, tightly packed — what the encode path feeds
    /// `encode_frame_impl` when this picture is TF-filtered.
    pub fn extract_luma(&self) -> Vec<u8> {
        let p = &self.pa.full;
        let mut out = alloc::vec![0u8; p.width * p.height];
        for r in 0..p.height {
            out[r * p.width..r * p.width + p.width]
                .copy_from_slice(&p.buf[p.org + r * p.stride..p.org + r * p.stride + p.width]);
        }
        out
    }

    /// [`extract_luma`](Self::extract_luma) for a chroma plane.
    fn extract_chroma(p: &PaPlane) -> Vec<u8> {
        let mut out = alloc::vec![0u8; p.width * p.height];
        for r in 0..p.height {
            out[r * p.width..r * p.width + p.width]
                .copy_from_slice(&p.buf[p.org + r * p.stride..p.org + r * p.stride + p.width]);
        }
        out
    }

    pub fn extract_u(&self) -> Vec<u8> {
        Self::extract_chroma(&self.u)
    }
    pub fn extract_v(&self) -> Vec<u8> {
        Self::extract_chroma(&self.v)
    }
}

// ---------------------------------------------------------------------------
// The scs-level inputs the driver reads
// ---------------------------------------------------------------------------

/// The `SequenceControlSet` fields `svt_av1_init_temporal_filtering` /
/// `produce_temporally_filtered_pic` reach for, bundled so the pipeline call
/// is one argument.
#[derive(Debug, Clone, Copy)]
pub struct RaTfScs {
    /// `scs->static_config.encoder_bit_depth` — the RA path is 8-bit only.
    pub bit_depth: u8,
    /// `scs->static_config.qp` — the CLI qp (NOT the per-picture qp).
    pub qp: u32,
    /// `scs->static_config.enable_tf` — 1 selects the fixed-strength arm,
    /// >1 the adaptive `calculate_tf_shift_factor` arm.
    pub enable_tf: u8,
    /// `scs->static_config.tf_strength` (`hdr.tf_strength` on the port's
    /// config; mainline default 3).
    pub tf_strength: u8,
    /// `scs->static_config.kf_tf_strength` under `SVT_HDR_MODE` — `Some(s)`
    /// selects the fork arm of the shift-factor split
    /// (`derive_tf_shift`), `None` the mainline arm. `None` in mainline
    /// mode regardless of the configured value, matching the `#if`.
    pub kf_tf_strength: Option<u8>,
    /// `scs->tf_ref_qp_based_th_scaling` — off only at ENC_MR.
    pub tf_ref_qp_based_th_scaling: bool,
    /// `scs->vq_ctrls.sharpness_ctrls.tf` (`derive_vq_params`).
    pub vq_sharpness_tf: bool,
    /// `scs->calculate_variance` (`enc_handle.c:4362-4365`).
    pub calculate_variance: bool,
    /// `pcs->compute_psnr` — the RA path has no psnr accounting.
    pub compute_psnr: bool,
    /// `pcs->compute_ssim`.
    pub compute_ssim: bool,
    /// `pcs->enc_mode`.
    pub enc_mode: i8,
    /// `scs->super_block_size` (64 or 128).
    pub sb_size: i32,
    /// `scs->input_resolution`.
    pub input_resolution: ResolutionRange,
    /// `scs->picture_analysis_number_of_regions_per_width`.
    pub regions_per_width: usize,
    /// `scs->picture_analysis_number_of_regions_per_height`.
    pub regions_per_height: usize,
    /// `scs->tf_segment_row_count` — scheduling only, transcribed so the
    /// segment->block mapping reads like C's.
    pub tf_segment_row_count: u32,
    /// `scs->tf_segment_column_count`.
    pub tf_segment_column_count: u32,
    /// `scs->max_input_luma_width/height` — the TRUE dims.
    pub true_width: u32,
    pub true_height: u32,
    /// The pipeline's ALIGNED dims (`self.width`/`self.height`) — what
    /// `pcs->aligned_width/height` carry.
    pub aligned_width: u32,
    pub aligned_height: u32,
}

/// `scs->tf_segment_row_count` / `tf_segment_column_count`
/// (`enc_handle.c:240-246`). The `lp == PARALLEL_LEVEL_1` arm collapses both
/// to 1; the port runs the single-threaded-kernel control flow where the
/// segment COUNT is scheduling metadata only, so the multi-core default's
/// values are derived unconditionally. The segment ORDER cannot affect the
/// output (per-block state is fully re-initialised; `tf_tot_*` is a
/// commutative sum).
pub fn tf_segment_counts(aligned_width: u32, aligned_height: u32) -> (u32, u32) {
    let rows = if ((aligned_height + 32) / 64) < 6 { 1 } else { 8 };
    let cols = if ((aligned_width + 32) / 64) < 10 { 1 } else { 6 };
    (rows, cols)
}

// ---------------------------------------------------------------------------
// The per-(block, ref) search state — C's `me_ctx->tf_*` arrays
// ---------------------------------------------------------------------------

/// The `me_ctx->tf_*` fields the four sub-pel drivers, the split decision and
/// the prediction requests mutate. The kernels' view of the same data is
/// [`TfKernelCtx`]; this is the whole of it.
#[derive(Debug, Clone)]
struct TfSearchState {
    /// `me_ctx->tf_64x64_mv_x` / `_mv_y` / `_block_error`.
    mv_64x64_x: i16,
    mv_64x64_y: i16,
    err_64x64: u64,
    mv_32x32_x: [i16; 4],
    mv_32x32_y: [i16; 4],
    err_32x32: [u64; 4],
    split_32x32: [i32; 4],
    mv_16x16_x: [i16; 16],
    mv_16x16_y: [i16; 16],
    err_16x16: [u64; 16],
    split_16x16: [[i32; 4]; 4],
    mv_8x8_x: [i16; 64],
    mv_8x8_y: [i16; 64],
    err_8x8: [u64; 64],
}

impl Default for TfSearchState {
    fn default() -> Self {
        Self {
            mv_64x64_x: 0,
            mv_64x64_y: 0,
            err_64x64: BLOCK_ERROR_INT_MAX,
            mv_32x32_x: [0; 4],
            mv_32x32_y: [0; 4],
            err_32x32: [0; 4],
            split_32x32: [0; 4],
            mv_16x16_x: [0; 16],
            mv_16x16_y: [0; 16],
            err_16x16: [0; 16],
            split_16x16: [[0; 4]; 4],
            mv_8x8_x: [0; 64],
            mv_8x8_y: [0; 64],
            err_8x8: [0; 64],
        }
    }
}

impl TfSearchState {
    /// The [`TfKernelCtx`] view the medium kernel and the 32x32 prediction
    /// requests read.
    fn kernel_ctx(
        &self,
        tf_mv_dist_th: u32,
        tf_chroma: bool,
        decay: [u32; 3],
    ) -> TfKernelCtx {
        TfKernelCtx {
            tf_block_col: 0,
            tf_block_row: 0,
            tf_mv_dist_th,
            tf_chroma,
            tf_32x32_block_split_flag: [
                self.split_32x32[0] as u8,
                self.split_32x32[1] as u8,
                self.split_32x32[2] as u8,
                self.split_32x32[3] as u8,
            ],
            tf_16x16_mv_x: self.mv_16x16_x,
            tf_16x16_mv_y: self.mv_16x16_y,
            tf_16x16_block_error: self.err_16x16,
            tf_32x32_mv_x: self.mv_32x32_x,
            tf_32x32_mv_y: self.mv_32x32_y,
            tf_32x32_block_error: self.err_32x32,
            tf_decay_factor_fp16: decay,
        }
    }
}

// ---------------------------------------------------------------------------
// Small free helpers
// ---------------------------------------------------------------------------

/// `pcs->tf_enable_hme_flag` / `tf_enable_hme_level{0,1,2}_flag`
/// (`enc_mode_config.c:2004-2031`) — the HME enables the picture-decision
/// stamps from `tf_ctrls.hme_me_level`.
fn tf_enable_hme_flags(hme_me_level: u8) -> (u8, u8, u8, u8) {
    match hme_me_level {
        0 => (1, 1, 1, 1),
        1 | 2 => (1, 1, 1, 0),
        3 | 4 => (1, 1, 0, 0),
        _ => (1, 1, 0, 0),
    }
}

/// `svt_aom_get_frame_update_type` (`resize.c:1246`) — under RANDOM_ACCESS
/// (`hierarchical_levels > 0`) this is `set_frame_update_type`'s own
/// derivation, which `PicParams::update_type` already carries; the function
/// exists to pin that equivalence rather than trust it.
fn tf_frame_update_type(pic: &PicParams) -> FrameUpdateType {
    if pic.is_key_frame {
        FrameUpdateType::Kf
    } else if pic.hierarchical_levels > 0 {
        if pic.temporal_layer_index == 0 {
            FrameUpdateType::Arf
        } else if pic.temporal_layer_index == pic.hierarchical_levels {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    } else {
        FrameUpdateType::Lf
    }
}

/// `SEGMENT_CONVERT_IDX_TO_XY` + `SEGMENT_START_IDX`/`SEGMENT_END_IDX`
/// (`av1_common.h:25-30`): segment `seg` owns the RECTANGLE
/// `[x_lo..x_hi) x [y_lo..y_hi)` of the block grid — contiguous strips would
/// visit the same set but not in C's shape.
fn tf_segment_block_rect(
    seg_idx: usize,
    seg_cols: usize,
    seg_rows: usize,
    blk_cols: usize,
    blk_rows: usize,
) -> (usize, usize, usize, usize) {
    let y_seg = seg_idx / seg_cols;
    let x_seg = seg_idx - y_seg * seg_cols;
    (
        x_seg * blk_cols / seg_cols,
        (x_seg + 1) * blk_cols / seg_cols,
        y_seg * blk_rows / seg_rows,
        (y_seg + 1) * blk_rows / seg_rows,
    )
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// What [`ra_mctf_filter`] returns.
pub struct RaMctfOut {
    /// `pcs->tf_tot_horz_blks` — accumulated across all segments.
    pub tf_tot_horz_blks: u32,
    /// `pcs->tf_tot_vert_blks`.
    pub tf_tot_vert_blks: u32,
    /// `pcs->filt_to_unfilt_diff` when the centre is an I slice and the save
    /// ran; `None` otherwise (the inherited value stands).
    pub filt_to_unfilt_diff: Option<u32>,
    /// `pd_ctx->tf_motion_direction` — the `mctf_frame` verdict.
    pub motion_direction: i8,
}

/// `svt_av1_init_temporal_filtering` for one centre picture — the whole
/// `produce_temporally_filtered_pic` body, serialised over every segment.
///
/// * `pics` is the picture-decision output (read-only except the centre's
///   own `filt_to_unfilt_diff`, returned through [`RaMctfOut`]);
/// * `centre_slot` is the RA-buffer index of the centre picture;
/// * `stats` is the `ra_stats` parallel array (the `average_intensity_per_region`
///   and `pic_avg_variance` the bright-change skip and VQ decay read);
/// * `frames` is the `ra_filtered` parallel array — the centre's planes are
///   rewritten in place, the members' are only read;
/// * the centre's `tf_window` must already be stamped (MCTF-B).
#[allow(clippy::too_many_arguments)]
pub fn ra_mctf_filter(
    scs: &RaTfScs,
    centre_slot: usize,
    centre: &PicParams,
    stats: &[Option<Box<pre::PictureStatistics>>],
    frames: &mut [TfPicBufs],
) -> RaMctfOut {
    debug_assert_eq!(scs.bit_depth, 8, "RA MCTF is the 8-bit path");
    let ctrls = centre.tf_ctrls;
    let window = centre
        .tf_window
        .as_deref()
        .expect("ra_mctf_filter requires the assembled tf_window");
    let index_center = window.past_altref_nframes;

    // `pcs_list`/`list_input_picture_ptr` — the window members in slot order.
    // A `None` slot is a window C could not fill; the RA assembly only emits
    // full windows, so treat it as unreachable like C's own assert.
    let member_of = |slot: usize| -> usize {
        let m = window.members[slot]
            .as_ref()
            .expect("TF window slot unfilled — C dereferences the same NULL");
        debug_assert!(matches!(m.pool, TfMemberPool::Window));
        m.index
    };

    // --- `svt_av1_init_temporal_filtering` head ---------------------------
    let tf_chroma = init_tf_chroma(ctrls.chroma_lvl, &centre.noise_levels_log1p_fp16);
    let tf_mv_dist_th = init_tf_mv_dist_th(scs.aligned_width, scs.aligned_height);
    let (aw, ah) = (scs.aligned_width as usize, scs.aligned_height as usize);
    let (tw, th) = (scs.true_width as usize, scs.true_height as usize);
    let mi_cols = tw as i32 >> 2;
    let mi_rows = th as i32 >> 2;

    // Split `frames` around the centre so the centre is mutable and the
    // members stay readable.
    let (head, tail) = frames.split_at_mut(centre_slot);
    let (centre_buf, after) = tail
        .split_first_mut()
        .expect("centre_slot indexes frames");
    let member_bufs = |i: usize| -> &TfPicBufs {
        if i < centre_slot {
            &head[i]
        } else {
            &after[i - centre_slot - 1]
        }
    };

    centre_buf.do_tf = true;

    // `temp_filt_prep_done` prep — the original-source save an I slice needs
    // for `filt_unfilt_dist`.
    let update_type = tf_frame_update_type(centre);
    // The superres-recode arm needs `static_config.superres_mode ==
    // SUPERRES_AUTO` with a dual/all search — the RA envelope refuses
    // superres outright, so both gates are `false` today.
    let save = tf::choose_src_save(
        scs.compute_psnr,
        scs.compute_ssim,
        /*superres_mode_is_auto=*/ false,
        /*superres_search_is_dual_or_all=*/ false,
        matches!(update_type, FrameUpdateType::Kf | FrameUpdateType::Arf),
        centre.slice_type == SliceType::I,
    );
    if !matches!(save, tf::SrcSaveChoice::None) {
        // `save_y_src_pic_buffers`: the aligned-dims luma, border 0. The
        // all-planes arm adds chroma at (aw>>1)x(ah>>1); the RA path never
        // takes it (no psnr/ssim/superres), so `saved_y` is luma only — and
        // `filt_unfilt_dist` only diffs luma.
        centre_buf.saved_y = Some(centre_buf.extract_luma());
    }

    // `mctf_frame_st`: me_type, then the TF signal derivation.
    let mut me_ctx = MeContext::default();
    me_ctx.me_type = MeType::Mctf;
    let (hme_f, hme_l0, hme_l1, hme_l2) = tf_enable_hme_flags(ctrls.hme_me_level);
    let signals = sig_deriv_me_tf(
        ctrls.hme_me_level,
        scs.input_resolution,
        ctrls.qp_opt,
        scs.tf_ref_qp_based_th_scaling,
        scs.qp,
        hme_f,
        hme_l0,
        hme_l1,
        hme_l2,
    )
    .expect("tf_ctrls.hme_me_level outside the tf_set_me_hme_params_oq table");
    apply_me_tf_signals(&mut me_ctx, &signals);
    // `create_me_context_and_picture_control` hardwires FULL_SAD for the HME
    // method; `sig_deriv_me_tf`'s method survives only on `me_search_method`.
    me_ctx.hme_search_method = FULL_SAD_SEARCH;
    // C resets the per-segment accumulators inside each `init` call.
    me_ctx.tf_tot_horz_blks = 0;
    me_ctx.tf_tot_vert_blks = 0;

    // `decay_control` / the strength chain (`produce_temporally_filtered_pic`
    // head, :2667-2818).
    let centre_stats = stats[centre_slot].as_deref();
    let pic_avg_variance = centre_stats.map_or(0, |s| s.pic_avg_variance);
    let decay_control = tf::derive_decay_control(
        scs.vq_sharpness_tf,
        centre.is_noise_level,
        scs.calculate_variance,
        pic_avg_variance,
        centre.slice_type == SliceType::I,
        centre.filt_to_unfilt_diff as i32,
        &centre.noise_levels_log1p_fp16,
    );
    let active_worst_quality =
        i32::from(crate::rate_control::QUANTIZER_TO_QINDEX[scs.qp.min(63) as usize]);
    let offset_idx = tf::tf_qp_offset_idx(
        centre.is_ref,
        centre.is_key_frame,
        i32::from(centre.temporal_layer_index),
    );
    let q_val_fp8 =
        crate::var_boost::convert_qindex_to_q_fp8(active_worst_quality, scs.bit_depth);
    let qtarget = tf::tf_q_val_target_fp8(
        q_val_fp8,
        offset_idx,
        u32::from(centre.hierarchical_levels),
    );
    let delta_qindex_f =
        crate::var_boost::compute_qdelta_fp(q_val_fp8, qtarget, scs.bit_depth);
    let q = active_worst_quality + delta_qindex_f;
    let q_decay_fp8 = tf::tf_q_decay_fp8(q);
    const CONST_0DOT7_FP16: i32 = 45875;
    let mut n_decay_fp10 =
        (decay_control[0] * (CONST_0DOT7_FP16 + centre.noise_levels_log1p_fp16[0])) / (1 << 6);

    // The shift-factor decision and `tf_decay_factor_fp16[3]`.
    let mut decay = [1u32 << 16; 3];
    // `tf_64x64_block_error` feeds only the `enable_tf > 1` adaptive arm —
    // C reads the stale value the previous centre's last block left in the
    // persisted me_context. Unreachable while the port's `enable_tf` is a
    // bool; a fresh ctx's 0 is passed so the day the config widens the only
    // divergence is that stale carry.
    let shift = derive_tf_shift(
        scs.enable_tf,
        scs.tf_strength,
        scs.vq_sharpness_tf,
        update_type == FrameUpdateType::Kf,
        0,
        scs.kf_tf_strength,
    );
    if shift.disable_on_this_frame {
        decay = [0, 0, 0];
    } else {
        let factor = if update_type == FrameUpdateType::Kf {
            shift.kf_shift_factor
        } else {
            shift.shift_factor
        };
        tf::calculate_decay_factor(
            &mut decay,
            &mut n_decay_fp10,
            q_decay_fp8,
            decay_control[1],
            decay_control[2],
            CONST_0DOT7_FP16,
            &centre.noise_levels_log1p_fp16,
            factor,
            tf_chroma,
        );
    }

    // --- `produce_temporally_filtered_pic` --------------------------------
    let grid = tf_block_grid(scs.aligned_width, scs.aligned_height, 1, 1);
    let (blk_cols, blk_rows) = (grid.blk_cols as usize, grid.blk_rows as usize);
    let stride_pred = [TF_BW, grid.blk_width_ch as usize, grid.blk_width_ch as usize];
    let (ss_x, ss_y) = (1u32, 1u32);

    let mut pred: [Vec<u8>; 3] =
        [alloc::vec![0u8; BLK_PELS], alloc::vec![0u8; BLK_PELS], alloc::vec![0u8; BLK_PELS]];
    let mut accum: [Vec<u32>; 3] =
        [alloc::vec![0u32; BLK_PELS], alloc::vec![0u32; BLK_PELS], alloc::vec![0u32; BLK_PELS]];
    let mut count: [Vec<u16>; 3] =
        [alloc::vec![0u16; BLK_PELS], alloc::vec![0u16; BLK_PELS], alloc::vec![0u16; BLK_PELS]];
    let mut conv_buf = alloc::vec![0u16; 128 * 128];

    let centre_pic = tf_pic_params(scs, centre, update_type);
    let mut me_out = MeB64Output::default();
    let mut st = TfSearchState::default();

    let total_segments =
        (scs.tf_segment_row_count * scs.tf_segment_column_count).max(1) as usize;
    let geom = RefGeometry {
        super_block_size: scs.sb_size,
        frame_width: scs.aligned_width as i32,
        frame_height: scs.aligned_height as i32,
    };

    for seg_idx in 0..total_segments {
        let (x_lo, x_hi, y_lo, y_hi) = tf_segment_block_rect(
            seg_idx,
            scs.tf_segment_column_count as usize,
            scs.tf_segment_row_count as usize,
            blk_cols,
            blk_rows,
        );
        for blk_row in y_lo..y_hi {
        for blk_col in x_lo..x_hi {
            let sb_origin_x = (blk_col * TF_BW) as u32;
            let sb_origin_y = (blk_row * TF_BH) as u32;

            let stride = [
                centre_buf.pa.full.stride,
                centre_buf.u.stride,
                centre_buf.v.stride,
            ];
            let blk_y_src_offset = blk_col * TF_BW + blk_row * TF_BH * stride[0];
            let blk_ch_src_offset = blk_col * grid.blk_width_ch as usize
                + blk_row * grid.blk_height_ch as usize * stride[1];

            for a in accum.iter_mut() {
                a.fill(0);
            }
            for c in count.iter_mut() {
                c.fill(0);
            }

            // `apply_filtering_central` — the centre picture's own weight.
            {
                let (org_y, org_c) = (centre_buf.pa.full.org, centre_buf.u.org);
                apply_filtering_central(
                    tf_chroma,
                    &centre_buf.pa.full.buf[org_y + blk_y_src_offset..],
                    &centre_buf.u.buf[org_c + blk_ch_src_offset..],
                    &centre_buf.v.buf[org_c + blk_ch_src_offset..],
                    stride[0],
                    &mut accum,
                    &mut count,
                    TF_BW,
                    TF_BH,
                    ss_x,
                    ss_y,
                );
            }

            // The three frame segments: past (0..past-1), centre (skipped —
            // C still walks it and hits `frame_index == index_center`),
            // future (past+1..past+future).
            let start_frame_index = [0usize, index_center, index_center + 1];
            let end_frame_index = [
                index_center.saturating_sub(1),
                index_center,
                index_center + window.future_altref_nframes,
            ];
            for seg in 0..3 {
                let mut frame_index = start_frame_index[seg];
                while frame_index <= end_frame_index[seg] {
                    let next = frame_index + usize::from(ctrls.ref_frame_factor.max(1));
                    if frame_index != index_center {
                        let m = window.members[frame_index]
                            .as_ref()
                            .expect("TF window slot unfilled");
                        // The ahd-error and bright-region skips
                        // (`:2864-2914`).
                        let low_ahd_err = centre.aligned_width * centre.aligned_height;
                        let th: i64 = if centre.slice_type == SliceType::I { 20 } else { 40 };
                        let ahd = i64::from(m.ahd_error_to_central);
                        let avg = i64::from(centre.tf_avg_ahd_error);
                        if ahd > i64::from(low_ahd_err)
                            && (ahd - avg) * 100 > th * avg
                        {
                            frame_index = next;
                            continue;
                        }
                        let mut bright_change_region_cnt = 0usize;
                        let m_stats = stats[m.index].as_deref();
                        let (m_regions, c_regions) = (
                            m_stats.map(|s| &s.average_intensity_per_region),
                            centre_stats.map(|s| &s.average_intensity_per_region),
                        );
                        for w in 0..scs.regions_per_width {
                            for h in 0..scs.regions_per_height {
                                let (mv, cv) = (
                                    m_regions.map_or(0, |r| r[w][h]) as i64,
                                    c_regions.map_or(0, |r| r[w][h]) as i64,
                                );
                                if (mv - cv).abs() > 2 && m.avg_luma != centre.tf_avg_luma {
                                    bright_change_region_cnt += 1;
                                }
                            }
                        }
                        if bright_change_region_cnt
                            >= (14 * scs.regions_per_width * scs.regions_per_height) / 16
                        {
                            frame_index = next;
                            continue;
                        }

                        // `create_me_context_and_picture_control` + the
                        // per-ref me_ctx stamps (:2921-2947).
                        let member = member_bufs(member_of(frame_index));
                        let src_bufs = MeSrcBufs {
                            b64: &centre_buf.pa.full.buf[centre_buf.pa.full.org
                                + sb_origin_y as usize * stride[0]
                                + sb_origin_x as usize..],
                            b64_stride: centre_buf.pa.full.stride,
                            quarter: &centre_buf.pa.quarter.buf[centre_buf.pa.quarter.org
                                + (sb_origin_y as usize >> 1) * centre_buf.pa.quarter.stride
                                + (sb_origin_x as usize >> 1)..],
                            quarter_stride: centre_buf.pa.quarter.stride,
                            sixteenth: &centre_buf.pa.sixteenth.buf[centre_buf.pa.sixteenth.org
                                + (sb_origin_y as usize >> 2) * centre_buf.pa.sixteenth.stride
                                + (sb_origin_x as usize >> 2)..],
                            sixteenth_stride: centre_buf.pa.sixteenth.stride,
                        };
                        let mut refs = MeRefs::default();
                        refs.arr[0][0] = Some(member.pa.ds_ref());
                        me_ctx.num_of_list_to_search = 1;
                        me_ctx.num_of_ref_pic_to_search = [1, 0];
                        me_ctx.temporal_layer_index = centre.temporal_layer_index;
                        me_ctx.is_ref = centre.is_ref;
                        me_ctx.tf_me_exit_th = ctrls.me_exit_th;
                        me_ctx.tf_use_pred_64x64_only_th = ctrls.use_pred_64x64_only_th;
                        let tf_subpel_early_exit_th = u32::from(ctrls.subpel_early_exit_th);
                        // `set_hme_search_params_mctf(ctx, 0)`.
                        let def_tf = tf::SearchAreaMinMax {
                            sa_min: (
                                me_ctx.hme_l0_sa_default_tf.sa_min.width,
                                me_ctx.hme_l0_sa_default_tf.sa_min.height,
                            ),
                            sa_max: (
                                me_ctx.hme_l0_sa_default_tf.sa_max.width,
                                me_ctx.hme_l0_sa_default_tf.sa_max.height,
                            ),
                        };
                        let hme_l0 = tf::set_hme_search_params_mctf(def_tf, 0)
                            .expect("hme_search_level 0 is always valid");
                        me_ctx.hme_l0_sa.sa_min.width = hme_l0.sa_min.0;
                        me_ctx.hme_l0_sa.sa_min.height = hme_l0.sa_min.1;
                        me_ctx.hme_l0_sa.sa_max.width = hme_l0.sa_max.0;
                        me_ctx.hme_l0_sa.sa_max.height = hme_l0.sa_max.1;

                        motion_estimation_b64(
                            &centre_pic,
                            sb_origin_x,
                            sb_origin_y,
                            &mut me_ctx,
                            &src_bufs,
                            &refs,
                            &mut me_out,
                        );

                        // --- the 64x64 / 32x32 / 16x16 / 8x8 ladder ---------
                        let search_interp = if ctrls.use_2tap {
                            make_interp_filters(InterpFilterKind::Bilinear, InterpFilterKind::Bilinear)
                        } else {
                            make_interp_filters(
                                InterpFilterKind::EightTapRegular,
                                InterpFilterKind::EightTapRegular,
                            )
                        };
                        let src_y_block =
                            &centre_buf.pa.full.buf[centre_buf.pa.full.org + blk_y_src_offset..];

                        let use_64 = me_ctx.tf_use_pred_64x64_only_th != 0
                            && (me_ctx.tf_use_pred_64x64_only_th == u8::MAX
                                || tf_use_64x64_pred(
                                    me_ctx.p_sb_best_sad[0][0][PU_64X64],
                                    &[
                                        me_ctx.p_sb_best_sad[0][0][PU_32X32_0],
                                        me_ctx.p_sb_best_sad[0][0][PU_32X32_0 + 1],
                                        me_ctx.p_sb_best_sad[0][0][PU_32X32_0 + 2],
                                        me_ctx.p_sb_best_sad[0][0][PU_32X32_0 + 3],
                                    ],
                                    i64::from(me_ctx.tf_use_pred_64x64_only_th),
                                ) != 0);

                        // `tf_64x64_sub_pel_search` — always runs (both arms
                        // refine the 64x64 candidate).
                        st.err_64x64 = BLOCK_ERROR_INT_MAX;
                        st.mv_64x64_x = if me_ctx.tf_use_pred_64x64_only_th == u8::MAX {
                            (i32::from(me_ctx.search_results[0][0].hme_sc_x) * 8) as i16
                        } else {
                            (i32::from(mvxt(me_ctx.p_sb_best_mv[0][0][PU_64X64])) * 8) as i16
                        };
                        st.mv_64x64_y = if me_ctx.tf_use_pred_64x64_only_th == u8::MAX {
                            (i32::from(me_ctx.search_results[0][0].hme_sc_y) * 8) as i16
                        } else {
                            (i32::from(mvyt(me_ctx.p_sb_best_mv[0][0][PU_64X64])) * 8) as i16
                        };
                        run_subpel(
                            64,
                            0,
                            0,
                            sb_origin_x,
                            sb_origin_y,
                            &ctrls,
                            search_interp,
                            member,
                            src_y_block,
                            stride[0],
                            &mut pred[0],
                            stride_pred[0],
                            &geom,
                            mi_cols,
                            mi_rows,
                            tf_subpel_early_exit_th,
                            &mut st.err_64x64,
                            &mut st.mv_64x64_x,
                            &mut st.mv_64x64_y,
                        );

                        if use_64 {
                            tf_64x64_predict(
                                member,
                                &mut pred,
                                sb_origin_x,
                                sb_origin_y,
                                tf_chroma,
                                &geom,
                                mi_cols,
                                mi_rows,
                                st.mv_64x64_x,
                                st.mv_64x64_y,
                                &mut conv_buf,
                            );
                            convert_64x64_info(
                                &mut st,
                                &pred[0],
                                stride_pred[0],
                                src_y_block,
                                stride[0],
                                ctrls.sub_sampling_shift,
                            );
                        } else {
                            // 32x32 sub-pel, all four quadrants.
                            for idx_32x32 in 0..4usize {
                                let (idx_x, idx_y) = subpel_idx_32x32(idx_32x32 as u32);
                                st.err_32x32[idx_32x32] = BLOCK_ERROR_INT_MAX;
                                let mv = me_ctx.p_sb_best_mv[0][0][PU_32X32_0 + idx_32x32];
                                st.mv_32x32_x[idx_32x32] =
                                    (i32::from(mvxt(mv)) * 8) as i16;
                                st.mv_32x32_y[idx_32x32] =
                                    (i32::from(mvyt(mv)) * 8) as i16;
                                let (mut bx, mut by) =
                                    (st.mv_32x32_x[idx_32x32], st.mv_32x32_y[idx_32x32]);
                                let mut be = st.err_32x32[idx_32x32];
                                run_subpel(
                                    32,
                                    idx_x,
                                    idx_y,
                                    sb_origin_x,
                                    sb_origin_y,
                                    &ctrls,
                                    search_interp,
                                    member,
                                    src_y_block,
                                    stride[0],
                                    &mut pred[0],
                                    stride_pred[0],
                                    &geom,
                                    mi_cols,
                                    mi_rows,
                                    tf_subpel_early_exit_th,
                                    &mut be,
                                    &mut bx,
                                    &mut by,
                                );
                                st.err_32x32[idx_32x32] = be;
                                st.mv_32x32_x[idx_32x32] = bx;
                                st.mv_32x32_y[idx_32x32] = by;
                            }
                            let sum_32x32 = st.err_32x32[0]
                                + st.err_32x32[1]
                                + st.err_32x32[2]
                                + st.err_32x32[3];
                            if st.err_64x64 * 14 < sum_32x32 * 16
                                && st.err_64x64 < (1 << 18)
                            {
                                tf_64x64_predict(
                                    member,
                                    &mut pred,
                                    sb_origin_x,
                                    sb_origin_y,
                                    tf_chroma,
                                    &geom,
                                    mi_cols,
                                    mi_rows,
                                    st.mv_64x64_x,
                                    st.mv_64x64_y,
                                    &mut conv_buf,
                                );
                                convert_64x64_info(
                                    &mut st,
                                    &pred[0],
                                    stride_pred[0],
                                    src_y_block,
                                    stride[0],
                                    ctrls.sub_sampling_shift,
                                );
                            } else {
                                for idx_32x32 in 0..4usize {
                                    if st.err_32x32[idx_32x32] < ctrls.pred_error_32x32_th {
                                        st.split_32x32[idx_32x32] = 0;
                                        st.split_16x16[idx_32x32] = [0; 4];
                                    } else {
                                        tf_16x16_subpel(
                                            idx_32x32,
                                            sb_origin_x,
                                            sb_origin_y,
                                            &ctrls,
                                            member,
                                            src_y_block,
                                            stride[0],
                                            &mut pred[0],
                                            stride_pred[0],
                                            &geom,
                                            mi_cols,
                                            mi_rows,
                                            tf_subpel_early_exit_th,
                                            &me_ctx,
                                            &mut st,
                                        );
                                        if ctrls.enable_8x8_pred {
                                            tf_8x8_subpel(
                                                idx_32x32,
                                                sb_origin_x,
                                                sb_origin_y,
                                                &ctrls,
                                                member,
                                                src_y_block,
                                                stride[0],
                                                &mut pred[0],
                                                stride_pred[0],
                                                &geom,
                                                mi_cols,
                                                mi_rows,
                                                tf_subpel_early_exit_th,
                                                &me_ctx,
                                                &mut st,
                                            );
                                        }
                                        let mut split_ctx = TfSplitCtx {
                                            idx_32x32,
                                            enable_8x8_pred: ctrls.enable_8x8_pred,
                                            tf_32x32_block_error: st.err_32x32,
                                            tf_16x16_block_error: st.err_16x16,
                                            tf_8x8_block_error: st.err_8x8,
                                            tf_32x32_block_split_flag: st.split_32x32,
                                            tf_16x16_block_split_flag: st.split_16x16,
                                        };
                                        derive_tf_32x32_block_split_flag(&mut split_ctx);
                                        st.err_16x16 = split_ctx.tf_16x16_block_error;
                                        st.split_32x32 = split_ctx.tf_32x32_block_split_flag;
                                        st.split_16x16 = split_ctx.tf_16x16_block_split_flag;
                                    }
                                    tf_32x32_predict(
                                        member,
                                        &mut pred,
                                        sb_origin_x,
                                        sb_origin_y,
                                        tf_chroma,
                                        &geom,
                                        mi_cols,
                                        mi_rows,
                                        &st,
                                        idx_32x32,
                                        &mut conv_buf,
                                    );
                                }
                            }
                        }

                        // Accumulate this reference's contribution, one
                        // 32x32 quadrant at a time.
                        for block_row in 0..2usize {
                            for block_col in 0..2usize {
                                let mut kctx = st.kernel_ctx(
                                    tf_mv_dist_th,
                                    tf_chroma,
                                    decay,
                                );
                                kctx.tf_block_col = block_col as i32;
                                kctx.tf_block_row = block_row as i32;
                                let off = apply_filtering_block_plane_wise_offsets(
                                    block_row,
                                    block_col,
                                    &stride,
                                    &stride_pred,
                                    TF_BW >> 1,
                                    TF_BH >> 1,
                                    ss_x,
                                    ss_y,
                                );
                                let [ay, au, av] = &mut accum;
                                let [cy, cu, cv] = &mut count;
                                apply_temporal_filter_planewise_medium(
                                    &kctx,
                                    &centre_buf.pa.full.buf
                                        [centre_buf.pa.full.org + blk_y_src_offset + off.src[0]..],
                                    stride[0],
                                    &pred[0][off.block[0]..],
                                    stride_pred[0],
                                    &centre_buf.u.buf
                                        [centre_buf.u.org + blk_ch_src_offset + off.src[1]..],
                                    &centre_buf.v.buf
                                        [centre_buf.v.org + blk_ch_src_offset + off.src[2]..],
                                    stride[1],
                                    &pred[1][off.block[1]..],
                                    &pred[2][off.block[2]..],
                                    stride_pred[1],
                                    TF_BW >> 1,
                                    TF_BH >> 1,
                                    ss_x,
                                    ss_y,
                                    &mut ay[off.block[0]..],
                                    &mut cy[off.block[0]..],
                                    &mut au[off.block[1]..],
                                    &mut cu[off.block[1]..],
                                    &mut av[off.block[2]..],
                                    &mut cv[off.block[2]..],
                                );
                            }
                        }
                    }
                    frame_index = next;
                }
            }

            // `get_final_filtered_pixels` — normalise the accumulated sums
            // back into the centre's source planes.
            {
                let (org_y, org_c) = (centre_buf.pa.full.org, centre_buf.u.org);
                get_final_filtered_pixels(
                    tf_chroma,
                    [
                        &mut centre_buf.pa.full.buf[org_y..],
                        &mut centre_buf.u.buf[org_c..],
                        &mut centre_buf.v.buf[org_c..],
                    ],
                    &accum,
                    &count,
                    &stride,
                    blk_y_src_offset,
                    blk_ch_src_offset,
                    grid.blk_width_ch as usize,
                    grid.blk_height_ch as usize,
                );
            }
        }
        }
    }

    // --- the last-segment tail --------------------------------------------
    // `pad_and_decimate_filtered_pic` — re-pad + re-decimate the FILTERED
    // pyramid so later ME reads the filtered pixels.
    {
        // `svt_aom_pad_and_decimate_filtered_pic` decimates under
        // `pcs->tf_enable_hme_*`, not the encode-path `enable_hme_*`.
        let hme = pre::HmeEnables {
            enable_hme: false,
            tf_enable_hme: hme_f != 0,
            enable_hme_level0: false,
            tf_enable_hme_level0: hme_l0 != 0,
            enable_hme_level1: false,
            tf_enable_hme_level1: hme_l1 != 0,
        };
        let (pad_right, pad_bottom) = (aw - tw, ah - th);
        let chroma_lvl = ctrls.chroma_lvl != 0;
        let cb = &mut *centre_buf;
        let mut input = cb.pa.full.pre_view();
        let mut u = cb.u.pre_view();
        let mut v = cb.v.pre_view();
        let mut quarter = cb.pa.quarter.pre_view();
        let mut sixteenth = cb.pa.sixteenth.pre_view();
        pad_and_decimate_filtered_pic(
            1,
            1,
            pad_right,
            pad_bottom,
            /*color_format=*/ 1,
            chroma_lvl,
            &hme,
            &mut input,
            Some(&mut u),
            Some(&mut v),
            &mut quarter,
            &mut sixteenth,
        );
    }

    let mut filt_to_unfilt_diff = None;
    if centre.slice_type == SliceType::I {
        if let Some(saved) = &centre_buf.saved_y {
            let (org, stride) = (centre_buf.pa.full.org, centre_buf.pa.full.stride);
            let dist = tf::filt_unfilt_dist(
                scs.aligned_width,
                scs.aligned_height,
                scs.sb_size as u32,
                stride,
                scs.aligned_width as usize,
                |f_off, f_stride, u_off, u_stride, w, h| {
                    svtav1_dsp::pic_operators::spatial_full_distortion_kernel(
                        &centre_buf.pa.full.buf[org..],
                        f_off,
                        f_stride,
                        saved,
                        u_off,
                        u_stride,
                        w as usize,
                        h as usize,
                    )
                },
            );
            filt_to_unfilt_diff = Some(dist);
        }
    }

    let motion_direction =
        tf_motion_direction(me_ctx.tf_tot_horz_blks, me_ctx.tf_tot_vert_blks);
    RaMctfOut {
        tf_tot_horz_blks: me_ctx.tf_tot_horz_blks,
        tf_tot_vert_blks: me_ctx.tf_tot_vert_blks,
        filt_to_unfilt_diff,
        motion_direction,
    }
}

/// The `MePicParams` `svt_aom_motion_estimation_b64` reads for the MCTF
/// centre picture. Under `ME_MCTF` the candidate-construction arm is dead,
/// so the per-pu limits are placeholders; the geometry fields are live.
fn tf_pic_params(scs: &RaTfScs, centre: &PicParams, update_type: FrameUpdateType) -> MePicParams {
    let res = scs.input_resolution;
    MePicParams {
        picture_number: centre.picture_number,
        aligned_width: scs.aligned_width as i16,
        aligned_height: scs.aligned_height as i16,
        enhanced_width: scs.aligned_width,
        enhanced_height: scs.aligned_height,
        ahd_error: u32::MAX,
        input_resolution: res.as_u8(),
        enable_me_8x8: get_enable_me_8x8(scs.enc_mode, res, /*rtc=*/ false) != 0,
        enable_me_16x16: true,
        max_number_of_pus_per_sb: 85,
        hierarchical_levels: centre.hierarchical_levels,
        similar_brightness_refs: false,
        frame_is_boosted: centre.is_intra_only
            || matches!(update_type, FrameUpdateType::Arf | FrameUpdateType::Gf | FrameUpdateType::Kf),
        frame_is_leaf: update_type == FrameUpdateType::Lf,
        gm_enabled: false,
        only_l_bwd: false,
        // `pcs->pa_me_data->max_*` — sized >= what the MCTF arm writes.
        max_cand: 23,
        max_refs: 7,
        max_l0: 4,
        b64_geom_width: 64,
        b64_geom_height: 64,
        input_width: scs.true_width as u16,
        input_height: scs.true_height as u16,
    }
}

/// `tf_*_sub_pel_search` for one block — `subpel_params_for_block` +
/// `subpel_search` with the `svt_check_position` score closure wired to the
/// port's compensator and variance kernel.
#[allow(clippy::too_many_arguments)]
fn run_subpel(
    bsize: u32,
    idx_x: u32,
    idx_y: u32,
    sb_origin_x: u32,
    sb_origin_y: u32,
    ctrls: &crate::port_picstruct::TfCtrls,
    interp_filters: u32,
    member: &TfPicBufs,
    src_y_block: &[u8],
    src_stride: usize,
    pred_y: &mut [u8],
    pred_stride: usize,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    early_exit_th: u32,
    best_dist: &mut u64,
    best_mv_x: &mut i16,
    best_mv_y: &mut i16,
) {
    let mut p = subpel_params_for_block(
        bsize,
        idx_x,
        idx_y,
        sb_origin_x,
        sb_origin_y,
        &TfSearchCtrls {
            half_pel_mode: ctrls.half_pel_mode,
            quarter_pel_mode: ctrls.quarter_pel_mode,
            eight_pel_mode: ctrls.eight_pel_mode,
            use_2tap: ctrls.use_2tap,
            sub_sampling_shift: ctrls.sub_sampling_shift,
            enable_8x8_pred: ctrls.enable_8x8_pred,
        },
        interp_filters,
        8,
    );
    let mi_size = (bsize / 4) as i32;
    let e = tf::mb_edges(
        p.pu_origin_x,
        p.pu_origin_y,
        mi_size,
        mi_size,
        mi_cols,
        mi_rows,
    );
    let edges = DspEdges {
        to_left: e.left,
        to_right: e.right,
        to_top: e.top,
        to_bottom: e.bottom,
    };
    // The WHOLE allocation + `src_origin` = the border offset: the subpel
    // fetch can step up to `AOM_INTERP_EXTEND + bsize` pels before the
    // active origin at frame edges — that is exactly what the PA border is
    // for, and `buf[org..]` + origin 0 would panic on the negative index.
    let ref_y = &member.pa.full.buf[..];
    let ref_org = member.pa.full.org;
    let ref_stride = member.pa.full.stride;
    let ref_w = member.pa.full.width as i32;
    let ref_h = member.pa.full.height as i32;
    let ss = ctrls.sub_sampling_shift;
    let (lox, loy) = (p.local_origin_x as usize, p.local_origin_y as usize);
    let (pu_x, pu_y) = (i32::from(p.pu_origin_x), i32::from(p.pu_origin_y));
    let (bs, bs_h) = (bsize as usize, (bsize >> ss) as usize);
    let score = |mv_x: i16, mv_y: i16, compensate_shift: u8| -> u64 {
        let _ = simple_luma_unipred(
            TfSrc::Lbd(ref_y),
            ref_org,
            ref_stride,
            TfDst::Lbd(pred_y),
            pred_stride,
            lox,
            loy,
            Mv { x: mv_x, y: mv_y },
            interp_filters,
            *geom,
            ref_w,
            ref_h,
            pu_x,
            pu_y,
            bs,
            bs,
            &edges,
            8,
            u32::from(compensate_shift),
        );
        let pred_off = bs * idx_y as usize * pred_stride + bs * idx_x as usize;
        let src_off = bs * idx_y as usize * src_stride + bs * idx_x as usize;
        u64::from(variance_diff(
            &pred_y[pred_off..],
            pred_stride << ss,
            &src_y_block[src_off..],
            src_stride << ss,
            bs,
            bs_h,
        )) << ss
    };
    let mut score = score;
    subpel_search(
        &mut p,
        &TfSearchCtrls {
            half_pel_mode: ctrls.half_pel_mode,
            quarter_pel_mode: ctrls.quarter_pel_mode,
            eight_pel_mode: ctrls.eight_pel_mode,
            use_2tap: ctrls.use_2tap,
            sub_sampling_shift: ss,
            enable_8x8_pred: ctrls.enable_8x8_pred,
        },
        early_exit_th,
        best_dist,
        best_mv_x,
        best_mv_y,
        &mut score,
    );
}

/// `tf_16x16_sub_pel_search` (`temporal_filtering.c:1880`) — the four 16x16
/// blocks of one 32x32 quadrant.
#[allow(clippy::too_many_arguments)]
fn tf_16x16_subpel(
    idx_32x32: usize,
    sb_origin_x: u32,
    sb_origin_y: u32,
    ctrls: &crate::port_picstruct::TfCtrls,
    member: &TfPicBufs,
    src_y_block: &[u8],
    src_stride: usize,
    pred_y: &mut [u8],
    pred_stride: usize,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    early_exit_th: u32,
    me_ctx: &MeContext,
    st: &mut TfSearchState,
) {
    let interp = make_interp_filters(InterpFilterKind::EightTapRegular, InterpFilterKind::EightTapRegular);
    for idx_16x16 in 0..4usize {
        let pu_index = IDX_32X32_TO_IDX_16X16[idx_32x32][idx_16x16] as usize;
        // `subpel_idx_16x16` returns `(idx_x, idx_y)` (its header's
        // "(idx_y, idx_x)" phrasing notwithstanding — the body yields
        // `[..][1]`/`[..][0]` of the row-major `subblock_xy_16x16`).
        let (idx_x, idx_y) = subpel_idx_16x16(idx_32x32, idx_16x16);
        // C's `tf_16x16_*` arrays live in the `idx_32x32 * 4 + idx_16x16`
        // space (the split-flag driver and `tf_32x32_inter_prediction` read
        // them back at the same index); `pu_index` only feeds
        // `subblock_xy_16x16` and the `tab16x16` MV lookup.
        let k = idx_32x32 * 4 + idx_16x16;
        st.err_16x16[k] = BLOCK_ERROR_INT_MAX;
        let mv = me_ctx.p_sb_best_mv[0][0][PU_16X16_0 + TAB16X16[pu_index]];
        st.mv_16x16_x[k] = (i32::from(mvxt(mv)) * 8) as i16;
        st.mv_16x16_y[k] = (i32::from(mvyt(mv)) * 8) as i16;
        let (mut bx, mut by, mut be) = (st.mv_16x16_x[k], st.mv_16x16_y[k], st.err_16x16[k]);
        run_subpel(
            16,
            idx_x,
            idx_y,
            sb_origin_x,
            sb_origin_y,
            ctrls,
            interp,
            member,
            src_y_block,
            src_stride,
            pred_y,
            pred_stride,
            geom,
            mi_cols,
            mi_rows,
            early_exit_th,
            &mut be,
            &mut bx,
            &mut by,
        );
        st.err_16x16[k] = be;
        st.mv_16x16_x[k] = bx;
        st.mv_16x16_y[k] = by;
    }
}

/// `tf_8x8_sub_pel_search` (`temporal_filtering.c:1990`) — the sixteen 8x8
/// blocks of one 32x32 quadrant.
#[allow(clippy::too_many_arguments)]
fn tf_8x8_subpel(
    idx_32x32: usize,
    sb_origin_x: u32,
    sb_origin_y: u32,
    ctrls: &crate::port_picstruct::TfCtrls,
    member: &TfPicBufs,
    src_y_block: &[u8],
    src_stride: usize,
    pred_y: &mut [u8],
    pred_stride: usize,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    early_exit_th: u32,
    me_ctx: &MeContext,
    st: &mut TfSearchState,
) {
    let interp = make_interp_filters(InterpFilterKind::EightTapRegular, InterpFilterKind::EightTapRegular);
    for idx_16x16 in 0..4usize {
        for idx_8x8 in 0..4usize {
            let pu_index = tf::IDX_32X32_TO_IDX_8X8[idx_32x32][idx_16x16][idx_8x8] as usize;
            let (idx_x, idx_y) = subpel_idx_8x8(idx_32x32, idx_16x16, idx_8x8);
            // `idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8` space, like the
            // 16x16 level.
            let k = idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8;
            st.err_8x8[k] = BLOCK_ERROR_INT_MAX;
            let mv = me_ctx.p_sb_best_mv[0][0][PU_8X8_0 + TAB8X8[pu_index] as usize];
            st.mv_8x8_x[k] = (i32::from(mvxt(mv)) * 8) as i16;
            st.mv_8x8_y[k] = (i32::from(mvyt(mv)) * 8) as i16;
            let (mut bx, mut by, mut be) = (st.mv_8x8_x[k], st.mv_8x8_y[k], st.err_8x8[k]);
            run_subpel(
                8,
                idx_x,
                idx_y,
                sb_origin_x,
                sb_origin_y,
                ctrls,
                interp,
                member,
                src_y_block,
                src_stride,
                pred_y,
                pred_stride,
                geom,
                mi_cols,
                mi_rows,
                early_exit_th,
                &mut be,
                &mut bx,
                &mut by,
            );
            st.err_8x8[k] = be;
            st.mv_8x8_x[k] = bx;
            st.mv_8x8_y[k] = by;
        }
    }
}

/// `convert_64x64_info_to_32x32_info` (`temporal_filtering.c:2509`) —
/// [`convert_64x64_info_to_32x32_info_mvs`] plus the 32x32 distortion
/// re-measure the ported helper deliberately leaves to this driver.
fn convert_64x64_info(
    st: &mut TfSearchState,
    pred_y: &[u8],
    pred_stride: usize,
    src_y_block: &[u8],
    src_stride: usize,
    ss: u8,
) {
    convert_64x64_info_to_32x32_info_mvs(
        st.mv_64x64_x,
        st.mv_64x64_y,
        &mut st.mv_32x32_x,
        &mut st.mv_32x32_y,
        &mut st.split_32x32,
        &mut st.split_16x16,
    );
    for block_row in 0..2usize {
        for block_col in 0..2usize {
            let idx_32x32 = block_col + (block_row << 1);
            let pred_off = 32 * block_row * pred_stride + 32 * block_col;
            let src_off = 32 * block_row * src_stride + 32 * block_col;
            st.err_32x32[idx_32x32] = u64::from(variance_diff(
                &pred_y[pred_off..],
                pred_stride << ss,
                &src_y_block[src_off..],
                src_stride << ss,
                32,
                32 >> ss,
            )) << ss;
        }
    }
}

/// `tf_64x64_inter_prediction` (`temporal_filtering.c:2102`) — one 64x64
/// sharp-filter prediction into `pred`.
#[allow(clippy::too_many_arguments)]
fn tf_64x64_predict(
    member: &TfPicBufs,
    pred: &mut [Vec<u8>; 3],
    sb_origin_x: u32,
    sb_origin_y: u32,
    tf_chroma: bool,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    mv_x: i16,
    mv_y: i16,
    conv_buf: &mut [u16],
) {
    let req = tf_64x64_inter_prediction_request(sb_origin_x, sb_origin_y, mv_x, mv_y);
    tf_predict_one(member, pred, &req, tf_chroma, geom, mi_cols, mi_rows, conv_buf);
}

/// `tf_32x32_inter_prediction` (`temporal_filtering.c:2199`) — the request
/// list for one 32x32 quadrant, predicted in order.
#[allow(clippy::too_many_arguments)]
fn tf_32x32_predict(
    member: &TfPicBufs,
    pred: &mut [Vec<u8>; 3],
    sb_origin_x: u32,
    sb_origin_y: u32,
    tf_chroma: bool,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    st: &TfSearchState,
    idx_32x32: usize,
    conv_buf: &mut [u16],
) {
    let kctx = TfKernelCtx {
        tf_block_col: 0,
        tf_block_row: 0,
        tf_mv_dist_th: 0,
        tf_chroma,
        tf_32x32_block_split_flag: [
            st.split_32x32[0] as u8,
            st.split_32x32[1] as u8,
            st.split_32x32[2] as u8,
            st.split_32x32[3] as u8,
        ],
        tf_16x16_mv_x: st.mv_16x16_x,
        tf_16x16_mv_y: st.mv_16x16_y,
        tf_16x16_block_error: st.err_16x16,
        tf_32x32_mv_x: st.mv_32x32_x,
        tf_32x32_mv_y: st.mv_32x32_y,
        tf_32x32_block_error: st.err_32x32,
        tf_decay_factor_fp16: [0; 3],
    };
    let reqs = tf_32x32_inter_prediction_requests(
        &kctx,
        &st.split_16x16,
        &st.mv_8x8_x,
        &st.mv_8x8_y,
        idx_32x32,
        sb_origin_x,
        sb_origin_y,
    );
    for req in &reqs {
        tf_predict_one(member, pred, req, tf_chroma, geom, mi_cols, mi_rows, conv_buf);
    }
}

/// `svt_aom_inter_prediction`'s unipred arm (`enc_inter_prediction.c:3204`)
/// for one [`TfPredictionRequest`]: `LAST_FRAME`, `SIMPLE_TRANSLATION`,
/// `NEWMV`, `MULTITAP_SHARP` on both axes — NOT the search filters.
#[allow(clippy::too_many_arguments)]
fn tf_predict_one(
    member: &TfPicBufs,
    pred: &mut [Vec<u8>; 3],
    req: &TfPredictionRequest,
    tf_chroma: bool,
    geom: &RefGeometry,
    mi_cols: i32,
    mi_rows: i32,
    conv_buf: &mut [u16],
) {
    let filters = broadcast_interp_filter(InterpFilterKind::MultiTapSharp);
    let sf = ScaleFactors::setup_for_frame(
        member.pa.full.width as i32,
        member.pa.full.height as i32,
        member.pa.full.width as i32,
        member.pa.full.height as i32,
    );
    let e = tf::mb_edges(
        req.pu_origin_x,
        req.pu_origin_y,
        req.mi_size,
        req.mi_size,
        mi_cols,
        mi_rows,
    );
    let edges = DspEdges {
        to_left: e.left,
        to_right: e.right,
        to_top: e.top,
        to_bottom: e.bottom,
    };
    let mv = Mv {
        x: req.mv_x,
        y: req.mv_y,
    };
    let bs = req.bsize as usize;
    let (lox, loy) = (req.local_origin_x as usize, req.local_origin_y as usize);

    // Luma: `svt_aom_enc_make_inter_predictor` with ss_x = ss_y = 0 —
    // `get_conv_params_no_round(0, buf, scs->sb_size, 0, EB_EIGHT_BIT)`.
    let conv_y = ConvolveParams::no_round(false, geom.super_block_size as usize, false, 8);
    enc_make_inter_predictor(
        SrcPlanes::Lbd(&member.pa.full.buf),
        member.pa.full.org,
        member.pa.full.stride,
        DstPlane::Lbd(&mut pred[0][lox + loy * 64..]),
        64,
        conv_buf,
        i32::from(req.pu_origin_y),
        i32::from(req.pu_origin_x),
        mv,
        &sf,
        &conv_y,
        filters,
        None,
        None,
        *geom,
        bs,
        bs,
        &edges,
        0,
        0,
        0,
        8,
        false,
        false,
    )
    .expect("luma unipred");

    if tf_chroma {
        // Chroma: `ROUND_UV(origin)/2` — every TF block origin is a multiple
        // of 8, so ROUND_UV is the identity. Conv stride `sb_size >> ss_x`.
        let conv_c =
            ConvolveParams::no_round(false, (geom.super_block_size >> 1) as usize, false, 8);
        let dst_off = lox / 2 + (loy / 2) * 32;
        let pu_x = i32::from(req.pu_origin_x) / 2;
        let pu_y = i32::from(req.pu_origin_y) / 2;
        for (plane, src_plane) in [(1usize, &member.u), (2, &member.v)] {
            enc_make_inter_predictor(
                SrcPlanes::Lbd(&src_plane.buf),
                src_plane.org,
                src_plane.stride,
                DstPlane::Lbd(&mut pred[plane][dst_off..]),
                32,
                conv_buf,
                pu_y,
                pu_x,
                mv,
                &sf,
                &conv_c,
                filters,
                None,
                None,
                *geom,
                bs >> 1,
                bs >> 1,
                &edges,
                plane,
                1,
                1,
                8,
                false,
                false,
            )
            .expect("chroma unipred");
        }
    }
}
