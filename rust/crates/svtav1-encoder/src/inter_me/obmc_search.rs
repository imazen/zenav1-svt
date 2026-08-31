//! The OBMC half of `Source/Lib/Codec/av1me.c` — the motion search used by
//! `OBMC_CAUSAL` blocks — plus the four C_DEFAULT kernels it drives that no
//! other module in this port needed yet.
//!
//! `av1me.c`'s IntraBC half is already ported in [`crate::intrabc`]
//! (`svt_av1_full_pixel_search`, `svt_av1_diamond_search_sad_c`,
//! `exhaustive_mesh_search`, `svt_av1_refining_search_sad`,
//! `full_pixel_diamond`, `intrabc_full_pixel_exhaustive`,
//! `svt_av1_set_mv_search_range`, `svt_av1_init3smotion_compensation`,
//! `svt_av1_get_mvpred_var`, `svt_aom_mv_err_cost{,_light}`,
//! `mvsad_err_cost{,_light}`), and its cost helpers are reused here rather
//! than re-transcribed.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`obmc_sad`] | `obmc_sad` (sad_av1.c:18) + the `OBMCSADMxN` wrappers | wrappers yes |
//! | [`obmc_variance`] | `obmc_variance` (variance.c:225) + `OBMC_VAR` | wrappers yes |
//! | [`obmc_sub_pixel_variance`] | `OBMC_SUBPIX_VAR` (variance.c:259) | wrappers yes |
//! | [`var_filter_block2d_bil_first_pass`] | `aom_var_filter_block2d_bil_first_pass_c` (variance.c:29) | no |
//! | [`var_filter_block2d_bil_second_pass`] | `aom_var_filter_block2d_bil_second_pass_c` (variance.c:55) | no |
//! | [`upsampled_pred`] | `svt_aom_upsampled_pred_c` (variance.c:88) | **yes** |
//! | [`convolve8_horiz`] / [`convolve8_vert`] | `svt_aom_convolve8_{horiz,vert}_c` (convolve.c:288/300) | **yes** |
//! | [`get_obmc_mvpred_var`] | `get_obmc_mvpred_var` (av1me.c:621) | no |
//! | [`obmc_refining_search_sad`] | `obmc_refining_search_sad` (av1me.c:635) | no |
//! | [`obmc_full_pixel_search`] | `svt_av1_obmc_full_pixel_search` (av1me.c:673) | **yes** |
//! | [`set_subpel_mv_search_range`] | `set_subpel_mv_search_range` (av1me.c:694) | no |
//! | [`setup_obmc_center_error`] | `setup_obmc_center_error` (av1me.c:723) | no |
//! | [`upsampled_obmc_pref_error`] | `upsampled_obmc_pref_error` (av1me.c:811) | no |
//! | [`upsampled_setup_obmc_center_error`] | `upsampled_setup_obmc_center_error` (av1me.c:850) | no |
//! | [`find_best_obmc_sub_pixel_tree_up`] | `svt_av1_find_best_obmc_sub_pixel_tree_up` (av1me.c:878) | **yes** |
//!
//! **The upsampled path is the live one.** The single C call site
//! (`mode_decision.c:2148`) passes `USE_8_TAPS`, so
//! `use_accurate_subpel_search` is never 0 in the shipping encoder and the
//! plain `osvf` branch of the tree is dead there. Both branches are ported —
//! per `docs/WORKING-ON-THIS.md` §7 a dead-looking branch stays translated —
//! but do not mistake the cheap one for the one that runs.
//!
//! **Not ported, and why:** the `CONFIG_AV1_HIGHBITDEPTH` arm of
//! `upsampled_obmc_pref_error` (`aom_highbd_upsampled_pred`). That macro is
//! NOT defined anywhere in this C tree — `grep -rn CONFIG_AV1_HIGHBITDEPTH`
//! finds no definition and `is_cur_buf_hbd` does not exist — so the 8-bit arm
//! ported here is the one the reference actually compiles.
//!
//! **A measured upstream defect this port does NOT inherit.** On aarch64 the C
//! RTCD table aliases every `svt_aom_obmc_sub_pixel_variance` above 4x8 to the
//! 4x8 NEON kernel (`aom_dsp_rtcd.c:731-750`), so the C BINARY's `osvf` branch
//! computes a different function from the C SOURCE. The port follows the
//! source. Full evidence and the consequence for testing:
//! `docs/SUSPECTED-C-BUGS.md` #11.

use svtav1_types::motion::{FullMvLimits, Mv};
use svtav1_types::tables::interp::{BILINEAR_FILTERS, InterpKernel, SUB_PEL_FILTERS_8};

