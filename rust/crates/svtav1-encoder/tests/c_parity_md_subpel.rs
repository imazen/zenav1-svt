//! Differential parity for `svtav1_encoder::md_subpel` — the port of
//! `Source/Lib/Codec/mcomp.c` — against the REAL exported C symbols in
//! `libSvtAv1Enc.a`. **Evidence tier 1** (`rust/docs/WORKING-ON-THIS.md` §4).
//!
//! ## What is driven, and why this shape
//!
//! `nm -g Bin/Release/libSvtAv1Enc.a` prints `T` for exactly three of
//! mcomp.c's 17 functions — `svt_av1_find_best_sub_pixel_tree`,
//! `svt_av1_find_best_sub_pixel_tree_pruned` and `svt_aom_fp_mv_err_cost`.
//! The other fourteen are `static` and print nothing (positive control: the
//! three above DO print; negative control: `svt_check_better_fast`,
//! `two_level_checks_fast`, `get_best_diag_step` print nothing).
//!
//! Those fourteen are reachable ONLY through the two entry points, so this
//! file drives the entry points through
//! `crates/svtav1-cref/shims/md_subpel_shims.c`, which builds
//! `SUBPEL_MOTION_SEARCH_PARAMS` + `MacroBlockD` + `ModeDecisionContext` from
//! plain scalars. A green run here is a tier-1 differential over the WHOLE
//! tree — every helper, every tie-break, every early exit — not fourteen
//! hand-derived vectors, which would only prove that two transcriptions of the
//! same logic agree.
//!
//! Compared on every cell: the returned `besterr`, the out `bestmv`, the out
//! `distortion`, the out `sse1`, and (when a context is passed) the
//! `ctx->fp_me_dist` C writes.

