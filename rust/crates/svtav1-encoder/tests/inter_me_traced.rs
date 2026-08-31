//! Coverage for the parts of `motion_estimation.c` that are C `static` and so
//! have **no exported symbol to differentially test against**.
//!
//! Read the tier label on each test before quoting it:
//!
//! * **Tier 1 (restricted domain)** — `hme_level_0`, `hme_level_1` and
//!   `prehme_core` are byte-for-byte the same C body as the EXPORTED
//!   `hme_level_2` apart from three documented deltas (which padding constant,
//!   how the search-area origin is seeded, and the x4 / x2 / x1 rescale of the
//!   reported centre). Configured so those deltas cancel, the port's level 0/1
//!   and pre-HME must equal the REAL C `hme_level_2` — which is what these
//!   tests assert. They drive C code, over a subset of the input domain.
//! * **Tier 4 (traced vectors)** — everything else: values hand-derived from
//!   the C source, asserted against the port. This is the weakest tier
//!   (`rust/docs/WORKING-ON-THIS.md` §4): it proves the port matches a reading
//!   of the C, not the C binary. Never quote a green run here as parity.
//! * **Structural invariants** — the eight-point and single-point search
//!   blocks are two C implementations of the same accumulation; they must
//!   agree. That catches an offset or MV-packing error without any oracle.

use svtav1_cref::inter_me as cref;
use svtav1_encoder::inter_me::candidates::{
    compute_distortion, construct_me_candidate_array, construct_me_candidate_array_mrp_off,
    construct_me_candidate_array_single_ref, perform_gm_detection,
};
use svtav1_encoder::inter_me::context::*;
use svtav1_encoder::inter_me::hme::{
    get_worst_quadrant, get_zz_sad, hme_level_0, hme_level_1, hme_prune_ref_and_adjust_sr, prehme_core,
    set_final_search_centre_sb,
};
use svtav1_encoder::inter_me::integer::{
    apply_me_sa_boost, get_eight_search_point_results_block, get_search_point_results_block, me_prune_ref,
};
use svtav1_encoder::inter_me::sad::pack_mv;
use svtav1_encoder::inter_me::{b64, motion_estimation_b64};

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

struct TestPlane {
    data: Vec<u8>,
    org: usize,
    stride: usize,
    width: u16,
    height: u16,
    border: u16,
}

impl TestPlane {
    fn new(rng: &mut Rng, width: u16, height: u16, border: u16) -> Self {
        let stride = width as usize + 2 * border as usize;
        let rows = height as usize + 2 * border as usize;
        Self {
            data: noise(rng, stride * rows + 64),
            org: border as usize * stride + border as usize,
            stride,
            width,
            height,
            border,
        }
    }
    fn view(&self) -> Plane<'_> {
        Plane {
            data: &self.data,
            org: self.org,
            stride: self.stride,
            width: self.width,
            height: self.height,
            border: self.border,
        }
    }
    fn px(&self, x: usize, y: usize) -> u8 {
        self.data[self.org + y * self.stride + x]
    }
}

fn one_buf(b: &[u8], stride: usize) -> MeSrcBufs<'_> {
    MeSrcBufs {
        b64: b,
        b64_stride: stride,
        quarter: b,
        quarter_stride: stride,
        sixteenth: b,
        sixteenth_stride: stride,
    }
}

/// **Tier 1 (restricted domain).** With `border == 64` (so `pad == 63 ==
/// BLOCK_SIZE_64 - 1`), `num_hme_sa_{w,h} == 1` and `sr_{w,h} == 0`, C's
/// `hme_level_0` reduces to C's `hme_level_2` with `hme_l1_sc == (0, 0)`,
/// except that it reports the centre multiplied by 4. So the port's level 0
/// must equal the REAL C `hme_level_2` under that transform.
#[test]
fn hme_level_0_equals_c_hme_level_2_in_the_shared_domain() {
    let mut rng = Rng(0xC4_1001);
    for case in 0..64 {
        let width = 64 + 16 * (rng.below(6) as u16);
        let height = 64 + 16 * (rng.below(6) as u16);
        let plane = TestPlane::new(&mut rng, width, height, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);
        let bw = 16 + 16 * rng.below(4) as u32;
        let bh = 16 + 16 * rng.below(4) as u32;
        let org_x = (rng.below(((width as u32 / bw).max(1)) as u64) as i16) * bw as i16;
        let org_y = (rng.below(((height as u32 / bh).max(1)) as u64) as i16) * bh as i16;
        let sa_w = 1 + rng.below(24) as i16;
        let sa_h = 1 + rng.below(12) as i16;
        let method = if case % 2 == 0 { FULL_SAD_SEARCH } else { SUB_SAD_SEARCH };

        let mut ctx = MeContext::default();
        ctx.hme_search_method = method;
        ctx.num_hme_sa_w = 1;
        ctx.num_hme_sa_h = 1;
        let src = one_buf(&src_buf, src_stride);

        let (sad, scx, scy) = hme_level_0(&ctx, &src, org_x, org_y, bw, bh, sa_w, sa_h, &plane.view(), 0, 0);
        let (c_sad, c_x, c_y) = cref::hme_level_2(
            &src_buf,
            src_stride,
            method,
            &plane.data,
            plane.org,
            plane.stride as u16,
            width,
            height,
            org_x,
            org_y,
            bw,
            bh,
            sa_w,
            sa_h,
            0,
            0,
        );
        assert_eq!(sad, c_sad, "case {case}: level-0 SAD");
        assert_eq!(scx, c_x.wrapping_mul(4), "case {case}: level-0 x (x4 of C level-2)");
        assert_eq!(scy, c_y.wrapping_mul(4), "case {case}: level-0 y (x4 of C level-2)");
    }
}