use crate::intrabc::{
    MAX_FULL_PEL_VAL, MV_LOW, MV_UPP, MvCostTables, mv_err_cost, mv_err_cost_light, mvsad_err_cost,
};

/// C `bilinear_filters_2t` (filter.h:39) — the 2-tap table the OBMC
/// sub-pixel variance uses (NOT the 8-tap `BILINEAR_FILTERS`).
pub const BILINEAR_FILTERS_2T: [[u8; 2]; 8] = [
    [128, 0],
    [112, 16],
    [96, 32],
    [80, 48],
    [64, 64],
    [48, 80],
    [32, 96],
    [16, 112],
];

/// C `sub_pel_filters_4` (inter_prediction.c:254) — the 4-tap kernel
/// `av1_interp_4tap[EIGHTTAP_REGULAR]` points at. Stored in the same 8-tap
/// layout C uses (`InterpKernel`), with the outer taps zero.
pub const SUB_PEL_FILTERS_4: [InterpKernel; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -4, 126, 8, -2, 0, 0],
    [0, 0, -8, 122, 18, -4, 0, 0],
    [0, 0, -10, 116, 28, -6, 0, 0],
    [0, 0, -12, 110, 38, -8, 0, 0],
    [0, 0, -12, 102, 48, -10, 0, 0],
    [0, 0, -14, 94, 58, -10, 0, 0],
    [0, 0, -12, 84, 66, -10, 0, 0],
    [0, 0, -12, 76, 76, -12, 0, 0],
    [0, 0, -10, 66, 84, -12, 0, 0],
    [0, 0, -10, 58, 94, -14, 0, 0],
    [0, 0, -10, 48, 102, -12, 0, 0],
    [0, 0, -8, 38, 110, -12, 0, 0],
    [0, 0, -6, 28, 116, -10, 0, 0],
    [0, 0, -4, 18, 122, -8, 0, 0],
    [0, 0, -2, 8, 126, -4, 0, 0],
];

/// C `USE_2_TAPS` (definitions.h:857).
pub const USE_2_TAPS: i32 = 1;
/// C `USE_4_TAPS` (definitions.h:858).
pub const USE_4_TAPS: i32 = 2;
/// C `USE_8_TAPS` (definitions.h:859).
pub const USE_8_TAPS: i32 = 3;

/// C `FILTER_BITS`.
const FILTER_BITS: i32 = 7;
/// C `SUBPEL_TAPS`.
const SUBPEL_TAPS: usize = 8;

/// C `av1_get_filter` (variance.c:126). Every `InterpFilterParams` in C
/// carries `taps == SUBPEL_TAPS == 8` — including the "4-tap" entry, whose
/// kernel merely has zero outer taps — so only the TABLE varies.
pub fn av1_get_filter(subpel_search: i32) -> &'static [InterpKernel; 16] {
    match subpel_search {
        USE_2_TAPS => &BILINEAR_FILTERS,
        USE_4_TAPS => &SUB_PEL_FILTERS_4,
        _ => &SUB_PEL_FILTERS_8,
    }
}

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + (1 << (n - 1))) >> n
}

#[inline]
fn round_power_of_two_signed(value: i32, n: i32) -> i32 {
    if value < 0 {
        -round_power_of_two(-value, n)
    } else {
        round_power_of_two(value, n)
    }
}