use svtav1_cref::md_subpel as cref;
use svtav1_encoder::entropy::mv_coding::{MvSubpelPrecision, NmvContext};
use svtav1_encoder::intrabc::{MV_MAX, MV_VALS, MvCostTables, build_nmv_cost_table};
use svtav1_encoder::md_subpel::{
    MvCostParams, MvCostType, SPEL_ME, SubpelMdContext, SubpelSearchParams, SubpelSearchVarParams,
    SubpelState, find_best_sub_pixel_tree, find_best_sub_pixel_tree_pruned,
};
use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn u8(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// C `block_size_wide` / `block_size_high` for the sizes exercised here.
fn dims(b: BlockSize) -> (usize, usize) {
    use BlockSize::*;
    match b {
        Block4x4 => (4, 4),
        Block4x8 => (4, 8),
        Block8x4 => (8, 4),
        Block8x8 => (8, 8),
        Block8x16 => (8, 16),
        Block16x8 => (16, 8),
        Block16x16 => (16, 16),
        Block16x32 => (16, 32),
        Block32x16 => (32, 16),
        Block32x32 => (32, 32),
        Block32x64 => (32, 64),
        Block64x32 => (64, 32),
        Block64x64 => (64, 64),
        Block64x128 => (64, 128),
        Block128x64 => (128, 64),
        Block128x128 => (128, 128),
        Block4x16 => (4, 16),
        Block16x4 => (16, 4),
        Block8x32 => (8, 32),
        Block32x8 => (32, 8),
        Block16x64 => (16, 64),
        Block64x16 => (64, 16),
    }
}

/// The MV cost tables, in both the port's form and the flat `[i32; MV_VALS]`
/// pair the C shim centres at `MV_MAX`.
struct Tables {
    port: MvCostTables,
    joint: [i32; 4],
    row: Box<[i32; MV_VALS]>,
    col: Box<[i32; MV_VALS]>,
}

fn build_tables() -> Tables {
    let ctx = NmvContext::default();
    let port = build_nmv_cost_table(&ctx, MvSubpelPrecision::High);
    let mut row = Box::new([0i32; MV_VALS]);
    let mut col = Box::new([0i32; MV_VALS]);
    for v in -MV_MAX..=MV_MAX {
        row[(MV_MAX + v) as usize] = port.comp_cost[0].cost(v);
        col[(MV_MAX + v) as usize] = port.comp_cost[1].cost(v);
    }
    let joint = port.joint_cost;
    Tables {
        port,
        joint,
        row,
        col,
    }
}

/// One reference plane + one source block, plus the geometry both sides use.
struct Planes {
    src: Vec<u8>,
    src_stride: usize,
    ref_alloc: Vec<u8>,
    ref_stride: usize,
    ref_base: usize,
}

/// `guard` full pixels of margin on every side of the reference block, so a
/// negative MV and the sub-pel kernel's `+1` row/column stay inside.
fn make_planes(w: usize, h: usize, guard: usize, rng: &mut Rng, flat: bool) -> Planes {
    let src_stride = w + 9;
    let src: Vec<u8> = (0..src_stride * (h + 2))
        .map(|_| if flat { 100 } else { rng.u8() })
        .collect();
    let ref_stride = w + 2 * guard + 5;
    let ref_alloc: Vec<u8> = (0..ref_stride * (h + 2 * guard + 4))
        .map(|_| if flat { 100 } else { rng.u8() })
        .collect();
    let ref_base = guard * ref_stride + guard;
    Planes {
        src,
        src_stride,
        ref_alloc,
        ref_stride,
        ref_base,
    }
}

/// Everything both sides need for one cell.
#[derive(Clone, Copy)]
struct Cell {
    pruned: bool,
    bsize: BlockSize,
    allow_hp: bool,
    forced_stop: i32,
    iters_per_step: i32,
    pred_variance_th: i32,
    abs_th_mult: u8,
    round_dev_th: i32,
    skip_diag_refinement: u8,
    subpel_search_type: i32,
    bias_fp: i32,
    mv_cost_type: MvCostType,
    error_per_bit: i32,
    early_exit_th: i32,
    start_mv: Mv,
    ref_mv: Mv,
    tight_limits: bool,
    with_ctx: bool,
    mvp_th: i32,
    hp_mv_th: i32,
    best_fp_mvp_dist: u32,
    best_fp_mvp: Mv,
    with_tables: bool,
}

fn mv_cost_type_index(t: MvCostType) -> i32 {
    t as i32
}

/// Run one cell on both sides and assert every observable agrees.
fn run_cell(c: &Cell, p: &Planes, t: &Tables, w: usize, h: usize, label: &str) {
    // Limits: `tight` clips half the candidate set so the out-of-range arm of
    // check_better{,_fast} (which returns INT_MAX and skips the metric) is
    // reached rather than assumed unreachable.
    let (col_min, col_max, row_min, row_max) = if c.tight_limits {
        (
            i32::from(c.start_mv.x) - 2,
            i32::from(c.start_mv.x) + 6,
            i32::from(c.start_mv.y) - 6,
            i32::from(c.start_mv.y) + 2,
        )
    } else {
        (-4096, 4096, -4096, 4096)
    };

    let tables = if c.with_tables { Some(&t.port) } else { None };
    let cref_tables = if c.with_tables {
        Some((&*t.row, &*t.col))
    } else {
        None
    };

    let cp = cref::SubpelParams {
        pruned: c.pruned,
        use_rtcd: false,
        use_ctx: c.with_ctx,
        w,
        h,
        bsize: c.bsize as i32,
        src_stride: p.src_stride,
        ref_base: p.ref_base as i64,
        ref_stride: p.ref_stride,
        allow_hp: c.allow_hp,
        forced_stop: c.forced_stop,
        iters_per_step: c.iters_per_step,
        pred_variance_th: c.pred_variance_th,
        abs_th_mult: c.abs_th_mult,
        round_dev_th: c.round_dev_th,
        skip_diag_refinement: c.skip_diag_refinement,
        search_stage: SPEL_ME,
        list_idx: 0,
        ref_idx: 0,
        subpel_search_type: c.subpel_search_type,
        bias_fp: c.bias_fp,
        col_min,
        col_max,
        row_min,
        row_max,
        ref_mv: (i32::from(c.ref_mv.x), i32::from(c.ref_mv.y)),
        mv_cost_type: mv_cost_type_index(c.mv_cost_type),
        error_per_bit: c.error_per_bit,
        early_exit_th: c.early_exit_th,
        pd_pass: 1,
        mvp_th: c.mvp_th,
        hp_mv_th: c.hp_mv_th,
        best_fp_mvp_dist: c.best_fp_mvp_dist,
        best_fp_mvp: (i32::from(c.best_fp_mvp.x), i32::from(c.best_fp_mvp.y)),
        start_mv: (i32::from(c.start_mv.x), i32::from(c.start_mv.y)),
    };
    let cres = cref::subpel_tree(&cp, &p.src, &p.ref_alloc, &t.joint, cref_tables);

    let ms = SubpelSearchParams {
        allow_hp: c.allow_hp,
        forced_stop: c.forced_stop,
        iters_per_step: c.iters_per_step,
        pred_variance_th: c.pred_variance_th,
        abs_th_mult: c.abs_th_mult,
        round_dev_th: c.round_dev_th,
        skip_diag_refinement: c.skip_diag_refinement,
        search_stage: SPEL_ME,
        list_idx: 0,
        ref_idx: 0,
        mv_limits: svtav1_encoder::md_subpel::SubpelMvLimits {
            col_min,
            col_max,
            row_min,
            row_max,
        },
    };
    let var_params = SubpelSearchVarParams {
        src: &p.src,
        src_base: 0,
        src_stride: p.src_stride,
        ref_alloc: &p.ref_alloc,
        ref_base: p.ref_base as i64,
        ref_stride: p.ref_stride,
        w,
        h,
        bias_fp: c.bias_fp,
        subpel_search_type: c.subpel_search_type,
    };
    let mv_cost_params = MvCostParams {
        ref_mv: c.ref_mv,
        mv_cost_type: c.mv_cost_type,
        tables,
        error_per_bit: c.error_per_bit,
        early_exit_th: c.early_exit_th,
    };
    let mut ctx = SubpelMdContext {
        pd_pass: 1,
        mvp_th: c.mvp_th,
        hp_mv_th: c.hp_mv_th,
        best_fp_mvp_dist: c.best_fp_mvp_dist,
        best_fp_mvp: c.best_fp_mvp,
        fp_me_dist: 0,
    };
    let ctx_arg = if c.with_ctx { Some(&mut ctx) } else { None };
    let (besterr, st): (u32, SubpelState) = if c.pruned {
        find_best_sub_pixel_tree_pruned(
            ctx_arg,
            &ms,
            &var_params,
            &mv_cost_params,
            c.start_mv,
            c.bsize,
        )
    } else {
        find_best_sub_pixel_tree(
            ctx_arg,
            &ms,
            &var_params,
            &mv_cost_params,
            c.start_mv,
            c.bsize,
        )
    };

    assert_eq!(
        (
            besterr,
            (i32::from(st.best_mv.x), i32::from(st.best_mv.y)),
            st.distortion,
            st.sse1
        ),
        (cres.besterr, cres.best_mv, cres.distortion, cres.sse1),
        "{label}: pruned={} {w}x{h} start={:?} stop={} hp={} iters={} skip={} bias={} \
         cost_type={:?} epb={} eeth={} abs_th={} pvth={} rdth={} tight={} ctx={}",
        c.pruned,
        (c.start_mv.x, c.start_mv.y),
        c.forced_stop,
        c.allow_hp,
        c.iters_per_step,
        c.skip_diag_refinement,
        c.bias_fp,
        c.mv_cost_type,
        c.error_per_bit,
        c.early_exit_th,
        c.abs_th_mult,
        c.pred_variance_th,
        c.round_dev_th,
        c.tight_limits,
        c.with_ctx,
    );
    if c.with_ctx {
        assert_eq!(ctx.fp_me_dist, cres.fp_me_dist, "{label}: ctx->fp_me_dist");
    }
}

const COST_TYPES: [MvCostType; 6] = [
    MvCostType::Entropy,
    MvCostType::L1LowRes,
    MvCostType::L1MidRes,
    MvCostType::L1HdRes,
    MvCostType::Opt,
    MvCostType::None,
];

fn base_cell(bsize: BlockSize, pruned: bool) -> Cell {
    Cell {
        pruned,
        bsize,
        allow_hp: true,
        forced_stop: 0,
        iters_per_step: 2,
        pred_variance_th: 0,
        abs_th_mult: 0,
        round_dev_th: 1000,
        skip_diag_refinement: 0,
        subpel_search_type: 3, // USE_8_TAPS
        bias_fp: 0,
        mv_cost_type: MvCostType::Entropy,
        error_per_bit: 40,
        early_exit_th: 1000,
        start_mv: Mv { x: 8, y: -8 },
        ref_mv: Mv { x: 0, y: 0 },
        tight_limits: false,
        with_ctx: false,
        mvp_th: 0,
        hp_mv_th: 0,
        best_fp_mvp_dist: 0,
        best_fp_mvp: Mv { x: 0, y: 0 },
        with_tables: true,
    }
}

/// Sizes covering square, 2:1, 1:2, 4:1 and 1:4 — every `vf`/`svf`
/// instantiation shape the search can be handed.
const SIZES: [BlockSize; 10] = [
    BlockSize::Block4x4,
    BlockSize::Block8x8,
    BlockSize::Block8x4,
    BlockSize::Block4x16,
    BlockSize::Block16x4,
    BlockSize::Block16x16,
    BlockSize::Block32x16,
    BlockSize::Block32x32,
    BlockSize::Block64x16,
    BlockSize::Block64x64,
];

#[test]
fn pruned_tree_matches_c_across_the_knobs() {
    let t = build_tables();
    let mut rng = Rng(0xdead_beef_1234_5678);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &forced_stop in &[0i32, 1, 2, 3] {
            for &allow_hp in &[true, false] {
                for &iters in &[1i32, 2, 3] {
                    for &skip in &[0u8, 1, 2, 3, 4, 5] {
                        let mut c = base_cell(bsize, true);
                        c.forced_stop = forced_stop;
                        c.allow_hp = allow_hp;
                        c.iters_per_step = iters;
                        c.skip_diag_refinement = skip;
                        run_cell(&c, &p, &t, w, h, "pruned/knobs");
                    }
                }
            }
        }
    }
}

