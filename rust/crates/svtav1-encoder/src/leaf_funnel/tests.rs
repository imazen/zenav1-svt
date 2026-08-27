//! Unit tests for the leaf funnel.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25 — this was `mod tests` in
//! that file, moved verbatim and de-indented one level.

use super::*;

/// IBC chunk 7: the inter txb origins must reproduce the C
/// `tx_org[bsize][is_inter=1]` rows exactly. Depths 0/1 equal the
/// intra raster everywhere; depth 2 is the var-tx z-order on exactly
/// 6 bsizes (values extracted from transforms.c:48 during the chunk-7
/// landing — the 16X8/16X16 rows locked verbatim here, the others by
/// the parent-major rule those two pin).
#[test]
fn inter_txb_origins_match_c_tx_org() {
    // Depth 0/1: identical to the intra raster for every dim pair.
    for &(w, h) in &[(64, 64), (32, 16), (16, 8), (8, 8), (16, 64)] {
        for depth in 0..=1u8 {
            let (txw, txh) = txb_dims_at_depth(w, h, depth);
            let cols = w / txw;
            let n = cols * (h / txh);
            for txb in 0..n {
                assert_eq!(
                    txb_org_inter(w, h, depth, txb),
                    ((txb % cols) * txw, (txb / cols) * txh),
                    "{w}x{h} d{depth} txb{txb}"
                );
            }
        }
    }
    // Depth 2, BLOCK_16X8 (C inter row):
    // {0,0},{4,0},{0,4},{4,4},{8,0},{12,0},{8,4},{12,4}.
    let c_16x8: [(usize, usize); 8] = [
        (0, 0),
        (4, 0),
        (0, 4),
        (4, 4),
        (8, 0),
        (12, 0),
        (8, 4),
        (12, 4),
    ];
    for (i, &xy) in c_16x8.iter().enumerate() {
        assert_eq!(txb_org_inter(16, 8, 2, i), xy, "16x8 d2 txb{i}");
    }
    // Depth 2, BLOCK_16X16 (C inter row).
    let c_16x16: [(usize, usize); 16] = [
        (0, 0),
        (4, 0),
        (0, 4),
        (4, 4),
        (8, 0),
        (12, 0),
        (8, 4),
        (12, 4),
        (0, 8),
        (4, 8),
        (0, 12),
        (4, 12),
        (8, 8),
        (12, 8),
        (8, 12),
        (12, 12),
    ];
    for (i, &xy) in c_16x16.iter().enumerate() {
        assert_eq!(txb_org_inter(16, 16, 2, i), xy, "16x16 d2 txb{i}");
    }
    // Vertical rects coincide with the raster even at depth 2
    // (verified against the C table): 8X16 d2.
    let (txw, txh) = txb_dims_at_depth(8, 16, 2);
    let cols = 8 / txw;
    for txb in 0..(cols * (16 / txh)) {
        assert_eq!(
            txb_org_inter(8, 16, 2, txb),
            ((txb % cols) * txw, (txb / cols) * txh),
            "8x16 d2 txb{txb}"
        );
    }
}

/// IBC chunk 7: `MdRates::txt_rate` INTER arm — the sentinel routes to
/// `inter_ext_tx` rows with the inter set indexing; the intra arm is
/// untouched (same inputs give the pre-chunk value).
#[test]
fn txt_rate_inter_sentinel_routes_to_inter_rows() {
    let fc = FrameContext::new_default();
    let cfc = cc::CoeffFc::default_for_qindex(60);
    let rates = build_md_rates(&fc, &cfc);
    // 8x8: intra set DTT4_IDTX_1DDCT (7 types), inter set ALL16.
    let tx = cc::TX_8X8;
    let intra_dct = rates.txt_rate(tx, 0, cc::DCT_DCT);
    let inter_dct = rates.txt_rate(tx, INTER_TXT_DIR, cc::DCT_DCT);
    // Both nonzero (multi-type sets), from DIFFERENT tables.
    assert!(intra_dct > 0 && inter_dct > 0);
    let set_inter = cc::ext_tx_set_type(tx, true, false);
    let eset_inter = cc::EXT_TX_SET_INDEX[1][set_inter] as usize;
    let sym = cc::AV1_EXT_TX_IND[set_inter][cc::DCT_DCT];
    assert_eq!(
        inter_dct,
        rates.inter_ext_tx[eset_inter * 4 + cc::TXSIZE_SQR_MAP[tx]][sym]
    );
    // 32x32: intra DCT-only (rate 0); inter DCT_IDTX (2 types, nonzero).
    assert_eq!(rates.txt_rate(cc::TX_32X32, 0, cc::DCT_DCT), 0);
    assert!(rates.txt_rate(cc::TX_32X32, INTER_TXT_DIR, cc::DCT_DCT) > 0);
}