#[inline]
fn clip_pixel(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// C `obmc_sad` (C_DEFAULT/sad_av1.c:18) — behind every `svt_aom_obmc_sadMxN_c`.
///
/// `wsrc` and `mask` are TIGHTLY packed at `width` per row (C advances them by
/// `width`, not by a stride); only `pre` is strided.
pub fn obmc_sad(
    pre: &[u8],
    pre_stride: usize,
    wsrc: &[i32],
    mask: &[i32],
    width: usize,
    height: usize,
) -> u32 {
    let mut sad = 0u32;
    for y in 0..height {
        let p = &pre[y * pre_stride..];
        let w = &wsrc[y * width..];
        let m = &mask[y * width..];
        for x in 0..width {
            sad = sad
                .wrapping_add(round_power_of_two((w[x] - i32::from(p[x]) * m[x]).abs(), 12) as u32);
        }
    }
    sad
}

/// C `obmc_variance` (C_DEFAULT/variance.c:225): returns `(sse, sum)`.
pub fn obmc_variance(
    pre: &[u8],
    pre_stride: usize,
    wsrc: &[i32],
    mask: &[i32],
    width: usize,
    height: usize,
) -> (u32, i32) {
    let mut sse = 0u32;
    let mut sum = 0i32;
    for y in 0..height {
        let p = &pre[y * pre_stride..];
        let w = &wsrc[y * width..];
        let m = &mask[y * width..];
        for x in 0..width {
            let diff = round_power_of_two_signed(w[x] - i32::from(p[x]) * m[x], 12);
            sum = sum.wrapping_add(diff);
            sse = sse.wrapping_add((diff * diff) as u32);
        }
    }
    (sse, sum)
}

/// C's `OBMC_VAR(W, H)` wrapper (variance.c:243) — i.e.
/// `svt_aom_obmc_varianceWxH_c`. Returns `(return_value, sse)`.
pub fn obmc_variance_wxh(
    pre: &[u8],
    pre_stride: usize,
    wsrc: &[i32],
    mask: &[i32],
    width: usize,
    height: usize,
) -> (u32, u32) {
    let (sse, sum) = obmc_variance(pre, pre_stride, wsrc, mask, width, height);
    let n = (width * height) as i64;
    let adj = ((i64::from(sum) * i64::from(sum)) / n) as u32;
    (sse.wrapping_sub(adj), sse)
}

/// C `aom_var_filter_block2d_bil_first_pass_c` (variance.c:29).
pub fn var_filter_block2d_bil_first_pass(
    a: &[u8],
    b: &mut [u16],
    src_pixels_per_line: usize,
    pixel_step: usize,
    output_height: usize,
    output_width: usize,
    filter: &[u8; 2],
) {
    let mut ai = 0usize;
    let mut bi = 0usize;
    for _ in 0..output_height {
        for j in 0..output_width {
            b[bi + j] = round_power_of_two(
                i32::from(a[ai]) * i32::from(filter[0])
                    + i32::from(a[ai + pixel_step]) * i32::from(filter[1]),
                FILTER_BITS,
            ) as u16;
            ai += 1;
        }
        ai += src_pixels_per_line - output_width;
        bi += output_width;
    }
}

/// C `aom_var_filter_block2d_bil_second_pass_c` (variance.c:55).
pub fn var_filter_block2d_bil_second_pass(
    a: &[u16],
    b: &mut [u8],
    src_pixels_per_line: usize,
    pixel_step: usize,
    output_height: usize,
    output_width: usize,
    filter: &[u8; 2],
) {
    let mut ai = 0usize;
    let mut bi = 0usize;
    for _ in 0..output_height {
        for j in 0..output_width {
            b[bi + j] = round_power_of_two(
                i32::from(a[ai]) * i32::from(filter[0])
                    + i32::from(a[ai + pixel_step]) * i32::from(filter[1]),
                FILTER_BITS,
            ) as u8;
            ai += 1;
        }
        ai += src_pixels_per_line - output_width;
        bi += output_width;
    }
}

/// C's `OBMC_SUBPIX_VAR(W, H)` wrapper (variance.c:259) — i.e.
/// `svt_aom_obmc_sub_pixel_varianceWxH_c`. Returns `(return_value, sse)`.
#[allow(clippy::too_many_arguments)]
pub fn obmc_sub_pixel_variance(
    pre: &[u8],
    pre_stride: usize,
    xoffset: usize,
    yoffset: usize,
    wsrc: &[i32],
    mask: &[i32],
    width: usize,
    height: usize,
) -> (u32, u32) {
    let mut fdata3 = alloc::vec![0u16; (height + 1) * width];
    let mut temp2 = alloc::vec![0u8; height * width];
    var_filter_block2d_bil_first_pass(
        pre,
        &mut fdata3,
        pre_stride,
        1,
        height + 1,
        width,
        &BILINEAR_FILTERS_2T[xoffset],
    );
    var_filter_block2d_bil_second_pass(
        &fdata3,
        &mut temp2,
        width,
        width,
        height,
        width,
        &BILINEAR_FILTERS_2T[yoffset],
    );
    obmc_variance_wxh(&temp2, width, wsrc, mask, width, height)
}

/// C `svt_aom_convolve_horiz` (convolve.c:252) specialised to the
/// `svt_aom_convolve8_horiz_c` call shape: one fixed sub-pel phase and
/// `x_step_q4 == 16`. `src_base` is the index in `src` of the block's (0, 0);
/// the kernel reads three columns to its left, exactly as C's `src -=
/// SUBPEL_TAPS / 2 - 1`.
#[allow(clippy::too_many_arguments)]
pub fn convolve8_horiz(
    src: &[u8],
    src_base: i64,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    kernel: &InterpKernel,
    w: usize,
    h: usize,
) {
    let base = src_base - (SUBPEL_TAPS as i64 / 2 - 1);
    for y in 0..h {
        let row = (base + (y * src_stride) as i64) as usize;
        for x in 0..w {
            let mut sum = 0i32;
            for k in 0..SUBPEL_TAPS {
                sum += i32::from(src[row + x + k]) * i32::from(kernel[k]);
            }
            dst[y * dst_stride + x] = clip_pixel(round_power_of_two(sum, FILTER_BITS));
        }
    }
}

/// C `svt_aom_convolve_vert` (convolve.c:269) specialised the same way.
#[allow(clippy::too_many_arguments)]
pub fn convolve8_vert(
    src: &[u8],
    src_base: i64,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    kernel: &InterpKernel,
    w: usize,
    h: usize,
) {
    let base = src_base - src_stride as i64 * (SUBPEL_TAPS as i64 / 2 - 1);
    for x in 0..w {
        for y in 0..h {
            let mut sum = 0i32;
            for k in 0..SUBPEL_TAPS {
                let idx = (base + ((y + k) * src_stride) as i64) as usize + x;
                sum += i32::from(src[idx]) * i32::from(kernel[k]);
            }
            dst[y * dst_stride + x] = clip_pixel(round_power_of_two(sum, FILTER_BITS));
        }
    }
}

/// C `MAX_SB_SIZE`.
const MAX_SB_SIZE: usize = 128;

/// C `svt_aom_upsampled_pred_c` (C_DEFAULT/variance.c:88), 8-bit arm.
/// `comp_pred` is written tightly at `width` per row.
#[allow(clippy::too_many_arguments)]
pub fn upsampled_pred(
    comp_pred: &mut [u8],
    width: usize,
    height: usize,
    subpel_x_q3: i32,
    subpel_y_q3: i32,
    reference: &[u8],
    ref_base: i64,
    ref_stride: usize,
    subpel_search: i32,
) {
    let filters = av1_get_filter(subpel_search);
    if subpel_x_q3 == 0 && subpel_y_q3 == 0 {
        for y in 0..height {
            let src = (ref_base + (y * ref_stride) as i64) as usize;
            comp_pred[y * width..y * width + width].copy_from_slice(&reference[src..src + width]);
        }
    } else if subpel_y_q3 == 0 {
        let kernel = &filters[(subpel_x_q3 << 1) as usize];
        convolve8_horiz(
            reference, ref_base, ref_stride, comp_pred, width, kernel, width, height,
        );
    } else if subpel_x_q3 == 0 {
        let kernel = &filters[(subpel_y_q3 << 1) as usize];
        convolve8_vert(
            reference, ref_base, ref_stride, comp_pred, width, kernel, width, height,
        );
    } else {
        let kernel_x = &filters[(subpel_x_q3 << 1) as usize];
        let kernel_y = &filters[(subpel_y_q3 << 1) as usize];
        let intermediate_height =
            ((((height as i32 - 1) * 8 + subpel_y_q3) >> 3) + SUBPEL_TAPS as i32) as usize;
        let mut temp = alloc::vec![0u8; MAX_SB_SIZE * (2 * MAX_SB_SIZE + 32)];
        convolve8_horiz(
            reference,
            ref_base - (ref_stride as i64) * (SUBPEL_TAPS as i64 / 2 - 1),
            ref_stride,
            &mut temp,
            MAX_SB_SIZE,
            kernel_x,
            width,
            intermediate_height,
        );
        let mid = MAX_SB_SIZE * (SUBPEL_TAPS / 2 - 1);
        convolve8_vert(
            &temp,
            mid as i64,
            MAX_SB_SIZE,
            comp_pred,
            width,
            kernel_y,
            width,
            height,
        );
    }
}

/// The state `svt_av1_obmc_full_pixel_search` / the sub-pixel tree read out of
/// `IntraBcContext` + `ModeDecisionContext`, gathered into one borrow.
pub struct ObmcSearch<'a> {
    /// C `x->xdplane[0].pre[is_second].buf`'s backing allocation.
    pub pre: &'a [u8],
    /// Index of the reference block's (0, 0) inside `pre`.
    pub pre_base: i64,
    /// C `x->xdplane[0].pre[is_second].stride`.
    pub pre_stride: usize,
    /// C `ctx->wsrc_buf`, tightly packed at `w` per row.
    pub wsrc: &'a [i32],
    /// C `ctx->mask_buf`, tightly packed at `w` per row.
    pub mask: &'a [i32],
    /// C `block_size_wide[ctx->blk_geom->bsize]`.
    pub w: usize,
    /// C `block_size_high[ctx->blk_geom->bsize]`.
    pub h: usize,
    /// C `x->mv_limits` (full-pel).
    pub mv_limits: FullMvLimits,
    /// C `x->approx_inter_rate` / `ctx->approx_inter_rate`.
    pub approx_inter_rate: bool,
    /// C `x->nmv_vec_cost` + `x->mv_cost_stack`.
    pub mv_cost: &'a MvCostTables,
    /// C `x->errorperbit`.
    pub errorperbit: i32,
}

