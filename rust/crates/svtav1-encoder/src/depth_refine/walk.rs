use super::*;

// `ctx->skip_sub_depth_ctrls` (enc_mode_config.c:6787): cond1 cancels
// sub-depth testing for blocks <= `max_size` whose winner has flat
// quadrant distortions and few coefficients. The LEVEL forks on the
// `svt_aom_sig_deriv_enc_dec_*` arm — allintra `enc_mode <= ENC_M7 -> 1
// else 2` (:8156), video `enc_mode <= ENC_M1 -> 1 else 2` (:7923) — and
// the levels differ only in `coeff_perc` (15 vs 25), which is why the
// walk reads the arm-stamped `FunnelCfg::skip_sub_depth` rather than
// baking level 1.
pub(crate) struct DepthWalk<'a, 'b> {
    /// One reusable [`NodeSnap`] per node SIZE — 8, 16, 32, 64, 128 — indexed
    /// by `size.trailing_zeros()`.
    ///
    /// The walk takes at most one snapshot per node and the recursion visits
    /// one node per size at a time, so a slot per size is exactly enough. It
    /// exists because `take_snap` used to build a fresh `NodeSnap` every time:
    /// `EntropyCtx` alone is SIXTEEN frame-width vectors, and that was 67,872
    /// allocating calls on the canonical alloc cell — the largest site left
    /// after the mode-decision buffers were pooled. Reusing the slot lets the
    /// fill be `clone_from` / `resize` + copy, which allocates nothing after
    /// the first node of each size.
    pub snaps: alloc::vec::Vec<NodeSnap>,
    pub fx: &'a mut FunnelCtx<'b>,
    /// Full luma source plane (absolute coordinates).
    pub y_src: &'a [u8],
    pub y_src_stride: usize,
    /// Full luma decision recon plane.
    pub y_recon: &'a mut [u8],
    pub y_stride: usize,
    pub lambda: u64,
    pub part_rates: &'a PartRates,
    pub nsq: &'a NsqCfg,
    /// `ctx->disallow_4x4` — the skip_sub quadrant-arm's 8x8 clause
    /// (product_coding_loop.c:10156-10158).
    pub disallow_4x4: bool,
    /// ALIGNED frame dims (the `av1_cm->mi_cols`/`mi_rows` grid, in pixels).
    /// Every `has_rows`/`has_cols` predicate in `set_blocks_to_test`
    /// (enc_dec_process.c:1397-1398), `test_depth` (:10896-10897),
    /// `test_split_partition` (:10805) and `svt_aom_partition_rate_cost`
    /// (rd_cost.c:1831-1832) is against this grid. On a 64-aligned frame every
    /// node has both flags true, so every branch keyed on them is dead.
    pub aligned_w: usize,
    pub aligned_h: usize,
    /// `ctx->nsq_geom_ctrls.enabled` (`svt_aom_get_nsq_geom_level_allintra`,
    /// enc_mode_config.c: allintra enc_mode <= M6 -> level 1/2/3 -> enabled).
    /// Gates the ONE-shape injection at a single-edge node: with geometry off,
    /// `set_blocks_to_test` yields `tot_shapes = 0` and the node force-splits
    /// (enc_dec_process.c:1405-1410).
    pub nsq_geom_enabled: bool,
}

/// C `set_blocks_to_test`'s edge predicate (enc_dec_process.c:1397-1398) — the
/// same `hbs`-against-the-aligned-grid test `test_depth`,
/// `test_split_partition` and `svt_aom_partition_rate_cost` each re-derive.
#[inline]
pub(super) fn edge_flags(
    abs_x: usize,
    abs_y: usize,
    size: usize,
    aw: usize,
    ah: usize,
) -> (bool, bool) {
    let half = size / 2;
    (abs_y + half < ah, abs_x + half < aw)
}

/// The free form of [`DepthWalk::shapes_at`] — C `set_blocks_to_test`
/// (enc_dec_process.c:1394-1438). Split out so the edge rules are unit-testable
/// without a live funnel context.
pub(super) fn shapes_at_edge(
    size: usize,
    nsq: &NsqCfg,
    nsq_geom_enabled: bool,
    has_rows: bool,
    has_cols: bool,
) -> &'static [PartitionType] {
    const NONE_AT_ALL: [PartitionType; 0] = [];
    const H_ONLY: [PartitionType; 1] = [PartitionType::Horz];
    const V_ONLY: [PartitionType; 1] = [PartitionType::Vert];
    if has_rows && has_cols {
        return shapes_for_size(size, nsq);
    }
    if (!has_rows && !has_cols) || !nsq_geom_enabled {
        return &NONE_AT_ALL;
    }
    if !has_rows { &H_ONLY } else { &V_ONLY }
}

/// The free form of [`DepthWalk::shape_block_cnt`] — C `test_depth`'s
/// `shape_block_cnt--` (product_coding_loop.c:10899-10904).
///
/// REACHABILITY, MEASURED 2026-08-04 (adversarial re-verification): the
/// `!has_rows || !has_cols` arm is LIVE — deleting it fails partial-SB cells.
/// The two H4/V4 quarter clauses are **inert on every cell measured so far**:
/// with both terms deleted, `tools/partial_sb_gate.sh` still reports 141/141
/// and a further 13 probe geometries chosen to target them (aligned 40/48 on
/// one axis x {96,120,128} on the other, gradient+screen, presets 0-3, where a
/// 64x64 node has both edge flags true yet its 4th quarter starts at/after the
/// aligned extent) are byte-identical to C with or without them. So the clause
/// is a faithful transcription of a C line whose effect no measured cell
/// observes — kept and documented per the "DEAD-LOOKING C STAYS TRANSLATED"
/// rule in rust/CLAUDE.md, NOT because it was seen to fire. Its behaviour is
/// pinned by `partial_sb_edge_tests::shape_block_cnt_drops_out_of_frame_subblocks`
/// alone. If you need it live, note that H4/V4 must first WIN the d1 compare at
/// such a node, which is what the probes did not achieve.
#[allow(clippy::too_many_arguments)]
pub(super) fn shape_block_cnt_edge(
    size: usize,
    shape: PartitionType,
    n: usize,
    abs_x: usize,
    abs_y: usize,
    aligned_w: usize,
    aligned_h: usize,
    has_rows: bool,
    has_cols: bool,
) -> usize {
    let quarter = size / 4;
    if !has_rows
        || !has_cols
        || (shape == PartitionType::Horz4 && abs_y + 3 * quarter >= aligned_h)
        || (shape == PartitionType::Vert4 && abs_x + 3 * quarter >= aligned_w)
    {
        n - 1
    } else {
        n
    }
}

pub(super) struct NodeRes {
    /// C `pc_tree->rdc.rd_cost` (partition rate + block/subtree cost).
    pub(super) rd: u64,
    /// The node's partition tree. It is the ONLY copy of the node's block
    /// decisions: a parallel `decisions: Vec<BlockDecision>` used to be
    /// carried alongside it, deep-cloned leaf by leaf, and the only thing
    /// anyone ever read out of it was its `len()` — which
    /// `PartitionTree::count_leaves` answers without allocating. A
    /// `BlockDecision` owns up to nine `Vec`s, so the duplicate was a full
    /// second allocate+memcpy+free of every coded block in the frame.
    pub(super) tree: PartitionTree,
}

pub(super) enum SplitOut {
    /// Early exit — parent wins without full child evaluation.
    Invalid,
    /// All quadrants evaluated; parent won the final compare.
    ParentKept,
    Chosen(Box<NodeRes>),
}

/// Snapshot of the node-rect decision state — C's
/// `svt_aom_copy_neighbour_arrays` [0] <-> [1] save/restore around NSQ
/// shape evaluation, expressed on our full-plane model: the whole
/// EntropyCtx (cheap: per-frame line buffers) + the node's recon rects.
#[derive(Default)]
pub(crate) struct NodeSnap {
    pub(super) ectx: crate::pipeline::EntropyCtx,
    pub(super) y: Vec<u8>,
    pub(super) u: Vec<u8>,
    pub(super) v: Vec<u8>,
}

/// The SQ (PART_N) evaluation of the current node + the derived gate
/// inputs (C pc_tree->block_data[PART_N][0] and ctx side-products).
pub(super) struct SqInfo {
    pub(super) ev: LeafEval,
    /// `ctx->rec_dist_per_quadrant` (calc_scr_to_recon_dist_per_quadrant
    /// on the winner, product_coding_loop.c:10153-10160) when armed.
    pub(super) quad: Option<[u64; 4]>,
    /// `ctx->min_nz_h / min_nz_v` (non_normative_txs :9641) when psq
    /// armed and the winner kept coefficients.
    pub(super) min_nz: Option<(u16, u16)>,
}

