//! Differential parity: the MD motion-search cost model and the PME SAD
//! kernel (`svtav1-encoder/src/port_md/pme.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `svt_pme_sad_loop_kernel_c` | product_coding_loop.c:1775 |
//! | `svt_aom_fp_mv_err_cost` | mcomp.c:775 |
//! | `svt_aom_get_sad_per_bit` (+ `svt_av1_init_me_luts`) | mode_decision.c:2048 |
//!
//! The `_c` suffix on the kernel is deliberate — it is the scalar
//! reference, which is what the port transcribes. Driving the RTCD
//! pointer would compare against whichever SIMD variant this host picks.
//!
//! `svt_mv_err_cost` (mcomp.c:42) is `static INLINE`; every one of its six
//! arms is nevertheless reached at tier 1 through `svt_aom_fp_mv_err_cost`,
//! which is a one-line exported forward to it.
//!
//! One boundary is deliberately NOT compared: a per-component MV diff of
//! exactly `+-16384`. C's `svt_mv_cost` clips to `CLIP3(MV_LOW, MV_UPP, .)`
//! and indexes `comp_cost[i]` (offset by `MV_MAX = 16383`) with the
//! result, so `+16384` reads one element PAST the `MV_VALS`-long table.
//! The port clamps to `+-MV_MAX` instead. Comparing there would be
//! comparing against a C out-of-bounds read, and `is_valid_mv_diff`
//! (mode_decision.c:776) rejects candidates at that distance before the
//! search ever sees them.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::pme as rpme;
use svtav1_types::motion::Mv;

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

/// C `MV_MAX` = `(1 << MV_IN_USE_BITS) - 1`.
const MV_MAX: i32 = (1 << 14) - 1;
const MV_VALS: usize = (2 * MV_MAX + 1) as usize;

/// A synthetic, deterministic pair of MV cost tables in BOTH the shapes
/// the two sides need: C's flat `[i32; MV_VALS]` pair plus the port's
/// `MvCostTables`. The values are arbitrary but identical.
struct Tables {
    joint: [i32; 4],
    comp0: Vec<i32>,
    comp1: Vec<i32>,
    port: rpme::MvCostTable,
}

fn build_tables(seed: u64) -> Tables {
    let mut rng = Rng(seed);
    let mut joint = [0i32; 4];
    for j in joint.iter_mut() {
        *j = rng.below(4096) as i32;
    }
    let mut comp0 = vec![0i32; MV_VALS];
    let mut comp1 = vec![0i32; MV_VALS];
    // A smooth, magnitude-dependent cost keeps the comparison meaningful
    // (a random table would still compare equal but tells you nothing
    // about which index each side used).
    for i in 0..MV_VALS {
        let v = i as i32 - MV_MAX;
        comp0[i] = 100 + v.abs() / 3 + (i as i32 % 7);
        comp1[i] = 137 + v.abs() / 5 + (i as i32 % 11);
    }
    let port = rpme::MvCostTable {
        joint_cost: joint,
        comp_cost: [
            svtav1_encoder::intrabc::MvComponentCost::from_table(comp0.clone()),
            svtav1_encoder::intrabc::MvComponentCost::from_table(comp1.clone()),
        ],
    };
    Tables {
        joint,
        comp0,
        comp1,
        port,
    }
}