/// SHIPPED-C QUIRK, second half: a tx type OUTSIDE the queried row's ext
/// set costs ZERO, because C's `{intra,inter}_tx_type_fac_bits` are
/// TX-TYPE-indexed and `svt_aom_get_syntax_rate_from_cdf(...,
/// av1_ext_tx_inv[set])` (md_rate_estimation.c:225-243) only ever writes
/// the set's own members — every other entry keeps its zero init.
///
/// Reachable in exactly one place: the IntraBC coeff cost, whose
/// `cost_dir` remap in `cost_coeffs_txb` (mirroring
/// `svt_av1_cost_coeffs_txb`'s `is_inter = is_inter_mode(mode)` WITHOUT
/// `|| use_intrabc`, rd_cost.c:392) reads the INTRA row for a tx type that
/// came from the INTER search set. This port's tables are SYMBOL-indexed,
/// so without the membership guard `AV1_EXT_TX_IND[set][out_of_set_type]`
/// returns its own 0 filler and the query silently prices SYMBOL 0.
///
/// Witness (measured, gb82-sc graph.png 512x512 q63 preset 2): block
/// mi(8,80), a 32x32 IntraBC leaf, luma txb (16,0) 16x16. C prices V_DCT
/// at 0 for a txb cost of 2808; symbol 0 of that row is IDTX and costs
/// 2489 more, taking the port to 5297 and flipping the per-txb TXT winner
/// to DCT_DCT/eob=0 where C codes V_DCT/eob=1.
#[test]
fn txt_rate_out_of_set_type_costs_zero_like_c() {
    let fc = FrameContext::new_default();
    let cfc = cc::CoeffFc::default_for_qindex(255);
    let rates = build_md_rates(&fc, &cfc);
    // The witnessed geometry: 16x16 on the INTRA row (the IntraBC
    // `cost_dir` remap sends intra_dir = DC_PRED = 0). TxType ids 10/11 =
    // V_DCT / H_DCT — both members of the INTER 16x16 set
    // (DTT9_IDTX_1DDCT) and neither a member of the INTRA one
    // (DTT4_IDTX).
    let tx = cc::TX_16X16;
    let intra_set = cc::ext_tx_set_type(tx, false, false);
    for t in [10usize, 11usize] {
        assert_eq!(
            AV1_EXT_TX_USED[intra_set][t], 0,
            "precondition: tx type {t} must be outside the intra 16x16 set"
        );
        assert_eq!(
            rates.txt_rate(tx, 0, t),
            0,
            "out-of-set tx type {t} must cost 0 on the intra row (C never \
             populates that entry), not symbol 0's rate"
        );
    }
    // Anti-vacuity: symbol 0 of that very row IS expensive, so the guard
    // is doing real work rather than agreeing with an all-zero table.
    let eset = cc::EXT_TX_SET_INDEX[0][intra_set] as usize;
    let row = (eset * 4 + cc::TXSIZE_SQR_MAP[tx]) * 13;
    assert!(
        rates.intra_ext_tx[row][0] > 0,
        "symbol 0 of the intra 16x16 DC row must be nonzero"
    );
    // In-set types on the same row still price normally.
    assert!(rates.txt_rate(tx, 0, cc::DCT_DCT) > 0);
}

/// bd10 ind_uv fast metric: [`residual_sad_hbd`] is the 16-bit SAD C sorts
/// the `search_best_independent_uv_mode` candidates by when `mds0_dist_type
/// != VAR` (product_coding_loop.c:7658, `sad_16b_kernel`) — the default,
/// since `mds0_dist_type` is never assigned in the C tree (0 = SAD). Pin it
/// BIT-EXACT to the real `svt_aom_sad_16b_kernel_c` across the chroma sizes
/// the uv search reaches, over randomized 10-bit content. Using variance
/// here (the mainline LUMA mds0 metric) mis-orders the SET on non-flat
/// recon and drops UV_PAETH from the survivors where C keeps it.
#[test]
fn residual_sad_hbd_matches_c_sad_16b_kernel() {
    // Deterministic xorshift so the test needs no rng dependency.
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for &(w, h) in &[
        (4usize, 4usize),
        (8, 8),
        (16, 16),
        (32, 32),
        (4, 8),
        (8, 4),
        (16, 8),
        (8, 16),
    ] {
        for _ in 0..300 {
            let n = w * h;
            let src: Vec<u16> = (0..n).map(|_| (next() % 1024) as u16).collect();
            let pred: Vec<u16> = (0..n).map(|_| (next() % 1024) as u16).collect();
            let port = residual_sad_hbd(&src, w, 0, 0, &pred, w, h);
            let c = svtav1_cref::sad_16b_kernel(&src, w, &pred, w, w, h) as u64;
            assert_eq!(port, c, "sad_16b mismatch at {w}x{h}: port={port} c={c}");
        }
    }
}

/// The bd10 CfL AC luma has TWO producers that must agree: the in-search
/// [`cfl_ac_subsample_hbd`], which overlays the block's *uncommitted*
/// winner recon onto the frame's ROUND_UV pair, and the re-encode
/// post-pass's [`cfl_ac_from_frame_recon_hbd`], which reads the pair
/// straight out of the committed frame recon. They are only allowed to
/// differ before the block is committed; once the frame recon HOLDS the
/// block (exactly the post-pass's situation, since `bd10_reencode_luma`
/// walks the whole frame before chroma starts) they must be identical.
/// This pins that invariant, which is what lets the post-pass reproduce
/// the search's CfL prediction and hence lets `bd10_tree_supported` admit
/// UV_CFL_PRED leaves at all.
#[test]
fn cfl_ac_producers_agree_once_block_is_committed() {
    use svtav1_dsp::intra_pred::CFL_BUF_LINE;
    let stride = 64usize;
    // Deterministic pseudo-random 10-bit frame recon.
    let mut frame = alloc::vec![0u16; stride * stride];
    let mut s: u32 = 0x1234_5678;
    for px in frame.iter_mut() {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *px = ((s >> 13) & 0x3ff) as u16;
    }
    // (block x, y, w, h) — the >=8 fast path, a 4-wide and a 4-high
    // sub-8 chroma-ref pair, and an off-origin 8x8.
    // Legal AV1 leaf geometries only: an N-wide block is N-aligned in x
    // (and N-high in y), so the ROUND_UV pair origin `abs & !7` never
    // splits the block across the pair. The >=8 fast path, then the two
    // sub-8 chroma-ref shapes (4xN at an 8-aligned x, Nx4 at an 8-aligned
    // x with an odd 4-row offset).
    for &(bx, by, w, h) in &[(8, 8, 8, 8), (16, 24, 16, 16), (8, 8, 4, 8), (8, 12, 8, 4)] {
        let cw = w.max(8) / 2;
        let chh = h.max(8) / 2;
        // The block's own recon, as the search carries it (`best_recon10`).
        let mut blk = alloc::vec![0u16; w * h];
        for r in 0..h {
            let src = (by + r) * stride + bx;
            blk[r * w..(r + 1) * w].copy_from_slice(&frame[src..src + w]);
        }
        let mut a = alloc::vec![0i16; CFL_BUF_LINE * chh.max(1)];
        cfl_ac_subsample_hbd(&frame, stride, &blk, bx, by, w, h, &mut a);
        svtav1_dsp::intra_pred::cfl_subtract_average(&mut a, cw, chh);
        let mut b = alloc::vec![0i16; CFL_BUF_LINE * chh.max(1)];
        cfl_ac_from_frame_recon_hbd(&frame, stride, bx, by, w, h, cw, chh, &mut b);
        assert_eq!(a, b, "CfL AC producers disagree for {w}x{h} at ({bx},{by})");
        // Non-degenerate: a flat AC would make the comparison vacuous.
        assert!(
            a[..cw].iter().any(|&v| v != a[0]),
            "AC row is constant for {w}x{h} — test content is degenerate"
        );
    }
}