/// SVTAV1_NSQDBG=1: mirror the instrumented C NSQDBG line format
/// (docs/captures/nsq_m2m3/) on stderr for direct MD-level diffing.
#[cfg(feature = "std")]
pub(super) fn nsqdbg_on() -> bool {
    crate::dbgenv::nsqdbg()
}

/// SVTAV1_DBG_MI="mi_row,mi_col": restrict NSQDBG output to the one 64px SB
/// containing that mi (e.g. `64,112`). Unset = whole frame. Frame-wide dumps
/// are ~45 MB / 35k lines on a 512x512 photo; one SB is ~50 lines — always
/// set this when drilling a known divergence (drill_cell.sh does).
#[cfg(feature = "std")]
pub(super) fn nsqdbg_sb() -> Option<(usize, usize)> {
    static SB: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
    *SB.get_or_init(|| {
        let v = crate::dbgenv::raw_var("SVTAV1_DBG_MI").ok()?;
        let (r, c) = v.split_once(',')?;
        Some((r.trim().parse().ok()?, c.trim().parse().ok()?))
    })
}

/// Dump gate for a record about the block at pixel (abs_x, abs_y).
#[cfg(feature = "std")]
pub(crate) fn nsqdbg_here(abs_x: usize, abs_y: usize) -> bool {
    nsqdbg_on()
        && match nsqdbg_sb() {
            None => true,
            Some((r, c)) => (abs_y >> 6, abs_x >> 6) == (r >> 4, c >> 4),
        }
}

/// C BLOCK_SIZES enum value of a square block (dump parity).
#[cfg(feature = "std")]
pub(super) fn c_bsize_sq(size: usize) -> u32 {
    match size {
        4 => 0,
        8 => 3,
        16 => 6,
        32 => 9,
        _ => 12,
    }
}

/// C `Part` enum value of a funnel shape (dump parity).
#[cfg(feature = "std")]
pub(super) fn c_part(p: PartitionType) -> u32 {
    match p {
        PartitionType::None => 0,
        PartitionType::Horz => 1,
        PartitionType::Vert => 2,
        PartitionType::Horz4 => 3,
        PartitionType::Vert4 => 4,
        PartitionType::HorzA => 5,
        PartitionType::HorzB => 6,
        PartitionType::VertA => 7,
        PartitionType::VertB => 8,
        _ => 255,
    }
}

impl DepthWalk<'_, '_> {
    /// C `CONSERVATIVE_OFFSET_0` / `AGGRESSIVE_OFFSET_1` (definitions.h:
    /// 255/258) — sq_weight adjustments in update_skip_nsq_shapes.
    const CONSERVATIVE_OFFSET_0: u64 = 5;

    /// `ctx->skip_sub_depth_ctrls` for THIS arm — the funnel cfg field
    /// `encdec_arm::apply` stamps per picture (allintra ladder baked by
    /// `FunnelCfg::for_preset`).
    pub(super) fn skip_sub(&self) -> SkipSubDepthCtrls {
        self.fx.frame.cfg.skip_sub_depth
    }

    /// `ctx->depth_early_exit_ctrls` + `ctx->parent_cost_bias` for THIS
    /// arm — the same per-picture `encdec_arm::apply` stamp as
    /// `skip_sub`. C reads the thresholds in `test_split_partition`'s
    /// per-quadrant early exit (product_coding_loop.c:10808-10818) with
    /// a 0 ctrl meaning 1000, and the bias again in its final compare
    /// (:10842). The video arm runs `early_exit_th` 900 above M6 where
    /// the allintra level 1 reads 1000 — the `fourpeople 128x128 q55 p7`
    /// key-frame fork documented on `FunnelCfg::depth_early_exit`.
    pub(super) fn early_exit(&self) -> (u128, u128, u128) {
        let cfg = &self.fx.frame.cfg;
        let ee = cfg.depth_early_exit;
        let th0 = if ee.split_cost_th == 0 {
            1000
        } else {
            ee.split_cost_th
        };
        let thn = if ee.early_exit_th == 0 {
            1000
        } else {
            ee.early_exit_th
        };
        (
            u128::from(th0),
            u128::from(thn),
            u128::from(cfg.parent_cost_bias),
        )
    }

