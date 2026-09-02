//! Number-of-InterCandidates staging: which candidates survive each MD stage.
//!
//! C keeps candidates in a fixed-size replacement POOL per candidate class
//! (`md_stage_0`, product_coding_loop.c:9342), sorts each class with its own
//! exchange sort, and then runs a chain of deviation-threshold prunes --
//! `post_mds0_nic_pruning` (:7819), `post_mds1_nic_pruning` (:7885),
//! `post_mds2_nic_pruning` (:7961) -- each relative to that class's OWN best
//! cost, never the global one. Getting that per-class scoping wrong is what
//! let palette candidates prune out every regular mode on screen content
//! (EPICA p6: 2064 palette blocks vs C's 178); see ba58a3ec2 / 765d60a7e.
//!
//! Both stages here are PURE over the candidate list -- stage 1 reads only
//! `cli_qp` besides the candidates and the config, stage 2 reads nothing
//! external at all. That is why they are the cleanest seam in `evaluate_leaf`
//! and the first phase to come out of it.

use super::*;

/// What the MDS0 -> MDS1 staging hands to the MDS1 full loop, and on to
/// [`stage_mds1_to_mds3`].
pub(super) struct Mds1Staging {
    /// Surviving candidate indices, class-CONCATENATED in class order
    /// (C0 regular, C3 palette, C4 IntraBC). NOT cost-merged across classes:
    /// C never merges them, and the winner scan's strict `<` breaks
    /// cross-class ties toward the earlier class.
    pub(super) order: Vec<usize>,
    /// Per-class segment lengths of `order` in C's class order — C0
    /// (regular intra), C1 (inter), C3 (palette), C4 (IntraBC) — or `None`
    /// on the
    /// single-class fast path.
    pub(super) seg: Option<[usize; LANES]>,
    /// C `mds0_best` -- strict `<` over the per-class sorted heads.
    pub(super) mds0_best_idx: usize,
    /// qp-scaled MDS2/MDS3 stage counts, needed by the later prunes.
    pub(super) nic2: u32,
    pub(super) nic3: u32,
    /// C `svt_aom_get_qp_based_th_scaling_factors` numerator / denominator.
    pub(super) qw: u64,
    pub(super) qwd: u64,
}

