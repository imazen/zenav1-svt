use super::*;

/// Encode tile rows, returning per-tile recon buffers.
///
/// When the `std` feature is enabled and there are multiple tile rows,
/// uses `std::thread::scope` for parallel encoding. Otherwise sequential.
#[allow(clippy::type_complexity)] // ported C signature: a `type` alias here would hide the shape and churn the byte-identity gate for no benefit
pub(super) fn encode_tile_rows(
    encode_input: &[u8],
    // Task #95 chunk 2: source padded to the SB extent (== `encode_input` for
    // full-SB frames) + its stride. The PD0 partition search and per-b64
    // variance read from THIS buffer so a partial SB sees C's replicated
    // border instead of stride-wrapping into the next row.
    sb_input: &[u8],
    in_stride: usize,
    w: usize,
    h: usize,
    sb_size: usize,
    sb_cols: usize,
    sb_rows: usize,
    // Task #96: the resolved tile grid — the SAME value the entropy walk
    // and the frame header use, so the MD search, the coded symbols and
    // the signalled geometry can never disagree about where the tile
    // boundaries are.
    tile_grid: crate::entropy::obu::TileGrid,
    base_qindex: u8,
    // Per-plane chroma qindexes (== base_qindex in mainline mode).
    qindex_u: u8,
    qindex_v: u8,
    // Effective AC bias for MD spatial distortion (0.0 = mainline default).
    ac_bias_eff: f64,
    // [SVT_HDR_MODE] per-SB qindex plan (variance boost) + frame chroma
    // AC deltas: the search must quantize each SB at its planned qindex.
    // Also the TPL `r0_delta_qp_md` map — `ctx->qp_index` reads it even
    // when the header signals no delta-q.
    sb_qindex_plan: Option<&[u8]>,
    // C `frm_hdr.delta_q_params.delta_q_present` — the SIGNALLED flag
    // (`delta_q_plan.is_some()` at the caller). The quantizer selection in
    // `quantize_inv_quantize` (full_loop.c:1668-1676) keys on THIS, not on
    // the plan being present: when false every block quantizes at the
    // frame `base_q_idx` + chroma deltas whatever `sb_qindex_plan` says.
    fh_delta_q_present: bool,
    // C `ppcs->blk_lambda_tuning` payload — `pa_me_data->tpl_sb_rdmult_
    // scaling_factors` after `sb_setup_lambda` folded each superblock's
    // rdmult ratio in. `evaluate_leaf` scales the per-BLOCK lambda by the
    // geometric mean over the synth-block cells it covers
    // (`svt_aom_set_tuned_blk_lambda`, coding_loop.c:368).
    tpl_rdmult: Option<alloc::sync::Arc<crate::port_md_lambda::TplRdmult>>,
    chroma_ac_deltas: (i8, i8),
    sharp_tx_active: bool,
    hdr_noise_norm: u8,
    qm_levels: [u8; 3],
    hdr_tx_bias: u8,
    hdr_complex_hvs: bool,
    // Reuse the frame result: derive_sc is pure and encode_input is unchanged.
    // This also preserves the tune-IQ forced detector preset resolved by the
    // caller. Using the raw speed preset here previously desynchronized
    // palette rates and CDF evolution from the real pack at presets >= 8.
    frame_sc: crate::sc_detect::ScDerivation,
    // The arm is also needed by the other per-picture tool ladders below.
    sc_arm: crate::sc_detect::ScArm,
    hdr_alt_ssim: bool,
    hdr_alt_lambda: bool,
    hdr_iq_lambda_weight: Option<u32>,
    ssim_rdmult: Option<&crate::tune::SsimRdmult>,
    fh_base_qindex: u8,
    // FH `tx_mode == TX_MODE_SELECT` — the value `frame_tx_mode_select()`
    // computed for the header, PASSED IN rather than re-derived here. The
    // funnel walk and the per-SB CDF-chain simulation must code exactly the
    // symbols the header announced, and re-deriving would key the qp band on
    // this function's `cli_qp` (== `tpl_adjusted_qp`) where the header keys it
    // on `static_config.qp`: equal at `aq_mode == 0`, not guaranteed
    // otherwise, and a disagreement there is an undecodable stream.
    walk_tx_mode_select: bool,
    cli_qp: u8,
    // C `ppcs->picture_qp = clamp_qp((base_q_idx + 2) >> 2)` (rc_process.c:861)
    // — the qp the frame `lambda_weight` ladder is keyed on. Equal to `cli_qp`
    // unless a fractional CRF put a non-zero `extended_crf_qindex_offset` into
    // the qindex; every LEVEL derivation keeps reading `cli_qp`
    // (`static_config.qp`), which the offset does not move.
    picture_qp: u8,
    // C's extended-CRF lambda bump: `lambda_weight += extended_crf_qindex_offset
    // * 28` when `static_config.qp == MAX_QP_VALUE (63)` and the offset is
    // non-zero (enc_mode_config.c:10109-10114) — i.e. CRF 63.25..70 only.
    // 0 on every other config, which makes it byte-inert there.
    lw_bump: u32,
    // C `static_config.tune == TUNE_IQ`: picks the still-picture
    // `lambda_weight` curve over the PSNR ladder (enc_mode_config.c:10094).
    tune_iq: bool,
    hdr_sharpness: i8,
    _lambda: u64, // unused by the funnel; the legacy per-SB QP-offset pass is gone
    speed_config: &crate::speed_config::SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    // C `ppcs->is_highest_layer` (`crate::port_picstruct::is_highest_layer`,
    // pd_process.c:5560), resolved by the CALLER from the frame's temporal
    // layer and the GOP depth — the same value the DLF / LR ladders read as
    // `!is_not_last_layer`. The funnel's NIC picture type
    // (`set_md_stage_counts`, product_coding_loop.c:1398) is the only reader
    // on this side. FALSE on every picture of a flat GOP.
    is_highest_layer: bool,
    // C `ppcs->temporal_layer_index` — the `is_base` of
    // `sig_deriv_mode_decision_config_default` (enc_mode_config.c:8904):
    // `temporal_layer_index == 0`. Every `is_base` the funnel-apply calls
    // below stamp comes from THIS value; on a flat GOP it is 0 for every
    // picture, which is what the previous literal `true` encoded.
    temporal_layer: u8,
    // The same reference picture with C's replicated margin
    // (docs/INTER-ENCODE-PLAN.md §1s item 4), all three planes — the
    // chroma pair is at the frame's chroma resolution (full-res at 4:4:4).
    // `Some` exactly when `ref_frame_data` is; the inter arm cannot
    // predict without it.
    ref_padded: Option<&crate::picture::PaddedRef>,
    // The INTER branch of mode decision (`docs/INTER-ENCODE-PLAN.md` §1s
    // items 1b/2/3/6): the padded DPB reference, this frame's open-loop
    // motion search, the inter rate tables and the MVP environment. `None`
    // on a key frame.
    inter_md: Option<&crate::inter_md_arm::InterMdFrame<'_>>,
    // The frame-level INTER syntax + MVP environment the CHAIN SIMULATION's
    // entropy context needs, the same two the real pack's `EntropyCtx` is
    // armed with in `encode_frame_impl`. `None` on a key frame.
    //
    // WHY THEY ARE HERE, MEASURED 2026-09-05. `sim_ectx` re-codes every
    // superblock to evolve the per-SB frame contexts, and it reaches
    // `encode_block_syntax`'s inter arm on any frame where the chain runs.
    // The chain's gate is `use_funnel && update_cdf_level(..) != 0 &&
    // multi_sb`, and `svt_aom_get_update_cdf_level_default`
    // (`inter_mv_code::update_cdf_level_default`) is non-zero on an INTER
    // frame only at `enc_mode <= 3` — exactly the preset band the global
    // motion refusal used to make unreachable. So `sim_ectx` was never armed
    // and the first inter block PANICKED ("an inter block on a frame with no
    // inter frame-syntax state", the `.expect` at the inter arm of
    // `encode_block_syntax`) on every multi-superblock cell at presets 0..3.
    // Reproduced on {uniform,gradient,diag,screen} 128x128 q40 p0 and p2,
    // frames=2; p4 does not chain (level 0 on an inter frame) and 64x64 is
    // single-SB, which is why the p4/64x64 cells passed.
    //
    // Arming it cannot move a byte on any cell that encodes today: before
    // this, every cell that reached the arm crashed.
    sim_inter_syntax: Option<&InterSyntaxState>,
    sim_inter_mvp_env: Option<&crate::partition::InterMdEnv>,
    // C `set_blocks_to_be_tested`'s per-SB `min_sq_size`
    // (enc_dec_process.c:1485) — what `depth_removal_ctrls` decides. Indexed
    // by RASTER superblock (`sb_row * sb_cols + sb_col`), like `all_trees`.
    // `None` on a key frame, where the controls are `enabled = 0`.
    pd0_min_sq: Option<&[u8]>,
    // The full per-SB `DepthRemovalResult` the `pd0_min_sq` fold was computed
    // from — `sig_deriv_enc_dec_pd0`'s `subres_level` ladder
    // (enc_mode_config.c:7344) reads `depth_removal_ctrls` and the post-call
    // `disallow_4x4`, both per superblock. `None` on a key frame.
    pd0_dr_res: Option<&[crate::port_enc_mode_config::common::DepthRemovalResult]>,
    // C `update_pred_th_offset`'s `use_ref_info` read (enc_dec_process.c:
    // 1614-1629): LAST reference's `sb_min_sq_size` / `sb_max_sq_size`
    // arrays, indexed by raster `sb_index`. `None` on a key frame — C's
    // `slice_type == I_SLICE || !is_ref_l0_avail` arm.
    ref_min_max_sq: Option<(&[u8], &[u8])>,
    // C `av1_lambda_assign_md` per SUPERBLOCK on an inter frame
    // (`svt_aom_mode_decision_configure_sb`, md_process.c:796). `None` on a
    // key frame and on every allintra cell, which is what keeps the still
    // envelope byte-identical by construction.
    sb_inter_lambda: Option<&[crate::pd0::SbInterLambda]>,
    // C `pcs->md_frame_context` (`init_frame_rate_tables`,
    // md_config_process.c:292-310) — §1s item 8. When the frame header names
    // a `primary_ref_frame`, MODE DECISION prices against THAT REFERENCE's
    // saved end-of-frame CDFs, not the defaults. `None` reproduces C's
    // `PRIMARY_REF_NONE` arm: `svt_av1_default_coef_probs(base_q_idx)` +
    // `svt_aom_init_mode_probs`, which is what the still path has always
    // built.
    md_frame_cdfs: Option<&crate::port_frame_cdf::FrameCdfs>,
    mv_map: &[svtav1_types::motion::Mv],
    mv_map_stride: usize,
    chroma_420: bool,
    // The frame's chroma format — the `ss_x`/`ss_y` base for every
    // chroma-geometry derivation below (chroma canvas dims, tile-clip
    // origins, EntropyCtx sizing). `chroma_420` stays as the "chroma
    // planes are present AND this is the C-parity surface" flag; a
    // non-420 format threads through here while `chroma_420` stays
    // false so the funnel (which is 4:2:0-only) never arms.
    chroma_format: svtav1_types::chroma::ChromaFormat,
    c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
    chroma_src: Option<(&[u8], &[u8])>,
    // Encode bit depth (8 or 10). At bd10 the partition search runs C's
    // hbd-forced PD0_LVL_0 (full-RD), NOT the preset's LVL_6/LVL_5 heuristic
    // (set_pd0_ctrls, enc_mode_config.c:5415). The tree is still decided at
    // 8-bit on the MSB-truncated plane; only the coded levels go 10-bit
    // (bd10_reencode_luma).
    bit_depth: u8,
    // Task #6 chunk 1: native 10-bit SOURCE planes (Y, U, V) for the bd10 MD
    // funnel — frame-strided over the ALIGNED frame (`w`, `w/2`). `Some` only
    // when the caller entered through `try_encode_frame_420_hbd`; every u8
    // path passes `None` and the funnel keeps widening `u8 << 2`.
    hbd_src: Option<(&[u16], &[u16], &[u16])>,
    // Set when the funnel actually consumed `hbd_src` (i.e. the bd10 luma
    // funnel armed). Read by `encode_frame_impl` to reject an encode that
    // would have silently dropped the caller's low bits.
    hbd_used: &core::sync::atomic::AtomicBool,
    // Superres chunk B.4: C's `pcs->variance` as picture analysis left it
    // (FULL-RESOLUTION b64 grid, raster order), read here through the CODED
    // grid's linear SB index — the indexing C itself does after
    // `scale_pcs_params` re-inits the geometry without rebuilding the array.
    // `None` on every non-superres path.
    stale_vars: Option<&[crate::pd0::SbVariance]>,
    // C `static_config.max_tx_size` (32 or 64) — tune IQ sets 32 at qp <= 45
    // (enc_handle.c:4914) and the partition search then refuses 64x64 squares
    // (enc_dec_process.c:1494-1495).
    max_tx_size: u8,
    // Issue #5: the frame is CODED-LOSSLESS (base_q_idx 0, spec 5.9.2). Selects
    // the forced 8x8 / TX_4X4 partition tree (`pd0::lossless_tree`) and the
    // funnel's lossless arms (`FunnelFrame::coded_lossless`,
    // `FunnelCfg::apply_coded_lossless`).
    coded_lossless: bool,
    // ZenEnhancement::DeepSearch: on the allintra arm, evaluate the
    // search-effort ladders below (intra/txs/funnel/nic/encdec/mds0 arms
    // and `DrCtrls`) at `enc_mode -1` regardless of `speed_config.preset`.
    // Byte-inert at preset -1 and on every video frame.
    deep_search: bool,
    reference: crate::reference::SvtReference,
    // Feature 4 (bounded threading): the maximum number of OS threads the
    // tile loop below may run at once (0 = auto via `available_parallelism`).
    // Bounds CONCURRENCY only — tiles are always joined and appended in
    // tile-index order — so the returned per-tile results (and the emitted
    // bytes) are identical for any value.
    thread_count: usize,
    // C `pd0_detector`'s frame-level + reference-side inputs
    // (enc_dec_process.c:2406-2470) — `transition_present`,
    // `ref_intra_percentage`, and the two lists' slot-0 references'
    // `sb_intra` arrays. The per-SB fields (`me_*`, this frame's coded
    // left/top neighbours) are gathered in the SB loop itself.
    pd0_det_frame: crate::part_arm::Pd0DetFrame<'_>,
    // C `md_encode_block`'s Light-PD1 dispatch inputs — the FRAME-level half
    // (`resolve_sb_lpd1` adds the per-SB pieces inside the walk). `None` on a
    // key frame (`md_config_signals` absent → `pic_lpd1_lvl` 0) and on every
    // allintra cell, which is what keeps the still envelope byte-neutral.
    lpd1_frame: Option<Lpd1FrameIn>,
    // Feature 1: cooperative cancellation, checked at the head of every SB
    // row of the MD search (the heaviest per-frame loop). Passed as
    // `&dyn Stop` so the threaded per-tile closure stays `Send` (the trait
    // is `Send + Sync`); the default `Unstoppable` token's `may_stop()` is
    // `false`, so the guarded check compiles to a cheap false-branch and the
    // search output stays byte-identical.
    stop: &dyn enough::Stop,
) -> crate::EncodeResult<
    Vec<(
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        // bd10 FULL-RD only: this tile's committed 10-bit winner recon, as
        // SB-extent-SIZED but ALIGNED-STRIDED (`w` / `w/2`) Y/U/V canvases with
        // only this tile's SB region written. The extra size absorbs a
        // right-straddle write's wrap; the stride is the aligned width, exactly
        // like the u8 `tile_frame_recon`. `None` outside the bd10 full-RD
        // envelope. See the merge site.
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        // Per-SB encode-pass RDOQ enable, aligned with `tile_trees`: C's
        // `ed_ctx->md_ctx->rdoq_ctrls->enabled`, resolved per superblock by
        // the SAME `sig_deriv_enc_dec` arm the mode-decision walk ran — a
        // light-PD1 superblock's row can turn RDOQ off
        // (`sig_deriv_enc_dec_light_pd1_default`, `lpd1 > LPD1_LVL_4` →
        // level 0) where the frame-level `pcs->rdoq_level` kept it. The bd10
        // re-encode post-pass quantizes under this value.
        Vec<bool>,
    )>,
