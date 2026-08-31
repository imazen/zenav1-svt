//! Differential parity for `svtav1_encoder::inter_me::obmc_search` — the OBMC
//! half of `av1me.c` and the four C_DEFAULT kernels it drives — against the
//! REAL exported C symbols. **Evidence tier 1**
//! (`rust/docs/WORKING-ON-THIS.md` §4).
//!
//! Oracles: `svt_aom_obmc_sadWxH_c`, `svt_aom_obmc_varianceWxH_c`,
//! `svt_aom_obmc_sub_pixel_varianceWxH_c`, `svt_aom_convolve8_horiz_c`,
//! `svt_aom_convolve8_vert_c`, `svt_aom_upsampled_pred_c`,
//! `svt_av1_obmc_full_pixel_search` and
//! `svt_av1_find_best_obmc_sub_pixel_tree_up`.
//!
//! The two search drivers are the whole point: they compose the kernels, the
//! MV-cost tables and the search-range clamping, so agreeing on them is a much
//! stronger statement than agreeing on the kernels alone.

use svtav1_cref as cref;
use svtav1_cref::inter_me as cme;
use svtav1_encoder::entropy::mv_coding::{
    CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_OFFSET_BITS, MvSubpelPrecision, NmvComponent,
    NmvContext,
};
use svtav1_encoder::inter_me::obmc_search as obmc;
use svtav1_encoder::intrabc;
use svtav1_types::motion::{FullMvLimits, Mv};
use svtav1_types::tables::interp::InterpKernel;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn noise(rng: &mut Rng, n: usize) -> Vec<u8> {
    (0..n).map(|_| rng.byte()).collect()
}

/// `wsrc` / `mask` in the ranges the real OBMC path produces: `mask` is a
/// weight scaled by 4096 and `wsrc` is the weighted source, also scaled.
fn wsrc_mask(rng: &mut Rng, n: usize) -> (Vec<i32>, Vec<i32>) {
    let mask: Vec<i32> = (0..n).map(|_| rng.below(4097) as i32).collect();
    let wsrc: Vec<i32> = (0..n).map(|_| rng.below(255 * 4096 + 1) as i32).collect();
    (wsrc, mask)
}

/// The ten (w, h) the C shim instantiates, with their `BlockSize` enum values.
const SIZES: [(usize, usize, i32); 10] = [
    (4, 4, 0),
    (4, 8, 1),
    (8, 4, 2),
    (8, 8, 3),
    (8, 16, 4),
    (16, 8, 5),
    (16, 16, 6),
    (16, 32, 7),
    (32, 16, 8),
    (32, 32, 9),
];

#[test]
fn obmc_sad_matches_c() {
    let mut rng = Rng(0xC4_2001);
    for &(w, h, _) in &SIZES {
        for _ in 0..24 {
            let stride = w + rng.below(24) as usize;
            let pre = noise(&mut rng, stride * h + 64);
            let (wsrc, mask) = wsrc_mask(&mut rng, w * h);
            assert_eq!(
                obmc::obmc_sad(&pre, stride, &wsrc, &mask, w, h),
                cme::obmc_sad(&pre, stride, &wsrc, &mask, w, h),
                "obmc_sad {w}x{h} stride {stride}"
            );
        }
    }
}

#[test]
fn obmc_variance_matches_c() {
    let mut rng = Rng(0xC4_2002);
    for &(w, h, _) in &SIZES {
        for _ in 0..24 {
            let stride = w + rng.below(24) as usize;
            let pre = noise(&mut rng, stride * h + 64);
            let (wsrc, mask) = wsrc_mask(&mut rng, w * h);
            assert_eq!(
                obmc::obmc_variance_wxh(&pre, stride, &wsrc, &mask, w, h),
                cme::obmc_variance(&pre, stride, &wsrc, &mask, w, h),
                "obmc_variance {w}x{h} stride {stride}"
            );
        }
    }
}

