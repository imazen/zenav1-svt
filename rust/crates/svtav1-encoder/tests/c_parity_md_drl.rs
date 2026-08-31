//! Differential parity: DRL selection
//! (`svtav1-encoder/src/port_md/drl.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the oracle is the
//! EXPORTED `svt_aom_choose_best_av1_mv_pred` (mode_decision.c:527),
//! driven over randomized ref-MV stacks, ref-MV counts, cost tables and
//! DRL fac-bit tables. It reaches the `static INLINE` `av1_drl_ctx`
//! (rd_cost.h:85) and `svt_av1_mv_bit_cost` / `_light` (rd_cost.c:59-78)
//! along the way, so one driver covers all four C functions.
//!
//! Both outputs are compared, including on the paths where C writes
//! NEITHER (`shut_fast_rate`) — the test seeds them with sentinel values
//! and asserts both sides preserve them.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::drl as rdrl;
use svtav1_encoder::port_md::pme::{MV_MAX, MV_VALS, MvCostTable};
use svtav1_types::motion::{CandidateMv, MAX_REF_MV_STACK_SIZE, Mv};
use svtav1_types::prediction::PredictionMode;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn mode_from_u8(v: u8) -> PredictionMode {
    const MODES: [PredictionMode; 12] = [
        PredictionMode::NearestMv,
        PredictionMode::NearMv,
        PredictionMode::GlobalMv,
        PredictionMode::NewMv,
        PredictionMode::NearestNearestMv,
        PredictionMode::NearNearMv,
        PredictionMode::NearestNewMv,
        PredictionMode::NewNearestMv,
        PredictionMode::NearNewMv,
        PredictionMode::NewNearMv,
        PredictionMode::GlobalGlobalMv,
        PredictionMode::NewNewMv,
    ];
    MODES[v as usize]
}

/// Deterministic MV cost tables in both shapes.
struct Tables {
    joint: [i32; 4],
    comp0: Vec<i32>,
    comp1: Vec<i32>,
    port: MvCostTable,
}

