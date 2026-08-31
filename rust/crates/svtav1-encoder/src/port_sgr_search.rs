//! Self-guided (SGR) restoration SEARCH — a port of the SGR half of
//! `Codec/restoration_pick.c`.
//!
//! The FILTER half lives in `svtav1_dsp::port_sgr`; this is the encoder-side
//! search that picks `(ep, xqd)` and prices the result.
//!
//! # Reachability
//!
//! Live at presets 0..3 in VIDEO mode — see `port_sgr`'s module doc and
//! `rust/CLAUDE.md` guard 5d for the derivation (`pd_process.c:4935-4938` ->
//! `svt_aom_get_sg_filter_level_default` -> level 3). The all-intra path never
//! reaches it, which is why the port did not have it.
//!
//! # What is here and what is NOT
//!
//! Ported: `svt_av1_lowbd_pixel_proj_error_c`,
//! `svt_av1_highbd_pixel_proj_error_c`, `get_pixel_proj_error`,
//! `finer_search_pixel_proj_error`, `svt_get_proj_subspace_c`, `encode_xq`,
//! `apply_sgr`, `search_selfguided_restoration`, `count_sgrproj_bits`, and the
//! decision bodies of `search_sgrproj_finish` and `search_switchable`.
//!
//! NOT here: the frame-walk plumbing that calls them
//! (`restoration_seg_search` / `rest_finish_search` / `try_restoration_unit_seg`
//! and the `RestSearchCtxt` they thread). Those live in
//! `svtav1-encoder/src/restoration.rs`, which this lane does not own, so the
//! decision bodies are exposed as pure functions over their inputs and the
//! wiring is a separate change. `search_sgrproj_seg` is therefore represented
//! by [`search_sgrproj_unit`] (its whole body except the `RestSearchCtxt`
//! field reads and the `try_restoration_unit_seg` SSE call).
//!
//! # Evidence
//!
//! `tests/c_parity_sgr_search.rs`:
//! * **Tier 1** for `svt_av1_lowbd_pixel_proj_error_c`,
//!   `svt_av1_highbd_pixel_proj_error_c` and `svt_get_proj_subspace_c` — all
//!   three are exported and driven directly.
//! * **Tier 4** for `encode_xq`, `count_sgrproj_bits`,
//!   `finer_search_pixel_proj_error`, `apply_sgr`,
//!   `search_selfguided_restoration` and the two decision bodies: every one is
//!   `static` (or `static INLINE`) in `restoration_pick.c` with no exported
//!   symbol, and the only exported driver is the whole
//!   `svt_aom_restoration_seg_search`, which needs a built `RestSearchCtxt` +
//!   `Av1Common` + `PictureControlSet`. Where a tier-4 body is BUILT OUT OF
//!   tier-1 pieces — `finer_search_pixel_proj_error` and
//!   `search_selfguided_restoration` both consist entirely of calls into
//!   exported kernels — the tests drive the C kernels through the port's own
//!   control flow and compare against the port's, so the arithmetic is tier 1
//!   even though the loop structure is tier 4. That distinction is stated per
//!   test rather than papered over.

use svtav1_dsp::port_sgr::{
    RESTORATION_PROC_UNIT_SIZE, RESTORATION_UNITPELS_MAX, SGR_PARAMS, SGRPROJ_PARAMS_BITS,
    SGRPROJ_PRJ_BITS, SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1, SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1,
    SGRPROJ_PRJ_SUBEXP_K, SGRPROJ_RST_BITS, SgrParamsType, SgrSrc, decode_xq,
    selfguided_restoration,
};

/// `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:29).
const AV1_PROB_COST_SHIFT: u32 = 9;

/// `SgrprojInfo` (restoration.h) — what a unit signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SgrprojInfo {
    pub ep: i32,
    pub xqd: [i32; 2],
}

/// `SgFilterCtrls`, re-exported shape from [`crate::port_lr_level`] so this
/// module's signatures read like C's.
pub use crate::port_lr_level::SgFilterCtrls;

/// `ROUND_POWER_OF_TWO(value, n)` on `i32`.
#[inline]
const fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + ((1 << n) >> 1)) >> n
}

// --------------------------------------------------------------------------
// Distortion (restoration_pick.c:161 / :228 / :301)
// --------------------------------------------------------------------------