> {
    // Mode-decision chroma blocks can cross the aligned right edge. Give
    // sources and reconstruction canvases a real SB-wide stride; extra rows
    // alone let right-edge reads wrap into unrelated samples in the next row.
    // All chroma dims derive from `chroma_format` (`>> ss`), never a local /2.
    let (acw, ach) = (
        chroma_format.chroma_width(w),
        chroma_format.chroma_height(h),
    );
    let md_cw = chroma_format.chroma_width(w.div_ceil(sb_size) * sb_size);
    let md_ch = chroma_format.chroma_height(h.div_ceil(sb_size) * sb_size);
    let padded_chroma = chroma_src
        .filter(|_| md_cw != acw)
        .map(|(u, v)| {
            Ok::<_, whereat::At<crate::EncodeError>>((
                pad_plane_replicate(u, acw, acw, ach, md_cw, md_ch)?,
                pad_plane_replicate(v, acw, acw, ach, md_cw, md_ch)?,
            ))
        })
        .transpose()?;
    let chroma_src = padded_chroma
        .as_ref()
        .map(|(u, v)| (u.as_slice(), v.as_slice()))
        .or(chroma_src);
    let padded_chroma10 = hbd_src
        .filter(|(_, u, _)| md_cw != acw && !u.is_empty())
        .map(|(_, u, v)| {
            Ok::<_, whereat::At<crate::EncodeError>>((
                pad_plane_replicate_u16(u, acw, acw, ach, md_cw, md_ch)?,
                pad_plane_replicate_u16(v, acw, acw, ach, md_cw, md_ch)?,
            ))
        })
        .transpose()?;
    let hbd_src = hbd_src.map(|(y, u, v)| match padded_chroma10.as_ref() {
        Some((pu, pv)) => (y, pu.as_slice(), pv.as_slice()),
        None => (y, u, v),
    });
    let encode_one_tile = |tile_idx: usize| -> crate::EncodeResult<(
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        Vec<bool>,
    )> {
        encode_one_tile_body(
            encode_input,
            sb_input,
            in_stride,
            w,
            h,
            sb_size,
            sb_cols,
            sb_rows,
            tile_grid,
            base_qindex,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            sb_qindex_plan,
            fh_delta_q_present,
            &tpl_rdmult,
            chroma_ac_deltas,
            sharp_tx_active,
            hdr_noise_norm,
            qm_levels,
            hdr_tx_bias,
            hdr_complex_hvs,
            frame_sc,
            sc_arm,
            hdr_alt_ssim,
            hdr_alt_lambda,
            hdr_iq_lambda_weight,
            ssim_rdmult,
            fh_base_qindex,
            walk_tx_mode_select,
            cli_qp,
            picture_qp,
            lw_bump,
            tune_iq,
            hdr_sharpness,
            speed_config,
            ref_frame_data,
            is_highest_layer,
            temporal_layer,
            ref_padded,
            inter_md,
            sim_inter_syntax,
            sim_inter_mvp_env,
            pd0_min_sq,
            pd0_dr_res,
            ref_min_max_sq,
            sb_inter_lambda,
            md_frame_cdfs,
            mv_map,
            mv_map_stride,
            chroma_420,
            chroma_format,
            &c_quant,
            bit_depth,
            hbd_used,
            stale_vars,
            max_tx_size,
            coded_lossless,
            deep_search,
            reference,
            pd0_det_frame,
            lpd1_frame,
            stop,
            acw,
            ach,
            md_cw,
            chroma_src,
            hbd_src,
            tile_idx,
        )
    };

    // Parallel encoding with std::thread::scope when available, BOUNDED to
    // `thread_count` concurrent OS threads (Feature 4). Previously every tile
    // (up to 256) was spawned at once; now tiles run in fixed-size waves so a
    // heavily-tiled frame cannot oversubscribe the box. Order-preserving:
    // each wave's handles are joined and pushed in tile-index order, and the
    // waves themselves advance in order, so the assembled `Vec` is in exact
    // tile-index order — byte-identical to the old all-at-once collect for any
    // `thread_count`.
    #[cfg(feature = "std")]
    if tile_grid.num_tiles() > 1 {
        let num_tiles = tile_grid.num_tiles();
        let limit = match thread_count {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        }
        .clamp(1, num_tiles);
        return std::thread::scope(|s| {
            // Each tile's closure now yields an `EncodeResult`; collect them in
            // tile-index order and short-circuit to the FIRST error (in tile
            // order). On the success/default path every element is `Ok`, so the
            // collect is byte-identical to the previous `Vec` assembly.
            let mut results = Vec::with_capacity(num_tiles);
            let mut start = 0;
            while start < num_tiles {
                let end = (start + limit).min(num_tiles);
                let handles: Vec<_> = (start..end)
                    .map(|tile_idx| s.spawn(move || encode_one_tile(tile_idx)))
                    .collect();
                for h in handles {
                    results.push(h.join().unwrap());
                }
                start = end;
            }
            results.into_iter().collect()
        });
    }

    // Sequential fallback (single tile, or no-std build).
    (0..tile_grid.num_tiles()).map(encode_one_tile).collect()
}

mod cu;
use cu::*;

mod tile_body;
use tile_body::*;
