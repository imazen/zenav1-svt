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
use crate::vecpool::{dirty_pool, zeroed_pool};

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

/// C `svt_aom_intra_fast_cost`'s luma-MODE rate for an INTRA candidate
/// (rd_cost.c:545-630), which is slice-type dependent in three places:
///
/// * `intra_mode_bits_num` = `mb_mode_fac_bits[size_group][mode]` when
///   `slice_type != I_SLICE`, else ZERO (`:558-560`);
/// * `intra_luma_mode_bits_num` = the KEY-frame `y_mode_fac_bits[top][left]
///   [mode]` when `slice_type == I_SLICE`, else ZERO (`:568-570`);
/// * `is_inter_rate` = `intra_inter_fac_bits[is_inter_ctx][0]` when
///   `slice_type != I_SLICE`, else ZERO (`:624-626`).
///
/// The first two are EXCLUSIVE, not additive — pricing an inter frame's
/// intra candidate from the key-frame table both over-prices it against C
/// and disagrees with the pack, which already writes
/// `write_intra_mode_inter` (`pipeline.rs`) there.
fn intra_mode_rate(frame: &FunnelFrame, rates: &MdRates, g: &LeafGeom, mode: u8) -> u64 {
    if frame.non_i_slice {
        let group = crate::entropy::context::block_size_group(g.w, g.h);
        rates.mb_mode[group][mode as usize] as u64 + rates.intra_inter[g.is_inter_ctx][0] as u64
    } else {
        rates.kf_y[g.above_ctx][g.left_ctx][mode as usize] as u64
    }
}

/// C `MAX_CU_COST` (`definitions.h`) — `(uint64_t)~0 >> 1`, the seed for the
/// per-mode regular-intra costs and for `best_reg_intra_cost`. Every real fast
/// cost, including the `MAX_MODE_COST` sentinel a pruned MDS0 candidate
/// carries, is below it.
const MAX_CU_COST: u64 = u64::MAX >> 1;

/// C `process_cand_itr` (product_coding_loop.c:1559-1639): whether a class-0
/// candidate belongs to the current MDS0 iteration.
///
/// Iteration 0 takes the REGULAR modes; iteration 1 takes the angular and
/// filter-intra ones, pruned against iteration 0's results:
///
/// * an angular candidate is dropped when its own mode's regular cost is more
///   than `skip_angular_delta<|delta|>_th` percent worse than the best regular
///   cost — and the whole comparison is skipped when this mode IS the best
///   regular one, which is C's "eval the child-angular if the parent-angular
///   is the best" rule;
/// * a filter-intra candidate other than FILTER_DC is dropped unless the
///   non-filter mode it maps to won iteration 0. FILTER_DC is always tested.
///
/// Only reached when `tot_itr > 1`. The arithmetic is `i128` where C's is
/// `uint64_t`: a mode never scored at iteration 0 keeps the `MAX_CU_COST`
/// seed, and `(MAX_CU_COST - best) * 100` overflows 64 bits in C. Both forms
/// answer "skip", which is the only thing the comparison is asked.
#[allow(clippy::too_many_arguments)]
fn process_cand_itr(
    cfg: &FunnelCfg,
    mode: u8,
    delta: i8,
    fi: u8,
    itr: u8,
    best_reg_mode: i32,
    best_reg_cost: u64,
    regular_intra_cost: &[u64; 13],
) -> bool {
    let ang_skip_armed = cfg.skip_ang_delta_th.iter().any(|&t| t != -1);
    if itr == 0 {
        // Which of the three shapes iteration 0 takes depends on what armed
        // the split, exactly as C's three-way branch does.
        return if cfg.reduce_filter_intra && ang_skip_armed {
            delta == 0 && fi == FI_NONE
        } else if ang_skip_armed {
            delta == 0
        } else {
            fi == FI_NONE
        };
    }
    // Iteration 1 is the complement of whatever iteration 0 took.
    let take = if cfg.reduce_filter_intra && ang_skip_armed {
        !(delta == 0 && fi == FI_NONE)
    } else if ang_skip_armed {
        delta != 0
    } else {
        fi != FI_NONE
    };
    if !take {
        return false;
    }
    if fi != FI_NONE {
        // FILTER_DC (mode 0) is always tested; the rest need their mapped
        // non-filter mode to have won iteration 0.
        return fi == 0 || i32::from(FIMODE_TO_INTRAMODE[fi as usize]) == best_reg_mode;
    }
    // Angular pruning, and only when this mode is not itself the winner.
    if best_reg_mode == i32::from(mode) {
        return true;
    }
    let idx = (delta.unsigned_abs() as usize).wrapping_sub(1);
    let Some(&th) = cfg.skip_ang_delta_th.get(idx) else {
        return true;
    };
    if th == -1 {
        return true;
    }
    let mine = i128::from(regular_intra_cost[usize::from(mode)]);
    let best = i128::from(best_reg_cost);
    (mine - best.max(1)) * 100 <= i128::from(th) * best
}

/// `fast_loop_core`'s MDS0 prune (product_coding_loop.c:1309-1334), the
/// PD1 arm. `pruning_method_th` selects which compare runs:
///
/// - any value besides 0/`(uint8_t)~0` (level 1): the PER-CLASS arm, but
///   only when `MIN(md_me_dist, md_pme_dist) / (bw * bh)` exceeds it — the
///   `min_dist_div_area` argument carries that precomputed ratio; on a
///   miss the per-class check does NOT run and the global arm decides.
/// - `(uint8_t)~0` (level 2), or a level-1 gate miss: the GLOBAL arm —
///   `dist_to_cost_th`, which is armed-0 at both levels (level 1 leaves it
///   at the context's zero-init, level 2 writes 0 explicitly), so the
///   check degenerates to `distortion_cost > best`.
///
/// Level 0 carries `mds0_dist_to_cost_th = None` and never reaches here.
/// `cand_class` is the CAND_CLASS the candidate was injected under —
/// intra 0, inter 1/2, palette 3, IntraBC 4 (`mode_decision.c:3646-3672`).
fn mds0_prune_fires(
    cfg: &FunnelCfg,
    min_dist_div_area: Option<u64>,
    distortion_cost: u64,
    mds0_best_cost: Option<u64>,
    mds0_best_cost_per_class: &[Option<u64>; 5],
    cand_class: u8,
) -> bool {
    if let (Some(pc), Some(mda)) = (cfg.mds0_per_class_prune, min_dist_div_area)
        && mda > u64::from(pc.min_dist_div_area_th)
    {
        let cls = usize::from(cand_class);
        let th = pc.dist_to_cost_th[cls];
        return th != u16::MAX
            && mds0_best_cost_per_class[cls].is_some_and(|best| {
                100i128 * (i128::from(distortion_cost) - i128::from(best))
                    > i128::from(best) * i128::from(th)
            });
    }
    matches!(
        (cfg.mds0_dist_to_cost_th, mds0_best_cost),
        (Some(th), Some(best))
            if 100i128 * (i128::from(distortion_cost) - i128::from(best))
                > i128::from(best) * i128::from(th)
    )
}