/// Port of `svt_av1_lowbd_pixel_proj_error_c` (restoration_pick.c:161) — the
/// SSE of an SGR-projected unit against the source, which is the distortion
/// term every SGR RD comparison uses.
///
/// C has FOUR separate loops (both filters / only r0 / only r1 / neither),
/// not one loop with conditionals, and they are NOT algebraically identical:
/// the "neither" arm compares `dat - src` directly with no `SGRPROJ_RST_BITS`
/// round-trip at all. The port keeps all four.
#[allow(clippy::too_many_arguments)]
pub fn lowbd_pixel_proj_error(
    src: &[u8],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u8],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &[i32],
    flt0_stride: usize,
    flt1: &[i32],
    flt1_stride: usize,
    xq: &[i32; 2],
    params: &SgrParamsType,
) -> i64 {
    let mut err: i64 = 0;
    let shift = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    for i in 0..height {
        let sr = src_origin + i * src_stride;
        let dr = dat_origin + i * dat_stride;
        let f0 = i * flt0_stride;
        let f1 = i * flt1_stride;
        for j in 0..width {
            let e = if params.r[0] > 0 && params.r[1] > 0 {
                let u = i32::from(dat[dr + j]) << SGRPROJ_RST_BITS;
                let mut v = u << SGRPROJ_PRJ_BITS;
                v += xq[0] * (flt0[f0 + j] - u) + xq[1] * (flt1[f1 + j] - u);
                round_power_of_two(v, shift) - i32::from(src[sr + j])
            } else if params.r[0] > 0 {
                let u = i32::from(dat[dr + j]) << SGRPROJ_RST_BITS;
                let mut v = u << SGRPROJ_PRJ_BITS;
                v += xq[0] * (flt0[f0 + j] - u);
                round_power_of_two(v, shift) - i32::from(src[sr + j])
            } else if params.r[1] > 0 {
                let u = i32::from(dat[dr + j]) << SGRPROJ_RST_BITS;
                let mut v = u << SGRPROJ_PRJ_BITS;
                v += xq[1] * (flt1[f1 + j] - u);
                round_power_of_two(v, shift) - i32::from(src[sr + j])
            } else {
                // NOTE: no RST_BITS round-trip in this arm at all.
                i32::from(dat[dr + j]) - i32::from(src[sr + j])
            };
            err += i64::from(e) * i64::from(e);
        }
    }
    err
}

/// Port of `svt_av1_highbd_pixel_proj_error_c` (restoration_pick.c:228).
///
/// This is NOT the 8-bit kernel with a wider type. C restructures it: it adds
/// a `half = 1 << (RST_BITS + PRJ_BITS - 1)` bias, drops the `u << PRJ_BITS`
/// term, uses a PLAIN `>>` (not `ROUND_POWER_OF_TWO`) and then adds `d` back:
/// `e = (v >> shift) + d - s`. Algebraically close to the 8-bit form but not
/// identical in rounding, and the two-filter arm hoists `xq` into locals.
/// Transcribed as written.
#[allow(clippy::too_many_arguments)]
pub fn highbd_pixel_proj_error(
    src: &[u16],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u16],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &[i32],
    flt0_stride: usize,
    flt1: &[i32],
    flt1_stride: usize,
    xq: &[i32; 2],
    params: &SgrParamsType,
) -> i64 {
    let mut err: i64 = 0;
    let shift = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    let half = 1i32 << (shift - 1);

    if params.r[0] > 0 && params.r[1] > 0 {
        let (xq0, xq1) = (xq[0], xq[1]);
        for i in 0..height {
            let sr = src_origin + i * src_stride;
            let dr = dat_origin + i * dat_stride;
            let f0 = i * flt0_stride;
            let f1 = i * flt1_stride;
            for j in 0..width {
                let d = i32::from(dat[dr + j]);
                let s = i32::from(src[sr + j]);
                let u = d << SGRPROJ_RST_BITS;
                let mut v = half;
                v += xq0 * (flt0[f0 + j] - u);
                v += xq1 * (flt1[f1 + j] - u);
                let e = (v >> shift) + d - s;
                err += i64::from(e) * i64::from(e);
            }
        }
    } else if params.r[0] > 0 || params.r[1] > 0 {
        let (exq, flt, flt_stride) = if params.r[0] > 0 {
            (xq[0], flt0, flt0_stride)
        } else {
            (xq[1], flt1, flt1_stride)
        };
        for i in 0..height {
            let sr = src_origin + i * src_stride;
            let dr = dat_origin + i * dat_stride;
            let fr = i * flt_stride;
            for j in 0..width {
                let d = i32::from(dat[dr + j]);
                let s = i32::from(src[sr + j]);
                let u = d << SGRPROJ_RST_BITS;
                let mut v = half;
                v += exq * (flt[fr + j] - u);
                let e = (v >> shift) + d - s;
                err += i64::from(e) * i64::from(e);
            }
        }
    } else {
        for i in 0..height {
            let sr = src_origin + i * src_stride;
            let dr = dat_origin + i * dat_stride;
            for j in 0..width {
                let e = i32::from(dat[dr + j]) - i32::from(src[sr + j]);
                err += i64::from(e) * i64::from(e);
            }
        }
    }
    err
}