#[test]
fn pruned_tree_matches_c_across_mv_cost_types() {
    let t = build_tables();
    let mut rng = Rng(0x0bad_c0de_0000_0001);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &ct in &COST_TYPES {
            for &epb in &[0i32, 7, 40, 255] {
                for &eeth in &[100i32, 1000, 5000] {
                    // `with_tables = false` is C's null `mvcost[i]`. On the
                    // MV_COST_ENTROPY arm that input CRASHES the real C
                    // function rather than returning 0 (mcomp.c's `if
                    // (mvcost)` tests the array ADDRESS — see the PORT-NOTE on
                    // `md_subpel::mv_err_cost`), so it is only driven on the
                    // four arms that never dereference the tables.
                    for &with_tables in &[true, false] {
                        if !with_tables && ct == MvCostType::Entropy {
                            continue;
                        }
                        let mut c = base_cell(bsize, true);
                        c.mv_cost_type = ct;
                        c.error_per_bit = epb;
                        c.early_exit_th = eeth;
                        c.with_tables = with_tables;
                        c.ref_mv = Mv { x: -3, y: 5 };
                        run_cell(&c, &p, &t, w, h, "pruned/costtype");
                    }
                }
            }
        }
    }
}

#[test]
fn pruned_tree_matches_c_on_bias_fp_and_limits() {
    let t = build_tables();
    let mut rng = Rng(0x5151_5151_a5a5_a5a5);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &bias in &[0i32, 50, 90, 100, 110, 200] {
            for &tight in &[false, true] {
                for &start in &[
                    Mv { x: 0, y: 0 },
                    Mv { x: 8, y: 8 },
                    Mv { x: -16, y: 24 },
                    Mv { x: -24, y: -24 },
                ] {
                    let mut c = base_cell(bsize, true);
                    c.bias_fp = bias;
                    c.tight_limits = tight;
                    c.start_mv = start;
                    run_cell(&c, &p, &t, w, h, "pruned/bias");
                }
            }
        }
    }
}

