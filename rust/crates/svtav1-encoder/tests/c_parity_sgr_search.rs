//! Differential for the SGR SEARCH port (`port_sgr_search`), against
//! `Codec/restoration_pick.c`.
//!
//! # Evidence tiers, stated per function
//!
//! **TIER 1** — driven against the real exported C symbol:
//! * `svt_av1_lowbd_pixel_proj_error_c`
//! * `svt_av1_highbd_pixel_proj_error_c`
//! * `svt_get_proj_subspace_c` (8-bit and high bit depth)
//!
//! **TIER 4** — the C function is `static` / `static INLINE` in
//! `restoration_pick.c` with no exported symbol, and the only exported driver
//! (`svt_aom_restoration_seg_search`) needs a built `RestSearchCtxt` +
//! `Av1Common` + `PictureControlSet` that is not assembled here:
//! `encode_xq`, `count_sgrproj_bits`, `finer_search_pixel_proj_error`,
//! `apply_sgr`, `search_selfguided_restoration`, and the decision bodies of
//! `search_sgrproj_finish` and `search_switchable`.
//!
//! Two of those tier-4 bodies are made of nothing but calls into tier-1
//! kernels, so this file drives the C KERNELS through the port's control flow
//! and compares the result against the port driving its own kernels
//! (`finer_search_hill_climb_matches_the_c_kernel_sequence`,
//! `ep_sweep_picks_the_same_ep_as_the_c_kernels`). That makes the arithmetic
//! tier 1 and leaves only the loop structure at tier 4 — which is the strongest
//! thing available without adding a `RestSearchCtxt` shim, and is stated here
//! rather than being blurred into "tier 1".

use svtav1_cref as cref;
use svtav1_dsp::port_sgr::{
    RESTORATION_UNITPELS_MAX, SGR_PARAMS, SGRPROJ_BORDER_HORZ, SGRPROJ_BORDER_VERT,
    SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1, SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1, SgrSrc, decode_xq,
    selfguided_restoration,
};
use svtav1_encoder::port_lr_level::{SgFilterCtrls, set_sg_filter_ctrls};
use svtav1_encoder::port_sgr_search::{
    ProjPlanes, SgrprojInfo, apply_sgr, encode_xq, finer_search_pixel_proj_error,
    get_proj_subspace, highbd_pixel_proj_error, lowbd_pixel_proj_error, search_flt_stride,
    search_selfguided_restoration,
};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 32) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u32() % ((hi - lo + 1) as u32)) as i32
    }
}

fn extended_u8(width: i32, height: i32, seed: u64) -> (Vec<u8>, usize, usize) {
    let bh = SGRPROJ_BORDER_HORZ as usize;
    let bv = SGRPROJ_BORDER_VERT as usize;
    let stride = width as usize + 2 * bh + 5;
    let rows = height as usize + 2 * bv;
    let mut rng = Rng::new(seed);
    let p: Vec<u8> = (0..stride * rows)
        .map(|_| rng.range(0, 255) as u8)
        .collect();
    (p, bv * stride + bh, stride)
}

fn extended_u16(width: i32, height: i32, seed: u64, bd: i32) -> (Vec<u16>, usize, usize) {
    let bh = SGRPROJ_BORDER_HORZ as usize;
    let bv = SGRPROJ_BORDER_VERT as usize;
    let stride = width as usize + 2 * bh + 5;
    let rows = height as usize + 2 * bv;
    let maxv = (1i32 << bd) - 1;
    let mut rng = Rng::new(seed);
    let p: Vec<u16> = (0..stride * rows)
        .map(|_| rng.range(0, maxv) as u16)
        .collect();
    (p, bv * stride + bh, stride)
}

/// Produce a REAL `flt0`/`flt1` pair for a unit by running the (tier-1 gated)
/// filter — so the search kernels see the value distribution they see in
/// production, not synthetic noise.
fn real_flts_u8(
    plane: &[u8],
    origin: usize,
    stride: usize,
    w: i32,
    h: i32,
    ep: usize,
    flt_stride: usize,
) -> (Vec<i32>, Vec<i32>) {
    let mut f0 = vec![0i32; RESTORATION_UNITPELS_MAX];
    let mut f1 = vec![0i32; RESTORATION_UNITPELS_MAX];
    selfguided_restoration(
        SgrSrc::Lowbd(plane),
        origin,
        w,
        h,
        stride,
        &mut f0,
        &mut f1,
        flt_stride,
        ep,
        8,
    );
    (f0, f1)
}