/// A (source, degraded) pair at one bit depth, so the dispatcher below cannot
/// be handed an 8-bit buffer with `use_highbitdepth = 1`.
pub enum ProjPlanes<'a> {
    Lowbd { src: &'a [u8], dat: &'a [u8] },
    Highbd { src: &'a [u16], dat: &'a [u16] },
}

/// Port of `get_pixel_proj_error` (restoration_pick.c:301) — decodes `xqd`
/// into `xq` and dispatches on bit depth.
#[allow(clippy::too_many_arguments)]
pub fn get_pixel_proj_error(
    planes: &ProjPlanes<'_>,
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat_origin: usize,
    dat_stride: usize,
    flt0: &[i32],
    flt0_stride: usize,
    flt1: &[i32],
    flt1_stride: usize,
    xqd: &[i32; 2],
    params: &SgrParamsType,
) -> i64 {
    let xq = decode_xq(xqd, params);
    match planes {
        ProjPlanes::Lowbd { src, dat } => lowbd_pixel_proj_error(
            src,
            src_origin,
            width,
            height,
            src_stride,
            dat,
            dat_origin,
            dat_stride,
            flt0,
            flt0_stride,
            flt1,
            flt1_stride,
            &xq,
            params,
        ),
        ProjPlanes::Highbd { src, dat } => highbd_pixel_proj_error(
            src,
            src_origin,
            width,
            height,
            src_stride,
            dat,
            dat_origin,
            dat_stride,
            flt0,
            flt0_stride,
            flt1,
            flt1_stride,
            &xq,
            params,
        ),
    }
}

