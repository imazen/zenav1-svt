use super::*;

/// Decide the partition tree of one 64x64 superblock exactly like the C
/// PD0 pass at allintra effective-M9 (CLI preset >= 9).
///
/// `src` is the full luma plane (64-aligned frame, the caller's padding
/// convention), `qp` the CLI 0..63 qp, `qindex` the frame base_q_idx.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` for this SB — `Some` skips the
    // `qindex`/`lambda_weight` rederivation (which drops the delta-q stats
    // factor whenever the SB qindex differs from base). See
    // [`pd0_frame_lambda_and_min_sq`].
    sb_lambda: Option<u64>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let max_sq = max_block_size_allintra(vars.0[0], qp).min(max_tx_size as usize);
    let mode = if pd0_detector_allintra_demotes(&vars, qp) {
        Pd0Mode::Lvl5
    } else {
        Pd0Mode::Lvl6
    };
    let lambda = sb_lambda.unwrap_or_else(|| kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode,
        lvl1: None,
        max_sq,
        // disallow_4x4 = 1 (pic_disallow_4x4 for these presets),
        // disallow_8x8_allintra() = false, no depth removal flags.
        min_sq: 8,
        // C enc_mode_config.c:7326: LVL_5 subres is forced OFF (level 0) on an
        // INCOMPLETE b64 (`!b64_geom->is_complete_b64`, i.e. an SB whose 64x64
        // extent reaches past the ALIGNED frame). Seed is_subres_safe to the
        // "determined, not safe" sentinel (0) on such SBs so the 64x64
        // odd/even-deviation check never runs and every LVL_5 block keeps
        // step 0 — matching C, which computes the full-res transform there.
        // Complete SBs keep 255 (the 64x64 block determines subres exactly as
        // before — byte-neutral for every full-SB cell).
        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        // Allintra LVL_5's `subres_ctrls.step` — the level default (1); the
        // per-SB `sig_deriv_enc_dec_pd0` ladder is threaded on the VIDEO arm.
        subres_step: mode.default_subres_step(),
        ires_factor,
        // LVL_5/6 use their own closed-form coeff rates; unused here.
        coeff_rate_est_lvl: 0,
        ac_bias_eff,
        // eff-M9 (preset >= 9) => enc_mode > M6 => nsq_geom_level 0 =>
        // NSQ disabled: every one-false boundary node force-splits.
        // `use_accurate_part_ctx` = `enc_mode <= M8` is FALSE here
        // (enc_mode_config.c:9939) — LVL_5/6 double the SPLIT rate.
        accurate_part_ctx: false,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: take_scratch(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    return_scratch(core::mem::take(&mut ctx.scratch));
    eval.tree()
}

/// Decide the partition tree of one 64x64 superblock exactly like C's
/// **PD0_LVL_0** full-RD pass — the level `set_pd0_ctrls`
/// (enc_mode_config.c:5415) FORCES at bit-depth 10 (`hbd_md` set),
/// regardless of preset. PD0 runs at 8-bit, so `src` is the 8-bit
/// MSB-truncated luma plane (the same plane the bd8 pickers read); the
/// resulting tree is fed to `pipeline::bd10_reencode_luma`, which recomputes
/// the bd10 coded levels + recon over this fixed partition.
///
/// Differences from the eff-M9 (LVL_6/LVL_5) entry
/// [`pd0_pick_sb_partition`]:
/// - NO PD0-level detector: every block runs the full closed-form encode
///   (`lvl0_block_cost` = LVL_5 cost with subres OFF), never the LVL_6
///   variance heuristic — this is the whole point (the heuristic over-splits
///   where the full-RD keeps the parent);
/// - the 64x64 variance cap on the depth set still applies
///   (`get_max_block_size_allintra`, bit-depth-independent), so a busy SB
///   (`var64 > qp-scaled 7500`) force-splits the 64x64 to 32x32 exactly as at
///   bd8;
/// - split rate DOUBLED (`use_accurate_part_ctx = 0` above M8), like LVL_5.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_lvl0(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    // [SVT_HDR_MODE] Frame luma QM level (base_qindex-derived
    // `frm_hdr.quantization_params.qm[PLANE_Y]`); 15 = no matrices. C forces
    // PD0_LVL_0 at bd10 and its light encode applies QM when using_qmatrix
    // (fork default), so this is the ONLY PD0 entry that carries a live QM
    // level. Mainline / QM-off callers pass 15 (byte-inert non-QM path).
    qm_level: u8,
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` for this SB — `Some` skips the
    // `qindex`/`lambda_weight` rederivation (which drops the delta-q stats
    // factor whenever the SB qindex differs from base). See
    // [`pd0_frame_lambda_and_min_sq`].
    sb_lambda: Option<u64>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let max_sq = max_block_size_allintra(vars.0[0], qp).min(max_tx_size as usize);
    let lambda = sb_lambda.unwrap_or_else(|| kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        qm_level,
        lambda,
        mode: Pd0Mode::Lvl0,
        lvl1: None,
        max_sq,
        min_sq: 8,
        // subres OFF (LVL_0 is pd0_level <= PD0_LVL_2 -> subres_level 0). The
        // "determined, not safe" sentinel (0) makes lvl5_like_block_cost keep
        // step 0 for every block AND skip the 64x64 odd/even-deviation check.
        is_subres_safe: 0,
        subres_step: 0,
        ires_factor,
        // coeff_rate_est_lvl 0 (PD0 rate_est_level 0 above M8): closed-form
        // coeff rate. Unused by the LVL_0/LVL_5 closed forms directly (they
        // read `ires_factor`), kept 0 for consistency.
        coeff_rate_est_lvl: 0,
        ac_bias_eff,
        // enc_mode > M6 => nsq_geom_level 0 => NSQ disabled: one-false
        // boundary nodes force-split (inert on 64-aligned frames).
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: take_scratch(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    return_scratch(core::mem::take(&mut ctx.scratch));
    eval.tree()
}

/// Decide the partition tree of one 64x64 superblock exactly like the C
/// PD0 pass at allintra M2..M8 (`pic_pd0_lvl = 1` -> PD0_LVL_1,
/// depth-refinement level 10 -> PRED_PART_ONLY, so this tree IS the coded
/// tree). Differences from the eff-M9 entry above, all instrumented-C
/// verified (docs/IDENTITY-STATUS.md M6 chunk):
/// - no variance cap on the depth set (`base_var_th_cap = ~0` below M8):
///   the 64x64 depth is always evaluated;
/// - no PD0-level detector (`use_pd0_detector[PD0_LVL_1] = 0`): every SB
///   runs the LVL_1 block encode;
/// - LVL_1 block costs (real coeff rate, qindex+0, no subres);
/// - split rate NOT doubled (`use_accurate_part_ctx = 1`).
///
/// `tables` carries the frame-level default cost tables (C
/// `md_frame_context` for the first SB; the per-SB refresh from the
/// evolving frame context under `cdf_ctrl.enabled` is not yet ported).
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_m6(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    coeff_rate_est_lvl: u8,
    nsq_enabled: bool,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let lambda = kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64;
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode: Pd0Mode::Lvl1,
        lvl1: Some(tables),
        max_sq: 64usize.min(max_tx_size as usize),
        min_sq: 8,
        is_subres_safe: 255,
        // LVL_1 configures `subres_level = 0` (pd0_level <= PD0_LVL_2).
        subres_step: 0,
        ires_factor: 0,
        coeff_rate_est_lvl,
        ac_bias_eff,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: take_scratch(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    return_scratch(core::mem::take(&mut ctx.scratch));
    eval.tree()
}

/// The two `Pd0Ctx` fields an INTER frame resolves differently from a key
/// frame, in ONE place so the video-arm entry points cannot drift apart.
///
/// * **the MD lambda.** `full_sb_lambda_md[EB_8_BIT_MD]` is the FRAME's MD
///   lambda (`av1_lambda_assign_md`, md_process.c:763), and the KF chain only
///   on a key frame. MEASURED through C's own `SVT_PD0COST_OUT` on
///   `gradient 64x64 q20 p6 frames=2`: 1407 on frame 0 and **24898** on
///   frame 1, where the KF chain at that frame's qindex gives 25650.
/// * **`min_sq_size`.** `set_blocks_to_be_tested` (enc_dec_process.c:1485)
///   reads `depth_removal_ctrls`, which `set_depth_removal_level_controls`
///   returns `enabled = 0` for on an I-slice — so a key frame is always the
///   `disallow_4x4 ? 8 : 4` arm and only a non-key frame can raise it. Same
///   cell: C's `SVT_PD0CFG_OUT` reports `dr=1/0/1/1` on frame 1
///   (`disallow_below_32x32`), and C's PD0 evaluates exactly FIVE nodes there
///   — one 64x64 and four 32x32 — against 80 when `min_sq` is left at 8.
///
/// The third inter-only fact, PD0's ONE candidate being an inter `NEWMV`
/// rather than the DC intra prediction, is the `Pd0Ctx::inter` field itself
/// (`product_prediction_fun_table_pd0[is_inter_mode(mode)]`,
/// product_coding_loop.c:970).
pub(super) fn pd0_frame_lambda_and_min_sq(
    qindex: u8,
    lambda_weight: u32,
    key_min_sq: usize,
    inter: Option<&Pd0InterRef<'_>>,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` — the per-SB `av1_lambda_assign_md`
    // snapshot `svt_aom_full_cost_pd0` prices every PD0 block with
    // (product_coding_loop.c:5967-5999). The caller computes it for this SB
    // (delta-q stats factor, lambda_weight, scale factors — md_process.c:
    // 724-764); the qindex/lambda_weight rederivation below is the fallback
    // for callers without it (unit tests) and loses the per-SB qdiff factor
    // whenever `sb_qindex != base_q_idx`.
    sb_lambda: Option<u64>,
) -> (u64, usize) {
    let min_sq = inter.map_or(key_min_sq, |ir| ir.min_sq);
    if let Some(l) = sb_lambda {
        return (l, min_sq);
    }
    match inter {
        None => (
            kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64,
            key_min_sq,
        ),
        Some(ir) => (
            inter_full_lambda_8bit(
                qindex,
                ir.base_update_type,
                ir.factor_update_type,
                ir.alt_lambda_factors,
                ir.me_qdiff,
                ir.lambda_mod_intra,
                lambda_weight,
            ) as u64,
            ir.min_sq,
        ),
    }
}

