use super::*;
use crate::port_md::pme::{MvCostTable, MvCostType};

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

fn zero_cost() -> MvCostTable {
    MvCostTable::zeroed()
}

fn params(t: &MvCostTable) -> MvCostParams<'_> {
    MvCostParams {
        ref_mv: Mv::ZERO,
        mv_cost_type: MvCostType::None,
        tables: Some(t),
        error_per_bit: 0,
        early_exit_th: 0,
    }
}

/// A distortion that is a pure function of the reference index, so a
/// test can state exactly which positions were visited and which one
/// won without needing pixels.
struct Probe {
    /// Every `ref_origin_index` the search asked about, in order.
    visited: Vec<i32>,
    /// The index whose cost is 0; everything else costs 1000.
    target: i32,
    /// Feeds `subpel_variance` / `variance_vs_flat`.
    subpel: Vec<((i32, i32, i32), u32)>,
    flat: u32,
}

impl Probe {
    fn new(target: i32) -> Self {
        Self {
            visited: Vec::new(),
            target,
            subpel: Vec::new(),
            flat: 0,
        }
    }
    fn cost(&mut self, idx: i32) -> u32 {
        self.visited.push(idx);
        if idx == self.target { 0 } else { 1000 }
    }
}

impl PmeSadLoop for Probe {
    fn run_pme_sad_loop(
        &mut self,
        _p: &MvCostParams<'_>,
        _input_origin_index: usize,
        ref_origin_index: i32,
        search_area_width: i32,
        search_area_height: i32,
        _start_x: i32,
        _start_y: i32,
        _search_step: i32,
        _mvx: i16,
        _mvy: i16,
        _best: &mut PmeBest,
    ) {
        // Record that the 8-aligned half ran, and with what extent.
        self.visited.push(-1_000_000 - ref_origin_index);
        self.visited.push(search_area_width);
        self.visited.push(search_area_height);
    }
}

impl DistortionSource for Probe {
    fn variance(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
        self.cost(ref_origin_index)
    }
    fn subpel_variance(&mut self, idx: i32, subx: i32, suby: i32, _i: usize) -> u32 {
        self.visited.push(idx);
        for &(k, v) in &self.subpel {
            if k == (idx, subx, suby) {
                return v;
            }
        }
        1000
    }
    fn sad(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
        self.cost(ref_origin_index)
    }
    fn ssd(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
        self.cost(ref_origin_index)
    }
    fn variance_vs_flat(&mut self, _idx: i32) -> u32 {
        self.flat
    }
}

fn fp_ctx() -> FullPelCtx {
    FullPelCtx {
        blk_org_x: 32,
        blk_org_y: 32,
        bwidth: 16,
        bheight: 16,
        enable_psad: false,
        hbd_md: false,
        sprs_lev0_start_x: 0,
        sprs_lev0_end_x: 0,
        sprs_lev0_start_y: 0,
        sprs_lev0_end_y: 0,
    }
}

fn geom() -> RefPicGeom {
    RefPicGeom {
        border: 64,
        max_width: 320,
        max_height: 240,
        y_stride: 448,
    }
}

/// TIER 4 — the search-area clamp is asymmetric: the start sides drop
/// only the origin plus the MV, the end sides also drop the block
/// dimension.
#[test]
fn tier4_clamp_search_window_is_asymmetric() {
    let ctx = fp_ctx();
    let r = geom();
    let mut w = SearchWindow {
        start_x: -1000,
        end_x: 1000,
        start_y: -1000,
        end_y: 1000,
        sparse_search_step: 1,
        is_sprs_lev0_performed: false,
    };
    clamp_search_window(&ctx, &r, 0, 0, &mut w);
    assert_eq!(w.start_x, (-r.border + 1) - ctx.blk_org_x);
    assert_eq!(
        w.end_x,
        (r.border + r.max_width - 1) - (ctx.blk_org_x + ctx.bwidth)
    );
    assert_eq!(w.start_y, (-r.border + 1) - ctx.blk_org_y);
    assert_eq!(
        w.end_y,
        (r.border + r.max_height - 1) - (ctx.blk_org_y + ctx.bheight)
    );

    // The MV shifts the whole window, in FULL-pel units (>> 3).
    let mut w2 = SearchWindow {
        start_x: -1000,
        end_x: 0,
        start_y: 0,
        end_y: 0,
        sparse_search_step: 1,
        is_sprs_lev0_performed: false,
    };
    clamp_search_window(&ctx, &r, 80, 0, &mut w2);
    assert_eq!(w2.start_x, (-r.border + 1) - (ctx.blk_org_x + 10));

    // A window already inside is untouched.
    let mut w3 = SearchWindow {
        start_x: -2,
        end_x: 2,
        start_y: -2,
        end_y: 2,
        sparse_search_step: 1,
        is_sprs_lev0_performed: false,
    };
    clamp_search_window(&ctx, &r, 0, 0, &mut w3);
    assert_eq!((w3.start_x, w3.end_x, w3.start_y, w3.end_y), (-2, 2, -2, 2));
}

/// TIER 4 — the scan order is x-OUTER, y-INNER, and both bounds are
/// INCLUSIVE.
#[test]
fn tier4_md_full_pel_search_scan_order_and_inclusive_bounds() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Sad,
        0,
        0,
        SearchWindow {
            start_x: -1,
            end_x: 1,
            start_y: -1,
            end_y: 1,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        },
        &mut best,
    );
    // 3x3 inclusive = 9 positions.
    assert_eq!(probe.visited.len(), 9);
    let base = ctx.blk_org_x + ctx.blk_org_y * r.y_stride as i32;
    let at = |dx: i32, dy: i32| base + dx + dy * r.y_stride as i32;
    // x outer, y inner: (-1,-1), (-1,0), (-1,1), (0,-1), ...
    assert_eq!(probe.visited[0], at(-1, -1));
    assert_eq!(probe.visited[1], at(-1, 0));
    assert_eq!(probe.visited[2], at(-1, 1));
    assert_eq!(probe.visited[3], at(0, -1));
}