/// Port of `finer_search_pixel_proj_error` (restoration_pick.c:324) — the
/// +-`s` refinement of the two `xqd` components, gated by
/// `SgFilterCtrls::refine` (1 for lane 0 at sg level 3).
///
/// The control flow is fiddly and is transcribed literally:
/// * `s` halves from `start_step` down to 1.
/// * For each component `p`, C tries `-s` FIRST. If that improves, it sets
///   `skip = 1` and — at the TOP step only — keeps stepping in the same
///   direction; then `break`s out of the `p` loop entirely (`if (skip) break;`),
///   so component 1 is NOT tried at that step size.
/// * Only if `-s` did not improve does it try `+s`, with the same
///   keep-going-at-the-top-step rule but WITHOUT setting `skip`.
///
/// `xqd` is refined IN PLACE, and the returned error corresponds to the
/// refined value.
#[allow(clippy::too_many_arguments)]
pub fn finer_search_pixel_proj_error(
    planes: &ProjPlanes<'_>,
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat_origin: usize,
    dat_stride: usize,
    flt0: &[i32],
    flt0_stride: usize,
    flt1: &[i32],
    flt1_stride: usize,
    start_step: i32,
    xqd: &mut [i32; 2],
    do_refine: bool,
    params: &SgrParamsType,
) -> i64 {
    let err_of = |xqd: &[i32; 2]| {
        get_pixel_proj_error(
            planes,
            src_origin,
            width,
            height,
            src_stride,
            dat_origin,
            dat_stride,
            flt0,
            flt0_stride,
            flt1,
            flt1_stride,
            xqd,
            params,
        )
    };
    let mut err = err_of(xqd);
    if !do_refine {
        return err;
    }

    let tap_min = [SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1];
    let tap_max = [SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1];

    let mut s = start_step;
    while s >= 1 {
        for p in 0..2usize {
            if (params.r[0] == 0 && p == 0) || (params.r[1] == 0 && p == 1) {
                continue;
            }
            let mut skip = false;
            // C's `do { ... } while (1)` with a trailing `break`: the loop
            // body repeats only via the explicit `continue` at the top step.
            loop {
                if xqd[p] - s >= tap_min[p] {
                    xqd[p] -= s;
                    let err2 = err_of(xqd);
                    if err2 > err {
                        xqd[p] += s;
                    } else {
                        err = err2;
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
                if xqd[p] + s <= tap_max[p] {
                    xqd[p] += s;
                    let err2 = err_of(xqd);
                    if err2 > err {
                        xqd[p] -= s;
                    } else {
                        err = err2;
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
    err
}

// --------------------------------------------------------------------------
// The subspace solve (restoration_pick.c:422)
// --------------------------------------------------------------------------

/// Port of `svt_get_proj_subspace_c` (restoration_pick.c:422) — solves the
/// 2x2 normal equations for the projection weights given `flt0`/`flt1` and the
/// source.
///
/// This is `double` arithmetic in C, so it is `f64` here, accumulated in
/// exactly C's order (row-major, `H` before `C`, one pass) because f64
/// addition is not associative and a reordered sum is a different number.
/// `rint` is round-half-to-EVEN under the default rounding mode, i.e. Rust's
/// `f64::round_ties_even`, NOT `f64::round` (which is half-away-from-zero).
///
/// Returns `[0, 0]` on an ill-posed system (`det < 1e-8`), like C.
#[allow(clippy::too_many_arguments)]
pub fn get_proj_subspace(
    planes: &ProjPlanes<'_>,
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat_origin: usize,
    dat_stride: usize,
    flt0: &[i32],
    flt0_stride: usize,
    flt1: &[i32],
    flt1_stride: usize,
    params: &SgrParamsType,
) -> [i32; 2] {
    let mut h = [[0.0f64; 2]; 2];
    let mut c = [0.0f64; 2];
    let size = (width * height) as f64;

    let mut accumulate = |dv: i32, sv: i32, f0: i32, f1: i32| {
        let u = f64::from(dv << SGRPROJ_RST_BITS);
        let s = f64::from(sv << SGRPROJ_RST_BITS) - u;
        let f1v = if params.r[0] > 0 {
            f64::from(f0) - u
        } else {
            0.0
        };
        let f2v = if params.r[1] > 0 {
            f64::from(f1) - u
        } else {
            0.0
        };
        h[0][0] += f1v * f1v;
        h[1][1] += f2v * f2v;
        h[0][1] += f1v * f2v;
        c[0] += f1v * s;
        c[1] += f2v * s;
    };

    for i in 0..height {
        for j in 0..width {
            let (dv, sv) = match planes {
                ProjPlanes::Lowbd { src, dat } => (
                    i32::from(dat[dat_origin + i * dat_stride + j]),
                    i32::from(src[src_origin + i * src_stride + j]),
                ),
                ProjPlanes::Highbd { src, dat } => (
                    i32::from(dat[dat_origin + i * dat_stride + j]),
                    i32::from(src[src_origin + i * src_stride + j]),
                ),
            };
            accumulate(dv, sv, flt0[i * flt0_stride + j], flt1[i * flt1_stride + j]);
        }
    }

    h[0][0] /= size;
    h[0][1] /= size;
    h[1][1] /= size;
    h[1][0] = h[0][1];
    c[0] /= size;
    c[1] /= size;

    let scale = f64::from(1 << SGRPROJ_PRJ_BITS);
    if params.r[0] == 0 {
        let det = h[1][1];
        if det < 1e-8 {
            return [0, 0];
        }
        [0, (c[1] / det * scale).round_ties_even() as i32]
    } else if params.r[1] == 0 {
        let det = h[0][0];
        if det < 1e-8 {
            return [0, 0];
        }
        [(c[0] / det * scale).round_ties_even() as i32, 0]
    } else {
        let det = h[0][0] * h[1][1] - h[0][1] * h[1][0];
        if det < 1e-8 {
            return [0, 0];
        }
        let x0 = (h[1][1] * c[0] - h[0][1] * c[1]) / det;
        let x1 = (h[0][0] * c[1] - h[1][0] * c[0]) / det;
        [
            (x0 * scale).round_ties_even() as i32,
            (x1 * scale).round_ties_even() as i32,
        ]
    }
}

// --------------------------------------------------------------------------
// encode_xq / apply_sgr / the ep sweep (restoration_pick.c:512 / :526 / :554)
// --------------------------------------------------------------------------

/// Port of `encode_xq` (restoration_pick.c:512) — maps the solved `xq` to the
/// SIGNALLED `xqd` pair.
///
/// This is NOT the inverse of `svt_decode_xq`, and the difference is
/// load-bearing: in the `r[1] == 0` arm C computes
/// `xqd[1] = clamp((1 << PRJ_BITS) - xqd[0], ...)` — note `xqd[0]`, the
/// ALREADY-CLAMPED value, not `xq[0]` — while `decode_xq` returns `xq[1] = 0`
/// for that case and never reads `xqd[1]`. Likewise the third arm subtracts
/// the clamped `xqd[0]`. Reading `xq[0]` there instead would produce a
/// different signalled pair whenever the clamp bites.
#[inline]
pub fn encode_xq(xq: &[i32; 2], params: &SgrParamsType) -> [i32; 2] {
    if params.r[0] == 0 {
        [
            0,
            ((1 << SGRPROJ_PRJ_BITS) - xq[1]).clamp(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
        ]
    } else if params.r[1] == 0 {
        let x0 = xq[0].clamp(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0);
        [
            x0,
            ((1 << SGRPROJ_PRJ_BITS) - x0).clamp(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
        ]
    } else {
        let x0 = xq[0].clamp(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0);
        [
            x0,
            ((1 << SGRPROJ_PRJ_BITS) - x0 - xq[1]).clamp(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
        ]
    }
}

/// Port of `apply_sgr` (restoration_pick.c:526) — runs the SGR filter over a
/// whole restoration unit in `pu_height` x `pu_width` processing units,
/// producing `flt0`/`flt1` for the subspace solve.
///
/// Note the flt buffers are indexed at the UNIT's stride, so each processing
/// unit writes into its own sub-rectangle rather than a packed block.
#[allow(clippy::too_many_arguments)]
pub fn apply_sgr(
    sgr_params_idx: usize,
    dat: SgrSrc<'_>,
    dat_origin: usize,
    width: i32,
    height: i32,
    dat_stride: usize,
    bit_depth: i32,
    pu_width: i32,
    pu_height: i32,
    flt0: &mut [i32],
    flt1: &mut [i32],
    flt_stride: usize,
) {
    let mut i = 0i32;
    while i < height {
        let h = pu_height.min(height - i);
        let row = i as usize * flt_stride;
        let dat_row = dat_origin + i as usize * dat_stride;
        let mut j = 0i32;
        while j < width {
            let w = pu_width.min(width - j);
            selfguided_restoration(
                dat,
                dat_row + j as usize,
                w,
                h,
                dat_stride,
                &mut flt0[row + j as usize..],
                &mut flt1[row + j as usize..],
                flt_stride,
                sgr_params_idx,
                bit_depth,
            );
            j += pu_width;
        }
        i += pu_height;
    }
}

/// `flt_stride` used by `search_selfguided_restoration`
/// (restoration_pick.c:562): `((width + 7) & ~7) + 8`. Different from the
/// filter's own `ab_buf_stride`; both are load-bearing where they appear.
#[inline]
pub fn search_flt_stride(width: i32) -> usize {
    (((width + 7) & !7) + 8) as usize
}

/// Port of `search_selfguided_restoration` (restoration_pick.c:554) — the `ep`
/// sweep proper.
///
/// `ctrls.start_ep/end_ep/ep_inc/refine` are indexed by `!!plane` (0 for luma,
/// 1 for ANY chroma plane) — C writes `plane = !!plane;`, so U and V share
/// lane 1. At sg level 3 that is `ep 0..16 step 8` with refinement for luma and
/// `ep 4..5` without for chroma.
///
/// The `besterr == -1` sentinel means "no candidate yet"; C's comparison is
/// strict `<`, so the FIRST `ep` wins ties.
#[allow(clippy::too_many_arguments)]
pub fn search_selfguided_restoration(
    dat: SgrSrc<'_>,
    dat_origin: usize,
    width: i32,
    height: i32,
    dat_stride: usize,
    planes: &ProjPlanes<'_>,
    src_origin: usize,
    src_stride: usize,
    bit_depth: i32,
    pu_width: i32,
    pu_height: i32,
    ctrls: &SgFilterCtrls,
    plane: usize,
) -> SgrprojInfo {
    debug_assert!(
        pu_width == RESTORATION_PROC_UNIT_SIZE >> 1 || pu_width == RESTORATION_PROC_UNIT_SIZE
    );
    debug_assert!(
        pu_height == RESTORATION_PROC_UNIT_SIZE >> 1 || pu_height == RESTORATION_PROC_UNIT_SIZE
    );
    let flt_stride = search_flt_stride(width);
    let mut flt0 = alloc::vec![0i32; RESTORATION_UNITPELS_MAX];
    let mut flt1 = alloc::vec![0i32; RESTORATION_UNITPELS_MAX];

    // C: `plane = !!plane;` — U and V share the chroma lane.
    let lane = usize::from(plane > 0);
    let start_ep = i32::from(ctrls.start_ep[lane]);
    let end_ep = i32::from(ctrls.end_ep[lane]);
    let ep_inc = i32::from(ctrls.ep_inc[lane]);
    let do_refine = ctrls.refine[lane];

    let mut bestep = 0i32;
    let mut besterr = -1i64;
    let mut bestxqd = [0i32; 2];

    let mut ep = start_ep;
    while ep < end_ep {
        apply_sgr(
            ep as usize,
            dat,
            dat_origin,
            width,
            height,
            dat_stride,
            bit_depth,
            pu_width,
            pu_height,
            &mut flt0,
            &mut flt1,
            flt_stride,
        );
        let params = &SGR_PARAMS[ep as usize];
        let exq = get_proj_subspace(
            planes,
            src_origin,
            width as usize,
            height as usize,
            src_stride,
            dat_origin,
            dat_stride,
            &flt0,
            flt_stride,
            &flt1,
            flt_stride,
            params,
        );
        let mut exqd = encode_xq(&exq, params);
        let err = finer_search_pixel_proj_error(
            planes,
            src_origin,
            width as usize,
            height as usize,
            src_stride,
            dat_origin,
            dat_stride,
            &flt0,
            flt_stride,
            &flt1,
            flt_stride,
            2,
            &mut exqd,
            do_refine,
            params,
        );
        if besterr == -1 || err < besterr {
            bestep = ep;
            besterr = err;
            bestxqd = exqd;
        }
        ep += ep_inc;
    }

    SgrprojInfo {
        ep: bestep,
        xqd: bestxqd,
    }
}

/// The body of `search_sgrproj_seg` (restoration_pick.c:1237) minus the two
/// `RestSearchCtxt` reads C wraps it in: it computes the proc-unit geometry
/// from the plane's subsampling and runs the sweep. The caller still has to
/// call `try_restoration_unit_seg` for `sse[RESTORE_SGRPROJ]`, which lives in
/// `restoration.rs`.
///
/// `procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x` and likewise for
/// height, with `ss_* = is_uv && cm->subsampling_*`.
#[allow(clippy::too_many_arguments)]
pub fn search_sgrproj_unit(
    dat: SgrSrc<'_>,
    dat_origin: usize,
    width: i32,
    height: i32,
    dat_stride: usize,
    planes: &ProjPlanes<'_>,
    src_origin: usize,
    src_stride: usize,
    bit_depth: i32,
    plane: usize,
    subsampling_x: bool,
    subsampling_y: bool,
    ctrls: &SgFilterCtrls,
) -> SgrprojInfo {
    let is_uv = plane > 0;
    let ss_x = i32::from(is_uv && subsampling_x);
    let ss_y = i32::from(is_uv && subsampling_y);
    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let procunit_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    search_selfguided_restoration(
        dat,
        dat_origin,
        width,
        height,
        dat_stride,
        planes,
        src_origin,
        src_stride,
        bit_depth,
        procunit_width,
        procunit_height,
        ctrls,
        plane,
    )
}

// --------------------------------------------------------------------------
// Rate + the two decisions (restoration_pick.c:634 / :1276 / :1154)
// --------------------------------------------------------------------------

/// Port of `count_sgrproj_bits` (restoration_pick.c:634) — the rate of an SGR
/// unit: the `ep` literal plus a `refsubexpfin` delta per LIVE component.
///
/// A component whose radius is 0 costs nothing, because it is not signalled.
pub fn count_sgrproj_bits(info: &SgrprojInfo, ref_info: &SgrprojInfo) -> i32 {
    let mut bits = SGRPROJ_PARAMS_BITS;
    let params = &SGR_PARAMS[info.ep as usize];
    if params.r[0] > 0 {
        bits += crate::entropy::lr::count_primitive_refsubexpfin(
            (SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K as u16,
            (ref_info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
            (info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
        );
    }
    if params.r[1] > 0 {
        bits += crate::entropy::lr::count_primitive_refsubexpfin(
            (SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K as u16,
            (ref_info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
            (info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
        );
    }
    bits
}

/// C `RDCOST_DBL` (restoration.h:344) — rate already `>> 4`-ed by the caller.
#[inline]
fn rdcost_dbl(rdmult: i64, rate: i64, dist: i64) -> f64 {
    (rate as f64 * rdmult as f64) / f64::from(1u32 << 9) + dist as f64 * f64::from(1u32 << 7)
}

/// The outcome of one unit's SGR-vs-NONE decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SgrprojFinish {
    /// `RESTORE_SGRPROJ` (2) when SGR won, `RESTORE_NONE` (0) otherwise.
    pub chose_sgr: bool,
    /// The `bits` term the frame accumulator adds.
    pub bits: i64,
    /// The `sse` term the frame accumulator adds.
    pub sse: i64,
}

/// Port of the decision in `search_sgrproj_finish` (restoration_pick.c:1276).
///
/// `sgrproj_restore_cost[0..2]` are the symbol costs of the per-unit
/// `sgrproj_restore` flag. Ties go to NONE — C's test is strictly
/// `cost_sgr < cost_none`.
#[allow(clippy::too_many_arguments)]
pub fn sgrproj_finish_decision(
    rdmult: i64,
    sgrproj_restore_cost: [i64; 2],
    info: &SgrprojInfo,
    ref_info: &SgrprojInfo,
    sse_none: i64,
    sse_sgr: i64,
) -> SgrprojFinish {
    let bits_none = sgrproj_restore_cost[0];
    let bits_sgr = sgrproj_restore_cost[1]
        + (i64::from(count_sgrproj_bits(info, ref_info)) << AV1_PROB_COST_SHIFT);
    let cost_none = rdcost_dbl(rdmult, bits_none >> 4, sse_none);
    let cost_sgr = rdcost_dbl(rdmult, bits_sgr >> 4, sse_sgr);
    let chose_sgr = cost_sgr < cost_none;
    SgrprojFinish {
        chose_sgr,
        bits: if chose_sgr { bits_sgr } else { bits_none },
        sse: if chose_sgr { sse_sgr } else { sse_none },
    }
}

/// `RestorationType` discriminants (definitions.h): the values
/// `search_switchable` indexes with.
pub const RESTORE_NONE: usize = 0;
pub const RESTORE_WIENER: usize = 1;
pub const RESTORE_SGRPROJ: usize = 2;
/// `RESTORE_SWITCHABLE_TYPES` — the three candidates switchable chooses from.
pub const RESTORE_SWITCHABLE_TYPES: usize = 3;

/// The outcome of one unit's switchable decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchableChoice {
    /// `RESTORE_NONE` / `RESTORE_WIENER` / `RESTORE_SGRPROJ`.
    pub best_rtype: usize,
    pub bits: i64,
    pub sse: i64,
}

/// Port of `search_switchable` (restoration_pick.c:1154) — the per-unit
/// three-way decision that `RESTORE_SWITCHABLE` needs. Without it the port
/// cannot emit `RESTORE_SWITCHABLE` at all, so the frame's
/// `frame_restoration_type` field itself is wrong.
///
/// Two details that a paraphrase gets wrong:
/// * A candidate whose own per-unit search already lost to NONE is SKIPPED,
///   not re-priced: `if (r > RESTORE_NONE && rusi->best_rtype[r-1] ==
///   RESTORE_NONE) continue;`. So `best_rtype_wiener` / `best_rtype_sgr` here
///   are the results of the earlier `search_*_finish` passes, and passing
///   `false` for either removes that candidate entirely.
/// * `r == 0 || cost < best_cost` — NONE is always taken first and ties go to
///   the EARLIER type.
///
/// `coeff_pcost_wiener` is `count_wiener_bits` at the SYNTAX window (7-tap
/// luma), which is a different window from the one `search_wiener_finish`
/// prices with — see `docs/SUSPECTED-C-BUGS.md` #7. That asymmetry is C's and
/// this function does not correct it; the caller supplies the value.
#[allow(clippy::too_many_arguments)]
pub fn switchable_decision(
    rdmult: i64,
    switchable_restore_cost: [i64; 3],
    sse: [i64; 3],
    wiener_available: bool,
    coeff_pcost_wiener: i32,
    sgr_available: bool,
    coeff_pcost_sgr: i32,
) -> SwitchableChoice {
    let mut best_cost = 0.0f64;
    let mut best_bits = 0i64;
    let mut best_rtype = RESTORE_NONE;

    for r in 0..RESTORE_SWITCHABLE_TYPES {
        if r == RESTORE_WIENER && !wiener_available {
            continue;
        }
        if r == RESTORE_SGRPROJ && !sgr_available {
            continue;
        }
        let coeff_pcost = match r {
            RESTORE_NONE => 0,
            RESTORE_WIENER => i64::from(coeff_pcost_wiener),
            RESTORE_SGRPROJ => i64::from(coeff_pcost_sgr),
            _ => unreachable!(),
        };
        let coeff_bits = coeff_pcost << AV1_PROB_COST_SHIFT;
        let bits = switchable_restore_cost[r] + coeff_bits;
        let cost = rdcost_dbl(rdmult, bits >> 4, sse[r]);
        if r == 0 || cost < best_cost {
            best_cost = cost;
            best_bits = bits;
            best_rtype = r;
        }
    }

    SwitchableChoice {
        best_rtype,
        bits: best_bits,
        sse: sse[best_rtype],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_xq_is_not_the_inverse_of_decode_xq_in_the_r1_zero_arm() {
        // ep 14 has r[1] == 0. encode_xq derives xqd[1] from the CLAMPED
        // xqd[0]; decode_xq ignores xqd[1] entirely and returns xq[1] = 0.
        let p = SGR_PARAMS[14];
        let xqd = encode_xq(&[1000, 0], &p);
        assert_eq!(xqd[0], SGRPROJ_PRJ_MAX0, "xq[0] must clamp");
        assert_eq!(
            xqd[1],
            ((1 << SGRPROJ_PRJ_BITS) - SGRPROJ_PRJ_MAX0).clamp(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1)
        );
        assert_eq!(decode_xq(&xqd, &p), [xqd[0], 0]);
    }

    #[test]
    fn switchable_skips_candidates_that_already_lost_to_none() {
        // Wiener has the lowest SSE, but it lost its own finish pass, so it
        // must not be considered at all.
        let choice = switchable_decision(
            1000,
            [10, 20, 30],
            [1_000_000, 1, 500_000],
            false, // wiener unavailable
            0,
            true,
            0,
        );
        assert_eq!(choice.best_rtype, RESTORE_SGRPROJ);
        // With Wiener admitted, it wins.
        let choice = switchable_decision(
            1000,
            [10, 20, 30],
            [1_000_000, 1, 500_000],
            true,
            0,
            true,
            0,
        );
        assert_eq!(choice.best_rtype, RESTORE_WIENER);
    }

    #[test]
    fn switchable_ties_go_to_the_earlier_type() {
        // Identical bits and sse for all three -> NONE wins (C's `r == 0 ||
        // cost < best_cost`, strict <).
        let choice = switchable_decision(1000, [10, 10, 10], [500, 500, 500], true, 0, true, 0);
        assert_eq!(choice.best_rtype, RESTORE_NONE);
    }

    #[test]
    fn sgrproj_finish_ties_go_to_none() {
        let info = SgrprojInfo { ep: 0, xqd: [0, 0] };
        let f = sgrproj_finish_decision(1000, [100, 100], &info, &info, 500, 500);
        assert!(!f.chose_sgr, "an exact tie must choose NONE");
    }

    #[test]
    fn search_flt_stride_matches_c_formula() {
        // ((width + 7) & ~7) + 8
        assert_eq!(search_flt_stride(1), 16);
        assert_eq!(search_flt_stride(8), 16);
        assert_eq!(search_flt_stride(9), 24);
        assert_eq!(search_flt_stride(64), 72);
    }
}