/// `cfl_idx_to_alpha` round-trips the packed `(u << 4) + v` index and the
/// joint sign exactly as C's `cfl_idx_to_alpha` (intra_prediction.h:134);
/// the re-encode post-pass re-derives both plane alphas from the leaf's
/// stored `cfl_alpha_idx`/`cfl_alpha_signs`, so a mis-unpack there would
/// silently mispredict chroma on every CfL leaf.
#[test]
fn cfl_idx_to_alpha_unpacks_both_planes() {
    // joint_sign 6 decodes to (signU = POS, signV = NEG) via C's
    // CFL_SIGN_U/V ((js+1)/3, (js+1)%3). The magnitude index c maps to
    // |alpha| = c + 1, so c=1 POS is +2 and c=2 NEG is -3 — cross-checked
    // against a real C `md_cfl_rd_pick_alpha` dump, where idx=2/sgn=6
    // evaluated alpha +1 on U (c=0) and -3 on V (c=2).
    assert_eq!(cfl_idx_to_alpha((1 << 4) + 2, 6, 0), 2); // u c=1, POS
    assert_eq!(cfl_idx_to_alpha((1 << 4) + 2, 6, 1), -3); // v c=2, NEG
    assert_eq!(cfl_idx_to_alpha(2, 6, 0), 1); // u c=0, POS
    assert_eq!(cfl_idx_to_alpha(2, 6, 1), -3); // v c=2, NEG
    // CFL_SIGN_ZERO on a plane forces alpha 0 regardless of magnitude.
    let js = plane_sign_to_joint_sign(0, 0, 1); // (ZERO, NEG)
    assert_eq!(cfl_idx_to_alpha((7 << 4) + 7, js, 0), 0);
}

/// Instrumented-capture pins: `M6FNL NICS c0` lines — mds1/2/3
/// counts at CLI qp 20/40/55 (M6 nic level 6, nums 6/6/6, base
/// 24/12/6 q-scaled).
///
/// (These docs + a stray duplicate `#[test]` were left attached to the
/// CfL producers test by the 977136df8 splice; relocated here, where the
/// test they describe lives.)
#[test]
fn nic_counts_match_c() {
    // M6 (nic level 6): nums 6/6/6.
    assert_eq!(nic_counts(20, (6, 6, 6)), (8, 4, 2));
    assert_eq!(nic_counts(40, (6, 6, 6)), (15, 8, 4));
    assert_eq!(nic_counts(55, (6, 6, 6)), (22, 11, 5));
    // M8 (nic level 11 -> scaling level 15 -> nums 0/0/0): the min-1
    // floor (scaling num == 0) pins every stage to 1 at all tracked qps.
    assert_eq!(nic_counts(20, (0, 0, 0)), (1, 1, 1));
    assert_eq!(nic_counts(40, (0, 0, 0)), (1, 1, 1));
    assert_eq!(nic_counts(55, (0, 0, 0)), (1, 1, 1));
}

/// RDCOST identity from the captured g64 q55 MDS3 rows: the DC
/// candidate's full cost decomposition
/// (rate 547+273+176560+112+112+1280+26, dist 10963760).
#[test]
fn rdcost_matches_capture() {
    assert_eq!(rdcost(1527856, 178910, 10963760), 1937245493);
    // H row: rate 181608, dist 10996528 -> 1949490882.
    assert_eq!(rdcost(1527856, 181608, 10996528), 1949490882);
    // MDS0 fast cost, DC @ q55: rate 820, satd 204088 << 4.
    assert_eq!(rdcost(1527856, 820, 204088 << 4), 420419181);
}

/// Mode/uv/fi/tx_size rate pins from the M6FNL MDS0/FLC dumps
/// (default contexts, coeff tables at the respective qindexes).
#[test]
fn md_rates_match_c_captures() {
    let fc = svtav1_entropy::context::FrameContext::new_default();
    let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(220);
    let r = build_md_rates(&fc, &cfc);
    // kf y mode at ctx (0,0): DC 547, SMOOTH 1556 (q55 64x64 flr).
    assert_eq!(r.kf_y[0][0][0], 547);
    assert_eq!(r.kf_y[0][0][9], 1556);
    // V/H flr include the angle0 symbol: 2874 / 2555.
    assert_eq!(r.kf_y[0][0][1] + r.angle[0][3], 2874);
    assert_eq!(r.kf_y[0][0][2] + r.angle[1][3], 2555);
    // uv fcr rows: 64x64 (CFL-disallowed) DC 273, V 1033, H 1009;
    // 32x32 (CFL-allowed) DC 845, SMOOTH 1362.
    assert_eq!(r.uv[0][0][0], 273);
    assert_eq!(r.uv[0][1][1] + r.angle[0][3], 1033);
    assert_eq!(r.uv[0][2][2] + r.angle[1][3], 1009);
    assert_eq!(r.uv[1][0][0], 845);
    assert_eq!(r.uv[1][9][9], 1362);
    // filter-intra at 32x32 (bsize_idx 9): flag-off 281 (DC flr
    // 828 - 547), flag-on + FILTER_DC mode = 1803 (FI flr 2350).
    assert_eq!(r.fi_flag[9][0], 281);
    assert_eq!(r.fi_flag[9][1] + r.fi_mode[0], 1803);
    // skip=0 at ctx 0: 26.
    assert_eq!(r.skip[0][0], 26);
    // tx_size bits: 64x64 ctx0 depth0/1 = 1280/1292; 32x32 ctx0
    // depth0 = 683 (q40 FLC nsk_txsz).
    assert_eq!(r.tx_size[3][0][0], 1280);
    assert_eq!(r.tx_size[3][0][1], 1292);
    assert_eq!(r.tx_size[2][0][0], 683);
}