/// TIER 4 — the winning MV is the search offset x8 added to the
/// centre, and a tie keeps the EARLIER position (`<`, not `<=`).
#[test]
fn tier4_md_full_pel_search_best_mv_and_tie_break() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    // Centre MV (16, -8) is full-pel (2, -1); the winner sits at
    // search offset (+1, +1) from that centre.
    let target = (ctx.blk_org_x + 2 + 1) + (ctx.blk_org_y - 1 + 1) * r.y_stride as i32;
    let mut probe = Probe::new(target);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Var,
        16,
        -8,
        SearchWindow {
            start_x: -1,
            end_x: 1,
            start_y: -1,
            end_y: 1,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        },
        &mut best,
    );
    // The centre MV is (16, -8) = full-pel (2, -1); the winner is at
    // offset (+1, +1) relative to that centre.
    assert_eq!(best.cost, 0);
    assert_eq!(best.mvx, 16 + 8);
    assert_eq!(best.mvy, -8 + 8);

    // All-equal costs: the FIRST position scanned wins.
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Var,
        0,
        0,
        SearchWindow {
            start_x: -1,
            end_x: 1,
            start_y: -1,
            end_y: 1,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        },
        &mut best,
    );
    assert_eq!((best.mvx, best.mvy), (-8, -8));
}

/// TIER 4 — the sparse level-1 skip fires only for
/// `sparse_search_step == 2`, inside the recorded level-0 window, and
/// only on positions that are multiples of 4 in BOTH axes.
#[test]
fn tier4_md_full_pel_search_sparse_level1_skip() {
    let mut ctx = fp_ctx();
    ctx.sprs_lev0_start_x = -8;
    ctx.sprs_lev0_end_x = 8;
    ctx.sprs_lev0_start_y = -8;
    ctx.sprs_lev0_end_y = 8;
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let w = SearchWindow {
        start_x: -4,
        end_x: 4,
        start_y: -4,
        end_y: 4,
        sparse_search_step: 2,
        is_sprs_lev0_performed: true,
    };
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Sad,
        0,
        0,
        w,
        &mut best,
    );
    // 5x5 lattice at step 2 = 25 positions; the 9 with both
    // coordinates in {-4, 0, 4} are skipped.
    assert_eq!(probe.visited.len(), 25 - 9);

    // Same window with the flag off: nothing is skipped.
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    let mut w2 = w;
    w2.is_sprs_lev0_performed = false;
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Sad,
        0,
        0,
        w2,
        &mut best,
    );
    assert_eq!(probe.visited.len(), 25);

    // Step 1 never skips, even with the flag on.
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    let mut w3 = w;
    w3.sparse_search_step = 1;
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Sad,
        0,
        0,
        w3,
        &mut best,
    );
    assert_eq!(probe.visited.len(), 81);
}

