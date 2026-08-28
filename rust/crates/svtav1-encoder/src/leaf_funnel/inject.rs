//! Candidate injection and the MDS0 fast loop.
//!
//! C `generate_md_stage_0_cand` (mode_decision.c:3621) and the three injectors
//! it drives, in C's order: regular intra modes DC..`intra_mode_end` with the
//! angular-delta inner loop, then filter-intra
//! (`inject_filter_intra_candidates`), then palette
//! (`inject_palette_candidates`, :3356), then IntraBC
//! (`inject_intra_bc_candidates`). Each candidate is predicted whole-block and
//! scored with `fast_loop_core`'s Hadamard SATD fast cost
//! (product_coding_loop.c:1258).
//!
//! Split out of `evaluate_leaf` on 2026-08-25. The body is VERBATIM -- the
//! carriers are destructured back into the same local names at the top, so the
//! moved code needed no edits and the diff is checkable by comparing it to the
//! original line range.

use super::*;

/// Inject every candidate class and score each one's MDS0 fast cost.
///
/// Returns the candidate list in C's PROCESSING order, which is load-bearing:
/// the MDS0 replacement pool's argmax-victim tie rule reads it (see [`nic`]),
/// so a stable re-sort here would change which of two fast-cost-tied
/// candidates survives.
///
/// `ind_uv` is threaded by `&mut` because it is written in two different
/// stages: here, when the M0/M1 chroma config runs the independent-uv search
/// BEFORE MDS0 (`ind_uv_last_mds == 0`, product_coding_loop.c:9260, so every
/// candidate's fast cost prices its FINAL uv pair), and again at MDS3 for
/// every other config.
#[allow(clippy::too_many_arguments)]
pub(super) fn inject_candidates(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    bd: LeafBd10<'_>,
    pal: PalFlagRates,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    dc_only: bool,
    ind_uv: &mut Option<[(u8, i8); 13]>,
) -> Vec<Cand> {
    // Destructure the carriers back into the names the moved body uses, so the
    // body itself is byte-for-byte what it was inside `evaluate_leaf`.
    let frame = fx.frame;
    let rates = fx.rates;
    let cfg = frame.cfg;
    let LeafGeom {
        w,
        h,
        abs_x,
        abs_y,
        has_uv,
        y_geom,
        filt_type_y,
        bsize_idx,
        cfl_allowed,
        use_angle,
        fi_allowed_bsize,
        above_ctx,
        left_ctx,
        // `skip_ctx` and `aligned_dims` are MDS3 inputs; injection prices no
        // residual and takes no distortion crop, so it reads neither.
        ..
    } = *g;
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    let uv_geom = cx.uv_geom;
    let filt_type_uv = cx.filt_type_uv;
    let PalFlagRates {
        // `allow` only gates the rates below, which are already 0 when it is
        // false, so injection reads the rates and not the flag.
        allow: _,
        mode_ctx: pal_mode_ctx,
        y_no: pal_y_no,
        uv_no: pal_uv_no,
        uv_no_y1: pal_uv_no_y1,
    } = pal;
    let bd10_funnel = bd.active;
    let blk_y_src10 = bd.blk_y_src10;
    let bd10_rd = bd.rd;
    let lambda_bd10_fast = bd.lambda_fast;

    // C: at ind_uv_last_mds == 0 (the M0/M1 chroma config) the independent
    // uv search runs BEFORE MDS0 (product_coding_loop.c:9260, ind_uv_avail=1
    // at injection) so every candidate's MDS0 fast cost prices its FINAL uv
    // pair — which drives the NIC survivor order. The table itself is
    // candidate-independent, so building it here is timing-exact.
    if has_uv && let Some(ind_uv_independent) = cfg.ind_uv_independent {
        // C `search_best_independent_uv_mode` (product_coding_loop.c:7778),
        // chroma_level 1/2 (ind_uv_last_mds 0/1): a FULL independent uv
        // search over ALL uv modes, not just the survivors' uv-follows-luma
        // modes. `perform_ind_uv_search_last_mds` (:7899) is true whenever
        // an intra candidate survived (skip_ind_uv_if_only_dc = 0 here, and
        // the inter-vs-intra arm is I-slice-dead) — so it always runs for
        // our intra blocks.
        let uv_nic = ind_uv_independent as u64;

        // 1. Inject ALL uv modes DC..mode_end with angle deltas, in the C
        //    uv_mode-then-delta order (:7807-7849): angular_pred_level >= 4
        //    skips D45..D67; directional modes get 7 deltas (-3..3) when
        //    use_angle_delta && level <= 2, else 1; |1|/|2| are dropped at
        //    level >= 2 (all inert for M0/M1 at angular_pred_level 1).
        let mut uv_cands: Vec<(u8, i8)> = Vec::new();
        for uvm in 0u8..=cfg.mode_end {
            let directional = matches!(uvm, 1..=8);
            if directional && ((cfg.angular_level >= 4 && uvm >= 3) || cfg.angular_level == 0) {
                continue;
            }
            let ndelta = if use_angle && directional && cfg.angular_level <= 2 {
                7
            } else {
                1
            };
            // Coded-lossless: C skips every uv candidate whose chroma tx
            // type is not DCT_DCT (`search_best_independent_uv_mode`,
            // product_coding_loop.c:7584-7587) — only UV_DC, UV_PAETH and
            // UV_CFL map to DCT (`svt_aom_get_intra_uv_tx_type`).
            if frame.coded_lossless && uv_tx_type(uvm, cw, chh) != cc::DCT_DCT {
                continue;
            }
            for k in 0..ndelta {
                let d: i8 = if ndelta == 1 { 0 } else { k as i8 - 3 };
                if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                    continue;
                }
                uv_cands.push((uvm, d));
            }
        }

        // 2. Fast loop: SAD (u + v) per candidate, NO rate at this stage
        //    (product_coding_loop.c:7604-7674). C's `mds0_dist_type` is
        //    zero-initialized = SAD (never assigned in `Source/Lib`), so BOTH
        //    bit depths score plain SAD — bd8 `svt_nxm_sad_kernel`, bd10
        //    `sad_16b_kernel` — NOT the `vf` variance. The sort order (which
        //    candidates enter the full loop) is decided HERE, so the metric
        //    must match C's SAD or a different candidate SET is admitted.
        // bd10 (task #94, root #1): C runs this fast loop at `hbd_md` too — the
        // 10-bit prediction scored by `sad_16b_kernel` on the 10-bit source.
        let mut u_pred = alloc::vec![0u8; cw * chh];
        let mut v_pred = alloc::vec![0u8; cw * chh];
        let mut u_pred10 = alloc::vec![0u16; cw * chh];
        let mut v_pred10 = alloc::vec![0u16; cw * chh];
        let mut fast: Vec<(u64, usize)> = Vec::with_capacity(uv_cands.len());
        for (idx, &(uvm, uvd)) in uv_cands.iter().enumerate() {
            // Both bit depths score SAD (`mds0_dist_type` default 0 = SAD);
            // it is the fast-loop sort key below.
            let fast_dist = match bd10_rd.as_ref() {
                Some(b) => {
                    predict_unit_hbd(
                        fx.u_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut u_pred10,
                        b.bd,
                    );
                    predict_unit_hbd(
                        fx.v_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut v_pred10,
                        b.bd,
                    );
                    residual_sad_hbd(&b.u_src10, cw, 0, 0, &u_pred10, cw, chh)
                        + residual_sad_hbd(&b.v_src10, cw, 0, 0, &v_pred10, cw, chh)
                }
                None => {
                    predict_unit(
                        fx.u_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut u_pred,
                    );
                    predict_unit(
                        fx.v_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut v_pred,
                    );
                    residual_sad(fx.u_src, fx.c_stride, ccx, ccy, &u_pred, cw, chh)
                        + residual_sad(fx.v_src, fx.c_stride, ccx, ccy, &v_pred, cw, chh)
                }
            };
            fast.push((fast_dist, idx));
        }

        // 3. Sort by fast cost. C `sort_fast_cost_based_candidates`
        //    (product_coding_loop.c:1415, called by the ind-uv search at
        //    :7680) is a swap-on-`<` selection sort:
        //    `for i { for j>i { if cost[j] < cost[i] swap(i,j) } }`. It is NOT
        //    stable — a swap displaces the element at `i` down to `j`, so
        //    equal-cost candidates do NOT keep injection order, and which of a
        //    SAD tie group (e.g. the three `cbd=96` D45 deltas) lands inside
        //    `nfl` is decided by this exact ordering. BOTH bit depths must
        //    replicate C bit-for-bit. (The bd8 arm briefly kept a stable
        //    `sort_by_key`, believed byte-inert from the then-green gates —
        //    WRONG on real content: flat-chroma SAD tie groups straddle the
        //    nfl cut constantly, admitting a different full-loop SET. Two
        //    independent witnesses, same day:
        //    - CID22 1200348 512x512 q32 p0 at org=(192,128) 32x32 — C fully
        //      evaluates (V,-3) but never (V,0); the stable port did the
        //      opposite, flipping the coded chroma angle delta and cascading
        //      into every later chroma DC base in SB(1,1)+.
        //    - codec_wiki 512^2 p0 q32 (16x16 at mi(4,24)) — C's exchange
        //      order kept UV_SMOOTH inside the 32-survivor cut where the
        //      stable order kept an extra D113 delta, so the whole ind-uv
        //      table and every MDS0 fast cost pricing it diverged.)
        {
            let n = fast.len();
            for i in 0..n.saturating_sub(1) {
                for j in (i + 1)..n {
                    if fast[j].0 < fast[i].0 {
                        fast.swap(i, j);
                    }
                }
            }
        }

        // 4. Full-loop count: allintra path -> base is_highest_layer ? 16
        //    : 32 (:7919). Under OPT_USE_HL0_FLAT a still KF (temporal layer
        //    0, hierarchical_levels 0) has is_highest_layer = FALSE
        //    (pd_process.c:6212: `(tli == hl) && hl != 0`), so base = 32;
        //    scaled by uv_nic_scaling_num/16, min 1 (:7919-7925). UV_DC is
        //    always tested (:7927-7947); it is injected first (sorted index
        //    0 on the flat-chroma tie) so it is already within the first
        //    nfl, but the explicit force is kept for content where DC sorts
        //    late. -> nfl = 16 at M1 (uv_nic 8), 32 at M0 (uv_nic 16).
        let mut nfl = div_round(32 * uv_nic, 16).max(1) as usize;
        nfl = nfl.min(uv_cands.len()).max(1);
        let mut set: Vec<(u8, i8)> = fast.iter().take(nfl).map(|&(_, i)| uv_cands[i]).collect();
        if !set.iter().any(|&(m, _)| m == 0) {
            set.push((0, 0));
        }

        // 5. Full loop: coeff_rate + SSD distortion per uv candidate
        //    (:7949-8003).
        let mut uv_rd: Vec<(u8, i8, u64, u64)> = Vec::with_capacity(set.len());
        for &(uvm, uvd) in &set {
            // bd10 (root #1): the full loop is `svt_aom_full_loop_uv` at
            // `hbd_md` (product_coding_loop.c:7523 full_lambda, 10-bit pred/
            // residual/distortion), same as the mds3-uv fix. bd8 keeps the u8
            // `chroma_eval` (the `None` arm is the original code).
            let (bits, dist) = match bd10_rd.as_ref() {
                Some(b) => {
                    let (u_out, v_out) = chroma::eval_uv_hbd(cx, fx, b, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
                None => {
                    let (u_out, v_out) = chroma::eval_uv(cx, fx, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
            };
            uv_rd.push((uvm, uvd, bits, dist));
            #[cfg(feature = "std")]
            if crate::dbgenv::nsqdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG UVRD mi=({},{}) {}x{} uv={uvm} uvd={uvd} bits={bits} dist={dist}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                );
            }
        }

        // 6. Per luma mode: best uv by RD with the uv rate conditioned on
        //    the (real) luma mode (:8005-8039). All luma modes DC..mode_end
        //    get an entry (no directional skip at angular_pred_level 1); the
        //    rewrite below reads only the surviving luma modes.
        // bd10 (root #1): C prices this compare with the SAME full_lambda the
        // 10-bit full loop used (`full_lambda_md[EB_10_BIT_MD]`, :7523/:7994),
        // matching the 10-bit `uv_rd` above; bd8 keeps the u8 `lambda`.
        let uv_lambda = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
        let mut table = [(0u8, 0i8); 13];
        for luma in 0..=(cfg.mode_end as usize) {
            let mut best_cost = u64::MAX;
            for &(uvm, uvd, bits, dist) in &uv_rd {
                let mut fcr2 = rates.uv[cfl_allowed][luma][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr2 += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                if uvm == 0 {
                    fcr2 += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                let cost = rdcost(uv_lambda, bits + fcr2, dist);
                if cost < best_cost {
                    best_cost = cost;
                    table[luma] = (uvm, uvd);
                }
            }
        }
        *ind_uv = Some(table);
    }
    #[cfg(feature = "std")]
    if crate::dbgenv::nsqdbg()
        && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
        && let Some(t) = &ind_uv
    {
        eprintln!(
            "NSQDBG UVTAB mi=({},{}) {}x{} t={:?}",
            abs_y / 4,
            abs_x / 4,
            w,
            h,
            t
        );
    }
    let fi_elig = cfg.filter_intra && fi_allowed_bsize;
    let mut cand_modes: Vec<(u8, i8, u8)> = Vec::new();
    if dc_only {
        // eff-M9 dc_cand_only injection: exactly {DC_PRED}, no filter-intra.
        cand_modes.push((0, 0, FI_NONE));
    } else {
        for mode in 0..=cfg.mode_end {
            let directional = matches!(mode, 1..=8);
            // directional_mode_skip_mask at angular_pred_level >= 4 masks
            // D45_PRED (3) .. D67_PRED (8) — V/H stay
            // (inject_intra_candidates, mode_decision.c:3246-3250).
            if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
                continue;
            }
            if directional && cfg.angular_level <= 2 && use_angle {
                for d in -3i8..=3 {
                    if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                        continue;
                    }
                    cand_modes.push((mode, d, FI_NONE));
                }
            } else {
                cand_modes.push((mode, 0, FI_NONE));
            }
        }
    }
    if fi_elig && !dc_only {
        // Inject FILTER_DC_PRED..max_filter_intra_mode (each is a DC_PRED
        // block carrying filter_intra_mode 0..N). fi_max 0 = FILTER_DC only
        // (M1..M6); fi_max 4 = all five filter-intra modes (M0, filter_intra
        // level 1). inject_filter_intra_candidates, mode_decision.c:3318-3330.
        for fi_mode in 0..=cfg.fi_max {
            cand_modes.push((0, 0, fi_mode));
        }
    }
    // Coded-lossless (issue #5): C's regular / filter-intra / palette
    // injection loops all `continue` past a candidate whose CHROMA tx type is
    // not DCT_DCT (mode_decision.c:3245-3247, :3298-3300, :3393-3395) — the
    // check uses the candidate's uv pair (uv-follows-luma, or the independent
    // table when `ind_uv_avail`) and runs whether or not the block carries
    // chroma. With `svt_aom_get_intra_uv_tx_type` only UV_DC / UV_PAETH /
    // UV_CFL are DCT, so at qp 0 the regular set collapses to {DC, PAETH}
    // (+ the filter modes that map to DC/PAETH). The filter runs on the
    // injection LIST so `prune_best_mode` below sees the same sequence C's
    // fast loop does. Palette candidates carry UV_DC and always pass.
    if frame.coded_lossless {
        cand_modes.retain(|&(mode, _delta, fi)| {
            let map_mode = if fi != FI_NONE {
                FIMODE_TO_INTRAMODE[fi as usize]
            } else {
                mode
            };
            let uv = match ind_uv.as_ref() {
                Some(tbl) if !cfg.ind_uv_last_mds1 => tbl[map_mode as usize].0,
                _ => uv_from_y(map_mode),
            };
            uv_tx_type(uv, cw, chh) == cc::DCT_DCT
        });
    }

    // C `mds0_use_hadamard_blk` (product_coding_loop.c:9473):
    //
    //     ctx->mds0_use_hadamard_blk =
    //         ctx->mds0_use_hadamard_sb && fast_candidate_total_count > 1;
    //
    // `mds0_use_hadamard_sb` is true on the all-intra path
    // (enc_mode_config.c:8148, svt_aom_sig_deriv_enc_dec_allintra), so the live
    // term is the injected-candidate count. When it is 1, C's `fast_loop_core`
    // takes the VARIANCE arm (:1296-1306) instead of `hadamard_path` (:1283) —
    // both then shift by 4 and feed the same fast cost. At preset >= 9 the
    // `dc_only` gate injects exactly {DC_PRED}, so C runs NO Hadamard there at
    // all: profiling C at 512x512 and 1024x1024 preset 10 found ZERO samples in
    // any hadamard/satd symbol across 7,126 and 19,073 samples respectively,
    // while svt_aom_variance*_neon_dotprod appeared in both
    // (benchmarks/perf_class_attrib_2026-08-13.meta). The port was computing the
    // Hadamard SATD unconditionally — 4.8 % (512^2) / 5.1 % (1024^2) of its
    // whole frame at p10.
    //
    // `fast_candidate_total_count` in C counts EVERY injected candidate, and C
    // injects all of them before `md_stage_0` runs. This funnel interleaves
    // injection with evaluation, so the palette and intra-BC candidate counts
    // are not knowable here (the palette count is an output of the k-means
    // search below). The count is therefore OVER-approximated: whenever palette
    // or IBC injection can run at all, the Hadamard arm is kept. That direction
    // is byte-safe by domination — it can only preserve the pre-existing
    // behaviour on blocks where C would have used variance, which is exactly
    // what shipped and passed 168/168 byte identity before this change; it can
    // never take the variance arm on a block where C takes the Hadamard one.
    let palette_can_inject =
        crate::entropy::context::allow_palette(cfg.allow_sct, w, h) && cfg.palette_level > 0;
    let mds0_use_hadamard = cand_modes.len() > 1 || palette_can_inject || cfg.allow_intrabc;

    let mut cands: Vec<Cand> = Vec::with_capacity(cand_modes.len());
    // MDS0 with `prune_using_best_mode` (product_coding_loop.c:1680-1737):
    // candidates are evaluated in injection order; the running best REGULAR
    // (class-0, non-filter-intra) mode by fast cost is tracked and used to
    // SKIP later candidates — H when V is currently best, SMOOTH when DC is
    // still best. Skipped candidates never get a fast cost (never enter the
    // pool). At M6 (prune off) every candidate is evaluated, identical to
    // the original funnel.
    let mut best_reg_cost = u64::MAX;
    let mut best_reg_mode: i32 = -1;
    for &(mode, delta, fi) in &cand_modes {
        if cfg.prune_best_mode && fi == FI_NONE {
            // intra_mode_end SMOOTH >= H_PRED, so the gate is armed.
            if mode == 2 && best_reg_mode == 1 {
                continue; // V better than DC -> skip H
            }
            if mode == 9 && best_reg_mode == 0 {
                continue; // DC still best -> skip SMOOTH
            }
        }
        // C injection (inject_intra_candidates / inject_filter_intra_candidates,
        // mode_decision.c:3286-3292): uv = ind_uv_avail ? best_uv_mode[map]
        // : intra_luma_to_chroma[map], angle_uv = ind_uv_avail ?
        // best_uv_angle[map] : angle_y — with map = fimode_to_intramode[fi]
        // for FILTER candidates (their coded luma mode is DC, but the chroma
        // follows the fi-mapped DIRECTION). ind_uv_avail at injection is 1
        // exactly for the ind_uv_last_mds==0 (independent) presets, whose
        // table was built above; the ind_uv_mds3 presets stay on the
        // luma_to_chroma mapping here and rewrite at MDS3 (C :7063).
        let map_mode = if fi != FI_NONE {
            FIMODE_TO_INTRAMODE[fi as usize]
        } else {
            mode
        };
        // At ind_uv_last_mds==1 (M1) the C search hasn't run yet at
        // injection time (`ind_uv_avail` = 0, site :9477 is pre-MDS3), so
        // candidates inject uv-follows-luma and only the MDS3 rewrite
        // applies the table.
        let (uv, uv_delta) = match &ind_uv {
            Some(tbl) if !cfg.ind_uv_last_mds1 => tbl[map_mode as usize],
            _ => (uv_from_y(map_mode), if fi != FI_NONE { 0 } else { delta }),
        };
        let mut pred = vec![0u8; w * h];
        predict_unit(
            y_recon,
            y_stride,
            abs_x,
            abs_y,
            w,
            h,
            mode,
            delta,
            fi,
            &y_geom,
            cfg.edge_filter,
            filt_type_y,
            &mut pred,
        );
        // [SVT_HDR_MODE] complex-hvs: plain whole-block spatial SSD, no
        // shift (C fast_loop_core SSD arm). SATD path shifts << 4 below.
        // PORT-NOTE(unverified): fork mds0 SSD fast cost vs C — verify by
        // a C-side fast_loop_core dump once the C hybrid carries the
        // fork's set_mds0_controls case 3 (the hybrid currently assert(0)s
        // on mds0_level 3; see docs/HDR-ON-4.2.md complex-hvs row).
        let satd = if frame.mds0_ssd {
            let mut sse: u64 = 0;
            for r in 0..h {
                let srow = y_src_off + r * y_src_stride;
                for c in 0..w {
                    let d = i64::from(y_src[srow + c]) - i64::from(pred[r * w + c]);
                    sse += (d * d) as u64;
                }
            }
            sse
        } else if mds0_use_hadamard {
            hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
        } else {
            // C fast_loop_core's variance arm (product_coding_loop.c:1296-1302):
            // `fn_ptr->vf(pred, pred_stride, src, src_stride, &sse)` with
            // `fn_ptr = &svt_aom_mefn_ptr[bsize]`, i.e. svt_aom_variance{W}x{H}.
            // Argument order is (pred, src); the metric is symmetric in the two
            // buffers (sse is, and only sum^2 is used), so this matches.
            u64::from(svtav1_dsp::variance::variance_diff(
                &pred,
                w,
                &y_src[y_src_off..],
                y_src_stride,
                w,
                h,
            ))
        };

        let mut flr = rates.kf_y[above_ctx][left_ctx][mode as usize] as u64;
        if use_angle && matches!(mode, 1..=8) {
            flr += rates.angle[mode as usize - 1][(3 + delta) as usize] as u64;
        }
        if fi_elig && mode == 0 {
            flr += rates.fi_flag[bsize_idx][usize::from(fi != FI_NONE)] as u64;
            if fi != FI_NONE {
                flr += rates.fi_mode[fi as usize] as u64;
            }
        }
        // No-palette y flag (rd_cost.c:579-585): every DC-coded candidate
        // (fi included) prices palette_ymode_fac_bits[bctx][mode_ctx][0]
        // (via pal_y_no, computed above with the neighbour mode ctx) when
        // allow_palette. pal_y_no is 0 when palette is disallowed.
        if mode == 0 {
            flr += pal_y_no;
        }
        // No-intrabc flag (rd_cost.c:629-631, IBC chunk 3): on an IBC frame
        // EVERY non-IBC candidate's luma rate carries intrabc_fac_bits[0]
        // (the use_intrabc=0 flag the writer codes per block). 0-cost
        // structurally when !allow_intrabc (the C fill is gated the same).
        if cfg.allow_intrabc {
            flr += rates.intrabc_fac_bits[0] as u64;
        }
        let mut fcr = if has_uv {
            rates.uv[cfl_allowed][mode as usize][uv as usize] as u64
        } else {
            // C fast cost: chroma_rate only when ctx->has_uv
            // (av1_intra_fast_cost, rd_cost.c:619).
            0
        };
        if has_uv && use_angle && matches!(uv, 1..=8) {
            fcr += rates.angle[uv as usize - 1][(3 + uv_delta) as usize] as u64;
        }
        if has_uv && uv == 0 {
            fcr += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
        }
        // bd10 mode funnel (task #94): when the bd10 recon canvas is present,
        // score this candidate's MDS0 fast cost at TRUE 10-bit — predict from
        // the 10-bit canvas, SATD the 10-bit residual (`y_src<<2 - pred10`),
        // with the bd10 fast lambda. This re-orders the survivor (C's bd10
        // winner). The rate (flr+fcr) is bit-depth-independent. The u8 `pred`
        // and `satd` above are still computed (MDS1/MDS3 reuse `cand.pred`);
        // only the fast COST switches. `None` (bd8) is the exact u8 path.
        // Diagnostic-only (read by the std-gated NSQDBG PFAST dump below).
        #[cfg(feature = "std")]
        let mut dbg_satd10: u64 = 0;
        #[cfg(feature = "std")]
        let mut dbg_pred0: u16 = 0;
        // The 10-bit prediction is RETAINED (`cand.pred10`) — MDS1/MDS3 need it
        // as their depth-0 predictor, exactly as they reuse the u8 `cand.pred`.
        // It used to be dropped here because only MDS0 ran at bd10.
        let mut pred10: Vec<u16> = Vec::new();
        let fast_cost = match fx.y_recon10.as_deref() {
            Some(canvas10) => {
                pred10 = vec![0u16; w * h];
                predict_unit_hbd(
                    canvas10,
                    y_stride,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    mode,
                    delta,
                    fi,
                    &y_geom,
                    cfg.edge_filter,
                    filt_type_y,
                    &mut pred10,
                    frame.bit_depth,
                );
                let satd10 = hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h);
                #[cfg(feature = "std")]
                {
                    dbg_satd10 = satd10;
                    dbg_pred0 = pred10[0];
                }
                rdcost(lambda_bd10_fast, flr + fcr, satd10 << 4)
            }
            None => rdcost(
                lambda,
                flr + fcr,
                if frame.mds0_ssd { satd } else { satd << 4 },
            ),
        };
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG PFAST mi=({},{}) {}x{} mode={} fi={} delta={} uv={} uvd={} flr={} fcr={} satd={} satd10={} pred10_0={} fast={}",
                abs_y / 4,
                abs_x / 4,
                w,
                h,
                mode,
                fi,
                delta,
                uv,
                uv_delta,
                flr,
                fcr,
                satd,
                dbg_satd10,
                dbg_pred0,
                fast_cost,
            );
        }
        // C updates best_reg_intra_mode after fast_loop_core for regular
        // class-0 candidates when prune is armed (line 1727).
        if cfg.prune_best_mode && fi == FI_NONE && fast_cost < best_reg_cost {
            best_reg_cost = fast_cost;
            best_reg_mode = mode as i32;
        }
        cands.push(Cand {
            mode,
            delta,
            fi,
            uv,
            uv_delta,
            pred,
            pred10,
            flr,
            fcr,
            fast_cost,
            full_cost: u64::MAX,
            mds3_cost_ssim: u64::MAX,
            mds1_has_coeff: false,
            tx_depth: 0,
            txb_q: Vec::new(),
            txb_eob: Vec::new(),
            txb_cul: Vec::new(),
            txb_type: Vec::new(),
            y_recon: Vec::new(),
            y_recon10: Vec::new(),
            u_recon10: Vec::new(),
            v_recon10: Vec::new(),
            y_recon_d0: Vec::new(),
            y_bits: 0,
            y_dist: 0,
            u_q: Vec::new(),
            v_q: Vec::new(),
            u_eob: 0,
            v_eob: 0,
            u_cul: 0,
            v_cul: 0,
            u_recon: Vec::new(),
            v_recon: Vec::new(),
            cfl_alpha_idx: 0,
            cfl_alpha_signs: 0,
            palette: None,
            ibc: None,
            mds3_cost: u64::MAX,
            block_has_coeff: false,
            total_rate: 0,
            full_dist: 0,
        });
    }
    // ---- inject_palette_candidates (mode_decision.c:3356-3406) ----
    // C order: regular+fi intra first, palette after (IBC would follow).
    // PORT-NOTE(unverified): C classes palette CAND_CLASS_3 with its own
    // MDS lanes/pool + class dist-to-cost th 50 (enc_mode_config.c:6775);
    // this funnel is single-class, so palette candidates share the one
    // pool — near-tie survivor sets can differ from C. Verify on the
    // EPICA cells; if a cell diverges on survivor membership, split the
    // pool per class. Neighbor state (mode ctx `pal_mode_ctx` + color cache
    // `pal_cache`) is read from the MD decision grid (stamped by commit_leaf
    // in coding order); both are 0/empty for blocks with no palette
    // neighbours — always true for non-screen content — so those stay
    // byte-identical to the pre-neighbour stub.
    // PALETTE AT 10 BITS (task #94 / #71). C has ONE palette search
    // parameterized by `is16bit` (palette.c:391-399): it reads
    // `pcs->input_frame16bit` instead of `enhanced_pic`, swaps
    // `svt_av1_count_colors` for `svt_av1_count_colors_highbd`, clips centroids
    // with `clip_pixel_highbd` (:310-312), widens the cache-snap threshold by
    // `<< (bit_depth - 8)` (:265), and codes the colour literals at
    // `encoder_bit_depth` (entropy_coding.c:4369, rd_cost.c:600).
    //
    // This funnel used to gate palette injection OUT at bd10 entirely
    // (`!bd10_funnel`), because a surviving palette candidate reached
    // `tx_unit_hbd` with only a u8 prediction and panicked. That was a graceful
    // stand-in for a crash, but its parity cost was never measured: since
    // `bd10_funnel` is true for EVERY 64-aligned bd10 4:2:0 frame at every
    // preset, the port offered ZERO palette candidates where C codes palette
    // blocks, so those leaves resolved to ordinary intra. MEASURED on the
    // production corpus (benchmarks/imazen26_sweep_2026-07-24_summary.tsv):
    // preset 6 bd8 = 515/515 byte-identical but preset 6 bd10 = 380/515, and
    // the 135 failing cells are EXACTLY the eight screen-detecting content
    // classes — the whole M6 bd10 gap was this gate. (At M6 IBC is already off,
    // so M6 bd10 is a pure palette divergence; M0 adds IBC on top.)
    //
    // Now the search runs at the real depth and the candidate carries BOTH
    // predictions: `pred` (u8, for the MDS1/MDS3 u8 stages) and `pred10` (u16,
    // what `tx_unit_hbd` needs). Palette prediction is a position-only colour
    // substitution with no neighbour edges (enc_intra_prediction.c:631-651), so
    // the 10-bit form is the same index map through the 10-bit colours — no new
    // predictor kernel is required, which is why this is a small change rather
    // than an hbd-predictor port.
    //
    // C's `eval_intrabc` narrowing scope (mode_decision.c:3587-3594): the
    // palette-hint coupling reads whether the palette injection RAN for
    // this block and whether it produced any candidate.
    let palette_ran =
        crate::entropy::context::allow_palette(cfg.allow_sct, w, h) && cfg.palette_level > 0;
    let cands_before_palette = cands.len();
    if palette_ran {
        let ctrls = crate::palette::PaletteCtrls::for_level(cfg.palette_level);
        let bctx = crate::entropy::context::palette_bsize_ctx(w, h);
        // Neighbour palette color cache (C svt_get_palette_cache_y): merged
        // above+left palette colours, feeding BOTH the k-means centroid snap
        // (optimize_palette_colors, opt_colors=TRUE) INSIDE the search AND
        // the cache-aware color cost below. Empty => bit-identical search +
        // cost (the n_cache==0 fast paths in index_color_cache /
        // optimize_palette_colors / palette_color_cost_y).
        let pal_cache = crate::pipeline::palette_cache(&*fx.ectx, abs_x, abs_y);
        // C svt_aom_write_uniform_cost (entropy_coding.c:4308):
        // truncated-binary literal bits << AV1_PROB_COST_SHIFT(9).
        let uniform_cost = |n: usize, v: u8| -> u64 {
            let l = usize::BITS - n.leading_zeros(); // get_unsigned_bits
            if l == 0 {
                return 0;
            }
            let m = (1usize << l) - n;
            let bits = if (v as usize) < m { l - 1 } else { l };
            (bits as u64) << 9
        };
        // The funnel receives the source as (plane, stride, block offset);
        // decompose the offset back to plane coords for the search.
        // C picks the source plane by `is16bit = ctx->hbd_md > 0`
        // (palette.c:391-399). `blk_y_src10` is this block's 10-bit luma at
        // stride `w` (the real u16 samples when the caller entered through a
        // `*_hbd` entry point, else the `u8 << 2` widening), so the hbd search
        // reads exactly what C's `input_frame16bit` would give it.
        //
        // C SEARCHES over the IN-FRAME part of the block, not the whole block:
        // `search_palette_luma` (palette.c:401-403) takes its `rows`/`cols`
        // from `svt_aom_get_block_dimensions`' `rows_within_bounds` /
        // `cols_within_bounds` (palette.c:217-245), and feeds exactly those to
        // `svt_av1_count_colors` (:409-411), to the `data[]` / `lb` / `ub` fill
        // (:427-439) and to `av1_calc_indices` (:323) — the index map is then
        // edge-REPLICATED out to the nominal block (`extend_palette_color_map`,
        // :324). Passing the full block instead lets the padded rows/columns
        // beyond the picture edge vote in the colour histogram, the
        // dominant-colour scan, the k-means seed range `[lb, ub]` and every
        // k-means iteration — so a straddling block gets DIFFERENT palette
        // colours than C's, and the colour literals desync the bitstream from
        // C's (issue #15).
        //
        // The RATE side (`map_rows`/`map_cols` below) and the PACK side
        // (`pipeline.rs`, `write_palette_map_tokens`) already cropped; only the
        // SEARCH did not, so the three sites disagreed with each other about
        // which block a palette candidate describes.
        //
        // Identical to `w`/`h` on every 64-aligned frame, where nothing
        // straddles — which is why this was invisible to every gate until the
        // unaligned real-content scan crossed the two axes.
        let pal_rows = h.min(frame.frame_h_px.saturating_sub(abs_y));
        let pal_cols = w.min(frame.frame_w_px.saturating_sub(abs_x));
        let pal_cands = if bd10_funnel {
            crate::palette::search_palette_luma_hbd(
                blk_y_src10,
                w,
                pal_rows,
                pal_cols,
                w,
                h,
                &ctrls,
                &pal_cache,
                frame.base_qindex,
                u32::from(frame.bit_depth),
            )
        } else {
            crate::palette::search_palette_luma(
                y_src,
                y_src_stride,
                y_src_off % y_src_stride,
                y_src_off / y_src_stride,
                pal_rows,
                pal_cols,
                w,
                h,
                &ctrls,
                &pal_cache,
                frame.base_qindex,
            )
        };
        for pc in pal_cands {
            let n = pc.colors.len();
            // Substitution prediction (enc_intra_prediction.c:631-651): a
            // position-only colour lookup, no neighbour edges. At bd10 the
            // colours are 10-bit, so `pred10` is the authoritative prediction
            // and the u8 `pred` is its MSB-truncated twin, kept because the
            // MDS1/MDS3 u8 stages and `commit_leaf` still read `cand.pred`.
            let mut pred = vec![0u8; w * h];
            let mut pred10: Vec<u16> = if bd10_funnel {
                vec![0u16; w * h]
            } else {
                Vec::new()
            };
            let shift = u32::from(frame.bit_depth - 8);
            for (o, &idx) in pc.idx_map.iter().enumerate().take(w * h) {
                let c = pc.colors[idx as usize];
                if bd10_funnel {
                    pred10[o] = c;
                    pred[o] = (c >> shift) as u8;
                } else {
                    pred[o] = c as u8;
                }
            }
            // MDS0 fast distortion at the real depth, mirroring the regular
            // candidates' bd10 arm above (10-bit SATD against the 10-bit
            // source, u8 SATD otherwise).
            let satd = if bd10_funnel {
                hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h)
            } else {
                hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
            };
            // Luma rate: DC mode + fi-off flag (fi eligible blocks price it
            // for every DC candidate) + the palette slice (rd_cost.c:579-605
            // use_palette=1 arm): ymode YES + size + (0,0) uniform + colors
            // + map tokens.
            let r_mode = rates.kf_y[above_ctx][left_ctx][0] as u64;
            // C prices NO filter-intra flag on a palette candidate:
            // svt_aom_filter_intra_allowed (mode_decision.c:106) returns 0
            // whenever palette_size > 0, so the use_filter_intra syntax is
            // never written for a palette block (rd_cost.c pals the DC-mode
            // + palette rate only). The port was adding fi_flag[bsize][0]
            // here, over-pricing every palette candidate by that flag cost
            // (measured 1053 at EPICA 8x8) — a real, agent-verified rate
            // divergence vs C. Palette candidates get zero fi bits.
            let r_fi = 0u64;
            let _ = fi_elig; // (fi eligibility is a DC-candidate concept)
            let r_yes = rates.palette_y_yes[bctx][pal_mode_ctx] as u64;
            let r_size = rates.palette_ysize[bctx][n - 2] as u64;
            let r_uniform = uniform_cost(n, pc.idx_map[0]);
            // Colors (C svt_av1_palette_color_cost_y, palette.c:143-152):
            // one flag bit per neighbour-cache entry (n_cache) + delta-code
            // only the out-of-cache colours; av1_cost_literal shifts the
            // whole total by 9. index_color_cache splits pc.colors on the
            // neighbour cache — at n_cache==0 out == pc.colors, so this is
            // bit-identical to the former empty-cache all-colours cost.
            let mut pal_found = alloc::vec![false; pal_cache.len()];
            let mut pal_out = alloc::vec![0u16; pc.colors.len()];
            let n_out = crate::palette::index_color_cache(
                &pal_cache,
                &pc.colors,
                &mut pal_found,
                &mut pal_out,
            );
            // C passes `scs->static_config.encoder_bit_depth` here
            // (`svt_av1_palette_color_cost_y`, rd_cost.c:600) — the same width
            // the WRITER uses (entropy_coding.c:4369). A hardcoded 8 would
            // under-price every 10-bit palette candidate's colours by 2 bits
            // for the first literal (and shift the whole delta ladder), biasing
            // the palette-vs-regular RD tie.
            let r_colors = ((pal_cache.len() as u64)
                + crate::palette::delta_encode_bits(
                    &pal_out[..n_out],
                    u32::from(frame.bit_depth),
                    1,
                ) as u64)
                << 9;
            let mut map_bits = 0u64;
            // C prices the map over the IN-FRAME part of the block, not the
            // whole block: `get_palette_params_rate` (palette.c:569-580) fills
            // `params->rows` / `params->cols` from `svt_aom_get_block_dimensions`
            // -- the same `rows_within_bounds` / `cols_within_bounds` the PACK
            // side uses (entropy_coding.c:5083). Both sides must agree, or a
            // straddling palette block is priced over rows the writer never
            // emits and the RD tie moves.
            //
            // Identical to `w`/`h` unless the block straddles the aligned
            // extent, which only happens on a partial SB. Same numbers the
            // SEARCH above uses — ONE definition, because the search, the rate
            // and the pack disagreeing about the block's in-frame extent is
            // exactly the defect issue #15 turned out to be.
            let (map_rows, map_cols) = (pal_rows, pal_cols);
            crate::palette::color_map_wavefront(
                &pc.idx_map,
                w, // stride: the FULL block width, only the traversal shrinks
                map_rows,
                map_cols,
                n,
                |_i, _j, ctx, idx| {
                    map_bits += rates.palette_ycolor[n - 2][ctx][idx as usize] as u64;
                },
            );
            // Palette candidates flow through the same svt_aom_intra_fast_cost
            // else-arm tail as regular intra — the no-intrabc flag charge
            // (rd_cost.c:629-631) applies to them identically (IBC chunk 3).
            let r_ibc_no = if cfg.allow_intrabc {
                rates.intrabc_fac_bits[0] as u64
            } else {
                0
            };
            let flr = r_mode + r_fi + r_yes + r_size + r_uniform + r_colors + map_bits + r_ibc_no;
            #[cfg(feature = "std")]
            if crate::dbgenv::palbrk() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG PALBRK mi=({},{}) n={} mode={} fi={} yes={} size={} uniform={} colors={} map={} (63tok? map/512={})",
                    abs_y / 4,
                    abs_x / 4,
                    n,
                    r_mode,
                    r_fi,
                    r_yes,
                    r_size,
                    r_uniform,
                    r_colors,
                    map_bits,
                    map_bits / 512,
                );
                eprintln!(
                    "NSQDBG PALDATA mi=({},{}) n={} colors={:?} idxmap={:?}",
                    abs_y / 4,
                    abs_x / 4,
                    n,
                    pc.colors,
                    pc.idx_map,
                );
            }
            // Chroma: DC (palette-uv unsupported) with the y-palette-ON uv
            // flag row. C prices palette_uv_mode_fac_bits[1][0] here
            // (rd_cost.c:514-521, use_palette_y=1 because this candidate has a
            // luma palette). This is the ONLY leaf-funnel site that takes the
            // [1] row; every regular candidate keeps pal_uv_no ([0]). The port
            // formerly priced [0][0] here too, under-costing the palette
            // candidate's chroma flag (icdf 307 vs the correct 11280) and
            // biasing the palette-vs-regular RD tie toward palette — a #71
            // over-picking contributor (agent-confirmed via the triage drill).
            let (uv, uv_delta) = match &ind_uv {
                Some(tbl) if !cfg.ind_uv_last_mds1 => tbl[0],
                _ => (0u8, 0i8),
            };
            let mut fcr = if has_uv {
                rates.uv[cfl_allowed][0][uv as usize] as u64
            } else {
                0
            };
            if has_uv && use_angle && matches!(uv, 1..=8) {
                fcr += rates.angle[uv as usize - 1][(3 + uv_delta) as usize] as u64;
            }
            if has_uv && uv == 0 {
                fcr += pal_uv_no_y1; // [1][0]: this candidate's luma palette is on
            }
            let fast_cost = rdcost(
                lambda,
                flr + fcr,
                if frame.mds0_ssd { satd } else { satd << 4 },
            );
            #[cfg(feature = "std")]
            if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG PFAST mi=({},{}) {}x{} PAL n={} flr={} fcr={} satd={} fast={}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                    n,
                    flr,
                    fcr,
                    satd,
                    fast_cost,
                );
            }
            cands.push(Cand {
                mds3_cost_ssim: u64::MAX,
                mode: 0,
                delta: 0,
                fi: FI_NONE,
                uv,
                uv_delta,
                pred,
                // The 10-bit substitution prediction (empty at bd8). This is
                // what `tx_unit_hbd` residuals against; it used to be
                // unconditionally empty, which is why a palette candidate
                // reaching the bd10 full-RD stage panicked and why palette was
                // gated out of the bd10 funnel entirely.
                pred10,
                flr,
                fcr,
                fast_cost,
                full_cost: u64::MAX,
                mds1_has_coeff: false,
                tx_depth: 0,
                txb_q: Vec::new(),
                txb_eob: Vec::new(),
                txb_cul: Vec::new(),
                txb_type: Vec::new(),
                y_recon: Vec::new(),
                y_recon10: Vec::new(),
                u_recon10: Vec::new(),
                v_recon10: Vec::new(),
                y_recon_d0: Vec::new(),
                y_bits: 0,
                y_dist: 0,
                u_q: Vec::new(),
                v_q: Vec::new(),
                u_eob: 0,
                v_eob: 0,
                u_cul: 0,
                v_cul: 0,
                u_recon: Vec::new(),
                v_recon: Vec::new(),
                cfl_alpha_idx: 0,
                cfl_alpha_signs: 0,
                palette: Some((pc.colors, pc.idx_map)),
                ibc: None,
                mds3_cost: u64::MAX,
                block_has_coeff: false,
                total_rate: 0,
                full_dist: 0,
            });
        }
    }

    // ---- inject_intra_bc_candidates (IBC chunk 8; mode_decision.c
    //      :3596-3618 gate + :3127-3163 injection + :2976-3126 search) ----
    // IBC AT 10 BITS (task #94 / #71). Formerly `&& !bd10_funnel`, on the
    // grounds that the IBC predictor was u8-only: at bd10 the frame header
    // still carried allow_intrabc while every block coded use_intrabc=0.
    // Decodable, but a guaranteed divergence from C wherever C picks a DV --
    // and the cost is far larger than the palette one it shipped alongside.
    // MEASURED on the gb82-sc screen corpus (512x512 centre crops, q20,
    // port vs real C at bd10, IBC gated out):
    //     terminal  p2  C 7611 B   port 13338 B   +75.2%
    //     terminal  p3  C 7889 B   port 13584 B   +72.2%
    //     windows95 p3  C 13398 B  port 13810 B    +3.1%
    // At bd8 the same cells code 890 / 362 IntraBC blocks, so this is the
    // whole of the delta on copy-friendly content.
    //
    // The DV SEARCH already ran at 8 bits and stays there -- that is C's own
    // asymmetry, not a shortcut (the search reads the source plane, the
    // predictor reads the recon; map SS A.6), and it is the arm the
    // c_parity_intrabc_search / _hash / _mvp differentials pin. What was
    // missing is only the COMPENSATION at 10 bits, which is now generic over
    // `ReconSample` (see intrabc_pred.rs: the bilinear closed forms are
    // provably identical for bd <= 10).
    if cfg.allow_intrabc
        && let (Some(ibc), Some(dvt)) = (fx.ibc, frame.dv_tables.as_ref())
    {
        let gate = fx.ibc_gate;
        let do_ibc = crate::intrabc::do_intra_bc_gate(
            &ibc.ctrls,
            palette_ran,
            (cands.len() - cands_before_palette) as u32,
            gate.is_part_n,
            w.max(h) as i32, // sq_size: only the (allintra-off) b4 gate reads it
            (false, false),  // parent_n0: b4_parent_gating is off at every level
            gate.sibling_n0,
        );
        if do_ibc {
            let mi_row = (abs_y / 4) as i32;
            let mi_col = (abs_x / 4) as i32;
            let grid_stride = ibc.mi_cols;
            let base = mi_row * grid_stride + mi_col;
            // C's MVP scan runs against the live mi state where the
            // CURRENT cell carries the block's own partition (the
            // `has_top_right` VERT_A read) — stamp it before building
            // the stack (commit will overwrite the cell either way).
            let mvp = fx.ibc_mvp.as_deref_mut().expect("ibc_mvp with ibc state");
            mvp[base as usize].partition = gate.partition;
            let stack = {
                let grid = crate::intrabc_mvp::MvpGrid {
                    entries: mvp,
                    stride: grid_stride,
                    base,
                };
                let bctx = crate::intrabc_mvp::derive_block_ctx(
                    mi_row,
                    mi_col,
                    c_bsize_index(w, h),
                    ibc.mi_rows,
                    ibc.mi_cols,
                    ibc.tile,
                    ibc.sb_mi_size,
                );
                crate::intrabc_mvp::generate_mvp_table_intra_frame(&grid, &bctx)
            };
            // dv_ref = nearest/near coercion + find_ref_dv fallback
            // (mode_decision.c:3019-3033); C stamps it back onto
            // ref_mv_stack[INTRA_FRAME][0].this_mv = cand->pred_mv[0].
            let dv_ref =
                crate::intrabc_mvp::compose_dv_ref(&stack, ibc.tile, ibc.sb_mi_size, mi_row);
            // Per-block hash query (square + size-gated), the bucket
            // fetched once and offered to both directions.
            let hash_eligible = crate::intrabc::hash_search_eligible(
                w as i32,
                h as i32,
                ibc.ctrls.max_block_size_hash,
            );
            let (bucket_entries, hv2) = if hash_eligible {
                let mut bufs = crate::intrabc_hash::BlockHashBuffers::default();
                let (hv1, hv2) = crate::intrabc_hash::get_block_hash_value(
                    &y_src[abs_y * y_src_stride + abs_x..],
                    y_src_stride,
                    w,
                    &mut bufs,
                );
                (
                    ibc.hash
                        .bucket(hv1)
                        .iter()
                        .map(|e| crate::intrabc::BlockHashEntry {
                            x: i32::from(e.x),
                            y: i32::from(e.y),
                            hash_value2: e.hash_value2,
                        })
                        .collect::<Vec<_>>(),
                    hv2,
                )
            } else {
                (Vec::new(), 0)
            };
            let buckets: [Option<&[crate::intrabc::BlockHashEntry]>; 2] = if hash_eligible {
                [Some(&bucket_entries), Some(&bucket_entries)]
            } else {
                [None, None]
            };
            let dvs = crate::intrabc::intra_bc_search(
                y_src, // SOURCE pixels (A.3 fact 1), frame-origin absolute
                y_src_stride,
                w as i32,
                h as i32,
                (w / 4) as i32,
                (h / 4) as i32,
                mi_row,
                mi_col,
                ibc.mi_rows,
                ibc.mi_cols,
                ibc.sb_mi_size,
                ibc.sb_size_log2_mi,
                ibc.sb_size_px,
                ibc.tile,
                dv_ref,
                &ibc.sites,
                &ibc.ctrls,
                ibc.sad_per_bit,
                ibc.error_per_bit,
                false, // approx_inter_rate: structurally 0 on allintra
                &ibc.search_tables,
                buckets,
                hv2,
            );
            // Diagnostic (SVTAV1_IBCDBG): what the DV search actually
            // returned for this block. Without it a "C codes IntraBC here
            // and the port does not" verdict cannot distinguish "the
            // search found no DV" from "it found one and the RD lost".
            #[cfg(feature = "std")]
            if crate::dbgenv::ibcdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG IBCSEARCH mi=({},{}) {}x{} hash_elig={} bucket={} dv_ref=({},{}) ndv={} dvs={:?}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                    hash_eligible,
                    bucket_entries.len(),
                    dv_ref.y,
                    dv_ref.x,
                    dvs.len(),
                    dvs,
                );
            }
            for dv in dvs {
                // Prediction: the RECON-domain block copy (the ONE
                // search-vs-predict asymmetry — map §A.6).
                let mut pred = vec![0u8; w * h];
                crate::intrabc_pred::predict_intrabc_luma(
                    y_recon, y_stride, abs_x, abs_y, w, h, dv, &mut pred,
                );
                // The SAME block copy on the 10-bit canvas. This is the
                // prediction `tx_unit_hbd` residuals against; leaving it
                // empty is what made an IBC candidate unrepresentable at
                // bd10 (and is why the injection was gated out).
                let mut pred10: Vec<u16> = Vec::new();
                if bd10_funnel {
                    pred10 = vec![0u16; w * h];
                    crate::intrabc_pred::predict_intrabc_luma(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        dv,
                        &mut pred10,
                    );
                }
                let satd = if bd10_funnel {
                    // Score MDS0 at the real depth, like every other
                    // candidate's bd10 arm above.
                    hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h)
                } else if frame.mds0_ssd {
                    let mut sse: u64 = 0;
                    for r in 0..h {
                        let srow = y_src_off + r * y_src_stride;
                        for c in 0..w {
                            let d = i64::from(y_src[srow + c]) - i64::from(pred[r * w + c]);
                            sse += (d * d) as u64;
                        }
                    }
                    sse
                } else {
                    hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
                };
                // svt_aom_intra_fast_cost use_intrabc arm (rd_cost.c
                // :531-545): rate = mv_bit_cost(dv, pred_dv, dv tables,
                // MV_COST_WEIGHT_SUB) + intrabc_fac_bits[1]; chroma 0.
                let (flr32, _) = crate::intrabc::intrabc_fast_cost_rates(
                    dv,
                    dv_ref,
                    dvt,
                    &rates.intrabc_fac_bits,
                );
                let flr = u64::from(flr32);
                let fast_cost = if bd10_funnel {
                    rdcost(lambda_bd10_fast, flr, satd << 4)
                } else {
                    rdcost(lambda, flr, if frame.mds0_ssd { satd } else { satd << 4 })
                };
                cands.push(Cand {
                    mode: 0, // DC_PRED (the coded neighbour-visible mode)
                    delta: 0,
                    fi: FI_NONE,
                    uv: 0, // UV_DC_PRED
                    uv_delta: 0,
                    pred,
                    pred10,
                    flr,
                    fcr: 0,
                    fast_cost,
                    full_cost: u64::MAX,
                    mds3_cost_ssim: u64::MAX,
                    mds1_has_coeff: false,
                    tx_depth: 0,
                    txb_q: Vec::new(),
                    txb_eob: Vec::new(),
                    txb_cul: Vec::new(),
                    txb_type: Vec::new(),
                    y_recon: Vec::new(),
                    y_recon10: Vec::new(),
                    u_recon10: Vec::new(),
                    v_recon10: Vec::new(),
                    y_recon_d0: Vec::new(),
                    y_bits: 0,
                    y_dist: 0,
                    u_q: Vec::new(),
                    v_q: Vec::new(),
                    u_eob: 0,
                    v_eob: 0,
                    u_cul: 0,
                    v_cul: 0,
                    u_recon: Vec::new(),
                    v_recon: Vec::new(),
                    cfl_alpha_idx: 0,
                    cfl_alpha_signs: 0,
                    palette: None,
                    ibc: Some((dv, dv_ref)),
                    mds3_cost: u64::MAX,
                    block_has_coeff: false,
                    total_rate: 0,
                    full_dist: 0,
                });
            }
        }
    }
    cands
}