/// **Tier 1 (restricted domain).** Same argument as level 0, but level 1 seeds
/// the origin from `hme_l0_sc` exactly as level 2 seeds it from `hme_l1_sc`,
/// and rescales by 2.
#[test]
fn hme_level_1_equals_c_hme_level_2_in_the_shared_domain() {
    let mut rng = Rng(0xC4_1002);
    for case in 0..64 {
        let width = 64 + 16 * (rng.below(6) as u16);
        let height = 64 + 16 * (rng.below(6) as u16);
        let plane = TestPlane::new(&mut rng, width, height, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);
        let bw = 16 + 16 * rng.below(4) as u32;
        let bh = 16 + 16 * rng.below(4) as u32;
        let org_x = (rng.below(((width as u32 / bw).max(1)) as u64) as i16) * bw as i16;
        let org_y = (rng.below(((height as u32 / bh).max(1)) as u64) as i16) * bh as i16;
        let sa_w = 1 + rng.below(24) as i16;
        let sa_h = 1 + rng.below(12) as i16;
        let l0x = rng.below(33) as i16 - 16;
        let l0y = rng.below(33) as i16 - 16;
        let method = if case % 2 == 0 { FULL_SAD_SEARCH } else { SUB_SAD_SEARCH };

        let mut ctx = MeContext::default();
        ctx.hme_search_method = method;
        let src = one_buf(&src_buf, src_stride);

        let (sad, scx, scy) = hme_level_1(&ctx, &src, org_x, org_y, bw, bh, &plane.view(), sa_w, sa_h, l0x, l0y);
        let (c_sad, c_x, c_y) = cref::hme_level_2(
            &src_buf,
            src_stride,
            method,
            &plane.data,
            plane.org,
            plane.stride as u16,
            width,
            height,
            org_x,
            org_y,
            bw,
            bh,
            sa_w,
            sa_h,
            l0x,
            l0y,
        );
        assert_eq!(sad, c_sad, "case {case}: level-1 SAD");
        assert_eq!(scx, c_x.wrapping_mul(2), "case {case}: level-1 x (x2 of C level-2)");
        assert_eq!(scy, c_y.wrapping_mul(2), "case {case}: level-1 y (x2 of C level-2)");
    }
}

/// **Tier 1 (restricted domain).** `prehme_core` is level 0 with `sr == 0`
/// and `num_hme_sa == 1` folded in, plus `skip_search_line`. It differs from
/// `hme_level_2` in TWO further places, both of which the domain below makes
/// inert — and the second one was found by this test failing:
///
/// * pre-HME does NOT round the search width up to a multiple of 8, and does
///   NOT apply the `sa_width & ~7` round-DOWN after the right-edge crop. So
///   the widths are passed as multiples of 8 AND the block is placed with the
///   whole search area inside the picture, so neither crop fires.
/// * `skip_search_line` is forced to 0 here; the non-zero path is covered at
///   tier 1 directly by `c_parity_inter_me::sad_loop_kernel_matches_c`.
#[test]
fn prehme_core_equals_c_hme_level_2_in_the_shared_domain() {
    let mut rng = Rng(0xC4_1003);
    let width = 256u16;
    let height = 256u16;
    for case in 0..64 {
        let plane = TestPlane::new(&mut rng, width, height, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);
        let bw = 16u32;
        let bh = 16u32;
        // Keep the whole search area inside the picture so neither the
        // left/right crop nor pre-HME's missing round-down can differ.
        let org_x = 64 + 16 * (rng.below(7) as i16);
        let org_y = 64 + 16 * (rng.below(7) as i16);
        let sa_w = 8 * (1 + rng.below(4) as u16);
        let sa_h = 1 + rng.below(12) as u16;
        let method = if case % 2 == 0 { FULL_SAD_SEARCH } else { SUB_SAD_SEARCH };

        let mut ctx = MeContext::default();
        ctx.hme_search_method = method;
        ctx.prehme_ctrl.skip_search_line = 0;
        ctx.prehme_data[0][0][0].sa = SearchArea { width: sa_w, height: sa_h };
        let src = one_buf(&src_buf, src_stride);

        prehme_core(&mut ctx, &src, org_x, org_y, bw, bh, &plane.view(), 0, 0, 0);
        let d = ctx.prehme_data[0][0][0];

        let (c_sad, c_x, c_y) = cref::hme_level_2(
            &src_buf,
            src_stride,
            method,
            &plane.data,
            plane.org,
            plane.stride as u16,
            width,
            height,
            org_x,
            org_y,
            bw,
            bh,
            sa_w as i16,
            sa_h as i16,
            0,
            0,
        );
        assert_eq!(d.sad, c_sad, "case {case}: pre-HME SAD");
        assert_eq!(d.best_mv.x, c_x.wrapping_mul(4), "case {case}: pre-HME x");
        assert_eq!(d.best_mv.y, c_y.wrapping_mul(4), "case {case}: pre-HME y");
        assert_eq!(d.valid, 1, "pre-HME must mark the slot valid");
    }
}

