//! Differential parity for `svtav1_encoder::inter_me` against the REAL
//! exported C symbols in `libSvtAv1Enc.a` — **evidence tier 1**
//! (`rust/docs/WORKING-ON-THIS.md` §4).
//!
//! Every assertion in this file drives C code. The ten oracles are:
//! `svt_aom_compute8x4_sad_kernel_c`, `svt_nxm_sad_kernel_helper_c`,
//! `svt_sad_loop_kernel_c` (plus the RTCD-dispatched
//! `svt_sad_loop_kernel` as a positive control),
//! `svt_ext_sad_calculation_8x8_16x16_c`,
//! `svt_ext_sad_calculation_32x32_64x64_c`,
//! `svt_ext_all_sad_calculation_8x8_16x16_c`,
//! `svt_ext_eight_sad_calculation_32x32_64x64_c`,
//! `svt_aom_get_scaled_picture_distance`, `hme_level_2` and `check_00_center`.
//!
//! **What this file does NOT prove.** The other 30 functions in
//! `motion_estimation.c` are `static`: no symbol, no tier-1 oracle. Their
//! coverage lives in `inter_me_traced.rs` and is labelled tier 4 there. Do not
//! read a green run here as "the ME matches C" — read it as "every C function
//! the linker can reach agrees, and the search that composes them is only
//! hand-traced".

// `MeContext` has ~60 fields and each test sets one or two of them; spelling
// `..Default::default()` at that size buries the one line that matters.
#![allow(clippy::field_reassign_with_default)]

use svtav1_cref::inter_me as cref;
use svtav1_encoder::inter_me::context::{
    FULL_SAD_SEARCH, MeContext, MeSrcBufs, Plane, SUB_SAD_SEARCH,
};
use svtav1_encoder::inter_me::{hme, sad};

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

#[test]
fn compute8x4_sad_matches_c() {
    let mut rng = Rng(0xC4_0001);
    for _ in 0..256 {
        let ss = 8 + rng.below(40) as usize;
        let rs = 8 + rng.below(40) as usize;
        let src = noise(&mut rng, ss * 8 + 64);
        let rf = noise(&mut rng, rs * 8 + 64);
        let got = sad::compute8x4_sad_kernel(&src, ss, &rf, rs);
        let want = cref::compute8x4_sad(&src, ss, &rf, rs);
        assert_eq!(got, want, "8x4 SAD, strides {ss}/{rs}");
    }
}

#[test]
fn nxm_sad_matches_c() {
    let mut rng = Rng(0xC4_0002);
    for _ in 0..256 {
        let w = 1 + rng.below(64) as usize;
        let h = 1 + rng.below(64) as usize;
        let ss = w + rng.below(32) as usize;
        let rs = w + rng.below(32) as usize;
        let src = noise(&mut rng, ss * h + 64);
        let rf = noise(&mut rng, rs * h + 64);
        let got = sad::nxm_sad_kernel(&src, ss, &rf, rs, h, w);
        let want = cref::nxm_sad(&src, ss, &rf, rs, h, w);
        assert_eq!(got, want, "{w}x{h} SAD");
    }
}

/// `svt_sad_loop_kernel_c` — the kernel HME level 0/1/2 and pre-HME all drive.
/// The returned search centre matters as much as the SAD: C keeps the FIRST
/// minimum, so a different tie-break is a different MV.
#[test]
fn sad_loop_kernel_matches_c() {
    let mut rng = Rng(0xC4_0003);
    for case in 0..96 {
        let bw = [8usize, 16, 32, 64][(case % 4) as usize];
        let bh = [8usize, 16, 32, 64][((case / 4) % 4) as usize];
        let sa_w = 1 + rng.below(24) as i16;
        let sa_h = 1 + rng.below(12) as i16;
        let skip = if case % 3 == 0 { 1u8 } else { 0 };
        let src_stride = bw + rng.below(16) as usize;
        let ref_stride = bw + sa_w as usize + rng.below(16) as usize;
        let src = noise(&mut rng, src_stride * bh + 64);
        let rows = bh + sa_h as usize + 2;
        let rf = noise(&mut rng, ref_stride * rows + 128);

        let got = sad::sad_loop_kernel(
            &src, src_stride, &rf, 0, ref_stride, bh, bw, ref_stride, skip, sa_w, sa_h,
        );
        let want = cref::sad_loop_kernel(
            &src, src_stride, &rf, 0, ref_stride, bh, bw, ref_stride, skip, sa_w, sa_h,
        );
        assert_eq!(
            (got.best_sad, got.x_search_center, got.y_search_center),
            want,
            "sad_loop_kernel {bw}x{bh} sa {sa_w}x{sa_h} skip {skip}"
        );
    }
}