/// TIER 4 — the psad dispatch: SAD + enable_psad + 8-bit + a CLAMPED
/// width >= 7. The threshold is 7, not 8.
#[test]
fn tier4_md_full_pel_search_psad_dispatch_threshold() {
    let mut ctx = fp_ctx();
    ctx.enable_psad = true;
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let run = |w: i32, dist_type, hbd: bool| {
        let mut c = ctx;
        c.hbd_md = hbd;
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        md_full_pel_search(
            &c,
            &r,
            &mut probe,
            &p,
            0,
            dist_type,
            0,
            0,
            SearchWindow {
                start_x: 0,
                end_x: w,
                start_y: 0,
                end_y: 0,
                sparse_search_step: 1,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        // The Probe records a large negative marker when the
        // 8-aligned kernel ran.
        probe.visited.iter().any(|&v| v < -100_000)
    };
    assert!(!run(6, DistortionType::Sad, false), "width 6 stays generic");
    assert!(run(7, DistortionType::Sad, false), "width 7 takes mpsad");
    assert!(run(8, DistortionType::Sad, false));
    // Not SAD, or hbd, and it never dispatches.
    assert!(!run(16, DistortionType::Var, false));
    assert!(!run(16, DistortionType::Sad, true));
}

/// TIER 4 — the mpsad variant rounds the x extent UP to a multiple of
/// 8, so it does NOT search the same positions as the generic path.
#[test]
fn tier4_large_lbd_rounds_x_extent_up_to_a_multiple_of_eight() {
    let mut ctx = fp_ctx();
    ctx.enable_psad = true;
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let mut probe = Probe::new(i32::MIN);
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    // Requested width 9 (start 0, end 9) -> 9 % 8 = 1, remain = 7,
    // rounded end 16, so search_area_width = 16 and the tail is empty.
    md_full_pel_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Sad,
        0,
        0,
        SearchWindow {
            start_x: 0,
            end_x: 9,
            start_y: 0,
            end_y: 0,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        },
        &mut best,
    );
    // Probe records [marker, width, height] for the 8-aligned half.
    let marker = probe.visited.iter().position(|&v| v < -100_000).unwrap();
    assert_eq!(probe.visited[marker + 1], 16, "x extent rounded up to 16");
    assert_eq!(probe.visited[marker + 2], 1, "height is end - start + 1");
    // Nothing else was scanned: the tail is empty when the rounded
    // width is already a multiple of 8.
    assert_eq!(probe.visited.len(), 3);
}

/// TIER 4 — `sparse_extent`'s cap applies to the pre-multiplier
/// product and the half-extent is floored to the step.
#[test]
fn tier4_sparse_extent_cap_and_step_alignment() {
    // 20 * 3 * 2 = 120, capped at 64, * 100 / 100 = 64, half 32,
    // floored to a multiple of 8 -> 32.
    assert_eq!(sparse_extent(100, 20, 3, 2, 64, 8), 32);
    // The multiplier scales AFTER the cap.
    assert_eq!(sparse_extent(50, 20, 3, 2, 64, 8), 16);
    // The step floor bites: 30 / 2 = 15, floored to a multiple of 4.
    assert_eq!(sparse_extent(100, 30, 1, 1, 1000, 4), 12);
}

/// TIER 4 — the level-1 nudge pushes a 4-aligned bound OUT by 2.
#[test]
fn tier4_nudge_sprs_lev1() {
    assert_eq!(nudge_sprs_lev1(-8, 8), (-10, 10));
    assert_eq!(nudge_sprs_lev1(-6, 6), (-6, 6));
    assert_eq!(nudge_sprs_lev1(0, 0), (-2, 2));
}

/// TIER 4 — the high-motion detector's threshold comparison, and that
/// the two categories come from DIFFERENT functions.
#[test]
fn tier4_sq_search_area_multiplier() {
    let ctrls = MdSqMeCtrls {
        pame_distortion_th: 10,
        ..Default::default()
    };
    // Blocks above 64 never trigger.
    assert_eq!(
        sq_search_area_multiplier(&ctrls, 128, 16, 16, u32::MAX, false, &[], 0, 0, Mv::ZERO),
        0
    );
    // Below the threshold: 0.
    assert_eq!(
        sq_search_area_multiplier(&ctrls, 32, 16, 16, 10 * 256, false, &[], 0, 0, Mv::ZERO),
        0
    );
    // Above it with an INTER reference: the temporal (absolute)
    // category.
    assert_eq!(
        sq_search_area_multiplier(
            &ctrls,
            32,
            16,
            16,
            10 * 256 + 1,
            false,
            &[],
            0,
            0,
            mv(0, -3000)
        ),
        2
    );
    // Above it with an INTRA/key reference: the spatial (signed)
    // category, which ignores the same negative magnitude.
    assert_eq!(
        sq_search_area_multiplier(
            &ctrls,
            32,
            16,
            16,
            10 * 256 + 1,
            true,
            &[mv(0, -3000)],
            0,
            0,
            mv(0, -3000)
        ),
        0
    );
    assert_eq!(
        sq_search_area_multiplier(
            &ctrls,
            32,
            16,
            16,
            10 * 256 + 1,
            true,
            &[mv(0, 3000)],
            0,
            0,
            Mv::ZERO
        ),
        3
    );
}

/// TIER 4 — the NSQ MVC list: SQ MV first, deduped sub-block MVs,
/// then a zero MV only if absent.
#[test]
fn tier4_nsq_mvc_list() {
    let l = nsq_mvc_list(mv(8, 8), &[mv(16, 16), mv(8, 8), mv(24, 24)]);
    assert_eq!(l, vec![mv(8, 8), mv(16, 16), mv(24, 24), Mv::ZERO]);
    // A list already containing (0,0) does not get a second one.
    let l = nsq_mvc_list(Mv::ZERO, &[mv(8, 0)]);
    assert_eq!(l, vec![Mv::ZERO, mv(8, 0)]);
    // The cap is 6 entries.
    let many: Vec<Mv> = (1..12).map(|i| mv(i * 8, 0)).collect();
    assert_eq!(
        nsq_mvc_list(Mv::ZERO, &many).len(),
        MAX_MD_NSQ_SEARCH_MVC_CNT
    );
}

/// TIER 4 — `(v + 4) & ~7` rounds to nearest full pel with ties UP,
/// which is NOT truncation toward zero.
#[test]
fn tier4_round_to_full_pel_ties_up() {
    assert_eq!(round_to_full_pel(0), 0);
    assert_eq!(round_to_full_pel(3), 0);
    assert_eq!(round_to_full_pel(4), 8);
    assert_eq!(round_to_full_pel(-4), 0);
    assert_eq!(round_to_full_pel(-5), -8);
    assert_eq!(round_to_full_pel(-8), -8);
}

/// TIER 4 — the fixed-stage sub-pel search's three exits and its bias
/// rule.
#[test]
fn tier4_md_subpel_search_fixed_stage_exits() {
    let ctrls = MdSubpelCtrls {
        abs_th_mult: 1,
        max_precision: 2, // > QUARTER_PEL: no quarter-pel stage
        ..Default::default()
    };
    // The integer baseline is already below th_normalizer
    // (16 * 16 * 1 = 256), so C returns WITHOUT writing me_mv.
    let mut probe = Probe::new(i32::MIN);
    probe.subpel.clear();
    let mut m = mv(24, -16);
    // Probe::variance returns 1000 unless the index is the target;
    // make the baseline the target so it costs 0.
    let base = 32 + 3 + (32 - 2) * 448;
    probe.target = base;
    let got = md_subpel_search_fixed_stage(&ctrls, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
    assert_eq!(got, 0);
    assert_eq!(m, mv(24, -16), "the early exit leaves me_mv untouched");

    // With a high baseline the half-pel stage runs: four probes.
    let mut probe = Probe::new(i32::MIN);
    let mut m = mv(24, -16);
    let got = md_subpel_search_fixed_stage(&ctrls, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
    assert_eq!(got, 1000);
    // 1 integer + 4 half-pel probes.
    assert_eq!(probe.visited.len(), 5);
    // No improvement, so the MV is unchanged (best_dx/dy stay 0).
    assert_eq!(m, mv(24, -16));

    // max_precision <= QUARTER_PEL adds four more probes.
    let ctrls_q = MdSubpelCtrls {
        max_precision: QUARTER_PEL,
        ..ctrls
    };
    let mut probe = Probe::new(i32::MIN);
    let mut m = mv(24, -16);
    md_subpel_search_fixed_stage(&ctrls_q, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
    assert_eq!(probe.visited.len(), 9);
}

/// TIER 4 — `pred_variance_th`'s flat-reference exit is normalised
/// per pixel by `ROUND_POWER_OF_TWO(var, num_pels_log2)`.
#[test]
fn tier4_md_subpel_fixed_stage_pred_variance_exit() {
    let ctrls = MdSubpelCtrls {
        abs_th_mult: 0, // never exit on th_normalizer
        pred_variance_th: 100,
        max_precision: 2,
        ..Default::default()
    };
    let mut probe = Probe::new(i32::MIN);
    // 256x256 block would be log2 8; var 25000 >> 8 = 98 (rounded),
    // below 100 -> exit before any sub-pel probe.
    probe.flat = 25_000;
    let mut m = mv(0, 0);
    md_subpel_search_fixed_stage(&ctrls, &mut probe, 0, 0, 16, 16, 8, 448, 0, &mut m);
    assert_eq!(probe.visited.len(), 1, "only the integer baseline ran");

    // A higher flat variance keeps the search going.
    let mut probe = Probe::new(i32::MIN);
    probe.flat = 26_000;
    let mut m = mv(0, 0);
    md_subpel_search_fixed_stage(&ctrls, &mut probe, 0, 0, 16, 16, 8, 448, 0, &mut m);
    assert_eq!(probe.visited.len(), 5);
}

/// TIER 4 — the MVP list: `shut_fast_rate` gives ONE ZERO MVP, not
/// the stack's nearest.
#[test]
fn tier4_build_single_ref_mvp_list() {
    let r = geom();
    let stack = [mv(4, 4), mv(12, 12), mv(20, 20), mv(28, 28)];
    assert_eq!(
        build_single_ref_mvp_list(true, &stack, 4, 32, 32, 16, 16, &r),
        vec![Mv::ZERO]
    );

    // ref_mv_count 4 -> max_drl_index for NEARMV is 3.
    let l = build_single_ref_mvp_list(false, &stack, 4, 32, 32, 16, 16, &r);
    assert_eq!(l.len(), 4);
    assert_eq!(l[0], mv(8, 8), "nearest rounded to full pel");
    assert_eq!(l[1], mv(16, 16));

    // ref_mv_count 0 -> max_drl_index 1, so NEAREST plus one NEAR.
    let l = build_single_ref_mvp_list(false, &stack, 0, 32, 32, 16, 16, &r);
    assert_eq!(l.len(), 2);

    // Duplicates after rounding collapse.
    let dup = [mv(4, 4), mv(5, 5), mv(6, 6), mv(7, 7)];
    let l = build_single_ref_mvp_list(false, &dup, 4, 32, 32, 16, 16, &r);
    assert_eq!(l, vec![mv(8, 8)]);
}

/// TIER 4 — the best-MVP pick breaks ties toward the EARLIER index.
#[test]
fn tier4_best_mvp_by_distortion_ties_to_the_first() {
    let mut probe = Probe::new(i32::MIN); // every cost is 1000
    let mvps = [mv(0, 0), mv(8, 0), mv(16, 0)];
    let (idx, cost) = best_mvp_by_distortion(&mvps, &mut probe, 0, 0, 448, 0);
    assert_eq!((idx, cost), (0, 1000));

    // A unique minimum wins wherever it is.
    let mut probe = Probe::new(2);
    let (idx, cost) = best_mvp_by_distortion(&mvps, &mut probe, 0, 0, 448, 0);
    assert_eq!((idx, cost), (2, 0));
}

/// **`md_nsq_motion_search`'s MVC pass rounds IN PLACE and the ladder
/// only wins on a STRICT improvement.** Two cells:
///
/// 1. The winning position is one of the MVC entries, so `me_mv`
///    comes back as that entry ROUNDED to full pel.
/// 2. Every position costs the same, so `best_search_cost` ties
///    `search_center_cost` and the MVC winner must STAND — C's
///    comparison is `<`, not `<=`. Mutating it to `<=` fails this
///    test (measured).
///
/// **What this cell does NOT witness, and why.** Deleting the in-place
/// `round_to_full_pel` leaves it GREEN (measured). That is a property
/// of the arithmetic, not a gap in the assertions: rounding to the
/// nearest multiple of 8 eighth-pels moves the centre by at most one
/// FULL pel after `md_full_pel_search`'s `>> 3`, and the ladder's last
/// pass is `±1` at step 1 — so on any surface with a single minimum
/// the ladder recovers the same MV either way. The rounding is
/// observable only through which position becomes `search_center_cost`
/// (and therefore whether the ladder's strict win fires); a cell for
/// that needs a cost surface with two competing minima and is not
/// written here.
///
/// Evidence tier 4.
#[test]
fn tier4_md_nsq_motion_search_rounds_mvcs_and_needs_a_strict_win() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);

    // MVC[1] is (12, -4) eighth-pel, which ROUNDS to (16, 0) = full-pel
    // (2, 0). Make that the unique zero-cost position.
    let target = (ctx.blk_org_x + 2) + ctx.blk_org_y * r.y_stride as i32;
    let mut probe = Probe::new(target);
    let mut mv = Mv::ZERO;
    md_nsq_motion_search(
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Var,
        0,
        0,
        Mv { x: 0, y: 0 },
        &[Mv { x: 12, y: -4 }],
        &mut mv,
    );
    assert_eq!(
        (mv.x, mv.y),
        (16, 0),
        "the winning MVC must come back ROUNDED to full pel"
    );

    // Every position ties: the MVC winner stands, because the ladder's
    // result is taken only on `best_search_cost < search_center_cost`.
    let mut flat = Probe::new(i32::MIN);
    let mut mv2 = Mv::ZERO;
    md_nsq_motion_search(
        &ctx,
        &r,
        &mut flat,
        &p,
        0,
        DistortionType::Var,
        4,
        4,
        Mv { x: 0, y: 0 },
        &[],
        &mut mv2,
    );
    assert_eq!(
        (mv2.x, mv2.y),
        (0, 0),
        "on an all-equal cost surface the MVC winner must survive the ladder"
    );

    // POSITIVE CONTROL: the ladder DOES take over when it strictly
    // wins. With `full_pel_search_width/height = 8` the FIRST ladder
    // pass is +-4 at step 4, i.e. full-pel offsets {-4, 0, +4} on each
    // axis, so a zero-cost position 4 full pels right of the only MVC
    // is reachable there and NOT by the MVC pass's zero window.
    let far = (ctx.blk_org_x + 4) + ctx.blk_org_y * r.y_stride as i32;
    let mut probe3 = Probe::new(far);
    let mut mv3 = Mv::ZERO;
    md_nsq_motion_search(
        &ctx,
        &r,
        &mut probe3,
        &p,
        0,
        DistortionType::Var,
        8,
        8,
        Mv { x: 0, y: 0 },
        &[],
        &mut mv3,
    );
    assert_eq!(
        (mv3.x, mv3.y),
        (32, 0),
        "the refinement ladder must be able to move the MV off the MVC"
    );
}

// -----------------------------------------------------------------
// pme_search (per-reference body)
// -----------------------------------------------------------------

fn pme_ctrls() -> MdPmeCtrls {
    MdPmeCtrls {
        enabled: true,
        full_pel_search_width: 2,
        full_pel_search_height: 2,
        sa_q_weight: false,
        enable_psad: false,
        early_check_mv_th_multiplier: PME_EARLY_CHECK_OFF,
        // DIFFERENT on purpose: swapping the two thresholds must be
        // observable, and it is not when both are 0.
        pre_fp_pme_to_me_mv_th: 0,
        pre_fp_pme_to_me_cost_th: i64::MAX,
        post_fp_pme_to_me_mv_th: 100,
        post_fp_pme_to_me_cost_th: i64::MAX,
    }
}

/// **Three of `pme_search`'s four exits hand back the ME MV, and all
/// three write `valid_pme_mv = 1`** — so `valid` alone cannot tell a
/// searched MV from an ME echo, which is why the port returns
/// [`PmeExit`]. One cell per exit:
///
/// * `Skipped` — `valid` stays FALSE and nothing is written.
/// * `EarlyMvpCheck` — the ME MV agrees with every MVP.
/// * `PreFullPel` — the MVP is within `pre_fp_pme_to_me_mv_th` of the
///   ME MV.
/// * `Searched` — the search ran and its own MV came back.
///
/// Each bail returns `sub_me_mv` and `post_subpel_me_mv_cost`, NOT the
/// MVP and not the full-pel cost; the assertions pin that pairing
/// because C mixes the two deliberately (`me_mv_cost` is the FULL-pel
/// `fp_me_dist` while the stored dist is the SUB-pel one).
///
/// **What these cells do NOT witness.** Both `*_cost_th` are
/// `i64::MAX` here, so the COST arm of [`pme_bails_to_me`] never fires
/// and swapping `me_mv_cost`'s source from `fp_me_dist` to
/// `post_subpel_me_mv_cost` leaves them green (measured). The
/// deviation arithmetic is covered by [`pme_to_me_cost_dev`]'s own
/// cell; what is uncovered is WHICH of the two ME costs feeds it, and
/// a cell for that needs a cost threshold low enough to make the arm
/// decide.
///
/// Evidence tier 4.
#[test]
fn tier4_pme_search_exits_and_what_each_one_writes() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    // DIFFERENT on purpose: every bail-out must return the SUB-pel MV,
    // and a port that returned `fp_me_mv` instead would be
    // indistinguishable if these were equal.
    let sub_me = Mv { x: 26, y: -7 };
    let fp_me = Mv { x: 24, y: -8 };

    // 1. Skipped.
    let mut probe = Probe::new(i32::MIN);
    let out = pme_search_for_ref(
        &pme_ctrls(),
        &ctx,
        &r,
        &mut probe,
        &p,
        0,
        DistortionType::Var,
        true,
        true,
        fp_me,
        sub_me,
        10,
        20,
        &[Mv::ZERO],
        2,
        2,
        320,
        240,
        None,
    );
    assert_eq!(out.exit, PmeExit::Skipped);
    assert!(!out.valid);
    assert!(
        probe.visited.is_empty(),
        "a skipped ref must touch no pixel"
    );

    // 2. EarlyMvpCheck: the multiplier is ON and the ME MV agrees with
    //    the single (0,0) MVP, so C adopts the ME MV.
    let mut c2 = pme_ctrls();
    c2.early_check_mv_th_multiplier = 10;
    let mut probe2 = Probe::new(i32::MIN);
    let out2 = pme_search_for_ref(
        &c2,
        &ctx,
        &r,
        &mut probe2,
        &p,
        0,
        DistortionType::Var,
        false,
        true,
        fp_me,
        sub_me,
        10,
        20,
        &[Mv::ZERO],
        2,
        2,
        320,
        240,
        None,
    );
    assert_eq!(out2.exit, PmeExit::EarlyMvpCheck);
    assert_eq!(
        (out2.valid, out2.best_pme_mv, out2.dist),
        (true, sub_me, 20)
    );
    assert!(
        probe2.visited.is_empty(),
        "the early check runs BEFORE any distortion is computed"
    );

    // 3. PreFullPel: the MVP is the ME MV itself, so the pre-full-pel
    //    MV threshold of 0 is satisfied.
    let mut probe3 = Probe::new(i32::MIN);
    let out3 = pme_search_for_ref(
        &pme_ctrls(),
        &ctx,
        &r,
        &mut probe3,
        &p,
        0,
        DistortionType::Var,
        false,
        true,
        fp_me,
        sub_me,
        10,
        20,
        &[fp_me],
        2,
        2,
        320,
        240,
        None,
    );
    assert_eq!(out3.exit, PmeExit::PreFullPel);
    assert_eq!((out3.best_pme_mv, out3.dist), (sub_me, 20));

    // 4. Searched: no ME data at all, so every bail-out is skipped and
    //    the full-pel search's own MV comes back. The zero-cost
    //    position is +1 full pel from the (0,0) MVP, inside the +-1
    //    window.
    let target = (ctx.blk_org_x + 1) + ctx.blk_org_y * r.y_stride as i32;
    let mut probe4 = Probe::new(target);
    let out4 = pme_search_for_ref(
        &pme_ctrls(),
        &ctx,
        &r,
        &mut probe4,
        &p,
        0,
        DistortionType::Var,
        false,
        false,
        fp_me,
        sub_me,
        10,
        20,
        &[Mv::ZERO],
        2,
        2,
        320,
        240,
        None,
    );
    assert_eq!(out4.exit, PmeExit::Searched);
    assert_eq!(out4.best_pme_mv, Mv { x: 8, y: 0 });
    assert_eq!(
        out4.dist,
        u32::MAX,
        "with no subpel search C stores its `~0` sentinel into pme_res.dist"
    );

    // 5. PostFullPel, and the PRE/POST thresholds are not
    //    interchangeable. The MVP is one eighth-pel off the ME MV, so
    //    the PRE check (th 0) does NOT fire and the search runs; the
    //    POST check (th 100) then does. Using the post threshold in the
    //    pre check turns this into `PreFullPel` and fails.
    let mut probe5 = Probe::new(i32::MIN);
    let out5 = pme_search_for_ref(
        &pme_ctrls(),
        &ctx,
        &r,
        &mut probe5,
        &p,
        0,
        DistortionType::Var,
        false,
        true,
        fp_me,
        sub_me,
        10,
        20,
        &[Mv {
            x: fp_me.x + 1,
            y: fp_me.y,
        }],
        2,
        2,
        320,
        240,
        None,
    );
    assert_eq!(out5.exit, PmeExit::PostFullPel);
    assert_eq!((out5.best_pme_mv, out5.dist), (sub_me, 20));
    assert!(
        !probe5.visited.is_empty(),
        "the PRE check must NOT have bailed — the full-pel search ran"
    );
}

// -----------------------------------------------------------------
// read_refine_me_mvs (per-reference body)
// -----------------------------------------------------------------

fn refine_in() -> RefineMeIn {
    RefineMeIn {
        blk_avail_sqi: false,
        bsize_is_64x128_or_128x64: false,
        bsize_is_4x4: false,
        parent_tested: false,
        sq_sb_me_mv: Mv::ZERO,
        raw_me_mv_full_pel: Mv::ZERO,
        md_nsq_me_enabled: false,
        do_subpel: false,
        subpel_fixed_stage: false,
        needs_fp_me_dist: false,
        shape_is_part_n: true,
    }
}

/// **`sub_me_mv` keeps the post-search MV and `sb_me_mv` gets a SECOND
/// clip**, so the two differ whenever the search left the picture. C
/// clips once before `choose_best_av1_mv_pred` and again into
/// `sb_me_mv` after the searches; a port that clipped once, or that
/// wrote the clipped value into both, is a different MV downstream.
///
/// The MV here is 1000 full pels right of a block at x=32 in a
/// 320-wide reference with a 64 border, so both clips fire.
///
/// Also asserts the two sentinels: `post_subpel_me_mv_cost` stays
/// `u32::MAX` (C's `(int32_t)~0`) when subpel is off, and `fp_me_dist`
/// stays absent unless somebody asked for it.
///
/// Evidence tier 4.
#[test]
fn tier4_refine_me_clips_twice_and_leaves_the_cost_sentinel() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let mut probe = Probe::new(i32::MIN);

    let mut inp = refine_in();
    inp.raw_me_mv_full_pel = Mv { x: 1000, y: 0 };
    let out = refine_me_mv_for_ref(inp, &ctx, &r, None, None, None, &mut probe, &p, 0);

    // C's clip: `(max_width - blk_org_x) * 8` = (320 - 32) * 8.
    let clipped = ((r.max_width - ctx.blk_org_x) * 8) as i16;
    assert_eq!(out.fp_me_mv.x, clipped, "the CENTRE is clipped first");
    assert_eq!(out.sub_me_mv.x, clipped);
    assert_eq!(out.sb_me_mv.x, clipped, "and clipped again into sb_me_mv");
    assert_eq!(
        out.post_subpel_me_mv_cost,
        u32::MAX,
        "no subpel search ran, so C's `~0` sentinel must survive"
    );
    assert_eq!(out.fp_me_dist, None);
    assert_eq!(out.sq_sb_me_mv, Some(out.sb_me_mv), "PART_N writes it back");

    // A NON-square shape must NOT write `sq_sb_me_mv`.
    let mut nsq_in = refine_in();
    nsq_in.shape_is_part_n = false;
    let out2 = refine_me_mv_for_ref(nsq_in, &ctx, &r, None, None, None, &mut probe, &p, 0);
    assert_eq!(out2.sq_sb_me_mv, None);

    // THE SECOND CLIP, witnessed: let the SEARCH push the MV back out
    // of the picture after the first clip already ran. `sub_me_mv`
    // must keep the out-of-range value and `sb_me_mv` must be clipped
    // — deleting the second clip makes them equal and fails here.
    let mut moved = refine_in();
    moved.do_subpel = true;
    let mut push_out = |mv: &mut Mv| {
        mv.x = 20_000;
        0u32
    };
    let out3 = refine_me_mv_for_ref(
        moved,
        &ctx,
        &r,
        None,
        Some(&mut push_out),
        None,
        &mut probe,
        &p,
        0,
    );
    assert_eq!(
        out3.sub_me_mv.x, 20_000,
        "sub_me_mv keeps the UNCLIPPED post-search MV"
    );
    assert_eq!(
        out3.sb_me_mv.x, clipped,
        "sb_me_mv is the same MV clipped a SECOND time"
    );
    assert_ne!(out3.sub_me_mv.x, out3.sb_me_mv.x);
}