    /// C `calc_scr_to_recon_dist_per_quadrant` (product_coding_loop.c:
    /// 8290): per-quadrant SSE vs the source — luma always, both chroma
    /// planes when quadrant_size > 4 (chroma dims quartered).
    ///
    /// LUMA reads the TX_DEPTH-0 recon, NOT the winning depth's: C's
    /// `cand_bf->recon` is the shared ctx temp buffer; deeper tx depths
    /// reconstruct into the aux tx-depth buffers and `update_tx_cand_bf`
    /// copies pred/coeffs/eob back but never the recon, so at gate time the
    /// shared buffer still holds the depth-0 recon. Proven on 1147124 q20 p4
    /// SB(4,6) (76,96): C's fill luma quads sum to its OWN depth-0 dist
    /// (971<<4 == 15536) while the winning depth-1 recon measures 744<<4.
    /// Chroma has no tx-depth split — the winner chroma recon is correct
    /// (and was already byte-matching C).
    pub(super) fn quad_rec_dists(&self, ev: &LeafEval) -> [u64; 4] {
        let sq = ev.w;
        let quad = sq / 2;
        let mut dists = [0u64; 4];
        // bd10 (task #94, root #2): C `calc_scr_to_recon_dist_per_quadrant`
        // (product_coding_loop.c:8065) scores the per-quadrant SSE with
        // `svt_full_distortion_kernel16_bits` at `hbd_md` — the 10-bit source
        // (`input_pic` u16, real low bits under SVTAV1_HBD_SRC) vs the 10-bit
        // `cand_bf->recon`. The measured `rec_dist_per_quadrant` therefore
        // sits ~16x above its u8 twin — and `skip_sub_depth` cond1's
        // `quad_deviation_th` (250 at every enabled level) is calibrated to
        // that scale, so feeding it the u8 SSE over-fires the skip: at p4+
        // the port quashed splits C still tested (measured on
        // partial-chroma p4q10 (48,52): C quad=[1495,519,1972,712] std=588
        // vs the port's u8 std=69).
        //
        // The recon the gate reads is the shared `cand_bf->recon` state:
        // bypass_encdec=0 -> the WINNER's rebuild (`win_recon10`);
        // bypass_encdec=1 -> the LAST MDS3 candidate's depth-0 luma recon +
        // its chroma (`gate_y10`/`gate_uv10`). Chroma has no tx-depth split,
        // so the chroma twin is the same recon in both cases.
        let yrec = ev.gate_y();
        let (yrec10, urec10, vrec10) = if self.fx.frame.cfg.bypass_encdec {
            let (u, v) = ev.gate_uv10();
            (ev.gate_y10(), u, v)
        } else {
            let (u, v) = ev.win_uv_recon10();
            (ev.win_recon10(), u, v)
        };
        // The 10-bit SOURCE: `fx.src10` (real u16 planes on a native-HBD
        // encode, where SVTAV1_HBD_SRC low bits are load-bearing) when it
        // exists; `u8 << 2` otherwise — C's driver feeds exactly `u8 << 2`
        // for an 8-bit input, so the fallback is C-exact there.
        let s10 = self.fx.src10;
        // Take the bd10 path only when every plane the gate reads has its
        // 10-bit recon (luma always; chroma only when the sub-quadrant
        // carries it, `quad > 4`). This can never mix a 10-bit plane with
        // an 8-bit one in a quadrant SSE, and falls back to the u8 path on
        // any block whose bd10 recon is absent (rather than panicking).
        let bd10 = !yrec10.is_empty() && (quad <= 4 || (!urec10.is_empty() && !vrec10.is_empty()));
        for r in 0..2usize {
            for c in 0..2usize {
                let mut d: u64 = 0;
                for y in 0..quad {
                    let sy = (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                    let ry = (r * quad + y) * sq + c * quad;
                    for x in 0..quad {
                        let diff = if bd10 {
                            let src = match s10 {
                                Some(s) => {
                                    s.y[(ev.abs_y + r * quad + y) * s.y_stride
                                        + ev.abs_x
                                        + c * quad
                                        + x] as i64
                                }
                                None => (self.y_src[sy + x] as i64) << 2,
                            };
                            src - yrec10[ry + x] as i64
                        } else {
                            self.y_src[sy + x] as i64 - yrec[ry + x] as i64
                        };
                        d += (diff * diff) as u64;
                    }
                }
                if quad > 4 {
                    let cq = quad / 2;
                    let (urec, vrec) = ev.gate_uv();
                    let cw = sq / 2;
                    let ccx = ev.abs_x / 2 + c * cq;
                    let ccy = ev.abs_y / 2 + r * cq;
                    for y in 0..cq {
                        let sy = (ccy + y) * self.fx.c_stride + ccx;
                        let ry = (r * cq + y) * cw + c * cq;
                        for x in 0..cq {
                            let (du, dv) = if bd10 {
                                let (us, vs) = match s10 {
                                    Some(s) => (
                                        s.u[(ccy + y) * s.c_stride + ccx + x] as i64,
                                        s.v[(ccy + y) * s.c_stride + ccx + x] as i64,
                                    ),
                                    None => (
                                        (self.fx.u_src[sy + x] as i64) << 2,
                                        (self.fx.v_src[sy + x] as i64) << 2,
                                    ),
                                };
                                (us - urec10[ry + x] as i64, vs - vrec10[ry + x] as i64)
                            } else {
                                (
                                    self.fx.u_src[sy + x] as i64 - urec[ry + x] as i64,
                                    self.fx.v_src[sy + x] as i64 - vrec[ry + x] as i64,
                                )
                            };
                            d += (du * du) as u64 + (dv * dv) as u64;
                        }
                    }
                }
                dists[r * 2 + c] = d;
            }
        }
        #[cfg(feature = "std")]
        if nsqdbg_here(ev.abs_x, ev.abs_y) {
            // Luma-only re-pass for the SKIPSUBQ-parity dump.
            let mut luma = [0u64; 4];
            for r in 0..2usize {
                for c in 0..2usize {
                    let mut d: u64 = 0;
                    for y in 0..quad {
                        let sy =
                            (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                        let ry = (r * quad + y) * sq + c * quad;
                        for x in 0..quad {
                            let diff = self.y_src[sy + x] as i64 - yrec[ry + x] as i64;
                            d += (diff * diff) as u64;
                        }
                    }
                    luma[r * 2 + c] = d;
                }
            }
            // Pred-vs-input quads from the whole-block depth-0 prediction —
            // the C-side probe's predq counterpart (what cand_bf->pred holds
            // at C's fill time is the open question this answers).
            let pred = ev.dbg_pred();
            let mut predq = [0u64; 4];
            for r in 0..2usize {
                for c in 0..2usize {
                    let mut d: u64 = 0;
                    for y in 0..quad {
                        let sy =
                            (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                        let ry = (r * quad + y) * sq + c * quad;
                        for x in 0..quad {
                            let diff = self.y_src[sy + x] as i64 - pred[ry + x] as i64;
                            d += (diff * diff) as u64;
                        }
                    }
                    predq[r * 2 + c] = d;
                }
            }
            // u16-domain luma-only quads — the C SUBSKIP dump's luma half
            // (C's rec_dist_per_quadrant mixes luma+chroma; the split
            // isolates which plane diverges).
            let mut luma10 = [0u64; 4];
            if bd10 {
                for r in 0..2usize {
                    for c in 0..2usize {
                        let mut d: u64 = 0;
                        for y in 0..quad {
                            let ry = (r * quad + y) * sq + c * quad;
                            for x in 0..quad {
                                let src = match s10 {
                                    Some(s) => {
                                        s.y[(ev.abs_y + r * quad + y) * s.y_stride
                                            + ev.abs_x
                                            + c * quad
                                            + x] as i64
                                    }
                                    None => {
                                        (self.y_src[(ev.abs_y + r * quad + y) * self.y_src_stride
                                            + ev.abs_x
                                            + c * quad
                                            + x] as i64)
                                            << 2
                                    }
                                };
                                let diff = src - yrec10[ry + x] as i64;
                                d += (diff * diff) as u64;
                            }
                        }
                        luma10[r * 2 + c] = d;
                    }
                }
            }
            let (rec_smp, src_smp, pred_smp): (Vec<u16>, Vec<u16>, Vec<u16>) = if bd10 {
                (
                    (0..8).map(|i| yrec10[i]).collect(),
                    (0..4)
                        .map(|x| match s10 {
                            Some(s) => s.y[ev.abs_y * s.y_stride + ev.abs_x + x],
                            None => 0,
                        })
                        .collect(),
                    (0..4)
                        .map(|i| ev.dbg_pred10().get(i).copied().unwrap_or(0))
                        .collect(),
                )
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };
            eprintln!(
                "NSQDBG SKIPSUBQ mi=({},{}) sq={} luma={:?} tot={:?} predq={:?} luma10={:?} rec10={:?} src10={:?} pred10={:?}",
                ev.abs_y / 4,
                ev.abs_x / 4,
                sq,
                luma,
                dists,
                predq,
                luma10,
                rec_smp,
                src_smp,
                pred_smp,
            );
        }
        dists
    }

    /// C `eval_sub_depth_skip_cond1` (product_coding_loop.c:10871): f32
    /// std-deviation of the winner's per-quadrant recon SSE and the
    /// nonzero-coefficient percentage.
    pub(super) fn sub_depth_skip_cond1(&self, ev: &LeafEval, quad: &[u64; 4]) -> bool {
        let ss = self.skip_sub();
        // C float arithmetic (sum/average/pow/sqrtf).
        let n = 4f32;
        let sum: f32 = quad.iter().map(|&d| d as f32).sum();
        let average = sum / n;
        let sum1: f32 = quad
            .iter()
            .map(|&d| {
                let x = d as f32 - average;
                x * x
            })
            .sum();
        let variance = sum1 / n;
        let std_deviation = variance.sqrt();
        let total_samples = (ev.w * ev.h) as u32;
        let coeff_perc = ev.cnt_nz_coeff() * 100 / total_samples;
        std_deviation < ss.quad_deviation_th as f32 && coeff_perc < u32::from(ss.coeff_perc)
    }

    /// Save/restore span of a node rect on the ALIGNED-strided recon planes.
    /// A node whose square extent STRADDLES past the aligned width would, at
    /// stride `y_stride`, read/write the off-aligned columns out of the row and
    /// into the NEXT row's low columns. `commit_leaf` already clips its writes
    /// to the row boundary for exactly that reason (leaf_funnel.rs:7705), so
    /// nothing inside the node ever modifies those wrapped bytes and the
    /// snapshot must clip identically — otherwise `restore_snap` would write
    /// stale bytes over an already-committed neighbour's recon. Byte-neutral
    /// wherever nothing straddles (`abs + span <= stride`), i.e. always on a
    /// 64-aligned frame.
    #[inline]
    pub(super) fn clip_span(stride: usize, abs: usize, span: usize) -> usize {
        span.min(stride.saturating_sub(abs))
    }

    /// Fill this node size's reusable snapshot slot, IN PLACE.
    ///
    /// `clone_from` and `resize` rather than fresh vectors: the derived
    /// `clone_from` is field-wise and `Vec::clone_from` reuses the
    /// destination's allocation, so after the first node of a given size this
    /// allocates nothing. See [`DepthWalk::snaps`].
    pub(super) fn take_snap(&mut self, abs_x: usize, abs_y: usize, size: usize) {
        let slot = size.trailing_zeros() as usize;
        let mut snap = core::mem::take(&mut self.snaps[slot]);
        let yw = Self::clip_span(self.y_stride, abs_x, size);
        snap.y.clear();
        snap.y.resize(size * size, 0);
        for r in 0..size {
            let src = (abs_y + r) * self.y_stride + abs_x;
            snap.y[r * size..r * size + yw].copy_from_slice(&self.y_recon[src..src + yw]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        let cwid = Self::clip_span(self.fx.c_stride, cx, half);
        snap.u.clear();
        snap.u.resize(half * half, 0);
        snap.v.clear();
        snap.v.resize(half * half, 0);
        for r in 0..half {
            let src = (cy + r) * self.fx.c_stride + cx;
            snap.u[r * half..r * half + cwid].copy_from_slice(&self.fx.u_recon[src..src + cwid]);
            snap.v[r * half..r * half + cwid].copy_from_slice(&self.fx.v_recon[src..src + cwid]);
        }
        snap.ectx.restore_from(self.fx.ectx);
        self.snaps[slot] = snap;
    }

    pub(super) fn restore_snap(
        &mut self,
        snap: &NodeSnap,
        abs_x: usize,
        abs_y: usize,
        size: usize,
    ) {
        // `restore_from`, NOT `= clone()` and not `Clone::clone_from`:
        // `#[derive(Clone)]` does not override `clone_from`, so the trait
        // default is `*self = source.clone()` and allocates every vector.
        // See `EntropyCtx::restore_from`.
        self.fx.ectx.restore_from(&snap.ectx);
        let yw = Self::clip_span(self.y_stride, abs_x, size);
        for r in 0..size {
            let dst = (abs_y + r) * self.y_stride + abs_x;
            self.y_recon[dst..dst + yw].copy_from_slice(&snap.y[r * size..r * size + yw]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        let cwid = Self::clip_span(self.fx.c_stride, cx, half);
        for r in 0..half {
            let dst = (cy + r) * self.fx.c_stride + cx;
            self.fx.u_recon[dst..dst + cwid].copy_from_slice(&snap.u[r * half..r * half + cwid]);
            self.fx.v_recon[dst..dst + cwid].copy_from_slice(&snap.v[r * half..r * half + cwid]);
        }
    }

    /// C `update_skip_nsq_based_on_split_rate` (product_coding_loop.c:
    /// 10181): the four partition-rate sub-gates.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn skip_by_split_rate(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        best_part: PartitionType,
        ctx_row: usize,
        sq_size: usize,
        split_flag: bool,
    ) -> bool {
        let nsq = self.nsq;
        let sq_cost = sq.ev.block_cost(self.lambda);

        let mut nsq_split_cost_th = nsq.nsq_split_cost_th;
        if nsq_split_cost_th != 0 {
            if sq_size <= 16 {
                // C `MAX(1, nsq_split_cost_th - rate_th_offset_lte16)`
                // (product_coding_loop.c:9732) in **uint32_t**: both fields are
                // `uint32_t` (md_process.h:565/576), so when the qp-scaled
                // threshold is SMALLER than the offset the subtraction
                // UNDERFLOWS to ~4.29e9 and `MAX(1, ..)` keeps it — turning a
                // gate that would skip every shape into one that skips none.
                //
                // Reproduced, not tidied: a C bug is still the oracle
                // (docs/SUSPECTED-C-BUGS.md #28). It is REACHABLE and it moves
                // bytes: on the video arm at M6 the level-18 row's
                // `nsq_split_cost_th` 40 scales by `qp/63`, giving 13 at CLI qp
                // 20 against a `rate_th_offset_lte16` of 15 — the only cell in
                // the campaign's grid where the two cross (q40 gives 25, q55
                // gives 37). `saturating_sub(..).max(1)` here made the port skip
                // the HORZ shape at `diag 64x64 q20 p6` mi=(8,12) that C
                // evaluates and CHOOSES.
                nsq_split_cost_th = u64::from(
                    u32::try_from(nsq_split_cost_th)
                        .unwrap_or(u32::MAX)
                        .wrapping_sub(u32::try_from(nsq.rate_th_offset_lte16).unwrap_or(u32::MAX))
                        .max(1),
                );
            }
            let split_rate = self.part_rates.bits(ctx_row, shape);
            let part_cost = rdcost(self.lambda, split_rate, 0);
            if part_cost * 1000 > sq_cost * nsq_split_cost_th {
                return true;
            }
        }

        let mut h_vs_v_th = nsq.h_vs_v_split_rate_th;
        if h_vs_v_th != 0 && matches!(shape, PartitionType::Horz | PartitionType::Vert) {
            if sq_size <= 16 {
                h_vs_v_th += nsq.rate_th_offset_lte16;
            }
            let h_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Horz),
                0,
            );
            let v_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Vert),
                0,
            );
            if shape == PartitionType::Horz && h_cost * h_vs_v_th > v_cost * 100 {
                return true;
            }
            if shape == PartitionType::Vert && v_cost * h_vs_v_th > h_cost * 100 {
                return true;
            }
        }

        let mut non_hv_th = nsq.non_hv_split_rate_th;
        if non_hv_th != 0 && !matches!(shape, PartitionType::Horz | PartitionType::Vert) {
            if sq_size <= 16 {
                non_hv_th += nsq.rate_th_offset_lte16;
            }
            let part_cost = rdcost(self.lambda, self.part_rates.bits(ctx_row, shape), 0);
            let best_cost = rdcost(self.lambda, self.part_rates.bits(ctx_row, best_part), 0);
            if part_cost * non_hv_th > best_cost * 100 {
                return true;
            }
        }

        let mut lower_th = nsq.lower_depth_split_cost_th;
        if lower_th != 0 && split_flag {
            if sq_size <= 16 {
                lower_th += nsq.rate_th_offset_lte16;
            }
            let split_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Split),
                0,
            );
            if split_cost * 10000 < sq_cost * lower_th {
                return true;
            }
        }

        if nsq.component_multiple_th != 0 {
            let rate_cost = rdcost(self.lambda, sq.ev.total_rate(), 0);
            let dist_cost = rdcost(self.lambda, 0, sq.ev.full_dist());
            let max_comp = rate_cost.max(dist_cost);
            let min_comp = rate_cost.min(dist_cost);
            if max_comp > nsq.component_multiple_th * min_comp {
                return true;
            }
        }
        false
    }

