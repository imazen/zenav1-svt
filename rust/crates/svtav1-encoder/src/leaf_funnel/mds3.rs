//! MDS3: the last full loop, plus the independent-chroma search that feeds it.
//!
//! C `md_stage_3` (product_coding_loop.c:7397). For each surviving candidate:
//! the TXS depth sweep, the per-txb transform-type search
//! ([`super::txt::txt_search`]), RDOQ under the frame policy with real
//! contexts, the SPATIAL SSE distortion, and the chroma full loop
//! (`svt_aom_full_loop_uv`) whose uv mode either follows luma or comes from
//! the independent search below.
//!
//! `search_best_mds3_uv_mode` (:7301) runs FIRST, and only when
//! `perform_ind_uv_search_last_mds` (:1472) says so. That predicate has two
//! arms and the second one -- `inter_vs_intra_cost_th`, where `is_inter` means
//! `is_inter_mode(mode) || use_intrabc` -- was missing from the port for a long
//! time and cost two partial-SB cells; see the "defect 5" note in
//! `rust/CLAUDE.md`. The gate lives here, next to the search it gates.
//!
//! Split out of `evaluate_leaf` on 2026-08-25. The body is VERBATIM: the
//! carriers are destructured back into the same local names at the top, so the
//! moved code needed no edits.

use super::*;
use crate::vecpool::{dirty_pool, zeroed_pool};

/// Run the independent-chroma search (when C would) and then the MDS3 full
/// loop over `order1[..n3]`, writing each candidate's `mds3_cost` and winner
/// data back into `cands`.
#[allow(clippy::too_many_arguments)]
// Same block-scoped `> 0` division guards as the rest of the funnel; see the
// note on `stage_mds0_to_mds1` in [`super::nic`].
#[allow(unknown_lints, clippy::manual_checked_ops)]
pub(super) fn run_mds3(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    bd10_rd: &Option<Bd10Rd>,
    // The bd10 state for `search_best_uv_mode` ONLY. It differs from
    // `bd10_rd` under the bypass-encdec `hbd_md = 2` bump
    // (product_coding_loop.c:9649): C's `search_best_mds3_uv_mode` runs
    // BEFORE the bump (:9637), so the independent-chroma search stays in
    // the 8-bit domain while `eval_candidate` runs at 10 bits.
    ind_uv_rd: &Option<Bd10Rd>,
    pal: PalFlagRates,
    qt: &QuantTable,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    sb_is_lvl6: bool,
    cands: &mut [Cand],
    order1: &[usize],
    n3: usize,
    ind_uv: &mut Option<[(u8, i8); 13]>,
    perform_mds1: bool,
    use_tx_shortcuts_mds3: bool,
) {
    // This function is now the MDS3 DRIVER: derive the per-leaf depth-sweep
    // constants, run the independent-uv search, then hand each candidate to
    // `eval_candidate`. It needs only what those three steps read.
    let frame = fx.frame();
    let cfg = frame.cfg;
    let LeafGeom {
        w, h, abs_x, abs_y, ..
    } = *g;

    // -- MDS3: full loop with TXS + TXT + RDOQ + spatial SSE + chroma --
    // txs_level 0 (M8) -> depth 0 only; else get_end_tx_depth clamped by
    // the config's intra sq/nsq max depths. At eff-M9 the enable is per-SB
    // (txs_lvl6_gate): C only bumps txs on for SBs the pd0 detector left at
    // PD0_LVL_6 (undemoted); demoted PD0_LVL_5 SBs keep TXS off (depth 0).
    let txs_active = cfg.txs_on && (!cfg.txs_lvl6_gate || sb_is_lvl6);
    // C `get_start_end_tx_depth` (product_coding_loop.c:6710-6717):
    //
    //     // end_tx_depth set to zero for blocks which go beyond the picture
    //     // boundaries
    //     if (blk_org_x + bwidth <= aligned_width &&
    //         blk_org_y + bheight <= aligned_height)
    //         *end_tx_depth = get_end_tx_depth(bsize);
    //     else
    //         *end_tx_depth = 0;
    //
    // A leaf that STRADDLES the aligned frame edge is searched at tx depth 0
    // only. The port had no boundary term and searched a depth C never tests.
    //
    // MEASURED reachability (2026-08-03, `gradient {80,104,72}x88 q55`, one
    // straddling leaf per frame):
    //   p6  leaf (0,64) 64x32   txs_active=true   end_tx_depth would be 0 -> no-op
    //   p7  leaf (32,64) 32x32  txs_active=true   end_tx_depth would be 1 -> LIVE
    //   p8  leaf (32,64) 32x32  txs_active=false  end_tx_depth would be 0 -> no-op
    // So this is a real divergence at preset 7. It is byte-INERT on the 48
    // partial-SB cells swept ({80x88,104x88,72x88,96x80,88x72,120x104,72x120,
    // 104x72} x p{6,7,8} x q{32,55}) — it changes the searched depth set without
    // flipping any cell's verdict there — but "inert on what we measured" is not
    // "unreachable", and the p7 arm is exercised.
    let in_frame = abs_x + w <= frame.frame_w_px && abs_y + h <= frame.frame_h_px;
    let end_depth = if txs_active && in_frame {
        end_tx_depth(w, h, &cfg)
    } else {
        0
    };
    let tsz_cat = tx_size_cat(w, h);
    let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);

    // -- Independent chroma search before MDS3 -- see [`search_best_uv_mode`].
    search_best_uv_mode(
        fx, g, cx, ind_uv_rd, pal.uv_no, lambda, cands, order1, n3, ind_uv,
    );

    let lambda3 = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
    let mds3_ctx = Mds3Ctx {
        txs_active,
        end_depth,
        in_frame,
        tsz_cat,
        tsz_ctx,
        lambda3,
        perform_mds1,
        use_tx_shortcuts_mds3,
    };
    // ONE borrow per leaf, amortised over every candidate and every depth —
    // see [`Mds3Scratch`] for why that is the whole design. `try_borrow_mut`
    // rather than `borrow_mut`: nothing re-enters `run_mds3` today, and a
    // future caller that does gets its own buffers instead of a panic.
    let mut own = None;
    #[cfg(feature = "std")]
    let taken = MDS3_SCRATCH.with(|cell| {
        cell.try_borrow_mut().ok().map(|mut sc| {
            for &ci in order1.iter().take(n3) {
                eval_candidate(
                    fx,
                    g,
                    cx,
                    &mds3_ctx,
                    pal,
                    bd10_rd,
                    qt,
                    lambda,
                    y_src,
                    y_src_stride,
                    y_src_off,
                    y_recon,
                    y_stride,
                    ind_uv,
                    cands,
                    ci,
                    &mut sc,
                );
            }
        })
    });
    #[cfg(not(feature = "std"))]
    let taken: Option<()> = None;
    if taken.is_none() {
        let sc = own.insert(Mds3Scratch::default());
        for &ci in order1.iter().take(n3) {
            eval_candidate(
                fx,
                g,
                cx,
                &mds3_ctx,
                pal,
                bd10_rd,
                qt,
                lambda,
                y_src,
                y_src_stride,
                y_src_off,
                y_recon,
                y_stride,
                ind_uv,
                cands,
                ci,
                sc,
            );
        }
    }
}