/// **The full-pel ME cost is a CONDITIONAL side effect, not a
/// fallback.** C computes `fp_me_dist` only when subpel is off AND
/// `updated_enable_pme || ref_pruning_ctrls.enabled`; with subpel ON it
/// is never written no matter who wants it, and with both off it is
/// skipped even though nothing else fills the slot.
///
/// Evidence tier 4.
#[test]
fn tier4_refine_me_fp_dist_needs_subpel_off_and_a_consumer() {
    let ctx = fp_ctx();
    let r = geom();
    let t = zero_cost();
    let p = params(&t);
    let mut probe = Probe::new(i32::MIN);

    // subpel OFF + a consumer -> written.
    let mut a = refine_in();
    a.needs_fp_me_dist = true;
    let mut fp = |_mv: Mv| 4242u32;
    let out = refine_me_mv_for_ref(a, &ctx, &r, None, None, Some(&mut fp), &mut probe, &p, 0);
    assert_eq!(out.fp_me_dist, Some(4242));
    assert_eq!(out.post_subpel_me_mv_cost, u32::MAX);

    // subpel ON -> the subpel cost is written and fp_me_dist is not,
    // even with a consumer asking.
    let mut b = refine_in();
    b.needs_fp_me_dist = true;
    b.do_subpel = true;
    let mut sp = |mv: &mut Mv| {
        mv.x += 3;
        77u32
    };
    let mut fp2 = |_mv: Mv| 4242u32;
    let out2 = refine_me_mv_for_ref(
        b,
        &ctx,
        &r,
        None,
        Some(&mut sp),
        Some(&mut fp2),
        &mut probe,
        &p,
        0,
    );
    assert_eq!(out2.post_subpel_me_mv_cost, 77);
    assert_eq!(out2.fp_me_dist, None);
    assert_eq!(
        (out2.fp_me_mv.x, out2.sub_me_mv.x),
        (0, 3),
        "fp_me_mv is the PRE-subpel MV and sub_me_mv the POST-subpel one"
    );

    // subpel OFF and nobody needs it -> nothing written.
    let out3 = refine_me_mv_for_ref(
        refine_in(),
        &ctx,
        &r,
        None,
        None,
        Some(&mut fp),
        &mut probe,
        &p,
        0,
    );
    assert_eq!(out3.fp_me_dist, None);
}