impl Tables {
    fn c_ref(&self) -> cmd::MvCostTablesRef<'_> {
        cmd::MvCostTablesRef {
            joint: &self.joint,
            comp0: &self.comp0,
            comp1: &self.comp1,
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_get_sad_per_bit — EXHAUSTIVE over the whole qindex range x bd.
// ---------------------------------------------------------------------------

#[test]
fn sad_per_bit_matches_c_exhaustively() {
    for qidx in 0..256usize {
        for hbd in [false, true] {
            assert_eq!(
                cmd::get_sad_per_bit(qidx as i32, hbd),
                rpme::get_sad_per_bit(qidx, hbd),
                "svt_aom_get_sad_per_bit(qidx={qidx}, hbd={hbd})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_fp_mv_err_cost — all six MV_COST_TYPE arms, with and without a
// cost table.
// ---------------------------------------------------------------------------

#[test]
fn fp_mv_err_cost_matches_c_across_every_cost_type() {
    let t = build_tables(0xFEED_2026_0831_0002);
    let mut rng = Rng(0xC057_2026_0831_0002);
    let types = [
        (0, rpme::MvCostType::Entropy),
        (1, rpme::MvCostType::L1LowRes),
        (2, rpme::MvCostType::L1MidRes),
        (3, rpme::MvCostType::L1HdRes),
        (4, rpme::MvCostType::Opt),
        (5, rpme::MvCostType::None),
    ];
    let mut nonzero = 0usize;
    let mut checked = 0usize;
    let cref = t.c_ref();
    for _ in 0..2000 {
        // Keep |diff| strictly below MV_MAX so neither side is near C's
        // one-past-the-end index (see the module doc).
        let mv = Mv {
            x: (rng.below(8000) as i32 - 4000) as i16,
            y: (rng.below(8000) as i32 - 4000) as i16,
        };
        let rmv = Mv {
            x: (rng.below(8000) as i32 - 4000) as i16,
            y: (rng.below(8000) as i32 - 4000) as i16,
        };
        let epb = 1 + rng.below(4096) as i32;
        for (ct_c, ct_r) in types {
            // `use_tables == false` is compared only for the arms that
            // never touch the table. C's ENTROPY arm guards on
            // `if (mvcost)`, but `mvcost` is an ARRAY member and can
            // never be null, so with NULL component pointers C
            // dereferences them: driving it that way SIGSEGVs the test
            // binary. That zero-return is dead code in C.
            let table_choices: &[bool] = if ct_c == 0 { &[true] } else { &[false, true] };
            for &use_tables in table_choices {
                let c = cmd::fp_mv_err_cost(
                    (mv.x, mv.y),
                    (rmv.x, rmv.y),
                    ct_c,
                    epb,
                    if use_tables { Some(&cref) } else { None },
                );
                let params = rpme::MvCostParams {
                    ref_mv: rmv,
                    mv_cost_type: ct_r,
                    tables: if use_tables { Some(&t.port) } else { None },
                    error_per_bit: epb,
                    early_exit_th: 0,
                };
                let r = rpme::fp_mv_err_cost(mv, &params);
                assert_eq!(
                    c, r,
                    "svt_aom_fp_mv_err_cost(mv={mv:?}, ref={rmv:?}, type={ct_c}, \
                     epb={epb}, tables={use_tables})"
                );
                if c != 0 {
                    nonzero += 1;
                }
                checked += 1;
            }
        }
    }
    // 6 cost types x 2 table choices, minus the ENTROPY-without-table
    // combination that C cannot survive.
    assert_eq!(checked, 2000 * (6 * 2 - 1));
    assert!(
        nonzero > checked / 4,
        "positive control: only {nonzero} of {checked} costs were non-zero"
    );
}

// ---------------------------------------------------------------------------
// svt_pme_sad_loop_kernel_c — randomized pixel data over a spread of block
// sizes, search rectangles and search steps.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn one_kernel_case(
    rng: &mut Rng,
    t: &Tables,
    block_width: usize,
    block_height: usize,
    search_area_width: i16,
    search_area_height: i16,
    search_step: i16,
    mv_cost_type: i32,
    port_cost_type: rpme::MvCostType,
) -> (cmd::PmeBest, rpme::PmeBest) {
    let src_stride = block_width + 8;
    let src: Vec<u8> = (0..src_stride * (block_height + 4))
        .map(|_| rng.below(256) as u8)
        .collect();
    // The reference plane must cover the whole search rectangle plus the
    // block, in BOTH dimensions, plus the left/top margin the offset eats.
    let ref_stride = block_width + search_area_width as usize + 32;
    let ref_rows = block_height + search_area_height as usize + 32;
    let ref_buf: Vec<u8> = (0..ref_stride * ref_rows)
        .map(|_| rng.below(256) as u8)
        .collect();
    let ref_offset = 8 * ref_stride + 8;

    let ref_mv = Mv {
        x: (rng.below(512) as i32 - 256) as i16,
        y: (rng.below(512) as i32 - 256) as i16,
    };
    let epb = 1 + rng.below(2048) as i32;
    let start_x = (rng.below(9) as i32 - 4) as i16;
    let start_y = (rng.below(9) as i32 - 4) as i16;
    let mvx = (rng.below(256) as i32 - 128) as i16;
    let mvy = (rng.below(256) as i32 - 128) as i16;

    let mut cbest = cmd::PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    let cref = t.c_ref();
    cmd::pme_sad_loop_kernel(
        (ref_mv.x, ref_mv.y),
        mv_cost_type,
        epb,
        Some(&cref),
        &src,
        src_stride,
        &ref_buf,
        ref_offset,
        ref_stride,
        block_height,
        block_width,
        &mut cbest,
        start_x,
        start_y,
        search_area_width,
        search_area_height,
        search_step,
        mvx,
        mvy,
    );

    let params = rpme::MvCostParams {
        ref_mv,
        mv_cost_type: port_cost_type,
        tables: Some(&t.port),
        error_per_bit: epb,
        early_exit_th: 0,
    };
    let mut rbest = rpme::PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    rpme::pme_sad_loop_kernel(
        &params,
        &src,
        src_stride,
        &ref_buf[ref_offset..],
        ref_stride,
        block_height,
        block_width,
        &mut rbest,
        start_x,
        start_y,
        search_area_width,
        search_area_height,
        search_step,
        mvx,
        mvy,
    );

    (cbest, rbest)
}

#[test]
fn pme_sad_loop_kernel_matches_c() {
    let t = build_tables(0xB10C_2026_0831_0003);
    let mut rng = Rng(0x5AD0_2026_0831_0003);
    let sizes: [(usize, usize); 8] = [
        (4, 4),
        (8, 8),
        (8, 16),
        (16, 8),
        (16, 16),
        (32, 16),
        (32, 32),
        (64, 64),
    ];
    let mut improved = 0usize;
    let mut checked = 0usize;
    for (bw, bh) in sizes {
        for saw in [8i16, 9, 15, 16, 17, 24] {
            for sah in [1i16, 3, 8] {
                for step in [1i16, 2, 4] {
                    for (ct_c, ct_r) in [(0, rpme::MvCostType::Entropy), (4, rpme::MvCostType::Opt)]
                    {
                        let (c, r) =
                            one_kernel_case(&mut rng, &t, bw, bh, saw, sah, step, ct_c, ct_r);
                        assert_eq!(
                            c.cost, r.cost,
                            "cost: {bw}x{bh} saw={saw} sah={sah} step={step} type={ct_c}"
                        );
                        assert_eq!(
                            c.mvx, r.mvx,
                            "mvx: {bw}x{bh} saw={saw} sah={sah} step={step} type={ct_c}"
                        );
                        assert_eq!(
                            c.mvy, r.mvy,
                            "mvy: {bw}x{bh} saw={saw} sah={sah} step={step} type={ct_c}"
                        );
                        if c.cost != u32::MAX {
                            improved += 1;
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 8 * 6 * 3 * 3 * 2);
    // Positive control: a kernel that never entered its inner body would
    // leave cost at u32::MAX in every case and still "match".
    assert!(
        improved > checked * 3 / 4,
        "positive control: only {improved} of {checked} cases updated the best cost"
    );
}

/// The `search_area_width < 8` case: C's guard `continue`s for EVERY x,
/// so the kernel scans nothing at all and leaves the caller's best
/// untouched. Pinned explicitly because "matches C" and "does nothing"
/// are indistinguishable without saying which one is expected.
#[test]
fn pme_sad_loop_kernel_narrow_search_area_scans_nothing_like_c() {
    let t = build_tables(0x2A2A_2026_0831_0004);
    let mut rng = Rng(0x2A2A_2026_0831_0004);
    for saw in [1i16, 2, 7] {
        let (c, r) = one_kernel_case(
            &mut rng,
            &t,
            16,
            16,
            saw,
            4,
            1,
            0,
            rpme::MvCostType::Entropy,
        );
        assert_eq!(c.cost, u32::MAX, "C scanned a <8-wide area (saw={saw})");
        assert_eq!(r.cost, c.cost);
        assert_eq!(r.mvx, c.mvx);
        assert_eq!(r.mvy, c.mvy);
    }
}
