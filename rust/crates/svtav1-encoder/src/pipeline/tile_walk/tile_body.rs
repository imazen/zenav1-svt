use super::*;

#[inline(always)]
pub(super) fn encode_one_tile_body(
    encode_input: &[u8],
    sb_input: &[u8],
    in_stride: usize,
    w: usize,
    h: usize,
    sb_size: usize,
    sb_cols: usize,
    sb_rows: usize,
    tile_grid: crate::entropy::obu::TileGrid,
    base_qindex: u8,
    qindex_u: u8,
    qindex_v: u8,
    ac_bias_eff: f64,
    sb_qindex_plan: Option<&[u8]>,
    fh_delta_q_present: bool,
    tpl_rdmult: &Option<alloc::sync::Arc<crate::port_md_lambda::TplRdmult>>,
    chroma_ac_deltas: (i8, i8),
    sharp_tx_active: bool,
    hdr_noise_norm: u8,
    qm_levels: [u8; 3],
    hdr_tx_bias: u8,
    hdr_complex_hvs: bool,
    frame_sc: crate::sc_detect::ScDerivation,
    sc_arm: crate::sc_detect::ScArm,
    hdr_alt_ssim: bool,
    hdr_alt_lambda: bool,
    hdr_iq_lambda_weight: Option<u32>,
    ssim_rdmult: Option<&crate::tune::SsimRdmult>,
    fh_base_qindex: u8,
    walk_tx_mode_select: bool,
    cli_qp: u8,
    picture_qp: u8,
    lw_bump: u32,
    tune_iq: bool,
    hdr_sharpness: i8,
    speed_config: &SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    is_highest_layer: bool,
    temporal_layer: u8,
    ref_padded: Option<&crate::picture::PaddedRef>,
    inter_md: Option<&crate::inter_md_arm::InterMdFrame<'_>>,
    sim_inter_syntax: Option<&InterSyntaxState>,
    sim_inter_mvp_env: Option<&crate::partition::InterMdEnv>,
    pd0_min_sq: Option<&[u8]>,
    pd0_dr_res: Option<&[crate::port_enc_mode_config::common::DepthRemovalResult]>,
    ref_min_max_sq: Option<(&[u8], &[u8])>,
    sb_inter_lambda: Option<&[crate::pd0::SbInterLambda]>,
    md_frame_cdfs: Option<&crate::port_frame_cdf::FrameCdfs>,
    mv_map: &[svtav1_types::motion::Mv],
    mv_map_stride: usize,
    chroma_420: bool,
    chroma_format: svtav1_types::chroma::ChromaFormat,
    c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
    bit_depth: u8,
    hbd_used: &core::sync::atomic::AtomicBool,
    stale_vars: Option<&[crate::pd0::SbVariance]>,
    max_tx_size: u8,
    coded_lossless: bool,
    deep_search: bool,
    reference: crate::reference::SvtReference,
    pd0_det_frame: crate::part_arm::Pd0DetFrame<'_>,
    lpd1_frame: Option<Lpd1FrameIn>,
    stop: &dyn Stop,
    acw: usize,
    ach: usize,
    md_cw: usize,
    chroma_src: Option<(&[u8], &[u8])>,
    hbd_src: Option<(&[u16], &[u16], &[u16])>,
    tile_idx: usize,
) -> Result<
    (
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        Vec<bool>,
    ),
    whereat::prelude::At<EncodeError>,