impl ObmcSearch<'_> {
    /// C `get_buf_from_mv` (av1me.c:93) for the reference plane: a FULL-PEL
    /// offset from the block origin.
    #[inline]
    fn buf_at(&self, mv: Mv) -> i64 {
        self.pre_base + i64::from(mv.y) * self.pre_stride as i64 + i64::from(mv.x)
    }

    #[inline]
    fn osdf(&self, mv: Mv) -> u32 {
        let base = self.buf_at(mv) as usize;
        obmc_sad(
            &self.pre[base..],
            self.pre_stride,
            self.wsrc,
            self.mask,
            self.w,
            self.h,
        )
    }

    #[inline]
    fn ovf_at(&self, base: i64) -> (u32, u32) {
        obmc_variance_wxh(
            &self.pre[base as usize..],
            self.pre_stride,
            self.wsrc,
            self.mask,
            self.w,
            self.h,
        )
    }
}

/// C `is_mv_in` (av1me.c:190) — full-pel bounds test.
#[inline]
fn is_mv_in(limits: FullMvLimits, mv: Mv) -> bool {
    i32::from(mv.x) >= limits.col_min
        && i32::from(mv.x) <= limits.col_max
        && i32::from(mv.y) >= limits.row_min
        && i32::from(mv.y) <= limits.row_max
}