/// Positive control: the SIMD tier this host dispatches to must agree with the
/// `_c` kernel the port transcribes — SAD value AND tie-broken centre. If this
/// ever fails, the port is still faithful to `_c` but the shipping encoder is
/// running a different search on this ISA, which is a finding for
/// `docs/SUSPECTED-C-BUGS.md`, not a reason to weaken the test above.
#[test]
fn sad_loop_kernel_rtcd_agrees_with_c() {
    let mut rng = Rng(0xC4_0004);
    for case in 0..64 {
        let bw = [8usize, 16, 32, 64][(case % 4) as usize];
        let bh = [8usize, 16, 32, 64][((case / 4) % 4) as usize];
        let sa_w = 8 + rng.below(24) as i16;
        let sa_h = 1 + rng.below(12) as i16;
        let src_stride = bw + rng.below(16) as usize;
        let ref_stride = bw + sa_w as usize + rng.below(16) as usize;
        let src = noise(&mut rng, src_stride * bh + 64);
        let rows = bh + sa_h as usize + 2;
        let rf = noise(&mut rng, ref_stride * rows + 128);

        let c = cref::sad_loop_kernel(
            &src, src_stride, &rf, 0, ref_stride, bh, bw, ref_stride, 0, sa_w, sa_h,
        );
        let rtcd = cref::sad_loop_kernel_rtcd(
            &src, src_stride, &rf, 0, ref_stride, bh, bw, ref_stride, 0, sa_w, sa_h,
        );
        assert_eq!(
            c, rtcd,
            "RTCD vs _c sad_loop_kernel {bw}x{bh} sa {sa_w}x{sa_h}"
        );
    }
}

#[test]
fn ext_sad_calculation_8x8_16x16_matches_c() {
    let mut rng = Rng(0xC4_0005);
    for case in 0..128 {
        let sub_sad = case % 2 == 0;
        let ss = 16 + rng.below(48) as usize;
        let rs = 16 + rng.below(48) as usize;
        let src = noise(&mut rng, ss * 16 + 64);
        let rf = noise(&mut rng, rs * 16 + 64);
        let mv = rng.next() as u32;
        let off8 = 21 + 4 * (case % 16);
        let off16 = 5 + (case % 16);

        // Seed the "best so far" arrays with a mix of MAX and plausible SADs so
        // both the improving and the non-improving branch are exercised.
        let seed_sad: [u32; 85] = core::array::from_fn(|_| {
            if rng.below(2) == 0 {
                u32::MAX
            } else {
                rng.below(6000) as u32
            }
        });
        let seed_mv: [u32; 85] = core::array::from_fn(|_| rng.next() as u32);

        let mut r_sad = seed_sad;
        let mut r_mv = seed_mv;
        let mut r_s16 = [0u32; 16];
        let mut r_s8 = [0u32; 64];
        sad::ext_sad_calculation_8x8_16x16(
            &src,
            ss,
            &rf,
            rs,
            &mut r_sad,
            &mut r_mv,
            off8,
            off16,
            mv,
            &mut r_s16,
            case % 16,
            &mut r_s8,
            4 * (case % 16),
            sub_sad,
        );

        let mut c_sad = seed_sad;
        let mut c_mv = seed_mv;
        let mut c_s16 = [0u32; 16];
        let mut c_s8 = [0u32; 64];
        cref::ext_sad_calculation_8x8_16x16(
            &src,
            ss,
            &rf,
            rs,
            &mut c_sad,
            &mut c_mv,
            off8,
            off16,
            mv,
            &mut c_s16,
            case % 16,
            &mut c_s8,
            4 * (case % 16),
            sub_sad,
        );

        assert_eq!(r_sad, c_sad, "best_sad, sub_sad={sub_sad}");
        assert_eq!(r_mv, c_mv, "best_mv, sub_sad={sub_sad}");
        assert_eq!(r_s16, c_s16, "p_sad16x16");
        assert_eq!(r_s8, c_s8, "p_sad8x8");
    }
}