> {
    let (tile_sb_row_start, tile_sb_row_end) = tile_grid.row_span(tile_idx / tile_grid.tile_cols);
    let (tile_sb_col_start, tile_sb_col_end) = tile_grid.col_span(tile_idx % tile_grid.tile_cols);
    let tile_sb_cols = tile_sb_col_end - tile_sb_col_start;
    // The tile's IN-FRAME luma pixel count, which is EXACTLY what the
    // per-SB `extend_from_slice` below appends in total (each superblock
    // contributes `sb_cur_h` rows of `sb_cur_w`, and the SB extents
    // partition the tile).
    //
    // Reserving it is not cosmetic. `Vec::new()` + `extend_from_slice`
    // doubles: a 4 MP tile reaches 4.19 MB through ~13 reallocations, each
    // copying everything written so far, and holds up to 2x the payload at
    // the moment it grows past the last power of two. `RawVecInner::
    // finish_grow` is 4.53 M over 43,630 calls in
    // `benchmarks/mem_heaptrack_2026-09-03.txt`, and 4.19 M of it is this
    // collect. Capacity does not affect contents, so this is byte-neutral
    // by construction.
    let tile_luma_px = ((tile_sb_row_end - tile_sb_row_start) * sb_size)
        .min(h.saturating_sub(tile_sb_row_start * sb_size))
        * (tile_sb_cols * sb_size).min(w.saturating_sub(tile_sb_col_start * sb_size));
    let mut tile_recon: Vec<u8> = svtav1_types::try_with_capacity!(tile_luma_px)?;
    // PD0_LVL_1 rate tables (presets 6..8), built once per tile on
    // first use — default CDFs at the frame qindex (C md_frame_context).
    let mut m6_pd0_tables: Option<crate::pd0::M6Pd0Tables> = None;
    // C `init_frame_rate_tables` (md_config_process.c:292-310) seeds
    // `md_frame_context` from the primary reference's SAVED end-of-frame
    // CDFs whenever the header names a `primary_ref_frame`, and only
    // otherwise from `svt_av1_default_coef_probs(base_q_idx)` +
    // `svt_aom_init_mode_probs`. PD0 prices against the SAME
    // `md_rate_est_ctx` the funnel does — `fun_rates` already reads
    // `md_frame_cdfs` for exactly this reason (§1s item 8) — so PD0's
    // tables come from the same place. On a KEY frame `md_frame_cdfs` is
    // `None` (C's `PRIMARY_REF_NONE` arm), which is the default pair the
    // still path has always built, so this is byte-inert there by
    // construction.
    //
    // MEASURED before the change (`diag 64x64 q40 p8 frames=2`, frame 1):
    // C's `SVT_PD0COST_OUT` reports `ybits=1519` for the 64x64 where the
    // port's `SVTAV1_PD0DBG` reported 2423, at the same
    // `coeff_rate_est_lvl` and the same eob — the whole gap was the
    // tables.
    let pd0_frame_tables = |qindex: u8| match md_frame_cdfs {
        Some(prev) => crate::pd0::build_m6_pd0_tables_from_ctx(&prev.fc, &prev.coeff),
        None => crate::pd0::build_m6_pd0_tables(qindex),
    };
    // M6 leaf funnel state (preset 6, 4:2:0 still): decision-phase
    // chroma recon planes + neighbor-context state + rate tables.
    // Single-SB frames use the default contexts (C md_frame_context);
    // multi-SB frames currently reuse them for every SB — C chains
    // per-SB contexts (ec_ctx_array averaging), a documented residual
    // gap for the 128-cell decisions.
    // The C-exact leaf intra funnel covers still/420 allintra presets
    // 2, 3, 4, 5, 6, 7, 8, and eff-M9 (presets >= 9 clamp to M9).
    // On the ALLINTRA arm presets 2/3 use update_cdf_level 1 and 4..=6
    // level 2 — for I-slices the two are identical (only update_mv
    // differs, forced 0 on I-slices; set_cdf_controls,
    // enc_mode_config.c:8495) — and 7/8/9+ use update_cdf_level 0
    // (static default tables all frame). The VIDEO arm keeps level 1 up
    // to M8, so the chain gate is arm-dependent and is now derived by
    // `rate_arm::update_cdf_level` rather than spelled as a preset range
    // (see the `funnel_chain` binding below and docs/rate-arm-port-map.md).
    // eff-M9 (intra_level 8) arms the is_dc_only gate inside the funnel.
    let use_funnel = chroma_420 && chroma_src.is_some() && c_quant.is_some();
    // The detector has already run on this immutable source for the frame.
    // Copy its small result instead of scanning the full picture per tile.
    let tile_sc = frame_sc;
    // ZenEnhancement::DeepSearch (allintra arm only): the base bake,
    // the six search-effort applies below (`intra_arm` … `mds0_arm`)
    // and the depth-refinement ladder read `md_preset`/`md_eff_mode`
    // — `enc_mode -1` (MR, the deepest C rows) at any caller preset.
    // Lambda, quantizer, rate-estimation and frame-header derivations
    // keep `speed_config.preset` — only WHAT IS SEARCHED changes.
    // At preset -1 `md_preset` is -1 either way (byte-inert), and
    // `deep_search` is never set on a video frame.
    let md_preset = if deep_search && matches!(sc_arm, crate::sc_detect::ScArm::Allintra) {
        -1
    } else {
        speed_config.preset
    };
    let md_eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, md_preset);
    let mut funnel_cfg = crate::leaf_funnel::FunnelCfg::for_preset(md_preset);
    // `pcs->rate_est_level` -> `set_rate_est_ctrls` (enc_mode_config.c:6428),
    // for THIS arm. `for_preset` bakes the allintra ladder (1 through M6,
    // 4 at M7/M8, 0 above); the video arm assigns a flat 1 at every preset
    // (:8942), so a video KEY frame at M7/M8 keeps the real neighbour
    // contexts and the real luma coeff rate where the still arm switches
    // to the fast approximation. Byte-neutral on the still path by
    // construction — `rate_arm::allintra_flattening_matches_the_ladder`
    // pins the pair against `for_preset`'s baked values at every preset.
    let (rate_est_coeff_lvl, rate_est_real_ctx) =
        crate::rate_arm::rate_est_ctrls(crate::rate_arm::rate_est_level(sc_arm, md_eff_mode));
    funnel_cfg.coeff_rate_est_lvl = rate_est_coeff_lvl;
    funnel_cfg.real_coeff_ctx = rate_est_real_ctx;
    // `pcs->pic_filter_intra_level` -> `set_filter_intra_ctrls` and
    // `(intra_level, dist_based_ang_intra_level)` -> `set_intra_ctrls`,
    // for THIS arm (`crate::intra_arm`). `for_preset` bakes the ALLINTRA
    // rows; the video arm drops filter-intra entirely at M6+ and takes a
    // lower intra_level (M6 video = intra_level 2 = the still path's M5
    // candidate shape). Byte-neutral on the still path by construction —
    // `intra_arm::allintra_flattening_matches_the_ladder` pins the six
    // stamped fields against `for_preset`'s baked values at every preset.
    //
    // `is_base` is `temporal_layer_index == 0`
    // (enc_mode_config.c:8904): true on every picture of a flat GOP, and
    // on the base-layer pictures of a hierarchical one — the ladder's
    // `is_base` rows (e.g. `intra_level` 1-vs-2/6 at M0..M7) must see a
    // non-base layer as false.
    //
    // `is_islice` is NOT constant any more, and that is what makes
    // `dist_based_ang_intra_level` non-zero: the ladder's rows read
    // `is_islice ? 0 : 2` from M3 up, so every INTER frame from preset 3
    // on carries level 2 and the `skip_angular_delta*_th` triple
    // `intra_arm::apply` now stamps. It used to `debug_assert` that the
    // level was 0, which was true only because no inter frame could reach
    // this code.
    crate::intra_arm::apply(
        &mut funnel_cfg,
        sc_arm,
        md_eff_mode,
        matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
            || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
        temporal_layer == 0,
    );
    // `cand_reduction_ctrls.reduce_filter_intra`, the OTHER thing that
    // splits MDS0 into two iterations. Read from the picture's own
    // `cand_reduction` rather than re-derived, so this cannot become a
    // second transcription of `set_cand_reduction_ctrls`. `None` (a still,
    // or a key frame with no inter env) keeps C's I-slice value: C assigns
    // `cand_reduction_level = 0` unconditionally on an I_SLICE
    // (enc_mode_config.c:9040 video, :9623 allintra), and level 0 is the
    // only one with `reduce_filter_intra == 0`.
    funnel_cfg.reduce_filter_intra =
        inter_md.is_some_and(|m| m.cand_reduction.reduce_filter_intra != 0);
    // `pcs->txs_level` -> `set_txs_controls`, for THIS arm
    // (`crate::txs_arm`). The arms agree at M4..M7 and diverge at M8/M9,
    // where the video ladder keeps the tx-size search on (level 3 / 4)
    // and the allintra one turns it off at the picture level.
    crate::txs_arm::apply(
        &mut funnel_cfg,
        sc_arm,
        md_eff_mode,
        temporal_layer == 0,
        u32::from(cli_qp),
    );
    // `pcs->txt_level` -> `svt_aom_set_txt_controls` and `pcs->cfl_level`
    // -> `set_cfl_ctrls`, for THIS arm (`crate::funnel_arm`).
    crate::funnel_arm::apply(
        &mut funnel_cfg,
        sc_arm,
        md_eff_mode,
        matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
            || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
        temporal_layer == 0,
    );
    // `pcs->nic_level` -> `svt_aom_set_nic_controls`, for THIS arm
    // (`crate::nic_arm`). At M6 the video arm is level 8 against the still
    // arm's 6: stage counts {2,1,1} instead of {6,6,6} and candidate
    // thresholds 300/3/3 instead of 1200/15/15. Byte-neutral on the still
    // path except for one baked-table correction the pin names
    // (`nic_arm::allintra_flattening_matches_the_ladder`).
    crate::nic_arm::apply(&mut funnel_cfg, sc_arm, md_eff_mode, temporal_layer == 0);
    // `uv_mode_nfl_count`'s base (product_coding_loop.c:7693-7696), for
    // THIS arm and picture: 32 on an allintra still, 64 on a video KEY
    // frame, 32 / 16 on a non-highest / highest-layer inter picture.
    // `for_preset` bakes the still's 32; a video key frame at M0/M1 was
    // running half of C's independent-uv full loop until 2026-09-04.
    funnel_cfg.ind_uv_nfl_base = crate::intra_arm::ind_uv_nfl_base(sc_arm, is_highest_layer);
    // `ctx->mds0_use_hadamard_sb` and `ctx->skip_sub_depth_ctrls`, for
    // THIS arm (`crate::encdec_arm`). Neither comes from
    // `sig_deriv_mode_decision_config` — the first is a literal in each
    // `svt_aom_sig_deriv_enc_dec_*` body and the second a per-arm
    // `skip_sub_depth_lvl` ladder there — which is why §1c's
    // field-for-field divergence table cannot see them. The video
    // arm's `false` sends MDS0's luma distortion down C's two-buffer
    // VARIANCE arm instead of the Hadamard SATD, and variance is
    // DC-invariant where SATD is not; the video arm's `enc_mode <= M1`
    // skip_sub ladder puts every video key frame at M2+ on level 2
    // (`coeff_perc` 25 vs the still's 15), the gate that decides
    // whether a flat <=16x16 node's split is even tested.
    crate::encdec_arm::apply(
        &mut funnel_cfg,
        sc_arm,
        md_eff_mode,
        temporal_layer == 0,
        pd0_det_frame.is_not_last_layer,
        bit_depth,
    );
    // `pcs->mds0_level` -> `set_mds0_controls`, for THIS arm
    // (`crate::mds0_arm`). The arms agree on a key frame through M10 and
    // diverge above it: the video arm takes level 2 (`fast_loop_core`'s
    // global dist-to-cost prune, product_coding_loop.c:1325) where the
    // allintra arm is a literal 0 at every preset. Byte-neutral on the
    // still path by construction.
    crate::mds0_arm::apply(
        &mut funnel_cfg,
        sc_arm,
        md_eff_mode,
        temporal_layer == 0,
        matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
            || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
    );
    // C `ctx->pic_pred_depth_only` = `depth_refinement_ctrls.mode ==
    // PD0_DEPTH_PRED_PART_ONLY`, which only level 10 sets — frame-level,
    // read by the per-SB `refined_pd0_model` below (it feeds
    // `set_depth_early_exit_ctrls`, enc_mode_config.c:7229-7233).
    let pd0_pred_depth_only = crate::depth_refine::DrCtrls::for_arm(
        sc_arm,
        md_preset,
        tile_sc.classes.sc_class5,
        u32::from(cli_qp),
        c_quant
            .as_ref()
            .map_or(crate::quant::CoeffLvl::Normal, |q| q.input_coeff_level),
    )
    .pred_depth_only;
    // C `pcs->pic_pd0_lvl` -> `set_pd0_ctrls` -> `pd0_detector` is resolved
    // PER SUPERBLOCK — `pd0_detector` walks `md_ctx->pd0_ctrls.pd0_level`
    // down per SB from the reference's `sb_intra` and this frame's own
    // ME statistics (enc_dec_process.c:2406), so the level the refinement
    // path's PD0 model sees is a per-SB value, not a picture one. Both are
    // computed at the top of the SB loop below (`sb_pd0_det` /
    // `sb_refined_pd0`), because the inputs need `sb_index`.
    // C `ctx->pd0_use_src_samples = allintra || pcs->hbd_md`
    // (enc_mode_config.c:7309). FALSE on every video frame, so the video
    // arm's PD0 predicts from the recon it generates per block instead of
    // from the source — see `crate::pd0::Pd0ReconCanvas`. bd10 sets
    // `hbd_md`, which puts C back on the source samples, so the port keeps
    // the source path there too.
    let pd0_video_recon =
        matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }) && bit_depth != 10;
    // C `product_prediction_fun_table_pd0[1]` — PD0's INTER arm, live on
    // exactly the frames that have a reference. `inter_md.is_some()` IS
    // "this frame is a non-I slice with a DPB reference" (the same
    // predicate `inter_md.is_some()` keys on), so a key frame and every
    // allintra cell get `None` and are byte-neutral by construction.
    let pd0_inter_base = inter_md.map(|f| crate::pd0::Pd0InterRef {
        padded_y: &f.padded.y,
        padded_by_ref: f.padded_by_ref,
        ref_mode_not_single: f.reference_mode_is_select,
        me: f.me,
        sb_size: f.sb_size,
        frame_w: f.frame_w,
        frame_h: f.frame_h,
        base_update_type: f.base_update_type,
        factor_update_type: f.factor_update_type,
        alt_lambda_factors: f.alt_lambda_factors,
        lambda_mod_intra: f.lambda_mod_intra,
        // Overwritten per superblock below; 8 is C's `disallow_4x4 ? 8`
        // arm, i.e. the value with depth removal OFF.
        min_sq: 8,
        // Overwritten per superblock below.
        me_qdiff: 0,
    });
    if coded_lossless {
        funnel_cfg.apply_coded_lossless();
    }
    funnel_cfg.allow_sct = tile_sc.allow_screen_content_tools;
    // THE palette flip-on: with the level stamped, the funnel injects
    // palette candidates (chunk 4) and the pack codes the winners
    // (chunk 5). sc_derivation.palette_level is 0 on every non-sc
    // frame, so non-screen-content streams are untouched.
    funnel_cfg.palette_level = tile_sc.palette_level;
    // SVTAV1_SC_TOOLS (DIAGNOSTIC ONLY, absent = unchanged): force one of
    // the two screen-content tools off to localize a screen divergence to
    // palette or IntraBC without editing and rebuilding.
    //
    //   SVTAV1_SC_TOOLS=nopalette   palette_level = 0
    //   SVTAV1_SC_TOOLS=noibc       allow_intrabc = false
    //   SVTAV1_SC_TOOLS=none        both off
    //
    // These deliberately do NOT touch `allow_screen_content_tools` (the
    // frame-header bit), so the streams stay comparable: only the RD
    // candidate set changes. A bisect that also flipped the header would
    // move the syntax and prove nothing about which tool caused a flip.
    //
    // Exists because localizing the graph.png preset-0..4 divergence
    // otherwise meant hand-editing this line and rebuilding, once per
    // hypothesis -- and an edit-to-measure loop is where measurements stop
    // getting made.
    match crate::dbgenv::sc_tools() {
        Some("nopalette") => funnel_cfg.palette_level = 0,
        Some("none") => funnel_cfg.palette_level = 0,
        _ => {}
    }
    // IBC chunk 3: the frame-level svt_aom_allow_intrabc (always
    // I-slice + sct on this path) — arms the per-candidate
    // intrabc_fac_bits[0] charge in the funnel. False on every
    // non-screen / p5+ frame (byte-inert there).
    funnel_cfg.allow_intrabc = tile_sc.allow_intrabc;
    // See SVTAV1_SC_TOOLS above.
    match crate::dbgenv::sc_tools() {
        Some("noibc") | Some("none") => funnel_cfg.allow_intrabc = false,
        _ => {}
    }
    let cwid = md_cw;
    // Chroma reconstruction uses an SB-wide stride, matching the padded
    // mode-decision source. A right-straddling candidate must not overwrite
    // the beginning of the next visible row. Luma keeps its existing
    // aligned-stride canvas layout.
    let ext_w = w.div_ceil(sb_size) * sb_size;
    let ext_h = h.div_ceil(sb_size) * sb_size;
    // chroma buffer capacity at `cwid` stride
    let ext_cbuf = chroma_format.chroma_width(ext_w) * chroma_format.chroma_height(ext_h);
    let mut fun_u_recon = svtav1_types::try_vec![128u8; if use_funnel { ext_cbuf } else { 0 }]?;
    let mut fun_v_recon = svtav1_types::try_vec![128u8; if use_funnel { ext_cbuf } else { 0 }]?;
    let mut fun_ectx = if use_funnel || coded_lossless {
        let mut e = EntropyCtx::new(
            w / 4,
            h / 4,
            chroma_420,
            walk_tx_mode_select,
            tile_sc.allow_screen_content_tools,
            bit_depth,
            chroma_format,
        );
        // Task #86: consistent with the other EntropyCtx instances
        // this tile constructs — see the real pack walk's identical
        // assignment for the rationale (leaf_funnel.rs itself is a
        // separate, out-of-scope workstream file; this only sets a
        // field on an EntropyCtx pipeline.rs already owns).
        e.tile_top_px = tile_sb_row_start * sb_size;
        e.tile_left_px = tile_sb_col_start * sb_size; // task #96
        e.tile_mi = crate::intra_edge::TileMi {
            mi_row_start: tile_sb_row_start * sb_size / 4,
            mi_row_end: (tile_sb_row_end * sb_size / 4).min(h / 4),
            mi_col_start: tile_sb_col_start * sb_size / 4,
            mi_col_end: (tile_sb_col_end * sb_size / 4).min(w / 4),
        };
        Some(e)
    } else {
        None
    };
    let fun_rates = if use_funnel || coded_lossless {
        // §1s item 8: C's `init_frame_rate_tables` (md_config_process.c:292)
        // seeds `md_frame_context` from the primary reference's SAVED
        // end-of-frame CDFs when the header names one, and only otherwise
        // from `svt_av1_default_coef_probs(base_q_idx)` +
        // `svt_aom_init_mode_probs`. Pricing an inter frame against the
        // defaults understates the coefficient rate by roughly half on
        // this campaign's reference cell — measured — and that is what
        // makes C's `blk_skip_decision` pick `skip` where a
        // default-priced MD picks a coded residual.
        match md_frame_cdfs {
            Some(prev) => Some(crate::leaf_funnel::build_md_rates(&prev.fc, &prev.coeff)),
            None => {
                let fc = crate::entropy::context::FrameContext::new_default();
                let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                Some(crate::leaf_funnel::build_md_rates(&fc, &cfc))
            }
        }
    } else {
        None
    };
    #[allow(unused_mut)]
    // The frame `av1_lambda_assign_md` lambda (with `lambda_weight`)
    // — IntraBC's error-per-bit derivation and the per-SB fallbacks.
    let pic_lambda: u64 = c_quant.as_ref().map_or(0, |cq| u64::from(cq.lambda));
    let mut fun_frame = if use_funnel {
        let cq = c_quant.as_ref().unwrap();
        Some(crate::leaf_funnel::FunnelFrame {
            native_preset: speed_config.preset,
            reference,
            // C `pcs->slice_type != I_SLICE`.
            // `ref_frame_data` is `Some` exactly on a non-key frame
            // (`encode_frame_impl`'s `if !is_key` binding).
            non_i_slice: ref_frame_data.is_some(),
            // C `ppcs->is_highest_layer`, resolved by the caller.
            is_highest_layer,
            // C `seq_header.sb_mi_size` (task #91): 16 at SB64, 32 at
            // SB128. 16 for every SB64 encode -> byte-neutral there.
            sb_mi_size: sb_size / 4,
            sharpness: hdr_sharpness,
            sharp_tx_active,
            noise_norm_strength: hdr_noise_norm,
            qm_levels,
            tx_bias: hdr_tx_bias,
            mds0_ssd: hdr_complex_hvs,
            tune_ssim: hdr_alt_ssim,
            tune_ssim_threshold: if w * h > 1_665 * 1_120 { 1.02 } else { 1.03 },
            lambda: cq.lambda as u64,
            // `full_lambda_md[EB_10_BIT_MD]` — overwritten per superblock
            // below on the inter arm (the MDS3 hbd_md=2 bump reads it);
            // zero elsewhere.
            lambda10: 0,
            // Overwritten per superblock below on the inter arm; zero on
            // a key frame, where no inter search reads it.
            inter_fast_lambda: sb_inter_lambda
                .and_then(|v| v.first())
                .map_or(0, |l| l.fast_8bit),
            // `aom_av1_set_ssim_rdmult`'s factors + PICTURE lambda bases
            // — `evaluate_leaf` computes the per-BLOCK scale under the
            // SSIM/IQ/MS_SSIM tunes (coding_loop.c:373-382).
            ssim_rdmult: ssim_rdmult.cloned().map(alloc::sync::Arc::new),
            // `blk_lambda_tuning` — `svt_aom_set_tuned_blk_lambda`'s
            // post-`sb_setup_lambda` factor grid (coding_loop.c:368),
            // built by the TPL arm in `generate_sb_qindex` when
            // `r0_delta_qp_md` ran; `None` elsewhere.
            tpl_rdmult: tpl_rdmult.clone(),
            cli_qp: cli_qp as u32,
            rdoq_level: cq.rdoq_level,
            // `ctx->rdoq_ctrls` for the regular lane —
            // `set_rdoq_controls(pcs->rdoq_level)`
            // (sig_deriv_enc_dec_*: enc_mode_config.c:7865/8093) with
            // `md_stage_3`'s `bypass_encdec && PD_PASS_1` clear of
            // `skip_uv`/`dct_dct_only` applied (product_coding_loop.c
            // :7163-7167). The light-PD1 lane's per-SB row lives on
            // `LightPd1Signals::rdoq`.
            rdoq: {
                let mut r = crate::port_enc_mode_config::encdec::set_rdoq_controls(cq.rdoq_level)
                    .unwrap_or(crate::port_enc_mode_config::encdec::RdoqCtrls::DISABLED);
                r.clear_when_bypassed(funnel_cfg.bypass_encdec);
                r
            },
            // Same source as `cq.allintra_rd_mult` (set beside
            // `CodingQuantCfg::new`) so the MD funnel and the bd10
            // re-encode cannot disagree about the RDOQ rate-weight arm.
            rdoq_allintra_rd_mult: cq.allintra_rd_mult,
            base_qindex,
            delta_q_present: fh_delta_q_present,
            fh_qindex: [base_qindex, qindex_u, qindex_v],
            bit_depth,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            // IBC chunk 7: frame-constant DV RD tables (default ndvc at
            // MV_SUBPEL_NONE — `build_dv_cost_tables`'s cadence doc) +
            // the aligned frame height for the vartx bottom clip.
            dv_tables: crate::intrabc::build_dv_cost_tables(
                &crate::entropy::mv_coding::NmvContext::default(),
                funnel_cfg.allow_intrabc,
                false, // approx_inter_rate: structurally 0 on allintra
            )
            .map(alloc::sync::Arc::new),
            frame_h_px: h,
            // The ALIGNED frame width (C `pcs->ppcs->aligned_width`) — the
            // other half of the cropped-TX RD distortion bound. `w`/`h` in
            // this scope are already the aligned dims (see the `ext_w` /
            // `ext_h` SB-extent derivation above, which rounds them UP).
            frame_w_px: w,
            coded_lossless,
            cfg: funnel_cfg,
        })
    } else {
        None
    };
    // ---- IBC chunk 8: frame-level IntraBC state + the MD mi grid ----
    // C md_config_process.c:946-969 (gated frm_hdr->allow_intrabc):
    // the frame hash table over the SOURCE (enhanced_pic), the diamond
    // site config (source stride baked), the one-shot QP mesh rescale;
    // plus this port's search cost tables (nmvc @ LOW precision — the
    // I-slice frame-constant `svt_aom_estimate_mv_rate` build) and the
    // per-block scalars (sadperbit16 from base_q_idx, errorperbit from
    // the funnel lambda >> RD_EPB_SHIFT).
    let ibc_state: Option<alloc::boxed::Box<crate::leaf_funnel::IbcFrameState>> =
        if use_funnel && funnel_cfg.allow_intrabc {
            let mut ctrls = crate::intrabc::IbcCtrls::for_level(tile_sc.intrabc_level);
            // enc_handle.c:3841: all-intra MR disables this sequence flag;
            // the default/video arm keeps it enabled even at MR.
            let mesh_qp_scaling =
                !matches!(sc_arm, crate::sc_detect::ScArm::Allintra) || md_preset > -1;
            crate::intrabc::scale_mesh_patterns_by_qp(&mut ctrls, mesh_qp_scaling, cli_qp as u32);
            let hash = crate::intrabc_hash::generate_ibc_data(
                encode_input,
                w,
                w,
                h,
                ctrls.max_block_size_hash,
                ctrls.max_cand_per_bucket,
                // `pcs->pic_disallow_4x4` — arm-forked at M3
                // (`part_arm::disallow_4x4`), not the flat `preset >= 4`.
                crate::part_arm::disallow_4x4(sc_arm, md_preset),
            );
            // svt_aom_get_sad_per_bit(base_q_idx, 0): init_me_luts_bd's
            // `(int)(0.0418*q + 2.4107)` with q = ac_qlookup/4.0
            // (rc_process.c:186-190, mode_decision.c:2052-2063).
            let q8 = f64::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[base_qindex as usize]) / 4.0;
            let sad_per_bit = (0.0418 * q8 + 2.4107) as i32;
            let error_per_bit = ((pic_lambda) >> crate::intrabc::RD_EPB_SHIFT).max(1) as i32;
            Some(alloc::boxed::Box::new(crate::leaf_funnel::IbcFrameState {
                ctrls,
                hash,
                sites: crate::intrabc::init_search_sites(w),
                search_tables: crate::intrabc::build_nmv_cost_table(
                    &crate::entropy::mv_coding::NmvContext::default(),
                    crate::entropy::mv_coding::MvSubpelPrecision::Low,
                ),
                sad_per_bit,
                error_per_bit,
                mi_rows: (h / 4) as i32,
                mi_cols: (w / 4) as i32,
                tile: crate::intrabc::TileMiBounds {
                    mi_row_start: (tile_sb_row_start * sb_size / 4) as i32,
                    mi_row_end: ((tile_sb_row_end * sb_size / 4).min(h / 4)) as i32,
                    mi_col_start: (tile_sb_col_start * sb_size / 4) as i32,
                    mi_col_end: ((tile_sb_col_end * sb_size / 4).min(w / 4)) as i32,
                },
                sb_mi_size: (sb_size / 4) as i32,
                sb_size_log2_mi: (sb_size as u32 / 4).trailing_zeros(),
                sb_size_px: sb_size as i32,
                disallow_4x4: crate::part_arm::disallow_4x4(sc_arm, md_preset),
            }))
        } else {
            None
        };
    // The MD mode-info grid the MVP scans read (C mi_grid_base as MD
    // stamps it) — frame-wide, one entry per 4x4 cell.
    // The MD mode-info grid — C `mi_grid_base` as MD stamps it. Shared by
    // the IntraBC MVP scans and the inter ones; IntraBC is intra-frame
    // only and inter prediction needs a reference, so the two can never
    // both be live and there is exactly one grid.
    let mut ibc_mvp_grid: alloc::vec::Vec<crate::intrabc_mvp::MvpMiEntry> =
        if ibc_state.is_some() || inter_md.is_some() {
            alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); (w / 4) * (h / 4)]
        } else {
            alloc::vec::Vec::new()
        };

    // Per-SB CDF refresh chain (C update_cdf_level 2 at M4..M6:
    // ec_ctx_array[sb] copied per the left/top-right rule at SB
    // configure, evolved by that SB's coded symbols, and the MD rate
    // tables rebuilt from the copy — enc_dec_process.c:2991-3043).
    // The evolution is simulated by re-coding each decided SB through
    // the real entropy walk against the chain contexts (bypass-encdec
    // makes MD symbols == coded symbols, so the funnel-consumed CDF
    // rows — kf_y/uv/angle/fi/skip/tx_size/coeff — evolve exactly like
    // C's). For frames wider than 2 SBs the both-neighbors case seeds
    // each SB's rate CDF with avg_cdf_symbols (left 3x + top-right 1x,
    // FrameContext::avg_cdf_with + CoeffFc::avg_cdf_with) per the C
    // neighbor rule below — matching enc_dec_process.c:3002-3022.
    let multi_sb = sb_cols * sb_rows > 1;
    // The per-SB CDF-refresh chain is only C-correct at M4..M6
    // (update_cdf_level 2, svt_aom_get_update_cdf_level_allintra
    // enc_mode_config.c:12154). M7/M8/eff-M9 (update_cdf_level 0) keep
    // the static default rate tables for every SB, so they never chain.
    // Gated on use_funnel so it only fires for the chroma/420 funnel
    // path (chroma_src is Some) — mono never chains.
    // `update_cdf_level != 0` for THIS arm — C `set_cdf_controls`'
    // `cdf_ctrl.enabled`, which is what gates `rtime_alloc_ec_ctx_array`
    // and therefore the per-SB context chain at all (enc_mode_config.c:8496,
    // :8945/:9927). allintra: 1 at M0..M3, 2 at M4..M6, 0 above — the
    // `0..=6` this line used to spell inline. video, I-slice: 1 at
    // M0..M8, 0 above, so a video KEY frame CHAINS at presets 7 and 8
    // where the still arm does not.
    //
    // `is_base` is `pcs->temporal_layer_index == 0`: true for every
    // picture of a flat GOP and for a hierarchical one's base layer. It
    // only selects 1-vs-2 in the M1..M3 band and both are nonzero, so
    // this gate's truth value does not depend on it — the real layer is
    // still passed so a future ladder row that flips 0-vs-nonzero cannot
    // silently mis-gate.
    let funnel_chain = use_funnel
        && crate::rate_arm::update_cdf_level(sc_arm, md_eff_mode, temporal_layer == 0) != 0
        && multi_sb;
    let mut chain_snaps: Vec<(
        crate::entropy::context::FrameContext,
        alloc::boxed::Box<crate::entropy::coeff_c::CoeffFc>,
    )> = Vec::new();
    let mut sim_ectx = if funnel_chain {
        // The chain simulation re-codes each SB's symbols to evolve the
        // per-SB frame contexts — it must code the same no-palette
        // flags as the real pack or the palette CDF rows drift.
        let mut e = EntropyCtx::new(
            w / 4,
            h / 4,
            true,
            walk_tx_mode_select,
            tile_sc.allow_screen_content_tools,
            bit_depth,
            chroma_format,
        );
        // IBC chunk 1: same use_intrabc flag coding as the real pack —
        // the chain's intrabc_cdf must evolve identically (the C
        // MD-side twin, update_stats md_rate_estimation.c:854-855).
        e.allow_intrabc = tile_sc.allow_intrabc;
        e.tile_top_px = tile_sb_row_start * sb_size; // task #86, see fun_ectx above
        e.tile_left_px = tile_sb_col_start * sb_size; // task #96
        e.tile_mi = crate::intra_edge::TileMi {
            mi_row_start: tile_sb_row_start * sb_size / 4,
            mi_row_end: (tile_sb_row_end * sb_size / 4).min(h / 4),
            mi_col_start: tile_sb_col_start * sb_size / 4,
            mi_col_end: (tile_sb_col_end * sb_size / 4).min(w / 4),
        };
        // The two the real pack's context carries on an inter frame — see
        // `sim_inter_syntax` in this function's parameter list for the
        // panic this closes. Both are `None` on a key frame, where the
        // inter arm of `encode_block_syntax` is unreachable.
        e.inter_syntax = sim_inter_syntax.cloned();
        if let Some(env) = sim_inter_mvp_env.cloned() {
            e.arm_inter_mvp(env);
        }
        Some(e)
    } else {
        None
    };
    // WRITE-ONLY sink: the funnel chain's simulated entropy walk needs a
    // `DeblockGeom` to `record_block` into, but nothing ever filters
    // through it (the real one is built in `encode_frame_impl` and is the
    // only geom `apply_deblock_frame` / the DLF search ever see). The
    // dims must stay real — `encode_partition_tree` reads `mi_cols`/
    // `mi_rows` for edge clipping — but the `record_*` calls are gated
    // on `writer.cdf_only` in `encode_block_syntax`, so nothing is ever
    // written into it.
    let mut sim_geom = crate::deblock::DeblockGeom::new(w, h, w, h);
    let mut sim_u = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
    let mut sim_v = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
    let mut sim_prev_sb_row = usize::MAX;
    let mut fun_rates = fun_rates;
    // One tree per superblock of the tile — the exact final length, for
    // the same reason `tile_recon` is reserved above. A `PartitionTree` is
    // a large value, so each doubling memcpy's the whole array.
    let mut tile_trees: Vec<crate::partition::PartitionTree> =
        svtav1_types::try_with_capacity!((tile_sb_row_end - tile_sb_row_start) * tile_sb_cols)?;
    // Aligned with `tile_trees` — see the return-tuple comment.
    let mut tile_enc_rdoq: Vec<bool> =
        svtav1_types::try_with_capacity!((tile_sb_row_end - tile_sb_row_start) * tile_sb_cols)?;
    // C `pcs->sb_intra` / `pcs->sb_skip` MID-WALK (`update_b`,
    // coding_loop.c:1606/1643): `pd0_detector` reads the LEFT and TOP
    // superblocks' values (enc_dec_process.c:2513-2525), which are final
    // by then because the SB walk is raster-ordered. Written at each
    // `tile_trees` push below. FRAME-GRID indexed (`sb_index`), per-tile
    // in scope: a neighbour belonging to a different tile reads C's init
    // value (0 / 1), the "not yet coded" state — C's own cross-tile read
    // is a thread race, and the single-tile case this is verified on is
    // exact.
    let mut sb_intra_acc = svtav1_types::try_vec![0u8; sb_cols * sb_rows]?;
    let mut sb_skip_acc = svtav1_types::try_vec![1u8; sb_cols * sb_rows]?;
    let mut tile_frame_recon = svtav1_types::try_vec![128u8; ext_w * ext_h]?;
    // bd10 LUMA mode funnel (task #94): a parallel TRUE 10-bit recon canvas
    // so the per-block mode decision (evaluate_leaf MDS0) is made on the
    // 10-bit recon rather than the MSB-truncated u8 recon (which scales
    // SATD ×4 on `sample<<2` content and cannot flip the survivor). bd8
    // allocates NOTHING and passes `None` into FunnelCtx → the funnel is
    // byte-IDENTICAL. Frame-persistent (a block reads its left/above SB's
    // committed 10-bit recon); each SB's FunnelCtx borrows it.
    //
    // The canvas is SB-extent SIZED at the ALIGNED stride, exactly like the
    // u8 `tile_frame_recon` above, and `commit_leaf` applies the SAME
    // straddle clip to it as to the u8 recon — so a partial SB is in bounds
    // by construction. This predicate used to carry `w % 64 == 0 && h % 64
    // == 0`; that was the fourth independent copy of the bd10 alignment
    // gate, and it was screening a hazard the buffer shape had already
    // removed.
    let bd10_canvas_ok = bit_depth == 10;
    // bd10 FULL-RD (task #94, MODE axis): below eff-M9 the coded mode is
    // the MDS1/MDS3 full-RD winner, not the MDS0 survivor, so the bd10
    // canvas alone is not enough — widening only MDS0 to M6..M8 was
    // measured to close ZERO cells (docs/bd10-port-map.md). `full_rd10`
    // runs the whole full-RD chain (luma depth loop with TXS/TXT + chroma)
    // at 10 bits. It is gated on the arms that ARE ported at 10 bits:
    //   - CfL off: the CfL compare inside MDS3 is 8-bit only, and mixing it
    //     into a 10-bit block cost would be silently wrong (there is a
    //     debug_assert backstop in evaluate_leaf).
    //   - palette off: a palette candidate has no 10-bit prediction here.
    //   - mainline tools only: ac-bias / noise-norm are fork features whose
    //     u16 psy kernels are unported (tx_unit_hbd applies neither).
    // Everything outside that envelope keeps the existing behaviour.
    let bd10_full_rd = bd10_full_rd_supported(
        coded_lossless,
        bit_depth,
        md_preset,
        chroma_420,
        // C `pcs->slice_type == I_SLICE`. `inter_md` is `Some` on exactly
        // the frames that carry a reference, so its absence IS the I-slice
        // test at this layer.
        inter_md.is_none(),
        w,
        h,
    );
    // The `preset >= 9` arm needs the same frame-type term as
    // `bd10_full_rd`: C derives `pcs->hbd_md = is_islice ? 2 : 0` at
    // M6+ (enc_mode_config.c:2163 — TRACED 2026-09-18 on bd10 p6 video:
    // the I-frame runs DUAL, every P-frame is 0), so a non-I frame at
    // preset >= 9 keeps the whole MD — canvas included — in the u8 domain.
    // Running the u16 funnel there computed a 10-bit canvas C never
    // builds.
    let bd10_luma_funnel =
        bd10_canvas_ok && (bd10_full_rd || (md_preset >= 9 && inter_md.is_none()));
    // C's bypass-encdec MDS3 `hbd_md = 2` bump (product_coding_loop.c:9649):
    // `encoder_bit_depth > 8 && bypass_encdec && !hbd_md &&
    // pd_pass == PD_PASS_1 && perform_md_recon`. On a bd10 video frame the
    // leaf funnel IS the PD1 pass and always runs the intra search, so
    // `perform_md_recon` (`need_md_rec_for_intra_pred`, full_loop.c:2763)
    // is true; `!hbd_md` is `!bd10_full_rd` (video `hbd_md` derives 0).
    // The canvases must be LIVE so MDS3 can predict/quantize/reconstruct at
    // 10 bits — while MDS0/MDS1 keep the u8 domain (see
    // `FunnelCtx::mds3_hbd`). Coded-lossless is out: C clears
    // `bypass_encdec` there (md_config_process.c:1046).
    let bd10_mds3_bump = bd10_canvas_ok
        && !bd10_full_rd
        && inter_md.is_some()
        && funnel_cfg.bypass_encdec
        && !coded_lossless;
    // "Plumbing" — the 10-bit canvases/sources/pred buffers exist — is a
    // superset of the MDS0 decision funnel: the still arms decide at
    // 10 bits, the bump arm only hands MDS3 the same buffers.
    let bd10_plumb = bd10_luma_funnel || bd10_mds3_bump;
    // Task #6 chunk 1: hand the funnel the REAL 10-bit source when the
    // caller supplied one AND a bd10 stage is armed to read it. The planes
    // arrive already SB-extent-padded when the frame has a partial SB
    // (`hbd_sb_owned`), so the LUMA stride is `in_stride` — the same stride
    // the u8 `sb_input` gather uses — and a block's `(abs_x, abs_y)` indexes
    // them identically. Chroma was repadded above to the SB-wide `cwid`
    // stride, matching the mode-decision reconstruction canvases.
    let funnel_src10 = hbd_src.filter(|_| bd10_plumb).map(|(y10, u10, v10)| {
        debug_assert!(
            y10.len() >= in_stride * h,
            "hbd luma plane must cover the frame"
        );
        crate::leaf_funnel::FunnelSrc10 {
            y: y10,
            y_stride: in_stride,
            u: u10,
            v: v10,
            c_stride: cwid,
        }
    });
    if funnel_src10.is_some() {
        hbd_used.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    // Ghost Robot `2c66d9ea` — `pcs->input_frame16bit` for PD0: the fork
    // removed the `hbd_md = 0` pin around the multi-pass PD loop, so at
    // bd10 PD0 runs its forced `PD0_LVL_0` full-RD search on the 16-bit
    // input (u16 neighbours/DC pred/residual, `quants_bd`, the
    // `full_sb_lambda_md[EB_10_BIT_MD]` lambda). Scoped to the ALLINTRA
    // arm: `pd0_use_src_samples` is `allintra`-only after the commit —
    // a video frame's PD0 predicts from the 16-bit RECON canvas, which
    // is not threaded here.
    //
    // The plane mirrors the u8 arm's extent: real `hbd_src` rows where
    // a true 16-bit input reached the tile, `<<2`-widened `sb_input`
    // rows otherwise (`unpack_plane` clears `bit_inc`, so C's
    // `input_frame16bit` holds the same MSB<<2 image there), with the
    // bottom margin replicated to the SB extent like the padded plane.
    let pd0_hbd_active = reference == crate::reference::SvtReference::GhostRobot
        && bit_depth == 10
        && matches!(sc_arm, crate::sc_detect::ScArm::Allintra);
    let pd0_src16_owned = pd0_hbd_active.then(|| {
        let rows = ext_h.max(h);
        let mut v = alloc::vec::Vec::with_capacity(in_stride * rows);
        for y in 0..rows {
            let sy = y.min(h - 1);
            match hbd_src {
                Some((y10, _, _)) => {
                    v.extend_from_slice(&y10[sy * in_stride..sy * in_stride + in_stride]);
                }
                None => {
                    let shift = u32::from(bit_depth - 8);
                    v.extend(
                        sb_input[sy * in_stride..(sy + 1) * in_stride]
                            .iter()
                            .map(|&s| u16::from(s) << shift),
                    );
                }
            }
        }
        v
    });
    let pd0_hbd_src = pd0_src16_owned.as_deref().map(|v| (v, in_stride));
    let mut tile_frame_recon10: alloc::vec::Vec<u16> = if bd10_plumb {
        svtav1_types::try_vec![512u16; ext_w * ext_h]?
    } else {
        alloc::vec::Vec::new()
    };
    // bd10 chroma decision canvases (the chroma twins of the luma one).
    // 4:2:0 -> half dims; seeded with the 10-bit DC default like the luma.
    // The bump arm needs them too: C's `md_stage_3` chroma full loop at
    // `hbd_md = 2` predicts from `recon_pic(1)`'s u16 chroma planes.
    let (mut tile_frame_u_recon10, mut tile_frame_v_recon10): (
        alloc::vec::Vec<u16>,
        alloc::vec::Vec<u16>,
    ) = if bd10_full_rd || bd10_mds3_bump {
        let n = chroma_format.chroma_width(ext_w) * chroma_format.chroma_height(ext_h);
        (
            svtav1_types::try_vec![512u16; n]?,
            svtav1_types::try_vec![512u16; n]?,
        )
    } else {
        (alloc::vec::Vec::new(), alloc::vec::Vec::new())
    };

    let mut part_config = crate::partition::PartitionSearchConfig::from_speed_config(speed_config);
    // Task #86: this tile's own top row (luma pixels) — MD search
    // prediction must not treat it as having a real "above" neighbor
    // just because it isn't the frame's own top row (AV1 intra
    // prediction never crosses a tile boundary).
    part_config.tile_top_px = tile_sb_row_start * sb_size;
    // Task #96: ditto for this tile's own left column — MD prediction
    // must not read across a tile-COLUMN boundary either.
    part_config.tile_left_px = tile_sb_col_start * sb_size;
    // C `seq_header.sb_mi_size` (task #91): 16 at SB64 (the struct
    // default, so every pre-SB128 path is byte-identical), 32 at SB128.
    part_config.sb_mi_size = sb_size / 4;
    // The ALIGNED luma extent (C `pcs->ppcs->aligned_width/height`): the
    // clamp for intra reference samples. `w`/`h` here ARE the aligned dims
    // (`encode_frame_420` pads TRUE -> ALIGNED before calling this), NOT
    // the SB extent the recon working buffers are sized to.
    part_config.aligned_w = w;
    part_config.aligned_h = h;
    if chroma_420 {
        // 4:2:0 policy: min luma block dim 8, so every coded block is a
        // chroma reference with chroma dims exactly (w/2, h/2) >= 4.
        part_config.min_block_dim = 8;
    }
    // Preset 5 signals SH enable_intra_edge_filter=1 on the still/420
    // surface (C-exact — the ONLY allintra preset with the bit). A
    // conforming decoder then edge-filters/upsamples directional
    // predictions whose p_angle != 90/180; the homegrown leaf coder
    // predicts UNFILTERED, so until the M5 funnel (which will predict
    // with the C edge filter) routes this preset, D45..D203 candidates
    // must not be emitted — V (exactly 90) and H (exactly 180) are
    // skipped by the decoder's filter and stay recon-exact.
    if md_preset == 5 && chroma_420 && ref_frame_data.is_none() {
        part_config.enable_directional = false;
    }
    // Frame-level C-exact coding quantizer (still path — quant.rs).
    part_config.c_quant = c_quant.clone();

    for sb_row in tile_sb_row_start..tile_sb_row_end {
        // Feature 1: cooperative cancellation, checked once per SB row of
        // the MD search. `may_stop()` short-circuits to `false` for the
        // default `Unstoppable` token, so this is byte-inert unless a real
        // stop token was installed via `with_stop`.
        if stop.may_stop() {
            stop.check()
                .map_err(EncodeError::from)
                .map_err(whereat::at)?;
        }
        for sb_col in tile_sb_col_start..tile_sb_col_end {
            crate::stop_check(&stop)?;
            let sb_x0 = sb_col * sb_size;
            let sb_y0 = sb_row * sb_size;
            let sb_cur_w = sb_size.min(w - sb_x0);
            let sb_cur_h = sb_size.min(h - sb_y0);

            // C `svt_aom_mode_decision_configure_sb` (md_process.c:796):
            // this superblock's MD lambdas, from its own
            // `svt_aom_get_me_qindex`. `sb_inter_lambda` is `None` on a
            // key frame and on every allintra cell, so the still
            // envelope takes neither branch.
            if let (Some(sl), Some(f)) = (
                sb_inter_lambda.and_then(|v| v.get(sb_row * sb_cols + sb_col)),
                fun_frame.as_mut(),
            ) {
                f.lambda = u64::from(sl.full_8bit);
                f.lambda10 = u64::from(sl.full_10bit);
                f.inter_fast_lambda = sl.fast_8bit;
            }
            // The SAME value, for the two partition-side costs C also
            // prices with `full_lambda_md[EB_8_BIT_MD]` /
            // `full_sb_lambda_md[EB_8_BIT_MD]`: the PD0 depth-refinement
            // scan (`perform_pred_depth_refinement`,
            // enc_dec_process.c:3017) and the PD1 walk. Falls back to the
            // frame lambda on a key frame / allintra cell, where
            // `sb_inter_lambda` is `None` — UNLESS the per-SB plan arm
            // below computes this SB's `av1_lambda_assign_md` output,
            // which IS C's `full_sb_lambda_md` snapshot
            // (md_process.c:763-764).
            let mut sb_md_full_lambda: u64 = sb_inter_lambda
                .and_then(|v| v.get(sb_row * sb_cols + sb_col))
                .map_or_else(
                    || c_quant.as_ref().map_or(0, |cq| u64::from(cq.lambda)),
                    |l| u64::from(l.full_8bit),
                );
            // Ghost Robot `2c66d9ea` — `full_sb_lambda_md[EB_10_BIT_MD]`:
            // the lambda the fork's 16-bit PD0 prices every block with.
            // `av1_lambda_assign_md` at this SB's qindex — dc_qlookup_10
            // rdmult, KF frame-type factor (alt row under
            // `alt_lambda_factors`), the delta-q stats factor,
            // `lambda_weight`, then `*16` (md_process.c:753).
            let mut sb_md_full_lambda10: u64 = if pd0_hbd_src.is_some() {
                u64::from(crate::pd0::kf_full_lambda_bd10_tuned(
                    base_qindex,
                    u32::from(picture_qp),
                    hdr_alt_lambda,
                    0,
                    Some(crate::pd0::frame_lambda_weight_for_preset(
                        speed_config.preset,
                        u32::from(picture_qp),
                        tune_iq,
                        lw_bump,
                    )),
                ))
            } else {
                0
            };

            // [SVT_HDR_MODE] variance boost: this SB searches/quantizes
            // at its PLANNED qindex (luma + per-plane chroma) with the
            // matching lambda (C per-SB svt_aom_lambda_assign). The
            // frame-level CDF bucket stays at the FH base (C behavior).
            // [SVT_HDR_MODE] tune-SSIM lambda: `evaluate_leaf` applies
            // C's per-BLOCK `aom_av1_set_ssim_rdmult` scale to the
            // PICTURE lambdas (coding_loop.c:373-382 /
            // product_coding_loop.c:9054/:9371) — no SB-level override
            // here, since a sub-SB leaf covers fewer 16x16 cells than
            // the SB and the scale is the geometric mean over exactly
            // the cells the block touches. `frame.ssim_rdmult` carries
            // the factors + the `lambda_assign` picture bases.
            if let (Some(plan), Some(f)) = (sb_qindex_plan, fun_frame.as_mut()) {
                let sbq = plan[sb_row * sb_cols + sb_col];
                f.base_qindex = sbq;
                f.qindex_u = (i32::from(sbq) + i32::from(chroma_ac_deltas.0)).clamp(0, 255) as u8;
                f.qindex_v = (i32::from(sbq) + i32::from(chroma_ac_deltas.1)).clamp(0, 255) as u8;
                // [SVT_HDR_MODE] per-SB lambda: alt KF factor (fork
                // default) + the delta-q qdiff stats factor
                // (rc_process.c:437-446; this path is fork-only).
                #[cfg(feature = "std")]
                if crate::dbgenv::lambda_dbg_set() {
                    std::eprintln!(
                        "sb lam alt={} sbq={} base={} -> {}",
                        hdr_alt_lambda,
                        sbq,
                        fh_base_qindex,
                        crate::pd0::kf_full_lambda_8bit_ex(
                            sbq,
                            u32::from(picture_qp),
                            hdr_alt_lambda,
                            i32::from(sbq) - i32::from(fh_base_qindex),
                        )
                    );
                }
                if ssim_rdmult.is_none() {
                    // Frame `lambda_weight` + the extended-CRF bump.
                    // `None` with a zero bump keeps the pre-existing
                    // per-SB PSNR ladder this site has always used.
                    let lw_opt = match (hdr_iq_lambda_weight, lw_bump) {
                        (Some(w), b) => Some(w + b),
                        (None, 0) => None,
                        (None, b) => Some(crate::pd0::frame_lambda_weight_for_preset(
                            speed_config.preset,
                            u32::from(picture_qp),
                            false,
                            b,
                        )),
                    };
                    let sb_assigned = u64::from(crate::pd0::kf_full_lambda_8bit_tuned(
                        sbq,
                        // C keys `pcs->lambda_weight` on `ppcs->picture_qp`
                        // — the FRAME qp (enc_mode_config.c:10101-10107),
                        // constant across SBs. A per-SB `qindex_to_qp(sbq)`
                        // here drops the weight to 0 on SBs whose delta-q
                        // falls under the >=16 rung while the frame qp
                        // stays above it (measured: sbq=62/base=71 -> qp15
                        // vs frame qp18, lambda 9718 where C has 11388).
                        u32::from(picture_qp),
                        hdr_alt_lambda,
                        i32::from(sbq) - i32::from(fh_base_qindex),
                        lw_opt,
                    ));
                    f.lambda = sb_assigned;
                    // C `full_sb_lambda_md` (md_process.c:763-764): the
                    // per-SB `av1_lambda_assign_md` snapshot, priced on
                    // EVERY block in this SB's partition/depth walk and
                    // in the `blk_ptr->cost` recompute after winner
                    // select (mode_decision.c:3880-3883). On a frame
                    // with `sb_inter_lambda` the per-SB inter assign
                    // already bound the correct value above — this arm
                    // is the key/allintra-frame path.
                    if sb_inter_lambda.is_none() {
                        sb_md_full_lambda = sb_assigned;
                        // Ghost Robot `2c66d9ea`: the SAME snapshot at
                        // `EB_10_BIT_MD` for the 16-bit PD0 arm — the
                        // bd10 twin chain at this SB's qindex.
                        if pd0_hbd_src.is_some() {
                            sb_md_full_lambda10 = u64::from(crate::pd0::kf_full_lambda_bd10_tuned(
                                sbq,
                                u32::from(picture_qp),
                                hdr_alt_lambda,
                                i32::from(sbq) - i32::from(fh_base_qindex),
                                lw_opt,
                            ));
                        }
                    }
                }
            }

            let ref_ctx = ref_frame_data.map(|rf| crate::partition::RefFrameCtx {
                y_padded: ref_padded.map(|p| &p.y),
                uv_padded: ref_padded.and_then(|p| p.uv.as_ref()),
                ss_x: chroma_format.subsampling_x() as usize,
                ss_y: chroma_format.subsampling_y() as usize,
                sb_size,
                y_plane: rf,
                stride: w,
                pic_width: w,
                pic_height: h,
                mv_map: Some(mv_map),
                mv_map_stride,
            });
            // C `svt_aom_mode_decision_configure_sb` (md_process.c:800-803):
            //     ctx->qp_index = delta_q_present || r0_delta_qp_md
            //                   ? sb_qp : base_q_idx;
            // and `ctx->qp_index` drives the WHOLE MD context — the PD0
            // tables and the partition search included, not just the leaf.
            //
            // This was pinned to `base_qindex`. The per-SB value was already
            // threaded into the leaf funnel a few lines above
            // (`f.base_qindex = sbq`), so with variance boost on the funnel
            // and the partition search were pricing against DIFFERENT
            // quantizers within the same superblock.
            //
            // `sb_qindex_plan.is_some()` is exactly the port's
            // `delta_q_present`: the plan is built only under
            // `hdr.enable_variance_boost`, and the same `Option` gates the
            // frame header's delta-q signalling (`delta_q_res_signal`). C's
            // second disjunct, `r0_delta_qp_md`, is TPL-driven and always
            // false for a single still (no lookahead), so it is not modelled
            // — see the CRF==CQP note in rust/CLAUDE.md.
            let sb_qindex = match sb_qindex_plan {
                Some(plan) => plan[sb_row * sb_cols + sb_col],
                None => base_qindex,
            };
            // ---------------------------------------------- SB128 (#91)
            // The b64 CODING UNITS of this superblock, in C's coding
            // order (`sb128_geom::sb_coding_units`). SVT's b64 grid is
            // ALWAYS 64x64 while the sb grid follows super_block_size, so
            // the per-64 machinery (PD0 tree, variance map, leaf funnel,
            // recon) is size-agnostic — only the visiting ORDER and the
            // extra 128-root partition symbol differ. At SB64 there is
            // exactly ONE unit (the SB itself) with `unit_size ==
            // sb_size`, so the loop below is byte-identical to the
            // pre-SB128 code by construction.
            //
            // Everything OUTSIDE the unit loop stays per-SB — notably the
            // `ec_ctx` chain base / rate tables, because C's
            // `ec_ctx_array[sb]` is genuinely SB-indexed: at SB128 the
            // rate-estimation CDF seed refreshes once per 128 REGION
            // (4x coarser), which is the map's §"Pipeline state"
            // behavioural delta. Keeping the chain here gets that right
            // for free.
            let units = crate::sb128_geom::sb_coding_units(sb_x0, sb_y0, sb_size, w, h);
            let unit_size = if sb_size == 128 { 64 } else { sb_size };
            // §1s item 1: BOTH gates used to carry a `ref_*.is_none()`
            // term, so any frame with a reference bypassed the C-exact
            // PD0 partition search AND the leaf funnel and ran the
            // pre-campaign `partition::partition_search_with_config`
            // recursion — the code every video-KEY chunk of this campaign
            // was built to replace. They are gone; the funnel's inter
            // candidate (item 1b) is what makes taking them off pay.
            let use_pd0 = md_preset >= 6 || (matches!(md_preset, -1..=5) && use_funnel);
            // CLI-qp-calibrated lambda via the exact inverse mapping
            // (see qp_to_lambda's domain note). On the PD0 fixed-tree
            // path the leaf funnel must be preset-INDEPENDENT like
            // C's (the C decision lambda is the same kf chain at M6
            // and eff-M9 — instrumented 1527856 at qindex 220 in
            // both), so it pins the scale the byte-identical M10/M13
            // cells validated instead of the per-preset homegrown
            // scale.
            let leaf_scale = if use_pd0 {
                crate::speed_config::SpeedConfig::from_preset(13).lambda_scale()
            } else {
                speed_config.lambda_scale()
            };
            let sb_lambda =
                (crate::rate_control::qp_to_lambda(crate::rate_control::qindex_to_qp(sb_qindex))
                    * leaf_scale) as u64;

            // C-exact partition source: at allintra presets >= 9 the C
            // library (which clamps allintra presets to M9) decides the
            // ENTIRE partition tree in PD0 with a fixed {NONE, SPLIT}
            // quadtree and no NSQ search (docs/IDENTITY-STATUS.md
            // 2026-07-13 diagnosis), and at M2..M8 the same
            // PRED_PART_ONLY architecture runs the prediction-based
            // PD0_LVL_1 block encode instead (M6 chunk diagnosis).
            // Key/still frames at presets >= 6 — and preset 5 when
            // the M5 leaf funnel is live (still/420) — take the
            // ported PD0 decisions (crate::pd0) and encode the fixed
            // tree; everything else keeps the homegrown search.
            // (Presets 2..4 also run PD0_LVL_1 in C, but their PD1
            // leaf configs are unported, so they stay on the
            // homegrown path until they land. M5 depth refinement is
            // ADAPTIVE level 9 — the refined depths lose the
            // inter-depth compare on every tracked cell, the coded
            // tree == the PD0 tree; see docs/IDENTITY-STATUS.md.)
            // The search reads intra neighbors from — and reconstructs
            // directly into — the live frame buffer, exactly like the
            // decoder (fixes within-SB predictions that previously fell
            // back to 128).
            // Chain: select this SB's context base per the C rule and
            // rebuild the funnel rate tables from it.
            // Frame-grid raster index — also read by the std-gated
            // CHAINDUMP / SEED debug dumps below.
            let sb_index = sb_row * sb_cols + sb_col;
            // C `pd0_detector`'s per-superblock input
            // (enc_dec_process.c:2406-2525). C runs the detector per SB
            // inside `svt_aom_mode_decision_kernel` — BEFORE this SB's
            // blocks are coded but AFTER the left/top SBs were — so the
            // level it lands on is per-SB, and so is everything derived
            // from it. `me_*` are `ppcs->me_*[sb_index]`; the neighbour
            // entries are `sb_index - 1` / `sb_index - pic_width_in_sb`,
            // read only in the non-edge branch (`sb_col`/`sb_row` > 0,
            // guaranteed by `is_edge_sb`'s complement) — the
            // `checked_sub` guards are for the edge case the read never
            // reaches. The left/top `sb_intra`/`sb_skip` come from THIS
            // frame's accumulating flags — see `sb_intra_acc` above.
            let pd0_me = |idx: usize| inter_md.and_then(|f| f.me.per_b64.get(idx));
            let pd0_sb_in = crate::port_pd0_detector::Pd0SbInput {
                slice_type_is_intra: inter_md.is_none(),
                transition_present: pd0_det_frame.transition_present,
                picture_qp: u32::from(picture_qp),
                ref_intra_percentage: u32::from(pd0_det_frame.ref_intra_percentage),
                me_8x8_cost_variance: pd0_me(sb_index).map_or(0, |o| o.me_8x8_cost_variance),
                me_64x64_distortion: pd0_me(sb_index).map_or(0, |o| o.me_64x64_distortion),
                is_edge_sb: sb_col == 0 || sb_row == 0,
                left_me_8x8_cost_variance: sb_index
                    .checked_sub(1)
                    .and_then(&pd0_me)
                    .map_or(0, |o| o.me_8x8_cost_variance),
                top_me_8x8_cost_variance: sb_index
                    .checked_sub(sb_cols)
                    .and_then(&pd0_me)
                    .map_or(0, |o| o.me_8x8_cost_variance),
                left_me_64x64_distortion: sb_index
                    .checked_sub(1)
                    .and_then(&pd0_me)
                    .map_or(0, |o| o.me_64x64_distortion),
                top_me_64x64_distortion: sb_index
                    .checked_sub(sb_cols)
                    .and_then(&pd0_me)
                    .map_or(0, |o| o.me_64x64_distortion),
                left_sb_intra: sb_index
                    .checked_sub(1)
                    .and_then(|i| sb_intra_acc.get(i))
                    .is_some_and(|&v| v != 0),
                top_sb_intra: sb_index
                    .checked_sub(sb_cols)
                    .and_then(|i| sb_intra_acc.get(i))
                    .is_some_and(|&v| v != 0),
                left_sb_skip: sb_index
                    .checked_sub(1)
                    .and_then(|i| sb_skip_acc.get(i))
                    .is_none_or(|&v| v != 0),
                top_sb_skip: sb_index
                    .checked_sub(sb_cols)
                    .and_then(|i| sb_skip_acc.get(i))
                    .is_none_or(|&v| v != 0),
                ref_l0: crate::port_pd0_detector::RefSbInfo {
                    was_intra: pd0_det_frame
                        .l0
                        .sb_intra
                        .and_then(|v| v.get(sb_index).copied()),
                    was_skip: pd0_det_frame
                        .l0
                        .sb_skip
                        .and_then(|v| v.get(sb_index).copied())
                        .map(|v| v != 0),
                    is_intra_slice: pd0_det_frame.l0.is_islice,
                    me_64x64_dist: pd0_det_frame
                        .l0
                        .me_64x64_dist
                        .and_then(|v| v.get(sb_index).copied()),
                    me_8x8_cost_var: pd0_det_frame
                        .l0
                        .me_8x8_cost_var
                        .and_then(|v| v.get(sb_index).copied()),
                },
                ref_l1: crate::port_pd0_detector::RefSbInfo {
                    was_intra: pd0_det_frame
                        .l1
                        .sb_intra
                        .and_then(|v| v.get(sb_index).copied()),
                    was_skip: pd0_det_frame
                        .l1
                        .sb_skip
                        .and_then(|v| v.get(sb_index).copied())
                        .map(|v| v != 0),
                    is_intra_slice: pd0_det_frame.l1.is_islice,
                    me_64x64_dist: pd0_det_frame
                        .l1
                        .me_64x64_dist
                        .and_then(|v| v.get(sb_index).copied()),
                    me_8x8_cost_var: pd0_det_frame
                        .l1
                        .me_8x8_cost_var
                        .and_then(|v| v.get(sb_index).copied()),
                },
            };
            // This superblock's `ctx->disallow_4x4` for PD0 — the pic
            // value (`get_disallow_4x4_default`; FALSE at video M0..M2,
            // so a key frame there floors PD0 at 4x4) where depth
            // removal resolved nothing, else that result's flag.
            let pd0_disallow_4x4 = pd0_dr_res
                .and_then(|v| v.get(sb_index).copied())
                .map_or_else(
                    || crate::part_arm::disallow_4x4(sc_arm, md_preset),
                    |r| r.disallow_4x4,
                );
            // The RESOLVED post-detector `Pd0Level` for THIS superblock,
            // plus the things `video_pd0_params` derives alongside it —
            // `coeff_rate_est_lvl`, `use_accurate_part_ctx` and the
            // `subres_ctrls.step` `sig_deriv_enc_dec_pd0` resolves per
            // superblock (enc_mode_config.c:7322-7357). `None` off the
            // video arm (a still picture's PD0 level comes from the
            // allintra detector instead), which is every callsite's
            // "leave the pre-existing model alone" arm.
            let sb_pd0_det = matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }).then(|| {
                // This superblock's `DepthRemovalResult` — computed
                // once for the frame in `encode_frame_impl`'s
                // `pd0_min_sq` fold. `None` on a key frame, where
                // `set_depth_removal_level_controls` returns
                // `enabled = 0` and `disallow_4x4` stays at the pic
                // value (`get_disallow_4x4_default`).
                let sb_dr = pd0_dr_res.and_then(|v| v.get(sb_index).copied());
                crate::part_arm::video_pd0_params(
                    md_eff_mode,
                    u32::from(cli_qp),
                    w * h,
                    // C `pcs->coeff_lvl` — `derive_inter_coeff_level`'s
                    // output for an inter frame (carried on the coding
                    // quantizer); `video_pd0_params` maps an I-slice
                    // to `INVALID_LVL` itself.
                    c_quant
                        .as_ref()
                        .map_or(crate::port_enc_mode_config::InputCoeffLvl::Normal, |q| {
                            crate::part_arm::input_coeff_lvl(q.input_coeff_level)
                        }),
                    temporal_layer,
                    // C `pcs->hbd_md != 0` — `set_pd0_ctrls`
                    // (enc_mode_config.c:5415) forces `PD0_LVL_0` on
                    // exactly these frames. MAINLINE derivation
                    // (`is_islice ? 2 : 0` at M6+, `is_base ? 2 : 0` at
                    // M0..M5, 1 at MR — enc_mode_config.c:2158-2163);
                    // `is_base` is always true on this port's flat GOP
                    // and MR is below the preset floor, so the term
                    // reduces to `preset <= 5 || I-slice`. GHOST ROBOT
                    // (`2c66d9ea`, enc_mode_config.c:2180-2199) keeps
                    // `hbd_md` set on inter frames through M9 inside the
                    // temporal-layer bound (<= M5 always, <= M8 at
                    // TL<=2, <= M9 at TL<=1) as well as on every
                    // I-slice — a bd10 non-I frame at M6..M9 also runs
                    // PD0_LVL_0 there.
                    bit_depth == 10
                        && if reference == crate::reference::SvtReference::GhostRobot {
                            inter_md.is_none()
                                || md_eff_mode <= 5
                                || (md_eff_mode <= 8 && temporal_layer <= 2)
                                || (md_eff_mode <= 9 && temporal_layer <= 1)
                        } else {
                            speed_config.preset <= 5 || inter_md.is_none()
                        },
                    &pd0_sb_in,
                    &crate::part_arm::Pd0SigDerivInput {
                        is_not_last_layer: pd0_det_frame.is_not_last_layer,
                        pic_pred_depth_only: pd0_pred_depth_only,
                        disallow_4x4: pd0_disallow_4x4,
                        disallow_8x8: crate::port_enc_mode_config::leaf::get_disallow_8x8_default(),
                        // `b64_geom->is_complete_b64` — against the
                        // ALIGNED dims (`b64_geom_init` gets
                        // `pcs->aligned_width`), per b64 on the
                        // 64-grid this SB-loop index is on (sb_size
                        // is 64 on every video path today).
                        b64_is_complete: sb_col * sb_size + 64 <= w && sb_row * sb_size + 64 <= h,
                        depth_removal: sb_dr.map_or_else(Default::default, |r| r.ctrls),
                        fast_lambda_8bit: sb_inter_lambda
                            .and_then(|v| v.get(sb_index))
                            .map_or(0, |l| l.fast_8bit),
                        me_8x8_distortion: pd0_me(sb_index).map_or(0, |o| o.me_8x8_distortion),
                        base_q_idx: u32::from(base_qindex),
                        super_block_size: u32::try_from(sb_size).unwrap_or(u32::MAX),
                    },
                )
            });
            // C `ctx->depth_removal_ctrls.disallow_below_64x64` for
            // `md_encode_block`'s `skip_pd_pass_0` gate — this SB's
            // `DepthRemovalResult`, the same one `sb_pd0_det` reads.
            let sb_disallow_below_64x64 = pd0_dr_res
                .and_then(|v| v.get(sb_index).copied())
                .is_some_and(|r| r.ctrls.disallow_below_64x64 != 0);
            // The refinement path's PD0 model at the same per-SB level —
            // C derives everything downstream of `pd0_detector` from the
            // ONE `md_ctx->pd0_ctrls.pd0_level` it resolved.
            let (pd0_refined_mode, pd0_refined_eexit_th, pd0_refined_rate_lvl) =
                crate::part_arm::refined_pd0_model(
                    sc_arm,
                    sb_pd0_det.map_or(0, |t| t.0),
                    pd0_pred_depth_only,
                );
            let pd0_refined_rate_lvl =
                pd0_refined_rate_lvl.unwrap_or(funnel_cfg.coeff_rate_est_lvl);
            // `SVTAV1_PD0DBG`: the port-side twin of C's `SVT_PD0CFG_OUT`
            // `lvl=`/`rate_lvl=` fields — the POST-detector values, so the
            // two dumps join per superblock per frame.
            #[cfg(feature = "std")]
            if crate::dbgenv::pd0dbg()
                && let Some((lvl, rate_lvl, _, subres, _)) = sb_pd0_det
            {
                eprintln!(
                    "PD0CFG sb={sb_index} lvl={lvl} rate_lvl={rate_lvl} subres={subres} \
                         ref0in={:?} ref1in={:?} lin={} tin={} lsk={} tsk={} \
                         med={} mev={} edge={}",
                    pd0_sb_in.ref_l0.was_intra,
                    pd0_sb_in.ref_l1.was_intra,
                    pd0_sb_in.left_sb_intra as u8,
                    pd0_sb_in.top_sb_intra as u8,
                    pd0_sb_in.left_sb_skip as u8,
                    pd0_sb_in.top_sb_skip as u8,
                    pd0_sb_in.me_64x64_distortion,
                    pd0_sb_in.me_8x8_cost_variance,
                    pd0_sb_in.is_edge_sb as u8,
                );
            }
            // Superres chunk B.4: this SB's entry in C's STALE variance
            // array — the CODED grid's linear index into an array laid out
            // on the FULL-RES grid (exactly the indexing C does after
            // `scale_pcs_params`). `None` on every non-superres path, where
            // the variance is recomputed from the coded source instead.
            let sb_stale_vars: Option<&crate::pd0::SbVariance> =
                stale_vars.and_then(|v| v.get(sb_row * sb_cols + sb_col));
            // PORT-NOTE(unverified): `chain_snaps` is a PER-TILE
            // accumulator (pushed once per SB in this tile's own
            // raster order, starting empty at tile_idx's first SB —
            // see the push site below), so it must be indexed
            // TILE-LOCALLY, not by the absolute frame-wide `sb_index`.
            // Before task #86 `tile_rows` was always 1 (tile_idx == 0,
            // tile_sb_row_start == 0), so local == absolute and this
            // bug was unreachable — real `--tile-rows` use is what
            // exposed it (`sb_index - 1` / `sb_index - sb_cols + 1`
            // underflowed/out-of-bounded on tile_idx >= 1, a hard
            // panic, not a byte divergence). `topright_avail`'s row
            // check now gates on the TILE's own top row
            // (`sb_row > tile_sb_row_start`), matching this being a
            // per-tile-reset rate-ESTIMATE chain (mirrors the real
            // entropy walk's per-tile above-context reset in
            // `run_entropy_walk`) — not verified against C's own
            // per-tile `ec_ctx_array` neighbor rule at a tile-row
            // boundary specifically (only the single-tile-frame shape
            // was ever C-cross-checked); this only affects MD RATE
            // ESTIMATES (candidate cost comparisons), never the
            // coded bitstream, whose entropy state comes from the
            // separately-reset `run_entropy_walk`.
            let local_sb_index =
                (sb_row - tile_sb_row_start) * tile_sb_cols + (sb_col - tile_sb_col_start);
            let chain_base = if funnel_chain {
                // C `ec_ctx_array[sb]` neighbor rule for the rate-estimation
                // CDF (enc_dec_process.c:3002-3022). `pic_based_rate_est` is
                // only ever false (enc_handle.c), so the weighted-average
                // branch always runs. Availability predicates match C for a
                // single-tile SB-aligned frame: left = not tile-left column,
                // top-right = not tile-top row AND the SB one to the right
                // exists (so the last column has no top-right).
                let left_avail = sb_col > tile_sb_col_start;
                let topright_avail = sb_row > tile_sb_row_start && sb_col + 1 < tile_sb_col_end;
                if left_avail && topright_avail {
                    // both -> copy left, then avg with top-right (3:1).
                    // C AVG_CDF_WEIGHT_LEFT / AVG_CDF_WEIGHT_TOP
                    // (enc_dec_process.c:2665-2666, :3016-3021).
                    const WT_LEFT: i32 = 3;
                    const WT_TOP: i32 = 1;
                    let mut base = chain_snaps[local_sb_index - 1].clone();
                    let tr = &chain_snaps[local_sb_index - tile_sb_cols + 1];
                    base.0.avg_cdf_with(&tr.0, WT_LEFT, WT_TOP);
                    base.1.avg_cdf_with(tr.1.as_ref(), WT_LEFT, WT_TOP);
                    Some(base)
                } else if left_avail {
                    // left only -> copy left (sb-1)
                    Some(chain_snaps[local_sb_index - 1].clone())
                } else if topright_avail {
                    // top-right only -> copy top-right (sb - tile_sb_cols + 1)
                    Some(chain_snaps[local_sb_index - tile_sb_cols + 1].clone())
                } else {
                    // neither -> md_frame_context (default)
                    None
                }
            } else {
                None
            };
            // Diagnostic aid: SVTAV1_CHAIN_DUMP=1 prints each SB's
            // post-configure (chain_base) coeff CDF — the exact
            // per-SB rate-estimation context C builds from
            // ec_ctx_array[sb] (enc_dec_process.c:3010-3022). Used to
            // verify the avg_cdf chain against instrumented C
            // (2026-07-15 M6 diagnosis: chain proven C-exact through
            // sb36; the recon divergence is a downstream leaf-coeff
            // issue, NOT the chain). No encoder-output change.
            #[cfg(feature = "std")]
            if funnel_chain && crate::dbgenv::chain_dump() {
                let dflt_cfc;
                let cfc: &crate::entropy::coeff_c::CoeffFc = match &chain_base {
                    Some((_, cfc)) => cfc.as_ref(),
                    None => {
                        dflt_cfc =
                            crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                        &dflt_cfc
                    }
                };
                eprint!("CHAINDUMP CFG sb={sb_index} col={sb_col} row={sb_row}");
                eprint!(" cbeobY");
                for c in 0..4 {
                    let e = &cfc.coeff_base_eob_cdf[c];
                    eprint!(" {},{}", e[0], e[1]);
                }
                eprint!(" cbeobU");
                for c in 0..4 {
                    let e = &cfc.coeff_base_eob_cdf[4 + c];
                    eprint!(" {},{}", e[0], e[1]);
                }
                // Drill rows: tx1 (8x8) Y + dc_sign/eob16/t0-skip, to
                // join against the C-side CSEED dump row-for-row.
                eprint!(" t1eobY");
                for c in 0..4 {
                    let e = &cfc.coeff_base_eob_cdf[(1 * 2 + 0) * 4 + c];
                    eprint!(" {},{}", e[0], e[1]);
                }
                eprint!(" t1baseY");
                for c in 0..6 {
                    let e = &cfc.coeff_base_cdf[(1 * 2 + 0) * 42 + c];
                    eprint!(" {},{},{}", e[0], e[1], e[2]);
                }
                eprint!(" t1brY");
                for c in 0..4 {
                    let e = &cfc.coeff_br_cdf[(1 * 2 + 0) * 21 + c];
                    eprint!(" {},{}", e[0], e[1]);
                }
                eprint!(" t1skip=");
                for c in 0..13 {
                    eprint!(
                        "{}{}",
                        if c == 0 { "" } else { "," },
                        cfc.txb_skip_cdf[1 * 13 + c][0]
                    );
                }
                let ds = &cfc.dc_sign_cdf[0];
                let eo = &cfc.eob_flag_cdf16[1];
                eprint!(" dcsign={} eobY1={},{}", ds[0], eo[0], eo[1]);
                eprint!(" t0skip=");
                for c in 0..13 {
                    eprint!(
                        "{}{}",
                        if c == 0 { "" } else { "," },
                        cfc.txb_skip_cdf[c][0]
                    );
                }
                eprintln!();
            }
            // SVTAV1_SEED_DUMP=1: one line per SB with salient SYNTAX-CDF
            // seed rows, field-for-field matching the C-side SVT_SEED_OUT
            // interposer (wrap on svt_aom_estimate_syntax_rate). diff the
            // two files -> first SB whose rate seed diverges (the "every
            // leaf cost in the SB shifted" divergence class).
            #[cfg(feature = "std")]
            if funnel_chain && crate::dbgenv::seed_dump() {
                let dflt;
                let (fc, cfc): (
                    &crate::entropy::context::FrameContext,
                    &crate::entropy::coeff_c::CoeffFc,
                ) = match &chain_base {
                    Some((fc, cfc)) => (fc, cfc.as_ref()),
                    None => {
                        dflt = (
                            crate::entropy::context::FrameContext::new_default(),
                            crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                        );
                        (&dflt.0, &dflt.1)
                    }
                };
                eprintln!(
                    "SEED sb={} part0={},{},{} kf00={},{},{} txs00={},{} skip0={} ang0={},{},{} cfls={},{},{} cfla0={},{},{} xtx={},{},{}",
                    sb_index,
                    fc.partition_cdf[0][0],
                    fc.partition_cdf[0][1],
                    fc.partition_cdf[0][2],
                    fc.kf_y_mode_cdf[0][0][0],
                    fc.kf_y_mode_cdf[0][0][1],
                    fc.kf_y_mode_cdf[0][0][2],
                    fc.tx_size_cdf[0][0][0],
                    fc.tx_size_cdf[1][0][0],
                    fc.skip_cdf[0][0],
                    fc.angle_delta_cdf[0][0],
                    fc.angle_delta_cdf[0][1],
                    fc.angle_delta_cdf[0][2],
                    fc.cfl_sign_cdf[0],
                    fc.cfl_sign_cdf[1],
                    fc.cfl_sign_cdf[2],
                    fc.cfl_alpha_cdf[0][0],
                    fc.cfl_alpha_cdf[0][1],
                    fc.cfl_alpha_cdf[0][2],
                    cfc.intra_ext_tx_cdf[52][0],
                    cfc.intra_ext_tx_cdf[52][1],
                    cfc.intra_ext_tx_cdf[52][2],
                );
                // SEED2 (2026-09-05): the rows the screen-content residuals
                // turned on — joins line-for-line against the C
                // interposer's `SEED2` (wrap_recon.c, SVT_SEED_OUT).
                let join = |v: &[u16]| {
                    v.iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                };
                eprintln!(
                    "SEED2 sb={} kfy00=[{}] fi8=[{},{}] fim=[{}] txfmp=[{}] iext11=[{}] paly0=[{},{},{}]",
                    sb_index,
                    join(&fc.kf_y_mode_cdf[0][0][..13]),
                    fc.filter_intra_cdfs[3][0],
                    fc.filter_intra_cdfs[3][1],
                    join(&fc.filter_intra_mode_cdf[..5]),
                    join(
                        &fc.txfm_partition_cdf
                            .iter()
                            .map(|c| c[0])
                            .collect::<Vec<_>>()
                    ),
                    join(&cfc.inter_ext_tx_cdf[1 * 4 + 1][..16]),
                    fc.palette_y_mode_cdf[0][0][0],
                    fc.palette_y_mode_cdf[0][1][0],
                    fc.palette_y_mode_cdf[0][2][0],
                );
                // SEED3: full partition/skip/txsize ctx[0] rows — joins
                // the C interposer's SEED3 (wrap_recon.c).
                eprintln!(
                    "SEED3 sb={} part=[{}] skip=[{}] txs=[{}]",
                    sb_index,
                    join(&fc.partition_cdf.iter().map(|r| r[0]).collect::<Vec<_>>()),
                    join(&fc.skip_cdf.iter().map(|r| r[0]).collect::<Vec<_>>()),
                    join(
                        &fc.tx_size_cdf
                            .iter()
                            .flat_map(|cat| cat.iter().map(|ctx| ctx[0]).collect::<Vec<_>>())
                            .collect::<Vec<_>>()
                    )
                );
            }
            if funnel_chain {
                fun_rates = Some(match &chain_base {
                    Some((fc, cfc)) => crate::leaf_funnel::build_md_rates(fc, cfc),
                    None => {
                        let fc = crate::entropy::context::FrameContext::new_default();
                        let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                        crate::leaf_funnel::build_md_rates(&fc, &cfc)
                    }
                });
            }
            // Per-b64 coding units (SB128: up to 4 in Z-order; SB64: the
            // SB itself — see the `units` comment above).
            let unit_results: Vec<crate::partition::PartitionResult> =
                Vec::with_capacity(units.len());
            // Per-unit encode-pass RDOQ enable (C's per-SB
            // `ed_ctx->md_ctx->rdoq_ctrls->enabled`), folded into one
            // `tile_enc_rdoq` entry when the units merge below.
            let unit_enc_rdoq: Vec<bool> = Vec::with_capacity(units.len());
            // SB128 depth-refinement: C's `get_max_min_pd0_depths`
            // (enc_dec_process.c:1943) derives max/min PD0 block sizes over
            // the WHOLE 128 SB pc_tree (all four 64x64 quadrants), and feeds
            // them to `set_start_end_depth`'s `limit_max_min_to_pd0` gate.
            // The port's per-64-unit refined scan must see that SAME whole-SB
            // fold, not this one quadrant's — else a quadrant with PD0 max 16
            // caps its shallowest tested depth at 16x16 and force-splits the
            // 32x32 nodes a sibling quadrant's max-32 keeps. Folded once per
            // SB, lazily, from the same pure PD0 eval the unit loop recomputes
            // (`pd0_pick_sb_partition_m6_eval` reads only source pixels).
            // `None` at SB64 (units.len() == 1) → byte-identical.
            // This superblock's PD0 inter context. The frame-level base
            // is copied and given THIS SB's `min_sq`, because
            // `depth_removal_ctrls` is resolved per superblock from its
            // own ME distortions (`set_depth_removal_level_controls`,
            // enc_mode_config.c:3160-3270).
            let pd0_inter = pd0_inter_base.map(|b| crate::pd0::Pd0InterRef {
                min_sq: pd0_min_sq
                    .and_then(|v| v.get(sb_row * sb_cols + sb_col).copied())
                    .map_or(8, usize::from),
                // C `full_sb_lambda_md[EB_8_BIT_MD]` is this superblock's
                // MD lambda (`av1_lambda_assign_md`'s last two lines), so
                // PD0's own rdcost is per-SB too.
                me_qdiff: sb_inter_lambda
                    .and_then(|v| v.get(sb_row * sb_cols + sb_col))
                    .map_or(0, |l| l.me_qdiff),
                ..b
            });
            let sb_pd0_max_min: Option<(usize, usize)> = None;
            let (sb_result, sb_enc_rdoq) = encode_coding_unit(
                encode_input,
                sb_input,
                in_stride,
                w,
                h,
                sb_size,
                base_qindex,
                qindex_u,
                qindex_v,
                qm_levels,
                sc_arm,
                cli_qp,
                picture_qp,
                lw_bump,
                tune_iq,
                speed_config,
                ref_frame_data,
                temporal_layer,
                ref_padded,
                inter_md,
                pd0_dr_res,
                ref_min_max_sq,
                md_frame_cdfs,
                c_quant,
                bit_depth,
                max_tx_size,
                coded_lossless,
                pd0_det_frame,
                lpd1_frame,
                chroma_src,
                tile_sb_row_start,
                tile_sb_col_start,
                &mut m6_pd0_tables,
                pd0_frame_tables,
                use_funnel,
                tile_sc,
                md_preset,
                pd0_pred_depth_only,
                pd0_video_recon,
                cwid,
                &mut fun_u_recon,
                &mut fun_v_recon,
                &mut fun_ectx,
                &fun_frame,
                &ibc_state,
                &mut ibc_mvp_grid,
                funnel_chain,
                &mut chain_snaps,
                &mut sim_ectx,
                &mut sim_geom,
                &mut sim_u,
                &mut sim_v,
                &mut sim_prev_sb_row,
                &fun_rates,
                &mut tile_frame_recon,
                bd10_full_rd,
                bd10_mds3_bump,
                bd10_plumb,
                funnel_src10,
                &mut tile_frame_recon10,
                &mut tile_frame_u_recon10,
                &mut tile_frame_v_recon10,
                &part_config,
                sb_row,
                sb_x0,
                sb_y0,
                sb_md_full_lambda,
                pd0_hbd_src,
                sb_md_full_lambda10,
                ref_ctx,
                sb_qindex,
                units,
                unit_size,
                use_pd0,
                sb_lambda,
                sb_index,
                pd0_sb_in,
                pd0_disallow_4x4,
                sb_pd0_det,
                sb_disallow_below_64x64,
                pd0_refined_mode,
                pd0_refined_eexit_th,
                pd0_refined_rate_lvl,
                sb_stale_vars,
                local_sb_index,
                chain_base,
                unit_results,
                unit_enc_rdoq,
                pd0_inter,
                sb_pd0_max_min,
                ac_bias_eff,
                reference,
            );

            // Keep the per-SB recon list layout for downstream consumers.
            // Append this SB's rows straight from the tile canvas. The
            // staging `vec![0u8; sb_cur_w * sb_cur_h]` this replaces was
            // filled row by row and then copied wholesale into
            // `tile_recon` — an allocation, a zero-fill and a second pass
            // over every pixel of every superblock, for bytes that were
            // already contiguous per row. Same bytes, same order.
            for r in 0..sb_cur_h {
                let src_off = (sb_y0 + r) * w + sb_x0;
                tile_recon.extend_from_slice(&tile_frame_recon[src_off..src_off + sb_cur_w]);
            }
            if let Some(tree) = sb_result.tree {
                // C `update_b`'s `pcs->sb_intra[sb] |= is_intra` /
                // `pcs->sb_skip[sb] &= !block_has_coeff` fold — the
                // values the NEXT superblocks' `pd0_detector` neighbour
                // arms read.
                let (intra, skip) = tree.sb_intra_skip();
                sb_intra_acc[sb_index] = u8::from(intra);
                sb_skip_acc[sb_index] = u8::from(skip);
                tile_trees.push(tree);
                tile_enc_rdoq.push(sb_enc_rdoq);
            }
        }
    }
    // The bd10 FULL-RD canvases hold this tile's committed 10-bit
    // winner recon (`commit_leaf` writes `win_recon10` / `win_*_recon10`
    // into them per block). Outside that envelope they were never
    // allocated. Note `bd10_luma_funnel` alone is not enough: the eff-M9
    // band (p9..p13) allocates the LUMA canvas without the chroma ones,
    // so the complete 3-plane canvas exists exactly at `bd10_full_rd`.
    let tile_canvas10 = if bd10_full_rd {
        // The frame merger consumes aligned-stride canvases. Compact
        // only visible chroma rows after all mode decisions are complete.
        if cwid != acw {
            for plane in [&mut tile_frame_u_recon10, &mut tile_frame_v_recon10] {
                for row in 0..ach {
                    plane.copy_within(row * cwid..row * cwid + acw, row * acw);
                }
            }
        }
        Some((
            tile_frame_recon10,
            tile_frame_u_recon10,
            tile_frame_v_recon10,
        ))
    } else {
        None
    };
    // The reservation above must be EXACT, or it is a different bug from
    // the one it fixes: too small and the doubling is back, too large and
    // the slack it was meant to remove is still there. `sb_cur_w` /
    // `sb_cur_h` are `sb_size.min(w - sb_x0)` / `sb_size.min(h - sb_y0)`
    // and the SB extents partition the tile, so the product below is the
    // sum of their areas.
    debug_assert_eq!(
        tile_recon.len(),
        tile_luma_px,
        "tile_recon reservation is not the final length"
    );
    debug_assert_eq!(
        tile_recon.len(),
        tile_recon.capacity(),
        "tile_recon grew past its reservation"
    );
    Ok((tile_recon, tile_trees, tile_canvas10, tile_enc_rdoq))
}