/// **Tier 1.** `get_zz_sad` is one `svt_nxm_sad_kernel` call at the block
/// origin with a `<<1` row subsample; the SAD itself is the exported C kernel.
#[test]
fn get_zz_sad_matches_the_c_kernel_it_wraps() {
    let mut rng = Rng(0xC4_1004);
    for _ in 0..64 {
        let plane = TestPlane::new(&mut rng, 128, 128, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);
        let ox = (rng.below(2) as u32) * 64;
        let oy = (rng.below(2) as u32) * 64;
        let src = one_buf(&src_buf, src_stride);

        let got = get_zz_sad(&plane.view(), &src, ox, oy, 64, 64);
        let base = plane.org + oy as usize * plane.stride + ox as usize;
        let want = cref::nxm_sad(&src_buf, src_stride * 2, &plane.data[base..], plane.stride * 2, 32, 64) << 1;
        assert_eq!(got, want);
    }
}

/// **Structural invariant.** `open_loop_me_get_eight_search_point_results_block`
/// and eight successive `open_loop_me_get_search_point_results_block` calls are
/// two C paths for the same accumulation. They must produce identical
/// `p_sb_best_sad` / `p_sb_best_mv` — including the packed MVs, which is what
/// catches a wrong `start_16x16_pos` map or a wrong `_MVXT` offset.
#[test]
fn eight_point_block_equals_eight_single_point_blocks() {
    let mut rng = Rng(0xC4_1005);
    for case in 0..24 {
        let plane = TestPlane::new(&mut rng, 192, 192, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);
        let src = one_buf(&src_buf, src_stride);
        let sub = case % 2 == 0;

        let base_x = rng.below(16) as i32 - 8;
        let base_y = rng.below(16) as i32 - 8;
        let ref_off = plane.org as i64 + 64 * plane.stride as i64 + 64;

        let mk = || {
            let mut c = MeContext::default();
            c.me_search_method = if sub { SUB_SAD_SEARCH } else { FULL_SAD_SEARCH };
            c.interpolated_full_stride[0][0] = plane.stride;
            c.integer_buffer_off[0][0] = ref_off;
            c.p_sb_best_sad[0][0] = [MAX_SAD_VALUE; SQUARE_PU_COUNT];
            c
        };

        let mut eight = mk();
        get_eight_search_point_results_block(&mut eight, &src, &plane.view(), 0, 0, 0, base_x, base_y);

        let mut singles = mk();
        for i in 0..8i64 {
            get_search_point_results_block(
                &mut singles,
                &src,
                &plane.view(),
                0,
                0,
                i,
                base_x + i as i32,
                base_y,
            );
        }

        assert_eq!(
            eight.p_sb_best_sad[0][0], singles.p_sb_best_sad[0][0],
            "case {case} (sub_sad={sub}): best SAD"
        );
        assert_eq!(
            eight.p_sb_best_mv[0][0], singles.p_sb_best_mv[0][0],
            "case {case} (sub_sad={sub}): best MV"
        );
    }
}

