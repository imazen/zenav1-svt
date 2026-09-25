use super::*;

#[inline(always)]
pub(super) fn encode_coding_unit(
    encode_input: &[u8],
    sb_input: &[u8],
    in_stride: usize,
    w: usize,
    h: usize,
    sb_size: usize,
    base_qindex: u8,
    qindex_u: u8,
    qindex_v: u8,
    qm_levels: [u8; 3],
    sc_arm: crate::sc_detect::ScArm,
    cli_qp: u8,
    picture_qp: u8,
    lw_bump: u32,
    tune_iq: bool,
    speed_config: &SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    temporal_layer: u8,
    ref_padded: Option<&crate::picture::PaddedRef>,
    inter_md: Option<&crate::inter_md_arm::InterMdFrame<'_>>,
    pd0_dr_res: Option<&[crate::port_enc_mode_config::common::DepthRemovalResult]>,
    ref_min_max_sq: Option<(&[u8], &[u8])>,
    md_frame_cdfs: Option<&crate::port_frame_cdf::FrameCdfs>,
    c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
    bit_depth: u8,
    max_tx_size: u8,
    coded_lossless: bool,
    pd0_det_frame: crate::part_arm::Pd0DetFrame<'_>,
    lpd1_frame: Option<Lpd1FrameIn>,
    chroma_src: Option<(&[u8], &[u8])>,
    tile_sb_row_start: usize,
    tile_sb_col_start: usize,
    m6_pd0_tables: &mut Option<crate::pd0::M6Pd0Tables>,
    pd0_frame_tables: impl Fn(u8) -> crate::pd0::M6Pd0Tables,
    use_funnel: bool,
    tile_sc: crate::sc_detect::ScDerivation,
    md_preset: i8,
    pd0_pred_depth_only: bool,
    pd0_video_recon: bool,
    cwid: usize,
    fun_u_recon: &mut Vec<u8>,
    fun_v_recon: &mut Vec<u8>,
    fun_ectx: &mut Option<EntropyCtx>,
    fun_frame: &Option<crate::leaf_funnel::FunnelFrame>,
    ibc_state: &Option<Box<crate::leaf_funnel::IbcFrameState>>,
    ibc_mvp_grid: &mut Vec<crate::intrabc_mvp::MvpMiEntry>,
    funnel_chain: bool,
    chain_snaps: &mut Vec<(
        crate::entropy::context::FrameContext,
        Box<crate::entropy::coeff_c::CoeffFc>,
    )>,
    sim_ectx: &mut Option<EntropyCtx>,
    sim_geom: &mut crate::deblock::DeblockGeom,
    sim_u: &mut Vec<u8>,
    sim_v: &mut Vec<u8>,
    sim_prev_sb_row: &mut usize,
    fun_rates: &Option<Box<crate::leaf_funnel::MdRates>>,
    tile_frame_recon: &mut Vec<u8>,
    bd10_full_rd: bool,
    bd10_mds3_bump: bool,
    bd10_plumb: bool,
    funnel_src10: Option<crate::leaf_funnel::FunnelSrc10<'_>>,
    tile_frame_recon10: &mut Vec<u16>,
    tile_frame_u_recon10: &mut Vec<u16>,
    tile_frame_v_recon10: &mut Vec<u16>,
    part_config: &crate::partition::PartitionSearchConfig,
    sb_row: usize,
    sb_x0: usize,
    sb_y0: usize,
    sb_md_full_lambda: u64,
    ref_ctx: Option<crate::partition::RefFrameCtx<'_>>,
    sb_qindex: u8,
    units: Vec<(usize, usize)>,
    unit_size: usize,
    use_pd0: bool,
    sb_lambda: u64,
    sb_index: usize,
    pd0_sb_in: crate::port_pd0_detector::Pd0SbInput,
    pd0_disallow_4x4: bool,
    sb_pd0_det: Option<(u8, u8, bool, u32, u32)>,
    sb_disallow_below_64x64: bool,
    pd0_refined_mode: crate::pd0::Pd0Mode,
    pd0_refined_eexit_th: u128,
    pd0_refined_rate_lvl: u8,
    sb_stale_vars: Option<&crate::pd0::SbVariance>,
    local_sb_index: usize,
    chain_base: Option<(
        crate::entropy::context::FrameContext,
        Box<crate::entropy::coeff_c::CoeffFc>,
    )>,
    mut unit_results: Vec<crate::partition::PartitionResult>,
    mut unit_enc_rdoq: Vec<bool>,
    pd0_inter: Option<crate::pd0::Pd0InterRef<'_>>,
    mut sb_pd0_max_min: Option<(usize, usize)>,
    // Fork `get_effective_ac_bias(ac_bias, is_islice, tl)` — the value
    // every PD0/light-PD1 `svt_psy_adjust_rate_light` site consumes.
    ac_bias_eff: f64,
    // The ac-bias rate adjustment is Ghost Robot-behaviour only at this
    // scope — Hybrid3115 keeps its pinned surface (plan 3.3).
    reference: crate::reference::SvtReference,
) -> (crate::partition::PartitionResult, bool) {
    let ac_bias_eff = if reference == crate::reference::SvtReference::GhostRobot {
        ac_bias_eff
    } else {
        0.0
    };
    for &(x0, y0) in units.iter() {
        let cur_w = unit_size.min(w - x0);
        let cur_h = unit_size.min(h - y0);
        // C-exact partition source gate.
        // Task #95 chunk 2: partial units (cur_w/cur_h < unit_size)
        // take the PD0 fixed-tree path too — C decides the ENTIRE
        // partition tree in PD0 for every b64 including incomplete
        // ones, starting from a 64x64 root that carries the
        // spec-5.11.4 forced edge splits. Complete units are
        // unaffected (cur_w == cur_h == unit_size).
        // Retained (underscored) rather than deleted: the PD1 walk no
        // longer needs it, but it is the canonical "is this unit
        // complete?" predicate and the next edge-aware path will.
        let _full_sb = cur_w == unit_size && cur_h == unit_size;
        // C `md_ctx->fixed_partition = md_ctx->pred_depth_only &&
        // md_ctx->md_disallow_nsq_search` (enc_dec_process.c:3054),
        // with `md_disallow_nsq_search = !nsq_geom_ctrls.enabled ||
        // !nsq_search_ctrls.enabled` (:7846). §1z⁗ fixed the SECOND
        // conjunct on the `< 9` gate below; this gate still spelled
        // it `preset >= 9`, which is only `pred_depth_only`.
        //
        // On the ALLINTRA arm the one-term form is right at every
        // preset: `svt_aom_get_nsq_geom_level_allintra` returns 0
        // from M7 up (enc_mode_config.c:8240), so
        // `md_disallow_nsq_search` is true and the conjunction adds
        // nothing — `NsqCfg::for_levels` short-circuits to `off()`
        // on a zero geometry level, which makes that byte-inert by
        // construction rather than by measurement alone.
        //
        // The VIDEO arm separates them. `..._default` never returns
        // a zero geometry level, and `get_nsq_search_level_default`
        // has base 19 above M7 with a seq-QP offset that SATURATES
        // TO ZERO: qp <= 48 gives 19 + 1 > 19 -> 0 (search off,
        // this gate right), qp >= 49 keeps 19 or 18 (search ON,
        // this gate wrong). That is why `video_key_matrix.sh` at
        // its default qp 40 reports IDENTICAL at p9..p13 and could
        // not witness this.
        //
        // KNOWN GAP, stated rather than hidden: bd10 keeps the
        // fixed tree unconditionally at preset >= 9. C's
        // `fixed_partition` does not read the bit depth, so a bd10
        // video frame at qp >= 49 would take the refinement walk
        // there too — but the port's bd10 partition path is
        // `pd0_pick_sb_partition_lvl0` (C forces PD0_LVL_0 at
        // `hbd_md`) and nothing has dumped C's bd10 video tree, so
        // widening it blind would trade a green gate for a guess.
        let p9_fixed_partition = md_preset >= 9
            && (bit_depth == 10
                || !crate::depth_refine::NsqCfg::for_arm_with_coeff(
                    sc_arm,
                    md_preset,
                    u32::from(cli_qp),
                    c_quant
                        .as_ref()
                        .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
                    temporal_layer,
                )
                .enabled);
        // C `ed_ctx->md_ctx->rdoq_ctrls->enabled` for the encode
        // pass — `set_rdoq_controls(pcs->rdoq_level)` on the
        // regular arm (`sig_deriv_enc_dec_default`); a light-PD1
        // superblock's funnel-ctx site below overrides it with
        // that arm's row (`sig.rdoq.enabled`).
        let mut enc_rdoq_enabled = c_quant.as_ref().map_or(false, |q| q.rdoq_level != 0);
        let sb_result = if coded_lossless && !use_funnel {
            let tree = crate::pd0::lossless_tree(x0, y0, unit_size, w, h);
            crate::lossless_mono::encode_tree(
                &sb_input[y0 * in_stride + x0..],
                in_stride,
                tile_frame_recon,
                w,
                &tree,
                x0,
                y0,
                unit_size,
                part_config,
                fun_rates.as_ref().unwrap(),
                fun_ectx.as_mut().unwrap(),
            )
        } else if use_pd0 {
            // At presets 0..3 C permits 4x4 blocks and runs PD1
            // even for lossless. Higher presets have only 8x8 leaves.
            if (coded_lossless && md_preset >= 4) || p9_fixed_partition {
                // C `md_encode_block`'s Light-PD1 per-SB dispatch:
                // `resolve_sb_lpd1` reads the PD0 root eval the
                // video arm below produces, so the Option is
                // populated there and read by the leaf walk's
                // `FunnelCtx::lpd1`. `None` on every non-light
                // superblock (the whole still envelope).
                let mut sb_lpd1 = None;
                let tree = if coded_lossless {
                    crate::pd0::lossless_tree(x0, y0, unit_size, w, h)
                } else if bit_depth == 10 && (speed_config.preset <= 5 || inter_md.is_none()) {
                    // C `set_pd0_ctrls` (enc_mode_config.c:5415) FORCES
                    // PD0_LVL_0 (full-RD partition search) at bd10 (hbd_md
                    // set), regardless of preset — where bd8 uses the
                    // preset's LVL_6/LVL_5 variance heuristic. The force
                    // is conditional on `pcs->hbd_md != 0`, NOT on the
                    // bit depth alone: a bd10 non-I frame at M6+ derives
                    // `hbd_md = 0` (`is_islice ? 2 : 0`,
                    // enc_mode_config.c:2163 — TRACED 2026-09-18), so
                    // it keeps the video ladder's level and falls to the
                    // `ScArm::Video` arm below. LVL_0 runs
                    // at 8-bit on the same MSB-truncated `sb_input`, so
                    // this is a pure partition change; the coded levels
                    // are recomputed at 10-bit by bd10_reencode_luma.
                    crate::pd0::pd0_pick_sb_partition_lvl0(
                        sb_input,
                        in_stride,
                        x0,
                        y0,
                        cli_qp as u32,
                        sb_qindex,
                        // C `pcs->lambda_weight` for this frame — the PSNR
                        // ladder keyed on `picture_qp` plus the extended-CRF
                        // bump (pd0::frame_lambda_weight). Identical to the
                        // old `kf_full_lambda_8bit(qindex, cli_qp)` ladder
                        // whenever the CRF offset is 0.
                        crate::pd0::frame_lambda_weight_for_preset(
                            speed_config.preset,
                            u32::from(picture_qp),
                            tune_iq,
                            lw_bump,
                        ),
                        // [SVT_HDR_MODE] Frame luma QM level. C forces
                        // PD0_LVL_0 at bd10 whose light encode applies
                        // the matrix when using_qmatrix (fork default);
                        // mainline/QM-off leave qm_levels = [15;3], so
                        // this is the non-QM (byte-inert) path there.
                        qm_levels[0],
                        crate::pd0::input_resolution_factor(w * h),
                        w,
                        h,
                        // Superres chunk B.4: this SB's STALE full-res variance entry.
                        sb_stale_vars,
                        // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                        max_tx_size,
                        // C `full_sb_lambda_md[EB_8_BIT_MD]`
                        // (svt_aom_full_cost_pd0's lambda — PD0
                        // runs at 8-bit even at bd10).
                        Some(sb_md_full_lambda),
                        ac_bias_eff,
                    )
                } else if matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }) {
                    // The VIDEO arm's PD0, which is a different
                    // LEVEL, an uncapped max block size and NSQ
                    // geometry ON — see
                    // `pd0::pd0_pick_sb_partition_video`. The
                    // allintra arm below is untouched, so the still
                    // envelope is byte-neutral by construction.
                    // `sb_pd0_det` resolved the level + rate level
                    // + subres step per superblock at the top of
                    // the loop — `unreachable!` is off the arm
                    // that guards this branch (`ScArm::Video`).
                    let (
                        pic_pd0_lvl,
                        pd0_coeff_rate_est_lvl,
                        accurate_part_ctx,
                        sb_subres_step,
                        sb_parent_cost_bias,
                    ) = sb_pd0_det
                        .unwrap_or_else(|| unreachable!("ScArm::Video always builds sb_pd0_det"));
                    let tables = m6_pd0_tables.get_or_insert_with(|| pd0_frame_tables(sb_qindex));
                    let eval = crate::pd0::pd0_pick_sb_partition_video_eval(
                        sb_input,
                        in_stride,
                        x0,
                        y0,
                        u32::from(cli_qp),
                        sb_qindex,
                        crate::pd0::frame_lambda_weight_for_preset(
                            speed_config.preset,
                            u32::from(picture_qp),
                            tune_iq,
                            lw_bump,
                        ),
                        tables,
                        pic_pd0_lvl,
                        pd0_coeff_rate_est_lvl,
                        accurate_part_ctx,
                        crate::part_arm::nsq_geom_enabled(sc_arm, md_preset),
                        // This branch IS C's `pic_pred_depth_only`
                        // case: `depth_refinement_ctrls.mode ==
                        // PD0_DEPTH_PRED_PART_ONLY` is what makes
                        // the port code the PD0 tree directly
                        // instead of running the refinement walk,
                        // and it is the same flag
                        // `set_depth_early_exit_ctrls` reads
                        // (enc_mode_config.c:7232).
                        true,
                        crate::pd0::input_resolution_factor(w * h),
                        w,
                        h,
                        tile_sb_row_start * sb_size,
                        tile_sb_col_start * sb_size,
                        sb_stale_vars,
                        max_tx_size,
                        // C `set_blocks_to_be_tested`'s
                        // `disallow_4x4 ? 8 : 4` — 4 at video
                        // M0..M2, where `pic_disallow_4x4` is 0.
                        if pd0_disallow_4x4 { 8 } else { 4 },
                        // C `pd0_use_src_samples` (video arm: recon)
                        // — the same value the refinement path gets.
                        pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                        crate::dbgenv::pd0_nosplit() && inter_md.is_some(),
                        pd0_inter.as_ref(),
                        // `ctx->subres_ctrls.step`, resolved per
                        // superblock by `sb_pd0_det`.
                        Some(sb_subres_step),
                        // `ctx->parent_cost_bias`, same source —
                        // read only by the inter PD0_LVL_6 arm.
                        sb_parent_cost_bias,
                        // C `full_sb_lambda_md[EB_8_BIT_MD]`
                        // (svt_aom_full_cost_pd0's lambda).
                        Some(sb_md_full_lambda),
                        ac_bias_eff,
                    );
                    // C `md_encode_block`'s `lpd1` per-SB dispatch
                    // off the PD0 root result. The light path uses
                    // the SAME fixed tree — `pd1_level` only
                    // switches the leaf evaluator, not the shape.
                    sb_lpd1 = lpd1_frame.as_ref().and_then(|i| {
                        resolve_sb_lpd1(
                            i,
                            &pd0_sb_in,
                            &pd0_det_frame,
                            eval.root_det.as_ref(),
                            pic_pd0_lvl,
                            sb_index,
                            u32::from(sb_qindex),
                            sb_md_full_lambda,
                            sb_disallow_below_64x64,
                        )
                    });
                    eval.tree()
                } else {
                    crate::pd0::pd0_pick_sb_partition(
                        sb_input,
                        in_stride,
                        x0,
                        y0,
                        cli_qp as u32,
                        sb_qindex,
                        // C `pcs->lambda_weight` for this frame — the PSNR
                        // ladder keyed on `picture_qp` plus the extended-CRF
                        // bump (pd0::frame_lambda_weight). Identical to the
                        // old `kf_full_lambda_8bit(qindex, cli_qp)` ladder
                        // whenever the CRF offset is 0.
                        crate::pd0::frame_lambda_weight_for_preset(
                            speed_config.preset,
                            u32::from(picture_qp),
                            tune_iq,
                            lw_bump,
                        ),
                        // C `input_resolution_factor[input_resolution]`:
                        // per-picture coeff-rate addend keyed on w*h.
                        crate::pd0::input_resolution_factor(w * h),
                        // ALIGNED dims — the spec-5.11.4 edge predicate grid.
                        w,
                        h,
                        // Superres chunk B.4: this SB's STALE full-res variance entry.
                        sb_stale_vars,
                        // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                        max_tx_size,
                        // C `full_sb_lambda_md[EB_8_BIT_MD]`
                        // (svt_aom_full_cost_pd0's lambda).
                        Some(sb_md_full_lambda),
                        ac_bias_eff,
                    )
                };
                // The same per-SB variance map C's picture analysis
                // feeds to is_dc_only_safe (pcs->ppcs->variance): the
                // fixed-tree leaves use it to force the C-exact
                // DC-only intra candidate set where the gate fires.
                let sb_vars = sb_stale_vars.copied().unwrap_or_else(|| {
                    crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0)
                });
                // C `ctx->sq_sb_me_mv` — see
                // `inter_search_arm::SqMeState`. Declared per
                // superblock walk, which is where C's own
                // `pc_tree` is reset; the state is node-KEYED, so
                // a leftover from another superblock could not be
                // read anyway.
                let mut inter_sq_me = crate::inter_search_arm::SqMeState::default();
                // Same per-SB `sig_deriv` arm the funnel ctx below
                // encodes: a light-PD1 superblock's `rdoq_ctrls`
                // row, not the frame `rdoq_level`.
                if let Some(l) = sb_lpd1.as_ref() {
                    enc_rdoq_enabled = l.sig.rdoq.enabled;
                }
                let mut funnel_ctx = if use_funnel {
                    let (u_src, v_src) = chroma_src.unwrap();
                    Some(crate::leaf_funnel::FunnelCtx {
                        u_src,
                        v_src,
                        src10: funnel_src10,
                        u_recon: fun_u_recon,
                        v_recon: fun_v_recon,
                        c_stride: cwid,
                        ectx: fun_ectx.as_mut().unwrap(),
                        rates: fun_rates.as_deref().unwrap(),
                        frame: fun_frame.as_ref().unwrap(),
                        frame_ssim: None,
                        // bd10 luma mode funnel (task #94): true 10-bit
                        // recon canvas for the per-block mode decision;
                        // None (bd8 / other presets / partial-SB) is
                        // byte-identical. `bd10_plumb` also covers the
                        // bypass-encdec MDS3 bump (C `hbd_md = 2`,
                        // product_coding_loop.c:9649) — the canvas is
                        // C's recon_pic(1) under that bump too.
                        y_recon10: if bd10_plumb {
                            Some(tile_frame_recon10)
                        } else {
                            None
                        },
                        u_recon10: if bd10_full_rd || bd10_mds3_bump {
                            Some(tile_frame_u_recon10)
                        } else {
                            None
                        },
                        v_recon10: if bd10_full_rd || bd10_mds3_bump {
                            Some(tile_frame_v_recon10)
                        } else {
                            None
                        },
                        // IBC chunk 8: frame IntraBC state + the MD mi grid.
                        ibc: ibc_state.as_deref(),
                        inter: inter_md,
                        // C `ctx->sq_sb_me_mv` +
                        // `pc_tree->tested_blk[PART_N][0]` — one
                        // slot per superblock walk, written by a
                        // square block's MD motion search and read
                        // by the NSQ shapes at the same node.
                        inter_sq_me: inter_md.map(|_| &mut inter_sq_me),
                        ibc_mvp: if ibc_state.is_some() || inter_md.is_some() {
                            Some(ibc_mvp_grid)
                        } else {
                            None
                        },
                        ibc_gate: Default::default(),
                        full_rd10: bd10_full_rd,
                        lpd1: sb_lpd1.clone(),
                        ii_preds: None,
                    })
                } else {
                    None
                };
                crate::partition::encode_fixed_tree(
                    sb_input,
                    in_stride,
                    tile_frame_recon,
                    w,
                    &tree,
                    unit_size,
                    sb_qindex,
                    part_config,
                    x0,
                    y0,
                    w,
                    h,
                    &sb_vars,
                    (x0, y0),
                    funnel_ctx.as_mut(),
                    ref_ctx.as_ref(),
                )
            } else {
                // Per-SB PD0 rate tables from the chain (C rebuilds
                // rate_est_table from ec_ctx_array[sb] BEFORE the
                // SB's PD0 runs — the drifting SPLIT rates).
                let chained_tables = if funnel_chain {
                    Some(match &chain_base {
                        Some((fc, cfc)) => crate::pd0::build_m6_pd0_tables_from_ctx(fc, cfc),
                        None => pd0_frame_tables(sb_qindex),
                    })
                } else {
                    None
                };
                let tables = match &chained_tables {
                    Some(t) => t,
                    None => m6_pd0_tables.get_or_insert_with(|| pd0_frame_tables(sb_qindex)),
                };
                // The PD1 depth-refinement walk IS edge-aware as of
                // 2026-08-04 (depth_refine.rs: forced split at a
                // both-false node, the single injected shape at a
                // one-false node priced from the BINARY alphabet, and
                // off-frame quadrants skipped), so partial SBs no
                // longer fall back to the plain PD0 fixed tree. That
                // fallback was the structural reason presets 0..=5
                // could not byte-match C on non-64-aligned geometry:
                // C runs its PD1 refinement on every SB, complete or
                // not, so a partial SB taking a DIFFERENT SEARCH could
                // only match by coincidence.
                // C `ctx->pred_depth_only` (enc_mode_config.c:7095)
                // is `mode == PD0_DEPTH_PRED_PART_ONLY`, i.e. the
                // refinement walk runs whenever the level is NOT 10.
                // This used to be `matches!(preset, 0..=5)`, which is
                // the same predicate ON THE ALLINTRA ARM (that ladder
                // returns 10 at M6 and above and an adaptive level
                // below) but NOT on the video arm, where M6/M7 are
                // levels 6/8 — adaptive. Deriving it from the ctrls
                // keeps the still path byte-identical by construction
                // and lets a video key frame take the refinement C
                // runs for it.
                let dr = if coded_lossless {
                    crate::depth_refine::DrCtrls::lossless(false)
                } else {
                    crate::depth_refine::DrCtrls::for_arm(
                        sc_arm,
                        md_preset,
                        tile_sc.classes.sc_class5,
                        cli_qp as u32,
                        c_quant
                            .as_ref()
                            .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
                    )
                };
                // C `md_ctx->fixed_partition = md_ctx->pred_depth_only &&
                // md_ctx->md_disallow_nsq_search`
                // (enc_dec_process.c:3054, comment: "If there is only
                // one depth and no NSQ search at PD1, then the
                // partition structure is fixed"), with
                // `md_disallow_nsq_search = !nsq_geom_ctrls.enabled ||
                // !nsq_search_ctrls.enabled` (:7846). This gate had
                // only the FIRST conjunct, so a picture that is
                // pred-depth-only but still SEARCHES NSQ shapes took
                // the fixed-tree path and coded squares where C codes
                // an H/V/4-way shape at the same depth.
                let nsq_search_on = crate::depth_refine::NsqCfg::for_arm_with_coeff(
                    sc_arm,
                    md_preset,
                    cli_qp as u32,
                    c_quant
                        .as_ref()
                        .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
                    temporal_layer,
                )
                .enabled;
                let mut refined = use_funnel && (dr.adaptive || nsq_search_on);
                let nsq_geom_enabled =
                    !coded_lossless && crate::part_arm::nsq_geom_enabled(sc_arm, md_preset);
                // C md_config_process.c forces lossless PD0 level 0,
                // but its resolved cost model uses QP offset 0 and
                // fast coefficient estimation 2: Rust's Lvl1 model.
                // The Lvl0 helper's QP+8/closed-rate model is wrong here.
                // C caps lossless candidate squares at 8x8.
                let search_max_sq = if coded_lossless { 8 } else { max_tx_size };
                // C `md_encode_block` order (enc_dec_process.c:2977-3031):
                // PD0 runs ONCE — under the frame's ORIGINAL
                // `depth_early_exit_lvl` — `lpd1_detector_post_pd0`
                // reads its PART_N root block, and only AFTER that
                // does a kept level force
                // `depth_refinement_ctrls.mode = PD0_DEPTH_PRED_PART_ONLY`
                // + `pred_depth_only = 1` (:3024-3027) while the light
                // signal derivation forces `md_disallow_nsq_search = 1`
                // (enc_mode_config.c:7569) — so `fixed_partition` wins
                // and `pick_partition_lpd1` walks the SAME PD0 tree
                // (it is NOT rebuilt under the forced flag). This
                // closure is therefore the refinement arm's own eval:
                // the detector reads the root C read, a kept light
                // level walks the tree C walked, and a demoted SB's
                // refinement walk reuses it rather than paying for a
                // second PD0.
                let eval_pd0 = || -> crate::pd0::Pd0Eval {
                    // Levels 1..=2 keep the `m6_eval` fallback —
                    // `video_pd0_mode` has no block cost for them.
                    // Level 0 is the `set_pd0_ctrls` hbd_md force
                    // (a bd10 frame's every SB) and HAS the real
                    // closed form, so it must take the video arm.
                    if matches!(sc_arm, crate::sc_detect::ScArm::Video { .. })
                        && sb_pd0_det.is_some_and(|t| t.0 == 0 || (3..=6).contains(&t.0))
                    {
                        // The VIDEO arm's own PD0 entry point — the
                        // same one the non-refined arm takes below.
                        // `refined_pd0_model` carries only `Pd0Level`s
                        // 3 and 4 and falls back to LVL_1's block
                        // cost otherwise, so the M9+ `PD0_LVL_5` a
                        // video key frame resolves to (INVALID
                        // `coeff_lvl` arm of `set_pic_pd0_lvl_default`)
                        // was priced with the wrong model here —
                        // observed on `gradient 64x64 q55 p9` frame 0
                        // as a 4x32x32-square tree where C's PD0+NSQ
                        // walk commits a 5-leaf H/V tree (C 289 B,
                        // port 406 B). `video_eval` has the real
                        // LVL_5 cost, the `use_accurate_part_ctx = 0`
                        // doubled SPLIT rate above M8, and the per-SB
                        // `subres_step`.
                        let (
                            pic_pd0_lvl,
                            pd0_coeff_rate_est_lvl,
                            accurate_part_ctx,
                            sb_subres_step,
                            sb_parent_cost_bias,
                        ) = sb_pd0_det.unwrap_or_else(|| {
                            unreachable!("ScArm::Video always builds sb_pd0_det")
                        });
                        crate::pd0::pd0_pick_sb_partition_video_eval(
                            sb_input,
                            in_stride,
                            x0,
                            y0,
                            cli_qp as u32,
                            sb_qindex,
                            crate::pd0::frame_lambda_weight_for_preset(
                                speed_config.preset,
                                u32::from(picture_qp),
                                tune_iq,
                                lw_bump,
                            ),
                            tables,
                            pic_pd0_lvl,
                            pd0_coeff_rate_est_lvl,
                            accurate_part_ctx,
                            nsq_geom_enabled,
                            // C `depth_early_exit_lvl` is 1 when
                            // `pd0_level <= PD0_LVL_1 ||
                            // ctx->pic_pred_depth_only`
                            // (enc_mode_config.c:7231-7233) —
                            // `refined_pd0_model`'s own `th`
                            // computation, minus its `_` arm's
                            // forced 1000.
                            pic_pd0_lvl <= 1 || pd0_pred_depth_only,
                            crate::pd0::input_resolution_factor(w * h),
                            w,
                            h,
                            tile_sb_row_start * sb_size,
                            tile_sb_col_start * sb_size,
                            sb_stale_vars,
                            search_max_sq,
                            // Same `disallow_4x4 ? 8 : 4` fold —
                            // 4 at video M0..M2.
                            if pd0_disallow_4x4 { 8 } else { 4 },
                            pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                            crate::dbgenv::pd0_nosplit() && inter_md.is_some(),
                            pd0_inter.as_ref(),
                            Some(sb_subres_step),
                            sb_parent_cost_bias,
                            // C `full_sb_lambda_md[EB_8_BIT_MD]`
                            // (svt_aom_full_cost_pd0's lambda).
                            Some(sb_md_full_lambda),
                            ac_bias_eff,
                        )
                    } else {
                        crate::pd0::pd0_pick_sb_partition_m6_eval(
                            // SB-EXTENT padded plane, not the raw frame:
                            // `compute_b64_variance` reads a full 64x64
                            // unclamped, so a partial SB must read C's
                            // replicated border rather than running off the
                            // end (or stride-wrapping into the next row).
                            // Identical to `encode_input`/`w` on a
                            // 64-aligned frame -- `sb_input_owned` is None
                            // there -- so this is byte-neutral.
                            sb_input,
                            in_stride,
                            x0,
                            y0,
                            cli_qp as u32,
                            sb_qindex,
                            // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                            // ladder on `picture_qp` + the extended-CRF bump.
                            crate::pd0::frame_lambda_weight_for_preset(
                                speed_config.preset,
                                u32::from(picture_qp),
                                tune_iq,
                                lw_bump,
                            ),
                            tables,
                            if dr.disallow_4x4 { 8 } else { 4 },
                            // M4/M5: rate_est_level 1 -> coeff_rate_est_lvl 1
                            // (real PD0 coeff rate). M7/M8's level-2 PD0
                            // approximation only fires when this is >= 2.
                            if coded_lossless {
                                1
                            } else {
                                pd0_refined_rate_lvl
                            },
                            if coded_lossless {
                                crate::pd0::Pd0Mode::Lvl1
                            } else {
                                pd0_refined_mode
                            },
                            if coded_lossless {
                                1000
                            } else {
                                pd0_refined_eexit_th
                            },
                            // max-block variance cap. Allintra: M8+
                            // only (`get_max_block_size_allintra`'s
                            // `base_var_th_cap` is `(uint16_t)~0`
                            // through M7). Video: never
                            // (`get_max_block_size_default` returns
                            // `super_block_size` outright). Either way
                            // false on this `refined` p<=5 branch —
                            // routed through the arm helper so the two
                            // PD0 call sites cannot drift apart.
                            crate::part_arm::max_block_cap_active(
                                sc_arm,
                                speed_config.preset,
                                true,
                            ),
                            // NSQ geometry: a one-false node keeps its
                            // edge shape when NSQ shapes exist, and
                            // force-splits when they do not (p4/p5, where
                            // `NsqCfg::for_preset_qp` is `off()`).
                            nsq_geom_enabled,
                            // ALIGNED dims — this `refined` path is
                            // full-SB-gated (see `refined` above), so the
                            // edge/off branches never fire; passing the
                            // frame dims keeps the predicate well-defined.
                            w,
                            h,
                            // Tile pixel origin (full-SB refined path is
                            // single-tile-only in the tested envelope; 0
                            // when untiled → byte-inert).
                            tile_sb_row_start * sb_size,
                            tile_sb_col_start * sb_size,
                            // Superres chunk B.4: this SB's STALE full-res variance entry.
                            sb_stale_vars,
                            // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                            search_max_sq,
                            // C `pd0_use_src_samples` (video arm: recon).
                            pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                            // PD0's INTER arm on the REFINEMENT path.
                            //
                            // MEASURED 2026-09-02 on `gradient 64x64
                            // q20 p6` frame 1, against C's own
                            // `SVT_PD0COST_OUT` + `SVT_PICKPART_OUT`:
                            // without it this call ran the ALLINTRA
                            // PD0 on an inter frame — a DC prediction
                            // (64x64 dist 2_045_904 against C's
                            // 50_800), the KEY-frame lambda (25650
                            // against C's 24898), and `min_sq` 8, so
                            // it evaluated EIGHTY nodes (1x64, 4x32,
                            // 15x16, 60x8) where C evaluates FIVE.
                            // C's own `dr=1/0/1/1` and the port's
                            // `PD0DR ... minsq=32` already AGREED on
                            // that superblock; the value simply never
                            // reached this entry point.
                            //
                            // `None` on a key frame and on every
                            // allintra cell (see `pd0_inter_base`),
                            // so the still envelope is untouched by
                            // construction.
                            pd0_inter.as_ref(),
                            // `ctx->subres_ctrls.step` — the per-SB
                            // value `sig_deriv_enc_dec_pd0` resolved
                            // (video arm only; `None` on allintra
                            // keeps the level default).
                            sb_pd0_det.map(|t| t.3),
                            // C `full_sb_lambda_md[EB_8_BIT_MD]`
                            // (svt_aom_full_cost_pd0's lambda).
                            Some(sb_md_full_lambda),
                            ac_bias_eff,
                        )
                    }
                };
                // The probe only pays where light can fire: a nonzero
                // picture level, or the >M10 ME bump that can raise a
                // per-SB level from 0 (`sig_deriv_enc_dec_common`,
                // enc_mode_config.c:7170-7174 — that arm is
                // `enc_mode > M10`).
                let lpd1_probe = if refined
                    && lpd1_frame.is_some_and(|i| {
                        i.pic_lpd1_lvl != 0
                            || i.enc_mode > crate::port_enc_mode_config::enc_mode::M10
                    })
                    && matches!(sc_arm, crate::sc_detect::ScArm::Video { .. })
                {
                    Some(eval_pd0())
                } else {
                    None
                };
                // `lpd1_detector_post_pd0` on the probe's root — `None`
                // keeps the regular PD1 arm (the detector only demotes).
                let sb_lpd1 = lpd1_probe.as_ref().and_then(|e| {
                    lpd1_frame.as_ref().and_then(|i| {
                        resolve_sb_lpd1(
                            i,
                            &pd0_sb_in,
                            &pd0_det_frame,
                            e.root_det.as_ref(),
                            sb_pd0_det.map_or(0, |t| t.0),
                            sb_index,
                            u32::from(sb_qindex),
                            sb_md_full_lambda,
                            sb_disallow_below_64x64,
                        )
                    })
                });
                if sb_lpd1.is_some() {
                    refined = false;
                }
                if refined {
                    // M4/M5 (`dr_mode = 1`, PD0_DEPTH_ADAPTIVE):
                    // PD1 re-decides depths around the PD0 tree —
                    // depth_refine.rs. The refinement gates run on
                    // the PD0 PART_N costs; the walk evaluates the
                    // admitted depths through the leaf funnel and
                    // compares with real partition rates
                    // (bias 995). M6+ (PRED_PART_ONLY) keeps the
                    // fixed-tree path below (identical outcome:
                    // s = e = 0 everywhere).
                    // C's allintra depth-refinement level is sc_class5-
                    // aware (enc_mode_config.c:10067-10090): screen
                    // content at M0-M2 uses a lower/more-thorough level
                    // (1/1/5) that admits the depth descent the
                    // !sc_class5 level-6 row over-prunes.
                    // ONE predicate, used by BOTH the PD0 eval that
                    // builds the refinement scan and the PD1 walk that
                    // consumes it. They MUST agree: if PD0 injects an
                    // edge shape at a one-false node while the walk
                    // force-splits it, the walk descends into a scan
                    // node that has no children and panics.
                    // `svt_aom_get_nsq_geom_level_allintra` returns 0
                    // -- geometry DISABLED -- only for allintra
                    // enc_mode > M6 (enc_mode_config.c:8240). This
                    // branch is presets 0..=5, so geometry is always on
                    // and a one-false node keeps its injected edge
                    // shape.
                    //
                    // Do NOT reach for `NsqCfg::for_preset_qp(..).
                    // enabled` here. That is `set_nsq_search_ctrls`
                    // (:6496-6786) -- the SEARCH heuristics -- and it
                    // returns `off()` at p4/p5, which is a different
                    // statement from "no NSQ shapes exist". MEASURED
                    // 2026-08-04: wiring it in force-split every
                    // one-false node at p4/p5 and cost 29 cells --
                    // partial-SB p4 28/36 -> 12/36 and p5 25/36 ->
                    // 13/36. Search-off is not geometry-off.
                    // `eval_pd0` is this arm's eval — hoisted so the
                    // light-PD1 probe above and this walk share ONE
                    // PD0 run, the way C's `md_encode_block` runs
                    // `pick_partition_pd0` once before
                    // `lpd1_detector_post_pd0` reads its root.
                    let eval = match lpd1_probe {
                        Some(e) => e,
                        None => eval_pd0(),
                    };
                    // 8-BIT lambda even at bd10 — deliberate, not an
                    // oversight. C's `perform_pred_depth_refinement`
                    // (enc_dec_process.c:3017) runs INSIDE the window
                    // where `hbd_md` is forced to 0 (:2965, restored at
                    // :3023), so `is_parent_to_current_deviation_small`
                    // / `is_child_to_current_deviation_small` select
                    // `full_lambda_md[EB_8_BIT_MD]` /
                    // `full_sb_lambda_md[EB_8_BIT_MD]` at BOTH bit
                    // depths, over PD0 costs that are themselves
                    // bit-depth-identical. The bd10 lambda belongs to
                    // the PD1 WALK below, not to this scan.
                    // Whole-128-SB PD0 max/min fold (C
                    // `get_max_min_pd0_depths`). At SB128 (units.len() >
                    // 1) fold every coding-unit quadrant's PD0 eval;
                    // cached across the unit loop. `None` at SB64 → the
                    // scan derives max/min from `eval` alone, unchanged.
                    let sb_max_min = if units.len() > 1 {
                        if sb_pd0_max_min.is_none() {
                            let mut mx = 0usize;
                            let mut mn = 255usize;
                            for &(ux, uy) in units.iter() {
                                // Include partial edge units: C folds the whole
                                // superblock's chosen PD0 tree. The evaluator
                                // receives padded source planes and aligned
                                // frame bounds, so its partial-node handling
                                // applies here exactly as in the main walk.
                                crate::pd0::pd0_pick_sb_partition_m6_eval(
                                    sb_input,
                                    in_stride,
                                    ux,
                                    uy,
                                    cli_qp as u32,
                                    sb_qindex,
                                    // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                                    // ladder on `picture_qp` + the extended-CRF bump.
                                    crate::pd0::frame_lambda_weight_for_preset(
                                        speed_config.preset,
                                        u32::from(picture_qp),
                                        false,
                                        lw_bump,
                                    ),
                                    tables,
                                    if dr.disallow_4x4 { 8 } else { 4 },
                                    if coded_lossless {
                                        1
                                    } else {
                                        pd0_refined_rate_lvl
                                    },
                                    if coded_lossless {
                                        crate::pd0::Pd0Mode::Lvl1
                                    } else {
                                        pd0_refined_mode
                                    },
                                    if coded_lossless {
                                        1000
                                    } else {
                                        pd0_refined_eexit_th
                                    },
                                    // Same cap predicate as the
                                    // sibling call above; this
                                    // SB128 unit loop skips
                                    // incomplete units outright.
                                    crate::part_arm::max_block_cap_active(
                                        sc_arm,
                                        speed_config.preset,
                                        true,
                                    ),
                                    nsq_geom_enabled,
                                    w,
                                    h,
                                    tile_sb_row_start * sb_size,
                                    tile_sb_col_start * sb_size,
                                    // Superres chunk B.4: this SB's STALE full-res variance entry.
                                    sb_stale_vars,
                                    // C `static_config.max_tx_size`.
                                    search_max_sq,
                                    // C `pd0_use_src_samples` (video arm: recon).
                                    pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                    // Same frame and same arm as
                                    // the sibling eval this fold
                                    // summarises — a fold taken
                                    // from a DIFFERENT PD0 model
                                    // than the scan it caps would
                                    // be worse than no fold.
                                    pd0_inter.as_ref(),
                                    sb_pd0_det.map(|t| t.3),
                                    // C `full_sb_lambda_md`
                                    // (svt_aom_full_cost_pd0's lambda).
                                    Some(sb_md_full_lambda),
                                    ac_bias_eff,
                                )
                                .max_min_picked(&mut mx, &mut mn);
                            }
                            sb_pd0_max_min = Some((mx, mn));
                        }
                        sb_pd0_max_min
                    } else {
                        None
                    };
                    // This superblock's `DepthRemovalResult` —
                    // the same per-SB record `sb_pd0_det` reads
                    // for `Pd0SigDerivInput`. `None` on a key
                    // frame, where `set_depth_removal_level_controls`
                    // leaves `enabled = 0`.
                    let sb_dr_res = pd0_dr_res.and_then(|v| v.get(sb_index).copied());
                    let scan = crate::depth_refine::build_refined_scan_at(
                        &eval,
                        &dr,
                        sb_md_full_lambda,
                        tables,
                        x0,
                        y0,
                        sb_max_min,
                        // C `static_config.max_tx_size` -- the same
                        // value already threaded into the PD0 entries
                        // just above, and the reason `max_sq_size` is
                        // not always 64 (enc_dec_process.c:1814-1817).
                        search_max_sq,
                        sb_size,
                        // `use_ref_info` (:1606-1631): LAST ref's
                        // sb_min/max_sq_size at THIS superblock.
                        ref_min_max_sq.and_then(|(mn, mx)| {
                            mn.get(sb_index).copied().zip(mx.get(sb_index).copied())
                        }),
                        // `coeff_lvl_modulation`
                        // (:1865-1870): `pcs->slice_type ==
                        // I_SLICE` covers allintra and the video
                        // key frame alike.
                        matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                            || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
                        c_quant
                            .as_ref()
                            .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
                        // C `ctx->disallow_4x4` as
                        // `set_depth_removal_level_controls`
                        // left it — pic value possibly SET per
                        // SB. Same resolution `Pd0SigDerivInput`
                        // uses above.
                        sb_dr_res.map_or_else(
                            || crate::part_arm::disallow_4x4(sc_arm, speed_config.preset),
                            |r| r.disallow_4x4,
                        ),
                        // C `ctx->disallow_8x8`
                        // (`get_disallow_8x8_default`) — false on
                        // every reachable arm.
                        crate::port_enc_mode_config::leaf::get_disallow_8x8_default(),
                        // C `ctx->depth_removal_ctrls` — the
                        // per-SB `disallow_below_*` ladder
                        // `set_start_end_depth` clamps against
                        // (:1799-1813). Zeroed on a key frame.
                        sb_dr_res.map_or_else(Default::default, |r| r.ctrls),
                    );
                    // Partition rates at the real contexts, from
                    // the same (possibly chained) frame context as
                    // the funnel's syntax rates. When the per-SB
                    // chain is OFF (`cdf_ctrl.enabled == 0` —
                    // inter frames at every reachable preset:
                    // `get_update_cdf_level_default` returns 0 for
                    // `!is_islice` above M3), C's
                    // `init_frame_rate_tables` builds the table
                    // ONCE from `pcs->md_frame_context` — the
                    // primary ref's saved end-of-frame CDFs —
                    // which is `md_frame_cdfs`, NOT a fresh
                    // default. The `new_default` fallback here
                    // under-priced PARTITION_NONE on adapted rows
                    // and suppressed a split the C reference took
                    // (johnny frame 2, 32x32 at mi (56,56)).
                    let part_rates_default_fc;
                    let part_rates = match &chain_base {
                        Some((fc, _)) => crate::depth_refine::PartRates::from_fc(fc),
                        None => {
                            let fc = match md_frame_cdfs {
                                Some(c) => &c.fc,
                                None => {
                                    part_rates_default_fc =
                                        crate::entropy::context::FrameContext::new_default();
                                    &part_rates_default_fc
                                }
                            };
                            crate::depth_refine::PartRates::from_fc(fc)
                        }
                    };
                    #[cfg(feature = "std")]
                    if crate::dbgenv::nsqdbg() {
                        eprintln!(
                            "NSQDBG PARTRATE sb=({},{}) chained={} mdfc={}",
                            sb_y0 / 64,
                            sb_x0 / 64,
                            chain_base.is_some(),
                            md_frame_cdfs.is_some(),
                        );
                    }
                    let (u_src, v_src) = chroma_src.unwrap();
                    let mut inter_sq_me = crate::inter_search_arm::SqMeState::default();
                    let mut fx = crate::leaf_funnel::FunnelCtx {
                        u_src,
                        v_src,
                        src10: funnel_src10,
                        u_recon: fun_u_recon,
                        v_recon: fun_v_recon,
                        c_stride: cwid,
                        ectx: fun_ectx.as_mut().unwrap(),
                        rates: fun_rates.as_deref().unwrap(),
                        frame: fun_frame.as_ref().unwrap(),
                        frame_ssim: None,
                        // bd10 PART axis (task #94): the PD1
                        // depth-refine + NSQ walk compares LEAF block
                        // costs, and C's PD1 runs at `hbd_md = 2` (true
                        // 10-bit) — `test_depth` /
                        // `test_split_partition` sum
                        // `block_data[shape][nsi]->cost` from an MDS3
                        // that predicted, quantized and measured
                        // distortion at 10 bits. Running that walk on
                        // 8-bit leaf costs picked C's *bd8* shape. The
                        // same `full_rd10` chain that closed p7/p8
                        // (MODE axis) now feeds this walk. bd8 and
                        // every out-of-envelope bd10 frame keep `None`
                        // / `false` → byte-IDENTICAL.
                        y_recon10: if bd10_plumb {
                            Some(tile_frame_recon10)
                        } else {
                            None
                        },
                        u_recon10: if bd10_full_rd || bd10_mds3_bump {
                            Some(tile_frame_u_recon10)
                        } else {
                            None
                        },
                        v_recon10: if bd10_full_rd || bd10_mds3_bump {
                            Some(tile_frame_v_recon10)
                        } else {
                            None
                        },
                        // IBC chunk 8: frame IntraBC state + the MD mi grid.
                        ibc: ibc_state.as_deref(),
                        inter: inter_md,
                        // C `ctx->sq_sb_me_mv` +
                        // `pc_tree->tested_blk[PART_N][0]` — one
                        // slot per superblock walk, written by a
                        // square block's MD motion search and read
                        // by the NSQ shapes at the same node.
                        inter_sq_me: inter_md.map(|_| &mut inter_sq_me),
                        ibc_mvp: if ibc_state.is_some() || inter_md.is_some() {
                            Some(ibc_mvp_grid)
                        } else {
                            None
                        },
                        ibc_gate: Default::default(),
                        full_rd10: bd10_full_rd,
                        // Light-PD1 never fires on the refinement
                        // arm — `pic_lpd1_lvl` is 0 through M6,
                        // and `dr.adaptive`/NSQ only run there.
                        lpd1: None,
                        ii_preds: None,
                    };
                    // C `set_nsq_search_ctrls`'s `me_dist_mod`
                    // per-SB inputs (enc_mode_config.c:4956-4984):
                    // the +1 level bump is gated on THIS
                    // superblock's `me_8x8_distortion` /
                    // `me_8x8_cost_variance` — `ppcs->me_8x8_*
                    // [sb_index]` at `super_block_size == 64`, the
                    // `get_sb128_me_data` quadrant aggregate at 128
                    // (:62-114: dist averaged over the in-bounds
                    // 64x64 cells, variance maxed). `None` on a
                    // key frame (`slice_type == I_SLICE` ->
                    // me_dist_mod 0) and at ENC_MR
                    // (`enc_mode <= ENC_MR` -> 0).
                    let nsq_me_stats = inter_md
                        .filter(|_| {
                            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset)
                                > crate::port_enc_mode_config::enc_mode::MR
                        })
                        .and_then(|f| {
                            if sb_size == 64 {
                                f.me.per_b64
                                    .get(sb_index)
                                    .map(|o| (o.me_8x8_distortion, o.me_8x8_cost_variance))
                            } else {
                                let (bx, by) = (x0 / 64, y0 / 64);
                                let (mut d8, mut var, mut n) = (0u64, 0u32, 0u64);
                                for dy in 0..2usize {
                                    for dx in 0..2usize {
                                        if let Some(o) =
                                            f.me.per_b64.get((by + dy) * f.me.b64_cols + bx + dx)
                                        {
                                            d8 += u64::from(o.me_8x8_distortion);
                                            var = var.max(o.me_8x8_cost_variance);
                                            n += 1;
                                        }
                                    }
                                }
                                (n > 0).then_some(((d8 / n) as u32, var))
                            }
                        });
                    let nsq = if coded_lossless {
                        crate::depth_refine::NsqCfg::off()
                    } else {
                        crate::depth_refine::NsqCfg::for_arm_sb(
                            sc_arm,
                            speed_config.preset,
                            cli_qp as u32,
                            c_quant
                                .as_ref()
                                .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
                            temporal_layer,
                            sb_size,
                            nsq_me_stats,
                        )
                    };
                    crate::depth_refine::decide_sb_refined(
                        &scan,
                        &mut fx,
                        sb_input,
                        in_stride,
                        tile_frame_recon,
                        w,
                        // PD1 partition-rate lambda. C `test_depth` /
                        // `test_split_partition` /
                        // `update_skip_nsq_based_on_split_rate` /
                        // `update_skip_nsq_based_on_sq_recon_dist` all
                        // select `full_sb_lambda_md[EB_10_BIT_MD]` (==
                        // `full_lambda_md[EB_10_BIT_MD]`,
                        // md_process.c:763-764) when `hbd_md != 0`
                        // (product_coding_loop.c:9725, 9859, 10782,
                        // 10887). It MUST move with the leaf costs: the
                        // gates are ratio compares between an
                        // RDCOST(λ, part_rate, 0) term and a block cost.
                        // NOTE the refinement SCAN above deliberately
                        // keeps the 8-bit lambda — see
                        // `build_refined_scan_at`'s call site.
                        if bd10_full_rd {
                            u64::from(crate::pd0::kf_full_lambda_bd10(
                                base_qindex,
                                cli_qp as u32,
                                speed_config.preset,
                            ))
                        } else {
                            sb_md_full_lambda
                        },
                        &part_rates,
                        &nsq,
                        dr.disallow_4x4,
                        x0,
                        y0,
                        // ALIGNED extent for the spec-5.11.4 edge
                        // predicate. Dead on a 64-aligned frame.
                        w,
                        h,
                        // Whether NSQ geometry exists at this preset,
                        // which decides what a ONE-FALSE boundary node
                        // does: inject the single edge shape (H/V), or
                        // force-split like a both-false node.
                        //
                        // NOT hardcoded true. `NsqCfg::for_preset_qp`
                        // returns `off()` for presets 4 and 5 (its base
                        // table is nonzero only for 0..=3), and
                        // `shapes_for_size` already treats `!enabled` as
                        // square-only -- so at p4/p5 there is no legal
                        // edge shape to inject and C force-splits.
                        //
                        // MEASURED: with `true` here, p5 coded a 16x8 at
                        // (16,80) on `gradient 72x88 q20` where C splits
                        // to two 8x8s -- the ONLY structural difference
                        // between the port's tree and C's on that frame.
                        nsq_geom_enabled,
                    )
                } else {
                    // Same computation as pd0_pick_sb_partition_m6
                    // (that fn is exactly _eval(min_sq=8).tree()),
                    // via the eval form so the per-node PD0 costs
                    // are dumpable (SVTAV1_PD0DBG + SVTAV1_DBG_MI)
                    // for depth-flip drills at M6-M8 — the C
                    // counterpart is the PICKPART wrap, which fires
                    // at every preset. `lpd1_probe` is the SAME
                    // video-arm eval, already run above to feed the
                    // detector — reuse it rather than evaluate PD0
                    // twice (C runs PD0 once; its result feeds both
                    // `lpd1_detector_post_pd0` and this walk).
                    let eval = if let Some(e) = lpd1_probe {
                        e
                    } else {
                        if matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }) {
                            // The VIDEO arm's own PD0 entry point, the
                            // one the `preset >= 9` branch above already
                            // uses (docs/INTER-ENCODE-PLAN.md §1z⁶).
                            // This branch is C's `pic_pred_depth_only`
                            // case, which on the video arm starts at M8,
                            // and `set_pic_pd0_lvl_default`'s M8 row is
                            // `3 + ldp0_lvl_offset[qp_band]` — level 5 at
                            // CLI qp <= 27, 4 at 40..=43, 3 above.
                            // `refined_pd0_model` carries only 3 and 4
                            // and documents that it falls back to LVL_1
                            // otherwise, so a q20 M8 video KEY frame ran
                            // PD0_LVL_1's block cost against C's
                            // PD0_LVL_5. `pd0_pick_sb_partition_video`
                            // has `Pd0Mode::Lvl5` and the `ires_factor`
                            // its closed form needs, which
                            // `pd0_pick_sb_partition_m6_eval` hardcodes
                            // to 0.
                            //
                            // Byte-inert on the ALLINTRA arm by
                            // construction (this is an arm match), and
                            // equivalent at levels 3 and 4: with
                            // `cap_max_block` false — which
                            // `max_block_cap_active` always is on the
                            // video arm — the two entry points build the
                            // same `Pd0Ctx`, and `ires_factor` is read
                            // only by LVL_5's closed form.
                            // `sb_pd0_det` resolved the level +
                            // rate level per superblock at the top
                            // of the loop (`ScArm::Video` guard).
                            let (
                                pic_pd0_lvl,
                                pd0_coeff_rate_est_lvl,
                                accurate_part_ctx,
                                sb_subres_step,
                                sb_parent_cost_bias,
                            ) = sb_pd0_det.unwrap_or_else(|| {
                                unreachable!("ScArm::Video always builds sb_pd0_det")
                            });
                            crate::pd0::pd0_pick_sb_partition_video_eval(
                                sb_input,
                                in_stride,
                                x0,
                                y0,
                                cli_qp as u32,
                                sb_qindex,
                                crate::pd0::frame_lambda_weight_for_preset(
                                    speed_config.preset,
                                    u32::from(picture_qp),
                                    tune_iq,
                                    lw_bump,
                                ),
                                tables,
                                pic_pd0_lvl,
                                pd0_coeff_rate_est_lvl,
                                accurate_part_ctx,
                                crate::part_arm::nsq_geom_enabled(sc_arm, speed_config.preset),
                                // C `pd0_level <= PD0_LVL_1 ||
                                // ctx->pic_pred_depth_only` — this branch
                                // IS the pred-depth-only one.
                                true,
                                crate::pd0::input_resolution_factor(w * h),
                                w,
                                h,
                                tile_sb_row_start * sb_size,
                                tile_sb_col_start * sb_size,
                                sb_stale_vars,
                                max_tx_size,
                                // Same `disallow_4x4 ? 8 : 4`
                                // fold — 4 at video M0..M2.
                                if pd0_disallow_4x4 { 8 } else { 4 },
                                pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                crate::dbgenv::pd0_nosplit() && inter_md.is_some(),
                                pd0_inter.as_ref(),
                                Some(sb_subres_step),
                                sb_parent_cost_bias,
                                // C `full_sb_lambda_md[EB_8_BIT_MD]`
                                // (svt_aom_full_cost_pd0's lambda).
                                Some(sb_md_full_lambda),
                                ac_bias_eff,
                            )
                        } else {
                            crate::pd0::pd0_pick_sb_partition_m6_eval(
                                sb_input,
                                in_stride,
                                x0,
                                y0,
                                cli_qp as u32,
                                sb_qindex,
                                // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                                // ladder on `picture_qp` + the extended-CRF bump.
                                crate::pd0::frame_lambda_weight_for_preset(
                                    speed_config.preset,
                                    u32::from(picture_qp),
                                    tune_iq,
                                    lw_bump,
                                ),
                                tables,
                                8,
                                // M6: coeff_rate_est_lvl 1 (real PD0 coeff
                                // rate, unchanged). M7/M8: 2 -> the C
                                // perform_tx_pd0 `eob<th ? 6000+eob*500`
                                // approximation that lowers the parent-NONE
                                // cost and matches C's partition depth.
                                pd0_refined_rate_lvl,
                                pd0_refined_mode,
                                pd0_refined_eexit_th,
                                // The max-block variance cap, per ARM.
                                // Allintra (`get_max_block_size_allintra`,
                                // enc_mode_config.c:7042): fires at M8+
                                // only, and stays at sb_size for
                                // incomplete edge SBs. VIDEO
                                // (`get_max_block_size_default`, :6991):
                                // no cap at any preset.
                                crate::part_arm::max_block_cap_active(
                                    sc_arm,
                                    speed_config.preset,
                                    x0 + 64 <= w && y0 + 64 <= h,
                                ),
                                // NSQ geometry, per ARM. Allintra
                                // (`svt_aom_get_nsq_geom_level_allintra`,
                                // enc_mode_config.c:8240): presets 0..=6 →
                                // level 1/2/3 → enabled, presets 7+ → level 0
                                // → disabled; when disabled a one-false
                                // boundary node force-splits (no edge shape)
                                // — the presets 7/8 partial-SB fix. VIDEO
                                // (`svt_aom_get_nsq_geom_level_default`,
                                // :8216) never returns 0, so geometry stays
                                // on at every preset there.
                                crate::part_arm::nsq_geom_enabled(sc_arm, speed_config.preset),
                                // ALIGNED dims — the spec-5.11.4 edge grid.
                                w,
                                h,
                                // This tile's pixel origin: the M6 PD0 leaf-cost
                                // DC prediction must not read across a tile
                                // boundary (C up/left_available respect tiles).
                                // 0 for a single-tile frame → byte-inert.
                                tile_sb_row_start * sb_size,
                                tile_sb_col_start * sb_size,
                                // Superres chunk B.4: this SB's STALE full-res variance entry.
                                sb_stale_vars,
                                // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                                max_tx_size,
                                // C `pd0_use_src_samples` (video arm: recon).
                                pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                // The ALLINTRA arm of the
                                // non-refined branch: its VIDEO
                                // sibling above is
                                // `pd0_pick_sb_partition_video_eval`
                                // and already carries the inter
                                // context. `pd0_inter` is `None`
                                // on every frame that reaches
                                // here, so this is the same value
                                // written explicitly rather than
                                // a second `None` that hides an
                                // arm decision.
                                pd0_inter.as_ref(),
                                // Allintra keeps the level
                                // default — `sb_pd0_det` is None
                                // on this arm by construction.
                                None,
                                // C `full_sb_lambda_md[EB_8_BIT_MD]`
                                // (svt_aom_full_cost_pd0's lambda).
                                Some(sb_md_full_lambda),
                                ac_bias_eff,
                            )
                        }
                    };
                    // C `md_encode_block`'s `lpd1` per-SB
                    // dispatch off the PD0 root result — the
                    // light path uses the SAME fixed tree below,
                    // so `pd1_level` only switches the leaf
                    // evaluator. `None` off the video arm
                    // (`md_config_signals` is absent on a key
                    // frame) and on every regular-PD1 SB. The
                    // probe-resolved value wins when it exists —
                    // `refined` was cleared exactly because it is
                    // `Some`.
                    let sb_lpd1 = sb_lpd1.or_else(|| {
                        lpd1_frame.as_ref().and_then(|i| {
                            resolve_sb_lpd1(
                                i,
                                &pd0_sb_in,
                                &pd0_det_frame,
                                eval.root_det.as_ref(),
                                sb_pd0_det.map_or(0, |t| t.0),
                                sb_index,
                                u32::from(sb_qindex),
                                sb_md_full_lambda,
                                sb_disallow_below_64x64,
                            )
                        })
                    });
                    // Encode pass quantizes under the SAME per-SB
                    // `sig_deriv` arm — a light-PD1 row can turn
                    // RDOQ off where `pcs->rdoq_level` kept it.
                    if let Some(l) = sb_lpd1.as_ref() {
                        enc_rdoq_enabled = l.sig.rdoq.enabled;
                    }
                    #[cfg(feature = "std")]
                    if crate::dbgenv::pd0dbg() && crate::depth_refine::nsqdbg_here(x0, y0) {
                        fn walk(e: &crate::pd0::Pd0Eval, x: usize, y: usize) {
                            eprintln!(
                                "NSQDBG PD0 mi=({},{}) sq={} tested={} cost={} split={}",
                                y / 4,
                                x / 4,
                                e.sq,
                                e.tested,
                                e.cost,
                                e.split
                            );
                            if let Some(ch) = e.children.as_ref() {
                                let h = e.sq / 2;
                                walk(&ch[0], x, y);
                                walk(&ch[1], x + h, y);
                                walk(&ch[2], x, y + h);
                                walk(&ch[3], x + h, y + h);
                            }
                        }
                        walk(&eval, x0, y0);
                    }
                    let tree = eval.tree();
                    let sb_vars = sb_stale_vars.copied().unwrap_or_else(|| {
                        crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0)
                    });
                    let mut inter_sq_me = crate::inter_search_arm::SqMeState::default();
                    let mut funnel_ctx = if use_funnel {
                        let (u_src, v_src) = chroma_src.unwrap();
                        Some(crate::leaf_funnel::FunnelCtx {
                            u_src,
                            v_src,
                            src10: funnel_src10,
                            u_recon: fun_u_recon,
                            v_recon: fun_v_recon,
                            c_stride: cwid,
                            ectx: fun_ectx.as_mut().unwrap(),
                            rates: fun_rates.as_deref().unwrap(),
                            frame: fun_frame.as_ref().unwrap(),
                            frame_ssim: None,
                            y_recon10: if bd10_plumb {
                                Some(tile_frame_recon10)
                            } else {
                                None
                            },
                            u_recon10: if bd10_full_rd || bd10_mds3_bump {
                                Some(tile_frame_u_recon10)
                            } else {
                                None
                            },
                            v_recon10: if bd10_full_rd || bd10_mds3_bump {
                                Some(tile_frame_v_recon10)
                            } else {
                                None
                            },
                            // bd10 post-pass: IBC is bd8-only (the injection
                            // self-gates on bd10 too).
                            ibc: None,
                            // The INTER arm, which this site DROPPED
                            // (docs/INTER-ENCODE-PLAN.md §1z'''). This is
                            // the third of three `FunnelCtx`
                            // constructions and the only one that reached
                            // a leaf with `inter: None` while the frame
                            // had an inter arm: it is the PD0 FIXED-TREE
                            // path taken when `refined` is false, i.e.
                            // `DrCtrls::for_arm` reports a
                            // non-adaptive depth-refinement level. On the
                            // VIDEO arm that is exactly M8 and above, so
                            // every preset-8 inter frame in the campaign's
                            // grid decided its blocks from an intra-only
                            // candidate set and the injector never ran.
                            // MEASURED with `SVTAV1_CANDDBG=1 SVTAV1_NSQDBG=1`'s new
                            // `NSQDBG PINTER` line: 2 inter candidates at
                            // p6, ZERO at p8, on `gradient 64x64 q40`.
                            //
                            // Byte-inert on the still envelope BY
                            // CONSTRUCTION: `inter_md` is `None` on every
                            // key frame, so both fields keep the values
                            // they had. `ibc` deliberately stays `None` —
                            // wiring IBC here is a separate question with
                            // a separate byte risk, and this chunk does
                            // not measure it.
                            inter: inter_md,
                            // C `ctx->sq_sb_me_mv` +
                            // `pc_tree->tested_blk[PART_N][0]` — one
                            // slot per superblock walk, written by a
                            // square block's MD motion search and read
                            // by the NSQ shapes at the same node.
                            inter_sq_me: inter_md.map(|_| &mut inter_sq_me),
                            ibc_mvp: if inter_md.is_some() {
                                Some(ibc_mvp_grid)
                            } else {
                                None
                            },
                            ibc_gate: Default::default(),
                            full_rd10: bd10_full_rd,
                            // Light-PD1 leaf evaluator — `Some`
                            // only when `resolve_sb_lpd1` kept a
                            // `pd1_level > REGULAR_PD1` for this
                            // superblock.
                            lpd1: sb_lpd1,
                            ii_preds: None,
                        })
                    } else {
                        None
                    };
                    crate::partition::encode_fixed_tree(
                        sb_input,
                        in_stride,
                        tile_frame_recon,
                        w,
                        &tree,
                        unit_size,
                        sb_qindex,
                        part_config,
                        x0,
                        y0,
                        w,
                        h,
                        &sb_vars,
                        (x0, y0),
                        funnel_ctx.as_mut(),
                        ref_ctx.as_ref(),
                    )
                }
            }
        } else {
            crate::partition::partition_search_frame_edges(
                &encode_input[y0 * w + x0..],
                w,
                tile_frame_recon,
                w,
                unit_size,
                sb_qindex,
                sb_lambda,
                speed_config.max_partition_depth as u32,
                part_config,
                x0,
                y0,
                ref_ctx.as_ref(),
            )
        };
        unit_results.push(sb_result);
        unit_enc_rdoq.push(enc_rdoq_enabled);
    }
    // end per-b64 coding-unit loop

    // Merge the b64 units into this SUPERBLOCK's result. At SB64
    // there is exactly one unit and this is the identity (the
    // moved-out `PartitionResult`, byte-for-byte the old value).
    // At SB128 the four b64 quadrants become the children of a
    // PARTITION_SPLIT node rooted at the 128 square — which is
    // what C codes: an 8-symbol partition symbol at CDF row
    // bsl=4 (ctx 16..19), then the quadrants in Z-order.
    let sb_result = merge_sb_units(unit_results, sb_size, unit_size, ref_frame_data.is_none());
    // SB128 fold of the per-unit RDOQ enables: C resolves
    // `rdoq_ctrls` once per 128 SB while the funnel resolves lpd1
    // per 64 unit, so disagreeing quadrants are already an MD-side
    // approximation — `any` keeps RDOQ where any unit kept it.
    // Identity at SB64 (`units.len() == 1`).
    let sb_enc_rdoq = unit_enc_rdoq.iter().any(|&e| e);

    // Chain: evolve this SB's contexts by re-coding the decided
    // tree (throwaway arithmetic state; only the CDF updates
    // matter) and snapshot them for the following SBs.
    if funnel_chain {
        // The chain's SEED for the first SB is C's
        // `md_frame_context` (enc_dec_process.c:3002-3022's
        // "neither" arm), which on a frame naming a
        // `primary_ref_frame` is the RESTORED context, not the
        // defaults — the same rule `pd0_frame_tables` above
        // follows.
        let (mut fc, mut cfc) = chain_base.unwrap_or_else(|| match md_frame_cdfs {
            Some(prev) => (prev.fc.clone(), prev.coeff.clone()),
            None => (
                crate::entropy::context::FrameContext::new_default(),
                crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
            ),
        });
        // Issue #16: this is C's MD-side `ec_ctx_array[sb]`, not
        // the bitstream's context. C's encode pass adapts an
        // IntraBC txb's tx type on the INTRA DC row here
        // (`svt_av1_cost_coeffs_txb`'s `is_inter_mode(mode)`
        // ignores `use_intrabc`), so the per-SB MD rate tables
        // rebuilt from this chain must see that adaptation — the
        // same quirk `cost_coeffs_txb`'s `cost_dir` remap reads.
        // Sticky across snapshots (the struct is cloned).
        cfc.md_side_ibc_txt_update = true;
        cfc.md_side_lossless_txt_update = coded_lossless;
        // Same class, mode side: C's MD-side context skips the
        // palette CDF update for non-chroma-reference blocks
        // (`FrameContext::md_side_chroma_gated_palette`).
        fc.md_side_chroma_gated_palette = true;
        if let Some(tree) = sb_result.tree.as_ref() {
            let se = sim_ectx.as_mut().unwrap();
            if sb_row != *sim_prev_sb_row {
                se.reset_left_for_sb_row();
                *sim_prev_sb_row = sb_row;
            }
            let (u_src, v_src) = chroma_src.unwrap();
            // This writer re-codes ONE superblock purely to evolve
            // the chain's CDFs — its arithmetic output is thrown
            // away unread. `cdf_only` skips `encode_cdf_q15`/
            // `encode_bool_q15` entirely and applies only the
            // `update_cdf` side effect each symbol carries, the
            // same division C's `allow_update_cdf` estimate paths
            // (`svt_aom_txb_estimate_coeff_bits`,
            // `svt_aom_tx_size_bits` with `allow_update_cdf=1`)
            // draw: context mutation without a bit writer. CDF
            // states are identical to a full pass because
            // `update_cdf` is a pure function of (cdf, symbol).
            // Capacity 0 is valid — no byte is ever emitted.
            let mut sim_writer = crate::entropy::writer::AomWriter::new(0);
            sim_writer.cdf_only = true;
            let mut sim_chroma = Some(ChromaPass {
                u_src,
                v_src,
                u_recon: sim_u,
                v_recon: sim_v,
                stride: cwid,
                qindex_u,
                qindex_v,
                qm_u: qm_levels[1],
                qm_v: qm_levels[2],
                c_quant: None,
                ref_uv: ref_padded.and_then(|p| p.uv.as_ref()).map(|(u, v)| (u, v)),
                sb_size,
                frame_w: w,
                frame_h: h,
            });
            encode_partition_tree(
                tree,
                &mut sim_writer,
                &mut fc,
                &mut cfc,
                base_qindex,
                se,
                true,
                sb_x0,
                sb_y0,
                &mut sim_chroma,
                sim_geom,
                false,
            );
        }
        chain_snaps.push((fc, cfc));
        debug_assert_eq!(chain_snaps.len(), local_sb_index + 1);
    }
    (sb_result, sb_enc_rdoq)
}