    /// C `update_skip_nsq_based_on_sq_txs` (:10533): parent-SQ TX-split
    /// nonzero counts vs the SQ winner's count.
    pub(super) fn skip_by_sq_txs(&self, shape: PartitionType, sq: &SqInfo) -> bool {
        if !self.nsq.psq_txs {
            return false;
        }
        let Some((nz_h, nz_v)) = sq.min_nz else {
            return false;
        };
        let cnt_nz = sq.ev.cnt_nz_coeff() as u64;
        // psq_txs_lvl 1: hv_to_sq_th 1000, h_to_v_th 100.
        let (hv_to_sq_th, h_to_v_th) = (1000u64, 100u64);
        let cnt_h_best = (nz_h as u64) << 1;
        let cnt_v_best = (nz_v as u64) << 1;
        if cnt_h_best >= cnt_nz * hv_to_sq_th / 100 && cnt_v_best >= cnt_nz * hv_to_sq_th / 100 {
            return true;
        }
        if matches!(
            shape,
            PartitionType::Horz
                | PartitionType::Horz4
                | PartitionType::HorzA
                | PartitionType::HorzB
        ) && cnt_v_best <= cnt_h_best
            && cnt_h_best >= cnt_nz * h_to_v_th / 100
        {
            return true;
        }
        if matches!(
            shape,
            PartitionType::Vert
                | PartitionType::Vert4
                | PartitionType::VertA
                | PartitionType::VertB
        ) && cnt_h_best <= cnt_v_best
            && cnt_v_best >= cnt_nz * h_to_v_th / 100
        {
            return true;
        }
        false
    }

    /// The parent-SQ-mode modulation of `max_part0_to_part1_dev`, C's
    /// `switch (sq_blk_ptr->block_mi.mode)` at
    /// `product_coding_loop.c:9867-9895`, transcribed WHOLE.
    ///
    /// `mode` is C's unified `block_mi.mode` (see
    /// [`crate::leaf_funnel::types::LeafEval::block_mi_mode`]), so the inter
    /// arms are reachable. Three of C's five groups have inter members and
    /// none of them is the `default:` arm:
    ///
    /// | C arm | modes | effect |
    /// |---|---|---|
    /// | `:9868` | `NEWMV`(16), `NEW_NEWMV`(24) | `* 75 / 100` |
    /// | `:9872` | `DC`(0), `H`(2), `V`(1), `NEAREST_NEARESTMV`(17), `NEAR_NEARMV`(18) | `* 2` |
    /// | `:9879` | `D45..D67`(3..8), `SMOOTH*`(9..11), `PAETH`(12), `GLOBALMV`(15), `GLOBAL_GLOBALMV`(23) | `<< 2` |
    /// | `default` | `NEARESTMV`(13), `NEARMV`(14), and the mixed compounds (19..22) | unchanged |
    ///
    /// C writes the first arm as `((max * 75) / 100)` on a `uint32_t`; the
    /// port's `u64` cannot differ for any value the level table produces
    /// (the largest is 80).
    pub(super) fn nsq_dev_by_parent_mode(mode: u8, max_dev: u64) -> u64 {
        use svtav1_types::prediction::PredictionMode as Pm;
        const NEWMV: u8 = Pm::NewMv as u8;
        const NEW_NEWMV: u8 = Pm::NewNewMv as u8;
        const NEAREST_NEARESTMV: u8 = Pm::NearestNearestMv as u8;
        const NEAR_NEARMV: u8 = Pm::NearNearMv as u8;
        const GLOBALMV: u8 = Pm::GlobalMv as u8;
        const GLOBAL_GLOBALMV: u8 = Pm::GlobalGlobalMv as u8;
        match mode {
            NEWMV | NEW_NEWMV => (max_dev * 75) / 100,
            0..=2 | NEAREST_NEARESTMV | NEAR_NEARMV => max_dev * 2,
            3..=12 | GLOBALMV | GLOBAL_GLOBALMV => max_dev << 2,
            _ => max_dev,
        }
    }