/// **Tier 4** (`apply_me_sa_boost`, motion_estimation.c:1163). The four
/// thresholds and the 3x5 multiplier table, hand-read from the C source. Note
/// index 1 is unreachable: the ladder yields 0, 2, 3 or 4.
#[test]
fn apply_me_sa_boost_matches_the_c_table() {
    // (hme_sad, boost) -> (width, height) for a 100x50 input.
    let cases: &[(u64, u8, i16, i16)] = &[
        (0, 1, 100, 50),                 // index 0 -> 1.0
        (2 * 64 * 64, 1, 100, 50),       // not > 2*4096 -> index 0
        (2 * 64 * 64 + 1, 1, 300, 150),  // index 2 -> 3.0
        (3 * 64 * 64 + 1, 1, 400, 200),  // index 3 -> 4.0
        (4 * 64 * 64 + 1, 1, 500, 250),  // index 4 -> 5.0
        (2 * 64 * 64 + 1, 2, 250, 125),  // boost 2, index 2 -> 2.5
        (3 * 64 * 64 + 1, 2, 350, 175),  // boost 2, index 3 -> 3.5
        (4 * 64 * 64 + 1, 2, 450, 225),  // boost 2, index 4 -> 4.5
        (2 * 64 * 64 + 1, 3, 200, 100),  // boost 3, index 2 -> 2.0
        (3 * 64 * 64 + 1, 3, 250, 125),  // boost 3, index 3 -> 2.5
        (4 * 64 * 64 + 1, 3, 350, 175),  // boost 3, index 4 -> 3.5
    ];
    for &(sad, boost, ew, eh) in cases {
        let mut w = 100i16;
        let mut h = 50i16;
        apply_me_sa_boost(&mut w, &mut h, sad, boost);
        assert_eq!((w, h), (ew, eh), "hme_sad {sad}, boost {boost}");
    }
}

/// **Tier 4** (`get_worst_quadrant`, motion_estimation.c:1737). C returns
/// without writing when the quadrant grid is not 2x2, and its LAST comparison
/// does not update `max_sad` — so quadrant (1,1) wins whenever it beats the
/// running max from the first three, which is the same thing here.
#[test]
fn get_worst_quadrant_picks_the_largest_sad() {
    let mut ctx = MeContext::default();
    ctx.num_hme_sa_w = 2;
    ctx.num_hme_sa_h = 2;
    // [w][h]
    ctx.hme_level0_sad[0][0] = [[10, 40], [30, 20]];
    assert_eq!(get_worst_quadrant(&ctx, 0, 0), Some((0, 1)));
    ctx.hme_level0_sad[0][0] = [[10, 20], [30, 40]];
    assert_eq!(get_worst_quadrant(&ctx, 0, 0), Some((1, 1)));
    ctx.hme_level0_sad[0][0] = [[0, 0], [0, 0]];
    // Nothing is > 0, so C leaves best_w/best_h at their seed of (0, 0).
    assert_eq!(get_worst_quadrant(&ctx, 0, 0), Some((0, 0)));
    ctx.num_hme_sa_w = 1;
    assert_eq!(get_worst_quadrant(&ctx, 0, 0), None, "non-2x2 grid: C asserts and returns");
}

/// **Tier 4** (`set_final_search_centre_sb`, motion_estimation.c:2026). Two
/// behaviours are pinned here because they look like bugs and are the oracle:
/// the level-2 quadrant scan seeds from (0,0) and skips (0,0) of the first row
/// only, and `hmeMvSad` is NOT reset per reference.
#[test]
fn set_final_search_centre_picks_the_min_sad_quadrant() {
    let mut ctx = MeContext::default();
    ctx.enable_hme_flag = true;
    ctx.enable_hme_level2_flag = true;
    ctx.num_of_list_to_search = 1;
    ctx.num_of_ref_pic_to_search = [2, 0];
    ctx.num_hme_sa_w = 2;
    ctx.num_hme_sa_h = 2;

    ctx.hme_level2_sad[0][0] = [[900, 100], [500, 700]];
    ctx.x_hme_level2_search_center[0][0] = [[1, 2], [3, 4]];
    ctx.y_hme_level2_search_center[0][0] = [[5, 6], [7, 8]];
    // ref 1: every quadrant worse than ref 0's winner.
    ctx.hme_level2_sad[0][1] = [[2000, 2100], [2200, 2300]];
    ctx.x_hme_level2_search_center[0][1] = [[9, 9], [9, 9]];
    ctx.y_hme_level2_search_center[0][1] = [[9, 9], [9, 9]];

    set_final_search_centre_sb(&mut ctx);

    // ref 0: seed (0,0)=900; the scan visits (1,0)=500, then row 1 from w=0:
    // (0,1)=100 wins, then (1,1)=700 loses.
    assert_eq!(ctx.search_results[0][0].hme_sad, 100);
    assert_eq!(ctx.search_results[0][0].hme_sc_x, 2);
    assert_eq!(ctx.search_results[0][0].hme_sc_y, 6);
    // ref 1: seed 2000, nothing better -> 2000.
    assert_eq!(ctx.search_results[0][1].hme_sad, 2000);
    assert_eq!(ctx.best_list_idx, 0);
    assert_eq!(ctx.best_ref_idx, 0, "ref 0 has the lower cost");
}