/// C `clamp_mv` (mv.h) in the full-pel domain the OBMC search uses.
#[inline]
fn clamp_mv(mv: &mut Mv, limits: FullMvLimits) {
    mv.x = i32::from(mv.x).clamp(limits.col_min, limits.col_max) as i16;
    mv.y = i32::from(mv.y).clamp(limits.row_min, limits.row_max) as i16;
}

/// C `get_obmc_mvpred_var` (av1me.c:621). `best_mv` is FULL-PEL, `center_mv`
/// eighth-pel; C converts `best_mv` with `* 8` before costing it.
pub fn get_obmc_mvpred_var(s: &ObmcSearch, best_mv: Mv, center_mv: Mv, use_mvcost: bool) -> i32 {
    let mv = Mv {
        x: best_mv.x.wrapping_mul(8),
        y: best_mv.y.wrapping_mul(8),
    };
    let (var, _sse) = s.ovf_at(s.buf_at(best_mv));
    let cost = if !use_mvcost {
        0
    } else if s.approx_inter_rate {
        mv_err_cost_light(mv, center_mv)
    } else {
        mv_err_cost(mv, center_mv, s.mv_cost, s.errorperbit)
    };
    (var as i32).wrapping_add(cost)
}

/// C's eight-neighbour order inside `obmc_refining_search_sad` (av1me.c:637):
/// the four axial steps first, then the four diagonals.
const OBMC_NEIGHBORS: [(i16, i16); 8] = [
    (0, -1),
    (-1, 0),
    (1, 0),
    (0, 1),
    (1, -1),
    (1, 1),
    (-1, 1),
    (-1, -1),
];

/// C `obmc_refining_search_sad` (av1me.c:635). `ref_mv` is FULL-PEL in/out;
/// `center_mv` is EIGHTH-pel and is shifted down by 3 internally, as C does.
///
/// Note C's double test: a neighbour's raw SAD must beat `best_sad` BEFORE the
/// MV cost is added, and then the sum must beat it again. A candidate whose
/// raw SAD ties the incumbent is rejected without ever being costed.
pub fn obmc_refining_search_sad(
    s: &ObmcSearch,
    ref_mv: &mut Mv,
    error_per_bit: i32,
    search_range: i32,
    center_mv: Mv,
    search_diag: bool,
) -> u32 {
    let fcenter_x = i32::from(center_mv.x) >> 3;
    let fcenter_y = i32::from(center_mv.y) >> 3;
    let mut best_sad = s.osdf(*ref_mv).wrapping_add(mvsad_err_cost(
        i32::from(ref_mv.x),
        i32::from(ref_mv.y),
        fcenter_x,
        fcenter_y,
        error_per_bit,
        s.approx_inter_rate,
        s.mv_cost,
    ) as u32);

    let n = if search_diag { 8 } else { 4 };
    for _ in 0..search_range {
        let mut best_site: i32 = -1;
        for (j, &(dx, dy)) in OBMC_NEIGHBORS.iter().enumerate().take(n) {
            let mv = Mv {
                x: ref_mv.x + dx,
                y: ref_mv.y + dy,
            };
            if is_mv_in(s.mv_limits, mv) {
                let mut sad = s.osdf(mv);
                if sad < best_sad {
                    sad = sad.wrapping_add(mvsad_err_cost(
                        i32::from(mv.x),
                        i32::from(mv.y),
                        fcenter_x,
                        fcenter_y,
                        error_per_bit,
                        s.approx_inter_rate,
                        s.mv_cost,
                    ) as u32);
                    if sad < best_sad {
                        best_sad = sad;
                        best_site = j as i32;
                    }
                }
            }
        }
        if best_site == -1 {
            break;
        }
        let (dx, dy) = OBMC_NEIGHBORS[best_site as usize];
        ref_mv.x += dx;
        ref_mv.y += dy;
    }
    best_sad
}