/// FunnelCfg::for_preset(5) pins vs the instrumented M5DBG CFG
/// enc_mode=5 dump (docs/captures/m0m5_config_dlf.txt): intra_level 2
/// -> mode_end PAETH / ang 2; fi_max 0 (FILTER_DC only); nic 6 with
/// M6's pruning ths; txt 6/6 satd 15 rate 250; chroma_level 4
/// (ind-uv MDS3); SH edge filter.
#[test]
fn m5_cfg_matches_capture() {
    let c = FunnelCfg::for_preset(5);
    assert_eq!(c.mode_end, 12);
    assert_eq!(c.angular_level, 2);
    assert!(c.filter_intra && !c.prune_best_mode);
    assert_eq!(c.nic_num, (6, 6, 6));
    assert_eq!(
        (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
        (1200, 3, 15)
    );
    assert_eq!((c.mds2_rel_dev_th, c.mds3_cand_base_th), (5, 15));
    assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
    assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
    assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
    assert!(c.ind_uv_mds3 && c.edge_filter && !c.dc_only_gate);
    assert_eq!(c.mds2_rank_factor, 1);
    // M6 keeps the original shape (regression pin for the shared tail).
    let m6 = FunnelCfg::for_preset(6);
    assert_eq!(m6.mode_end, 9);
    assert_eq!(m6.angular_level, 4);
    assert_eq!((m6.txt_group_lt16, m6.txt_group_ge16), (5, 4));
    assert_eq!((m6.txt_satd_th, m6.txt_rate_th), (10, 100));
    assert!(!m6.ind_uv_mds3 && !m6.edge_filter);
    assert_eq!(m6.mds2_rank_factor, 1);
}

/// FunnelCfg::for_preset(4) pins vs the instrumented M5DBG CFG
/// enc_mode=4 dump (docs/captures/m0m5_config_dlf.txt line 14):
/// intra_level 1 -> mode_end PAETH / angular_pred_level 1 (ALL 7
/// deltas); SH edge filter OFF (ang 1 not in {2,3}); nic case 5 —
/// scal 6, mds1 1200/rank 0, mds2 20/rank 0/rel-dev 0, mds3 15;
/// txt/txs/rdoq/chroma identical to M5.
#[test]
fn m4_cfg_matches_capture() {
    let c = FunnelCfg::for_preset(4);
    assert_eq!(c.mode_end, 12);
    assert_eq!(c.angular_level, 1);
    assert!(c.filter_intra && !c.prune_best_mode);
    assert_eq!(c.nic_num, (6, 6, 6));
    assert_eq!(
        (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
        (1200, 0, 20)
    );
    assert_eq!((c.mds2_rank_factor, c.mds2_rel_dev_th), (0, 0));
    assert_eq!(c.mds3_cand_base_th, 15);
    assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
    assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
    assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
    assert!(c.ind_uv_mds3 && !c.edge_filter && !c.dc_only_gate);
}

/// M4 candidate enumeration (angular_pred_level 1): every directional
/// mode carries all 7 deltas in counter order -3..+3
/// (mode_decision.c:3259-3271 — the |1|/|2| skip only arms at
/// level >= 2), non-directionals one entry each, FILTER_DC last:
/// 13 modes + 8 x 6 extra deltas = 61 regular + 1 filter-intra.
#[test]
fn m4_candidate_set_shape() {
    let cfg = FunnelCfg::for_preset(4);
    let mut n = 0usize;
    let mut first_dir_deltas: Vec<i8> = Vec::new();
    for mode in 0..=cfg.mode_end {
        let directional = matches!(mode, 1..=8);
        if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
            continue;
        }
        if directional && cfg.angular_level <= 2 {
            for d in -3i8..=3 {
                if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                    continue;
                }
                if mode == 1 {
                    first_dir_deltas.push(d);
                }
                n += 1;
            }
        } else {
            n += 1;
        }
    }
    assert_eq!(n, 61);
    assert_eq!(first_dir_deltas, alloc::vec![-3, -2, -1, 0, 1, 2, 3]);
}

/// The chroma tx type derivation confirmed by the WIN dumps
/// (ttuv 0/1/2/3 for DC/V/H/SMOOTH; DCT-only at >= 32) + the full
/// g_intra_mode_to_tx_type rows the M5 ind-uv modes reach.
#[test]
fn txb_geometry_matches_c_tables() {
    // Pinned against the instrumented tx_org/tx_blocks_per_depth/
    // tx_depth_to_tx_size dump (intra rows; docs/captures/nsq_m2m3
    // provenance): (w, h, depth) -> (txw, txh).
    const CASES: [(usize, usize, u8, usize, usize); 16] = [
        (64, 64, 1, 32, 32),
        (64, 64, 2, 16, 16),
        (32, 32, 2, 8, 8),
        (16, 16, 2, 4, 4),
        (64, 32, 0, 64, 32),
        (64, 32, 1, 32, 32),
        (64, 32, 2, 16, 16),
        (32, 64, 2, 16, 16),
        (64, 16, 1, 32, 16),
        (64, 16, 2, 16, 16),
        (16, 64, 2, 16, 16),
        (32, 8, 1, 16, 8),
        (32, 8, 2, 8, 8),
        (16, 8, 2, 4, 4),
        (4, 16, 1, 4, 8),
        (4, 16, 2, 4, 4),
    ];
    for &(w, h, d, tw, th) in &CASES {
        assert_eq!(txb_dims_at_depth(w, h, d), (tw, th), "{w}x{h} d{d}");
    }
}