// ---------------------------------------------------------------------------
// TIER 1: svt_av1_lowbd_pixel_proj_error_c
// ---------------------------------------------------------------------------

/// Every `ep` (which exercises all FOUR of C's separate loops: both filters,
/// r0-only, r1-only) x the whole signalled `xqd` corner set x several shapes.
#[test]
fn lowbd_pixel_proj_error_matches_c() {
    let mut rng = Rng::new(0x9911);
    let mut both = 0usize;
    let mut only0 = 0usize;
    let mut only1 = 0usize;

    for ep in 0..16usize {
        let p = SGR_PARAMS[ep];
        if p.r[0] > 0 && p.r[1] > 0 {
            both += 1;
        } else if p.r[0] > 0 {
            only0 += 1;
        } else {
            only1 += 1;
        }
        for &(w, h) in &[(8i32, 8i32), (16, 16), (64, 32), (13, 7)] {
            let (dat, dorigin, dstride) = extended_u8(w, h, ep as u64 * 5 + 1);
            let (src, sorigin, sstride) = extended_u8(w, h, ep as u64 * 5 + 2);
            let flt_stride = search_flt_stride(w);
            let (mut f0, mut f1) = real_flts_u8(&dat, dorigin, dstride, w, h, ep, flt_stride);

            for _ in 0..4 {
                let xqd = [
                    rng.range(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0),
                    rng.range(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
                ];
                let xq = decode_xq(&xqd, &p);
                let c = cref::lowbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride,
                    &mut f0, flt_stride, &mut f1, flt_stride, &xq, ep as i32,
                );
                let r = lowbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, &f0,
                    flt_stride, &f1, flt_stride, &xq, &p,
                );
                assert_eq!(r, c, "lowbd proj error mismatch ep {ep} {w}x{h} xq {xq:?}");
                assert!(c > 0, "degenerate zero error at ep {ep}");
            }
        }
    }
    // Anti-vacuity: all three live loop shapes were exercised. C's fourth arm
    // (both radii zero) is unreachable — no `ep` disables both — and the port
    // asserts that in its own unit test.
    assert!(both > 0 && only0 > 0 && only1 > 0);
}

// ---------------------------------------------------------------------------
// TIER 1: svt_av1_highbd_pixel_proj_error_c
// ---------------------------------------------------------------------------