#[test]
fn ext_sad_calculation_32x32_64x64_matches_c() {
    let mut rng = Rng(0xC4_0006);
    for _ in 0..256 {
        let p16: [u32; 16] = core::array::from_fn(|_| rng.below(40000) as u32);
        let mv = rng.next() as u32;
        let seed_sad: [u32; 85] = core::array::from_fn(|_| {
            if rng.below(2) == 0 {
                u32::MAX
            } else {
                rng.below(200_000) as u32
            }
        });
        let seed_mv: [u32; 85] = core::array::from_fn(|_| rng.next() as u32);

        let mut r_sad = seed_sad;
        let mut r_mv = seed_mv;
        let mut r_s32 = [0u32; 4];
        sad::ext_sad_calculation_32x32_64x64(&p16, &mut r_sad, &mut r_mv, 1, 0, mv, &mut r_s32);

        let mut c_sad = seed_sad;
        let mut c_mv = seed_mv;
        let mut c_s32 = [0u32; 4];
        cref::ext_sad_calculation_32x32_64x64(&p16, &mut c_sad, &mut c_mv, 1, 0, mv, &mut c_s32);

        assert_eq!(r_sad, c_sad);
        assert_eq!(r_mv, c_mv);
        assert_eq!(r_s32, c_s32);
    }
}

#[test]
fn ext_all_sad_calculation_8x8_16x16_matches_c() {
    let mut rng = Rng(0xC4_0007);
    for case in 0..64 {
        let sub_sad = case % 2 == 0;
        let ss = 64 + rng.below(32) as usize;
        let rs = 72 + rng.below(32) as usize;
        let src = noise(&mut rng, ss * 64 + 128);
        let rf = noise(&mut rng, rs * 64 + 128);
        let mv = ((rng.below(64) as u32) << 16) | rng.below(64) as u32;

        let seed_sad: [u32; 85] = core::array::from_fn(|_| {
            if rng.below(3) == 0 {
                u32::MAX
            } else {
                rng.below(60000) as u32
            }
        });
        let seed_mv: [u32; 85] = core::array::from_fn(|_| rng.next() as u32);

        let mut r_sad = seed_sad;
        let mut r_mv = seed_mv;
        let mut r_e16 = [[0u32; 8]; 16];
        sad::ext_all_sad_calculation_8x8_16x16(
            &src, ss, &rf, rs, mv, &mut r_sad, &mut r_mv, 21, 5, &mut r_e16, sub_sad,
        );

        let mut c_sad = seed_sad;
        let mut c_mv = seed_mv;
        let mut c_e16 = [[0u32; 8]; 16];
        cref::ext_all_sad_calculation_8x8_16x16(
            &src, ss, &rf, rs, mv, &mut c_sad, &mut c_mv, 21, 5, &mut c_e16, sub_sad,
        );

        assert_eq!(r_sad, c_sad, "best_sad, sub_sad={sub_sad}");
        assert_eq!(r_mv, c_mv, "best_mv, sub_sad={sub_sad}");
        assert_eq!(r_e16, c_e16, "p_eight_sad16x16, sub_sad={sub_sad}");
    }
}

#[test]
fn ext_eight_sad_calculation_32x32_64x64_matches_c() {
    let mut rng = Rng(0xC4_0008);
    for _ in 0..128 {
        let p16: [[u32; 8]; 16] =
            core::array::from_fn(|_| core::array::from_fn(|_| rng.below(40000) as u32));
        let mv = ((rng.below(64) as u32) << 16) | rng.below(64) as u32;
        let seed_sad: [u32; 85] = core::array::from_fn(|_| {
            if rng.below(3) == 0 {
                u32::MAX
            } else {
                rng.below(400_000) as u32
            }
        });
        let seed_mv: [u32; 85] = core::array::from_fn(|_| rng.next() as u32);

        let mut r_sad = seed_sad;
        let mut r_mv = seed_mv;
        let mut r_s32 = [[0u32; 8]; 4];
        sad::ext_eight_sad_calculation_32x32_64x64(
            &p16, &mut r_sad, &mut r_mv, 1, 0, mv, &mut r_s32,
        );

        let mut c_sad = seed_sad;
        let mut c_mv = seed_mv;
        let mut c_s32 = [[0u32; 8]; 4];
        cref::ext_eight_sad_calculation_32x32_64x64(
            &p16, &mut c_sad, &mut c_mv, 1, 0, mv, &mut c_s32,
        );

        assert_eq!(r_sad, c_sad);
        assert_eq!(r_mv, c_mv);
        assert_eq!(r_s32, c_s32);
    }
}

/// Exhaustive over the whole `uint16_t` domain — the function is one line and
/// its rounding is what scales every search area.
#[test]
fn get_scaled_picture_distance_matches_c_exhaustively() {
    for d in 0..=u16::MAX {
        assert_eq!(
            hme::get_scaled_picture_distance(d),
            cref::get_scaled_picture_distance(d),
            "dist {d}"
        );
    }
}

/// A padded luma plane: `data`, the index of pixel (0, 0), and the stride.
struct TestPlane {
    data: Vec<u8>,
    org: usize,
    stride: usize,
    width: u16,
    height: u16,
    border: u16,
}