/// **Tier 4** (`me_prune_ref`, motion_estimation.c:1415). `hme_sad` becomes the
/// sum of the 64 8x8 best SADs in `tab8x8` order, `do_ref == 0` slots get the
/// `MAX_SAD_VALUE * 64` sentinel, and refs beyond the deviation threshold are
/// switched off. The threshold arithmetic is `(sad - best) * 100 > th * best`.
#[test]
fn me_prune_ref_sums_8x8_and_prunes_on_deviation() {
    let mut ctx = MeContext::default();
    ctx.num_of_list_to_search = 1;
    ctx.num_of_ref_pic_to_search = [2, 0];
    ctx.me_hme_prune_ctrls.enable_me_hme_ref_pruning = true;
    // 50 % deviation allowed.
    ctx.me_hme_prune_ctrls.prune_ref_if_me_sad_dev_bigger_than_th = 50;
    for i in 0..64 {
        ctx.p_sb_best_sad[0][0][PU_8X8_0 + i] = 10;
        ctx.p_sb_best_sad[0][1][PU_8X8_0 + i] = 16; // 60 % worse
    }
    for li in 0..2 {
        for ri in 0..4 {
            ctx.search_results[li][ri].do_ref = 1;
            ctx.search_results[li][ri].hme_sad = 0;
        }
    }
    me_prune_ref(&mut ctx);
    assert_eq!(ctx.search_results[0][0].hme_sad, 640);
    assert_eq!(ctx.search_results[0][1].hme_sad, 1024);
    assert_eq!(ctx.search_results[0][0].do_ref, 1, "the best ref is never pruned");
    assert_eq!(ctx.search_results[0][1].do_ref, 0, "(1024-640)*100 > 50*640");
    // A ref that was already off gets the sentinel, not a sum.
    let mut ctx2 = MeContext::default();
    ctx2.num_of_list_to_search = 1;
    ctx2.num_of_ref_pic_to_search = [1, 0];
    ctx2.search_results[0][0].do_ref = 0;
    me_prune_ref(&mut ctx2);
    assert_eq!(ctx2.search_results[0][0].hme_sad, u64::from(MAX_SAD_VALUE) * 64);
}

/// **Tier 4** (`hme_prune_ref_and_adjust_sr`, motion_estimation.c:2290). The
/// stationary branch wins over the low-SAD branch, and both write
/// `reduce_me_sr_divisor`.
#[test]
fn hme_prune_ref_and_adjust_sr_sets_the_divisors() {
    let mut ctx = MeContext::default();
    ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment = 1;
    ctx.me_sr_adjustment_ctrls.reduce_me_sr_based_on_mv_length_th = 4;
    ctx.me_sr_adjustment_ctrls.stationary_hme_sad_abs_th = 1000;
    ctx.me_sr_adjustment_ctrls.stationary_me_sr_divisor = 8;
    ctx.me_sr_adjustment_ctrls.reduce_me_sr_based_on_hme_sad_abs_th = 5000;
    ctx.me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad = 2;
    for li in 0..2 {
        for ri in 0..4 {
            ctx.search_results[li][ri].hme_sad = 20_000;
            ctx.search_results[li][ri].hme_sc_x = 100;
            ctx.search_results[li][ri].hme_sc_y = 100;
        }
    }
    // stationary: |mv| <= 4 and sad < 1000
    ctx.search_results[0][0].hme_sc_x = 3;
    ctx.search_results[0][0].hme_sc_y = -4;
    ctx.search_results[0][0].hme_sad = 999;
    // low sad but a big MV: the else-if branch
    ctx.search_results[0][1].hme_sad = 4999;
    hme_prune_ref_and_adjust_sr(&mut ctx);
    assert_eq!(ctx.reduce_me_sr_divisor[0][0], 8);
    assert_eq!(ctx.reduce_me_sr_divisor[0][1], 2);
    assert_eq!(ctx.reduce_me_sr_divisor[0][2], 1, "untouched refs keep the divisor of 1");
}

/// **Tier 4** (`init_me_hme_data`, motion_estimation.c:2788). The R2R guard:
/// every `p_sb_best_mv` entry is zeroed for EVERY list/ref, not just the ones
/// this b64 will search.
#[test]
fn init_me_hme_data_wipes_every_mv_slot() {
    let mut ctx = MeContext::default();
    ctx.enable_hme_flag = true;
    ctx.p_sb_best_mv[1][3][84] = 0xDEAD_BEEF;
    ctx.x_hme_level1_search_center[1][2][1][1] = 77;
    ctx.search_results[1][3].do_ref = 0;
    b64::init_me_hme_data(&mut ctx);
    assert_eq!(ctx.p_sb_best_mv[1][3][84], 0);
    assert_eq!(ctx.x_hme_level1_search_center[1][2][1][1], 0);
    assert_eq!(ctx.search_results[1][3].do_ref, 1);
    assert_eq!(ctx.search_results[1][3].hme_sad, u64::from(u32::MAX));
    assert_eq!(ctx.zz_sad[1][3], u32::MAX);
    assert_eq!(ctx.reduce_me_sr_divisor[1][3], 1);
    assert_eq!(ctx.search_results[1][3].list_i, 1);
    assert_eq!(ctx.search_results[1][3].ref_i, 3);
}