// -----------------------------------------------------------------
// md_subpel_search
// -----------------------------------------------------------------

/// Index of the test block's (0, 0) inside the 64x64 test planes.
const SUBPEL_BLK_BASE: usize = 16 * 64 + 16;

/// A ctrls set that lets the search actually run: every early exit
/// disarmed, the PRUNED tree (C's default at every preset this port
/// reaches), quarter-pel stop.
fn subpel_ctrls() -> crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls {
    crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls {
        enabled: 1,
        subpel_search_type: 1,
        max_precision: 0,
        subpel_search_method:
            crate::port_enc_mode_config::encdec::subpel_search_method::SUBPEL_TREE_PRUNED,
        subpel_iters_per_step: 1,
        pred_variance_th: 0,
        abs_th_mult: 0,
        round_dev_th: i32::MIN,
        skip_diag_refinement: 0,
        min_blk_sz: 0,
        mvp_th: 0,
        hp_mv_th: 0,
        bias_fp: 0,
    }
}

fn subpel_geom() -> SubpelBlockGeom {
    SubpelBlockGeom {
        // The block sits at (16, 16) of a 64x64 plane, so the search's
        // NEGATIVE offsets stay inside the allocation — C's reference is
        // a padded picture and `ref_base` is the block's own (0, 0)
        // inside it, never index 0.
        mi_row: 4,
        mi_col: 4,
        mi_width: 4,
        mi_height: 4,
        mi_rows: 64,
        mi_cols: 64,
        bwidth: 16,
        bheight: 16,
        sq_size: 16,
    }
}