/// C `svt_av1_obmc_full_pixel_search` (av1me.c:673) — EXPORTED. Returns the
/// variance-domain cost of the MV it writes into `dst_mv` (full-pel).
///
/// C calls `clamp_mv` TWICE in a row; the second call is a no-op and is kept.
pub fn obmc_full_pixel_search(
    s: &ObmcSearch,
    mvp_full: Mv,
    sadpb: i32,
    ref_mv: Mv,
    dst_mv: &mut Mv,
    fpel_search_range: i32,
    fpel_search_diag: bool,
) -> i32 {
    *dst_mv = mvp_full;
    clamp_mv(dst_mv, s.mv_limits);
    clamp_mv(dst_mv, s.mv_limits);
    let thissme = obmc_refining_search_sad(
        s,
        dst_mv,
        sadpb,
        fpel_search_range,
        ref_mv,
        fpel_search_diag,
    );
    if thissme < u32::MAX {
        return get_obmc_mvpred_var(s, *dst_mv, ref_mv, true);
    }
    thissme as i32
}

/// C `set_subpel_mv_search_range` (av1me.c:694). Returns
/// `(col_min, col_max, row_min, row_max)` in the EIGHTH-pel domain.
pub fn set_subpel_mv_search_range(mv_limits: FullMvLimits, ref_mv: Mv) -> (i32, i32, i32, i32) {
    let max_mv = MAX_FULL_PEL_VAL * 8;
    let minc = i32::max(mv_limits.col_min * 8, i32::from(ref_mv.x) - max_mv);
    let maxc = i32::min(mv_limits.col_max * 8, i32::from(ref_mv.x) + max_mv);
    let minr = i32::max(mv_limits.row_min * 8, i32::from(ref_mv.y) - max_mv);
    let maxr = i32::min(mv_limits.row_max * 8, i32::from(ref_mv.y) + max_mv);
    (
        i32::max(MV_LOW + 1, minc),
        i32::min(MV_UPP - 1, maxc),
        i32::max(MV_LOW + 1, minr),
        i32::min(MV_UPP - 1, maxr),
    )
}

/// C `search_step_table` (av1me.c:709): three rounds of {left, right, up,
/// down} at 1/2, 1/4 and 1/8 pel.
const SEARCH_STEP_TABLE: [(i32, i32); 12] = [
    (-4, 0),
    (4, 0),
    (0, -4),
    (0, 4),
    (-2, 0),
    (2, 0),
    (0, -2),
    (0, 2),
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
];

/// C `sp` (av1me.c:869): eighth-pel component to sub-pixel phase.
#[inline]
fn sp(x: i32) -> i32 {
    x & 7
}

/// C `pre` (av1me.c:873): eighth-pel (r, c) to an integer-pel offset.
#[inline]
fn pre_off(stride: usize, r: i32, c: i32) -> i64 {
    i64::from(r >> 3) * stride as i64 + i64::from(c >> 3)
}

/// C `setup_obmc_center_error` (av1me.c:723). Returns `(besterr, sse,
/// distortion)`.
pub fn setup_obmc_center_error(
    s: &ObmcSearch,
    bestmv: Mv,
    ref_mv: Mv,
    error_per_bit: i32,
    offset: i64,
) -> (u32, u32, i32) {
    let (mut besterr, sse) = s.ovf_at(s.pre_base + offset);
    let distortion = besterr as i32;
    let cost = if s.approx_inter_rate {
        mv_err_cost_light(bestmv, ref_mv)
    } else {
        mv_err_cost(bestmv, ref_mv, s.mv_cost, error_per_bit)
    };
    besterr = besterr.wrapping_add(cost as u32);
    (besterr, sse, distortion)
}