/// **Tier 4** (`me_static_b64_bypass`, motion_estimation.c:2832). A block that
/// is identical to list0/ref0 has zz SAD 0, so the bypass fires, publishes the
/// >>2 / >>4 / >>6 SAD ladder, and switches every farther reference off.
#[test]
fn me_static_b64_bypass_fires_on_an_identical_block() {
    let mut rng = Rng(0xC4_1006);
    let plane = TestPlane::new(&mut rng, 128, 128, 64);
    // The source block IS the reference block at (0,0): zz SAD == 0.
    let mut src_buf = vec![0u8; 64 * 64];
    for y in 0..64 {
        for x in 0..64 {
            src_buf[y * 64 + x] = plane.px(x, y);
        }
    }
    let src = one_buf(&src_buf, 64);
    let refs = MeRefs {
        arr: [
            [
                Some(MeDsRef {
                    picture: plane.view(),
                    quarter: plane.view(),
                    sixteenth: plane.view(),
                    picture_number: 0,
                }),
                None,
                None,
                None,
            ],
            [None, None, None, None],
        ],
    };

    let mut ctx = MeContext::default();
    ctx.me_static_b64_th = 100;
    ctx.num_of_list_to_search = 2;
    ctx.num_of_ref_pic_to_search = [1, 0];
    ctx.b64_width = 64;
    ctx.b64_height = 64;
    assert!(b64::me_static_b64_bypass(&mut ctx, &src, &refs, 0, 0));
    assert_eq!(ctx.zz_sad[0][0], 0);
    assert_eq!(ctx.p_sb_best_sad[0][0][PU_64X64], 0);
    assert_eq!(ctx.search_results[0][0].do_ref, 1);

    // A non-zero, below-threshold SAD exercises the shift ladder.
    let mut ctx = MeContext::default();
    ctx.me_static_b64_th = u32::MAX;
    ctx.num_of_list_to_search = 1;
    ctx.num_of_ref_pic_to_search = [1, 0];
    ctx.b64_width = 64;
    ctx.b64_height = 64;
    let mut shifted = src_buf.clone();
    shifted[0] = shifted[0].wrapping_add(64);
    let src2 = one_buf(&shifted, 64);
    assert!(b64::me_static_b64_bypass(&mut ctx, &src2, &refs, 0, 0));
    let zz = ctx.zz_sad[0][0];
    assert_eq!(ctx.p_sb_best_sad[0][0][PU_64X64], zz);
    assert_eq!(ctx.p_sb_best_sad[0][0][PU_32X32_0], zz >> 2);
    assert_eq!(ctx.p_sb_best_sad[0][0][PU_16X16_0], zz >> 4);
    assert_eq!(ctx.p_sb_best_sad[0][0][PU_8X8_0], zz >> 6);
    assert_eq!(ctx.p_sb_best_sad[0][0][SQUARE_PU_COUNT - 1], zz >> 6);
}

fn pic_params() -> MePicParams {
    MePicParams {
        picture_number: 4,
        aligned_width: 128,
        aligned_height: 128,
        enhanced_width: 128,
        enhanced_height: 128,
        ahd_error: u32::MAX,
        input_resolution: 0,
        enable_me_8x8: true,
        enable_me_16x16: true,
        max_number_of_pus_per_sb: 85,
        hierarchical_levels: 0,
        similar_brightness_refs: false,
        frame_is_boosted: false,
        frame_is_leaf: false,
        gm_enabled: false,
        only_l_bwd: false,
        max_cand: 23,
        max_refs: 7,
        max_l0: 4,
        b64_geom_width: 64,
        b64_geom_height: 64,
        input_width: 128,
        input_height: 128,
    }
}

/// **Tier 4** (`construct_me_candidate_array_single_ref`,
/// motion_estimation.c:2446). One unipred candidate per PU, with C's
/// `ref0_list`/`ref1_list` both 0.
#[test]
fn single_ref_candidate_array_matches_the_traced_shape() {
    let pic = pic_params();
    let mut ctx = MeContext::default();
    ctx.num_of_list_to_search = 1;
    ctx.num_of_ref_pic_to_search = [1, 0];
    ctx.search_results[0][0].do_ref = 1;
    for i in 0..SQUARE_PU_COUNT {
        ctx.p_sb_best_sad[0][0][i] = 1000 + i as u32;
        ctx.p_sb_best_mv[0][0][i] = pack_mv(-3, 5);
    }
    let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
    construct_me_candidate_array_single_ref(&pic, &mut ctx, 1, &mut out);

    assert_eq!(out.total_me_candidate_index[0], 1);
    let c = out.me_candidate_array[0];
    assert_eq!((c.direction, c.ref_idx_l0, c.ref_idx_l1, c.ref0_list, c.ref1_list), (0, 0, 0, 0, 0));
    assert_eq!(out.me_mv_array[0].x, -3);
    assert_eq!(out.me_mv_array[0].y, 5);
    // me_distortion is written in z-order via z_to_raster.
    assert_eq!(ctx.me_distortion[0], 1000);
}