fn build_tables() -> Tables {
    let joint = [311i32, 907, 1103, 1499];
    let mut comp0 = vec![0i32; MV_VALS];
    let mut comp1 = vec![0i32; MV_VALS];
    for i in 0..MV_VALS {
        let v = i as i32 - MV_MAX;
        comp0[i] = 64 + v.abs() / 2 + (i as i32 % 13);
        comp1[i] = 96 + v.abs() / 4 + (i as i32 % 17);
    }
    let port = MvCostTable {
        joint,
        comp: [comp0.clone(), comp1.clone()],
    };
    Tables {
        joint,
        comp0,
        comp1,
        port,
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_one(
    t: &Tables,
    shut_fast_rate: bool,
    approx_inter_rate: u8,
    stack: &[(u32, u32, i32); MAX_REF_MV_STACK_SIZE],
    ref_mv_count: u8,
    ref_frame: i32,
    mode: PredictionMode,
    mv0: Mv,
    mv1: Mv,
    drl_fac_bits: &[[i32; 2]; 3],
    seed_drl: u8,
    seed_pred: [u32; 2],
) -> (u8, [u32; 2]) {
    let mut c_drl = seed_drl;
    let mut c_pred = seed_pred;
    cmd::choose_best_av1_mv_pred(
        shut_fast_rate,
        approx_inter_rate,
        stack,
        ref_mv_count,
        ref_frame,
        mode as u8,
        mv0.as_int(),
        mv1.as_int(),
        &t.joint,
        &t.comp0,
        &t.comp1,
        drl_fac_bits,
        &mut c_drl,
        &mut c_pred,
    );

    let mut port_stack = [CandidateMv::default(); MAX_REF_MV_STACK_SIZE];
    for (i, s) in stack.iter().enumerate() {
        port_stack[i] = CandidateMv {
            this_mv: Mv::from_int(s.0),
            comp_mv: Mv::from_int(s.1),
            weight: s.2,
        };
    }
    let ctx = rdrl::ChooseDrlCtx {
        shut_fast_rate,
        approx_inter_rate,
        ref_mv_stack: &port_stack,
        ref_mv_count,
        nmv_cost: &t.port,
        drl_mode_fac_bits: drl_fac_bits,
    };
    let mut r_drl = seed_drl;
    let mut r_pred = [Mv::from_int(seed_pred[0]), Mv::from_int(seed_pred[1])];
    rdrl::choose_best_av1_mv_pred(&ctx, mode, mv0, mv1, &mut r_drl, &mut r_pred);

    let r_pred_int = [r_pred[0].as_int(), r_pred[1].as_int()];
    assert_eq!(
        c_drl, r_drl,
        "drl_index: sfr={shut_fast_rate} air={approx_inter_rate} \
         cnt={ref_mv_count} rf={ref_frame} mode={mode:?}"
    );
    assert_eq!(
        c_pred, r_pred_int,
        "pred_mv: sfr={shut_fast_rate} air={approx_inter_rate} \
         cnt={ref_mv_count} rf={ref_frame} mode={mode:?}"
    );
    (c_drl, c_pred)
}

#[test]
fn choose_best_av1_mv_pred_matches_c() {
    let t = build_tables();
    let mut rng = Rng(0xD121_2026_0831_0005);
    let mut checked = 0usize;
    let mut nonzero_drl = 0usize;
    let mut drl_values = [false; 4];

    for _ in 0..4000 {
        let mut stack = [(0u32, 0u32, 0i32); MAX_REF_MV_STACK_SIZE];
        for s in stack.iter_mut() {
            let mv = |r: &mut Rng| {
                Mv {
                    x: (r.below(2048) as i32 - 1024) as i16,
                    y: (r.below(2048) as i32 - 1024) as i16,
                }
                .as_int()
            };
            let this = mv(&mut rng);
            let comp = mv(&mut rng);
            // Weights straddle REF_CAT_LEVEL so av1_drl_ctx takes all
            // three of its branches.
            let w = (rng.below(1400)) as i32;
            *s = (this, comp, w);
        }
        let ref_mv_count = rng.below(9) as u8;
        // Single refs 1..7 and compound types 8..28 — the whole
        // MODE_CTX_REF_FRAMES domain the C indexes.
        let ref_frame = 1 + rng.below(27) as i32;
        let mode = mode_from_u8(rng.below(12) as u8);
        let mv0 = Mv {
            x: (rng.below(4096) as i32 - 2048) as i16,
            y: (rng.below(4096) as i32 - 2048) as i16,
        };
        let mv1 = Mv {
            x: (rng.below(4096) as i32 - 2048) as i16,
            y: (rng.below(4096) as i32 - 2048) as i16,
        };
        let mut fac = [[0i32; 2]; 3];
        for r in fac.iter_mut() {
            r[0] = rng.below(4000) as i32;
            r[1] = rng.below(4000) as i32;
        }
        let approx = rng.below(3) as u8;
        let shut = rng.below(8) == 0;

        let (drl, _) = compare_one(
            &t,
            shut,
            approx,
            &stack,
            ref_mv_count,
            ref_frame,
            mode,
            mv0,
            mv1,
            &fac,
            0xEE,
            [0xDEAD_BEEF, 0xFEED_FACE],
        );
        if !shut && drl != 0xEE {
            drl_values[usize::from(drl.min(3))] = true;
            if drl != 0 {
                nonzero_drl += 1;
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 4000);
    // Positive controls: the search must actually visit DRL indices other
    // than 0, or a port that always answered 0 would pass.
    assert!(
        nonzero_drl > 100,
        "positive control: only {nonzero_drl} non-zero DRL picks"
    );
    assert_eq!(
        drl_values,
        [true, true, true, false],
        "positive control: DRL indices seen (max_drl_index caps at 3, so \
         index 3 is unreachable)"
    );
}

/// `shut_fast_rate` writes NEITHER output. Seeded with sentinels so
/// "matches C" cannot be satisfied by both sides writing the same wrong
/// thing.
#[test]
fn shut_fast_rate_leaves_both_outputs_untouched_like_c() {
    let t = build_tables();
    let stack = [(0x0001_0002u32, 0x0003_0004u32, 700i32); MAX_REF_MV_STACK_SIZE];
    let fac = [[100i32, 200]; 3];
    let (drl, pred) = compare_one(
        &t,
        true,
        0,
        &stack,
        4,
        1,
        PredictionMode::NewMv,
        Mv { x: 8, y: 8 },
        Mv::ZERO,
        &fac,
        0x5A,
        [0x1111_2222, 0x3333_4444],
    );
    assert_eq!(drl, 0x5A);
    assert_eq!(pred, [0x1111_2222, 0x3333_4444]);
}

/// `approx_inter_rate > 1` short-circuits to DRL 0 with slot 0's MVs,
/// without entering the loop. Distinguished from `== 1`, which runs the
/// loop with the light MV cost.
#[test]
fn approx_inter_rate_arms_are_distinct() {
    let t = build_tables();
    let mut stack = [(0u32, 0u32, 0i32); MAX_REF_MV_STACK_SIZE];
    for (i, s) in stack.iter_mut().enumerate() {
        *s = (
            Mv {
                x: (i as i16) * 16,
                y: (i as i16) * -16,
            }
            .as_int(),
            Mv {
                x: (i as i16) * 4,
                y: (i as i16) * 4,
            }
            .as_int(),
            700,
        );
    }
    let fac = [[100i32, 900]; 3];
    let (drl_gt1, pred_gt1) = compare_one(
        &t,
        false,
        2,
        &stack,
        8,
        1,
        PredictionMode::NewMv,
        Mv { x: 96, y: -96 },
        Mv::ZERO,
        &fac,
        0xEE,
        [0, 0],
    );
    assert_eq!(drl_gt1, 0);
    assert_eq!(pred_gt1, [stack[0].0, stack[0].1]);

    let (drl_eq1, _) = compare_one(
        &t,
        false,
        1,
        &stack,
        8,
        1,
        PredictionMode::NewMv,
        Mv { x: 96, y: -96 },
        Mv::ZERO,
        &fac,
        0xEE,
        [0, 0],
    );
    // The `== 1` arm searches, so it can land somewhere other than 0.
    // (Both sides agree — compare_one asserted that; this documents that
    // the two arms are genuinely different code paths.)
    assert!(drl_eq1 <= 2);
}