/// C `md_stage_0`'s replacement pool + `sort_fast_cost_based_candidates` +
/// `post_mds0_nic_pruning`, per candidate class.
// Every division here is guarded by a `best > 0` / `global_best != 0` test that
// scopes a whole block, not one expression, so `checked_div` cannot express it
// without restructuring hot RD control flow. `clippy::manual_checked_ops`
// post-dates the 1.89 MSRV floor's clippy, so the allow must tolerate being
// unknown there (`cargo +1.89 clippy` otherwise reports `unknown lint` here).
#[allow(unknown_lints, clippy::manual_checked_ops)]
pub(super) fn stage_mds0_to_mds1(cands: &[Cand], cfg: FunnelCfg, cli_qp: u32) -> Mds1Staging {
    let ncand = cands.len();

    // -- MDS0 -> MDS1 MEMBERSHIP: C's replacement POOL, not a sort. --
    // md_stage_0 keeps candidates in max_buffers = md_stage_1_count + 1
    // slots (product_coding_loop.c:9342): the first max_buffers candidates
    // fill slots in PROCESSING order; every later candidate OVERWRITES the
    // current worst slot, where the victim scan is a FIRST-argmax with
    // strict `>` (:1692-1699) — so when two candidates TIE on fast cost at
    // the pool boundary, the EARLIER-processed one is the victim and the
    // LATER-processed one survives. After the last candidate the current
    // victim is discarded (cost set to MAX, :1708). A stable
    // sort + take(n1) keeps the EARLIER tied candidate instead — one
    // swapped survivor flips the whole SB downstream (1624307 q32 p2
    // mi(66,108): (mode5,d-1) vs (mode5,d+3) tied at fast 19175060; C
    // carries d+3, the sort carried d-1, the mds3 uv table then lost its
    // uv=2 row and tbl[SMOOTH] flipped H->SMOOTH).
    // NOTE: ties BETWEEN adjacent same-mode deltas share our injection
    // order with C; cross-mode/cross-iteration ties additionally depend on
    // C's two-iteration MDS0 order (regulars, then angular+fi, :1600) —
    // refine if a cell ever demands it.
    let (nic1, nic2, nic3) = nic_counts(cli_qp, cfg.nic_num);
    // C runs md_stage_0's replacement pool PER CANDIDATE CLASS
    // (svt_aom_set_nics gives each class its own mds1_count, product_
    // coding_loop.c:1358; the pool + argmax-victim loop runs once per
    // cand_class_it, :9330-9360). On the allintra I-slice only two intra
    // classes are live: CAND_CLASS_0 (regular + fi intra) and
    // CAND_CLASS_3 (palette), and MD_STAGE_NICS gives BOTH base 64
    // (definitions.h:811), so each lane keeps up to `nic1` survivors and
    // MDS1/MDS3 evaluate the UNION (construct_best_sorted_arrays_md_
    // stage_3, :1455). A single shared pool let palette candidates
    // (huge SATD advantage on screen content) flood out the regular
    // survivors — EPICA p6 coded 2064 palette blocks vs C's 178. The
    // per-class dist-to-cost prune (product_coding_loop.c:1309) is INERT
    // here: allintra mds0_level == 0 (enc_mode_config.c:10042) sets
    // pruning_method_th = 0, so no class-th cut runs.
    let lane_pool = |lane: &[usize], cands: &[Cand], cap: usize| -> Vec<usize> {
        if lane.len() < cap {
            return lane.to_vec();
        }
        let argmax_first = |pool: &[usize]| -> usize {
            let mut vi = 0usize;
            let mut vc = cands[pool[0]].fast_cost;
            for (i, &ci) in pool.iter().enumerate().skip(1) {
                if cands[ci].fast_cost > vc {
                    vi = i;
                    vc = cands[ci].fast_cost;
                }
            }
            vi
        };
        let mut pool: Vec<usize> = Vec::with_capacity(cap);
        let mut victim = 0usize;
        for &ci in lane {
            if pool.len() < cap {
                pool.push(ci);
                if pool.len() == cap {
                    victim = argmax_first(&pool);
                }
            } else {
                pool[victim] = ci;
                victim = argmax_first(&pool);
            }
        }
        if pool.len() == cap {
            pool.remove(victim);
        }
        pool
    };
    // Class-partition preserving injection (processing) order within each
    // lane — the argmax-victim tie rule depends on it (the MDS0 pool
    // fix, 1624307). Regular (C0) then palette (C3), matching C's class
    // iteration order in construct_best_sorted_arrays.
    let has_palette_lane = cands.iter().any(|c| c.palette.is_some());

    // -- post_mds0_nic_pruning (product_coding_loop.c:7819) --
    let (qw, qwd) = qp_scale_factors(cli_qp);
    // nic_level 1 (M0) sets mds1_cand_base_th_intra = (uint64_t)~0 (no mds1
    // cand pruning); the qp-scaled threshold stays saturated so the loop
    // below never prunes (guard avoids the base*qw overflow).
    let mds1_cand_th = if cfg.mds1_cand_base_th == u64::MAX {
        u64::MAX
    } else {
        div_round(cfg.mds1_cand_base_th * qw, qwd)
    };
    // C runs the intra dev-threshold prune PER CLASS (`for cidx`, :7840),
    // each relative to that class's OWN best fast cost (`cand_buff[cidx]
    // [0]`, :7845/:7868) — never the global best. The inter-class
    // (class_th) block :7847-7862 is inert on the I-slice: mds1_class_th
    // == ~0 (:7826) forces band_idx 0 (:7859), so no class is zeroed or
    // band-reduced. Running this prune over the sorted UNION with the
    // global best (as a single shared pool did) let palette — whose
    // screen-content fast cost sits far below any regular mode — prune
    // out every regular candidate (EPICA p6: 2064 palette blocks vs C's
    // 178, and every port-only block's ONLY MDS1 survivors were palette).
    // Prune each lane against its own class-best, then union + sort.
    let dev_prune = |sorted: &[usize], cands: &[Cand]| -> usize {
        if sorted.is_empty() {
            return 0;
        }
        let best = cands[sorted[0]].fast_cost;
        let mut count = 1usize;
        if best > 0 {
            while count < sorted.len() {
                let dev = (cands[sorted[count]].fast_cost - best) * 100 / best;
                // C: `mds1_cand_th / (rank ? rank * cand_count : 1)`
                // (product_coding_loop.c:7869) — rank 0 (M4 nic case 5)
                // means the raw threshold, NOT a zero divisor.
                let div = if cfg.mds1_rank_factor != 0 {
                    cfg.mds1_rank_factor * count as u64
                } else {
                    1
                };
                if dev >= mds1_cand_th / div {
                    break;
                }
                count += 1;
            }
        }
        count
    };
    // C `sort_fast_cost_based_candidates` (product_coding_loop.c:1415) over
    // each class's surviving pool. MUST be the C exchange sort, not a stable
    // sort: on exact fast-cost ties the two differ (see [`c_exchange_sort_by`]),
    // and the pool arrangement entering it is C's buffer arrangement
    // (lane_pool), so the tie order here is the one C's MDS1 walks.
    let sort_lane = |mut lane: Vec<usize>, cands: &[Cand]| -> Vec<usize> {
        c_exchange_sort_by(&mut lane, |i| cands[i].fast_cost);
        lane
    };
    // IBC chunk 8: C classes IntraBC CAND_CLASS_4 (mode_decision.c:3659)
    // — its own MDS0 pool + per-class prunes, exactly like palette's C3.
    // The class NIC bases are all 64 on I-slices (MD_STAGE_NICS,
    // definitions.h:811-813: {64, 0, 0, 64, 64}) so every lane shares the
    // same `cap` derivation; with <= 2 IBC candidates the C4 pool never
    // overflows in practice. Union order = class order (C0, C3, C4 —
    // construct_best_sorted_arrays), stable-sorted by fast cost.
    let has_ibc_lane = cands.iter().any(|c| c.ibc.is_some());
    // The inter lanes are C's own classes 1 and 2 (`lane_of`). They must NOT
    // share lane 0 with the intra modes: the per-class dev-prune below
    // measures each candidate against its OWN class's best, and an inter
    // candidate on a well-predicted block has a fast cost far below any
    // intra mode's — the exact shape that let palette prune out every
    // regular mode before the per-class lanes landed (see the EPICA note
    // above).
    let has_inter_lane = cands.iter().any(|c| c.inter.is_some());
    // Multi-lane: `seg` carries the per-class segment lengths (k0, k3, k4)
    // of the CLASS-CONCATENATED `order` — C's cand_buff_indices structure.
    // C never merges the classes into one cost-sorted list: MDS1 evaluates
    // each class's own fast-sorted survivors (md_stage_1 per target_class),
    // and every later union (construct_best_sorted_arrays_md_stage_3,
    // :1454) is a pure concatenation in class order C0, C3, C4. The
    // previous union `sort_by_key(fast_cost)` matched C on all DISTINCT
    // costs but flipped cross-class tie/order corners (winner-scan ties,
    // uv_list order, mds1-best identity) — the screen multi-lane pins.
    let (order, seg): (Vec<usize>, Option<[usize; LANES]>) =
        if has_palette_lane || has_ibc_lane || has_inter_lane {
            let cap = (ncand as u32).min(nic1).max(1) as usize + 1;
            let lanes: [Vec<usize>; LANES] =
                core::array::from_fn(|l| (0..ncand).filter(|&i| lane_of(&cands[i]) == l).collect());
            // Per-class MDS0 replacement pool -> sort -> per-class dev-prune.
            let sorted: [Vec<usize>; LANES] =
                lanes.map(|l| sort_lane(lane_pool(&l, cands, cap), cands));
            let k: [usize; LANES] = core::array::from_fn(|l| dev_prune(&sorted[l], cands));
            // MDS1 evaluates the per-class survivors, class-concatenated in
            // class order (C0..C4) — NOT cost-merged.
            let mut u: Vec<usize> = Vec::new();
            for l in 0..LANES {
                u.extend_from_slice(&sorted[l][..k[l]]);
            }
            (u, Some(k))
        } else {
            // Single-class fast path (no palette candidates) — byte-identical
            // to the prior single-pool behaviour: pool -> sort -> dev-prune.
            let cap = (ncand as u32).min(nic1) as usize + 1;
            let all: Vec<usize> = (0..ncand).collect();
            let s = sort_lane(lane_pool(&all, cands, cap), cands);
            let k = dev_prune(&s, cands);
            (s[..k].to_vec(), None)
        };
    // C mds0_best (:9518-9524): strict `<` over the per-class sorted heads
    // in class order (the head survives every dev-prune, count >= 1). On
    // the single-class path this is order[0]; on the multi-lane concat it
    // must be scanned (the concat head is C0's head, not the global min).
    let mds0_best_idx = match seg {
        Some(k) => {
            let mut bi = order[0];
            let mut bc = u64::MAX;
            let mut off = 0usize;
            for len in k {
                if let Some(head) = order.get(off)
                    && len > 0
                    && cands[*head].fast_cost < bc
                {
                    bc = cands[*head].fast_cost;
                    bi = *head;
                }
                off += len;
            }
            bi
        }
        None => order[0],
    };
    Mds1Staging {
        order,
        seg,
        mds0_best_idx,
        nic2,
        nic3,
        qw,
        qwd,
    }
}