/// The two early exits that decide whether the search runs at all:
/// `abs_th_mult` (via `th_normalizer`) and `pred_variance_th`. A flat
/// reference makes the second one fire; random content makes it not.
#[test]
fn pruned_tree_matches_c_on_early_exits() {
    let t = build_tables();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        for &flat in &[false, true] {
            let p = make_planes(w, h, 8, &mut rng, flat);
            for &abs_th in &[0u8, 1, 4, 64, 255] {
                for &pv in &[0i32, 1, 16, 1024, 1 << 20] {
                    for &rdth in &[-1000i32, -1, 0, 5, 1000] {
                        let mut c = base_cell(bsize, true);
                        c.abs_th_mult = abs_th;
                        c.pred_variance_th = pv;
                        c.round_dev_th = rdth;
                        run_cell(&c, &p, &t, w, h, "pruned/earlyexit");
                    }
                }
            }
        }
    }
}

#[test]
fn unpruned_tree_matches_c_across_the_knobs() {
    let t = build_tables();
    let mut rng = Rng(0x1111_2222_3333_4444);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &forced_stop in &[0i32, 1, 2, 3] {
            for &allow_hp in &[true, false] {
                for &iters in &[1i32, 2, 3] {
                    for &subpel_type in &[1i32, 2, 3] {
                        let mut c = base_cell(bsize, false);
                        c.forced_stop = forced_stop;
                        c.allow_hp = allow_hp;
                        c.iters_per_step = iters;
                        c.subpel_search_type = subpel_type;
                        run_cell(&c, &p, &t, w, h, "unpruned/knobs");
                    }
                }
            }
        }
    }
}