#[test]
fn m2_m3_funnel_cfg_matches_capture() {
    // M5DBG CFG enc_mode=2/3 rows (docs/captures/m0m5_config_dlf.txt
    // lines 12-13): txt satd 20, groups 6/6, rate 250; txs 2/2 with
    // d1/d2 offsets 0; M2 nic case 3 (scal 12, mds1 1200/rank 0,
    // mds2 30/rank 0/rel 0, mds3 25); M3 nic case 5 == M4.
    for p in [2u8, 3] {
        let c = FunnelCfg::for_preset(p);
        assert_eq!(c.txt_satd_th, 20, "p{p}");
        assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
        assert_eq!(c.txt_rate_th, 250);
        assert_eq!((c.txs_max_sq, c.txs_max_nsq), (2, 2));
        assert_eq!((c.txt_d1_off, c.txt_d2_off), (0, 0));
        assert_eq!(c.mode_end, 12);
        assert_eq!(c.angular_level, 1);
        assert!(c.ind_uv_mds3);
        assert_eq!(c.mds1_rank_factor, 0);
        assert_eq!(c.mds2_rank_factor, 0);
        assert_eq!(c.mds2_rel_dev_th, 0);
    }
    let m2 = FunnelCfg::for_preset(2);
    assert_eq!(m2.nic_num, (12, 12, 12));
    assert_eq!(m2.mds2_cand_base_th, 30);
    assert_eq!(m2.mds3_cand_base_th, 25);
    let m3 = FunnelCfg::for_preset(3);
    assert_eq!(m3.nic_num, (6, 6, 6));
    assert_eq!(m3.mds2_cand_base_th, 20);
    assert_eq!(m3.mds3_cand_base_th, 15);
    // M4 (txs level 3) unchanged by the M2/M3 additions.
    let m4 = FunnelCfg::for_preset(4);
    assert_eq!((m4.txs_max_sq, m4.txs_max_nsq), (1, 0));
    assert_eq!((m4.txt_d1_off, m4.txt_d2_off), (3, 3));
    assert_eq!(m4.txt_satd_th, 15);
}

#[test]
fn uv_tx_type_matches_c() {
    // SMOOTH_V -> ADST_DCT, SMOOTH_H -> DCT_ADST, PAETH -> ADST_ADST,
    // D45 -> DCT_DCT, D135 -> ADST_ADST (mode_decision.c:2991 table).
    assert_eq!(uv_tx_type(10, 16, 16), 1);
    assert_eq!(uv_tx_type(11, 16, 16), 2);
    assert_eq!(uv_tx_type(12, 16, 16), 3);
    assert_eq!(uv_tx_type(3, 16, 16), 0);
    assert_eq!(uv_tx_type(4, 16, 16), 3);
}

#[test]
fn uv_tx_type_m6_subset_matches_c() {
    assert_eq!(uv_tx_type(0, 16, 16), 0);
    assert_eq!(uv_tx_type(1, 16, 16), 1);
    assert_eq!(uv_tx_type(2, 16, 16), 2);
    assert_eq!(uv_tx_type(9, 16, 16), 3);
    assert_eq!(uv_tx_type(2, 32, 32), 0); // 64x64 luma -> DCT only
}

/// A minimal mainline (non-fork) [`FunnelFrame`] for the TX-unit tests:
/// every fork knob off, so `tx_unit`'s spatial arm is the plain SSE << 4
/// that C's `svt_spatial_full_distortion_kernel_facade` produces at
/// `tx_bias == 0 && ac_bias == 0`.
fn test_frame(base_qindex: u8, frame_w_px: usize, frame_h_px: usize) -> FunnelFrame {
    FunnelFrame {
        sb_mi_size: 16,
        lambda: 100_000,
        cli_qp: 32,
        rdoq_level: 0,
        base_qindex,
        bit_depth: 8,
        qindex_u: base_qindex,
        qindex_v: base_qindex,
        ac_bias_eff: 0.0,
        sharpness: 0,
        sharp_tx_active: false,
        noise_norm_strength: 0,
        qm_levels: [15, 15, 15],
        mds0_ssd: false,
        tune_ssim: false,
        tune_ssim_threshold: 1.03,
        tx_bias: 0,
        dv_tables: None,
        frame_h_px,
        frame_w_px,
        cfg: FunnelCfg::for_preset(6),
    }
}