pub(super) fn inject_candidates(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    bd: LeafBd10<'_>,
    pal: PalFlagRates,
    lambda: u64,
    // C `dequants->y_dequant_qtx[base_q_idx][1]` — the luma AC dequant
    // the inter-intra/masked-compound searches' `model_rd` arms divide
    // by (enc_inter_prediction.c:2027-2029). Inert at the reachable
    // control levels but plumbed faithfully.
    quantizer: i16,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    dc_only: bool,
    ind_uv: &mut Option<[(u8, i8); 13]>,
    // The block-scoped inputs the MDS1 warp MV refinement needs. Threaded as
    // an out-param for the same reason `ind_uv` is: it is produced HERE, by
    // the inter injector, and consumed one stage later.
    warp_blk: &mut crate::inter_md_arm::WarpRefineBlock,
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
        // `above_ctx` / `left_ctx` are now read through `intra_mode_rate`
        // (they select the KEY-frame luma table), so they stay on `g`.
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
    // Under the bypass-encdec MDS3 bump (`bd.mds3_hbd`, C
    // product_coding_loop.c:9649) every decision in this injector — the MDS0
    // fast-cost domains below and the inject-time ind-uv table — still runs
    // at `hbd_md = 0`: the bump fires inside the MDS3 preamble. `bd10_decide`
    // selects the 10-bit scoring arms; `bd10_rd` sees `no_rd` so the chroma
    // table's fast/full-loop evals stay u8. The 10-bit PLUMBING (`pred10`
    // buffers, palette colours ×4) keys on `bd10_funnel` unchanged.
    let no_rd: Option<Bd10Rd> = None;
    let bd10_decide = bd10_funnel && !bd.mds3_hbd;
    let bd10_rd = if bd.mds3_hbd { &no_rd } else { bd.rd };
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

        // Pristine v4.2.0 ranks by variance; hybrid3115 MODE0/MODE1 ranks
        // by SAD because its added mds0_dist_type stays zero-initialized.
        // This fast loop admits the candidates for full RD evaluation, so
        // changing the metric changes the search set (no rate term here).
        // See docs/PARITY-REFERENCE-AUDIT-2026-09-08.md.
        // Pooled: this fast loop runs once per leaf per UV candidate and was
        // 660,053 allocating calls on the canonical alloc cell, the largest
        // site left after the tx-pipeline buffers were pooled.
        let mut u_pred = dirty_pool::<u8>(cw * chh);
        let mut v_pred = dirty_pool::<u8>(cw * chh);
        let mut u_pred10 = dirty_pool::<u16>(cw * chh);
        let mut v_pred10 = dirty_pool::<u16>(cw * chh);
        let mut fast: Vec<(u64, usize)> = Vec::with_capacity(uv_cands.len());
        // Every uv candidate sits at the same (ccx, ccy, cw, chh), so its
        // neighbour extraction is identical — cached per plane.
        let mut nb_u = None;
        let mut nb_v = None;
        for (idx, &(uvm, uvd)) in uv_cands.iter().enumerate() {
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
                    if frame.reference == crate::reference::SvtReference::Mainline420 {
                        residual_variance_hbd(&b.u_src10, cw, 0, 0, &u_pred10, cw, chh)
                            + residual_variance_hbd(&b.v_src10, cw, 0, 0, &v_pred10, cw, chh)
                    } else {
                        residual_sad_hbd(&b.u_src10, cw, 0, 0, &u_pred10, cw, chh)
                            + residual_sad_hbd(&b.v_src10, cw, 0, 0, &v_pred10, cw, chh)
                    }
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
                        &mut nb_u,
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
                        &mut nb_v,
                        &mut v_pred,
                    );
                    if frame.reference == crate::reference::SvtReference::Mainline420 {
                        let offset = ccy * fx.c_stride + ccx;
                        u64::from(svtav1_dsp::variance::variance_diff(
                            &u_pred,
                            cw,
                            &fx.u_src[offset..],
                            fx.c_stride,
                            cw,
                            chh,
                        )) + u64::from(svtav1_dsp::variance::variance_diff(
                            &v_pred,
                            cw,
                            &fx.v_src[offset..],
                            fx.c_stride,
                            cw,
                            chh,
                        ))
                    } else {
                        residual_sad(fx.u_src, fx.c_stride, ccx, ccy, &u_pred, cw, chh)
                            + residual_sad(fx.v_src, fx.c_stride, ccx, ccy, &v_pred, cw, chh)
                    }
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

        // 4. Full-loop count: base `cfg.ind_uv_nfl_base` -- C's
        //    `allintra ? (is_highest_layer ? 16 : 32) : I_SLICE ? 64 :
        //    !is_highest_layer ? 32 : 16` (:7693-7696, stamped per picture
        //    by `intra_arm::ind_uv_nfl_base`) -- scaled by
        //    uv_nic_scaling_num/16, min 1, capped by the candidate count
        //    (:7697-7699). Under OPT_USE_HL0_FLAT a KF (temporal layer 0,
        //    hierarchical_levels 0) has is_highest_layer = FALSE
        //    (pd_process.c:6212: `(tli == hl) && hl != 0`). UV_DC is always
        //    tested (:7700-7720); it is injected first (sorted index 0 on
        //    the flat-chroma tie) so it is already within the first nfl,
        //    but the explicit force is kept for content where DC sorts
        //    late. -> still: 16 at M1 (uv_nic 8), 32 at M0 (uv_nic 16);
        //    video KEY frame: 32 at M1, 64 at M0 (= all 61 injected).
        //    The base was a hard-coded 32 until 2026-09-04: on a video key
        //    frame that cut UV_SMOOTH*/UV_PAETH out of every flat-chroma
        //    SAD tie and mispriced every PAETH / FILTER_PAETH candidate from
        //    MDS0 on (`video_key_matrix` gradient/screenrep p0).
        let mut nfl = div_round(u64::from(cfg.ind_uv_nfl_base) * uv_nic, 16).max(1) as usize;
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
                    let (u_out, v_out) =
                        chroma::eval_uv_hbd(cx, fx, b, uvm, uvd, TxGate::default());
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
                None => {
                    let (u_out, v_out) = chroma::eval_uv(cx, fx, uvm, uvd, TxGate::default());
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
            };
            uv_rd.push((uvm, uvd, bits, dist));
            #[cfg(feature = "std")]
            if crate::dbgenv::nsqdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                let (ub, ud, vb, vd) = match bd10_rd.as_ref() {
                    Some(b) => {
                        let (u_out, v_out) =
                            chroma::eval_uv_hbd(cx, fx, b, uvm, uvd, TxGate::default());
                        (u_out.bits as u64, u_out.dist, v_out.bits as u64, v_out.dist)
                    }
                    None => {
                        let (u_out, v_out) = chroma::eval_uv(cx, fx, uvm, uvd, TxGate::default());
                        (u_out.bits as u64, u_out.dist, v_out.bits as u64, v_out.dist)
                    }
                };
                eprintln!(
                    "NSQDBG UVRD mi=({},{}) {}x{} uv={uvm} uvd={uvd} bits={bits} dist={dist} ub={ub} ud={ud} vb={vb} vd={vd}",
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
    // C's block-setup ordering (`md_product_coding_loop`,
    // product_coding_loop.c:9393-9447): the MVP stacks and the ME/PME
    // searches run BEFORE `generate_md_stage_0_cand`, so `md_me_dist` /
    // `md_pme_dist` exist when the intra candidate list is decided —
    // `eliminate_candidate_based_on_pme_me_results`
    // (mode_decision.c:3408-3417) reads them. The search output travels to
    // `build_inter_candidates` below, so the searches still run once.
    let mut inter_pre = None;
    if let Some(im) = fx.inter {
        let mi_row = (abs_y / 4) as i32;
        let mi_col = (abs_x / 4) as i32;
        let stride = im.mi_cols;
        let base = mi_row * stride + mi_col;
        // The MVP scan runs against the LIVE mi state, in which the CURRENT
        // cell already carries this block's own partition (the
        // `has_top_right` VERT_A read) — exactly as the IBC arm below does.
        let grid_mut = fx
            .ibc_mvp
            .as_deref_mut()
            .expect("the MD mi grid is allocated whenever the inter arm is armed");
        grid_mut[base as usize].partition = fx.ibc_gate.partition;
        let grid = fx
            .ibc_mvp
            .as_deref()
            .expect("the MD mi grid is allocated whenever the inter arm is armed");
        let neighbors =
            crate::inter_md_arm::neighbors_from_grid(grid, stride, mi_row, mi_col, im.tile);
        let bctx = crate::intrabc_mvp::derive_block_ctx(
            mi_row,
            mi_col,
            bsize_idx,
            im.mi_rows,
            im.mi_cols,
            im.tile,
            im.sb_mi_size,
        );
        let overlappable = crate::inter_mvp::count_overlappable_neighbors(
            &crate::intrabc_mvp::MvpGrid {
                entries: grid,
                stride,
                base,
            },
            &bctx,
            bsize_idx,
        );
        // C `ctx->is_inter_ctx` — `svt_av1_get_intra_inter_context`
        // over the same neighbour pair (entropy_coding.c:1127).
        //
        // Through `port_entropy_inter`'s transcription, NOT
        // `entropy::context::get_intra_inter_context`: this call
        // used to collapse "not available" into "intra" and then
        // read an INVERTED table, so a block with two INTER
        // neighbours priced at context 3 (both intra) instead of 0.
        // MEASURED 2026-09-02 against C's own
        // `svt_aom_inter_fast_cost`: 1207 rate units on EVERY inter
        // candidate of the block. The writer already used this
        // function (`write_intra_inter`'s call site); MD did not,
        // and the two disagreed for as long as both existed.
        let is_inter_ctx = crate::port_entropy_inter::intra_inter_context(&neighbors);
        // C `ctx->is_intra_bordered` (product_coding_loop.c:9115 light /
        // :9451 regular): `use_neighbouring_mode_ctrls.enabled ?
        // is_intra_bordered(ctx) : 0` — the gate runs on BOTH lanes, and
        // `is_intra_bordered` is "above and left both exist and are both
        // non-inter" (:8141-8157). The controls are the per-SB light sig's
        // on the light lane, the picture-level row's otherwise.
        let use_neighbouring = fx.lpd1.as_ref().map_or_else(
            || im.cand_reduction.use_neighbouring_mode_enabled != 0,
            |l| l.sig.cand_reduction.use_neighbouring_mode_enabled != 0,
        );
        let is_intra_bordered = use_neighbouring
            && neighbors.up_available
            && neighbors.left_available
            && neighbors.above.is_some_and(|m| !m.is_inter_block())
            && neighbors.left.is_some_and(|m| !m.is_inter_block());
        // `svt_init_mv_cost_params`'s SAD lambda — `fast_lambda_md
        // [EB_8_BIT_MD]`. Under the SSIM/IQ/MS_SSIM tunes C's per-block
        // `aom_av1_set_ssim_rdmult` also scales `pic_fast_lambda` into it
        // (mode_decision.c:4078); the ME's `dist_type == SAD` arm is the
        // reader (product_coding_loop.c:1923).
        let inter_fast_lambda = frame
            .ssim_rdmult
            .as_ref()
            .map_or(frame.inter_fast_lambda, |s| {
                let scale = crate::tune::ssim_scale_for_block(
                    &s.factors,
                    s.num_cols,
                    s.num_rows,
                    abs_y >> 2,
                    abs_x >> 2,
                    w >> 2,
                    h >> 2,
                );
                (f64::from(s.pic_fast8) * scale + 0.5) as u32
            });
        let prelude = crate::inter_md_arm::block_prelude(
            im,
            &mut crate::inter_md_arm::InterBlockCtx {
                org_x: abs_x,
                org_y: abs_y,
                bw: w,
                bh: h,
                bsize: bsize_idx as u8,
                grid,
                grid_stride: stride,
                neighbors,
                overlappable_neighbors: overlappable,
                is_inter_ctx,
                has_uv,
                // C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`:
                // one slot, written by a square block's own search and read
                // by the NSQ shapes that follow it at the same node. The
                // funnel walks a node's shapes with PART_N first, which is
                // what makes a single slot the faithful structure.
                sq_me: fx.inter_sq_me.as_deref_mut(),
                // C `ctx->part` — the wedge-mode selector between
                // `ii_wedge_mode_sq` and `ii_wedge_mode_nsq`. The
                // prelude's searches never read it; the field is real
                // anyway so the block ctx carries one coherent shape.
                is_part_n: fx.ibc_gate.is_part_n,
                // C `blk_ptr->y_src->buffer16` — read only under
                // `pcs->hbd_md` (nonzero at bd10 presets <= 5 on this
                // flat GOP, 0 elsewhere).
                y_src10: blk_y_src10,
                // C fills `ctx->intrapred_buf` AFTER the prelude
                // searches (`md_encode_block`, product_coding_loop.c
                // :9444-9449) — the prelude ctx has none yet.
                ii: None,
                // C `full_lambda_md[bit]` — the searches' model-RD
                // lambdas; inert at the reachable control levels.
                full_lambda8: u32::try_from(lambda).expect("full_lambda_md is a uint32_t in C"),
                full_lambda10: bd
                    .rd
                    .as_ref()
                    .map_or(0, |r| u32::try_from(r.lambda).expect("full_lambda_md[1]")),
                quantizer,
            },
            lambda,
            inter_fast_lambda,
            is_intra_bordered,
            fx.lpd1.as_ref().map(|l| (&l.sig, is_intra_bordered)),
        );
        inter_pre = Some((
            prelude,
            neighbors,
            overlappable,
            is_inter_ctx,
            is_intra_bordered,
        ));
    }
    // C `dc_cand_only_flag` (`generate_md_stage_0_cand`,
    // mode_decision.c:3576-3579). The caller's `dc_only` carries the
    // `intra_mode_end == DC_PRED || is_dc_only_safe` arm; this adds
    // `eliminate_candidate_based_on_pme_me_results` — when
    // `cand_elimination_ctrls` is enabled and the block's best post-subpel
    // ME/PME residual is under `dc_only_th * bheight * bwidth`, the intra
    // set collapses to {DC_PRED}. Dead on stills (`fx.inter` is None) and
    // at cand_reduction levels 0/1 (enabled == 0).
    let mut dc_only = dc_only;
    // C reads `ctx->cand_reduction_ctrls.cand_elimination_ctrls` — the
    // LIGHT-PD1 signal's copy on the light lane (its cand_reduction_level
    // is raised above the picture's), the frame's otherwise. Read once —
    // used by both the dc_cand_only injection collapse and the MDS0
    // cand_elimination early-out (fast_loop_core :1017-1030).
    let elim = inter_pre.as_ref().map(|_| {
        fx.lpd1.as_ref().map_or_else(
            || {
                &fx.inter
                    .expect("inter_pre is built exactly when the inter arm is armed")
                    .cand_reduction
                    .cand_elimination_ctrls
            },
            |l| &l.sig.cand_reduction.cand_elimination_ctrls,
        )
    });
    if let Some((prelude, ..)) = &inter_pre {
        let elim = elim.unwrap();
        if elim.enabled != 0 {
            let (me, pme) = (prelude.search.md_me_dist(), prelude.search.md_pme_dist());
            // `generate_md_stage_0_cand_light_pd1` reads `md_me_dist` ONLY
            // (mode_decision.c:3539-3545) — the light lane runs no
            // `pme_search`, so `md_pme_dist` is not part of its check. The
            // regular `eliminate_candidate_based_on_pme_me_results` takes
            // `MIN(md_me_dist, md_pme_dist)` (:3408-3416).
            let best = if fx.lpd1.is_some() { me } else { me.min(pme) };
            if best != u32::MAX {
                let th = u32::from(elim.dc_only_th)
                    .wrapping_mul(h as u32)
                    .wrapping_mul(w as u32);
                if best < th {
                    dc_only = true;
                }
            }
        }
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
    let mds0_use_hadamard = cfg.mds0_use_hadamard_sb
        && (cand_modes.len() > 1 || palette_can_inject || cfg.allow_intrabc);

    let mut cands: Vec<Cand> = Vec::with_capacity(cand_modes.len());
    // MDS0 with `prune_using_best_mode` (product_coding_loop.c:1680-1737):
    // candidates are evaluated in injection order; the running best REGULAR
    // (class-0, non-filter-intra) mode by fast cost is tracked and used to
    // SKIP later candidates — H when V is currently best, SMOOTH when DC is
    // still best. Skipped candidates never get a fast cost (never enter the
    // pool). At M6 (prune off) every candidate is evaluated, identical to
    // the original funnel.
    let mut best_reg_cost = MAX_CU_COST;
    let mut best_reg_mode: i32 = -1;
    // C `md_stage_0`'s CLASS-0 ITERATION SPLIT (product_coding_loop.c:1667).
    //
    //   itr 0: the REGULAR modes (angle_delta == 0, no filter-intra).
    //   itr 1: the ANGULAR modes and the filter-intra ones, each prunable
    //          against what itr 0 learned.
    //
    // `tot_itr` is 2 exactly when `reduce_filter_intra` is set or any
    // `skip_angular_delta*_th` is live. Both are 0 / -1 on every still — C
    // keys them on `is_islice` and an all-intra picture is always an I-slice
    // — so this whole structure collapses to the single pass the still path
    // has always run, and `identity_full_8bit.sh` (1100/1100) is what says so.
    //
    // On an INTER frame from preset 3 up, both are live.
    let tot_itr: u8 = if cfg.reduce_filter_intra || cfg.skip_ang_delta_th.iter().any(|&t| t != -1) {
        2
    } else {
        1
    };
    // C `regular_intra_cost[PAETH_PRED + 1]` (:1676), seeded `MAX_CU_COST`
    // and filled at itr 0 with each regular mode's fast cost. PAETH_PRED is
    // 12, so the array is 13 long.
    let mut regular_intra_cost = [MAX_CU_COST; 13];
    // C `ctx->mds0_best_cost` (product_coding_loop.c:8385 resets it to
    // `(uint64_t)~0` per block; `:1717` keeps it as the running MINIMUM of
    // every candidate's `*fast_cost`). It exists here only to feed the MDS0
    // prune below, which is why it is `None` until the first candidate is
    // scored — C's sentinel is checked explicitly at `:1326`.
    let mut mds0_best_cost: Option<u64> = None;
    // C `ctx->mds0_best_cost_per_class` (reset to `(uint64_t)~0` per block at
    // product_coding_loop.c:9477): the per-class best fast cost, read by the
    // level-1 arm of the MDS0 prune. Unlike `mds0_best_cost` it only ever
    // compares within one CAND_CLASS, so it is immune to the interleaved
    // lane order this funnel evaluates in.
    let mut mds0_best_cost_per_class: [Option<u64>; 5] = [None; 5];
    // C `ctx->mds0_best_idx` + `cand_bf_ptr_array[mds0_best_idx]->luma_fast_dist`
    // (:1019): the best-so-far candidate's un-shifted luma variance, read by the
    // `cand_elimination` early-out (:1020-1030). `None` until the first
    // candidate is scored — same sentinel as `mds0_best_cost`.
    let mut mds0_best_dist: Option<u64> = None;
    // `MIN(ctx->md_me_dist, ctx->md_pme_dist) / (bwidth * bheight)` — the
    // level-1 gate of `fast_loop_core`'s MDS0 prune (:1310-1313), block-level
    // and constant for every candidate. `None` where the inter prelude is
    // absent (intra-only pictures — which never assign level 1 anyway).
    let mds0_min_dist_div_area: Option<u64> = cfg
        .mds0_per_class_prune
        .is_some()
        .then(|| {
            inter_pre.as_ref().map(|(prelude, ..)| {
                let m = prelude
                    .search
                    .md_me_dist()
                    .min(prelude.search.md_pme_dist());
                u64::from(m) / (w as u64) / (h as u64)
            })
        })
        .flatten();
    // All candidates predict from the same `y_recon` neighbourhood at the
    // same (abs_x, abs_y, w, h) — the extraction is loop-invariant.
    let mut nb_y = None;
    for itr in 0..tot_itr {
        for &(mode, delta, fi) in &cand_modes {
            // C gates this skip on `itr == 0` (:1687). At itr 1 the regular modes
            // are already scored, so re-applying it there would drop angular
            // candidates of H / SMOOTH that C keeps.
            if cfg.prune_best_mode && fi == FI_NONE && itr == 0 {
                // intra_mode_end SMOOTH >= H_PRED, so the gate is armed.
                if mode == 2 && best_reg_mode == 1 {
                    continue; // V better than DC -> skip H
                }
                if mode == 9 && best_reg_mode == 0 {
                    continue; // DC still best -> skip SMOOTH
                }
            }
            if tot_itr > 1
                && !process_cand_itr(
                    &cfg,
                    mode,
                    delta,
                    fi,
                    itr,
                    best_reg_mode,
                    best_reg_cost,
                    &regular_intra_cost,
                )
            {
                continue;
            }
            // C `fast_loop_core`'s cand_elimination early-out
            // (product_coding_loop.c:1017-1030): when the best-so-far
            // candidate's un-shifted luma variance is already under
            // `th * area`, this intra candidate is MAX_MODE_COST without
            // running prediction or distortion. `skip_dc_th` (0 at every
            // shipping level) is the harsher DC_PRED threshold. Skipping
            // the push is equivalent — a MAX_MODE_COST candidate sorts to
            // the end of the survivor pool and is eliminated regardless.
            if let (Some(elim), Some(best_dist)) = (elim, mds0_best_dist) {
                if elim.enabled != 0 && fi == FI_NONE {
                    let th = u64::from(if mode == 0 {
                        elim.skip_dc_th
                    } else {
                        elim.dc_only_th
                    }) * (w * h) as u64;
                    if best_dist < th {
                        continue;
                    }
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
            // Pooled: one per injected candidate, 643,485 allocating calls on the
            // canonical alloc cell after the tx-pipeline buffers were pooled.
            let mut pred = dirty_pool::<u8>(w * h);
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
                &mut nb_y,
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

            // C `svt_aom_intra_fast_cost` prices the luma MODE from ONE of two
            // exclusive tables (rd_cost.c:558-570): on an I-slice the key-frame
            // `y_mode_fac_bits[top][left]`, on any other slice
            // `mb_mode_fac_bits[size_group]` — and the other contributes ZERO,
            // it is not an addend. On a non-I-slice the candidate also pays the
            // `is_inter = 0` flag (`:624-626`), which an I-slice never codes.
            let mut flr = intra_mode_rate(frame, rates, g, mode);
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
            let mut pred10 = crate::vecpool::PoolVec::<u16>::new();
            // The 10-bit prediction is PLUMBING — MDS1/MDS3 residual inputs —
            // so it is built whenever the canvas exists, including under the
            // bypass-encdec MDS3 bump where the MDS0 DECISION below must stay
            // in the 8-bit domain (`bd.mds3_hbd` → `bd10_decide` is false).
            if let Some(canvas10) = fx.y_recon10.as_deref() {
                pred10 = zeroed_pool::<u16>(w * h);
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
            }
            let (fast_cost, distortion_cost, fast_dist_metric) = if bd10_decide {
                // C `fast_loop_core`'s distortion switch at `hbd_md`
                // (product_coding_loop.c:1272-1307): SSD /
                // `mds0_use_hadamard_blk` / the `vf_hbd_10` VARIANCE arm —
                // the SAME three-way the u8 arm above takes. The VIDEO
                // arm's `mds0_use_hadamard_sb = false`
                // (enc_mode_config.c:7916, vs the allintra `true` at
                // :8148) sends a video key frame down the variance arm:
                // `svt_aom_highbd_10_variance` normalizes the u16
                // accumulators to the 8-bit scale, a different metric
                // than the hadamard SATD (variance is DC-invariant where
                // SATD is not), so scoring bd10 video frames with
                // `hadamard_satd_hbd` re-ordered C's MDS0 survivor
                // ranking at near-ties. `cand_bf->luma_fast_dist` carries
                // the PRE-shift metric; the <<4 lands in the fast-cost
                // dist term for the SATD/variance arms only.
                let metric10 = if frame.mds0_ssd {
                    svtav1_dsp::hbd::full_distortion_kernel16_bits(
                        blk_y_src10,
                        0,
                        w,
                        &pred10,
                        0,
                        w,
                        w,
                        h,
                    )
                } else if mds0_use_hadamard {
                    hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h, frame.reference)
                } else {
                    residual_variance_hbd(blk_y_src10, w, 0, 0, &pred10, w, h)
                };
                let dist10 = if frame.mds0_ssd {
                    metric10
                } else {
                    metric10 << 4
                };
                #[cfg(feature = "std")]
                {
                    dbg_satd10 = metric10;
                    dbg_pred0 = pred10[0];
                }
                (
                    rdcost(lambda_bd10_fast, flr + fcr, dist10),
                    rdcost(lambda_bd10_fast, 0, dist10),
                    metric10,
                )
            } else {
                let d = if frame.mds0_ssd { satd } else { satd << 4 };
                (rdcost(lambda, flr + fcr, d), rdcost(lambda, 0, d), satd)
            };
            // C `fast_loop_core`'s MDS0 prune (product_coding_loop.c:1309-1334),
            // PD1 only. `ctx->mds0_ctrls.pruning_method_th` selects the arm; level 2
            // (the video arm above M10) sets it to `(uint8_t)~0`, which takes the
            // GLOBAL arm at `:1325`:
            //
            //     distortion_cost = RDCOST(full_lambda, 0, luma_fast_dist);
            //     if (100 * (distortion_cost - mds0_best_cost)) >
            //         (mds0_best_cost * dist_to_cost_th)   ->  MAX_MODE_COST
            //
            // `luma_fast_dist` there is the SHIFTED local (`:1307`), i.e. the same
            // `satd << 4` this funnel feeds `rdcost`, so `distortion_cost` is this
            // candidate's fast cost with the rate term dropped. C then RETURNS
            // before assembling the fast cost, so the candidate carries the
            // sentinel into the pool, cannot lower `mds0_best_cost`, and cannot
            // become `best_reg_intra_mode` (`:1727` stores the sentinel it now
            // holds). `None` (allintra, and video through M10 on a key frame) is
            // byte-identical to the pre-arm path by construction.
            // Every intra candidate here is CAND_CLASS_0 (no palette, no
            // IntraBC — `mode_decision.c:3649-3652`).
            let fast_cost = if mds0_prune_fires(
                &cfg,
                mds0_min_dist_div_area,
                distortion_cost,
                mds0_best_cost,
                &mds0_best_cost_per_class,
                0,
            ) {
                crate::port_md::lpd1_loop::MAX_MODE_COST
            } else {
                fast_cost
            };
            if fast_cost < mds0_best_cost.unwrap_or(u64::MAX) {
                mds0_best_cost = Some(fast_cost);
                mds0_best_dist = Some(fast_dist_metric);
            }
            if fast_cost < mds0_best_cost_per_class[0].unwrap_or(u64::MAX) {
                mds0_best_cost_per_class[0] = Some(fast_cost);
            }
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
            // C `:1727`: at itr 0, a class-0 candidate with NO filter-intra mode
            // records its fast cost per luma mode, and the running best of those
            // becomes `best_reg_intra_mode`. The guard is C's own — `prune_best_mode
            // && intra_mode_end >= H_PRED`, or a split iteration. `intra_mode_end`
            // is SMOOTH (9) on every level that sets `prune_best_mode`, so the
            // second clause is always true where the first is, and the still path
            // keeps the exact behaviour it had.
            //
            // Note this records ANGULAR candidates too when the split came from
            // `reduce_filter_intra` alone (itr 0 evaluates them), and a later
            // delta of the same mode overwrites an earlier one. That is what C
            // does, index and all.
            if fi == FI_NONE
                && itr == 0
                && ((cfg.prune_best_mode && cfg.mode_end >= 2) || tot_itr > 1)
            {
                regular_intra_cost[usize::from(mode)] = fast_cost;
                if fast_cost < best_reg_cost {
                    best_reg_cost = fast_cost;
                    best_reg_mode = mode as i32;
                }
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
                // `cand_bf->luma_fast_dist` — the PRE-shift fast metric, in
                // the frame's MD domain (`vf_hbd_10`/hadamard/SSE16 at bd10,
                // the u8 twin otherwise).
                luma_fast_dist: fast_dist_metric,
                mds1_cnt_nz: 0,
                tx_depth: 0,
                txb_q: Vec::new(),
                txb_eob: smallvec::SmallVec::new(),
                txb_cul: smallvec::SmallVec::new(),
                txb_type: smallvec::SmallVec::new(),
                y_recon: crate::vecpool::PoolVec::new(),
                y_recon10: Vec::new(),
                y_recon10_d0: Vec::new(),
                u_recon10: Vec::new(),
                v_recon10: Vec::new(),
                y_recon_d0: crate::vecpool::PoolVec::new(),
                y_bits: 0,
                y_dist: 0,
                u_q: crate::vecpool::PoolVec::new(),
                v_q: crate::vecpool::PoolVec::new(),
                u_eob: 0,
                v_eob: 0,
                u_cul: 0,
                v_cul: 0,
                u_recon: crate::vecpool::PoolVec::new(),
                v_recon: crate::vecpool::PoolVec::new(),
                cfl_alpha_idx: 0,
                cfl_alpha_signs: 0,
                palette: None,
                ibc: None,
                inter: None,
                mds3_cost: u64::MAX,
                block_has_coeff: false,
                total_rate: 0,
                full_dist: 0,
            });
        }
    }
    // ---- inject_palette_candidates (mode_decision.c:3356-3406) ----
    // C order: regular+fi intra first, palette after (IBC would follow).
    // PORT-NOTE(unverified): C classes palette CAND_CLASS_3 with its own
    // MDS lanes/pool; the class dist-to-cost th 50
    // (enc_mode_config.c:6775) IS honoured — `mds0_prune_fires` indexes it
    // per lane for the level-1 arm. The pool itself is still shared, so
    // near-tie survivor sets can differ from C. Verify on the
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
        let pal_cands = if bd10_decide {
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
            // Under the MDS3 bump (`bd.mds3_hbd`) the palette SEARCH is
            // pre-bump (`hbd_md = 0`) and stays 8-bit; C widens the chosen
            // colours ×4 in the MDS3 preamble (`scale_palette`,
            // product_coding_loop.c:7164-7170) — mirrored in the `pred10`
            // fill below.
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
            let mut pred = dirty_pool::<u8>(w * h);
            let mut pred10 = if bd10_funnel {
                zeroed_pool::<u16>(w * h)
            } else {
                crate::vecpool::PoolVec::<u16>::new()
            };
            let shift = u32::from(frame.bit_depth - 8);
            for (o, &idx) in pc.idx_map.iter().enumerate().take(w * h) {
                let c = pc.colors[idx as usize];
                if bd10_decide {
                    // 10-bit search: `c` is already a 10-bit colour.
                    pred10[o] = c;
                    pred[o] = (c >> shift) as u8;
                } else if bd10_funnel {
                    // MDS3 bump: the search ran at 8 bits; C scales the
                    // palette colours ×4 in the MDS3 preamble
                    // (`scale_palette`, product_coding_loop.c:7164).
                    pred10[o] = c << shift;
                    pred[o] = c as u8;
                } else {
                    pred[o] = c as u8;
                }
            }
            // MDS0 fast distortion at the real depth, mirroring the regular
            // candidates' bd10 arm above — including the `mds0_use_hadamard` /
            // `mds0_ssd` split (`fast_loop_core`, product_coding_loop.c
            // :1272-1307): the video arm prices `vf_hbd_10` variance, not
            // SATD.
            let satd = if bd10_decide {
                if frame.mds0_ssd {
                    svtav1_dsp::hbd::full_distortion_kernel16_bits(
                        blk_y_src10,
                        0,
                        w,
                        &pred10,
                        0,
                        w,
                        w,
                        h,
                    )
                } else if mds0_use_hadamard {
                    hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h, frame.reference)
                } else {
                    residual_variance_hbd(blk_y_src10, w, 0, 0, &pred10, w, h)
                }
            } else {
                hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
            };
            // Luma rate: DC mode + fi-off flag (fi eligible blocks price it
            // for every DC candidate) + the palette slice (rd_cost.c:579-605
            // use_palette=1 arm): ymode YES + size + (0,0) uniform + colors
            // + map tokens.
            let r_mode = intra_mode_rate(frame, rates, g, 0);
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
            // The neighbour colour cache is at most 2 * PALETTE_MAX_SIZE = 16
            // entries and a palette at most 8 colours, so neither of these ever
            // spills. They were 57,128 allocating calls each on the canonical
            // alloc cell.
            let mut pal_found: smallvec::SmallVec<[bool; 16]> =
                smallvec::smallvec![false; pal_cache.len()];
            let mut pal_out: smallvec::SmallVec<[u16; 8]> =
                smallvec::smallvec![0u16; pc.colors.len()];
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
            // C fast_loop_core selects the lambda by hbd_md for palette
            // candidates as well as regular intra. Using the u8 lambda here
            // changes NIC admission even when palette predictions match C.
            // Palette candidates are CAND_CLASS_3 (`mode_decision.c:3654-3656`)
            // — same `fast_loop_core` MDS0 prune as every other class.
            let lam_p = if bd10_decide {
                lambda_bd10_fast
            } else {
                lambda
            };
            let pal_dist = if frame.mds0_ssd { satd } else { satd << 4 };
            let fast_cost = if mds0_prune_fires(
                &cfg,
                mds0_min_dist_div_area,
                rdcost(lam_p, 0, pal_dist),
                mds0_best_cost,
                &mds0_best_cost_per_class,
                3,
            ) {
                crate::port_md::lpd1_loop::MAX_MODE_COST
            } else {
                rdcost(lam_p, flr + fcr, pal_dist)
            };
            if fast_cost < mds0_best_cost.unwrap_or(u64::MAX) {
                mds0_best_cost = Some(fast_cost);
            }
            if fast_cost < mds0_best_cost_per_class[3].unwrap_or(u64::MAX) {
                mds0_best_cost_per_class[3] = Some(fast_cost);
            }
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
                luma_fast_dist: satd,
                mds1_cnt_nz: 0,
                tx_depth: 0,
                txb_q: Vec::new(),
                txb_eob: smallvec::SmallVec::new(),
                txb_cul: smallvec::SmallVec::new(),
                txb_type: smallvec::SmallVec::new(),
                y_recon: crate::vecpool::PoolVec::new(),
                y_recon10: Vec::new(),
                y_recon10_d0: Vec::new(),
                u_recon10: Vec::new(),
                v_recon10: Vec::new(),
                y_recon_d0: crate::vecpool::PoolVec::new(),
                y_bits: 0,
                y_dist: 0,
                u_q: crate::vecpool::PoolVec::new(),
                v_q: crate::vecpool::PoolVec::new(),
                u_eob: 0,
                v_eob: 0,
                u_cul: 0,
                v_cul: 0,
                u_recon: crate::vecpool::PoolVec::new(),
                v_recon: crate::vecpool::PoolVec::new(),
                cfl_alpha_idx: 0,
                cfl_alpha_signs: 0,
                palette: Some((pc.colors, pc.idx_map)),
                ibc: None,
                inter: None,
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
                // C intra_bc_search forces the search pixels/SAD LUT to
                // 8-bit, but derives errorperbit from full_lambda_md[hbd_md].
                // Using the 8-bit frame lambda here changes hash/mesh vector
                // ranking before the native-depth mode/transform decisions.
                // Under the MDS3 bump `hbd_md` is still 0 here (pre-bump) —
                // `bd10_decide`, not `bd10_funnel`.
                if bd10_decide {
                    (u64::from(crate::pd0::kf_full_lambda_bd10(
                        frame.base_qindex,
                        frame.cli_qp,
                        frame.native_preset,
                    )) >> crate::intrabc::RD_EPB_SHIFT)
                        .max(1) as i32
                } else {
                    ibc.error_per_bit
                },
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
                let mut pred = dirty_pool::<u8>(w * h);
                crate::intrabc_pred::predict_intrabc_luma(
                    y_recon, y_stride, abs_x, abs_y, w, h, dv, &mut pred,
                );
                // The SAME block copy on the 10-bit canvas. This is the
                // prediction `tx_unit_hbd` residuals against; leaving it
                // empty is what made an IBC candidate unrepresentable at
                // bd10 (and is why the injection was gated out).
                let mut pred10 = crate::vecpool::PoolVec::<u16>::new();
                if bd10_funnel {
                    pred10 = zeroed_pool::<u16>(w * h);
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
                let satd = if bd10_decide {
                    // Score MDS0 at the real depth, like every other
                    // candidate's bd10 arm above — same
                    // `mds0_use_hadamard`/`mds0_ssd` split (the video arm's
                    // `mds0_use_hadamard_sb = false` -> `vf_hbd_10` variance).
                    if frame.mds0_ssd {
                        svtav1_dsp::hbd::full_distortion_kernel16_bits(
                            blk_y_src10,
                            0,
                            w,
                            &pred10,
                            0,
                            w,
                            w,
                            h,
                        )
                    } else if mds0_use_hadamard {
                        hadamard_satd_hbd(blk_y_src10, w, 0, &pred10, w, h, frame.reference)
                    } else {
                        residual_variance_hbd(blk_y_src10, w, 0, 0, &pred10, w, h)
                    }
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
                // IntraBC candidates are CAND_CLASS_4 (`mode_decision.c:
                // 3657-3660`) — same `fast_loop_core` MDS0 prune; class 4's
                // threshold is the zero-init C leaves live (armed-0).
                let lam_i = if bd10_decide {
                    lambda_bd10_fast
                } else {
                    lambda
                };
                let ibc_dist = if frame.mds0_ssd { satd } else { satd << 4 };
                let fast_cost = if mds0_prune_fires(
                    &cfg,
                    mds0_min_dist_div_area,
                    rdcost(lam_i, 0, ibc_dist),
                    mds0_best_cost,
                    &mds0_best_cost_per_class,
                    4,
                ) {
                    crate::port_md::lpd1_loop::MAX_MODE_COST
                } else {
                    rdcost(lam_i, flr, ibc_dist)
                };
                if fast_cost < mds0_best_cost.unwrap_or(u64::MAX) {
                    mds0_best_cost = Some(fast_cost);
                }
                if fast_cost < mds0_best_cost_per_class[4].unwrap_or(u64::MAX) {
                    mds0_best_cost_per_class[4] = Some(fast_cost);
                }
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
                    luma_fast_dist: satd,
                    mds1_cnt_nz: 0,
                    tx_depth: 0,
                    txb_q: Vec::new(),
                    txb_eob: smallvec::SmallVec::new(),
                    txb_cul: smallvec::SmallVec::new(),
                    txb_type: smallvec::SmallVec::new(),
                    y_recon: crate::vecpool::PoolVec::new(),
                    y_recon10: Vec::new(),
                    y_recon10_d0: Vec::new(),
                    u_recon10: Vec::new(),
                    v_recon10: Vec::new(),
                    y_recon_d0: crate::vecpool::PoolVec::new(),
                    y_bits: 0,
                    y_dist: 0,
                    u_q: crate::vecpool::PoolVec::new(),
                    v_q: crate::vecpool::PoolVec::new(),
                    u_eob: 0,
                    v_eob: 0,
                    u_cul: 0,
                    v_cul: 0,
                    u_recon: crate::vecpool::PoolVec::new(),
                    v_recon: crate::vecpool::PoolVec::new(),
                    cfl_alpha_idx: 0,
                    cfl_alpha_signs: 0,
                    palette: None,
                    ibc: Some((dv, dv_ref)),
                    inter: None,
                    mds3_cost: u64::MAX,
                    block_has_coeff: false,
                    total_rate: 0,
                    full_dist: 0,
                });
            }
        }
    }

    // ---- The INTER candidate (docs/INTER-ENCODE-PLAN.md §1s item 1b) ----
    //
    // C injects its inter candidates in `inject_inter_candidates`
    // (mode_decision.c:2264) BEFORE the intra ones and MDS0 sorts the union;
    // this port appends and the same sort follows, so the order here is not
    // load-bearing. The candidate itself is built by `crate::inter_md_arm`,
    // which owns the motion search, the reference-MV stack, the DRL choice,
    // the motion-compensated prediction and C's `svt_aom_inter_fast_cost` —
    // see that module's header for the fraction of C's candidate set this is.
    if let Some(im) = fx.inter {
        let (prelude, neighbors, overlappable, is_inter_ctx, is_intra_bordered) = inter_pre
            .expect("the block prelude is built at the top whenever the inter arm is armed");
        // C `svt_aom_precompute_intra_pred_for_inter_intra`
        // (enc_intra_prediction.c:718-757), called from `md_encode_block`
        // (product_coding_loop.c:9447-9449) right before
        // `generate_md_stage_0_cand` under the same gate:
        // `inter_intra_comp_ctrls.enabled && is_interintra_allowed_bsize`.
        // C stores LUMA only (`ctx->intrapred_buf`) and recomputes chroma
        // inside `inter_intra_prediction` per call; the port keeps all
        // three planes in the same store — the prediction is pure in the
        // recon plane and the recon cannot change between injection and
        // arbitration, so this is the same numbers computed once.
        //
        // The mode set is `interintra_to_intra_mode[]` = {DC_PRED, V_PRED,
        // H_PRED, SMOOTH_PRED}, `INTERINTRA_MODES` entries at one block
        // each — the layout `inter_intra_search` and the predict-time
        // blend index by `interintra_mode`.
        const II_TO_INTRA: [u8; 4] = [0, 1, 2, 9];
        let ii = crate::port_enc_mode_config::ctrls::set_inter_intra_ctrls(im.inter_intra_level)
            .filter(|c| {
                c.enabled != 0
                    && crate::port_md::predicates::is_interintra_allowed_bsize(bsize_idx as u8)
            })
            .map(|_| {
                let mut p = crate::inter_md_arm::IiPreds {
                    luma: vec![0; II_TO_INTRA.len() * w * h],
                    u: vec![0; II_TO_INTRA.len() * cw * chh],
                    v: vec![0; II_TO_INTRA.len() * cw * chh],
                    luma16: Vec::new(),
                    u16: Vec::new(),
                    v16: Vec::new(),
                };
                // C's `intrapred_buf` is `uint16_t` under `hbd_md` — the
                // u16 twins exist wherever this leaf carries u16
                // prediction buffers, which is `y_recon10`'s canvas.
                if fx.y_recon10.is_some() {
                    p.luma16 = vec![0; II_TO_INTRA.len() * w * h];
                }
                if has_uv && fx.u_recon10.is_some() && fx.v_recon10.is_some() {
                    p.u16 = vec![0; II_TO_INTRA.len() * cw * chh];
                    p.v16 = vec![0; II_TO_INTRA.len() * cw * chh];
                }
                for (i, &mode) in II_TO_INTRA.iter().enumerate() {
                    predict_unit(
                        y_recon,
                        y_stride,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        mode,
                        0,
                        FI_NONE,
                        &y_geom,
                        cfg.edge_filter,
                        filt_type_y,
                        &mut None,
                        &mut p.luma[i * w * h..(i + 1) * w * h],
                    );
                    if has_uv {
                        predict_unit(
                            fx.u_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            mode,
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            &mut None,
                            &mut p.u[i * cw * chh..(i + 1) * cw * chh],
                        );
                        predict_unit(
                            fx.v_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            mode,
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            &mut None,
                            &mut p.v[i * cw * chh..(i + 1) * cw * chh],
                        );
                    }
                    if !p.luma16.is_empty() {
                        predict_unit_hbd(
                            fx.y_recon10.as_deref().expect("luma16 with recon10"),
                            y_stride,
                            abs_x,
                            abs_y,
                            w,
                            h,
                            mode,
                            0,
                            FI_NONE,
                            &y_geom,
                            cfg.edge_filter,
                            filt_type_y,
                            &mut p.luma16[i * w * h..(i + 1) * w * h],
                            im.bit_depth,
                        );
                    }
                    if has_uv && !p.u16.is_empty() {
                        predict_unit_hbd(
                            fx.u_recon10.as_deref().expect("u16 with recon10"),
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            mode,
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            &mut p.u16[i * cw * chh..(i + 1) * cw * chh],
                            im.bit_depth,
                        );
                        predict_unit_hbd(
                            fx.v_recon10.as_deref().expect("v16 with recon10"),
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            mode,
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            &mut p.v16[i * cw * chh..(i + 1) * cw * chh],
                            im.bit_depth,
                        );
                    }
                }
                p
            });
        // `ctx->intrapred_buf` lives on the mode-decision context for the
        // block's whole mode decision — the inject-time prediction AND the
        // IFS/MDS3 rebuild blend from it — so it parks on the funnel
        // context and `InterBlockCtx` borrows it.
        fx.ii_preds = ii;
        let built = crate::inter_md_arm::build_inter_candidates(
            im,
            &mut crate::inter_md_arm::InterBlockCtx {
                org_x: abs_x,
                org_y: abs_y,
                bw: w,
                bh: h,
                bsize: bsize_idx as u8,
                grid: fx
                    .ibc_mvp
                    .as_deref()
                    .expect("the MD mi grid is allocated whenever the inter arm is armed"),
                grid_stride: im.mi_cols,
                neighbors,
                overlappable_neighbors: overlappable,
                is_inter_ctx,
                has_uv,
                // C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`:
                // one slot, written by a square block's own search and read
                // by the NSQ shapes that follow it at the same node. The
                // funnel walks a node's shapes with PART_N first, which is
                // what makes a single slot the faithful structure.
                sq_me: fx.inter_sq_me.as_deref_mut(),
                // C `ctx->part` — the wedge-mode selector between
                // `ii_wedge_mode_sq` and `ii_wedge_mode_nsq`.
                is_part_n: fx.ibc_gate.is_part_n,
                // C `blk_ptr->y_src->buffer16` — read by the searches
                // only under `pcs->hbd_md` (nonzero at bd10 presets
                // <= 5 on this flat GOP, 0 elsewhere).
                y_src10: blk_y_src10,
                // C `ctx->intrapred_buf` — the inter-intra intra
                // predictions, `None` when the block is not II-eligible.
                ii: fx.ii_preds.as_ref(),
                // C `full_lambda_md[bit]` + `y_dequant_qtx` — the
                // searches' model-RD inputs; inert at the reachable
                // control levels (`use_rd_model`/`use_rate` are 0) but
                // plumbed so the faithful arms run.
                full_lambda8: u32::try_from(lambda).expect("full_lambda_md is a uint32_t in C"),
                full_lambda10: bd
                    .rd
                    .as_ref()
                    .map_or(0, |r| u32::try_from(r.lambda).expect("full_lambda_md[1]")),
                quantizer,
            },
            lambda,
            // `generate_md_stage_0_cand_light_pd1` reads the nic-level
            // `merge_inter_cands_mult`; on the light path it is the light
            // signal's copy (the funnel's `cfg` is the nic one — identical
            // at the levels this lane reaches).
            fx.lpd1
                .as_ref()
                .map_or(cfg.merge_inter_cands_mult, |l| l.merge_inter_cands_mult),
            prelude,
            warp_blk,
            is_intra_bordered,
            // Light-PD1 replaces the inter candidate set with the strict
            // MVP + ME-NEWMV subset (see `build_inter_candidates`) — the
            // sig rides along so the injection controls read the light
            // derivation, not the picture's.
            fx.lpd1.as_ref().map(|l| &l.sig),
        );
        for c in built {
            // MDS0's distortion is the SAME arm the intra candidates take:
            // `fast_loop_core` picks SSD / hadamard SATD / two-buffer
            // VARIANCE by `mds0_dist_type` and `mds0_use_hadamard_blk`
            // (product_coding_loop.c:1272-1306) regardless of the candidate
            // being intra or inter. The video arm's
            // `mds0_use_hadamard_sb = false` (enc_mode_config.c:7916/:8032)
            // sends inter blocks down the VARIANCE arm — `fn_ptr->vf`,
            // `svt_aom_mefn_ptr[bsize]` — which is DC-invariant where SATD
            // is not; scoring them with SATD here re-ordered C's
            // MDS0 -> MDS1 survivor ranking. `build_inter_candidates`
            // prices the RATE and the cost is re-formed here so the two
            // lanes are comparable.
            //
            // At bd10 C scores the u16 buffers —
            // `spatial_full_dist_type_fun` for `enc_mode <= M7`,
            // `vf_hbd_10` variance above it — NOT the 8-bit view, whose
            // downconverted SSE runs ~1/16 of the true one and re-orders
            // near-ties (MEASURED johnny 128x128 p6 bd10 frame 1
            // mi=(16,8): C's compound NEW_NEWMV dist 234912 beats
            // NEARMV's 278496; the u8 canvas flipped it).
            let (satd, d, lam) = if bd10_decide {
                let metric10 = if frame.mds0_ssd {
                    svtav1_dsp::hbd::full_distortion_kernel16_bits(
                        blk_y_src10,
                        0,
                        w,
                        &c.y_pred10,
                        0,
                        w,
                        w,
                        h,
                    )
                } else if mds0_use_hadamard {
                    hadamard_satd_hbd(blk_y_src10, w, 0, &c.y_pred10, w, h, frame.reference)
                } else {
                    residual_variance_hbd(blk_y_src10, w, 0, 0, &c.y_pred10, w, h)
                };
                (
                    metric10,
                    if frame.mds0_ssd {
                        metric10
                    } else {
                        metric10 << 4
                    },
                    lambda_bd10_fast,
                )
            } else {
                let satd = if frame.mds0_ssd {
                    let mut sse: u64 = 0;
                    for r in 0..h {
                        let srow = y_src_off + r * y_src_stride;
                        for col in 0..w {
                            let d = i64::from(y_src[srow + col]) - i64::from(c.y_pred[r * w + col]);
                            sse += (d * d) as u64;
                        }
                    }
                    sse
                } else if mds0_use_hadamard {
                    hadamard_satd(y_src, y_src_stride, y_src_off, &c.y_pred, w, h)
                } else {
                    // `fn_ptr->vf(pred, pred_stride, src, src_stride, &sse)`,
                    // product_coding_loop.c:1296-1299 — same call as the intra
                    // variance arm above.
                    u64::from(svtav1_dsp::variance::variance_diff(
                        &c.y_pred,
                        w,
                        &y_src[y_src_off..],
                        y_src_stride,
                        w,
                        h,
                    ))
                };
                (satd, if frame.mds0_ssd { satd } else { satd << 4 }, lambda)
            };
            let flr = u64::from(c.fast_luma_rate);
            // C's `*(cand_bf->fast_cost) = svt_aom_inter_fast_cost(...)`
            // (product_coding_loop.c:1342) charges the SKIP-MODE rate when
            // `skip_mode_rate < luma_rate` (rd_cost.c:997-1003). The cost is
            // re-formed here because the distortion is the funnel's, so the
            // charged rate — not `fast_luma_rate`, which C stores UNdiscounted
            // — is what the rdcost must see. Charging `flr` priced the
            // skip-mode candidate at its full mode rate and let the MDS0
            // replacement pool evict it (gradient 16x16 q40 p6 poc4: C
            // charged 1536 -> fcost 3230888 and kept it; the port charged
            // 4025 -> 4420902 and dropped it).
            let charged = u64::from(c.fast_cost_rate);
            // The SAME MDS0 dist-to-cost prune the intra lane applies
            // (product_coding_loop.c:1309-1334): the gate is inside
            // `fast_loop_core`, before the fast-cost call, and compares this
            // candidate's RATELESS distortion cost against the running
            // block-wide `mds0_best_cost` — which C shares across candidate
            // classes (set once per block at :9481, updated in every class's
            // `md_stage_0`), so the intra class-0 pass above has already
            // seeded it. At mds0_level 2 (`dist_to_cost_th = 0`) any
            // inter candidate whose distortion alone exceeds the best fast
            // cost is dropped before its rate is even priced — which is why
            // C's `SVT_IFCOST` dump showed the NEWMV candidates injected but
            // never fast-costed. A pruned candidate carries MAX_MODE_COST
            // into the pool exactly like the intra lane's.
            let fast_cost = if mds0_prune_fires(
                &cfg,
                mds0_min_dist_div_area,
                rdcost(lam, 0, d),
                mds0_best_cost,
                &mds0_best_cost_per_class,
                c.cand_class,
            ) {
                crate::port_md::lpd1_loop::MAX_MODE_COST
            } else {
                rdcost(lam, charged, d)
            };
            if fast_cost < mds0_best_cost.unwrap_or(u64::MAX) {
                mds0_best_cost = Some(fast_cost);
            }
            if fast_cost < mds0_best_cost_per_class[usize::from(c.cand_class)].unwrap_or(u64::MAX) {
                mds0_best_cost_per_class[usize::from(c.cand_class)] = Some(fast_cost);
            }
            // The intra lanes above each print an `NSQDBG PFAST` line; without
            // this one the inter candidate is INVISIBLE in the candidate dump,
            // and "the injector ran and the candidate lost" is indistinguishable
            // from "the injector never ran" (`docs/WORKING-ON-THIS.md` §5, the
            // silent-harness trap). Same gate, so it costs nothing when off.
            #[cfg(feature = "std")]
            if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG PINTER mi=({},{}) {}x{} mode={:?} rf={:?} mv=({},{}) pmv=({},{}) drl={} flr={} satd={} fast={}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                    c.mode,
                    c.ref_frame,
                    c.mv[0].y,
                    c.mv[0].x,
                    c.pred_mv[0].y,
                    c.pred_mv[0].x,
                    c.drl_index,
                    flr,
                    satd,
                    fast_cost,
                );
            }
            cands.push(Cand {
                // An inter block codes NEITHER an intra y_mode nor a uv_mode
                // (§1x defects 2 and 6), so these two stay at their C
                // initial values and the neighbour grid reads
                // `InterCand::mode` instead.
                mode: 0,
                delta: 0,
                fi: FI_NONE,
                uv: 0,
                uv_delta: 0,
                pred: crate::vecpool::PoolVec::from_slice(&c.y_pred),
                // The bd10 full-RD funnel residuals against this. It was
                // unconditionally EMPTY for an inter candidate, which is the
                // index-out-of-bounds `tx_unit_hbd` hit on the first 10-bit
                // inter frame; `inter_md_arm` fills it whenever the DPB carries
                // a 10-bit twin of the reference.
                //
                // GATED ON THE FUNNEL, not merely on the reference. The
                // invariant every consumer relies on is "`pred10` non-empty IFF
                // a bd10 stage will read it" — an intra candidate gets one only
                // when `fx.y_recon10` exists, and an inter candidate must match
                // that or `evaluate_leaf`'s `psq_resid10` indexes an empty
                // 10-bit source. C has no such split: at `hbd_md == 0` it makes
                // no 10-bit prediction at all.
                pred10: if bd10_funnel {
                    crate::vecpool::PoolVec::from_slice(&c.y_pred10)
                } else {
                    crate::vecpool::PoolVec::new()
                },
                flr,
                fcr: 0,
                fast_cost,
                full_cost: u64::MAX,
                mds3_cost_ssim: u64::MAX,
                mds1_has_coeff: false,
                luma_fast_dist: satd,
                mds1_cnt_nz: 0,
                tx_depth: 0,
                txb_q: Vec::new(),
                txb_eob: smallvec::SmallVec::new(),
                txb_cul: smallvec::SmallVec::new(),
                txb_type: smallvec::SmallVec::new(),
                y_recon: crate::vecpool::PoolVec::new(),
                y_recon10: Vec::new(),
                y_recon10_d0: Vec::new(),
                u_recon10: Vec::new(),
                v_recon10: Vec::new(),
                y_recon_d0: crate::vecpool::PoolVec::new(),
                y_bits: 0,
                y_dist: 0,
                u_q: crate::vecpool::PoolVec::new(),
                v_q: crate::vecpool::PoolVec::new(),
                u_eob: 0,
                v_eob: 0,
                u_cul: 0,
                v_cul: 0,
                u_recon: crate::vecpool::PoolVec::new(),
                v_recon: crate::vecpool::PoolVec::new(),
                cfl_alpha_idx: 0,
                cfl_alpha_signs: 0,
                palette: None,
                ibc: None,
                inter: Some(alloc::boxed::Box::new(super::types::InterCand {
                    mode: c.mode,
                    ref_frame: c.ref_frame,
                    mv: c.mv,
                    pred_mv: c.pred_mv,
                    drl_index: c.drl_index,
                    interp_filters: c.interp_filters,
                    motion_mode: c.motion_mode,
                    // C `cand->block_mi.num_proj_ref`. It is NOT decorative:
                    // the motion-mode ALPHABET the writer picks depends on it
                    // (docs/INTER-ENCODE-PLAN.md §1z¹⁸), so a zero here writes
                    // the two-symbol OBMC symbol where the decoder reads the
                    // three-symbol MOTION_MODES one.
                    num_proj_ref: c.num_proj_ref,
                    overlappable_neighbors: overlappable.min(255) as u8,
                    u_pred: c.u_pred,
                    v_pred: c.v_pred,
                    u_pred10: if bd10_funnel {
                        c.u_pred10
                    } else {
                        alloc::vec::Vec::new()
                    },
                    v_pred10: if bd10_funnel {
                        c.v_pred10
                    } else {
                        alloc::vec::Vec::new()
                    },
                    wm_params: c.wm_params_l0,
                    wm_params_l1: c.wm_params_l1,
                    cand_class: c.cand_class,
                    comp_group_idx: c.comp_group_idx,
                    compound_idx: c.compound_idx,
                    interinter_comp_type: c.interinter_comp_type,
                    interinter_mask_type: c.interinter_mask_type,
                    interinter_wedge_index: c.interinter_wedge_index,
                    interinter_wedge_sign: c.interinter_wedge_sign,
                    is_interintra_used: c.is_interintra_used,
                    interintra_mode: c.interintra_mode,
                    use_wedge_interintra: c.use_wedge_interintra,
                    interintra_wedge_index: c.interintra_wedge_index,
                    skip_mode_allowed: c.skip_mode_allowed,
                    skip_mode_ctx: c.skip_mode_ctx,
                    skip_mode: false,
                })),
                mds3_cost: u64::MAX,
                block_has_coeff: false,
                total_rate: 0,
                full_dist: 0,
            });
        }
    }
    cands
}