#[test]
fn unpruned_tree_matches_c_across_mv_cost_types_and_bias() {
    let t = build_tables();
    let mut rng = Rng(0x7777_8888_9999_aaaa);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &ct in &COST_TYPES {
            for &bias in &[0i32, 90, 110] {
                for &tight in &[false, true] {
                    let mut c = base_cell(bsize, false);
                    c.mv_cost_type = ct;
                    c.bias_fp = bias;
                    c.tight_limits = tight;
                    c.ref_mv = Mv { x: 7, y: -11 };
                    run_cell(&c, &p, &t, w, h, "unpruned/costtype");
                }
            }
        }
    }
}

/// The `ModeDecisionContext` arm at mcomp.c:706-723 — live only in
/// `PD_PASS_1` with a non-zero `mvp_th`. It can shorten `round` to 1 or 2,
/// which changes how many refinement iterations run and therefore the MV.
#[test]
fn unpruned_tree_matches_c_on_the_mvp_th_arm() {
    let t = build_tables();
    let mut rng = Rng(0xcafe_f00d_dead_10cc);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &mvp_th in &[0i32, 1, 10, 50, 99] {
            for &hp_mv_th in &[0i32, 4, 16, 64] {
                for &dist in &[0u32, 1, 1000, 100_000, u32::MAX / 2] {
                    let mut c = base_cell(bsize, false);
                    c.with_ctx = true;
                    c.mvp_th = mvp_th;
                    c.hp_mv_th = hp_mv_th;
                    c.best_fp_mvp_dist = dist;
                    c.best_fp_mvp = Mv { x: -16, y: 40 };
                    run_cell(&c, &p, &t, w, h, "unpruned/mvpth");
                }
            }
        }
    }
}

/// `ctx->fp_me_dist` is what the pruned entry point writes back, and MD reads
/// it for the `abs_th_mult` gate on the NEXT candidate. Drive it explicitly.
#[test]
fn pruned_tree_writes_the_same_fp_me_dist_as_c() {
    let t = build_tables();
    let mut rng = Rng(0x2468_ace0_1357_bdf9);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        let mut c = base_cell(bsize, true);
        c.with_ctx = true;
        run_cell(&c, &p, &t, w, h, "pruned/fpmedist");
    }
}