fn make_plane(rng: &mut Rng, width: u16, height: u16, border: u16) -> TestPlane {
    let stride = width as usize + 2 * border as usize;
    let rows = height as usize + 2 * border as usize;
    TestPlane {
        data: noise(rng, stride * rows + 64),
        org: border as usize * stride + border as usize,
        stride,
        width,
        height,
        border,
    }
}

impl TestPlane {
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
}

/// `hme_level_2` is EXPORTED, so the whole level-2 search — search-area
/// clamping, the sub-sampled SAD path, the `*2` SAD rescale and the origin
/// fold-back — is gated at tier 1 here.
#[test]
fn hme_level_2_matches_c() {
    let mut rng = Rng(0xC4_0009);
    for case in 0..96 {
        let width = 64 + 16 * (rng.below(8) as u16);
        let height = 64 + 16 * (rng.below(8) as u16);
        let plane = make_plane(&mut rng, width, height, 64);
        let bw = 64u32;
        let bh = 64u32;
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);

        let org_x = (rng.below((width / 64).max(1) as u64) as i16) * 64;
        let org_y = (rng.below((height / 64).max(1) as u64) as i16) * 64;
        let sa_w = 1 + rng.below(24) as i16;
        let sa_h = 1 + rng.below(12) as i16;
        let l1x = rng.below(33) as i16 - 16;
        let l1y = rng.below(33) as i16 - 16;
        let method = if case % 2 == 0 {
            FULL_SAD_SEARCH
        } else {
            SUB_SAD_SEARCH
        };

        let mut ctx = MeContext::default();
        ctx.hme_search_method = method;
        let src = MeSrcBufs {
            b64: &src_buf,
            b64_stride: src_stride,
            quarter: &src_buf,
            quarter_stride: src_stride,
            sixteenth: &src_buf,
            sixteenth_stride: src_stride,
        };

        let got = hme::hme_level_2(
            &ctx,
            &src,
            org_x,
            org_y,
            bw,
            bh,
            &plane.view(),
            sa_w,
            sa_h,
            l1x,
            l1y,
        );
        let want = cref::hme_level_2(
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
            l1x,
            l1y,
        );
        assert_eq!(
            got, want,
            "hme_level_2 case {case}: {width}x{height} org ({org_x},{org_y}) sa {sa_w}x{sa_h} l1 ({l1x},{l1y}) method {method}"
        );
    }
}

/// `check_00_center` is EXPORTED: the zero-MV-vs-HME-centre decision, its
/// clamping, and the `me_early_exit_th` short-circuit are all tier 1.
#[test]
fn check_00_center_matches_c() {
    let mut rng = Rng(0xC4_000A);
    for case in 0..96 {
        let width = 64 + 16 * (rng.below(8) as u16);
        let height = 64 + 16 * (rng.below(8) as u16);
        let plane = make_plane(&mut rng, width, height, 64);
        let src_stride = 64usize;
        let src_buf = noise(&mut rng, src_stride * 64 + 64);

        let org_x = (rng.below((width / 64).max(1) as u64) as u32) * 64;
        let org_y = (rng.below((height / 64).max(1) as u64) as u32) * 64;
        let x_sc = rng.below(129) as i16 - 64;
        let y_sc = rng.below(129) as i16 - 64;
        let early = if case % 3 == 0 {
            1 + rng.below(10000) as u32
        } else {
            0
        };
        let zz = rng.below(50000) as u32;

        let mut ctx = MeContext::default();
        ctx.me_early_exit_th = early;
        let src = MeSrcBufs {
            b64: &src_buf,
            b64_stride: src_stride,
            quarter: &src_buf,
            quarter_stride: src_stride,
            sixteenth: &src_buf,
            sixteenth_stride: src_stride,
        };

        let mut rx = x_sc;
        let mut ry = y_sc;
        let got = hme::check_00_center(
            &plane.view(),
            &ctx,
            &src,
            org_x,
            org_y,
            64,
            64,
            &mut rx,
            &mut ry,
            zz,
        );
        let want = cref::check_00_center(
            &src_buf,
            src_stride,
            early,
            &plane.data,
            plane.org,
            plane.stride as u16,
            width,
            height,
            org_x,
            org_y,
            64,
            64,
            x_sc,
            y_sc,
            zz,
        );
        assert_eq!(
            (got, rx, ry),
            want,
            "check_00_center case {case}: {width}x{height} org ({org_x},{org_y}) sc ({x_sc},{y_sc}) early {early}"
        );
    }
}