/// A 64x64 plane whose value at (r, c) is a fixed pseudo-random
/// function, so the block has real gradient in both directions.
fn subpel_plane(shift: usize) -> Vec<u8> {
    (0..64)
        .flat_map(|r: usize| {
            (0..64).map(move |c: usize| {
                let c = c.saturating_sub(shift);
                (((r * 7 + c * 13) % 251) ^ ((c * 3) & 0x3f)) as u8
            })
        })
        .collect()
}

/// **`md_subpel_search` is REACHED and reads real pixels.** The two
/// cells are the positive control: an identical reference scores zero
/// and does not move, a shifted one scores non-zero.
///
/// Evidence tier 4 — `md_subpel_search` is `static` in C. Its two
/// leaves (`svt_av1_find_best_sub_pixel_tree{,_pruned}`) are exported
/// and are called here, not re-transcribed.
#[test]
fn tier4_md_subpel_search_is_exact_on_an_identical_reference() {
    let ctrls = subpel_ctrls();
    let geom = subpel_geom();
    let src = subpel_plane(0);
    let same = subpel_plane(0);
    let mut mv = Mv::ZERO;
    let err = md_subpel_search(
        crate::md_subpel::SPEL_ME,
        &ctrls,
        geom,
        svtav1_types::block::BlockSize::Block16x16,
        0,
        0,
        true,
        Mv::ZERO,
        100,
        1 << 12,
        0,
        None,
        &src,
        SUBPEL_BLK_BASE,
        64,
        &same,
        SUBPEL_BLK_BASE as i64,
        64,
        None,
        &mut mv,
    );
    assert_eq!(
        (err, mv.x, mv.y),
        (0, 0, 0),
        "an identical reference has zero variance at the start MV, and \
             nothing can beat zero"
    );

    // POSITIVE CONTROL: the same call against a DIFFERENT reference
    // must score non-zero, otherwise the zero above would be a search
    // that never read a pixel.
    let shifted = subpel_plane(3);
    let mut mv2 = Mv::ZERO;
    let err2 = md_subpel_search(
        crate::md_subpel::SPEL_ME,
        &ctrls,
        geom,
        svtav1_types::block::BlockSize::Block16x16,
        0,
        0,
        true,
        Mv::ZERO,
        100,
        1 << 12,
        0,
        None,
        &src,
        SUBPEL_BLK_BASE,
        64,
        &shifted,
        SUBPEL_BLK_BASE as i64,
        64,
        None,
        &mut mv2,
    );
    assert!(
        err2 > 0,
        "a shifted reference must cost something; got {err2}"
    );
    assert!(
        mv2.x.abs() <= 8 && mv2.y.abs() <= 8,
        "the SUB-pel refinement cannot travel a whole pel from its start; \
             got ({}, {})",
        mv2.x,
        mv2.y
    );
}