/// A broad randomised sweep: every knob drawn independently, so combinations
/// the hand-written loops above do not enumerate still get exercised.
#[test]
fn randomised_sweep_matches_c() {
    let t = build_tables();
    let mut rng = Rng(0xfeed_face_0f0f_0f0f);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for _ in 0..60 {
            let pruned = rng.below(2) == 0;
            let mut c = base_cell(bsize, pruned);
            c.allow_hp = rng.below(2) == 0;
            c.forced_stop = rng.below(4) as i32;
            c.iters_per_step = 1 + rng.below(3) as i32;
            c.pred_variance_th = [0i32, 1, 64, 4096][rng.below(4) as usize];
            c.abs_th_mult = [0u8, 1, 8, 200][rng.below(4) as usize];
            c.round_dev_th = [-100i32, 0, 3, 1000][rng.below(4) as usize];
            c.skip_diag_refinement = rng.below(6) as u8;
            c.subpel_search_type = 1 + rng.below(3) as i32;
            c.bias_fp = [0i32, 60, 95, 105, 150][rng.below(5) as usize];
            c.mv_cost_type = COST_TYPES[rng.below(6) as usize];
            c.error_per_bit = [0i32, 3, 40, 200][rng.below(4) as usize];
            c.early_exit_th = [50i32, 500, 1000, 4000][rng.below(4) as usize];
            c.start_mv = Mv {
                x: (rng.below(49) as i32 - 24) as i16,
                y: (rng.below(49) as i32 - 24) as i16,
            };
            c.ref_mv = Mv {
                x: (rng.below(33) as i32 - 16) as i16,
                y: (rng.below(33) as i32 - 16) as i16,
            };
            c.tight_limits = rng.below(3) == 0;
            c.with_ctx = rng.below(2) == 0;
            c.mvp_th = [0i32, 5, 40][rng.below(3) as usize];
            c.hp_mv_th = [0i32, 8, 32][rng.below(3) as usize];
            c.best_fp_mvp_dist = [0u32, 100, 10_000][rng.below(3) as usize];
            c.best_fp_mvp = Mv { x: 8, y: -8 };
            // Null tables are only legal on the arms that never dereference
            // them — see the note in pruned_tree_matches_c_across_mv_cost_types.
            c.with_tables = rng.below(4) != 0 || c.mv_cost_type == MvCostType::Entropy;
            run_cell(&c, &p, &t, w, h, "random");
        }
    }
}

/// `svt_aom_fp_mv_err_cost` is the third exported symbol: the full-pel MD ME
/// rate term (`product_coding_loop.c:1816/1890/2040/2920`). It is the whole
/// five-way `MV_COST_TYPE` dispatch, so drive every arm, both the populated
/// and the NULL `mvcost`, and MV differences of both signs.
#[test]
fn fp_mv_err_cost_matches_c() {
    let t = build_tables();
    let mut rng = Rng(0x3141_5926_5358_9793);
    for &ct in &COST_TYPES {
        for &epb in &[0i32, 1, 7, 40, 128, 255, 1023] {
            for &with_tables in &[true, false] {
                if !with_tables && ct == MvCostType::Entropy {
                    continue;
                }
                for _ in 0..64 {
                    let mv = Mv {
                        x: (rng.below(4001) as i32 - 2000) as i16,
                        y: (rng.below(4001) as i32 - 2000) as i16,
                    };
                    let rmv = Mv {
                        x: (rng.below(4001) as i32 - 2000) as i16,
                        y: (rng.below(4001) as i32 - 2000) as i16,
                    };
                    let cres = cref::fp_mv_err_cost(
                        (i32::from(mv.x), i32::from(mv.y)),
                        (i32::from(rmv.x), i32::from(rmv.y)),
                        mv_cost_type_index(ct),
                        &t.joint,
                        if with_tables {
                            Some((&*t.row, &*t.col))
                        } else {
                            None
                        },
                        epb,
                    );
                    let params = MvCostParams {
                        ref_mv: rmv,
                        mv_cost_type: ct,
                        tables: if with_tables { Some(&t.port) } else { None },
                        error_per_bit: epb,
                        early_exit_th: 0,
                    };
                    let rres = svtav1_encoder::md_subpel::fp_mv_err_cost(mv, &params);
                    assert_eq!(
                        rres, cres,
                        "fp_mv_err_cost {ct:?} epb={epb} tables={with_tables} mv={mv:?} ref={rmv:?}"
                    );
                }
            }
        }
    }
}