/// C's five candidate CLASSES (`CandClass`, definitions.h:787-794), one lane
/// each, in class order. The lane index IS the class value.
pub(super) const LANES: usize = 5;

/// C's candidate class for one funnel candidate
/// (`mode_decision.c:3646-3672`, the loop that assigns `cand->cand_class`):
///
/// | class | C's own comment | this port |
/// |---|---|---|
/// | 0 | intra, no palette, no intrabc | the regular intra lane |
/// | 1 | "MVP Prediction" — every inter mode EXCEPT `NEWMV` / `NEW_NEWMV` | `NEARESTMV` |
/// | 2 | "MV Prediction" — `NEWMV`, `NEW_NEWMV`, and everything when `merge_inter_cands` | `NEWMV` |
/// | 3 | palette | palette |
/// | 4 | IntraBC | IntraBC |
///
/// **`merge_inter_cands` is NOT ported** (`mode_decision.c:3637-3643`): when
/// `nic_ctrls.pruning_ctrls.merge_inter_cands_mult != ~0` and
/// `min(md_me_dist, md_pme_dist) / (bw * bh)` is under its threshold, C puts
/// EVERY inter candidate in class 2. Both distortions are written by
/// `read_refine_me_mvs`, which this port does not have. It can only MERGE
/// classes, so its absence is a class-IDENTITY difference in the
/// rank-staging `+3` arm and in the cross-class tie order — never a missing
/// candidate.
pub(super) fn lane_of(c: &Cand) -> usize {
    match c.inter.as_deref() {
        Some(i) => {
            if matches!(
                i.mode,
                svtav1_types::prediction::PredictionMode::NewMv
                    | svtav1_types::prediction::PredictionMode::NewNewMv
            ) {
                2
            } else {
                1
            }
        }
        None if c.palette.is_some() => 3,
        None if c.ibc.is_some() => 4,
        None => 0,
    }
}