/// Task #95 (b)+(c) — the CROPPED-TX RD distortion, differentially pinned
/// to the REAL exported C kernel.
///
/// C computes a boundary TX block's SPATIAL distortion only over the part
/// inside the ALIGNED frame: `cropped_tx_width`/`cropped_tx_height`
/// (product_coding_loop.c:4664-4665, re-derived identically in
/// `perform_dct_dct_tx` at :5752-5754) and `cropped_tx_width_uv`/`_height_uv`
/// (full_loop.c:2228-2232), which it then passes straight into
/// `svt_spatial_full_distortion_kernel_facade` (:4818/:4846, :5781/:5809,
/// full_loop.c:2376/:2405) — the FULL tx stride, the CROPPED area.
///
/// That facade IS exported and is what `cref::spatial_facade` drives, so
/// this is the strongest evidence tier: the port's `tx_unit` distortion is
/// compared against the real C kernel invoked with the C-derived cropped
/// dims, on a straddling geometry taken from a real partial-superblock
/// frame (aligned 96x80: an SB(1,0) 16x32 txb at (32,64) hangs 16 rows
/// past the bottom; an SB(0,1) 32x16 txb at (64,32) hangs 8 columns past
/// the right — cf. `edge_flags_match_c_rule_on_the_96x80_milestone`).
///
/// ANTI-VACUITY is asserted in the test itself: the same C kernel over the
/// FULL tx dims must give a DIFFERENT number, which is exactly what the
/// port produced before the crop was wired.
/// The crop reaches THREE consumers in `tx_unit`'s spatial arm: the plain
/// SSE, `tx_bias::facade_bias`, and `ac_bias::psy_full_dist`. The
/// differential above runs at `tx_bias == 0 && ac_bias_eff == 0.0` —
/// exactly the configuration where the latter two are no-ops — so it
/// covered one of the three. Flagged by the adversarial pass.
///
/// This drives the psy consumer against the REAL exported C kernel
/// (`svt_cref::psy_distortion` -> `get_svt_psy_full_dist`'s inner
/// distortion, which C calls with `cropped_tx_width/height` and the FULL
/// recon stride at product_coding_loop.c:4834/:4862 and :5803/:5831).
///
/// ANTI-VACUITY: asserts the cropped and FULL-dims results differ, so a
/// regression that passed full dims to the psy call would fail here.
#[test]
fn cropped_psy_distortion_matches_c_on_a_straddling_txb() {
    use crate::frame_geom::{FrameDims, cropped_tx_dims};
    let dims = FrameDims::new(96, 80);
    let stride = 128usize;
    let mut src = alloc::vec![0u8; stride * stride];
    let mut s: u32 = 0x9e37_79b9;
    for px in src.iter_mut() {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *px = (s >> 20) as u8;
    }
    // Bottom straddle: a 16x32 txb at (32,64) keeps 80-64 = 16 of 32 rows.
    let (tx_x, tx_y, w, h) = (32usize, 64usize, 16usize, 32usize);
    let (crop_w, crop_h) = cropped_tx_dims(&dims, tx_x, tx_y, w, h);
    assert_eq!((crop_w, crop_h), (16, 16), "the txb must actually straddle");

    let src_off = tx_y * stride + tx_x;
    let mut recon = alloc::vec![0u8; w * h];
    for (i, r) in recon.iter_mut().enumerate() {
        *r = src[src_off + (i / w) * stride + (i % w)].wrapping_add(23);
    }

    // C: cropped area, FULL recon stride.
    let c_cropped = svtav1_cref::psy_distortion(
        &src[src_off..],
        stride as u32,
        &recon,
        w as u32,
        crop_w as u32,
        crop_h as u32,
    );
    let c_full = svtav1_cref::psy_distortion(
        &src[src_off..],
        stride as u32,
        &recon,
        w as u32,
        w as u32,
        h as u32,
    );
    assert_ne!(
        c_cropped, c_full,
        "ANTI-VACUITY: cropped and full psy distortion must differ on a \
         straddling txb, else this test cannot detect the wrong dims"
    );

    let port = svtav1_dsp::ac_bias::psy_full_dist(
        &src, src_off, stride, &recon, 0, w, crop_w, crop_h, 1.0,
    );
    // psy_full_dist folds C's `llrint(psy * ac_bias)`; at ac_bias 1.0 that
    // is the kernel value itself.
    assert_eq!(
        port, c_cropped,
        "port psy_full_dist at the cropped dims must equal the real C kernel"
    );
}