/// ANTI-VACUITY. Every assertion above would also pass if both sides returned
/// `start_mv` unrefined on every cell — the port's early-exit ladder and C's
/// agree trivially when nothing runs (`docs/WORKING-ON-THIS.md` §5: before you
/// trust a result, prove the probe fires). This test measures that the search
/// actually searches:
///
/// * the winning MV must differ from `start_mv` on a substantial fraction of
///   cells (so the refinement loop ran and moved),
/// * the winner must land on a FRACTIONAL position on a substantial fraction
///   (so half/quarter/eighth-pel candidates are being scored, not just the
///   full-pel seed),
/// * the pruned and unpruned trees must disagree somewhere (so the two entry
///   points are genuinely different code paths and both are driven).
#[test]
fn the_search_actually_searches() {
    let t = build_tables();
    let mut rng = Rng(0x00c0_ffee_00c0_ffee);
    let mut cells = 0usize;
    let mut moved = 0usize;
    let mut fractional = 0usize;
    let mut trees_disagree = 0usize;

    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &start in &[
            Mv { x: 0, y: 0 },
            Mv { x: 8, y: -8 },
            Mv { x: -16, y: 16 },
            Mv { x: 24, y: 24 },
        ] {
            let mut pruned_mv = None;
            for &pruned in &[true, false] {
                let mut c = base_cell(bsize, pruned);
                c.start_mv = start;
                run_cell(&c, &p, &t, w, h, "antivacuity");

                // Re-run the port alone to read the winner.
                let ms = SubpelSearchParams {
                    allow_hp: c.allow_hp,
                    forced_stop: c.forced_stop,
                    iters_per_step: c.iters_per_step,
                    pred_variance_th: c.pred_variance_th,
                    abs_th_mult: c.abs_th_mult,
                    round_dev_th: c.round_dev_th,
                    skip_diag_refinement: c.skip_diag_refinement,
                    search_stage: SPEL_ME,
                    list_idx: 0,
                    ref_idx: 0,
                    mv_limits: svtav1_encoder::md_subpel::SubpelMvLimits {
                        col_min: -4096,
                        col_max: 4096,
                        row_min: -4096,
                        row_max: 4096,
                    },
                };
                let var_params = SubpelSearchVarParams {
                    src: &p.src,
                    src_base: 0,
                    src_stride: p.src_stride,
                    ref_alloc: &p.ref_alloc,
                    ref_base: p.ref_base as i64,
                    ref_stride: p.ref_stride,
                    w,
                    h,
                    bias_fp: 0,
                    subpel_search_type: 3,
                };
                let mv_cost_params = MvCostParams {
                    ref_mv: c.ref_mv,
                    mv_cost_type: MvCostType::Entropy,
                    tables: Some(&t.port),
                    error_per_bit: c.error_per_bit,
                    early_exit_th: c.early_exit_th,
                };
                let (_e, st) = if pruned {
                    find_best_sub_pixel_tree_pruned(
                        None,
                        &ms,
                        &var_params,
                        &mv_cost_params,
                        start,
                        bsize,
                    )
                } else {
                    find_best_sub_pixel_tree(None, &ms, &var_params, &mv_cost_params, start, bsize)
                };
                cells += 1;
                if st.best_mv != start {
                    moved += 1;
                }
                if st.best_mv.x % 8 != 0 || st.best_mv.y % 8 != 0 {
                    fractional += 1;
                }
                match pruned_mv {
                    None => pruned_mv = Some(st.best_mv),
                    Some(pm) => {
                        if pm != st.best_mv {
                            trees_disagree += 1;
                        }
                    }
                }
            }
        }
    }

    assert!(cells >= 80, "only {cells} cells ran");
    assert!(
        moved * 4 >= cells,
        "the refinement moved the MV on only {moved} of {cells} cells — the \
         differential above may be comparing two no-ops"
    );
    assert!(
        fractional * 4 >= cells,
        "only {fractional} of {cells} winners are fractional — sub-pel \
         candidates are not being scored"
    );
    assert!(
        trees_disagree > 0,
        "the pruned and unpruned trees agreed on every one of {} pairs; one of \
         the two entry points may not be reached",
        cells / 2
    );
}