    /// C `update_skip_nsq_based_on_sq_recon_dist` (:9847).
    pub(super) fn skip_by_recon_dist(&self, shape: PartitionType, sq: &SqInfo) -> bool {
        let mut max_dev = self.nsq.max_part0_to_part1_dev;
        #[cfg(feature = "std")]
        if nsqdbg_here(sq.ev.abs_x, sq.ev.abs_y) {
            eprintln!(
                "NSQDBG RDENTRY sl={} mi=({},{}) bsize={} shape={} maxdev={} quad={}",
                u8::from(self.fx.frame.non_i_slice),
                sq.ev.abs_y / 4,
                sq.ev.abs_x / 4,
                c_bsize_sq(sq.ev.w),
                c_part(shape),
                max_dev,
                u8::from(sq.quad.is_some()),
            );
        }
        if max_dev == 0 {
            return false;
        }
        let Some(quad) = &sq.quad else {
            return false;
        };
        let full_lambda = self.lambda;
        let dist = rdcost(full_lambda, 0, sq.ev.full_dist());
        let cost = sq.ev.block_cost(self.lambda);
        let dist_cost_ratio = (dist * 100) / cost;
        let (min_ratio, max_ratio) = (50u64, 100u64);
        let modulated_th = if dist_cost_ratio > min_ratio {
            (100 * (dist_cost_ratio - min_ratio)) / (max_ratio - min_ratio)
        } else {
            0 // unused: the <= min_ratio arm forces the threshold to 0
        };

        // Parent SQ mode modulation, C's FULL table
        // (product_coding_loop.c:9867-9895). C switches on
        // `sq_blk_ptr->block_mi.mode`, which is the UNIFIED mode field: intra
        // modes 0..12 AND inter modes 13..24 all land in the same switch.
        //
        // TRANSCRIPTION DEFECT, fixed 2026-09-04 (`docs/INTER-ENCODE-PLAN.md`
        // §1z³⁵): this used to read `sq.ev.mode()` and match only `0..=2` /
        // `3..=12`. `sq.ev.mode()` is `Cand::mode`, the intra y_mode, which an
        // inter candidate is injected with as **0** (`leaf_funnel/inject.rs` —
        // an inter block codes no intra y_mode). So an inter winner took C's
        // `DC_PRED` arm and got `max_dev *= 2`, where C gives `* 75 / 100`
        // for NEWMV, `<< 2` for GLOBALMV and NO modulation for
        // NEARESTMV/NEARMV. Read `block_mi_mode()`, which is C's field.
        //
        // LATENT on the campaign envelope, and measured so, not assumed: on
        // the 96-cell grid (frames=2 and 3) this gate is never ENTERED on an
        // inter frame — `tools/nsq_inter_reach_census.sh` counts zero
        // `RDENTRY sl=1` because the split-rate gate ahead of it kills every
        // NSQ shape those frames test — so no grid cell moved. The arm is
        // reachable the moment an inter frame's SQ winner survives gate 1.
        let mode = sq.ev.block_mi_mode();
        #[cfg(feature = "std")]
        if nsqdbg_here(sq.ev.abs_x, sq.ev.abs_y) {
            // The join point for the §1z³⁵ differential: `bmm` is C's
            // `block_mi.mode` and `ymode` is the intra y_mode this gate used
            // to read. They differ on every inter winner.
            eprintln!(
                "NSQDBG RECONDIST sl={} mi=({},{}) bsize={} shape={} ymode={} bmm={} dev_in={} dev_mode={}",
                u8::from(self.fx.frame.non_i_slice),
                sq.ev.abs_y / 4,
                sq.ev.abs_x / 4,
                c_bsize_sq(sq.ev.w),
                c_part(shape),
                sq.ev.mode(),
                mode,
                max_dev,
                Self::nsq_dev_by_parent_mode(mode, max_dev),
            );
        }
        max_dev = Self::nsq_dev_by_parent_mode(mode, max_dev);

        let dq: [u64; 4] = [
            quad[0].max(1),
            quad[1].max(1),
            quad[2].max(1),
            quad[3].max(1),
        ];
        if matches!(
            shape,
            PartitionType::Horz
                | PartitionType::Horz4
                | PartitionType::HorzA
                | PartitionType::HorzB
        ) {
            // V/D67/D113/D45/D135 -> x4; H -> 0.
            if matches!(mode, 1 | 8 | 5 | 3 | 4) {
                max_dev <<= 2;
            } else if mode == 2 {
                max_dev = 0;
            }
            let dist_h0 = dq[0] + dq[1];
            let dist_h1 = dq[2] + dq[3];
            let dev =
                ((dist_h0 as i64 - dist_h1 as i64).unsigned_abs() * 100) / dist_h0.min(dist_h1);
            let quad_dev_t =
                ((dq[0] as i64 - dq[1] as i64).unsigned_abs() * 100) / dq[0].min(dq[1]);
            let quad_dev_b =
                ((dq[2] as i64 - dq[3] as i64).unsigned_abs() * 100) / dq[2].min(dq[3]);
            max_dev += max_dev * quad_dev_t.min(quad_dev_b) / 100;
            max_dev = if dist_cost_ratio <= min_ratio {
                0
            } else if dist_cost_ratio <= max_ratio {
                (max_dev * modulated_th) / 100
            } else {
                dist_cost_ratio
            };
            if dev < max_dev {
                return true;
            }
        }
        if matches!(
            shape,
            PartitionType::Vert
                | PartitionType::Vert4
                | PartitionType::VertA
                | PartitionType::VertB
        ) {
            // H/D157/D203/D45/D135 -> x4; V -> 0.
            if matches!(mode, 2 | 6 | 7 | 3 | 4) {
                max_dev <<= 2;
            } else if mode == 1 {
                max_dev = 0;
            }
            let dist_v0 = dq[0] + dq[2];
            let dist_v1 = dq[1] + dq[3];
            let dev =
                ((dist_v0 as i64 - dist_v1 as i64).unsigned_abs() * 100) / dist_v0.min(dist_v1);
            let quad_dev_l =
                ((dq[0] as i64 - dq[2] as i64).unsigned_abs() * 100) / dq[0].min(dq[2]);
            let quad_dev_r =
                ((dq[1] as i64 - dq[3] as i64).unsigned_abs() * 100) / dq[1].min(dq[3]);
            max_dev += max_dev * quad_dev_l.min(quad_dev_r) / 100;
            max_dev = if dist_cost_ratio <= min_ratio {
                0
            } else if dist_cost_ratio <= max_ratio {
                (max_dev * modulated_th) / 100
            } else {
                dist_cost_ratio
            };
            if dev < max_dev {
                return true;
            }
        }
        false
    }

    /// C `update_skip_nsq_shapes` (:10454): SQ-vs-H/V relative-cost skip
    /// for H4/V4 and the asymmetric HA/HB/VA/VB shapes.
    pub(super) fn skip_by_shapes(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        h_children: &Option<[(u64, bool); 2]>,
        v_children: &Option<[(u64, bool); 2]>,
    ) -> bool {
        let mut sq_weight = self.nsq.sq_weight;
        if sq_weight == u64::MAX {
            return false;
        }
        if matches!(shape, PartitionType::Horz4 | PartitionType::Vert4) {
            sq_weight += Self::CONSERVATIVE_OFFSET_0;
        }
        let sq_cost = sq.ev.block_cost(self.lambda);
        if matches!(
            shape,
            PartitionType::Horz4 | PartitionType::HorzA | PartitionType::HorzB
        ) && let Some(h) = h_children
        {
            if (shape == PartitionType::HorzA && !h[0].1)
                || (shape == PartitionType::HorzB && !h[1].1)
            {
                sq_weight -= 10; // C AGGRESSIVE_OFFSET_1
            }
            let h_cost = h[0].0 + h[1].0;
            let mut skip = h_cost > (sq_cost * sq_weight) / 100;
            if !skip && let Some(v) = v_children {
                let v_cost = v[0].0 + v[1].0;
                skip = h_cost > (v_cost * self.nsq.hv_weight) / 100;
            }
            return skip;
        }
        if matches!(
            shape,
            PartitionType::Vert4 | PartitionType::VertA | PartitionType::VertB
        ) && let Some(v) = v_children
        {
            if (shape == PartitionType::VertA && !v[0].1)
                || (shape == PartitionType::VertB && !v[1].1)
            {
                sq_weight -= 10; // C AGGRESSIVE_OFFSET_1
            }
            let v_cost = v[0].0 + v[1].0;
            let mut skip = v_cost > (sq_cost * sq_weight) / 100;
            if !skip && let Some(h) = h_children {
                let h_cost = h[0].0 + h[1].0;
                skip = v_cost > (h_cost * self.nsq.hv_weight) / 100;
            }
            return skip;
        }
        false
    }