/// C `upsampled_obmc_pref_error` (av1me.c:811), 8-bit arm. Returns
/// `(besterr, sse)`.
#[allow(clippy::too_many_arguments)]
pub fn upsampled_obmc_pref_error(
    s: &ObmcSearch,
    y_base: i64,
    subpel_x_q3: i32,
    subpel_y_q3: i32,
    w: usize,
    h: usize,
    subpel_search: i32,
) -> (u32, u32) {
    let mut pred = alloc::vec![0u8; w * h];
    upsampled_pred(
        &mut pred,
        w,
        h,
        subpel_x_q3,
        subpel_y_q3,
        s.pre,
        y_base,
        s.pre_stride,
        subpel_search,
    );
    obmc_variance_wxh(&pred, w, s.wsrc, s.mask, w, h)
}

/// C `upsampled_setup_obmc_center_error` (av1me.c:850). Returns
/// `(besterr, sse, distortion)`.
#[allow(clippy::too_many_arguments)]
pub fn upsampled_setup_obmc_center_error(
    s: &ObmcSearch,
    bestmv: Mv,
    ref_mv: Mv,
    error_per_bit: i32,
    offset: i64,
    subpel_search: i32,
) -> (u32, u32, i32) {
    let (mut besterr, sse) =
        upsampled_obmc_pref_error(s, s.pre_base + offset, 0, 0, s.w, s.h, subpel_search);
    let distortion = besterr as i32;
    let cost = if s.approx_inter_rate {
        mv_err_cost_light(bestmv, ref_mv)
    } else {
        mv_err_cost(bestmv, ref_mv, s.mv_cost, error_per_bit)
    };
    besterr = besterr.wrapping_add(cost as u32);
    (besterr, sse, distortion)
}

/// The three outputs C returns through pointers from the sub-pixel tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObmcSubpelResult {
    /// C's return value.
    pub besterr: u32,
    /// C `*distortion`.
    pub distortion: i32,
    /// C `*sse1`.
    pub sse: u32,
}

