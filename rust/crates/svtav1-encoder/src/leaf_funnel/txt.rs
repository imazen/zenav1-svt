//! Per-txb transform-type search.
//!
//! C `tx_type_search` (product_coding_loop.c:4660): the candidate tx-type
//! group for this block size and depth, the SATD early exit, and the RD pick.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// The 10-bit inputs one [`txt_search`] txb needs to run at true depth.
pub(super) struct Bd10Txb<'a> {
    /// Block-local 10-bit source and this txb's origin inside it.
    pub(super) src10: &'a [u16],
    pub(super) src10_stride: usize,
    pub(super) src10_off: usize,
    /// This txb's 10-bit prediction (txw*txh at stride txw).
    pub(super) pred10: &'a [u16],
    pub(super) qt: &'a QuantTable,
    pub(super) lambda: u64,
    pub(super) bd: u8,
}

/// txb skip / dc sign contexts from TX-local (block-span) overlay arrays.
/// `spans` are the block's above/left coeff-byte slices (4x4 units);
/// txb at (tx_x, tx_y) within the block, `tx` square dims.
pub(super) fn txb_ctx_from_spans(
    above_span: &[u8],
    left_span: &[u8],
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    block_eq_tx: bool,
) -> (usize, usize) {
    // A txb of a leaf that STRADDLES the aligned frame edge (partial-SB path,
    // task #95) can sit entirely past the frame extent — its 4x4 origin then
    // exceeds the block's coeff-context span, which `above_coeff_span` /
    // `left_coeff_span` already clip to the in-frame extent. Clamp the START so
    // the slice is empty rather than panicking (start > end). An empty span is
    // exactly what `get_txb_ctx` treats as "no coded neighbour" (== a 0xFF /
    // INVALID entry -> zero contribution), which is the context of an off-frame
    // neighbour — so this is byte-neutral for every in-frame txb (start <= len,
    // clamp is a no-op) and gives the off-frame txb the unavailable-neighbour
    // context. C reads its SB-extent-padded neighbour arrays here; the off-frame
    // cells were never coded, so C's contribution is likewise zero.
    let a0 = (tx_x / 4).min(above_span.len());
    let l0 = (tx_y / 4).min(left_span.len());
    let a = &above_span[a0..(a0 + txw / 4).min(above_span.len())];
    let l = &left_span[l0..(l0 + txh / 4).min(left_span.len())];
    cc::get_txb_ctx(0, a, l, block_eq_tx, false)
}

/// `SVTAV1_TXT_XY="x,y"` per-candidate TXT-search dump tag (block org +
/// txb identity), mirroring the sibling-C `SVT_TXT_OUT` instrument lines.
#[derive(Clone, Copy)]
pub(super) struct TxtDbg {
    pub(super) abs_x: usize,
    pub(super) abs_y: usize,
    pub(super) tx_x: usize,
    pub(super) tx_y: usize,
    pub(super) mode: u8,
    pub(super) fi: u8,
}