    /// C `get_skip_processing_nsq_block` (:10826): the gates in order.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn skip_processing_nsq(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        best_part: PartitionType,
        ctx_row: usize,
        sq_size: usize,
        split_flag: bool,
        h_children: &Option<[(u64, bool); 2]>,
        v_children: &Option<[(u64, bool); 2]>,
    ) -> bool {
        #[cfg(feature = "std")]
        if nsqdbg_here(sq.ev.abs_x, sq.ev.abs_y) {
            eprintln!(
                "NSQDBG SPENTRY sl={} mi=({},{}) bsize={} shape={}",
                u8::from(self.fx.frame.non_i_slice),
                sq.ev.abs_y / 4,
                sq.ev.abs_x / 4,
                c_bsize_sq(sq.ev.w),
                c_part(shape),
            );
        }
        if self.skip_by_split_rate(shape, sq, best_part, ctx_row, sq_size, split_flag) {
            return true;
        }
        if self.skip_by_sq_txs(shape, sq) {
            return true;
        }
        if self.skip_by_recon_dist(shape, sq) {
            return true;
        }
        if self.skip_by_shapes(shape, sq, h_children, v_children) {
            return true;
        }
        false
    }

    /// C `set_blocks_to_test` (enc_dec_process.c:1394-1438) for this node —
    /// the d1 shape list, honouring the frame-boundary rules. Returns an EMPTY
    /// list for C's `tot_shapes = 0` (forced SPLIT).
    ///
    /// The three C rules this reproduces, none of which can fire on a
    /// 64-aligned frame (both flags are always true there, so the function
    /// degenerates to `shapes_for_size`):
    ///  * both flags false -> `tot_shapes = 0` (:1405-1410);
    ///  * exactly one false, NSQ geometry OFF -> also `tot_shapes = 0` (same
    ///    clause; the `sq_size <= MAX(min_nsq, min_nsq_block_size)` term is
    ///    inert here — an edge node on an 8-aligned frame is always >= 16);
    ///  * exactly one false, NSQ geometry ON -> `inj_hv_incomp` keeps EXACTLY
    ///    ONE shape and EXCLUDES PARTITION_NONE (:1417-1421): PART_H when
    ///    `!has_rows`, PART_V when `!has_cols`. Note `max_part` is PART_V for
    ///    an incomplete node even when `md_disallow_nsq_search` is set (:1414
    ///    ANDs that term with `!inj_hv_incomp`), so presets 4/5 — whose NSQ
    ///    SEARCH is off — still inject the edge shape.
    pub(super) fn shapes_at(
        &self,
        size: usize,
        has_rows: bool,
        has_cols: bool,
    ) -> &'static [PartitionType] {
        shapes_at_edge(size, self.nsq, self.nsq_geom_enabled, has_rows, has_cols)
    }

    /// C `test_depth`'s `shape_block_cnt` adjustment (product_coding_loop.c:
    /// 10899-10904): a single-edge node codes only the FIRST rect of its
    /// injected shape (the in-frame half), and an H4/V4 whose 4th quarter
    /// starts outside the aligned frame drops that quarter.
    pub(super) fn shape_block_cnt(
        &self,
        size: usize,
        shape: PartitionType,
        n: usize,
        abs_x: usize,
        abs_y: usize,
        has_rows: bool,
        has_cols: bool,
    ) -> usize {
        shape_block_cnt_edge(
            size,
            shape,
            n,
            abs_x,
            abs_y,
            self.aligned_w,
            self.aligned_h,
            has_rows,
            has_cols,
        )
    }

    /// C `svt_aom_pick_partition` (product_coding_loop.c:11549) —
    /// test_depth (:11396, the d1 shape loop) + the sub-depth walk.
    /// `None` mirrors C's `pc_tree->rdc.valid == 0` return: the node produced
    /// no valid partition at all, which invalidates the parent's SPLIT.
    pub(super) fn pick(&mut self, scan: &RefScan, abs_x: usize, abs_y: usize) -> Option<NodeRes> {
        let size = scan.sq;
        let mut split_flag = scan.split_flag;
        let (has_rows, has_cols) = edge_flags(abs_x, abs_y, size, self.aligned_w, self.aligned_h);

        // C test_depth state: rdc (best partition so far), the SQ info,
        // the H/V child costs for the H4/V4 gates, and the winning
        // shape's evaluations for the final commit.
        let mut best: Option<(PartitionType, u64, Vec<LeafEval>)> = None;
        let mut sq_info: Option<SqInfo> = None;
        let mut h_children: Option<[(u64, bool); 2]> = None;
        let mut v_children: Option<[(u64, bool); 2]> = None;
        // C update_redundant: first-child reuse HB<-H, VB<-V, VA<-HA.
        // Keep only the three candidates needed by those future shapes.
        let mut redundant: [Option<LeafEval>; 3] = [None, None, None];
        // Whether this node's reusable snapshot slot (see [`DepthWalk::snaps`])
        // currently holds a valid save. It used to be an `Option<NodeSnap>`
        // that OWNED the snapshot, which meant a fresh one per node.
        let snap_slot = size.trailing_zeros() as usize;
        let mut snap_taken = false;
        let mut committed_since_snap = false;

        let shapes = self.shapes_at(size, has_rows, has_cols);
        // C `svt_aom_pick_partition`: `if (mds->tot_shapes) test_depth(...)`.
        // An empty list is `tot_shapes = 0` — the node force-splits.
        if scan.test_this && !shapes.is_empty() {
            // update_part_neighs: partition contexts read once per node.
            let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);
            #[cfg(feature = "std")]
            if nsqdbg_here(abs_x, abs_y) {
                let (ab, lb) = self.fx.ectx.part_ctx_bytes(abs_x, abs_y);
                eprintln!(
                    "NSQDBG PCTX mi=({},{}) bsize={} cr={} ab={} lb={} row={:?}",
                    abs_y / 4,
                    abs_x / 4,
                    c_bsize_sq(size),
                    ctx_row,
                    ab,
                    lb,
                    self.part_rates.row(ctx_row),
                );
            }

            for &shape in shapes {
                // Restore the pre-shape state (C: copy [1] -> [0] at
                // nsi == 0 when a previous shape saved it).
                if committed_since_snap && snap_taken {
                    // Move the slot out and back so the restore can borrow it
                    // while `self` is borrowed mutably; the slot is this node's
                    // alone for the whole call.
                    let sn = core::mem::take(&mut self.snaps[snap_slot]);
                    self.restore_snap(&sn, abs_x, abs_y, size);
                    self.snaps[snap_slot] = sn;
                    committed_since_snap = false;
                }

                // C `svt_aom_partition_rate_cost` (rd_cost.c:1837) returns 0 for
                // `bsize < BLOCK_8X8`: a 4x4 codes NO partition symbol. The only
                // square `size` node below 8 is the 4x4 (4x8/8x4 are NSQ children,
                // not square nodes), so gate the partition rate there.
                let part_rate = if size >= 8 {
                    self.part_rates
                        .bits_edge(ctx_row, shape, has_rows, has_cols)
                } else {
                    0
                };
                let mut part_cost = rdcost(self.lambda, part_rate, 0);
                let mut children = shape_children(size, shape);
                // C `shape_block_cnt--` (product_coding_loop.c:10899-10904):
                // drop the trailing out-of-frame sub-block. Inert on a
                // 64-aligned frame (both flags true, H4/V4 quarters in-frame).
                children.truncate(self.shape_block_cnt(
                    size,
                    shape,
                    children.len(),
                    abs_x,
                    abs_y,
                    has_rows,
                    has_cols,
                ));
                let mut evals: Vec<LeafEval> = Vec::with_capacity(children.len());
                let mut valid = true;

                for (nsi, &(dx, dy, cw, ch)) in children.iter().enumerate() {
                    // C `get_skip_processing_nsq_block`'s four gates each
                    // return false when `pc_tree->tested_blk[PART_N][0]` is
                    // unset (product_coding_loop.c:9717, 9852, 10067, and
                    // `update_skip_nsq_shapes`), which is exactly the
                    // single-edge node where PART_N is never tested.
                    if shape != PartitionType::None
                        && nsi == 0
                        && let Some(sq) = sq_info.as_ref()
                    {
                        // faster_md_settings_nsq: I-slice-dead (C gates
                        // the call on slice_type != I_SLICE, :11470).
                        let best_part = best
                            .as_ref()
                            .map(|(p, _, _)| *p)
                            .unwrap_or(PartitionType::None);
                        if self.skip_processing_nsq(
                            shape,
                            sq,
                            best_part,
                            ctx_row,
                            size,
                            scan.split_flag,
                            &h_children,
                            &v_children,
                        ) {
                            #[cfg(feature = "std")]
                            if nsqdbg_here(abs_x, abs_y) {
                                let g = if self.skip_by_split_rate(
                                    shape,
                                    sq,
                                    best_part,
                                    ctx_row,
                                    size,
                                    scan.split_flag,
                                ) {
                                    1
                                } else if self.skip_by_sq_txs(shape, sq) {
                                    2
                                } else if self.skip_by_recon_dist(shape, sq) {
                                    3
                                } else {
                                    4
                                };
                                eprintln!(
                                    "NSQDBG SKIP sl={} mi=({},{}) bsize={} shape={} gate={}",
                                    u8::from(self.fx.frame.non_i_slice),
                                    abs_y / 4,
                                    abs_x / 4,
                                    c_bsize_sq(size),
                                    c_part(shape),
                                    g,
                                );
                            }
                            valid = false;
                            break;
                        }
                    }

                    let cx = abs_x + dx;
                    let cy = abs_y + dy;
                    // IBC chunk 8: the do_intra_bc gate inputs for this
                    // leaf (mode_decision.c:3597-3616) — the shape under
                    // evaluation + this node's PART_N (square) winner.
                    self.fx.ibc_gate = crate::leaf_funnel::IbcGateInput {
                        partition: shape as u8,
                        is_part_n: shape == PartitionType::None,
                        sibling_n0: match &sq_info {
                            Some(sq) => (true, sq.ev.used_ibc()),
                            None => (false, false),
                        },
                    };
                    let reuse = if nsi == 0 {
                        match shape {
                            PartitionType::HorzB => redundant[0].take(),
                            PartitionType::VertB => redundant[1].take(),
                            PartitionType::VertA => redundant[2].take(),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let ev = reuse.unwrap_or_else(|| {
                        evaluate_leaf(
                            self.fx,
                            self.y_src,
                            self.y_src_stride,
                            cy * self.y_src_stride + cx,
                            self.y_recon,
                            self.y_stride,
                            cx,
                            cy,
                            cw,
                            ch,
                            false, // is_dc_only gate: eff-M9 only
                            // sb_is_lvl6: ignored here (txs_lvl6_gate is false for
                            // every preset that reaches the depth-refine walk).
                            true,
                        )
                    });
                    if self.nsq.allow_hva_hvb && nsi == 0 {
                        let slot = match shape {
                            PartitionType::Horz => Some(0),
                            PartitionType::Vert => Some(1),
                            PartitionType::HorzA => Some(2),
                            _ => None,
                        };
                        if let Some(slot) = slot {
                            redundant[slot] = Some(ev.clone());
                        }
                    }
                    #[cfg(feature = "std")]
                    if nsqdbg_here(abs_x, abs_y) {
                        eprintln!(
                            "NSQDBG BLK mi=({},{}) bsize={} shape={} nsi={} cost={} rate={} dist={} mode={} coeff={} nz={} txd={} uv={} txt=[{}] ye=[{}] ue={} ve={} fi={} ady={} aduv={} qdc=[{}] {}",
                            abs_y / 4,
                            abs_x / 4,
                            c_bsize_sq(size),
                            c_part(shape),
                            nsi,
                            ev.block_cost(self.lambda),
                            ev.total_rate(),
                            ev.full_dist(),
                            ev.mode(),
                            u8::from(ev.block_has_coeff()),
                            ev.cnt_nz_coeff(),
                            ev.tx_depth(),
                            ev.uv_mode(),
                            ev.dbg_txb_types(),
                            ev.dbg_txb_eobs(),
                            ev.dbg_uv_eobs().0,
                            ev.dbg_uv_eobs().1,
                            ev.dbg_fi(),
                            ev.dbg_deltas().0,
                            ev.dbg_deltas().1,
                            ev.dbg_qdcs(),
                            ev.dbg_inter(),
                        );
                    }
                    part_cost += ev.block_cost(self.lambda);
                    evals.push(ev);

                    if let Some((_, best_rd, _)) = &best
                        && part_cost >= *best_rd
                    {
                        #[cfg(feature = "std")]
                        if nsqdbg_here(abs_x, abs_y) {
                            eprintln!(
                                "NSQDBG ABORT mi=({},{}) bsize={} shape={} nsi={} part_cost={} best={}",
                                abs_y / 4,
                                abs_x / 4,
                                c_bsize_sq(size),
                                c_part(shape),
                                nsi,
                                part_cost,
                                best_rd,
                            );
                        }
                        valid = false;
                        break;
                    }

                    if nsi + 1 < children.len() {
                        if !snap_taken {
                            self.take_snap(abs_x, abs_y, size);
                            snap_taken = true;
                        }
                        committed_since_snap = true;
                        let ev = evals.last().unwrap();
                        commit_leaf(self.fx, self.y_recon, self.y_stride, ev, shape as u8);
                    }
                }

                // Track H/V child costs for the H4/V4 gates (C
                // tested_blk[PART_H/V][0..1] + block_has_coeff).
                if matches!(shape, PartitionType::Horz | PartitionType::Vert) && evals.len() == 2 {
                    let pair = [
                        (evals[0].block_cost(self.lambda), evals[0].block_has_coeff()),
                        (evals[1].block_cost(self.lambda), evals[1].block_has_coeff()),
                    ];
                    if shape == PartitionType::Horz {
                        h_children = Some(pair);
                    } else {
                        v_children = Some(pair);
                    }
                }

                if shape == PartitionType::None {
                    debug_assert!(valid, "PART_N cannot abort (rdc starts invalid)");
                    let ev = &evals[0];
                    // rec_dist_per_quadrant (C gate :10153): the NSQ
                    // recon-dist arm OR the skip_sub arm.
                    let nsq_arm = self.nsq.enabled
                        && self.nsq.max_part0_to_part1_dev != 0
                        && size >= 8
                        && size > self.nsq.min_nsq;
                    let ss = self.skip_sub();
                    let skip_sub_arm = ss.enabled != 0
                        && size <= usize::from(ss.max_size)
                        && scan.split_flag
                        && (size >= 16 || (!self.disallow_4x4 && size == 8));
                    let quad = if nsq_arm || skip_sub_arm {
                        Some(self.quad_rec_dists(ev))
                    } else {
                        None
                    };
                    // non_normative_txs (C gate :10174).
                    let min_nz = if self.nsq.enabled
                        && self.nsq.psq_txs
                        && size >= 8
                        && size > self.nsq.min_nsq
                    {
                        crate::leaf_funnel::min_nz_hv(
                            ev,
                            self.fx.frame.base_qindex,
                            self.fx.frame.qm_levels[0],
                            self.fx.frame.bit_depth,
                            self.fx.frame.sharpness,
                        )
                    } else {
                        None
                    };
                    sq_info = Some(SqInfo {
                        ev: evals.pop().unwrap(),
                        quad,
                        min_nz,
                    });
                    if valid {
                        best = Some((PartitionType::None, part_cost, Vec::new()));
                    }
                } else if valid {
                    let better = match &best {
                        None => true,
                        Some((_, rd, _)) => part_cost < *rd,
                    };
                    if better {
                        best = Some((shape, part_cost, evals));
                    }
                }
                #[cfg(feature = "std")]
                if nsqdbg_here(abs_x, abs_y) {
                    let (bp, brd) = best
                        .as_ref()
                        .map(|(p, rd, _)| (*p as u32, *rd))
                        .unwrap_or((255, 0));
                    eprint!(
                        "NSQDBG SHAPE mi=({},{}) bsize={} shape={} valid={} part_cost={} part_rate={} cr={} best={}/{}",
                        abs_y / 4,
                        abs_x / 4,
                        c_bsize_sq(size),
                        c_part(shape),
                        u8::from(valid),
                        part_cost,
                        part_rate,
                        ctx_row,
                        bp,
                        brd,
                    );
                    if shape == PartitionType::None {
                        let sq = sq_info.as_ref().unwrap();
                        let q = sq.quad.unwrap_or([0; 4]);
                        let (nzh, nzv) = sq.min_nz.unwrap_or((0, 0));
                        eprint!(
                            " q=[{},{},{},{}] nzh={} nzv={}",
                            q[0], q[1], q[2], q[3], nzh, nzv
                        );
                    }
                    eprintln!();
                }
            }

            // skip_sub_depth cond1 (svt_aom_pick_partition:11563-11568) —
            // on the SQ winner's quadrant dists.
            let ss = self.skip_sub();
            if let Some(sq) = &sq_info
                && split_flag
                && ss.enabled != 0
                && size <= usize::from(ss.max_size)
                && let Some(quad) = &sq.quad
                && self.sub_depth_skip_cond1(&sq.ev, quad)
            {
                split_flag = false;
            }

            // C: restore [1] -> [0] before the sub-depth walk.
            if committed_since_snap && split_flag && snap_taken {
                let sn = core::mem::take(&mut self.snaps[snap_slot]);
                self.restore_snap(&sn, abs_x, abs_y, size);
                self.snaps[snap_slot] = sn;
                committed_since_snap = false;
            }
        }

        let parent_rd = best.as_ref().map(|(_, rd, _)| *rd);
        if split_flag {
            match self.test_split(scan, abs_x, abs_y, parent_rd) {
                SplitOut::Chosen(res) => return Some(*res),
                SplitOut::ParentKept | SplitOut::Invalid => {
                    // Parent (best shape) stays; fall through to its
                    // commit (test_split_partition's winner overwrite).
                }
            }
        }

        // Commit the winning shape (C md_update_all_neighbour_arrays_
        // multiple over the chosen partition's blocks). If a losing
        // shape's partial commits are still live, restore first —
        // equivalent to C's winner-overwrite since every write spans
        // exactly the block.
        if committed_since_snap && snap_taken {
            let sn = core::mem::take(&mut self.snaps[snap_slot]);
            self.restore_snap(&sn, abs_x, abs_y, size);
            self.snaps[snap_slot] = sn;
        }
        // C `svt_aom_pick_partition` returns `pc_tree->rdc.valid` — 0 when the
        // node tested no shape AND its SPLIT was invalid. Reachable only on a
        // partial SB: a `set_child_to_be_tested`-created child (split_flag
        // false, tot_shapes forced 1) that lands on a BOTH-false node, where
        // `set_blocks_to_test` then zeroes tot_shapes and there is nothing to
        // fall back to. The parent's SPLIT is invalidated
        // (product_coding_loop.c:10826-10829), which keeps its own depth.
        let (win_part, win_rd, win_evals) = best?;
        if win_part == PartitionType::None {
            let sq = sq_info.expect("SQ info for PART_N winner");
            commit_leaf(
                self.fx,
                self.y_recon,
                self.y_stride,
                &sq.ev,
                PartitionType::None as u8,
            );
            let decision = crate::partition::funnel_block_decision(sq.ev.into_choice(), size, size);
            return Some(NodeRes {
                rd: win_rd,
                tree: PartitionTree::Leaf(decision),
            });
        }
        let mut child_trees: Vec<PartitionTree> = Vec::with_capacity(win_evals.len());
        for ev in win_evals {
            commit_leaf(self.fx, self.y_recon, self.y_stride, &ev, win_part as u8);
            let (ew, eh) = (ev.w, ev.h);
            let d = crate::partition::funnel_block_decision(ev.into_choice(), ew, eh);
            child_trees.push(PartitionTree::Leaf(d));
        }
        Some(NodeRes {
            rd: win_rd,
            tree: PartitionTree::Split {
                partition_type: win_part,
                width: size as u16,
                height: size as u16,
                children: child_trees,
            },
        })
    }

    /// C `test_split_partition` (product_coding_loop.c:11304).
    pub(super) fn test_split(
        &mut self,
        scan: &RefScan,
        abs_x: usize,
        abs_y: usize,
        parent_rd: Option<u64>,
    ) -> SplitOut {
        let size = scan.sq;
        let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);
        let (has_rows, has_cols) = edge_flags(abs_x, abs_y, size, self.aligned_w, self.aligned_h);
        // use_accurate_part_ctx = 1: no x2 bias.
        // The SPLIT rate is the boundary BINARY alphabet's at a one-false node
        // and 0 at a both-false node — C calls the same
        // `svt_aom_partition_rate_cost` here as everywhere else
        // (product_coding_loop.c:10784-10791). Identical to `bits` on a
        // 64-aligned frame.
        let split_rate =
            self.part_rates
                .bits_edge(ctx_row, PartitionType::Split, has_rows, has_cols);
        let mut split_cost = rdcost(self.lambda, split_rate, 0);

        let half = size / 2;
        let children = scan.children.as_ref().expect("split_flag children");
        let mut trees: Vec<PartitionTree> = Vec::with_capacity(4);
        let mut child_rd = [0u64; 4]; // NSQDBG only: per-quadrant pick() RD
        for (i, child) in children.iter().enumerate() {
            let cx = abs_x + (i & 1) * half;
            let cy = abs_y + (i >> 1) * half;
            // C `test_split_partition` (product_coding_loop.c:10802-10808):
            // "if block fully outside pic, don't process" — the quadrant is
            // skipped BEFORE the early-exit compare, so the next in-frame
            // quadrant still sees its own `i` (and hence the 1000 threshold).
            // Never taken on a 64-aligned frame.
            if cx >= self.aligned_w || cy >= self.aligned_h {
                continue;
            }
            // Per-quadrant early exit vs the parent depth cost
            // (product_coding_loop.c:10808-10818): the thresholds are the
            // arm-stamped `depth_early_exit_ctrls` — `split_cost_th` at
            // quadrant 0, `early_exit_th` after, a 0 ctrl read as 1000 —
            // times the arm-stamped `parent_cost_bias`. The video arm's
            // level-2 `early_exit_th` 900 aborts a split whose quadrants
            // have already accumulated ~90 % of the parent's leaf rd,
            // where level 1's 1000 waits for ~99.5 % — the
            // `fourpeople 128x128 q55 p7` mi(16,16) key-frame fork.
            if let Some(prd) = parent_rd {
                let (th0, thn, bias) = self.early_exit();
                let th = if i == 0 { th0 } else { thn };
                if (prd as u128) * th * bias <= (split_cost as u128) * 1_000_000 {
                    #[cfg(feature = "std")]
                    if nsqdbg_here(abs_x, abs_y) {
                        eprintln!(
                            "NSQDBG TSX mi=({},{}) bsize={} i={} parent={} split={}",
                            abs_y / 4,
                            abs_x / 4,
                            c_bsize_sq(size),
                            i,
                            prd,
                            split_cost,
                        );
                    }
                    return SplitOut::Invalid;
                }
            }
            // C: `if (!valid_split_partition) return false;` — every quadrant
            // must produce a valid partition for SPLIT to be selectable
            // (product_coding_loop.c:10825-10829).
            let Some(res) = self.pick(child, cx, cy) else {
                return SplitOut::Invalid;
            };
            child_rd[i] = res.rd;
            split_cost += res.rd;
            trees.push(res.tree);
        }

        // Final compare (:11375): parent wins on
        // bias * parent_rd <= split_cost * 1000.
        let (_, _, bias) = self.early_exit();
        #[cfg(feature = "std")]
        if nsqdbg_here(abs_x, abs_y) {
            let chose = match parent_rd {
                Some(prd) if bias * (prd as u128) <= (split_cost as u128) * 1000 => "parent",
                _ => "split",
            };
            eprintln!(
                "NSQDBG TS mi=({},{}) bsize={} parent_valid={} parent={} split={} sr={} c=[{},{},{},{}] chose={}",
                abs_y / 4,
                abs_x / 4,
                c_bsize_sq(size),
                u8::from(parent_rd.is_some()),
                parent_rd.unwrap_or(0),
                split_cost,
                split_rate,
                child_rd[0],
                child_rd[1],
                child_rd[2],
                child_rd[3],
                chose,
            );
        }
        if let Some(prd) = parent_rd
            && bias * (prd as u128) <= (split_cost as u128) * 1000
        {
            return SplitOut::ParentKept;
        }
        SplitOut::Chosen(Box::new(NodeRes {
            rd: split_cost,
            tree: PartitionTree::Split {
                partition_type: PartitionType::Split,
                width: size as u16,
                height: size as u16,
                children: trees,
            },
        }))
    }
}