/// **The start MV is TRUNCATED to full pel before the search.**
/// C does `me_mv >> 3` then `get_mv_from_fullmv` (`<< 3`), so a
/// fractional input is discarded. With an identical reference the
/// full-pel origin is exact, so a truncating implementation returns
/// (0, 0) from a `(7, 7)` start — a non-truncating one would start
/// 7/8 pel away and could not reach zero error in one half-pel step.
#[test]
fn tier4_md_subpel_search_truncates_its_start_mv_to_full_pel() {
    let ctrls = subpel_ctrls();
    let src = subpel_plane(0);
    let same = subpel_plane(0);
    let mut mv = Mv { x: 7, y: 7 };
    let err = md_subpel_search(
        crate::md_subpel::SPEL_ME,
        &ctrls,
        subpel_geom(),
        svtav1_types::block::BlockSize::Block16x16,
        0,
        0,
        true,
        Mv::ZERO,
        100,
        1 << 12,
        0,
        None,
        &src,
        SUBPEL_BLK_BASE,
        64,
        &same,
        SUBPEL_BLK_BASE as i64,
        64,
        None,
        &mut mv,
    );
    assert_eq!((err, mv.x, mv.y), (0, 0, 0));
}

/// **`svt_mv_err_cost` (mcomp.c:42-72) has ONE body in this crate**,
/// `md_subpel::mv_err_cost`; `port_md::pme::mv_err_cost`,
/// `port_md::pme::fp_mv_err_cost`, `md_subpel::fp_mv_err_cost` and
/// `intrabc::mv_err_cost` (av1me.c's `svt_aom_mv_err_cost`, the ENTROPY
/// arm under an older name) are forwards to it. Until 2026-09-04 this
/// test pinned two independent transcriptions to each other over these
/// 576 cells (`docs/WORKING-ON-THIS.md` §4); the second transcription is
/// gone, and the sweep now guards the fold itself — if anyone re-grows
/// a body behind one of the forwarding names, it drifts here first.
/// The ENTROPY cells also cover the av1me.c spelling, which takes the
/// tables by reference instead of through the params struct.
///
/// TEETH: the sweep spans every cost type, an `i16::MIN` wrap and four
/// `error_per_bit` values, so a re-grown body that changes any arm's
/// shift, `abs_sum`, or the rounding fails (`L1MidRes` excepted —
/// `SSE_LAMBDA_MIDRES` is 0, mcomp.c:33, so that arm is identically
/// zero whatever a copy does to it; a statement about the constant, not
/// coverage).
#[test]
fn tier4_every_mv_err_cost_spelling_is_the_one_body() {
    use crate::entropy::mv_coding::MvSubpelPrecision;
    let fc = crate::entropy::context::FrameContext::new_default();
    let tables = crate::intrabc::build_nmv_cost_table(&fc.nmvc, MvSubpelPrecision::High);

    let types = [
        crate::md_subpel::MvCostType::Entropy,
        crate::md_subpel::MvCostType::L1LowRes,
        crate::md_subpel::MvCostType::L1MidRes,
        crate::md_subpel::MvCostType::L1HdRes,
        crate::md_subpel::MvCostType::Opt,
        crate::md_subpel::MvCostType::None,
    ];
    let mvs = [
        (0i16, 0i16),
        (1, -1),
        (8, 8),
        (-8, 24),
        (255, -255),
        (1024, 2047),
        (-2048, 1),
        (i16::MIN, i16::MAX),
    ];
    let refs = [(0i16, 0i16), (8, -8), (-1000, 1000)];
    let epbs = [1i32, 7, 64, 4095];

    let mut cells = 0usize;
    let mut nonzero = 0usize;
    for mt in types {
        for (rx, ry) in refs {
            for (mx, my) in mvs {
                for epb in epbs {
                    let ref_mv = Mv { x: rx, y: ry };
                    let mv = Mv { x: mx, y: my };
                    let params = super::super::pme::init_mv_cost_params(
                        ref_mv,
                        64,
                        if mt == crate::md_subpel::MvCostType::Opt {
                            3
                        } else {
                            0
                        },
                        (epb as u32) << 6,
                        Some(&tables),
                    );
                    // `init_mv_cost_params` only ever picks ENTROPY or
                    // OPT; the other four arms are set directly.
                    let params = crate::md_subpel::MvCostParams {
                        mv_cost_type: mt,
                        ..params
                    };
                    assert_eq!(params.error_per_bit, epb);
                    let body = crate::md_subpel::mv_err_cost(mv, ref_mv, Some(&tables), epb, mt);
                    let label = format!("mv=({mx},{my}) ref=({rx},{ry}) epb={epb} type={mt:?}");
                    assert_eq!(super::super::pme::mv_err_cost(mv, &params), body, "{label}");
                    assert_eq!(
                        super::super::pme::fp_mv_err_cost(mv, &params),
                        body,
                        "{label}"
                    );
                    assert_eq!(
                        crate::md_subpel::fp_mv_err_cost(mv, &params),
                        body,
                        "{label}"
                    );
                    assert_eq!(params.err_cost(mv), body, "{label}");
                    if mt == crate::md_subpel::MvCostType::Entropy {
                        assert_eq!(
                            crate::intrabc::mv_err_cost(mv, ref_mv, &tables, epb),
                            body,
                            "{label} (av1me.c spelling)"
                        );
                    }
                    if body != 0 {
                        nonzero += 1;
                    }
                    cells += 1;
                }
            }
        }
    }
    assert_eq!(cells, 6 * 3 * 8 * 4, "the sweep must not silently shrink");
    assert!(
        nonzero > cells / 3,
        "positive control: only {nonzero} of {cells} non-zero"
    );
}