/// **Tier 4** (`construct_me_candidate_array_mrp_off`,
/// motion_estimation.c:2335). Both lists allowed => two unipred candidates and
/// one BI_PRED, and the bitfield truncation makes `ref0_list == 0` for the L1
/// unipred candidate even though C assigns the literal 24.
#[test]
fn mrp_off_candidate_array_emits_two_unipred_and_one_bipred() {
    let pic = pic_params();
    let mut ctx = MeContext::default();
    ctx.num_of_list_to_search = 2;
    ctx.num_of_ref_pic_to_search = [1, 1];
    ctx.search_results[0][0].do_ref = 1;
    ctx.search_results[1][0].do_ref = 1;
    for i in 0..SQUARE_PU_COUNT {
        ctx.p_sb_best_sad[0][0][i] = 500;
        ctx.p_sb_best_sad[1][0][i] = 700;
        ctx.p_sb_best_mv[0][0][i] = pack_mv(1, 2);
        ctx.p_sb_best_mv[1][0][i] = pack_mv(-1, -2);
    }
    let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
    construct_me_candidate_array_mrp_off(&pic, &mut ctx, 2, &mut out);

    assert_eq!(out.total_me_candidate_index[0], 3);
    let c0 = out.me_candidate_array[0];
    assert_eq!((c0.direction, c0.ref0_list, c0.ref1_list), (0, 0, 0));
    let c1 = out.me_candidate_array[1];
    assert_eq!(
        (c1.direction, c1.ref0_list, c1.ref1_list),
        (1, 0, 1),
        "ref0_list is C's literal 24 truncated into a 1-bit field"
    );
    let c2 = out.me_candidate_array[2];
    assert_eq!((c2.direction, c2.ref0_list, c2.ref1_list), (BI_PRED, 0, 1));
    assert_eq!(ctx.me_distortion[0], 500, "best of the two lists");
    // L1's MV lands at max_l0 offset inside the PU's MV slot group.
    assert_eq!(out.me_mv_array[pic.max_l0].x, -1);
}

/// **Tier 4** (`construct_me_candidate_array`, motion_estimation.c:2499). The
/// general path's three bi-pred sets, and the `only_l_bwd` gate that collapses
/// them to the single (L0[0], L1[0]) pair.
#[test]
fn general_candidate_array_emits_the_three_bipred_sets() {
    let mut pic = pic_params();
    let mut ctx = MeContext::default();
    ctx.num_of_list_to_search = 2;
    ctx.num_of_ref_pic_to_search = [2, 3];
    for li in 0..2 {
        for ri in 0..4 {
            ctx.search_results[li][ri].do_ref = 1;
        }
    }
    for li in 0..2 {
        for ri in 0..4 {
            for i in 0..SQUARE_PU_COUNT {
                ctx.p_sb_best_sad[li][ri][i] = 1000;
            }
        }
    }
    let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
    construct_me_candidate_array(&pic, &mut ctx, 2, &mut out);
    // 5 unipred (2 in L0 + 3 in L1) + 6 (2x3 pairs) + 1 (L0[0],L0[1]) + 1 (L1[0],L1[2])
    assert_eq!(out.total_me_candidate_index[0], 5 + 6 + 1 + 1);

    pic.only_l_bwd = true;
    let mut out2 = MeB64Output::new(pic.max_cand, pic.max_refs);
    construct_me_candidate_array(&pic, &mut ctx, 2, &mut out2);
    assert_eq!(out2.total_me_candidate_index[0], 5 + 1, "only_l_bwd keeps one bi-pred pair");
}

/// **Tier 4** (`compute_distortion`, motion_estimation.c:2739). The four
/// block-size sums, the 8x8 variance, and the `b64_size / pix_num`
/// normalisation for a partial b64.
#[test]
fn compute_distortion_sums_and_normalises() {
    let mut pic = pic_params();
    pic.b64_geom_width = 32;
    pic.b64_geom_height = 64; // half a b64 -> x2 normalisation
    let mut ctx = MeContext::default();
    ctx.me_distortion[0] = 6400;
    for i in 0..4 {
        ctx.me_distortion[1 + i] = 1600;
    }
    for i in 0..16 {
        ctx.me_distortion[5 + i] = 400;
    }
    for i in 0..64 {
        ctx.me_distortion[21 + i] = 100;
    }
    let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
    compute_distortion(&pic, &ctx, &mut out);
    assert_eq!(out.me_8x8_cost_variance, 0, "a flat 8x8 field has zero variance");
    assert_eq!(out.rc_me_distortion, 6400, "<= 480p reports the 8x8 sum");
    assert_eq!(out.me_64x64_distortion, 6400 * 4096 / 2048);
    assert_eq!(out.me_32x32_distortion, 6400 * 4096 / 2048);
    assert_eq!(out.me_16x16_distortion, 6400 * 4096 / 2048);
    assert_eq!(out.me_8x8_distortion, 6400 * 4096 / 2048);
}