/// C `svt_av1_find_best_obmc_sub_pixel_tree_up` (av1me.c:878) — EXPORTED.
///
/// `bestmv` enters FULL-PEL and leaves EIGHTH-PEL (C multiplies it by 8 on the
/// way in). `use_accurate_subpel_search` selects the upsampled predictor; the
/// only C call site passes `USE_8_TAPS`, so 0 is the dead branch.
///
/// C's `SECOND_LEVEL_CHECKS_BEST` runs only when `iters_per_step > 1` and the
/// round improved something; its `kc`/`kr` are the *diagonal* deltas computed
/// just above, adjusted when the winner moved along only one axis.
#[allow(clippy::too_many_arguments)]
pub fn find_best_obmc_sub_pixel_tree_up(
    s: &ObmcSearch,
    bestmv: &mut Mv,
    ref_mv: Mv,
    allow_hp: bool,
    error_per_bit: i32,
    forced_stop: i32,
    iters_per_step: i32,
    use_accurate_subpel_search: i32,
) -> ObmcSubpelResult {
    let (minc, maxc, minr, maxr) = set_subpel_mv_search_range(s.mv_limits, ref_mv);

    let mut br = i32::from(bestmv.y) * 8;
    let mut bc = i32::from(bestmv.x) * 8;
    let mut hstep = 4i32;
    let mut round = 3 - forced_stop;
    if !allow_hp && round == 3 {
        round = 2;
    }
    let offset = i64::from(bestmv.y) * s.pre_stride as i64 + i64::from(bestmv.x);
    bestmv.y = bestmv.y.wrapping_mul(8);
    bestmv.x = bestmv.x.wrapping_mul(8);

    let lp = s.approx_inter_rate;
    let (w, h) = (s.w, s.h);

    let (mut besterr, mut sse1, mut distortion) = if use_accurate_subpel_search != 0 {
        upsampled_setup_obmc_center_error(
            s,
            *bestmv,
            ref_mv,
            error_per_bit,
            offset,
            use_accurate_subpel_search,
        )
    } else {
        setup_obmc_center_error(s, *bestmv, ref_mv, error_per_bit, offset)
    };

    // The per-candidate distortion for a given eighth-pel (r, c).
    let dist = |r: i32, c: i32| -> (u32, u32) {
        let base = s.pre_base + pre_off(s.pre_stride, r, c);
        if use_accurate_subpel_search != 0 {
            upsampled_obmc_pref_error(s, base, sp(c), sp(r), w, h, use_accurate_subpel_search)
        } else {
            obmc_sub_pixel_variance(
                &s.pre[base as usize..],
                s.pre_stride,
                sp(c) as usize,
                sp(r) as usize,
                s.wsrc,
                s.mask,
                w,
                h,
            )
        }
    };
    let mvcost = |r: i32, c: i32| -> i32 {
        let this_mv = Mv {
            x: c as i16,
            y: r as i16,
        };
        if lp {
            mv_err_cost_light(this_mv, ref_mv)
        } else {
            mv_err_cost(this_mv, ref_mv, s.mv_cost, error_per_bit)
        }
    };

    let mut step = 0usize;
    let mut best_idx: i32 = -1;
    let mut tr;
    let mut tc;
    for _iter in 0..round {
        let mut cost_array = [u32::MAX; 5];
        for idx in 0..4usize {
            tr = br + SEARCH_STEP_TABLE[step + idx].1;
            tc = bc + SEARCH_STEP_TABLE[step + idx].0;
            if tc >= minc && tc <= maxc && tr >= minr && tr <= maxr {
                let (thismse, sse) = dist(tr, tc);
                cost_array[idx] = thismse.wrapping_add(mvcost(tr, tc) as u32);
                if cost_array[idx] < besterr {
                    best_idx = idx as i32;
                    besterr = cost_array[idx];
                    distortion = thismse as i32;
                    sse1 = sse;
                }
            } else {
                cost_array[idx] = u32::MAX;
            }
        }

        let kc = if cost_array[0] <= cost_array[1] {
            -hstep
        } else {
            hstep
        };
        let kr = if cost_array[2] <= cost_array[3] {
            -hstep
        } else {
            hstep
        };
        tc = bc + kc;
        tr = br + kr;
        if tc >= minc && tc <= maxc && tr >= minr && tr <= maxr {
            let (thismse, sse) = dist(tr, tc);
            cost_array[4] = thismse.wrapping_add(mvcost(tr, tc) as u32);
            if cost_array[4] < besterr {
                best_idx = 4;
                besterr = cost_array[4];
                distortion = thismse as i32;
                sse1 = sse;
            }
        } else {
            // C writes `cost_array[idx]` here, where `idx` has run off the
            // preceding loop to 4 — the same slot the diagonal candidate
            // would have used. Reproduced (and never read again: the next
            // round re-seeds the whole array).
            cost_array[4] = u32::MAX;
            let _ = cost_array[4];
        }

        if (0..4).contains(&best_idx) {
            br += SEARCH_STEP_TABLE[step + best_idx as usize].1;
            bc += SEARCH_STEP_TABLE[step + best_idx as usize].0;
        } else if best_idx == 4 {
            br = tr;
            bc = tc;
        }

        if iters_per_step > 1 && best_idx != -1 {
            // C `SECOND_LEVEL_CHECKS_BEST`.
            let br0 = br;
            let bc0 = bc;
            let mut kc2 = kc;
            let mut kr2 = kr;
            if tr == br && tc != bc {
                kc2 = bc - tc;
            } else if tr != br && tc == bc {
                kr2 = br - tr;
            }
            let check = |r: i32,
                         c: i32,
                         besterr: &mut u32,
                         distortion: &mut i32,
                         sse1: &mut u32,
                         br: &mut i32,
                         bc: &mut i32| {
                if c >= minc && c <= maxc && r >= minr && r <= maxr {
                    let (thismse, sse) = dist(r, c);
                    let v = mvcost(r, c) as u32;
                    if v.wrapping_add(thismse) < *besterr {
                        *besterr = v.wrapping_add(thismse);
                        *br = r;
                        *bc = c;
                        *distortion = thismse as i32;
                        *sse1 = sse;
                    }
                }
            };
            check(
                br0 + kr2,
                bc0,
                &mut besterr,
                &mut distortion,
                &mut sse1,
                &mut br,
                &mut bc,
            );
            check(
                br0,
                bc0 + kc2,
                &mut besterr,
                &mut distortion,
                &mut sse1,
                &mut br,
                &mut bc,
            );
            if br0 != br || bc0 != bc {
                check(
                    br0 + kr2,
                    bc0 + kc2,
                    &mut besterr,
                    &mut distortion,
                    &mut sse1,
                    &mut br,
                    &mut bc,
                );
            }
        }

        step += 4;
        hstep >>= 1;
        best_idx = -1;
    }

    bestmv.y = br as i16;
    bestmv.x = bc as i16;
    ObmcSubpelResult {
        besterr,
        distortion,
        sse: sse1,
    }
}