/// C `search_best_mds3_uv_mode` (product_coding_loop.c:7301) and the
/// `perform_ind_uv_search_last_mds` predicate (:1472) that gates it.
///
/// THE GATE IS THE POINT. That predicate has two arms and the port modelled
/// only the first for a long time. The second -- `inter_vs_intra_cost_th`
/// (:1498), which ZEROES the intra survivor count when
/// `best_inter_cost * th < best_intra_cost * 100` -- looks dead on an I-slice
/// until you notice `is_inter` there means `is_inter_mode(mode) || use_intrabc`
/// (:1479). On screen content an IntraBC candidate can win MDS1, and then
/// `best_inter_cost` is an ordinary finite cost and C SKIPS the search
/// entirely. Running it anyway cost two partial-SB cells; see "defect 5" in
/// `rust/CLAUDE.md`.
///
/// Writes the chosen (uv_mode, uv_delta) per luma mode into `ind_uv`, or
/// leaves it `None` when C would not have searched.
#[allow(clippy::too_many_arguments)]
fn search_best_uv_mode(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    bd10_rd: &Option<Bd10Rd>,
    pal_uv_no: u64,
    lambda: u64,
    cands: &[Cand],
    order1: &[usize],
    n3: usize,
    ind_uv: &mut Option<[(u8, i8); 13]>,
) {
    let (frame, rates) = (fx.frame(), fx.rates);
    let cfg = frame.cfg;
    let LeafGeom {
        abs_x,
        abs_y,
        has_uv,
        cfl_allowed,
        use_angle,
        ..
    } = *g;
    // -- Independent chroma search before MDS3 (chroma_level 4:
    //    `search_best_mds3_uv_mode`, product_coding_loop.c:7301, invoked at
    //    :9625-9637 when `perform_ind_uv_search_last_mds` (:1472-1504)
    //    returns true. Produces best_uv[(luma mode)] -> (uv mode, uv delta);
    //    `update_intra_chroma_mode` (:7063) then rewrites each MDS3
    //    candidate before its full loop. --
    //
    // The gate has TWO arms, and the second one is live here (issue #15):
    //
    //  a) `mds3_intra_count` (:1478-1487) counts the MDS3 survivors that are
    //     NOT inter-classified and — with `skip_ind_uv_if_only_dc = 1`, which
    //     is chroma_level 4's setting (enc_mode_config.c:4373) — whose
    //     injected (uv-follows-luma) uv mode is not UV_DC.
    //  b) the `inter_vs_intra_cost_th` arm (:1498-1501) then ZEROES that count
    //     when `best_inter_cost * th < best_intra_cost * 100`, th = 100 at
    //     chroma_level 4 (enc_mode_config.c:4372) — i.e. when the best
    //     inter-classified candidate's MDS1 full cost beats every intra
    //     candidate's.
    //
    // Arm (b) was previously commented here as "never fires on I-slices,
    // MAX_MODE_COST * 100 does not overflow and dwarfs any intra cost". The
    // overflow half is right (MAX_MODE_COST = 13754408443200 * 8,
    // coding_unit.h:37, so * 100 is ~1.1e16, far under 2^64) but the
    // conclusion was WRONG: `is_inter` here is
    // `is_inter_mode(mode) || use_intrabc` (:1479-1481), so on a SCREEN-CONTENT
    // I-slice a winning IntraBC candidate makes `best_inter_cost` an ordinary
    // finite cost and the arm fires. MEASURED on `terminal` 188x256 (the last
    // two divergent cells of tools/unaligned_identity_scan.sh): at p2 q55
    // mi=(50,42) C's MDS1 best intra = 97_762_561 vs best IntraBC = 84_376_537,
    // and at p4 q12 mi=(46,46) 163_691 vs 148_994 — the arm fires in both, C
    // sets `ind_uv_avail = 0` (confirmed directly by the
    // `svt_aom_get_intra_uv_fast_rate` interposer, `indavail=0`), every MDS3
    // candidate keeps its uv-follows-luma pair, and C codes uv=D113/-1 resp.
    // UV_CFL where the port's table said UV_DC.
    //
    // C's `is_inter` for both the count and the two cost minima is
    // `is_inter_mode(block_mi.mode) || block_mi.use_intrabc`; the port has no
    // inter modes on this all-intra path, so IntraBC is the whole of it.
    const IND_UV_INTER_VS_INTRA_TH: u64 = 100; // chroma_level 4, enc_mode_config.c:4372
    let ind_uv_gate = cfg.ind_uv_mds3 && has_uv && {
        let mut intra_count = 0usize;
        let mut best_intra = u64::MAX;
        let mut best_inter = u64::MAX;
        for &ci in order1.iter().take(n3) {
            let c = &cands[ci];
            if c.is_inter() {
                best_inter = best_inter.min(c.full_cost);
            } else {
                if c.uv != 0 {
                    intra_count += 1;
                }
                best_intra = best_intra.min(c.full_cost);
            }
        }
        // C SEEDS both minima with MAX_MODE_COST and only ever lowers them, so
        // an absent class — or a class whose every candidate costs more than
        // the seed — compares as that CONSTANT, not as "infinity". Clamping
        // the u64::MAX-seeded minima to it reproduces both cases exactly.
        // With no IntraBC candidate this makes the arm inert as the old
        // comment assumed: 1.1e16 is not < (an intra cost) * 100.
        const MAX_MODE_COST: u64 = 13_754_408_443_200 * 8; // coding_unit.h:37
        let best_intra = best_intra.min(MAX_MODE_COST);
        let best_inter = best_inter.min(MAX_MODE_COST);
        if best_inter * IND_UV_INTER_VS_INTRA_TH < best_intra * 100 {
            intra_count = 0;
        }
        intra_count > 0
    };
    if ind_uv_gate {
        // Distinct (uv, uv_delta) pairs of the MDS3 survivors, in
        // survivor order, excluding UV_DC; then UV_DC (delta 0) last.
        let mut tested = [[false; 7]; 13];
        let mut uv_list: Vec<(u8, i8)> = Vec::new();
        for &ci in order1.iter().take(n3) {
            let (uvm, uvd) = (cands[ci].uv, cands[ci].uv_delta);
            if uvm == 0 || tested[uvm as usize][(3 + uvd) as usize] {
                continue;
            }
            // Coded-lossless: `search_best_mds3_uv_mode` skips a uv
            // candidate whose chroma tx type is not DCT_DCT
            // (product_coding_loop.c:7376-7379).
            if frame.coded_lossless && uv_tx_type(uvm, cx.cw, cx.chh) != cc::DCT_DCT {
                continue;
            }
            tested[uvm as usize][(3 + uvd) as usize] = true;
            uv_list.push((uvm, uvd));
        }
        uv_list.push((0, 0));

        // Full loop per uv candidate: coeff_rate + SSD distortion
        // (DIST_CALC_RESIDUAL — both planes summed).
        //
        // bd10 FULL-RD (task #94): C runs search_best_mds3_uv_mode ENTIRELY at
        // hbd_md — `full_lambda = full_lambda_md[hbd_md ? EB_10_BIT_MD :
        // EB_8_BIT_MD]` (product_coding_loop.c:7307) with 10-bit prediction/
        // residual (:7397/:7415/:7429) and the 10-bit full-loop distortion
        // (svt_aom_full_loop_uv, :7443). Deciding the uv mode on the u8
        // `chroma_eval` + u8 `lambda` flips near-ties: on 1001682 q12 p5 block
        // (0,0) the port picked UV_V_PRED where C picks UV_DC_PRED. Use the
        // 10-bit twin at bd10; bd8 keeps `chroma_eval` and is byte-unchanged.
        let mut uv_rd: Vec<(u64, u64)> = Vec::with_capacity(uv_list.len());
        for &(uvm, uvd) in &uv_list {
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
            uv_rd.push((bits, dist));
        }

        // Per distinct surviving luma mode (survivor order), pick the
        // lowest-cost uv pair (strict less, list order on ties). At bd10 the
        // compare uses the SAME 10-bit lambda C prices this search with
        // (`full_lambda_md[EB_10_BIT_MD]`, :7307/:7491), matching the 10-bit
        // `uv_rd` above; bd8 takes the `None` arm and keeps the u8 `lambda`.
        let uv_lambda = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
        let mut table = [(0u8, 0i8); 13];
        let mut mode_seen = [false; 13];
        for &ci in order1.iter().take(n3) {
            // C search_best_mds3_uv_mode skips inter-classified candidates
            // (product_coding_loop.c:7335 — an IntraBC cand keeps UV_DC and
            // never seeds a per-luma-mode table row).
            if cands[ci].is_inter() {
                continue;
            }
            let luma = cands[ci].mode as usize;
            if mode_seen[luma] {
                continue;
            }
            mode_seen[luma] = true;
            let mut best_cost = u64::MAX;
            for (k, &(uvm, uvd)) in uv_list.iter().enumerate() {
                let mut fcr2 = rates.uv[cfl_allowed][luma][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr2 += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                if uvm == 0 {
                    fcr2 += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                let (bits, dist) = uv_rd[k];
                let cost = rdcost(uv_lambda, bits + fcr2, dist);
                #[cfg(feature = "std")]
                if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                    eprintln!(
                        "NSQDBG UVTAB2 mi=({},{}) luma={luma} uv={uvm} uvd={uvd} bits={bits} dist={dist} fcr={fcr2} cost={cost}",
                        abs_y / 4,
                        abs_x / 4,
                    );
                }
                if cost < best_cost {
                    best_cost = cost;
                    table[luma] = (uvm, uvd);
                }
            }
        }
        *ind_uv = Some(table);
    }

    // bd10 FULL-RD (task #94): every MDS3 rdcost — the depth compare, the txb
    // early exits and the final block cost — must use the SAME lambda domain
    // as the distortion it is comparing. C uses `full_lambda_md[hbd_md ? 1 : 0]`
    // throughout (md_process.c:753), so one substitution covers all of them.
}

/// The per-leaf MDS3 constants: what the depth sweep needs that does not vary
/// by candidate.
struct Mds3Ctx {
    /// TXS is on for this block (and, at eff-M9, for this superblock).
    txs_active: bool,
    /// C `get_end_tx_depth` clamped by the intra max depths, and forced to 0
    /// for a block that straddles the aligned frame edge
    /// (product_coding_loop.c:6710-6717).
    end_depth: u8,
    /// The block lies entirely inside the aligned frame (the
    /// product_coding_loop.c:6710-6717 boundary rule, which C applies to
    /// EVERY candidate class before the per-class clamp).
    in_frame: bool,
    /// tx-size category and its neighbour-derived context, for the depth rate.
    tsz_cat: usize,
    tsz_ctx: usize,
    /// C `full_lambda_md[hbd_md ? 1 : 0]`. EVERY MDS3 rdcost -- the depth
    /// compare, the txb early exits, the final block cost -- must use the same
    /// lambda domain as the distortion it compares, so this is the one
    /// substitution that covers all of them.
    lambda3: u64,
    /// C `ctx->perform_mds1` — false when `enable_skipping_mds1` collapsed
    /// the post-MDS0 pool to one candidate. The tx-shortcut `bypass_tx_th`
    /// arm requires MDS1 to have run (product_coding_loop.c:6812).
    perform_mds1: bool,
    /// C `use_tx_shortcuts_mds3` (product_coding_loop.c:9225-9232) — a
    /// BLOCK-LEVEL flag, derived once from the MDS0 winner's
    /// `luma_fast_dist` when `!perform_mds1`. When set it forces
    /// `search_dct_dct_only` on every txb and N4-shapes the coefficients.
    use_tx_shortcuts_mds3: bool,
}

/// One candidate's MDS3 evaluation: the TXS depth sweep, the per-txb transform
/// -type search, RDOQ, the spatial SSE distortion and the chroma full loop,
/// ending in the candidate's `mds3_cost`.
///
/// Iterations of C's MDS3 loop are INDEPENDENT given `(fx, cands, ci)` -- there
/// were no mutable locals outside the loop and no `continue`/`break` at its own
/// nesting level, so this is a faithful unit rather than a slice of a running
/// accumulation. It takes `cands` + `ci` rather than a `&mut Cand` because the
/// body reads and writes `cands[ci]` in several forms, and keeping that
/// verbatim is what makes the move checkable.
#[allow(clippy::too_many_arguments)]
// Same block-scoped `> 0` division guards as the rest of the funnel; see the
// note on `stage_mds0_to_mds1` in [`super::nic`].
#[allow(unknown_lints, clippy::manual_checked_ops)]
/// Per-thread scratch for [`eval_candidate`]'s PURE TEMPORARIES — the three
/// buffers that are rebuilt from scratch every iteration and never escape.
///
/// `txb_pred` is a `vec![0u8; txw * txh]` per TX BLOCK per depth per candidate,
/// and `loc_above` / `loc_left` a `.to_vec()` pair per DEPTH; on a 512x512
/// preset-2 still frame `mds3::eval_candidate` is the port's single largest
/// allocator caller (483 of 1,423 malloc/free self samples, 33.9 %) and these
/// are the part of it that can be recycled at all — `dep_recon`, `dep_pred`
/// and the `Vec<Vec<i32>>` of per-txb levels are MOVED into the depth winner
/// and then into the candidate, so they need a flat arena, not a buffer.
///
/// Borrowed ONCE per leaf, in [`run_mds3`], and threaded through the candidate
/// loop as `&mut`. That is deliberate and it is what separates this from the
/// hoists `benchmarks/mdscratch_null_2026-09-03.meta` measured NULL: those took
/// a `thread_local!` + `RefCell::try_borrow_mut` + a closure PER CALL, around a
/// few hundred arithmetic operations. Here the borrow is amortised over every
/// candidate and every depth of the leaf.
///
/// Nothing is re-zeroed: every element of every buffer is written before it is
/// read (`txb_pred` by a full-length `copy_from_slice`, a per-row
/// `copy_from_slice`, or `predict_unit_overlay`; the two spans by
/// `extend_from_slice`). The lengths are re-established per use, so a longer
/// previous block cannot leak into a shorter one.
#[derive(Default)]
pub(super) struct Mds3Scratch {
    txb_pred: Vec<u8>,
    loc_above: Vec<u8>,
    loc_left: Vec<u8>,
}

#[cfg(feature = "std")]
std::thread_local! {
    static MDS3_SCRATCH: core::cell::RefCell<Mds3Scratch> =
        const { core::cell::RefCell::new(Mds3Scratch {
            txb_pred: Vec::new(),
            loc_above: Vec::new(), loc_left: Vec::new(),
        }) };
}

fn eval_candidate(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    m: &Mds3Ctx,
    pal: PalFlagRates,
    bd10_rd: &Option<Bd10Rd>,
    qt: &QuantTable,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    ind_uv: &Option<[(u8, i8); 13]>,
    cands: &mut [Cand],
    ci: usize,
    sc: &mut Mds3Scratch,
) {
    // Destructure the carriers back into the names the moved body uses, so the
    // body itself is byte-for-byte what it was inside the loop. The Arc clone
    // keeps the per-leaf SSIM-override frame reachable without borrowing `fx`,
    // so `ifs_at_mds3(&mut fx, ..)` below can still take `fx` and every
    // downstream `frame.lambda` reader sees the block-scaled value.
    let frame_arc = fx.frame_ssim.clone();
    let frame = frame_arc.as_deref().unwrap_or(fx.frame);
    let rates = fx.rates;
    let cfg = frame.cfg;
    let do_rdoq = frame.rdoq_level > 0;
    let LeafGeom {
        w,
        h,
        abs_x,
        abs_y,
        has_uv,
        y_geom,
        filt_type_y,
        cfl_allowed,
        use_angle,
        skip_ctx,
        aligned_dims,
        ..
    } = *g;
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    // Every chroma predict_unit call below sits at the same (ccx, ccy, cw, chh)
    // on the same recon planes — the neighbour extraction is shared.
    let mut nb_u = None;
    let mut nb_v = None;
    let uv_geom = cx.uv_geom;
    let filt_type_uv = cx.filt_type_uv;
    let uv_crop = cx.uv_crop;
    let (qt_u, qt_v) = (cx.qt_u, cx.qt_v);
    let (cb_tsc, cb_dsc, cr_tsc, cr_dsc) = (cx.cb_tsc, cx.cb_dsc, cx.cr_tsc, cx.cr_dsc);
    let PalFlagRates {
        allow: allow_pal,
        uv_no: pal_uv_no,
        uv_no_y1: pal_uv_no_y1,
        ..
    } = pal;
    let Mds3Ctx {
        txs_active,
        end_depth,
        in_frame,
        tsz_cat,
        tsz_ctx,
        lambda3,
        perform_mds1,
        use_tx_shortcuts_mds3,
    } = *m;
    // `update_intra_chroma_mode`: rewrite the candidate's chroma from
    // the ind-uv table (fast chroma rate recomputed for the luma
    // mode + new uv pair — same formula as injection, so an
    // unconditional recompute is C-identical).
    // C gates the rewrite on `ind_uv_avail && ind_uv_last_mds` (:7063)
    // — it runs for last_mds 1 (M1) and 2 (M2/M3) but NOT for
    // last_mds 0 (M0), whose candidates were already injected FROM the
    // table and keep it. (The earlier "A/B proved rewrite needed for
    // both configs" note toggled M0+M1 together; the q40-64 breakage
    // came from the M1 cells, where C does rewrite.)
    if let Some(tbl) = &ind_uv {
        // C update_intra_chroma_mode skips inter-classified candidates
        // (:7077 `!is_inter` gate) — an IntraBC cand keeps UV_DC.
        if (cfg.ind_uv_last_mds1 || cfg.ind_uv_mds3) && !cands[ci].is_inter() {
            // The rewrite keys on the CODED luma mode (`cand->block_mi.mode`
            // in update_intra_chroma_mode — DC for FILTER candidates), NOT
            // the fi-mapped direction. A/B-verified (g64 p0): mapping the
            // key broke q40.
            let (uvm, uvd) = tbl[cands[ci].mode as usize];
            let c = &mut cands[ci];
            c.uv = uvm;
            c.uv_delta = uvd;
            let mut fcr = rates.uv[cfl_allowed][c.mode as usize][uvm as usize] as u64;
            if use_angle && matches!(uvm, 1..=8) {
                fcr += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
            }
            if uvm == 0 {
                // rd_cost.c:515-521 — the UV_DC palette-flag row is keyed on
                // `use_palette_y = cand->palette_info && palette_size[0] > 0`
                // read off the REAL candidate, and C's recompute here is
                // `svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1)` on
                // that same candidate (update_intra_chroma_mode,
                // product_coding_loop.c:7095). So a LUMA-PALETTE candidate
                // pays the [1] row, not the [0] row every regular candidate
                // pays — the same distinction the injection site (:4596)
                // already makes. C's rewrite is conditional (only when the uv
                // pair actually changed, :7084); when it does NOT fire the
                // candidate keeps the fast_chroma_rate injection gave it,
                // which for a palette candidate is ALSO the [1] row — so the
                // port's unconditional recompute is C-identical only if it
                // uses the same row. Charging [0] here undid :4596 for every
                // ind_uv_last_mds preset (M1..M5), under-costing a palette
                // candidate's chroma flag and biasing the palette-vs-regular
                // RD tie toward palette (#71 over-picking). MEASURED
                // 2026-08-04: flips `screen 64 64 63 1` (C 64B, port 71->64B)
                // and `screen 128 128 63 1` (C 185B, port 193->185B) to byte
                // MATCH — both KNOWN_DIFF pins of tools/identity_full_8bit.sh,
                // promoted in this commit — and moves NO other cell of the
                // 976-cell synthetic+dims scoreboard.
                fcr += if c.palette.is_some() {
                    pal_uv_no_y1
                } else {
                    pal_uv_no
                };
            }
            c.fcr = fcr;
        }
    }
    // ---- C `svt_aom_inter_pu_prediction_av1` at MDS3 (product_coding_loop.c
    //      :6848-6853): the interpolation-filter search, BEFORE the transform
    //      loop, on every inter candidate. See [`super::ifs`].
    if cands[ci].inter.is_some() {
        // C `dequants->y_dequant_qtx[base_q_idx][1]` (enc_inter_prediction.c
        // :2022-2025) — the FRAME-HEADER base qindex unconditionally (not
        // `ctx->qp_index`, not the `delta_q_present` selection), at the
        // `hbd_md`-selected table depth.
        let deq_ac = if bd10_rd.is_some() {
            i32::from(crate::bd10::ac_qlookup_10(frame.fh_qindex[0]))
        } else {
            i32::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[frame.fh_qindex[0] as usize])
        };
        let quantizer = i16::try_from(deq_ac).expect("y_dequant_qtx is int16_t in C");
        super::ifs::ifs_at_mds3(
            fx,
            g,
            lambda3,
            y_src,
            y_src_stride,
            y_src_off,
            quantizer,
            &mut cands[ci],
            bd10_rd,
        );
    }
    // ---- Luma: TX depth loop ----
    // RESOLVED 2026-09-05 — the "KNOWN GAP (pinned, screen-IBC grind
    // 2026-07-23)" that stood here said C keys this clamp on
    // `is_intra_mode(mode)` (so an IntraBC candidate gets the INTRA caps,
    // depth 2 at presets 0..3) but kept the inter caps because widening
    // them had produced 16 SELF-DESYNC cells "somewhere past the first
    // nonzero txb" of the depth-2 inter var-tx chain. That chain has since
    // gained C's z-order txb walk on BOTH the MD side (`txb_org_inter`) and
    // the pack (pipeline.rs `write_inter_txb_coeff` order), and the widened
    // caps now produce streams that decode under aomdec AND dav1d to the
    // port's own final recon on every gb82-sc cell tried. The note's second
    // half (a depth-2 IBC winner losing a cost comparison with every term
    // already equal to C's) was the MD txfm-context stamp — `commit.rs`,
    // same day. History: benchmarks/screen_ibc_map_2026-07-23.txt (22/100)
    // -> benchmarks/screen_ibc_map_2026-09-05.txt (150/150).
    // FIXED 2026-09-05: C's class clamp is keyed on the candidate's MODE —
    // `is_intra_mode(cand->block_mi.mode)` (product_coding_loop.c:6729-6732,
    // `is_intra_mode` = `mode < INTRA_MODE_END`, definitions.h:1614) — and
    // an IntraBC candidate is injected with `mode = DC_PRED`
    // (mode_decision.c:3150), so C searches an IBC block's tx depths under
    // the INTRA caps. Only a real inter MODE (NEWMV/NEARESTMV/...) takes the
    // inter caps. The port keyed this on `is_inter()` (= `use_intrabc ||
    // ref_frame[0] > INTRA_FRAME`, the wrong predicate for THIS site), which
    // capped IBC at the inter depth (1) where C reaches 2 at txs_level 2
    // (presets 0..3) — the mechanism behind every gb82-sc p0..p3 divergence
    // that begins at an IBC block's `txfm_partition` flag (terminal /
    // graph 512^2 q40 p2: first diverging op = the depth-0 split flag of the
    // 16x16 IBC leaf at mi(12,108), C=1 port=0). At txs_level 3 (p4..p7)
    // the square caps coincide (1 == 1), which is why p4 read clean.
    // The boundary rule (`in_frame`) precedes the class clamp in C and
    // applies to every class, so the inter arm takes it too.
    let cand_end_depth = if cands[ci].inter.is_some() {
        if txs_active && in_frame {
            end_tx_depth_inter(w, h, &cfg)
        } else {
            0
        }
    } else {
        end_depth
    };

    // C `get_start_end_tx_depth`'s bypass arm
    // (product_coding_loop.c:6811-6818): the MDS3 `bypass_tx_th`
    // shortcut — when MDS1 ran and left no coefficients, and the MDS0
    // distortion scaled by `bypass_tx_th` stays under the block-area *
    // qp product, C pins `start = end = 0` and evaluates depth-0
    // DCT-only (the transform is skipped entirely on the
    // `tx_search_skip_flag` path). This is PER-CANDIDATE, not
    // block-level: each candidate's own MDS1 state decides.
    let bypass_tx = perform_mds1
        && cfg.tx_shortcut.bypass_tx_th != 0
        && !cands[ci].mds1_has_coeff
        && cands[ci]
            .luma_fast_dist
            .saturating_mul(u64::from(cfg.tx_shortcut.bypass_tx_th))
            < u64::from((w * h) as u32) * u64::from(frame.base_qindex);
    let cand_end_depth = if bypass_tx { 0 } else { cand_end_depth };

    // C's MDS3 dispatch (product_coding_loop.c:6981-6997): when the
    // block is <=64 and `search_dct_dct_only` is true at depth 0 AND the
    // depth sweep is (0,0), C calls `perform_dct_dct_tx` — which has
    // only the `apply_pf_on_coeffs` pf_shape arm and NOT the
    // `use_tx_shortcuts_mds3` arm (:5725-5735). The alternative path is
    // `perform_tx_partitioning` -> per-txb `tx_type_search`, which has
    // BOTH arms (:4664-4676). This distinction is the ONLY thing that
    // separates the two N4 paths: when `use_tx_shortcuts_mds3` is set,
    // `search_dct_dct_only` is already true at every depth, so the
    // dispatch is decided by `end == 0` alone.
    let only_dct_d0 = {
        let c_tx = cc::tx_size_from_dims(w, h);
        let is_inter = cands[ci].is_inter();
        !frame.cfg.txt_on
            || use_tx_shortcuts_mds3
            || bypass_tx
            || w > 32
            || h > 32
            || cc::ext_tx_types(c_tx, is_inter, false) == 1
            || cc::ext_tx_set(c_tx, is_inter, false) == 0
    };
    let dct_tx_path = w <= 64 && h <= 64 && cand_end_depth == 0 && only_dct_d0;

    let mut best_depth = 0u8;
    let best_cost = u64::MAX;
    let mut best_bits: u64 = 0;
    let mut best_dist: u64 = 0;
    let mut best_txb_q: Vec<crate::vecpool::PoolVec<i32>> = Vec::new();
    let mut best_txb_eob: smallvec::SmallVec<[u16; 16]> = smallvec::SmallVec::new();
    let mut best_txb_cul: smallvec::SmallVec<[u8; 16]> = smallvec::SmallVec::new();
    let mut best_txb_type: smallvec::SmallVec<[u8; 16]> = smallvec::SmallVec::new();
    let mut best_recon: crate::vecpool::PoolVec<u8> = crate::vecpool::PoolVec::new();
    // The winning depth's TRUE 10-bit luma recon (bd10 full-RD only) —
    // the 10-bit twin of `best_recon`.
    let mut best_recon10: Vec<u16> = Vec::new();
    // The winning depth's luma PREDICTION, i.e. C `cand_bf->pred->y_buffer`
    // as it stands once the TX loop returns. NOT the same as `cand.pred`
    // (the MDS0 whole-block pred) whenever the winning depth > 0 — see the
    // detector call below for why the difference is observable.
    let mut best_pred: crate::vecpool::PoolVec<u8> = crate::vecpool::PoolVec::new();
    // The bd10 twin of `best_pred` — C's `cand_bf->pred->y_buffer` at
    // `hbd_md`, which is what `chroma_complexity_check_pred`'s SAD arm
    // reads (product_coding_loop.c:6049). Empty on every u8 path.
    let mut best_pred10: Vec<u16> = Vec::new();
    // The tx_depth-0 (whole-block-pred) recon, kept regardless of which
    // depth wins. C's `cand_bf->recon` is the SHARED ctx temp buffer:
    // deeper depths reconstruct into the AUX tx-depth buffers and
    // update_tx_cand_bf copies pred/coeffs/eob back but NEVER the recon —
    // so after the TX loop the shared recon still holds the DEPTH-0
    // recon, and that is what `calc_scr_to_recon_dist_per_quadrant`
    // (skip-sub-depth cond1 + the NSQ recon-dist gates) measures.
    // Proven on 1147124 q20 p4 (76,96): C fill luma quads sum 971<<4 ==
    // C's OWN depth-0 dist 15536, while the winning depth-1 dist is
    // 11904 (== this port's winner recon SSE).
    let mut d0_recon: crate::vecpool::PoolVec<u8> = crate::vecpool::PoolVec::new();
    // The 10-bit twin (`dep_recon10` at depth 0): the u16 `cand_bf->recon`
    // the bd10 quad-dist gates measure. Empty on the u8 path.
    let mut d0_recon10: Vec<u16> = Vec::new();
    let mut best_coeff_count = u32::MAX;

    // Coded-lossless: C `get_start_end_tx_depth` ends with "Force the use of
    // TX_4X4 for 8x8 block(s)": `if (pcs->mimic_only_tx_4x4 && sq_size == 8)
    // start = end = 1` (product_coding_loop.c:6734-6736), AFTER every other
    // rule — including the frame-boundary `end_tx_depth = 0` and the
    // `bypass_tx_th` shortcut — so a lossless 8x8 evaluates depth 1 only.
    // Lossless 4x4 blocks already take depth 0; only the 8x8 case needs
    // this override to force four TX_4X4 transforms.
    let (start_depth, cand_end_depth) = if frame.coded_lossless && w == 8 && h == 8 {
        (1u8, 1u8)
    } else {
        (0u8, cand_end_depth)
    };
    search_tx_depths(
        fx,
        bd10_rd,
        qt,
        lambda,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        cands,
        ci,
        sc,
        frame,
        rates,
        cfg,
        w,
        h,
        abs_x,
        abs_y,
        y_geom,
        filt_type_y,
        aligned_dims,
        end_depth,
        tsz_cat,
        tsz_ctx,
        lambda3,
        perform_mds1,
        use_tx_shortcuts_mds3,
        bypass_tx,
        dct_tx_path,
        &mut best_depth,
        best_cost,
        &mut best_bits,
        &mut best_dist,
        &mut best_txb_q,
        &mut best_txb_eob,
        &mut best_txb_cul,
        &mut best_txb_type,
        &mut best_recon,
        &mut best_recon10,
        &mut best_pred,
        &mut best_pred10,
        &mut d0_recon,
        &mut d0_recon10,
        &mut best_coeff_count,
        start_depth,
        cand_end_depth,
    );

    let chroma_luma = chroma_complexity_is_luma(
        fx,
        bd10_rd,
        y_src,
        y_src_stride,
        y_src_off,
        cands,
        ci,
        cfg,
        w,
        has_uv,
        cw,
        chh,
        ccx,
        ccy,
        &mut nb_u,
        &mut nb_v,
        uv_geom,
        filt_type_uv,
        use_tx_shortcuts_mds3,
        &best_pred,
        &best_pred10,
    );

    // Chroma N4 (full_loop.c:2240-2252): fires when the detector left
    // chroma_complexity at COMPONENT_LUMA and either
    // use_tx_shortcuts_mds3 forces it or the apply_pf_on_coeffs arm's
    // luma-coefficient test passes (post-MDS3 writeback, :7002-7007).
    // `txbwidth_uv`/`txbheight_uv` == `av1_get_max_uv_txsize` == the
    // C-coded (uncropped) chroma dims = cw/chh.
    let chroma_n4 = chroma_luma
        && (use_tx_shortcuts_mds3
            || (cfg.tx_shortcut.apply_pf_on_coeffs != 0
                && (best_coeff_count < (cw as u32 >> 4) * (chh as u32 >> 4)
                    || best_coeff_count == 0)));
    let chroma_gate = TxGate {
        n4: chroma_n4,
        ..TxGate::default()
    };

    // ---- Chroma full loop (uv per candidate: follows-luma at
    //      CHROMA_MODE_1, or the ind-uv table pick at chroma_level 4)
    //      + the complexity detector (CFL gate; see below) ----
    //      Skipped entirely for non-chroma-ref blocks (C gates every
    //      chroma stage on ctx->has_uv).
    let cand = &cands[ci];
    // The INTER chroma tx type the IBC arm derives below, so the bd10 twin
    // can use the SAME one instead of re-deriving it from the intra rule.
    let mut ibc_uv_tt: Option<usize> = None;
    let (mut u_out, mut v_out) = eval_inter_chroma(
        fx,
        cx,
        frame,
        rates,
        has_uv,
        cw,
        chh,
        ccx,
        ccy,
        uv_crop,
        qt_u,
        qt_v,
        cb_tsc,
        cb_dsc,
        cr_tsc,
        cr_dsc,
        &best_txb_eob,
        &best_txb_type,
        chroma_gate,
        cand,
        &mut ibc_uv_tt,
    );
    // bd10 chroma full loop — the decision terms for this candidate.
    let mut uv_out10 = match (&bd10_rd, has_uv) {
        // The INTER arm. It used to be a `panic!` saying the 10-bit chroma
        // prediction "is not built" — true at the time, and the reason a
        // 10-bit VIDEO frame was unreachable. `inter_md_arm` now produces it
        // with the luma in one `av1_inter_prediction_light_pd1_hbd` call, so
        // the arm scores C's own motion-compensated chroma rather than the
        // intra predictor's.
        //
        // The tx type is the same INTER rule the 8-bit arm above derives:
        // the luma winner if the chroma ext-tx set admits it, else DCT_DCT.
        // A candidate whose 10-bit chroma is EMPTY (no 10-bit reference in
        // the DPB) still refuses rather than scoring a prediction it does
        // not have.
        (Some(b), true) if cand.inter.is_some() => {
            let ic = cand.inter.as_ref().expect("matched inter");
            assert!(
                !ic.u_pred10.is_empty() && !ic.v_pred10.is_empty(),
                "the bd10 chroma full loop needs the inter candidate's 10-bit \
                 chroma prediction; it is empty, which means the DPB carried no \
                 10-bit twin of this reference"
            );
            let tt = inter_uv_tx_type(
                best_txb_eob.first().copied().unwrap_or(0),
                best_txb_type.first().copied().unwrap_or(0),
                frame.coded_lossless,
                cw,
                chh,
            );
            Some(chroma::eval_uv_inter_hbd(
                cx,
                fx,
                b,
                &ic.u_pred10,
                &ic.v_pred10,
                tt,
                chroma_gate,
            ))
        }
        (Some(b), true) => Some(match (cand.ibc, ibc_uv_tt) {
            // IBC: the DV copy at 10 bits, with the inter tx-type rule.
            (Some((dv, _)), Some(tt)) => chroma::eval_uv_ibc_hbd(cx, fx, b, dv, tt, chroma_gate),
            _ => chroma::eval_uv_hbd(cx, fx, b, cand.uv, cand.uv_delta, chroma_gate),
        }),
        // !has_uv: C runs NO chroma stage, so every chroma term is exactly
        // zero at either depth (TxUnitOut::absent()'s contract).
        _ => None,
    };
    // CfL override state, applied at the mutable-borrow writeback below.
    let mut uv_mode_final = cand.uv;
    let mut uv_delta_final = cand.uv_delta;
    let mut fcr_final = cand.fcr;
    let mut cfl_idx_final = 0u8;
    let mut cfl_signs_final = 0u8;
    eval_intra_chroma(
        fx,
        cx,
        bd10_rd,
        lambda,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        ind_uv,
        frame,
        rates,
        cfg,
        do_rdoq,
        w,
        h,
        abs_x,
        abs_y,
        has_uv,
        cfl_allowed,
        use_angle,
        cw,
        chh,
        ccx,
        ccy,
        nb_u,
        nb_v,
        uv_geom,
        filt_type_uv,
        uv_crop,
        qt_u,
        qt_v,
        cb_tsc,
        cb_dsc,
        cr_tsc,
        cr_dsc,
        allow_pal,
        pal_uv_no,
        pal_uv_no_y1,
        &best_recon,
        &best_recon10,
        best_pred,
        best_pred10,
        chroma_gate,
        cand,
        &mut u_out,
        &mut v_out,
        &mut uv_out10,
        &mut uv_mode_final,
        &mut uv_delta_final,
        &mut fcr_final,
        &mut cfl_idx_final,
        &mut cfl_signs_final,
    );

    // ---- svt_aom_full_cost (rd_cost.c:1357) ----
    // bd10 FULL-RD: the chroma eob/bits/dist that enter the block cost come
    // from the 10-bit chroma loop when it ran (the luma terms already do,
    // via `best_bits` / `best_dist`).
    let (uv_eob10, u_bits10, v_bits10, uv_dist10) = match &uv_out10 {
        Some((u, v)) => (
            (u.eob, v.eob),
            u.bits as u64,
            v.bits as u64,
            u.dist + v.dist,
        ),
        None => (
            (u_out.eob, v_out.eob),
            u_out.bits as u64,
            v_out.bits as u64,
            u_out.dist + v_out.dist,
        ),
    };
    let mut block_has_coeff = best_coeff_count > 0 || uv_eob10.0 > 0 || uv_eob10.1 > 0;
    // ---- C `blk_skip_decision` (rd_cost.c:1371-1406) ----
    //
    // An INTER block gets an explicit RD comparison between CODING its
    // residual and signalling `skip` (no coefficients at all). An intra
    // block does not — `is_inter_mode(cand->block_mi.mode)` gates it, and
    // `use_intrabc` is NOT part of that predicate here (C tests the MODE,
    // not `is_inter_block`), so an IntraBC candidate keeps its coefficients.
    //
    // Without it the funnel codes every inter residual it produces. On this
    // campaign's reference cell that is the whole remaining difference: C
    // commits `skip = 1` on a block whose MC prediction already matches, and
    // the port coded 452 luma coefficients against C's zero.
    //
    // `ctx->blk_skip_decision` is `uv_ctrls.uv_mode <= CHROMA_MODE_1`
    // (enc_mode_config.c:7858) — i.e. it is on exactly when MD evaluated
    // chroma, which on this path is `has_uv`.
    let mut skip_dist: Option<(u64, u64)> = None;
    // The prediction-domain distortion pair feeds TWO arbitrations: the
    // coefficient-skip one below (`blk_skip_decision`) and the skip-MODE one
    // after `full` is computed (`svt_aom_full_cost`, rd_cost.c:1423-1452),
    // which is NOT gated on `block_has_coeff` or `blk_skip_decision` — it
    // runs whenever `cand->skip_mode_allowed`, so the pred dists must exist
    // for it too.
    let sm_allowed = cand.inter.as_deref().is_some_and(|ic| ic.skip_mode_allowed);
    let mut pred_dists: Option<(u64, u64)> = None;
    // No `coded_lossless` exclusion: the skip-MODE arbitration below is not
    // gated on lossless either (C `svt_aom_full_cost`, rd_cost.c:1423-1452 —
    // `skip_mode_present` is a ref-config property, so a qp0 inter frame
    // still evaluates it), and it `expect`s these dists to exist whenever
    // `sm_allowed` is true. MEASURED: a 4-frame qp0 encode panicked here.
    if cand.inter.is_some() && ((block_has_coeff && has_uv) || sm_allowed) {
        // `y_distortion[DIST_SSD][1]` — the distortion with NO residual
        // coded, i.e. the prediction against the source, in the same
        // `sse << 4` domain the spatial arm of `tx_unit` produces.
        let (crop_w, crop_h) =
            crate::frame_geom::cropped_tx_dims(&aligned_dims, abs_x, abs_y, w, h);
        let ic = cand.inter.as_deref().expect("checked above");
        let (ucw, uch) = uv_crop;
        let (skip_y, skip_uv) = match bd10_rd.as_ref() {
            // `hbd_md = 2` arm (the still full-RD and the bypass-encdec bump
            // both land here): C's `svt_full_distortion_kernel16_bits` on the
            // 10-bit source vs the 10-bit prediction — `input_pic` and
            // `cand_bf->pred` are u16 under the bump (product_coding_loop.c
            // :9649-9663). The `<< 4` matches `tx_unit_hbd`'s spatial-dist
            // convention, so the arbitration below stays in one domain.
            Some(b) => {
                debug_assert!(
                    !cand.pred10.is_empty()
                        && (!has_uv || (!ic.u_pred10.is_empty() && !ic.v_pred10.is_empty())),
                    "bd10 MDS3 needs the 10-bit inter predictions"
                );
                let sy = (svtav1_dsp::hbd::full_distortion_kernel16_bits(
                    &b.y_src10,
                    0,
                    w,
                    &cand.pred10,
                    0,
                    w,
                    crop_w,
                    crop_h,
                ) << 4) as u64;
                let suv = if has_uv {
                    ((svtav1_dsp::hbd::full_distortion_kernel16_bits(
                        &b.u_src10,
                        0,
                        cw,
                        &ic.u_pred10,
                        0,
                        cw,
                        ucw,
                        uch,
                    ) + svtav1_dsp::hbd::full_distortion_kernel16_bits(
                        &b.v_src10,
                        0,
                        cw,
                        &ic.v_pred10,
                        0,
                        cw,
                        ucw,
                        uch,
                    )) << 4) as u64
                } else {
                    0
                };
                (sy, suv)
            }
            None => {
                let sy = (svtav1_dsp::variance::sse(
                    &y_src[y_src_off..],
                    y_src_stride,
                    &cand.pred,
                    w,
                    crop_w,
                    crop_h,
                ) << 4) as u64;
                let suv = if has_uv {
                    ((svtav1_dsp::variance::sse(
                        &fx.u_src[ccy * fx.c_stride + ccx..],
                        fx.c_stride,
                        &ic.u_pred,
                        cw,
                        ucw,
                        uch,
                    ) + svtav1_dsp::variance::sse(
                        &fx.v_src[ccy * fx.c_stride + ccx..],
                        fx.c_stride,
                        &ic.v_pred,
                        cw,
                        ucw,
                        uch,
                    )) << 4) as u64
                } else {
                    0
                };
                (sy, suv)
            }
        };
        pred_dists = Some((skip_y, skip_uv));
    }
    if let (Some((skip_y, skip_uv)), true) = (pred_dists, block_has_coeff && has_uv) {
        // C prices the NON-skip arm with the var-tx `tx_size` bits and the
        // skip arm with zero of them — the assert at rd_cost.c:1369 states
        // that `skip_tx_size_bits == 0` for every inter mode.
        let non_skip_tx_bits = if block_signals_txsize(w, h) && !frame.coded_lossless {
            crate::vartx::tx_size_bits_vartx(
                &rates.txfm_partition_fac_bits,
                fx.ectx.txfm_above_span(abs_x, w),
                fx.ectx.txfm_left_span(abs_y, h),
                w,
                h,
                best_depth,
                abs_y,
                frame.frame_h_px,
            )
        } else {
            0
        };
        let non_skip_cost = rdcost(
            lambda3,
            best_bits + u_bits10 + v_bits10 + non_skip_tx_bits + rates.skip[skip_ctx][0] as u64,
            best_dist + uv_dist10,
        );
        let skip_cost = rdcost(lambda3, rates.skip[skip_ctx][1] as u64, skip_y + skip_uv);
        if crate::dbgenv::skipdbg() {
            eprintln!(
                "RSKIP blk=({abs_x},{abs_y}) yb={best_bits} ub={u_bits10} vb={v_bits10} nstx={non_skip_tx_bits} sf0={} sf1={} yres={best_dist} uvres={uv_dist10} ypred={skip_y} uvpred={skip_uv} nsc={non_skip_cost} sc={skip_cost} lam={lambda3} -> {}",
                rates.skip[skip_ctx][0],
                rates.skip[skip_ctx][1],
                if skip_cost < non_skip_cost {
                    "SKIP"
                } else {
                    "KEEP"
                }
            );
        }
        if skip_cost < non_skip_cost {
            skip_dist = Some((skip_y, skip_uv));
            block_has_coeff = false;
        }
    } else if crate::dbgenv::skipdbg() && cand.inter.is_some() {
        eprintln!(
            "RSKIP blk=({abs_x},{abs_y}) ARM-MISS bhc={block_has_coeff} has_uv={has_uv} pred_dists={}",
            pred_dists.is_some()
        );
    }
    // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
    // IntraBC: svt_aom_full_cost prices non_skip_tx_size_bits = the
    // var-tx walk (block_has_coeff) and skip_tx_size_bits = 0
    // (rd_cost.c:1367-1377 + the `!(is_inter_tx && skip)` gate).
    let tx_size_bits_final = if cand.is_inter() {
        if block_has_coeff && block_signals_txsize(w, h) && !frame.coded_lossless {
            crate::vartx::tx_size_bits_vartx(
                &rates.txfm_partition_fac_bits,
                fx.ectx.txfm_above_span(abs_x, w),
                fx.ectx.txfm_left_span(abs_y, h),
                w,
                h,
                best_depth,
                abs_y,
                frame.frame_h_px,
            )
        } else {
            0
        }
    } else if block_signals_txsize(w, h) && !frame.coded_lossless {
        rates.tx_size[tsz_cat][tsz_ctx][best_depth as usize] as u64
    } else {
        0
    };
    // Chroma coeff rate. M6 (coeff_rate_est_lvl 1) prices the real
    // cost_coeffs_txb / cost_skip_txb (already in u_out.bits/v_out.bits):
    // C `skip_chroma_rate_est` returns false immediately at lvl 1, so the
    // caller runs the full estimate into a zeroed accumulator — clean.
    //
    // M7/M8 (lvl 2) + eff-M9 (lvl 0) go through C `skip_chroma_rate_est`
    // (full_loop.c:1922, th = (tx_w_uv * tx_h_uv) >> 6) — which we must
    // replicate byte-for-byte INCLUDING an order-dependent CB double-count.
    // skip_chroma_rate_est writes the CB approximation STRAIGHT INTO the
    // `*cb_coeff_bits` accumulator when `cb_eob < th`, then (lvl 2)
    // `return false` at the CR check when `cr_eob >= th` WITHOUT clearing
    // the CB write; the caller (svt_aom_full_loop_uv, full_loop.c:2636-2661)
    // then does `*cb_coeff_bits += cb_txb_coeff_bits` (the full estimate).
    // So in the `cb_eob < th && cr_eob >= th` case ONLY, CB is priced as
    // approx + full. CR never double-counts (CB is checked first; a `>= th`
    // CB `return false`s before the CR branch writes anything). At lvl 0 the
    // function never returns false — each plane gets `1500+eob*50` for
    // eob >= th — so it stays a clean per-plane approximation.
    // Instrumented C 2026-07-15: SB(224,192) q40 p7 H_PRED chroma
    // cb = 4500 approx + 6246 full = 10746, cr = 12848 (DC candidate cb
    // clean: cb_eob=6 >= th so CB returns before leaking). Pricing CB
    // clean (6246) undercharged the H candidate ~4500 and flipped the
    // leaf y_mode from C's DC to our H.
    let (u_bits, v_bits) = if cfg.real_coeff_ctx {
        (u_bits10, v_bits10)
    } else {
        let lvl = cfg.coeff_rate_est_lvl;
        let th = ((cw * chh) >> 6) as u16;
        let approx = |eob: u16| -> u64 {
            if eob == 0 {
                0
            } else if eob < th {
                3000 + eob as u64 * 500
            } else {
                1500 + eob as u64 * 50 // lvl-0 `eob >= th` fallback
            }
        };
        let mut cb_leak = 0u64;
        let mut cr_leak = 0u64;
        let mut need_full = false;
        // CB branch of skip_chroma_rate_est (checked first).
        if uv_eob10.0 < th || lvl == 0 {
            cb_leak = approx(uv_eob10.0);
        } else {
            need_full = true; // lvl-2, cb_eob >= th -> return false (nothing leaked)
        }
        // CR branch — only reached when CB didn't already force full.
        if !need_full {
            if uv_eob10.1 < th || lvl == 0 {
                cr_leak = approx(uv_eob10.1);
            } else {
                need_full = true; // lvl-2, cr_eob >= th -> return false (CB leak stays)
            }
        }
        if need_full {
            // Caller runs the full estimate and ADDS it to the accumulator.
            (cb_leak + u_bits10, cr_leak + v_bits10)
        } else {
            (cb_leak, cr_leak)
        }
    };
    let coeff_rate = if block_has_coeff {
        best_bits + u_bits + v_bits + tx_size_bits_final + rates.skip[skip_ctx][0] as u64
    } else {
        rates.skip[skip_ctx][1] as u64 + tx_size_bits_final
    };
    let mut dist = skip_dist.map_or(best_dist + uv_dist10, |(y, uv)| y + uv);
    // fcr_final == cand.fcr unless CfL was selected above (then the
    // UV_CFL_PRED mode + alpha rate replaces the non-CFL uv fast rate).
    let mut total_rate = cand.flr + fcr_final + coeff_rate;
    let mut full = rdcost(lambda3, total_rate, dist);
    // ---- C `svt_aom_full_cost`'s skip-MODE arm (rd_cost.c:1423-1452) ----
    //
    // When the candidate's ref pair is the frame's skip-mode pair, C prices
    // coding the WHOLE block as `skip_mode = 1`: every mode-info symbol
    // collapses into `skip_mode_fac_bits[skip_mode_ctx][1]`, so
    // `mode_rate`/`mode_distortion` are REPLACED by the symbol rate and the
    // prediction-domain distortion — not added to. `<=`: C takes skip mode
    // on a tie. The arm is NOT gated on `blk_skip_decision`; it runs
    // whenever `cand->skip_mode_allowed`.
    let mut skip_mode_win = false;
    if let Some(ic) = cand.inter.as_deref() {
        if ic.skip_mode_allowed {
            let (sy, suv) =
                pred_dists.expect("a skip-mode candidate computed its prediction dists");
            let sm_rate = fx
                .inter
                .expect("an inter candidate implies inter frame state")
                .fac
                .skip_mode[ic.skip_mode_ctx as usize][1] as u64;
            let sm_cost = rdcost(lambda3, sm_rate, sy + suv);
            #[cfg(feature = "std")]
            if crate::dbgenv::skipdbg() {
                eprintln!(
                    "SKMDEC blk=({abs_x},{abs_y}) mode={:?} rf={:?} skmctx={} smr={} sdist={} smc={} full={} -> {}",
                    ic.mode,
                    ic.ref_frame,
                    ic.skip_mode_ctx,
                    sm_rate,
                    sy + suv,
                    sm_cost,
                    full,
                    if sm_cost <= full { "SKM" } else { "keep" }
                );
            }
            if sm_cost <= full {
                full = sm_cost;
                total_rate = sm_rate;
                dist = sy + suv;
                // Reuse the coefficient-skip writeback below: skip mode
                // zeroes exactly the same candidate artefacts (C sets
                // `block_has_coeff = 0`, tx_depth 0, DCT_DCT, recon=pred).
                skip_dist = Some((sy, suv));
                skip_mode_win = true;
            }
        }
    }
    #[cfg(feature = "std")]
    if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
        eprintln!(
            "NSQDBG CAND mi=({},{}) {}x{} ci={} mode={} fi={} delta={} uv={} ibc={} txd={} enddepth={} flr={} fcr={} coeff_rate={} dist={} full={}",
            abs_y / 4,
            abs_x / 4,
            w,
            h,
            ci,
            cand.mode,
            cand.fi,
            cand.delta,
            uv_mode_final,
            u8::from(cand.is_inter()),
            best_depth,
            cand_end_depth,
            cand.flr,
            fcr_final,
            coeff_rate,
            dist,
            full,
        );
    }

    let cand = &mut cands[ci];
    // C resets `block_mi.skip_mode` inside the `skip_mode_allowed` gate at
    // EVERY stage (rd_cost.c:1438), so a flag an earlier stage set clears
    // here when the arm re-evaluates and loses — and only inside the gate.
    if let Some(ic) = cand.inter.as_deref_mut() {
        if ic.skip_mode_allowed {
            ic.skip_mode = skip_mode_win;
        }
    }
    // C's skip arm zeroes every coded artefact of the candidate
    // (rd_cost.c:1387-1405): no coefficients, no eobs, tx_depth 0 and
    // DCT_DCT on every txb — "signalling skip means no TX depth is used and
    // the TX type will be DCT_DCT". The RECON becomes the prediction, which
    // is what a decoder reconstructs from a skip block and therefore what
    // the next block's neighbours must read.
    if let Some((skip_y, _)) = skip_dist {
        // The tune-SSIM parallel cost below this writeback has no inter arm:
        // it would need the block-SSIM distortion of a prediction-only recon.
        // REFUSE rather than leave `mds3_cost_ssim` at MAX and let the winner
        // scan compare a real cost against a sentinel. The pipeline refuses
        // inter frames under `alt_ssim_tuning` (encode_frame_impl), so this is
        // unreachable — and an `assert!`, not a
        // `debug_assert!`, because `identity_run` builds RELEASE and
        // `docs/INTER-ENCODE-PLAN.md` §1x records a defect a debug-only check
        // hid for exactly that reason.
        assert!(
            !frame.tune_ssim,
            "the tune-SSIM parallel full cost has no INTER skip arm"
        );
        let ic = cand
            .inter
            .as_deref()
            .expect("the skip decision only runs for an inter candidate");
        // Recon = pred. One copy per destination buffer — the previous shape
        // cloned into a temporary AND then `from_slice`d it into a second
        // pooled buffer, paying two copies of every w*h plane per skip.
        cand.mds3_cost = full;
        cand.total_rate = total_rate;
        cand.full_dist = dist;
        cand.uv = uv_mode_final;
        cand.uv_delta = uv_delta_final;
        cand.fcr = fcr_final;
        cand.cfl_alpha_idx = 0;
        cand.cfl_alpha_signs = 0;
        cand.tx_depth = 0;
        cand.txb_q = alloc::vec![zeroed_pool::<i32>(w * h)];
        cand.txb_eob = smallvec::smallvec![0u16];
        cand.txb_cul = smallvec::smallvec![0u8];
        cand.txb_type = smallvec::smallvec![cc::DCT_DCT as u8];
        cand.y_recon = cand.pred.clone();
        cand.y_recon_d0 = cand.pred.clone();
        cand.y_bits = 0;
        cand.y_dist = skip_y;
        cand.u_q = zeroed_pool::<i32>(cw * chh);
        cand.v_q = zeroed_pool::<i32>(cw * chh);
        cand.u_eob = 0;
        cand.v_eob = 0;
        cand.u_cul = 0;
        cand.v_cul = 0;
        cand.u_recon = crate::vecpool::PoolVec::from_slice(&ic.u_pred);
        cand.v_recon = crate::vecpool::PoolVec::from_slice(&ic.v_pred);
        // Populate the 10-bit recons only where a 10-bit canvas consumes
        // them — `commit_leaf` asserts a canvas exists for every non-empty
        // chroma recon, and the pred10 buffers can be populated on paths
        // (e.g. the bd10 post-pass presets) whose canvases are absent.
        if fx.y_recon10.is_some() {
            cand.y_recon10_d0 = cand.pred10[..].to_vec();
            cand.y_recon10 = cand.pred10[..].to_vec();
        }
        if fx.u_recon10.is_some() && fx.v_recon10.is_some() {
            cand.u_recon10 = ic.u_pred10.clone();
            cand.v_recon10 = ic.v_pred10.clone();
        }
        cand.block_has_coeff = false;
        return;
    }
    cand.mds3_cost = full;
    cand.total_rate = total_rate;
    cand.full_dist = dist;
    cand.uv = uv_mode_final;
    cand.uv_delta = uv_delta_final;
    cand.fcr = fcr_final;
    cand.cfl_alpha_idx = cfl_idx_final;
    cand.cfl_alpha_signs = cfl_signs_final;
    cand.tx_depth = best_depth;
    cand.txb_q = best_txb_q;
    cand.txb_eob = best_txb_eob;
    cand.txb_cul = best_txb_cul;
    cand.txb_type = best_txb_type;
    cand.y_recon = best_recon;
    cand.y_recon_d0 = d0_recon;
    cand.y_recon10_d0 = d0_recon10;
    cand.y_bits = best_bits;
    cand.y_dist = best_dist;
    // Chroma coded levels / eobs / neighbour culs — 10-bit when the bd10
    // chroma full loop ran, for the same reason as luma above.
    match &uv_out10 {
        Some((u10, v10)) => {
            cand.u_q = crate::vecpool::PoolVec::from_slice(&u10.qcoeff);
            cand.v_q = crate::vecpool::PoolVec::from_slice(&v10.qcoeff);
            cand.u_eob = u10.eob;
            cand.v_eob = v10.eob;
            cand.u_cul = u10.cul;
            cand.v_cul = v10.cul;
            // NOTE: the u8 chroma recon is NOT set here. See the
            // unconditional assignment after this match and the measurement
            // that put it back there.
        }
        None => {
            cand.u_q = u_out.qcoeff;
            cand.v_q = v_out.qcoeff;
            cand.u_eob = u_out.eob;
            cand.v_eob = v_out.eob;
            cand.u_cul = u_out.cul;
            cand.v_cul = v_out.cul;
        }
    }
    // THE U8-QUANTIZER RECON, UNCONDITIONALLY — including on a bd10 leaf whose
    // 10-bit chroma full loop DID run.
    //
    // `3d8f5c517` moved this pair into the `None` arm above, on the reasoning
    // that the stored u8 recon must represent the CODED levels: at bd10 the
    // true recon is 10-bit, the post-filter searches are still 8-bit, so the
    // proxy should be the truncated 10-bit recon (the convention
    // `bd10_reencode_chroma_plane` uses). That commit recorded, honestly, that
    // it was byte-inert on every cell it could measure.
    //
    // IT IS NOT INERT, and C disagrees with it. MEASURED on CID22-512
    // `1484678` at bd10 q32 preset 5 — a cell `bd10_photo_gate.sh` group F
    // added LATER, which is why 3d8f5c517 could not have seen it:
    //
    //     truncated-10-bit proxy   port 9505 B   C 9501 B   DIVERGES
    //     u8-quantizer recon       port 9501 B   C 9501 B   IDENTICAL
    //
    // Bisected to 3d8f5c517 over the 1062 commits since group F's parent, one
    // build per step. So the "dead code" that commit removed was the behaviour
    // that matches C, and the branch it restored is the one that does not.
    //
    // The likely why, stated as a hypothesis and not as a finding: at these
    // presets C's own mode decision is 8-bit (`hbd_md`), so C's chroma recon
    // — the one its post-filter searches and its quad-dist gate read — is the
    // u8 quantizer's, exactly as this line stores. The port's bd10 full-RD
    // funnel is a 10-bit MD that C may not be running at all here, which is
    // the same open question the 10-bit VIDEO divergence points at
    // (benchmarks/bd10_video_2026-09-10.meta).
    cand.u_recon = u_out.recon;
    cand.v_recon = v_out.recon;
    if let Some((u10, v10)) = uv_out10.take() {
        cand.u_recon10 = u10.recon;
        cand.v_recon10 = v10.recon;
    }
    cand.y_recon10 = core::mem::take(&mut best_recon10);
    cand.block_has_coeff = block_has_coeff;
    // [SVT_HDR_MODE] alt-ssim-tuning: the parallel SSIM full cost —
    // same lambda and total rate, block-SSIM distortion on the FINAL
    // per-plane recon (C accumulates DIST_SSIM per txb with cropped
    // dims; whole-block equals the per-txb sum whenever the 8x8/4x4
    // tiling aligns with txb boundaries, which holds for the funnel's
    // square/half tx shapes).
    // PORT-NOTE(unverified): fork alt-ssim full_cost_ssim vs C — needs
    // a C-side MD dump with alt_ssim_tuning=1 (tune_ssim_level LVL_1).
    if frame.tune_ssim {
        let cand = &cands[ci];
        let mut ssim_dist = crate::ssim_md::spatial_full_distortion_ssim(
            y_src,
            y_src_off,
            y_src_stride,
            &cand.y_recon,
            0,
            w,
            w,
            h,
            frame.ac_bias_eff,
        );
        if !cand.u_recon.is_empty() {
            let c_off = ccy * fx.c_stride + ccx;
            ssim_dist += crate::ssim_md::spatial_full_distortion_ssim(
                fx.u_src,
                c_off,
                fx.c_stride,
                &cand.u_recon,
                0,
                cw,
                cw,
                chh,
                frame.ac_bias_eff,
            );
            ssim_dist += crate::ssim_md::spatial_full_distortion_ssim(
                fx.v_src,
                c_off,
                fx.c_stride,
                &cand.v_recon,
                0,
                cw,
                cw,
                chh,
                frame.ac_bias_eff,
            );
        }
        let total_rate = cand.total_rate;
        cands[ci].mds3_cost_ssim = rdcost(lambda, total_rate, ssim_dist);
    }
}

mod tx_depth;
use tx_depth::*;

mod uv_stage;
use uv_stage::*;

mod intra_chroma;
use intra_chroma::*;