/// **Tier 4** (`perform_gm_detection`, motion_estimation.c:2637). At <= 480p
/// the detector walks the 64 8x8 PUs with an activity threshold of 4 (in
/// quarter-pel units, i.e. `mv * 4`).
#[test]
fn perform_gm_detection_flags_a_uniformly_moving_block() {
    let pic = pic_params();
    let mut ctx = MeContext::default();
    ctx.num_of_list_to_search = 1;
    ctx.num_of_ref_pic_to_search = [1, 0];
    let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
    // Every candidate is L0/ref0 unipred with a +2 full-pel x MV (= +8 qpel).
    for i in 0..SQUARE_PU_COUNT {
        out.me_candidate_array[i * pic.max_cand].set(0, 0, 0, 0, 0);
        ctx.p_sb_best_mv[0][0][i] = pack_mv(2, 0);
    }
    perform_gm_detection(&pic, &ctx, &mut out);
    assert_eq!(out.rc_me_allow_gm, 1);

    // A zero field is below the threshold everywhere.
    let mut out2 = MeB64Output::new(pic.max_cand, pic.max_refs);
    for i in 0..SQUARE_PU_COUNT {
        out2.me_candidate_array[i * pic.max_cand].set(0, 0, 0, 0, 0);
        ctx.p_sb_best_mv[0][0][i] = 0;
    }
    perform_gm_detection(&pic, &ctx, &mut out2);
    assert_eq!(out2.rc_me_allow_gm, 0);
}

/// **Functional, not a parity claim.** The whole `motion_estimation_b64`
/// pipeline on a picture that is a pure integer translation of its reference:
/// the search must recover that exact MV for the 64x64 PU and every sub-PU,
/// with a SAD of zero. This is what catches a search-area/origin sign error
/// that no unit test of a leaf kernel can see.
#[test]
fn motion_estimation_b64_recovers_a_pure_translation() {
    let mut rng = Rng(0xC4_1007);
    for &(dx, dy) in &[(3i32, 0i32), (-5, 2), (0, -4), (7, 7), (0, 0)] {
        let plane = TestPlane::new(&mut rng, 128, 128, 64);
        // src(x, y) = ref(x + dx, y + dy) -> the best full-pel MV is (dx, dy).
        let mut src_buf = vec![0u8; 64 * 64];
        for y in 0..64usize {
            for x in 0..64usize {
                let sx = (x as i32 + dx) as usize;
                let sy = (y as i32 + dy) as usize;
                src_buf[y * 64 + x] = plane.data[(plane.org as i64
                    + sy as i64 * plane.stride as i64
                    + sx as i64) as usize];
            }
        }
        let src = one_buf(&src_buf, 64);
        let refs = MeRefs {
            arr: [
                [
                    Some(MeDsRef {
                        picture: plane.view(),
                        quarter: plane.view(),
                        sixteenth: plane.view(),
                        picture_number: 3,
                    }),
                    None,
                    None,
                    None,
                ],
                [None, None, None, None],
            ],
        };

        let pic = pic_params();
        let mut ctx = MeContext::default();
        ctx.num_of_list_to_search = 1;
        ctx.num_of_ref_pic_to_search = [1, 0];
        ctx.me_sa = SearchAreaMinMax {
            sa_min: SearchArea { width: 32, height: 32 },
            sa_max: SearchArea { width: 32, height: 32 },
        };
        ctx.me_search_method = FULL_SAD_SEARCH;
        let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
        motion_estimation_b64(&pic, 0, 0, &mut ctx, &src, &refs, &mut out);

        let want = pack_mv(dx as i16, dy as i16);
        assert_eq!(ctx.p_sb_best_sad[0][0][PU_64X64], 0, "exact match => zero SAD, d=({dx},{dy})");
        assert_eq!(ctx.p_sb_best_mv[0][0][PU_64X64], want, "64x64 MV, d=({dx},{dy})");
        for i in 0..64 {
            assert_eq!(ctx.p_sb_best_sad[0][0][PU_8X8_0 + i], 0, "8x8[{i}] SAD, d=({dx},{dy})");
        }
        assert_eq!(out.me_mv_array[0].x, dx as i16);
        assert_eq!(out.me_mv_array[0].y, dy as i16);
        assert_eq!(out.total_me_candidate_index[0], 1);
    }
}