/// Which C kernels the tree actually runs on THIS host.
///
/// The differential above installs the `_c` variance kernels, because that is
/// what the port transcribes. In a real encode C installs
/// `svt_aom_mefn_ptr[bsize]`, i.e. whatever tier RTCD dispatched to. If the
/// two disagree, the port matches the `_c` oracle and NOT the shipping
/// encoder, which would be a byte-identity gap hiding behind a green
/// differential — so ask the question instead of assuming the answer.
///
/// The unpruned tree additionally calls the RTCD `svt_aom_upsampled_pred`
/// (which the shim cannot override — it is not reached through `vfp`), while
/// the port calls the ported `_c` transcription; the unpruned cells above
/// passing is already evidence that those two agree on this host.
#[test]
fn host_simd_tier_agrees_with_the_c_kernels() {
    let t = build_tables();
    let mut rng = Rng(0xabcd_1234_5678_9f01);
    for &bsize in &SIZES {
        let (w, h) = dims(bsize);
        let p = make_planes(w, h, 8, &mut rng, false);
        for &pruned in &[true, false] {
            for &start in &[Mv { x: 0, y: 0 }, Mv { x: 8, y: -16 }] {
                let mut c = base_cell(bsize, pruned);
                c.start_mv = start;
                let mut cp = cref::SubpelParams {
                    pruned,
                    use_rtcd: false,
                    use_ctx: false,
                    w,
                    h,
                    bsize: bsize as i32,
                    src_stride: p.src_stride,
                    ref_base: p.ref_base as i64,
                    ref_stride: p.ref_stride,
                    allow_hp: c.allow_hp,
                    forced_stop: c.forced_stop,
                    iters_per_step: c.iters_per_step,
                    pred_variance_th: c.pred_variance_th,
                    abs_th_mult: c.abs_th_mult,
                    round_dev_th: c.round_dev_th,
                    skip_diag_refinement: c.skip_diag_refinement,
                    search_stage: SPEL_ME,
                    list_idx: 0,
                    ref_idx: 0,
                    subpel_search_type: c.subpel_search_type,
                    bias_fp: c.bias_fp,
                    col_min: -4096,
                    col_max: 4096,
                    row_min: -4096,
                    row_max: 4096,
                    ref_mv: (0, 0),
                    mv_cost_type: mv_cost_type_index(MvCostType::Entropy),
                    error_per_bit: c.error_per_bit,
                    early_exit_th: c.early_exit_th,
                    pd_pass: 1,
                    mvp_th: 0,
                    hp_mv_th: 0,
                    best_fp_mvp_dist: 0,
                    best_fp_mvp: (0, 0),
                    start_mv: (i32::from(start.x), i32::from(start.y)),
                };
                let plain = cref::subpel_tree(
                    &cp,
                    &p.src,
                    &p.ref_alloc,
                    &t.joint,
                    Some((&*t.row, &*t.col)),
                );
                cp.use_rtcd = true;
                let rtcd = cref::subpel_tree(
                    &cp,
                    &p.src,
                    &p.ref_alloc,
                    &t.joint,
                    Some((&*t.row, &*t.col)),
                );
                assert_eq!(
                    plain, rtcd,
                    "this host's RTCD variance tier disagrees with the `_c` kernels \
                     for {w}x{h} pruned={pruned} start={start:?}; the port targets `_c`, \
                     so the shipping C encoder would pick a different sub-pel MV here"
                );
            }
        }
    }
}