#[test]
fn obmc_sub_pixel_variance_matches_c() {
    let mut rng = Rng(0xC4_2003);
    for &(w, h, _) in &SIZES {
        for xo in 0..8usize {
            for yo in 0..8usize {
                let stride = w + 8 + rng.below(16) as usize;
                let pre = noise(&mut rng, stride * (h + 2) + 64);
                let (wsrc, mask) = wsrc_mask(&mut rng, w * h);
                assert_eq!(
                    obmc::obmc_sub_pixel_variance(&pre, stride, xo, yo, &wsrc, &mask, w, h),
                    cme::obmc_sub_pixel_variance(&pre, stride, xo, yo, &wsrc, &mask, w, h),
                    "obmc_subpel_variance {w}x{h} off ({xo},{yo})"
                );
            }
        }
    }
}

#[test]
fn convolve8_matches_c() {
    let mut rng = Rng(0xC4_2004);
    for case in 0..64 {
        let w = 4 << (case % 5);
        let h = 4 << ((case / 5) % 5);
        let stride = w + 32;
        // Enough rows/cols around the block for the 8-tap window.
        let alloc_rows = h + 32;
        let src = noise(&mut rng, stride * alloc_rows);
        let base = (16 * stride + 16) as i64;
        let filters = obmc::av1_get_filter(obmc::USE_8_TAPS);
        let kernel: &InterpKernel = &filters[rng.below(16) as usize];

        let mut got = vec![0u8; w * h];
        let mut want = vec![0u8; w * h];
        obmc::convolve8_horiz(&src, base, stride, &mut got, w, kernel, w, h);
        cme::convolve8_horiz(&src, base, stride, &mut want, w, kernel, w, h);
        assert_eq!(got, want, "convolve8_horiz {w}x{h}");

        let mut got = vec![0u8; w * h];
        let mut want = vec![0u8; w * h];
        obmc::convolve8_vert(&src, base, stride, &mut got, w, kernel, w, h);
        cme::convolve8_vert(&src, base, stride, &mut want, w, kernel, w, h);
        assert_eq!(got, want, "convolve8_vert {w}x{h}");
    }
}