#[test]
fn cropped_tx_distortion_matches_c_spatial_facade() {
    use crate::frame_geom::{FrameDims, cropped_tx_dims, cropped_tx_dims_uv};
    // Aligned 96x80 partial-SB frame (the #95 milestone geometry).
    let dims = FrameDims::new(96, 80);
    assert_eq!((dims.aligned_w, dims.aligned_h), (96, 80));
    let frame = test_frame(120, dims.aligned_w, dims.aligned_h);
    let fc = FrameContext::new_default();
    let cfc = cc::CoeffFc::default_for_qindex(frame.base_qindex);
    let rates = build_md_rates(&fc, &cfc);
    let qt = crate::quant::build_quant_table(frame.base_qindex);

    // Source plane at the SB extent (128x128) — a partial SB's straddling
    // rows/cols live in the edge-replicated pad, exactly as the encoder
    // lays it out (`frame_geom::pad_input_plane`).
    let stride = 128usize;
    let mut src = alloc::vec![0u8; stride * stride];
    let mut s: u32 = 0x2545_f491;
    for px in src.iter_mut() {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *px = (s >> 20) as u8;
    }

    // (tx origin x, y, tx w, h, plane_type) — a bottom straddle, a right
    // straddle, and a fully-interior control.
    let cases: [(usize, usize, usize, usize, usize); 3] = [
        (32, 64, 16, 32, 0), // bottom: 80 - 64 = 16 of 32 rows in frame
        (64, 32, 32, 16, 0), // right:  96 - 64 = 32 -> full; use 40 below
        (16, 16, 16, 16, 0), // interior control (crop == full)
    ];
    let mut saw_crop = false;
    for &(tx_x, tx_y, w, h, plane_type) in &cases {
        let crop = cropped_tx_dims(&dims, tx_x, tx_y, w, h);
        let src_off = tx_y * stride + tx_x;
        // A prediction that is NOT the source, so the SSE is non-trivial
        // and the cropped and uncropped sums genuinely differ.
        let mut pred = alloc::vec![0u8; w * h];
        for (i, p) in pred.iter_mut().enumerate() {
            *p = src[src_off + (i / w) * stride + (i % w)].wrapping_add(17);
        }
        let out = tx_unit(
            &src,
            stride,
            src_off,
            &pred,
            w,
            0,
            w,
            h,
            cc::DCT_DCT,
            plane_type,
            0,
            0,
            0,
            &qt,
            &frame,
            &rates,
            false, /* do_rdoq */
            true,  /* spatial_dist */
            crop,
            true,
            RateMode::Exact,
        );
        // The REAL C facade, cropped area + full recon stride — the exact
        // call C makes at product_coding_loop.c:4839-4853.
        let c_cropped = svtav1_cref::spatial_facade(
            &src[src_off..],
            stride as u32,
            &out.recon,
            w as u32,
            crop.0 as u32,
            crop.1 as u32,
            0,     // DC_PRED
            0,     // UV_DC_PRED
            false, // is_chroma
            false, // is_interintra
            0,     // comp_type
            0,     // temporal_layer_index (still = layer 0)
            0.0,   // ac_bias off
            0,     // tx_bias off (mainline)
        );
        assert_eq!(
            out.dist,
            c_cropped << 4,
            "cropped spatial dist mismatch at tx ({tx_x},{tx_y}) {w}x{h}"
        );
        // ANTI-VACUITY: on a straddling txb the UNcropped kernel — what
        // the port computed before this wiring — must differ.
        let c_full = svtav1_cref::spatial_facade(
            &src[src_off..],
            stride as u32,
            &out.recon,
            w as u32,
            w as u32,
            h as u32,
            0,
            0,
            false,
            false,
            0,
            0,
            0.0,
            0,
        );
        if crop != (w, h) {
            saw_crop = true;
            assert_ne!(
                c_full, c_cropped,
                "straddling txb ({tx_x},{tx_y}) {w}x{h} must be crop-sensitive"
            );
            assert_ne!(
                out.dist,
                c_full << 4,
                "tx_unit must NOT price the out-of-frame part of ({tx_x},{tx_y}) {w}x{h}"
            );
        } else {
            assert_eq!(c_full, c_cropped, "interior control must be crop-inert");
        }
    }
    assert!(saw_crop, "no straddling case exercised — test is vacuous");

    // ---- CHROMA (full_loop.c:2228-2232) ----
    // The chroma crop is taken in the CHROMA domain from the ROUND_UV
    // origin: `(aligned_w >> 1) - (ROUND_UV(luma_x) >> 1)`. Aligned 96x80
    // -> chroma 48x40; a 16x16 chroma txb at (32,32) hangs 8 rows past the
    // chroma bottom.
    let (ccx, ccy, cw, chh) = (32usize, 32usize, 16usize, 16usize);
    let uv_crop = cropped_tx_dims_uv(&dims, ccx, ccy, cw, chh);
    assert_eq!(uv_crop, (16, 8));
    let c_off = ccy * stride + ccx;
    let mut uv_pred = alloc::vec![0u8; cw * chh];
    for (i, p) in uv_pred.iter_mut().enumerate() {
        *p = src[c_off + (i / cw) * stride + (i % cw)].wrapping_sub(23);
    }
    let uv_out = tx_unit(
        &src,
        stride,
        c_off,
        &uv_pred,
        cw,
        0,
        cw,
        chh,
        cc::DCT_DCT,
        1,
        0,
        0,
        0,
        &qt,
        &frame,
        &rates,
        false,
        true,
        uv_crop,
        true,
        RateMode::Exact,
    );
    let uv_c_cropped = svtav1_cref::spatial_facade(
        &src[c_off..],
        stride as u32,
        &uv_out.recon,
        cw as u32,
        uv_crop.0 as u32,
        uv_crop.1 as u32,
        0,
        0,
        true, // is_chroma
        false,
        0,
        0,
        0.0,
        0,
    );
    assert_eq!(
        uv_out.dist,
        uv_c_cropped << 4,
        "chroma cropped dist mismatch"
    );
    let uv_c_full = svtav1_cref::spatial_facade(
        &src[c_off..],
        stride as u32,
        &uv_out.recon,
        cw as u32,
        cw as u32,
        chh as u32,
        0,
        0,
        true,
        false,
        0,
        0,
        0.0,
        0,
    );
    assert_ne!(
        uv_c_full, uv_c_cropped,
        "chroma case must be crop-sensitive"
    );
    assert_ne!(
        uv_out.dist,
        uv_c_full << 4,
        "chroma must not price out-of-frame rows"
    );
}

/// The cropped-TX bound itself, as hand-derived from the C expressions
/// (product_coding_loop.c:4664-4665 luma, full_loop.c:2228-2232 chroma).
/// Complements the differential above, which pins the CONSUMPTION.
#[test]
fn cropped_tx_dims_match_the_c_expressions() {
    use crate::frame_geom::{FrameDims, cropped_tx_dims, cropped_tx_dims_uv};
    let d = FrameDims::new(96, 80);
    // Interior: no crop.
    assert_eq!(cropped_tx_dims(&d, 0, 0, 64, 64), (64, 64));
    assert_eq!(cropped_tx_dims(&d, 32, 32, 16, 16), (16, 16));
    // Bottom straddle: MIN(32, 80 - 64) = 16 rows.
    assert_eq!(cropped_tx_dims(&d, 32, 64, 16, 32), (16, 16));
    // Right straddle: MIN(32, 96 - 80) = 16 cols.
    assert_eq!(cropped_tx_dims(&d, 80, 0, 32, 32), (16, 32));
    // Both.
    assert_eq!(cropped_tx_dims(&d, 80, 64, 32, 32), (16, 16));
    // Chroma: bound is (aligned >> 1) - chroma_origin, chroma dims 48x40.
    assert_eq!(cropped_tx_dims_uv(&d, 0, 0, 32, 32), (32, 32));
    assert_eq!(cropped_tx_dims_uv(&d, 32, 32, 16, 16), (16, 8));
    assert_eq!(cropped_tx_dims_uv(&d, 40, 32, 8, 8), (8, 8));
    assert_eq!(cropped_tx_dims_uv(&d, 32, 24, 16, 16), (16, 16));
    // 64-aligned frame: BOTH crops are the identity at every geometry —
    // the byte-neutrality guarantee for every full-SB gate cell.
    let full = FrameDims::new(128, 128);
    for &(x, y, w, h) in &[
        (0usize, 0usize, 64usize, 64usize),
        (64, 64, 64, 64),
        (96, 112, 32, 16),
    ] {
        assert_eq!(cropped_tx_dims(&full, x, y, w, h), (w, h));
    }
    for &(x, y, w, h) in &[(0usize, 0usize, 32usize, 32usize), (32, 48, 32, 16)] {
        assert_eq!(cropped_tx_dims_uv(&full, x, y, w, h), (w, h));
    }
}