/// TXT search for one luma txb (`tx_type_search`, product_coding_loop.c:
/// 4660): DCT-only above 16x16 intra (ext-tx set), otherwise the intra
/// tx-type groups with SATD early exit + rate-cost gate. Returns the best
/// type's unit output.
///
/// `crop` is this txb's cropped-TX distortion extent — C computes it ONCE
/// per txb at :4664-4665 (and identically in `perform_dct_dct_tx` at
/// :5752-5754), i.e. OUTSIDE the tx-type loop, so every candidate type is
/// scored over the same in-frame region. Passed in rather than derived
/// here because the caller owns the absolute txb origin.
#[allow(clippy::too_many_arguments)]
pub(super) fn txt_search(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
    crop: (usize, usize),
    depth: u8,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    lambda: u64,
    bd10: Option<&Bd10Txb<'_>>,
    dbg: Option<TxtDbg>,
    rate_mode: RateMode,
) -> (TxUnitOut, Option<TxUnitOutHbd>, usize) {
    macro_rules! txt_dbg {
        ($($t:tt)*) => {
            #[cfg(feature = "std")]
            if let Some(d) = &dbg {
                eprint!(
                    "PTXT org=({},{}) tx=({},{}) {w}x{h} d={depth} mode={} fi={} ",
                    d.abs_x, d.abs_y, d.tx_x, d.tx_y, d.mode, d.fi
                );
                eprintln!($($t)*);
            }
        };
    }
    let c_tx = cc::tx_size_from_dims(w, h);
    // IBC chunk 7: the INTER_TXT_DIR sentinel marks an IntraBC txb — the
    // whole search then runs over the INTER ext-tx machinery
    // (tx_type_search's `is_inter`, product_coding_loop.c:4597-4601).
    let is_inter = intra_dir == INTER_TXT_DIR;
    // search_dct_dct_only (product_coding_loop.c:4601): txt disabled
    // (eff-M9 txt_level 0 -> !mds_do_txt), dims > 32, a single-type ext
    // set, or ext set index 0.
    let only_dct = !frame.cfg.txt_on
        || w > 32
        || h > 32
        || cc::ext_tx_types(c_tx, is_inter, false) == 1
        || cc::ext_tx_set(c_tx, is_inter, false) == 0;
    // get_tx_type_group (product_coding_loop.c:4358): per-preset intra
    // group counts (M6 txt_level 8: ge16 4 / lt16 5; M5 txt_level 3:
    // 6 / 6 — the dump's txt_ge16/txt_lt16); depth-1 offset 3 (min 1).
    // INTER groups: at every IBC preset (M0-M4, txt_level 2/3) the C
    // inter group counts EQUAL the intra ones (both MAX=6/6,
    // set_txt_controls cases 2-3, enc_mode_config.c:3927-3955), so the
    // intra config fields are reused; presets >= M5 have allow_intrabc=0
    // so the inter arm is unreachable there.
    let mut groups: i32 = if only_dct {
        1
    } else if w >= 16 && h >= 16 {
        frame.cfg.txt_group_ge16
    } else {
        frame.cfg.txt_group_lt16
    };
    if depth == 1 && !only_dct {
        groups = (groups - frame.cfg.txt_d1_off).max(1);
    } else if depth == 2 && !only_dct {
        groups = (groups - frame.cfg.txt_d2_off).max(1);
    }

    const TX_TYPE_GROUPS: [&[usize]; 6] = [
        &[cc::DCT_DCT],
        &[10, 11], // V_DCT, H_DCT
        &[3],      // ADST_ADST
        &[1, 2],   // ADST_DCT, DCT_ADST
        &[6, 9],   // FLIPADST_FLIPADST, IDTX
        &[4, 5, 7, 8, 12, 13, 14, 15],
    ];

    let set_type = cc::ext_tx_set_type(c_tx, is_inter, false);
    // qp-scaled SATD early-exit th (satd_th_q_weight = 1; intra th 10 at
    // M6, 15 at M5 — txt_satd_intra in the dumps). INTER th: equal to the
    // intra th at every IBC preset (M0-M3: 20/20, M4: 15/15 —
    // set_txt_controls cases 2-3), so the intra field is reused (same
    // reasoning as the group counts above).
    let (qw, qwd) = qp_scale_factors(frame.cli_qp);
    let satd_th = if only_dct {
        0
    } else {
        div_round(frame.cfg.txt_satd_th * qw, qwd)
    } as i64;

    // C's level-0 closed form replaces `out.bits`, which also feeds the
    // per-tx-type `cost` compare below. That is inert only when exactly ONE
    // type is evaluated (`only_dct`), where `best` is assigned on the sole
    // iteration whatever the cost. `coeff_rate_est_lvl == 0` implies
    // `txt_on == false` (`FunnelCfg::for_preset`'s `_ =>` arm sets both), which
    // implies `only_dct`, so the demotion below never fires today — it is a
    // structural guard so a future preset table cannot make the search
    // rate-sensitive behind our back.
    let rate_mode = if only_dct { rate_mode } else { RateMode::Exact };
    debug_assert!(only_dct || rate_mode == RateMode::Exact);

    let mut best: Option<TxUnitOut> = None;
    // The bd10 twin of the SELECTED type (not of the u8-best type): when the
    // bd10 context is present the winner is chosen by the 10-bit cost, so both
    // outputs must come from the same tx_type.
    let mut best10: Option<TxUnitOutHbd> = None;
    let mut best_type = cc::DCT_DCT;
    let mut best_cost = u64::MAX;
    let mut dct_cost = u64::MAX;
    let mut best_satd = i64::MAX;

    'groups: for g in 0..groups as usize {
        for &tx_type in TX_TYPE_GROUPS[g] {
            if only_dct && tx_type != cc::DCT_DCT {
                continue;
            }
            if tx_type != cc::DCT_DCT {
                if AV1_EXT_TX_USED[set_type][tx_type] == 0 {
                    continue;
                }
                // txt_rate_cost_th (100 at M6, 250 at M5): skip types
                // whose signalling rate alone exceeds the DCT cost
                // fraction (product_coding_loop.c:4710-4716).
                //
                // The lambda here MUST be the same one that produced
                // `dct_cost`, or the gate compares two different scales. C
                // uses ONE `full_lambda` for both — `ctx->hbd_md ?
                // full_lambda_md[EB_10_BIT_MD] : full_lambda_md[EB_8_BIT_MD]`
                // (:4590) — in the gate at :4714 AND in the cost at :4944.
                // The port had the cost on the bd10 lambda but the gate on the
                // u8 one, so at bd10 the left side stayed 8-bit-scaled while
                // `dct_cost` was 10-bit-scaled: the gate under-fired and the
                // port evaluated (and sometimes picked) tx types C prunes
                // before it ever quantizes them. `bd10.map_or` mirrors
                // `lambda3`, so bd8 is byte-unchanged by construction.
                let gate_lambda = bd10.map_or(lambda, |b| b.lambda);
                let tx_type_rate = rates.txt_rate(c_tx, intra_dir, tx_type) as u64;
                if dct_cost != u64::MAX
                    && rdcost(gate_lambda, tx_type_rate, 0) * 1000
                        > dct_cost * frame.cfg.txt_rate_th
                {
                    txt_dbg!(
                        "cand txt={tx_type} SKIP rate_gate rate={tx_type_rate} dctcost={dct_cost}"
                    );
                    continue;
                }
                txt_dbg!(
                    "cand txt={tx_type} rate_gate_pass rate={tx_type_rate} dctcost={dct_cost}"
                );
            }
            let out = tx_unit(
                src,
                src_stride,
                src_off,
                pred,
                w,
                0,
                w,
                h,
                tx_type,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                intra_dir,
                qt,
                frame,
                rates,
                do_rdoq,
                true, // MDS3 spatial dist
                crop,
                true,
                rate_mode,
            );
            // bd10 FULL-RD (task #94): the same TX unit at true depth. Every
            // gate around it (group order, ext-tx set, the rate-cost th, the
            // SATD early exit, the non-signalable-eob rule) is bit-depth
            // INDEPENDENT — only the residual, the quant table, the lambda and
            // the distortion move — so the search structure is shared and only
            // the COST source switches.
            let out10 = bd10.map(|b| {
                tx_unit_hbd(
                    b.src10,
                    b.src10_stride,
                    b.src10_off,
                    b.pred10,
                    w,
                    0,
                    w,
                    h,
                    tx_type,
                    0,
                    txb_skip_ctx,
                    dc_sign_ctx,
                    b.qt,
                    frame.rdoq_level,
                    b.lambda,
                    frame.sharpness,
                    rates,
                    do_rdoq,
                    b.bd,
                    b.qt.qm_level,
                    Some(&TxRdArgs {
                        spatial_dist: true, // MDS3
                        intra_dir,
                        coeff_rate_est_lvl: frame.cfg.coeff_rate_est_lvl,
                        tx_bias: frame.tx_bias,
                        crop,
                    }),
                )
            });
            // SATD early exit between transform and quantize in C; we
            // apply it post-hoc on the transform coefficients via a
            // dedicated pass only when the th is armed.
            if satd_th > 0 {
                let satd = match bd10 {
                    Some(b) => txb_coeff_satd_hbd(
                        b.src10,
                        b.src10_stride,
                        b.src10_off,
                        b.pred10,
                        w,
                        h,
                        tx_type,
                    ),
                    None => txb_coeff_satd(src, src_stride, src_off, pred, w, h, tx_type),
                };
                txt_dbg!(
                    "cand txt={tx_type} satd={satd} best_satd={best_satd} skip={}",
                    i32::from(satd >= best_satd && (satd - best_satd) * 100 > best_satd * satd_th)
                );
                if satd < best_satd {
                    best_satd = satd;
                } else if (satd - best_satd) * 100 > best_satd * satd_th {
                    continue;
                }
            }
            // A non-DCT type with no coefficients is not signalable.
            let dec_eob = out10.as_ref().map_or(out.eob, |o| o.eob);
            txt_dbg!("cand txt={tx_type} eob={dec_eob}");
            if dec_eob == 0 && tx_type != cc::DCT_DCT {
                txt_dbg!("cand txt={tx_type} SKIP eob0");
                continue;
            }
            let cost = match (&out10, bd10) {
                (Some(o), Some(b)) => rdcost(b.lambda, o.bits as u64, o.dist),
                _ => rdcost(lambda, out.bits as u64, out.dist),
            };
            #[cfg(feature = "std")]
            if dbg.is_some() {
                let (b_, d_) = match &out10 {
                    Some(o) => (o.bits, o.dist),
                    None => (out.bits, out.dist),
                };
                txt_dbg!(
                    "cand txt={tx_type} bits={b_} dist={d_} cost={cost} best={best_cost}{}",
                    if cost < best_cost { " NEW_BEST" } else { "" }
                );
            }
            if cost < best_cost {
                best_cost = cost;
                best_type = tx_type;
                if tx_type == cc::DCT_DCT {
                    dct_cost = cost;
                }
                best = Some(out);
                best10 = out10;
            } else if tx_type == cc::DCT_DCT {
                dct_cost = cost;
            }
            if only_dct {
                break 'groups;
            }
        }
    }
    txt_dbg!("WINNER txt={best_type} cost={best_cost}");
    (best.expect("DCT_DCT always evaluated"), best10, best_type)
}