#[test]
fn upsampled_pred_matches_c() {
    let mut rng = Rng(0xC4_2005);
    for &(w, h, _) in &SIZES {
        for &subpel_search in &[obmc::USE_2_TAPS, obmc::USE_4_TAPS, obmc::USE_8_TAPS] {
            for sx in 0..8i32 {
                for sy in 0..8i32 {
                    let stride = w + 32;
                    let src = noise(&mut rng, stride * (h + 40));
                    let base = (20 * stride + 16) as i64;
                    let mut got = vec![0u8; w * h];
                    let mut want = vec![0u8; w * h];
                    obmc::upsampled_pred(&mut got, w, h, sx, sy, &src, base, stride, subpel_search);
                    cme::upsampled_pred(&mut want, w, h, sx, sy, &src, base, stride, subpel_search);
                    assert_eq!(
                        got, want,
                        "upsampled_pred {w}x{h} sub ({sx},{sy}) search {subpel_search}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The two search drivers.
// ---------------------------------------------------------------------------

fn random_cdf(rng: &mut Rng, out: &mut [u16]) {
    let nsymbs = out.len() - 1;
    loop {
        let mut cuts: Vec<u16> = (0..nsymbs - 1)
            .map(|_| 1 + rng.below(32766) as u16)
            .collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            out[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    out[nsymbs - 1] = 0;
    out[nsymbs] = rng.below(33) as u16;
}

fn random_nmv_context(rng: &mut Rng) -> NmvContext {
    let mut c = NmvComponent {
        classes_cdf: [0; MV_CLASSES + 1],
        class0_fp_cdf: [[0; MV_FP_SIZE + 1]; CLASS0_SIZE],
        fp_cdf: [0; MV_FP_SIZE + 1],
        sign_cdf: [0; 3],
        class0_hp_cdf: [0; 3],
        hp_cdf: [0; 3],
        class0_cdf: [0; CLASS0_SIZE + 1],
        bits_cdf: [[0; 3]; MV_OFFSET_BITS],
    };
    random_cdf(rng, &mut c.classes_cdf);
    for row in &mut c.class0_fp_cdf {
        random_cdf(rng, row);
    }
    random_cdf(rng, &mut c.fp_cdf);
    random_cdf(rng, &mut c.sign_cdf);
    random_cdf(rng, &mut c.class0_hp_cdf);
    random_cdf(rng, &mut c.hp_cdf);
    random_cdf(rng, &mut c.class0_cdf);
    for row in &mut c.bits_cdf {
        random_cdf(rng, row);
    }
    let mut ctx = NmvContext::default();
    random_cdf(rng, &mut ctx.joints_cdf);
    ctx.comps = [c.clone(), c];
    ctx
}

fn flatten_nmv(ctx: &NmvContext) -> Vec<u16> {
    let mut flat = Vec::new();
    flat.extend_from_slice(&ctx.joints_cdf);
    for comp in &ctx.comps {
        flat.extend_from_slice(&comp.classes_cdf);
        for row in &comp.class0_fp_cdf {
            flat.extend_from_slice(row);
        }
        flat.extend_from_slice(&comp.fp_cdf);
        flat.extend_from_slice(&comp.sign_cdf);
        flat.extend_from_slice(&comp.class0_hp_cdf);
        flat.extend_from_slice(&comp.hp_cdf);
        flat.extend_from_slice(&comp.class0_cdf);
        for b in &comp.bits_cdf {
            flat.extend_from_slice(b);
        }
    }
    flat
}

/// One random NMV context, turned into BOTH sides' cost tables: C's through
/// the real `svt_aom_estimate_mv_rate` chain, the port's through
/// `intrabc::build_nmv_cost_table`. The test asserts they agree BEFORE using
/// them, so a cost-table difference is reported as a precondition failure
/// rather than mis-attributed to the OBMC search.
struct Costs {
    c_joint: [i32; 4],
    c0: Vec<i32>,
    c1: Vec<i32>,
    port: intrabc::MvCostTables,
}

fn make_costs(rng: &mut Rng) -> Costs {
    let ndvc = if rng.below(2) == 0 {
        NmvContext::default()
    } else {
        random_nmv_context(rng)
    };
    let flat = flatten_nmv(&ndvc);
    let c = cref::estimate_mv_rate(false, true, false, None, Some(&flat), -1);
    let (c0, c1) = c.dv_costs.split_at(cref::MV_VALS);
    let mut joint = [0i32; 4];
    joint.copy_from_slice(&c.dv_joint[..4]);
    Costs {
        c_joint: joint,
        c0: c0.to_vec(),
        c1: c1.to_vec(),
        port: intrabc::build_nmv_cost_table(&ndvc, MvSubpelPrecision::None),
    }
}

impl Costs {
    fn check_tables_agree(&self) {
        let mv_max = (cref::MV_VALS / 2) as i32;
        for &v in &[-4096i32, -257, -8, -1, 0, 1, 8, 257, 4096] {
            assert_eq!(
                self.port.comp_cost[0].cost(v),
                self.c0[(mv_max + v) as usize],
                "PRECONDITION: comp0 cost table differs at {v}"
            );
            assert_eq!(
                self.port.comp_cost[1].cost(v),
                self.c1[(mv_max + v) as usize],
                "PRECONDITION: comp1 cost table differs at {v}"
            );
        }
        assert_eq!(
            self.port.joint_cost[..4],
            self.c_joint,
            "PRECONDITION: joint cost table differs"
        );
    }
    fn as_ref_cost(&self) -> cme::RefMvCost<'_> {
        cme::RefMvCost {
            joint: &self.c_joint,
            comp0: &self.c0,
            comp1: &self.c1,
        }
    }
}

struct Scene {
    pre: Vec<u8>,
    base: i64,
    stride: usize,
    wsrc: Vec<i32>,
    mask: Vec<i32>,
}

/// A reference plane with 96 px of slack on every side, so a full-pel MV of
/// +-32 plus the 8-tap subpel window stays inside the allocation.
fn scene(rng: &mut Rng, w: usize, h: usize) -> Scene {
    let pad = 96usize;
    let stride = w + 2 * pad;
    let rows = h + 2 * pad;
    let (wsrc, mask) = wsrc_mask(rng, w * h);
    Scene {
        pre: noise(rng, stride * rows),
        base: (pad * stride + pad) as i64,
        stride,
        wsrc,
        mask,
    }
}

#[test]
fn obmc_full_pixel_search_matches_c() {
    let mut rng = Rng(0xC4_2006);
    for case in 0..48 {
        let (w, h, bsize) = SIZES[case % SIZES.len()];
        let sc = scene(&mut rng, w, h);
        let costs = make_costs(&mut rng);
        costs.check_tables_agree();

        let limits = FullMvLimits {
            col_min: -(8 + rng.below(24) as i32),
            col_max: 8 + rng.below(24) as i32,
            row_min: -(8 + rng.below(24) as i32),
            row_max: 8 + rng.below(24) as i32,
        };
        let mvp = Mv {
            x: rng.below(17) as i16 - 8,
            y: rng.below(17) as i16 - 8,
        };
        let ref_mv = Mv {
            x: (rng.below(17) as i16 - 8) * 8,
            y: (rng.below(17) as i16 - 8) * 8,
        };
        let sadpb = 1 + rng.below(64) as i32;
        let errorperbit = 1 + rng.below(4096) as i32;
        let approx = case % 3 == 0;
        let range = 1 + rng.below(8) as i32;
        let diag = case % 2 == 0;

        let s = obmc::ObmcSearch {
            pre: &sc.pre,
            pre_base: sc.base,
            pre_stride: sc.stride,
            wsrc: &sc.wsrc,
            mask: &sc.mask,
            w,
            h,
            mv_limits: limits,
            approx_inter_rate: approx,
            mv_cost: &costs.port,
            errorperbit,
        };
        let mut dst = Mv::ZERO;
        let got = obmc::obmc_full_pixel_search(&s, mvp, sadpb, ref_mv, &mut dst, range, diag);

        let mut wsrc_c = sc.wsrc.clone();
        let mut mask_c = sc.mask.clone();
        let want = cme::obmc_full_pixel_search(
            &sc.pre,
            sc.base,
            sc.stride,
            &mut wsrc_c,
            &mut mask_c,
            bsize,
            (i32::from(mvp.x), i32::from(mvp.y)),
            sadpb,
            (i32::from(ref_mv.x), i32::from(ref_mv.y)),
            cme::RefMvLimits {
                col_min: limits.col_min,
                col_max: limits.col_max,
                row_min: limits.row_min,
                row_max: limits.row_max,
            },
            &costs.as_ref_cost(),
            errorperbit,
            approx,
            range,
            diag,
        );
        assert_eq!(
            (got, i32::from(dst.x), i32::from(dst.y)),
            want,
            "obmc_full_pixel_search case {case}: {w}x{h} approx {approx} range {range} diag {diag}"
        );
    }
}

#[test]
fn obmc_sub_pixel_tree_up_matches_c() {
    let mut rng = Rng(0xC4_2007);
    for case in 0..64 {
        let (w, h, bsize) = SIZES[case % SIZES.len()];
        let sc = scene(&mut rng, w, h);
        let costs = make_costs(&mut rng);
        costs.check_tables_agree();

        let limits = FullMvLimits {
            col_min: -(8 + rng.below(16) as i32),
            col_max: 8 + rng.below(16) as i32,
            row_min: -(8 + rng.below(16) as i32),
            row_max: 8 + rng.below(16) as i32,
        };
        let best = Mv {
            x: rng.below(9) as i16 - 4,
            y: rng.below(9) as i16 - 4,
        };
        let ref_mv = Mv {
            x: (rng.below(9) as i16 - 4) * 8,
            y: (rng.below(9) as i16 - 4) * 8,
        };
        let errorperbit = 1 + rng.below(4096) as i32;
        let approx = case % 3 == 0;
        let allow_hp = case % 2 == 0;
        let forced_stop = (case % 3) as i32;
        let iters = 1 + (case % 2) as i32;
        // The LIVE path: the sole C call site (mode_decision.c:2148) passes
        // USE_8_TAPS. The `use_accurate_subpel_search == 0` branch is covered
        // separately by `obmc_sub_pixel_tree_up_uas0_matches_c_where_the_
        // dispatch_is_faithful`, because on aarch64 the C binary drives a
        // MIS-WIRED `osvf` there (see `obmc_osvf_dispatch_control`).
        let uas = obmc::USE_8_TAPS;

        let s = obmc::ObmcSearch {
            pre: &sc.pre,
            pre_base: sc.base,
            pre_stride: sc.stride,
            wsrc: &sc.wsrc,
            mask: &sc.mask,
            w,
            h,
            mv_limits: limits,
            approx_inter_rate: approx,
            mv_cost: &costs.port,
            errorperbit,
        };
        let mut mv = best;
        let got = obmc::find_best_obmc_sub_pixel_tree_up(
            &s,
            &mut mv,
            ref_mv,
            allow_hp,
            errorperbit,
            forced_stop,
            iters,
            uas,
        );

        let mut wsrc_c = sc.wsrc.clone();
        let mut mask_c = sc.mask.clone();
        let want = cme::obmc_sub_pixel_tree_up(
            &sc.pre,
            sc.base,
            sc.stride,
            &mut wsrc_c,
            &mut mask_c,
            bsize,
            (i32::from(best.x), i32::from(best.y)),
            (i32::from(ref_mv.x), i32::from(ref_mv.y)),
            allow_hp,
            errorperbit,
            forced_stop,
            iters,
            cme::RefMvLimits {
                col_min: limits.col_min,
                col_max: limits.col_max,
                row_min: limits.row_min,
                row_max: limits.row_max,
            },
            &costs.as_ref_cost(),
            approx,
            uas,
        );
        assert_eq!(
            (
                got.besterr,
                i32::from(mv.x),
                i32::from(mv.y),
                got.distortion,
                got.sse
            ),
            want,
            "obmc subpel tree case {case}: {w}x{h} hp {allow_hp} stop {forced_stop} iters {iters} uas {uas}"
        );
    }
}

/// A fixed probe input for the dispatch control below — same shape for every
/// size so the comparison is apples to apples.
fn control_probe(w: usize, h: usize) -> (Vec<u8>, usize, Vec<i32>, Vec<i32>) {
    let mut rng = Rng(0xC4_2100 + (w * 64 + h) as u64);
    let stride = w + 16;
    let pre = noise(&mut rng, stride * (h + 4) + 64);
    let (wsrc, mask) = wsrc_mask(&mut rng, w * h);
    (pre, stride, wsrc, mask)
}

/// True when this host's RTCD `osvf` for `bsize` agrees with the `_c` kernel
/// the port transcribes.
fn osvf_dispatch_is_faithful(bsize: i32, w: usize, h: usize) -> bool {
    let (pre, stride, wsrc, mask) = control_probe(w, h);
    let c = cme::obmc_sub_pixel_variance(&pre, stride, 3, 5, &wsrc, &mask, w, h);
    let r = cme::obmc_kernel_rtcd(2, bsize, &pre, stride, 3, 5, &wsrc, &mask);
    c == r
}

/// **A measured upstream defect, pinned so it cannot rot.**
///
/// `Source/Lib/Codec/aom_dsp_rtcd.c:731-750` wires EVERY
/// `svt_aom_obmc_sub_pixel_variance` size from 4x16 through 128x128 to
/// `svt_aom_obmc_sub_pixel_variance4x8_neon` — a copy-paste error in the NEON
/// dispatch table. (`obmc_sad` and `obmc_variance` above it are wired
/// correctly, per size.) On aarch64 the shipping encoder therefore scores the
/// OBMC sub-pel search of an 8x16 block with a 4x8 variance.
///
/// This test asserts, per size, EITHER that the dispatch is faithful (x86,
/// where the SSE4 table is correct) OR that the RTCD kernel returns exactly
/// what the `_c` **4x8** kernel returns on the same inputs — the precise
/// signature of that aliasing. It therefore fails the day upstream fixes the
/// table, which is when the exclusion in the `uas == 0` test below must go.
#[test]
fn obmc_osvf_dispatch_control() {
    let mut aliased = 0usize;
    let mut faithful = 0usize;
    for &(w, h, bsize) in &SIZES {
        let (pre, stride, wsrc, mask) = control_probe(w, h);
        for xo in 0..8usize {
            for yo in 0..8usize {
                let c = cme::obmc_sub_pixel_variance(&pre, stride, xo, yo, &wsrc, &mask, w, h);
                let r = cme::obmc_kernel_rtcd(2, bsize, &pre, stride, xo, yo, &wsrc, &mask);
                if c == r {
                    faithful += 1;
                    continue;
                }
                let as_4x8 = cme::obmc_sub_pixel_variance(&pre, stride, xo, yo, &wsrc, &mask, 4, 8);
                assert_eq!(
                    r, as_4x8,
                    "{w}x{h} off ({xo},{yo}): the RTCD osvf differs from _c but is NOT the 4x8 \
                     kernel either — this is a NEW divergence, not aom_dsp_rtcd.c:731-750"
                );
                aliased += 1;
            }
        }
    }
    assert!(
        faithful + aliased == SIZES.len() * 64,
        "every (size, offset) cell must be classified"
    );
    assert!(faithful > 0, "4x4 and 4x8 are correctly wired on every ISA");
}

/// The `use_accurate_subpel_search == 0` branch of the sub-pixel tree, against
/// the C binary — for exactly the sizes where this host's `osvf` dispatch is
/// faithful. On a size where `obmc_osvf_dispatch_control` proved the dispatch
/// is the 4x8 alias, the C binary is not an oracle for this branch (it is
/// computing a different function), so comparing to it would assert the
/// upstream bug rather than the port. The test still runs on every size where
/// the oracle IS valid, and fails if that set is empty.
#[test]
fn obmc_sub_pixel_tree_up_uas0_matches_c_where_the_dispatch_is_faithful() {
    let mut rng = Rng(0xC4_2008);
    let mut compared = 0usize;
    for case in 0..48 {
        let (w, h, bsize) = SIZES[case % SIZES.len()];
        if !osvf_dispatch_is_faithful(bsize, w, h) {
            continue;
        }
        let sc = scene(&mut rng, w, h);
        let costs = make_costs(&mut rng);
        costs.check_tables_agree();
        let limits = FullMvLimits {
            col_min: -(8 + rng.below(16) as i32),
            col_max: 8 + rng.below(16) as i32,
            row_min: -(8 + rng.below(16) as i32),
            row_max: 8 + rng.below(16) as i32,
        };
        let best = Mv {
            x: rng.below(9) as i16 - 4,
            y: rng.below(9) as i16 - 4,
        };
        let ref_mv = Mv {
            x: (rng.below(9) as i16 - 4) * 8,
            y: (rng.below(9) as i16 - 4) * 8,
        };
        let errorperbit = 1 + rng.below(4096) as i32;
        let approx = case % 3 == 0;
        let allow_hp = case % 2 == 0;
        let forced_stop = (case % 3) as i32;
        let iters = 1 + (case % 2) as i32;

        let s = obmc::ObmcSearch {
            pre: &sc.pre,
            pre_base: sc.base,
            pre_stride: sc.stride,
            wsrc: &sc.wsrc,
            mask: &sc.mask,
            w,
            h,
            mv_limits: limits,
            approx_inter_rate: approx,
            mv_cost: &costs.port,
            errorperbit,
        };
        let mut mv = best;
        let got = obmc::find_best_obmc_sub_pixel_tree_up(
            &s,
            &mut mv,
            ref_mv,
            allow_hp,
            errorperbit,
            forced_stop,
            iters,
            0,
        );
        let mut wsrc_c = sc.wsrc.clone();
        let mut mask_c = sc.mask.clone();
        let want = cme::obmc_sub_pixel_tree_up(
            &sc.pre,
            sc.base,
            sc.stride,
            &mut wsrc_c,
            &mut mask_c,
            bsize,
            (i32::from(best.x), i32::from(best.y)),
            (i32::from(ref_mv.x), i32::from(ref_mv.y)),
            allow_hp,
            errorperbit,
            forced_stop,
            iters,
            cme::RefMvLimits {
                col_min: limits.col_min,
                col_max: limits.col_max,
                row_min: limits.row_min,
                row_max: limits.row_max,
            },
            &costs.as_ref_cost(),
            approx,
            0,
        );
        assert_eq!(
            (
                got.besterr,
                i32::from(mv.x),
                i32::from(mv.y),
                got.distortion,
                got.sse
            ),
            want,
            "obmc subpel tree (uas=0) case {case}: {w}x{h}"
        );
        compared += 1;
    }
    assert!(
        compared > 0,
        "no size on this host has a faithful osvf dispatch — the oracle is gone, not the test"
    );
}

// ---------------------------------------------------------------------------
// Two controls on the ORACLE itself, both added after a cross-ISA run on
// 2026-08-31 showed this file green on aarch64-darwin and broken on
// x86_64-linux for reasons that had nothing to do with either implementation.
// ---------------------------------------------------------------------------

/// 256-byte-aligned scratch so a tap array can be placed at a chosen residue.
#[repr(align(256))]
struct AlignedTaps([i16; 256]);

/// The C convolve entries take no phase index: they recover both the 16-phase
/// table and the phase from the **address** of the filter pointer
/// (`convolve.c:54` `get_filter_base`, `ptr & ~0xFF`, with the comment "this
/// assumes that the filter table is 256-byte aligned"). Every real call site
/// satisfies that; a differential shim that forwards a Rust `&[i16; 8]`
/// straight through does not, and the taps C actually applies become the ones
/// at `addr - (addr % 16)`.
///
/// That is not a theoretical hazard. **MEASURED 2026-08-31:** the Rust
/// `SUB_PEL_FILTERS_8` static landed at `%16 == 0` in the aarch64-darwin build
/// of this very test binary — so `convolve8_matches_c` passed — and at
/// `%16 == 8` in the x86_64-linux one, where C silently applied the taps 8
/// bytes earlier and the test failed with a whole-block value mismatch. A
/// coin-flip of static layout, not an ISA property.
///
/// So this control feeds the SAME taps to the oracle from every 2-byte residue
/// in a 256-byte window and asserts the oracle is invariant and equal to the
/// port. It fails the moment the shim goes back to forwarding the caller's
/// pointer — on either ISA, whichever way that binary's statics land.
#[test]
fn convolve8_oracle_is_alignment_invariant() {
    let mut rng = Rng(0xC4_2010);
    let filters = obmc::av1_get_filter(obmc::USE_8_TAPS);

    for &(w, h) in &[(4usize, 4usize), (8, 8), (16, 8), (32, 32)] {
        let stride = w + 32;
        let src = noise(&mut rng, stride * (h + 32));
        let base = (16 * stride + 16) as i64;

        for phase in 0..16usize {
            let taps = filters[phase];

            let mut want_h = vec![0u8; w * h];
            let mut want_v = vec![0u8; w * h];
            obmc::convolve8_horiz(&src, base, stride, &mut want_h, w, &taps, w, h);
            obmc::convolve8_vert(&src, base, stride, &mut want_v, w, &taps, w, h);

            // Every 2-byte residue in a 256-byte window, i.e. every `% 16`
            // class the linker can hand us, including the 15 that break a
            // raw-pointer shim.
            for slot in 0..124usize {
                let mut buf = AlignedTaps([0i16; 256]);
                buf.0[slot..slot + 8].copy_from_slice(&taps);
                let k: &[i16; 8] = buf.0[slot..slot + 8].try_into().unwrap();
                let addr = k.as_ptr() as usize;

                let mut got = vec![0u8; w * h];
                cme::convolve8_horiz(&src, base, stride, &mut got, w, k, w, h);
                assert_eq!(
                    got,
                    want_h,
                    "convolve8_horiz {w}x{h} phase {phase} taps at %256={} %16={}: \
                     the C oracle read a different kernel than it was handed",
                    addr % 256,
                    addr % 16
                );

                let mut got = vec![0u8; w * h];
                cme::convolve8_vert(&src, base, stride, &mut got, w, k, w, h);
                assert_eq!(
                    got,
                    want_v,
                    "convolve8_vert {w}x{h} phase {phase} taps at %256={} %16={}: \
                     the C oracle read a different kernel than it was handed",
                    addr % 256,
                    addr % 16
                );
            }
        }
    }
}

/// Minimal reproducer for the second half of that cross-ISA run: the shim's
/// `ref_upsampled_pred` did not initialize RTCD, and `svt_aom_upsampled_pred_c`
/// reaches bare `svt_memcpy` on its `subpel == (0, 0)` arm. `svt_memcpy` is an
/// RTCD function pointer in `.bss` (`common_dsp_rtcd.h:1083`), NULL until
/// `svt_aom_setup_common_rtcd_internal` runs; the header even offers a
/// null-safe `SVT_MEMCPY` that variance.c:92 does not use. On aarch64 NEON
/// devirtualization rewrites `svt_memcpy` to the concrete `svt_memcpy_neon`
/// (`common_dsp_rtcd_neon_devirt.h:266`), so the pointer never exists and the
/// bug cannot fire; on x86-64 the call lands at `rip = 0x0`.
///
/// **Observed failure before the fix (x86_64-linux, 2026-08-31):**
/// `SIGSEGV`, `#0 0x0 in ?? ()`, `#1 svt_aom_upsampled_pred_c`,
/// `#2 ref_upsampled_pred (… ref_base=736, ref_stride=36, subpel_search=1)`.
///
/// This test exists as its own cell so the reproducer is the FIRST C call in
/// its process — `cargo nextest` gives every test its own process, so a later
/// reordering of `upsampled_pred_matches_c`'s loops cannot quietly warm RTCD up
/// first and hide the regression. It can only fail on x86-64 (CI's
/// `ubuntu-latest` is x86-64); on aarch64 it is a passing no-op by construction.
#[test]
fn upsampled_pred_cold_rtcd_zero_subpel() {
    let mut rng = Rng(0xC4_2011);
    let (w, h) = (4usize, 4usize);
    let stride = w + 32;
    let src = noise(&mut rng, stride * (h + 40));
    let base = (20 * stride + 16) as i64;

    let mut got = vec![0u8; w * h];
    let mut want = vec![0u8; w * h];
    obmc::upsampled_pred(&mut got, w, h, 0, 0, &src, base, stride, obmc::USE_2_TAPS);
    cme::upsampled_pred(&mut want, w, h, 0, 0, &src, base, stride, obmc::USE_2_TAPS);
    assert_eq!(got, want, "upsampled_pred {w}x{h} at subpel (0, 0)");
}