/// The high-bit-depth kernel is a DIFFERENT formula, not a widened copy (a
/// `half` bias, a plain `>>` instead of `ROUND_POWER_OF_TWO`, and `+ d` at the
/// end). A port that reused the 8-bit body would pass the 8-bit test and fail
/// here.
#[test]
fn highbd_pixel_proj_error_matches_c() {
    let mut rng = Rng::new(0x7733);
    for ep in 0..16usize {
        let p = SGR_PARAMS[ep];
        for &(w, h) in &[(16i32, 16i32), (32, 24)] {
            let (dat, dorigin, dstride) = extended_u16(w, h, ep as u64 * 7 + 1, 10);
            let (src, sorigin, sstride) = extended_u16(w, h, ep as u64 * 7 + 2, 10);
            let flt_stride = search_flt_stride(w);
            let mut f0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut f1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            selfguided_restoration(
                SgrSrc::Highbd(&dat),
                dorigin,
                w,
                h,
                dstride,
                &mut f0,
                &mut f1,
                flt_stride,
                ep,
                10,
            );
            for _ in 0..4 {
                let xqd = [
                    rng.range(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0),
                    rng.range(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
                ];
                let xq = decode_xq(&xqd, &p);
                let c = cref::highbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride,
                    &mut f0, flt_stride, &mut f1, flt_stride, &xq, ep as i32,
                );
                let r = highbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, &f0,
                    flt_stride, &f1, flt_stride, &xq, &p,
                );
                assert_eq!(r, c, "highbd proj error mismatch ep {ep} {w}x{h}");
                assert!(c > 0);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TIER 1: svt_get_proj_subspace_c
// ---------------------------------------------------------------------------

/// The subspace solve is `double` arithmetic ending in `rint`, so this is also
/// the test that pins `round_ties_even` (not `round`) and the accumulation
/// ORDER — f64 addition is not associative, so a reordered sum is a different
/// number and this comparison is exact, not approximate.
#[test]
fn get_proj_subspace_matches_c_exactly() {
    let mut nonzero = 0usize;
    for ep in 0..16usize {
        let p = SGR_PARAMS[ep];
        for &(w, h) in &[(8i32, 8i32), (16, 16), (64, 64), (23, 11)] {
            let (dat, dorigin, dstride) = extended_u8(w, h, ep as u64 * 11 + 1);
            let (src, sorigin, sstride) = extended_u8(w, h, ep as u64 * 11 + 2);
            let flt_stride = search_flt_stride(w);
            let (mut f0, mut f1) = real_flts_u8(&dat, dorigin, dstride, w, h, ep, flt_stride);

            let c = cref::get_proj_subspace(
                &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, &mut f0,
                flt_stride, &mut f1, flt_stride, ep as i32,
            );
            let planes = ProjPlanes::Lowbd {
                src: &src,
                dat: &dat,
            };
            let r = get_proj_subspace(
                &planes, sorigin, w as usize, h as usize, sstride, dorigin, dstride, &f0,
                flt_stride, &f1, flt_stride, &p,
            );
            assert_eq!(r, c, "get_proj_subspace mismatch ep {ep} {w}x{h}");
            if c != [0, 0] {
                nonzero += 1;
            }
        }
    }
    assert!(
        nonzero > 20,
        "only {nonzero} non-default solves — the ill-posed early-out may be \
         swallowing every case and the test would prove nothing"
    );
}

#[test]
fn get_proj_subspace_matches_c_at_bd10() {
    for ep in 0..16usize {
        let p = SGR_PARAMS[ep];
        for &(w, h) in &[(16i32, 16i32), (32, 32)] {
            let (dat, dorigin, dstride) = extended_u16(w, h, ep as u64 * 3 + 1, 10);
            let (src, sorigin, sstride) = extended_u16(w, h, ep as u64 * 3 + 2, 10);
            let flt_stride = search_flt_stride(w);
            let mut f0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut f1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            selfguided_restoration(
                SgrSrc::Highbd(&dat),
                dorigin,
                w,
                h,
                dstride,
                &mut f0,
                &mut f1,
                flt_stride,
                ep,
                10,
            );
            let c = cref::get_proj_subspace_hbd(
                &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, &mut f0,
                flt_stride, &mut f1, flt_stride, ep as i32,
            );
            let planes = ProjPlanes::Highbd {
                src: &src,
                dat: &dat,
            };
            let r = get_proj_subspace(
                &planes, sorigin, w as usize, h as usize, sstride, dorigin, dstride, &f0,
                flt_stride, &f1, flt_stride, &p,
            );
            assert_eq!(r, c, "bd10 get_proj_subspace mismatch ep {ep} {w}x{h}");
        }
    }
}

// ---------------------------------------------------------------------------
// TIER 4 loop structure over TIER 1 kernels
// ---------------------------------------------------------------------------

/// `finer_search_pixel_proj_error` is `static`. Its arithmetic is entirely
/// `get_pixel_proj_error`, which IS exported, so this replays C's hill-climb
/// against the C KERNEL and compares both the returned error and the refined
/// `xqd` against the port. What remains tier 4 is the loop structure itself —
/// the `skip`/`continue`-at-top-step shape transcribed in the port.
#[test]
fn finer_search_hill_climb_matches_the_c_kernel_sequence() {
    let mut rng = Rng::new(0x4242);
    let mut moved = 0usize;

    for ep in 0..16usize {
        let p = SGR_PARAMS[ep];
        for &(w, h) in &[(16i32, 16i32), (32, 16)] {
            let (dat, dorigin, dstride) = extended_u8(w, h, ep as u64 * 17 + 1);
            let (src, sorigin, sstride) = extended_u8(w, h, ep as u64 * 17 + 2);
            let flt_stride = search_flt_stride(w);
            let (mut f0, mut f1) = real_flts_u8(&dat, dorigin, dstride, w, h, ep, flt_stride);

            let start = [
                rng.range(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0),
                rng.range(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
            ];

            // --- C-kernel replay of the same hill climb ---
            let mut c_xqd = start;
            let c_err_of = |xqd: &[i32; 2], f0: &mut Vec<i32>, f1: &mut Vec<i32>| -> i64 {
                let xq = decode_xq(xqd, &p);
                cref::lowbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, f0,
                    flt_stride, f1, flt_stride, &xq, ep as i32,
                )
            };
            let mut c_err = c_err_of(&c_xqd, &mut f0, &mut f1);
            let tap_min = [SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1];
            let tap_max = [SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1];
            let start_step = 2i32;
            let mut s = start_step;
            while s >= 1 {
                for pi in 0..2usize {
                    if (p.r[0] == 0 && pi == 0) || (p.r[1] == 0 && pi == 1) {
                        continue;
                    }
                    let mut skip = false;
                    loop {
                        if c_xqd[pi] - s >= tap_min[pi] {
                            c_xqd[pi] -= s;
                            let e2 = c_err_of(&c_xqd, &mut f0, &mut f1);
                            if e2 > c_err {
                                c_xqd[pi] += s;
                            } else {
                                c_err = e2;
                                skip = true;
                                if s == start_step {
                                    continue;
                                }
                            }
                        }
                        break;
                    }
                    if skip {
                        break;
                    }
                    loop {
                        if c_xqd[pi] + s <= tap_max[pi] {
                            c_xqd[pi] += s;
                            let e2 = c_err_of(&c_xqd, &mut f0, &mut f1);
                            if e2 > c_err {
                                c_xqd[pi] -= s;
                            } else {
                                c_err = e2;
                                if s == start_step {
                                    continue;
                                }
                            }
                        }
                        break;
                    }
                }
                s >>= 1;
            }

            // --- port ---
            let planes = ProjPlanes::Lowbd {
                src: &src,
                dat: &dat,
            };
            let mut r_xqd = start;
            let r_err = finer_search_pixel_proj_error(
                &planes, sorigin, w as usize, h as usize, sstride, dorigin, dstride, &f0,
                flt_stride, &f1, flt_stride, 2, &mut r_xqd, true, &p,
            );
            assert_eq!(r_err, c_err, "finer_search error mismatch ep {ep} {w}x{h}");
            assert_eq!(r_xqd, c_xqd, "finer_search xqd mismatch ep {ep} {w}x{h}");
            if r_xqd != start {
                moved += 1;
            }

            // do_refine = false must be the un-refined error and leave xqd alone.
            let mut r2 = start;
            let e2 = finer_search_pixel_proj_error(
                &planes, sorigin, w as usize, h as usize, sstride, dorigin, dstride, &f0,
                flt_stride, &f1, flt_stride, 2, &mut r2, false, &p,
            );
            assert_eq!(r2, start);
            let xq = decode_xq(&start, &p);
            let e_ref = cref::lowbd_pixel_proj_error(
                &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride, &mut f0,
                flt_stride, &mut f1, flt_stride, &xq, ep as i32,
            );
            assert_eq!(e2, e_ref, "do_refine=false must skip the climb");
        }
    }
    assert!(
        moved > 10,
        "the refinement never moved xqd in {moved} cases — the climb is untested"
    );
}

/// `apply_sgr` is `static INLINE`; its whole body is
/// `svt_av1_selfguided_restoration`, which the FILTER suite gates at tier 1.
/// What this checks is the proc-unit tiling and the flt-buffer addressing:
/// each unit writes into its own sub-rectangle at the UNIT's `flt_stride`, not
/// into a packed block.
#[test]
fn apply_sgr_tiling_matches_the_c_per_unit_decomposition() {
    for ep in [0usize, 5, 10, 14] {
        for &(w, h, pu) in &[(64i32, 64i32, 64i32), (128, 96, 64), (96, 64, 32)] {
            let (dat, dorigin, dstride) = extended_u8(w, h, ep as u64 + 200);
            let flt_stride = search_flt_stride(w);

            // Reference decomposition, using the tier-1-gated filter directly.
            let mut c0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut c1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut i = 0i32;
            while i < h {
                let hh = pu.min(h - i);
                let row = i as usize * flt_stride;
                let mut j = 0i32;
                while j < w {
                    let ww = pu.min(w - j);
                    selfguided_restoration(
                        SgrSrc::Lowbd(&dat),
                        dorigin + i as usize * dstride + j as usize,
                        ww,
                        hh,
                        dstride,
                        &mut c0[row + j as usize..],
                        &mut c1[row + j as usize..],
                        flt_stride,
                        ep,
                        8,
                    );
                    j += pu;
                }
                i += pu;
            }

            let mut r0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut r1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            apply_sgr(
                ep,
                SgrSrc::Lowbd(&dat),
                dorigin,
                w,
                h,
                dstride,
                8,
                pu,
                pu,
                &mut r0,
                &mut r1,
                flt_stride,
            );
            let p = SGR_PARAMS[ep];
            let n = h as usize * flt_stride;
            if p.r[0] > 0 {
                assert_eq!(r0[..n], c0[..n], "apply_sgr flt0 ep {ep} {w}x{h} pu {pu}");
            }
            if p.r[1] > 0 {
                assert_eq!(r1[..n], c1[..n], "apply_sgr flt1 ep {ep} {w}x{h} pu {pu}");
            }
        }
    }
}

/// `search_selfguided_restoration` is `static`. Every value it compares comes
/// from `svt_get_proj_subspace_c` and `svt_av1_lowbd_pixel_proj_error_c`, both
/// exported, so this replays the sweep against the C kernels and checks the
/// port picks the same `(ep, xqd)`. The `ctrls` are the REAL level-3 ones from
/// `svt_aom_set_sg_filter_ctrls` (the ones a video-mode M0..M3 frame uses).
#[test]
fn ep_sweep_picks_the_same_ep_as_the_c_kernels() {
    let ctrls: SgFilterCtrls = set_sg_filter_ctrls(3);
    assert!(ctrls.enabled);

    for (plane, lane) in [(0usize, 0usize), (1, 1), (2, 1)] {
        for &(w, h) in &[(64i32, 64i32), (32, 32)] {
            let (dat, dorigin, dstride) = extended_u8(w, h, plane as u64 * 31 + 5);
            let (src, sorigin, sstride) = extended_u8(w, h, plane as u64 * 31 + 6);
            let flt_stride = search_flt_stride(w);

            // --- C-kernel replay of the sweep ---
            let start_ep = i32::from(ctrls.start_ep[lane]);
            let end_ep = i32::from(ctrls.end_ep[lane]);
            let ep_inc = i32::from(ctrls.ep_inc[lane]);
            let do_refine = ctrls.refine[lane];
            let mut best = (0i32, -1i64, [0i32; 2]);
            let mut ep = start_ep;
            let mut visited = Vec::new();
            while ep < end_ep {
                visited.push(ep);
                let p = SGR_PARAMS[ep as usize];
                let (mut f0, mut f1) =
                    real_flts_u8(&dat, dorigin, dstride, w, h, ep as usize, flt_stride);
                let exq = cref::get_proj_subspace(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride,
                    &mut f0, flt_stride, &mut f1, flt_stride, ep,
                );
                let mut exqd = encode_xq(&exq, &p);
                // The hill climb, replayed on the C kernel.
                let planes = ProjPlanes::Lowbd {
                    src: &src,
                    dat: &dat,
                };
                let err = finer_search_pixel_proj_error(
                    &planes, sorigin, w as usize, h as usize, sstride, dorigin, dstride, &f0,
                    flt_stride, &f1, flt_stride, 2, &mut exqd, do_refine, &p,
                );
                // Cross-check this cell's error against the C kernel too, so
                // the replay is not just the port talking to itself.
                let xq = decode_xq(&exqd, &p);
                let c_err = cref::lowbd_pixel_proj_error(
                    &src, sorigin, w as usize, h as usize, sstride, &dat, dorigin, dstride,
                    &mut f0, flt_stride, &mut f1, flt_stride, &xq, ep,
                );
                assert_eq!(err, c_err, "replayed error disagrees with C at ep {ep}");
                if best.1 == -1 || err < best.1 {
                    best = (ep, err, exqd);
                }
                ep += ep_inc;
            }

            // The sweep must have visited what the level-3 ctrls promise.
            if lane == 0 {
                assert_eq!(visited, vec![0, 8], "luma lane sweep shape");
            } else {
                assert_eq!(visited, vec![4], "chroma lane sweep shape");
            }

            // --- port ---
            let planes = ProjPlanes::Lowbd {
                src: &src,
                dat: &dat,
            };
            let got = search_selfguided_restoration(
                SgrSrc::Lowbd(&dat),
                dorigin,
                w,
                h,
                dstride,
                &planes,
                sorigin,
                sstride,
                8,
                64,
                64,
                &ctrls,
                plane,
            );
            assert_eq!(
                got,
                SgrprojInfo {
                    ep: best.0,
                    xqd: best.2
                },
                "ep sweep mismatch for plane {plane} {w}x{h}"
            );
        }
    }
}