/// TIER 4 — the PME extents floor at 3 AFTER the rounding division.
#[test]
fn tier4_pme_search_extents_floor_is_three() {
    assert_eq!(pme_search_extents(16, 8, false, 1, 100), (16, 8));
    // 16 * 63 / 63 = 16.
    assert_eq!(pme_search_extents(16, 8, true, 63, 63), (16, 8));
    // A tiny weight collapses to the floor, not to 0.
    assert_eq!(pme_search_extents(16, 8, true, 1, 10_000), (3, 3));
}

/// TIER 4 — the ME-vs-MVP direction check is a sign PRODUCT, so a
/// zero ME component never counts as different.
#[test]
fn tier4_pme_me_mv_differs_from_mvps() {
    // th = ((640*480) >> 17) * 10 / 10 = 2.
    let (w, h, mult) = (640u32, 480u32, 10i32);
    assert!(pme_me_mv_differs_from_mvps(
        &[mv(16, 0)],
        mv(-16, 0),
        w,
        h,
        mult
    ));
    // Same sign -> not different.
    assert!(!pme_me_mv_differs_from_mvps(
        &[mv(16, 0)],
        mv(16, 0),
        w,
        h,
        mult
    ));
    // A zero ME component: the product is 0, which is not < 0.
    assert!(!pme_me_mv_differs_from_mvps(
        &[mv(16, 0)],
        mv(0, 0),
        w,
        h,
        mult
    ));
    // An MVP below the magnitude threshold is ignored entirely.
    assert!(!pme_me_mv_differs_from_mvps(
        &[mv(2, 0)],
        mv(-16, 0),
        w,
        h,
        mult
    ));
}

#[test]
fn tier4_pme_cost_dev_and_bail() {
    // Both clamp to 1 before the division.
    assert_eq!(pme_to_me_cost_dev(0, 0), 0);
    assert_eq!(pme_to_me_cost_dev(200, 100), 100);
    assert_eq!(pme_to_me_cost_dev(50, 100), -50);

    // Close in MV: bail regardless of cost.
    assert!(pme_bails_to_me(mv(8, 8), mv(10, 10), 4, -1000, 1000));
    // Far in MV but the cost deviation reaches the threshold: bail.
    assert!(pme_bails_to_me(mv(8, 8), mv(80, 80), 4, 50, 50));
    // Far and cheap: run the search.
    assert!(!pme_bails_to_me(mv(8, 8), mv(80, 80), 4, 49, 50));
}

/// TIER 4 — the subpel MV limits are asymmetric between the min and
/// max sides.
#[test]
fn tier4_subpel_mv_limits() {
    let (row_min, row_max, col_min, col_max) = subpel_mv_limits(4, 8, 4, 4, 64, 64);
    assert_eq!(row_min, -(((4 + 4) * 4) + 4));
    assert_eq!(col_min, -(((8 + 4) * 4) + 4));
    assert_eq!(row_max, (64 - 4) * 4 + 4);
    assert_eq!(col_max, (64 - 8) * 4 + 4);
}

/// TIER 4 — the ME centre inherits the SQ MV for NSQ blocks, EXCEPT
/// 64x128 / 128x64.
#[test]
fn tier4_me_mv_center_inheritance() {
    let sq = mv(20, -20);
    let raw = mv(3, -3);
    // NSQ, parent available, not the excluded sizes -> inherit,
    // rounded.
    assert_eq!(
        me_mv_center(true, 32, 16, false, false, false, sq, raw),
        mv(24, -16)
    );
    // The excluded shapes fall back to the raw ME MV x8.
    assert_eq!(
        me_mv_center(true, 64, 128, true, false, false, sq, raw),
        mv(24, -24)
    );
    // A square block does not inherit.
    assert_eq!(
        me_mv_center(true, 32, 32, false, false, false, sq, raw),
        mv(24, -24)
    );
    // A 4x4 whose parent was tested DOES inherit even though it is
    // square.
    assert_eq!(
        me_mv_center(false, 4, 4, false, true, true, sq, raw),
        mv(24, -16)
    );
    // ...but only when the parent was tested.
    assert_eq!(
        me_mv_center(false, 4, 4, false, true, false, sq, raw),
        mv(24, -24)
    );
}