/// Issue #16 — SHIPPED-C QUIRK, CDF-UPDATE half. C's encode pass evolves
/// the MD-side per-SB context (`ec_ctx_array[sb]`) through
/// `svt_av1_cost_coeffs_txb` at `allow_update_cdf = 1`, whose
/// `is_inter = is_inter_mode(mode)` (rd_cost.c) ignores `use_intrabc`. An
/// IntraBC luma txb therefore adapts `intra_ext_tx_cdf[..][DC_PRED]` with
/// the INTRA set's symbol there, while the bitstream writer
/// (`av1_write_tx_type`, `use_intrabc || is_inter_mode`) codes and adapts
/// the inter row. The chain simulation must reproduce the MD-side arm or
/// the per-SB rate tables it rebuilds price DCT_DCT on the DC row (and, via
/// the `cost_dir` remap, every IntraBC candidate) differently from C —
/// measured on `terminal` 188x256 p2 q55 mi=(50,42) as 3 of 57 MDS1 costs
/// cheaper by exactly 103 rate units (0.20 bits) with the same `ydist`.
#[test]
fn md_side_ibc_tx_type_update_adapts_the_intra_dc_row_like_c() {
    use svtav1_entropy::writer::AomWriter;
    let base = cc::CoeffFc::default_for_qindex(60);
    let tx = cc::TX_8X8;
    let sqr = cc::TXSIZE_SQR_MAP[tx];
    let intra_set = cc::ext_tx_set_type(tx, false, false);
    let intra_eset = cc::EXT_TX_SET_INDEX[0][intra_set] as usize;
    let dc_row = (intra_eset * 4 + sqr) * 13; // + DC_PRED = 0
    let inter_set = cc::ext_tx_set_type(tx, true, false);
    let inter_eset = cc::EXT_TX_SET_INDEX[1][inter_set] as usize;
    let inter_row = inter_eset * 4 + sqr;
    // One nonzero DC coefficient: eob = 1, so the tx type is coded.
    let mut coeffs = vec![0i32; 64];
    coeffs[0] = 3;
    let code_ibc = |md_side: bool| -> alloc::boxed::Box<cc::CoeffFc> {
        let mut fc = base.clone();
        fc.md_side_ibc_txt_update = md_side;
        let mut w = AomWriter::new(1024);
        cc::write_coeffs_txb_1d(
            &mut fc,
            &mut w,
            tx,
            cc::DCT_DCT,
            0,
            0,
            0,
            &coeffs,
            1,
            0,
            60,
            false,
            true, // is_inter: an IntraBC block
        );
        fc
    };
    let writer_side = code_ibc(false);
    let md_side = code_ibc(true);
    // Bitstream semantics: the inter row moved, the intra DC row did not.
    assert_ne!(
        writer_side.inter_ext_tx_cdf[inter_row], base.inter_ext_tx_cdf[inter_row],
        "writer: inter row must adapt"
    );
    assert_eq!(
        writer_side.intra_ext_tx_cdf[dc_row], base.intra_ext_tx_cdf[dc_row],
        "writer: intra DC row must not move"
    );
    // C's MD side: the intra DC row moved, the inter row did not.
    assert_ne!(
        md_side.intra_ext_tx_cdf[dc_row], base.intra_ext_tx_cdf[dc_row],
        "MD side: intra DC row must adapt (rd_cost.c:143 at is_inter = false)"
    );
    assert_eq!(
        md_side.inter_ext_tx_cdf[inter_row], base.inter_ext_tx_cdf[inter_row],
        "MD side: inter row must not move"
    );
    // And it is exactly the update an INTRA DC block coding DCT_DCT makes
    // (same row, same intra-set symbol, same nsymbs).
    let mut intra_ref = base.clone();
    let mut w = AomWriter::new(1024);
    cc::write_coeffs_txb_1d(
        &mut intra_ref,
        &mut w,
        tx,
        cc::DCT_DCT,
        0,
        0,
        0,
        &coeffs,
        1,
        0, // intra_dir = DC_PRED
        60,
        false,
        false,
    );
    assert_eq!(
        md_side.intra_ext_tx_cdf[dc_row], intra_ref.intra_ext_tx_cdf[dc_row],
        "MD side must equal an intra DC DCT_DCT update"
    );
    // 32x32: the intra set is DCT-only, so C's MD side updates NOTHING —
    // while the writer adapts the inter 2-type (DCT_IDTX) row.
    let mut coeffs32 = vec![0i32; 32 * 32];
    coeffs32[0] = 3;
    let mut md32 = base.clone();
    md32.md_side_ibc_txt_update = true;
    let mut w = AomWriter::new(4096);
    cc::write_coeffs_txb_1d(
        &mut md32,
        &mut w,
        cc::TX_32X32,
        cc::DCT_DCT,
        0,
        0,
        0,
        &coeffs32,
        1,
        0,
        60,
        false,
        true,
    );
    assert_eq!(
        md32.intra_ext_tx_cdf, base.intra_ext_tx_cdf,
        "32x32 MD side: no intra update"
    );
    assert_eq!(
        md32.inter_ext_tx_cdf, base.inter_ext_tx_cdf,
        "32x32 MD side: no inter update"
    );
}