/// The C `CandClass` VALUE for a lane index — what the rank-staging compare
/// tests for equality. Identity now that all five lanes exist; kept as a
/// function so the two concepts stay separable if a class is ever dropped.
pub(super) fn class_value(lane: usize) -> u8 {
    lane as u8
}

/// What the MDS1 -> MDS3 staging hands to the MDS3 full loop.
pub(super) struct Mds3Staging {
    /// Per-class full-cost-sorted survivors, class-concatenated (see
    /// [`Mds1Staging::order`] for why the concatenation order is load-bearing).
    pub(super) order1: Vec<usize>,
    /// How many of `order1` MDS3 evaluates.
    pub(super) n3: usize,
}

/// C `sort_full_cost_based_candidates` + `post_mds1_nic_pruning` +
/// `post_mds2_nic_pruning`, per candidate class.
///
/// Reads the MDS1 full costs the caller has just written into `cands`.
// Every division here is guarded by a `best > 0` / `global_best != 0` test that
// scopes a whole block, not one expression, so `checked_div` cannot express it
// without restructuring hot RD control flow. `clippy::manual_checked_ops`
// post-dates the 1.89 MSRV floor's clippy, so the allow must tolerate being
// unknown there (`cargo +1.89 clippy` otherwise reports `unknown lint` here).
#[allow(unknown_lints, clippy::manual_checked_ops)]
pub(super) fn stage_mds1_to_mds3(cands: &[Cand], cfg: FunnelCfg, st: &Mds1Staging) -> Mds3Staging {
    let order = &st.order;
    let seg = st.seg;
    let mds0_best_idx = st.mds0_best_idx;
    let (nic2, nic3, qw, qwd) = (st.nic2, st.nic3, st.qw, st.qwd);
    let n1 = order.len();
    // -- Sort survivors by full cost --
    // C `sort_full_cost_based_candidates` (product_coding_loop.c:1438, the
    // post-MDS1 :9561 sort). Same exchange-sort tie semantics as the fast
    // sort: on an exact full-cost TIE the survivor set into MDS3 depends on
    // it. Measured on clic 8426ed... 512^2 bd10 p6 q5, blk (472,208) 8x8:
    // MDS1 costs {DC+fi 2709194, SMOOTH 2710447, DC 2710447} in fast order
    // [SMOOTH, DC, DC+fi] — C's i=0/j=2 swap moves SMOOTH BEHIND the tied
    // DC, so C's MDS3 pair is {DC+fi, DC} while a stable sort keeps SMOOTH
    // -> the port coded SMOOTH and desynced the whole tail of the frame
    // (305 tree flips downstream of one tie).
    // Multi-lane: C sorts PER CLASS (`sort_full_cost_based_candidates(ctx,
    // md_stage_1_count[cidx], cand_buff_indices[cidx])` inside the per-class
    // MDS1 loop, :9560-9564) — never across the union. The class segments
    // stay contiguous; the mds1 best is the strict-`<` scan over the class
    // heads in class order (:9565-9569) — on a cross-class exact full-cost
    // tie the EARLIER class keeps the best (identity feeds the rank-staging
    // `mds0_best_idx == mds1_best_idx` compare and the class +3 arm).
    let mut order1: Vec<usize> = order[..n1].to_vec();
    let mds1_best_idx = match seg {
        Some(k) => {
            let mut off = 0usize;
            for len in k {
                c_exchange_sort_by(&mut order1[off..off + len], |i| cands[i].full_cost);
                off += len;
            }
            let mut bi = order1[0];
            let mut bc = u64::MAX;
            let mut off = 0usize;
            for len in k {
                if let Some(head) = order1.get(off)
                    && len > 0
                    && cands[*head].full_cost < bc
                {
                    bc = cands[*head].full_cost;
                    bi = *head;
                }
                off += len;
            }
            bi
        }
        None => {
            c_exchange_sort_by(&mut order1, |i| cands[i].full_cost);
            order1[0]
        }
    };

    // -- post_mds1_nic_pruning (:7885) + post_mds2_nic_pruning (:7961) --
    // BOTH run PER CANDIDATE CLASS in C (`for cidx`, :7903/:7969), each
    // dev-threshold relative to that class's OWN best full_cost
    // (cand_buff[cidx][0]). Running them over the sorted UNION with the
    // global best (as the single block below did) prunes the regular
    // (DC/dir) candidates out before MDS3 whenever a palette candidate's
    // lower full cost sets `best` — the MDS1/MDS3 sibling of the MDS0
    // dev-prune fix (ba58a3ec2). Without this DC never reaches MDS3, so
    // palette wins by default even though C's DC MDS3 (residual coded)
    // beats it. The post_mds1 inter-class (mds2_class_th) block IS inert on
    // the I-slice (forced ~0, :7897) — but the post_mds2 inter-class
    // (mds3_class_th) block is NOT (:7978-7979 re-floors it to
    // MAX(25, scaled*mult) for I_SLICE); that one is applied per lane below
    // (the #71 palette under-pick root: it zeroes the regular class when its
    // best cost deviates too far from the palette global best). Only the
    // palette (multi-class) path takes the per-lane branch; the single-class
    // path is byte-identical to before (best == global best => inert).
    let mds2_cand_th = div_round(cfg.mds2_cand_base_th * qw, qwd);
    let mds3_cand_th = div_round(cfg.mds3_cand_base_th * qw, qwd);
    // Inter-class MDS3 threshold (post_mds2_nic_pruning, :7975-7979). This
    // funnel is always the allintra KEY (I_SLICE), so the I-slice re-floor
    // MAX(25, scaled*i_mds3_class_th_mult) always applies. u64::MAX == the
    // `(uint64_t)~0` disabled sentinel (never set on palette-active presets).
    let mds3_class_th = if cfg.mds3_class_th == u64::MAX {
        u64::MAX
    } else {
        25u64.max(div_round(cfg.mds3_class_th * qw, qwd) * cfg.i_mds3_class_th_mult)
    };
    // C `best_md_stage_cost` at post_mds2: MDS2 is bypassed on this funnel
    // (no MD_STAGE_2 full loop), so it stays the MDS1 GLOBAL best
    // (product_coding_loop.c:9580-9585) — the overall cheapest MDS1 full cost.
    let global_best = cands[mds1_best_idx].full_cost;
    // Class id for the rank-staging compare: C's CandClass VALUE.
    let class_of = |c: &Cand| -> u8 { class_value(lane_of(c)) };
    let n3;
    if let Some(ks) = seg {
        let mds1_best_class = class_of(&cands[mds1_best_idx]);
        // post_mds1 (n2) then post_mds2 (n3) for one class lane, each
        // against that lane's own best. Returns the post_mds2 survivor
        // count. `cands`/`cfg`/thresholds captured by ref; no `order1`
        // capture (lanes are copied index lists).
        let prune_lane = |lane: &[usize]| -> usize {
            if lane.is_empty() {
                return 0;
            }
            let best = cands[lane[0]].full_cost;
            // post_mds1 -> n2
            let mut n2 = lane.len().min(nic2 as usize);
            if best > 0 && 1 < n2 {
                // C rank staging (:7934-7939): +3 when this lane is NOT
                // the MDS1-best class, else +2 when the MDS0 and MDS1
                // winners coincide (only if the base factor is nonzero).
                let lane_class = class_of(&cands[lane[0]]);
                let mut rank_factor = cfg.mds2_rank_factor;
                if rank_factor != 0 {
                    if lane_class != mds1_best_class {
                        rank_factor += 3;
                    } else if mds0_best_idx == mds1_best_idx {
                        rank_factor += 2;
                    }
                }
                let mut count = 1usize;
                let mut prev_dev = (cands[lane[count]].full_cost - best) * 100 / best;
                let mut dev = prev_dev;
                while (cfg.mds2_rel_dev_th == 0 || dev <= prev_dev + cfg.mds2_rel_dev_th)
                    && dev
                        < mds2_cand_th
                            / (if rank_factor != 0 {
                                rank_factor * count as u64
                            } else {
                                1
                            })
                {
                    count += 1;
                    if count >= n2 {
                        break;
                    }
                    prev_dev = dev;
                    dev = (cands[lane[count]].full_cost - best) * 100 / best;
                }
                n2 = count;
            }
            // post_mds2 -> n3. C: md_stage_3_count = min(md_stage_2_count,
            // nic3_base) (product_coding_loop.c:9589), then post_mds2 prunes.
            let mut n3l = n2.min(nic3 as usize);
            if n3l == 0 {
                return 0; // C guard :7986 md_stage_3_count[cidx] > 0
            }
            // INTER-CLASS prune (:7993-8008): zero a class whose best full
            // cost deviates >= mds3_class_th% from the GLOBAL best (`continue`
            // skips its intra prune), else band-reduce the count. `best` is
            // this lane's best; on the single-class path best == global_best
            // so this whole block is skipped (byte-inert). The zeroing arm is
            // the #71 fix: the regular lane (best 455607) vs the palette
            // global best (295193) gives dev 54 >= 50 at q5/p6, dropping DC
            // from MDS3 so palette (the C winner) is no longer beaten.
            if mds3_class_th != u64::MAX && best != 0 && global_best != 0 && best != global_best {
                if mds3_class_th == 0 {
                    return 0; // C :7994-7996 md_stage_3_count=0; continue
                }
                let dev = (best - global_best) * 100 / global_best;
                if dev != 0 {
                    if dev >= mds3_class_th {
                        return 0; // C :8000-8002 md_stage_3_count=0; continue
                    }
                    if cfg.mds3_band_cnt >= 3 && n3l > 1 {
                        // C :8004-8007 band reduce (DIVIDE_AND_ROUND).
                        let band_idx = dev * (cfg.mds3_band_cnt as u64 - 1) / mds3_class_th;
                        n3l = div_round(n3l as u64, band_idx + 1) as usize;
                    }
                }
            }
            // INTRA-CLASS prune (mds3_cand_th, :8011-8019): C floors cand_count
            // at 1, so a band-reduced 0 is lifted back to 1 here (only the
            // inter-class `continue` above yields a true 0).
            if best > 0 {
                let mut count = 1usize;
                while count < n3l {
                    let dev = (cands[lane[count]].full_cost - best) * 100 / best;
                    if dev >= mds3_cand_th {
                        break;
                    }
                    count += 1;
                }
                n3l = count;
            }
            n3l
        };
        // The class segments are contiguous in `order1` (per-class sorted
        // above) — C's cand_buff_indices[cidx] arrays.
        let mut off = 0usize;
        let lanes: [Vec<usize>; LANES] = core::array::from_fn(|l| {
            let v = order1[off..off + ks[l]].to_vec();
            off += ks[l];
            v
        });
        let kept: [usize; LANES] = core::array::from_fn(|l| prune_lane(&lanes[l]));
        // MDS3 evaluates the class-CONCATENATED survivors in class order —
        // C `construct_best_sorted_arrays_md_stage_3` (:1454) does NOT
        // re-sort the union; the winner scan's strict-`<` therefore breaks
        // cross-class full-cost ties toward the earlier class (C0 intra
        // beats palette/IBC on an exact tie), and the ind-uv uv_list /
        // MDS3 evaluation order follow the same concatenation.
        let mut u: Vec<usize> = Vec::new();
        for l in 0..LANES {
            u.extend_from_slice(&lanes[l][..kept[l]]);
        }
        n3 = u.len();
        order1 = u;
    } else {
        // Single-class fast path — byte-identical to the prior union prune.
        let mut n2 = (n1 as u32).min(nic2) as usize;
        {
            let best = cands[order1[0]].full_cost;
            let mut count = 1usize;
            if best > 0 && count < n2 {
                // C rank staging (product_coding_loop.c:8158-8166): only
                // when the config factor is nonzero — same class (the
                // inter-class +3 arm is dead: single intra class == the
                // mds1 best class), +2 when MDS0 and MDS1 winners coincide.
                let mut rank_factor = cfg.mds2_rank_factor;
                if rank_factor != 0 && mds0_best_idx == mds1_best_idx {
                    rank_factor += 2;
                }
                let mut prev_dev = (cands[order1[count]].full_cost - best) * 100 / best;
                let mut dev = prev_dev;
                while (cfg.mds2_rel_dev_th == 0 || dev <= prev_dev + cfg.mds2_rel_dev_th)
                    && dev
                        < mds2_cand_th
                            / (if rank_factor != 0 {
                                rank_factor * count as u64
                            } else {
                                1
                            })
                {
                    count += 1;
                    if count >= n2 {
                        break;
                    }
                    prev_dev = dev;
                    dev = (cands[order1[count]].full_cost - best) * 100 / best;
                }
                n2 = count;
            }
        }
        let mut n3v = (n2 as u32).min(nic3) as usize;
        {
            let best = cands[order1[0]].full_cost;
            let mut count = 1usize;
            if best > 0 {
                while count < n3v {
                    let dev = (cands[order1[count]].full_cost - best) * 100 / best;
                    if dev >= mds3_cand_th {
                        break;
                    }
                    count += 1;
                }
                n3v = count;
            }
        }
        n3 = n3v;
    }
    Mds3Staging { order1, n3 }
}