/// [`pd0_pick_sb_partition_m6`] returning the full evaluation record —
/// the PD1 depth refinement's input (per-node tested/cost like C's
/// `pc_tree` after PD0).
///
/// `min_sq`: 8 when `disallow_4x4` (presets >= 4), else 4 — C
/// `set_blocks_to_be_tested` (enc_dec_process.c:1494: depth removal off
/// on the allintra still path, `ctx->disallow_4x4 ? 8 : 4`); the PD0B
/// capture rows confirm C's LPD0 evaluates 4x4 blocks at M2/M3.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pd0_pick_sb_partition_m6_eval(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    min_sq: usize,
    coeff_rate_est_lvl: u8,
    // Which PD0 block-encode path prices a block. The ALLINTRA arm's level at
    // every preset this entry point serves is PD0_LVL_1
    // (`set_pic_pd0_lvl_allintra`); the VIDEO arm's is PD0_LVL_3 at M3..M7
    // (`set_pic_pd0_lvl_default`), which is the same block cost plus subres
    // step 1 — see [`Pd0Mode::Lvl3`].
    mode: Pd0Mode,
    // C `depth_early_exit_ctrls.early_exit_th` for the i > 0 quadrants, as
    // `test_split_partition_pd0` reads it: 1000 when `pd0_level <= PD0_LVL_1
    // || ctx->pic_pred_depth_only`, else 900 (enc_mode_config.c:7232).
    depth_early_exit_th: u128,
    cap_max_block: bool,
    nsq_enabled: bool,
    aligned_w: usize,
    aligned_h: usize,
    // Tile pixel origin (0 = single tile → frame-edge predicate, byte-inert).
    // The DC leaf-cost prediction that drives the M6 PD0 partition must not
    // read across a tile boundary, matching C's tile-scoped up/left_available.
    tile_top: usize,
    tile_left: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309): the
    // VIDEO arm's PD0 predicts each block from the RECON it generates rather
    // than from the source. `Some((md_recon_plane, stride))` is the frame's
    // MD recon at this SB's origin — what C's neighbour arrays hold on entry,
    // since `copy_neighbour_arrays_pd0` snapshots the live arrays rather than
    // clearing them. `None` = the ALLINTRA arm, byte-identical to before.
    video_recon: Option<(&[u8], usize)>,
    // C `product_prediction_fun_table_pd0[1]` — PD0's INTER arm, `Some` on a
    // NON-KEY frame only. It carries this superblock's `min_sq` and the
    // frame's update types as well as the reference, because all three are
    // things only a non-key frame has; see [`pd0_frame_lambda_and_min_sq`].
    //
    // `None` on every key frame and every allintra cell, which is what makes
    // this parameter byte-neutral for the still envelope BY CONSTRUCTION
    // rather than by measurement — the caller's value is `inter_md.map(..)`,
    // and `inter_md` IS "this frame is a non-I slice with a DPB reference".
    inter: Option<&Pd0InterRef<'_>>,
    // C `ctx->subres_ctrls.step` as `svt_aom_sig_deriv_enc_dec_pd0` resolves
    // it per superblock (enc_mode_config.c:7327-7352) — the video-arm caller
    // threads it in. `None` applies the level default
    // (`Pd0Mode::default_subres_step`), which is what every allintra caller
    // and every pre-resolution video caller priced with.
    subres_step: Option<u32>,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` for this SB — `Some` skips the
    // `qindex`/`lambda_weight` rederivation (which drops the delta-q stats
    // factor whenever the SB qindex differs from base). See
    // [`pd0_frame_lambda_and_min_sq`].
    sb_lambda: Option<u64>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Eval {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let (lambda, min_sq) =
        pd0_frame_lambda_and_min_sq(qindex, lambda_weight, min_sq, inter, sb_lambda);
    // C `get_max_block_size_allintra` (enc_mode_config.c:7042): the
    // 64-variance cap fires ONLY at enc_mode >= M8 (base_var_th_cap is
    // (uint16_t)~0 = unlimited through M7, 7500 at M8+). A busy SB
    // (var64 > qp-scaled 7500) never tests the 64x64 PART_N — forced
    // split. Missing this made p8 keep 64x64 NONE where C split
    // (6763758 p8: port 64-NONE 120718451 beat C's 4x16x16 split total
    // 124435885 that C never compared against a 64-NONE at all).
    // Callers pass cap_max_block = (preset >= 8) && complete-SB (C keeps
    // the cap at sb_size for incomplete edge SBs).
    let max_sq = if cap_max_block {
        max_block_size_allintra(vars.0[0], qp)
    } else {
        64
    }
    .min(max_tx_size as usize);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode,
        lvl1: Some(tables),
        max_sq,
        min_sq,
        // C forces PD0 `subres_level = 0` on an INCOMPLETE b64
        // (`!b64_geom->is_complete_b64`, enc_mode_config.c:7337), so seed the
        // "determined, not safe" sentinel (0) there and let a complete SB run
        // the 64x64 odd/even-deviation check. Byte-inert on every level whose
        // `subres_step_cfg()` is 0, which is every ALLINTRA level this entry
        // point serves.
        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        subres_step: subres_step.unwrap_or_else(|| mode.default_subres_step()),
        ires_factor: 0,
        coeff_rate_est_lvl,
        ac_bias_eff,
        // Every preset this entry point serves is <= M8 on both arms, where
        // `use_accurate_part_ctx` is true (enc_mode_config.c:8955 / :9937).
        accurate_part_ctx: true,
        depth_early_exit_th,
        // `ctx->parent_cost_bias` is off 1000 only for inter PD0_LVL_6, which
        // the refinement arm never reaches — `video_pd0_params` routes those
        // SBs to `pd0_pick_sb_partition_video_eval`.
        parent_cost_bias: 1000,
        nsq_enabled,
        tile_top,
        tile_left,
        recon_canvas: video_recon.map(|(r, st)| Pd0ReconCanvas::new(r, st, sb_y)),
        inter,
        pending_recon: None,
        scratch: take_scratch(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    let (rd_cost, mut eval) = ctx.pick(64, 0, 0);
    return_scratch(core::mem::take(&mut ctx.scratch));
    // `lpd1_detector_post_pd0` reads the PART_N root block at every
    // `pd0_level < PD0_LVL_6` — including the 0..=2 levels this entry point
    // serves — so surface it the same way `pd0_pick_sb_partition_video_eval`
    // does.
    eval.root_det = Some(Pd0RootDet {
        rd_cost,
        blk: ctx.root_det.blk,
    });
    eval
}

/// The block-cost model a RESOLVED C `Pd0Level` selects, on the VIDEO arm.
///
/// **The argument is a `Pd0Level` (0..=6), NOT C's `pcs->pic_pd0_lvl`
/// (0..=8).** Those two numberings are not the same map — `set_pd0_ctrls`
/// (`enc_mode_config.c:5413`) sends `lpd0_lvl` 5 AND 6 to `PD0_LVL_5`, and 7
/// AND 8 to `PD0_LVL_6` — and the caller has already run BOTH `set_pd0_ctrls`
/// and `pd0_detector` (which C does at `enc_dec_process.c:2957`, before
/// `svt_aom_sig_deriv_enc_dec_pd0`). See
/// [`crate::part_arm::video_pd0_params`], which is the single place that
/// resolution happens.
///
/// CORRECTED 2026-09-02. This function used to take the raw `pic_pd0_lvl`,
/// map `5 | 6` to `Lvl5`, and carry a doc comment asserting that "the video
/// ladder never assigns a level whose `pd0_level` is `PD0_LVL_6` at the
/// presets this port encodes". The ladder DOES assign it — a KEY frame at
/// 480p and up takes `set_pic_pd0_lvl_default`'s `lpd0_lvl` 7 — and what
/// keeps C's `assert(IMPLIES(I_SLICE, pd0_level < PD0_LVL_6))` (`:2517`)
/// true is `pd0_detector`'s I_SLICE demote, not the ladder. With the demote
/// missing the port panicked on 4 of 64 video completion cells; the invariant
/// the comment asserted was exactly the code that was absent.
///
/// # Panics
/// On `PD0_LVL_0..PD0_LVL_2`, whose block cost this port carries only in the
/// bd10 [`pd0_pick_sb_partition_lvl0`] entry point. `PD0_LVL_6`
/// (VERY_LIGHT_PD0) is ported — `Pd0Ctx::lvl6_block_cost_inter`, the
/// `compute_lpd0_cost_inter` variance closed form — and IS reachable from
/// [`crate::part_arm::video_pd0_params`]: the detector keeps 6 on a non-I
/// slice wherever the referenced superblock was not all-intra, which on a
/// multi-frame encode means an inter-coded reference (`vidyo1`/`vidyo3`
/// q20 p9..13, `video_selfcheck_gate.sh`).
#[must_use]
pub(super) fn video_pd0_mode(pd0_level: u8) -> Pd0Mode {
    match pd0_level {
        // `set_pd0_ctrls`'s `hbd_md` force (enc_mode_config.c:5415) — the
        // resolved level on every bd10 frame. NOT `Pd0Mode::Lvl0`: that
        // model is the ALLINTRA arm's LVL_0, whose `rate_est_level` is 0
        // (`lpd0_qp_offset = 8` + `coeff_rate_est_lvl = 0` closed form). On
        // the VIDEO arm `pcs->rate_est_level` is 1, so
        // `svt_aom_sig_deriv_enc_dec_pd0` (`pd0_level <= PD0_LVL_3`,
        // enc_mode_config.c:7357) resolves `rate_est_level = 2` ->
        // `lpd0_qp_offset = 0` + `coeff_rate_est_lvl = 1` — the LVL_1 block
        // cost exactly (`md_encode_block_pd0` + real coeff rate). The
        // remaining differences are level-keyed signals that also land the
        // same way: `intra_level` MAX_INTRA_LEVEL-1 -> DC-only pred, subres
        // off at `pd0_level <= PD0_LVL_2`, `depth_early_exit_lvl` 1.
        0 => Pd0Mode::Lvl1,
        3 => Pd0Mode::Lvl3,
        4 => Pd0Mode::Lvl4,
        5 => Pd0Mode::Lvl5,
        // VERY_LIGHT_PD0's INTER arm — `compute_lpd0_cost_inter`'s
        // ME-candidate variance closed form
        // (product_coding_loop.c:8267), ported as
        // `Pd0Ctx::lvl6_block_cost_inter`. `pd0_detector` demotes 6 on an
        // I-slice, so this is reachable only where the reference block was
        // all-inter.
        6 => Pd0Mode::Lvl6,
        other => panic!(
            "PD0_LVL_{other} has no block cost in this port's video arm \
             (1..=2 live only in the video arm's low presets, which do not \
             reach this dispatch)"
        ),
    }
}

/// Decide the partition tree of one 64x64 superblock on the VIDEO arm.
///
/// The allintra twin is [`pd0_pick_sb_partition`] (preset >= 9, whose level
/// comes from `pd0_detector_allintra` + `get_max_block_size_allintra`) and
/// [`pd0_pick_sb_partition_m6_eval`] (below it). Three things differ, all of
/// them arm facts rather than preset facts:
///
/// * **the LEVEL** comes from `set_pic_pd0_lvl_default` rather than the
///   allintra detector — at 240p and `seq_qp_mod = 2` that is a flat 3 for
///   M3..M7, `3 + ldp0_lvl_offset[qp_band]` at M8 and
///   `4 + ldp0_lvl_offset[qp_band]` from M9 up, so a video key frame runs
///   PD0_LVL_3 / _4 / _5 where the still path runs LVL_1 / LVL_5 / LVL_6.
///   (CORRECTED 2026-09-01: this said `4 + offset` "for M8 up", i.e. 5 at
///   M8/qp40. C's own `SVT_PD0CFG_OUT` dump on `gradient 72x88 q40 p8` reports
///   `lvl=4`, and `:8631` is `MIN(MAX_PD0_LVL, 3 + qp_offset)` for the whole
///   `enc_mode <= ENC_M8` arm. The IMPLEMENTATION was right — it is
///   tier-1 gated — only this comment was wrong.);
/// * **`ctx->max_block_size` is uncapped** — `get_max_block_size_default`
///   returns `scs->super_block_size` outright, with no 64x64-variance cap;
/// * **NSQ geometry is ON** at every preset (`nsq_geom_level` 2 or 3 against
///   the allintra arm's 0 above M6), so a one-false boundary node keeps its
///   single injected edge shape instead of force-splitting.
///
/// KNOWN REMAINING DELTA, stated rather than hidden: C's video PD0 predicts
/// each block from the RECON it generates per block, because
/// `ctx->pd0_use_src_samples` is `allintra || hbd_md` (enc_mode_config.c:7309)
/// and the recon-neighbour arrays are filled from the source ONLY on the
/// allintra arm (product_coding_loop.c:8370). This function still predicts
/// from source. See `docs/INTER-ENCODE-PLAN.md` §1f.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_video(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    // The RESOLVED C `Pd0Level` (0..=6) — `set_pic_pd0_lvl_default` ->
    // `set_pd0_ctrls` -> `pd0_detector`, all three of which
    // `crate::part_arm::video_pd0_params` runs. NOT `pcs->pic_pd0_lvl`,
    // whose 0..=8 numbering is a different map; see [`video_pd0_mode`].
    pd0_level: u8,
    // C `MAX(2, pcs->rate_est_level)` / `MAX(4, ..)` at PD0
    // (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7355) mapped
    // through `set_rate_est_ctrls` to `coeff_rate_est_lvl`. Read only by the
    // LVL_1 family; LVL_5's closed form ignores it.
    coeff_rate_est_lvl: u8,
    // C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode <= M8`).
    accurate_part_ctx: bool,
    // C `ctx->nsq_geom_ctrls.enabled`.
    nsq_enabled: bool,
    // C `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`.
    depth_early_exit_lvl1: bool,
    // C `input_resolution_factor[..]` — the LVL_5 closed form's per-picture
    // coeff-rate addend.
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    tile_top: usize,
    tile_left: usize,
    stale_vars: Option<&SbVariance>,
    max_tx_size: u8,
    // The same `key_min_sq` [`pd0_pick_sb_partition_video_eval`] takes.
    key_min_sq: usize,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309) — the
    // same parameter `pd0_pick_sb_partition_m6_eval` takes, and the same
    // value from the same call site. `Some((md_recon_plane, stride))` is the
    // frame's MD recon; `None` keeps the source prediction.
    video_recon: Option<(&[u8], usize)>,
    // C `product_prediction_fun_table_pd0[1]` — `Some` on a NON-KEY frame.
    inter: Option<&Pd0InterRef<'_>>,
    // C `ctx->subres_ctrls.step`, resolved per superblock by
    // `svt_aom_sig_deriv_enc_dec_pd0` — `None` applies the level default.
    subres_step: Option<u32>,
    // C `ctx->parent_cost_bias` from the same signal derivation — read only
    // by the inter PD0_LVL_6 arm; every other level holds 1000.
    parent_cost_bias: u32,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` for this SB — forwarded verbatim to
    // [`pd0_pick_sb_partition_video_eval`].
    sb_lambda: Option<u64>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Tree {
    pd0_pick_sb_partition_video_eval(
        src,
        stride,
        sb_x,
        sb_y,
        qp,
        qindex,
        lambda_weight,
        tables,
        pd0_level,
        coeff_rate_est_lvl,
        accurate_part_ctx,
        nsq_enabled,
        depth_early_exit_lvl1,
        ires_factor,
        aligned_w,
        aligned_h,
        tile_top,
        tile_left,
        stale_vars,
        max_tx_size,
        key_min_sq,
        video_recon,
        false,
        inter,
        subres_step,
        parent_cost_bias,
        sb_lambda,
        ac_bias_eff,
    )
    .tree()
}

/// [`pd0_pick_sb_partition_video`]'s tree, as the EVAL it is derived from, so
/// `SVTAV1_PD0DBG` can walk the per-node PD0 costs on the pred-depth-only path
/// the way it already can on the `pd0_pick_sb_partition_m6_eval` one. Same
/// computation — `pd0_pick_sb_partition_video` is `…_eval(..).tree()`.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_video_eval(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    // The RESOLVED C `Pd0Level` (0..=6) — `set_pic_pd0_lvl_default` ->
    // `set_pd0_ctrls` -> `pd0_detector`, all three of which
    // `crate::part_arm::video_pd0_params` runs. NOT `pcs->pic_pd0_lvl`,
    // whose 0..=8 numbering is a different map; see [`video_pd0_mode`].
    pd0_level: u8,
    // C `MAX(2, pcs->rate_est_level)` / `MAX(4, ..)` at PD0
    // (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7355) mapped
    // through `set_rate_est_ctrls` to `coeff_rate_est_lvl`. Read only by the
    // LVL_1 family; LVL_5's closed form ignores it.
    coeff_rate_est_lvl: u8,
    // C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode <= M8`).
    accurate_part_ctx: bool,
    // C `ctx->nsq_geom_ctrls.enabled`.
    nsq_enabled: bool,
    // C `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`.
    depth_early_exit_lvl1: bool,
    // C `input_resolution_factor[..]` — the LVL_5 closed form's per-picture
    // coeff-rate addend.
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    tile_top: usize,
    tile_left: usize,
    stale_vars: Option<&SbVariance>,
    max_tx_size: u8,
    // C `set_blocks_to_be_tested`'s `min_sq_size` on a key frame
    // (enc_dec_process.c:1485): `disallow_4x4 ? 8 : 4` — read only when
    // `inter` is `None`, since a non-key frame's per-SB `min_sq` rides
    // `Pd0InterRef::min_sq`. NOT always 8: `pic_disallow_4x4` is 0 at
    // video M0..M2 (`svt_aom_get_disallow_4x4_default`,
    // enc_mode_config.c:8169), so key frames there floor PD0 at 4x4.
    key_min_sq: usize,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309) — the
    // same parameter `pd0_pick_sb_partition_m6_eval` takes, and the same
    // value from the same call site. `Some((md_recon_plane, stride))` is the
    // frame's MD recon; `None` keeps the source prediction.
    video_recon: Option<(&[u8], usize)>,
    // `SVTAV1_PD0_NOSPLIT`, a CONTROL — see `crate::dbgenv::pd0_nosplit`.
    // Never true in a shipped configuration.
    ctl_nosplit: bool,
    // C `product_prediction_fun_table_pd0[1]` — `Some` on a NON-KEY frame
    // only. See [`Pd0InterRef`] for why an inter frame's PD0 has exactly one
    // candidate and it is never intra.
    inter: Option<&Pd0InterRef<'_>>,
    // C `ctx->subres_ctrls.step`, resolved per superblock by
    // `svt_aom_sig_deriv_enc_dec_pd0` — `None` applies the level default.
    subres_step: Option<u32>,
    // C `ctx->parent_cost_bias` from the same signal derivation — read only
    // by the inter PD0_LVL_6 arm; every other level holds 1000.
    parent_cost_bias: u32,
    // C `full_sb_lambda_md[EB_8_BIT_MD]` for this SB — `Some` skips the
    // `qindex`/`lambda_weight` rederivation (which drops the delta-q stats
    // factor whenever the SB qindex differs from base). See
    // [`pd0_frame_lambda_and_min_sq`].
    sb_lambda: Option<u64>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — drives
    // C's `svt_psy_adjust_rate_light` subtraction on the PD0 coeff
    // bits inside `perform_tx_pd0`. 0.0 under mainline.
    ac_bias_eff: f64,
) -> Pd0Eval {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let mode = video_pd0_mode(pd0_level);
    // C `full_sb_lambda_md[EB_8_BIT_MD]` = `av1_lambda_assign_md`'s
    // `full_lambda_md[0]` (md_process.c:763), which is the FRAME's MD lambda —
    // the KF chain only on a key frame. MEASURED on `diag 64x64 q40 p8`:
    // C's `svt_aom_full_cost_pd0` lambda is 18500 on frame 0 (`base_q_idx`
    // 67, the KF chain) and 241378 on frame 1 (`base_q_idx` 160, base 3.2 x
    // factor 150) — where the KF chain at qindex 160 would give 248207.
    // Shared with `pd0_pick_sb_partition_m6_eval`, which is the entry point
    // the REFINEMENT path takes — a second copy of this resolution is exactly
    // the duplicate-transcription trap docs/WORKING-ON-THIS.md §4 records.
    // `key_min_sq` is C's `disallow_4x4 ? 8 : 4` arm with depth removal off —
    // NOT always 8: `pic_disallow_4x4` is 0 at M0..M2 on the video arm
    // (`svt_aom_get_disallow_4x4_default`, enc_mode_config.c:8169), so a
    // key frame at those presets floors PD0 at 4x4, which the caller
    // resolves per arm/preset.
    let (lambda, inter_min_sq) =
        pd0_frame_lambda_and_min_sq(qindex, lambda_weight, key_min_sq, inter, sb_lambda);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        qm_level: 15,
        lambda,
        mode,
        lvl1: Some(tables),
        // `get_max_block_size_default` = `scs->super_block_size`, uncapped.
        max_sq: 64.min(max_tx_size as usize),
        // `key_min_sq` on a key frame (the caller's `disallow_4x4 ? 8 : 4`
        // fold — `pic_disallow_4x4` is 0 at video M0..M2, so this is 4
        // there), `Pd0InterRef::min_sq` on a non-key frame where
        // `depth_removal_ctrls` can raise it to 16, 32 or 64 per superblock.
        min_sq: if ctl_nosplit { 64 } else { inter_min_sq },

        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        subres_step: subres_step.unwrap_or_else(|| mode.default_subres_step()),
        ires_factor,
        coeff_rate_est_lvl,
        ac_bias_eff,
        accurate_part_ctx,
        depth_early_exit_th: if depth_early_exit_lvl1 { 1000 } else { 900 },
        // `sig_deriv_enc_dec_pd0`'s resolved value — off 1000 only at
        // `PD0_LVL_6` on a non-I slice, the arm that reads it.
        parent_cost_bias,
        nsq_enabled,
        tile_top,
        tile_left,
        recon_canvas: video_recon.map(|(r, st)| Pd0ReconCanvas::new(r, st, sb_y)),
        inter,
        pending_recon: None,
        scratch: take_scratch(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    let (rd_cost, mut eval) = ctx.pick(64, 0, 0);
    return_scratch(core::mem::take(&mut ctx.scratch));
    eval.root_det = Some(Pd0RootDet {
        rd_cost,
        blk: ctx.root_det.blk,
    });
    eval
}
