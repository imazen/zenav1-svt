//! Mode-decision sub-pixel motion search — the wholesale port of
//! `Source/Lib/Codec/mcomp.c`.
//!
//! This is the refinement that turns MD's full-pel ME winner into the
//! fractional MV the encoder actually codes. It is dispatched from
//! `product_coding_loop.c:2609` (`md_subpel_search`) and is a DIFFERENT search
//! from the open-loop ME in `motion_estimation.c` / `av1me.c`
//! (`crate::inter_me`): `PORTING.md:129` maps mcomp.c onto `motion_est`, but
//! `motion_est.rs` says in its own header that it is homegrown, so it is not a
//! port of this file and this module does not touch it.
//!
//! ## Coverage — 17 of 17 functions in mcomp.c
//!
//! | C function | line | here |
//! |---|---|---|
//! | `svt_mv_err_cost` | 42 | [`mv_err_cost`] |
//! | `svt_mv_err_cost_` | 74 | [`MvCostParams::err_cost`] |
//! | `svt_get_subpel_part` | 99 | [`get_subpel_part`] |
//! | `svt_get_buf_from_mv` | 106 | [`get_buf_from_mv`] |
//! | `svt_upsampled_pref_error` | 112 | [`SubpelSearchVarParams::upsampled_pref_error`] |
//! | `svt_estimated_pref_error` | 156 | [`SubpelSearchVarParams::estimated_pref_error`] |
//! | `svt_check_better_fast` | 176 | [`check_better_fast`] |
//! | `svt_check_better` | 219 | [`check_better`] |
//! | `get_best_diag_step` | 248 | [`get_best_diag_step`] |
//! | `svt_first_level_check` | 256 | [`first_level_check`] |
//! | `svt_second_level_check_v2` | 289 | [`second_level_check_v2`] |
//! | `svt_upsampled_setup_center_error` | 351 | [`upsampled_setup_center_error`] |
//! | `first_level_check_fast` | 364 | [`first_level_check_fast`] |
//! | `second_level_check_fast` | 422 | [`second_level_check_fast`] |
//! | `two_level_checks_fast` | 559 | [`two_level_checks_fast`] |
//! | `svt_av1_find_best_sub_pixel_tree_pruned` | 599 | [`find_best_sub_pixel_tree_pruned`] |
//! | `svt_av1_find_best_sub_pixel_tree` | 683 | [`find_best_sub_pixel_tree`] |
//! | `svt_aom_fp_mv_err_cost` | 775 | [`fp_mv_err_cost`] |
//!
//! ## Evidence
//!
//! Only three of those are linkable symbols (`nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` prints `T` for the two entry points and
//! `svt_aom_fp_mv_err_cost`; the other fourteen print nothing — they are
//! `static`). The fourteen are reachable ONLY through the entry points, so
//! `tests/c_parity_md_subpel.rs` drives the two entry points through a shim
//! that builds `SUBPEL_MOTION_SEARCH_PARAMS` + `MacroBlockD` +
//! `ModeDecisionContext` from plain scalars. That is **evidence tier 1**
//! (`docs/WORKING-ON-THIS.md` §4) over the whole tree at once — strictly
//! stronger than fourteen hand-derived vector tests, because a hand-derived
//! vector is a second transcription of the same logic.
//!
//! ## Integer-width notes, all binding for byte identity
//!
//! * `Mv` is a union of two `int16_t` in C (`mv.h:41`). Every `{{x ± hstep, y}}`
//!   initialiser therefore TRUNCATES to `i16`, and so does the `diff` inside
//!   `svt_mv_err_cost`. The port keeps `i16` at exactly those points.
//! * `svt_get_buf_from_mv` uses `mv.y >> 3` — an ARITHMETIC shift, which floors
//!   toward -inf. A truncating division would be wrong on half the MV plane.
//! * `cost` in `svt_check_better{,_fast}` is `unsigned int` and `cost +=
//!   thismse` mixes it with an `int`; the port uses `u32` with wrapping
//!   arithmetic so the C conversion is explicit rather than accidental.
//! * `int64_t bestcost = *distortion + cost` adds `int` to `unsigned int`, so C
//!   evaluates the sum in `unsigned int` FIRST and only then widens. The port
//!   spells that out.

use crate::intrabc::{MvCostTables, mv_table_cost};
use svtav1_dsp::subpel_variance::{sub_pixel_variance, variance_diff_sse};
use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;

// =============================================================================
// §0. Constants
// =============================================================================

/// C `INIT_SUBPEL_STEP_SIZE` (mcomp.c:86): 4/8 = 1/2 pel.
const INIT_SUBPEL_STEP_SIZE: i32 = 4;
/// C `PIXEL_TRANSFORM_ERROR_SCALE` (mcomp.c:39).
const PIXEL_TRANSFORM_ERROR_SCALE: u32 = 4;
/// C `SSE_LAMBDA_LOWRES` (mcomp.c:31).
const SSE_LAMBDA_LOWRES: i32 = 2;
/// C `SSE_LAMBDA_MIDRES` (mcomp.c:33).
const SSE_LAMBDA_MIDRES: i32 = 0;
/// C `SSE_LAMBDA_HDRES` (mcomp.c:35).
const SSE_LAMBDA_HDRES: i32 = 1;
/// C `RDDIV_BITS` (rd_cost.h:34).
const RDDIV_BITS: u32 = 7;
/// C `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:29).
const AV1_PROB_COST_SHIFT: u32 = 9;
/// C `RD_EPB_SHIFT` (restoration.h:342).
const RD_EPB_SHIFT: u32 = 6;

/// C `SUBPEL_FORCE_STOP` (definitions.h:868): `EIGHTH_PEL, QUARTER_PEL,
/// HALF_PEL, FULL_PEL`.
pub const EIGHTH_PEL: i32 = 0;
/// See [`EIGHTH_PEL`].
pub const QUARTER_PEL: i32 = 1;
/// See [`EIGHTH_PEL`].
pub const HALF_PEL: i32 = 2;
/// See [`EIGHTH_PEL`].
pub const FULL_PEL: i32 = 3;

/// C `SUBPEL_STAGE` (definitions.h:850): `SPEL_ME, SPEL_PME`.
pub const SPEL_ME: i32 = 0;
/// See [`SPEL_ME`].
pub const SPEL_PME: i32 = 1;

/// C `PD_PASS_1` (the `PdPass` enum) — the only pass whose `mvp_th` arm is
/// live in [`find_best_sub_pixel_tree`].
pub const PD_PASS_1: i32 = 1;

/// C `eb_num_pels_log2_lookup[BLOCK_SIZES_ALL]` (common_utils.c:39-40), in
/// [`BlockSize`] order (which is C's `BlockSize` order).
pub const NUM_PELS_LOG2_LOOKUP: [u8; 22] = [
    4, 5, 5, 6, 7, 7, 8, 9, 9, 10, 11, 11, 12, 13, 13, 14, 6, 6, 8, 8, 10, 10,
];

/// C `svt_aom_eb_av1_var_offs[MAX_SB_SIZE]` (pic_analysis_process.c:937): 128
/// copies of 128. The `pred_variance_th` probe passes it with `b_stride == 0`,
/// so every row of the block reads the same 128 bytes.
pub const EB_AV1_VAR_OFFS: [u8; 128] = [128; 128];

/// C `ROUND_POWER_OF_TWO(value, n)` (definitions.h:478).
#[inline]
fn round_power_of_two(value: u32, n: u32) -> u32 {
    (value + ((1u32 << n) >> 1)) >> n
}

/// C `ROUND_POWER_OF_TWO_64(value, n)` (definitions.h:485).
#[inline]
fn round_power_of_two_64(value: i64, n: u32) -> i64 {
    (value + ((1i64 << n) >> 1)) >> n
}

/// C `svt_av1_is_subpelmv_in_range` (mcomp.h:127-130).
#[inline]
pub fn is_subpelmv_in_range(limits: &SubpelMvLimits, mv: Mv) -> bool {
    i32::from(mv.x) >= limits.col_min
        && i32::from(mv.x) <= limits.col_max
        && i32::from(mv.y) >= limits.row_min
        && i32::from(mv.y) <= limits.row_max
}

/// C `SubpelMvLimits` (mv.h:33-38).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubpelMvLimits {
    pub col_min: i32,
    pub col_max: i32,
    pub row_min: i32,
    pub row_max: i32,
}

/// C `svt_av1_set_subpel_mv_search_range` (mcomp.h:112-125). `full` is the
/// FULL-PEL limit set; `ref_mv` is eighth-pel.
pub fn set_subpel_mv_search_range(full: (i32, i32, i32, i32), ref_mv: Mv) -> SubpelMvLimits {
    /// C `MAX_FULL_PEL_VAL` (av1me.h:27) = `(1 << 10) - 1`.
    const MAX_FULL_PEL_VAL: i32 = (1 << 10) - 1;
    /// C `MV_UPP` / `MV_LOW` (cabac_context_model.h:198-199).
    const MV_UPP: i32 = 1 << 14;
    const MV_LOW: i32 = -(1 << 14);
    let (col_min, col_max, row_min, row_max) = full;
    let max_mv = MAX_FULL_PEL_VAL * 8;
    let minc = (col_min * 8).max(i32::from(ref_mv.x) - max_mv);
    let maxc = (col_max * 8).min(i32::from(ref_mv.x) + max_mv);
    let minr = (row_min * 8).max(i32::from(ref_mv.y) - max_mv);
    let maxr = (row_max * 8).min(i32::from(ref_mv.y) + max_mv);
    SubpelMvLimits {
        col_min: (MV_LOW + 1).max(minc),
        col_max: (MV_UPP - 1).min(maxc),
        row_min: (MV_LOW + 1).max(minr),
        row_max: (MV_UPP - 1).min(maxr),
    }
}

// =============================================================================
// §1. MV cost — the 5-way MV_COST_TYPE dispatch
// =============================================================================

/// C `MV_COST_TYPE` (mcomp.h:29-35).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MvCostType {
    /// Entropy rate of the MV.
    Entropy = 0,
    /// L1 norm, < 480p.
    L1LowRes = 1,
    /// L1 norm, >= 480p.
    L1MidRes = 2,
    /// L1 norm, >= 720p.
    L1HdRes = 3,
    /// L1 norm scaled by `error_per_bit`, with an early exit in
    /// [`check_better_fast`].
    Opt = 4,
    /// Always zero.
    None = 5,
}

/// C `svt_mv_cost_param` (mcomp.h:37-48). `full_ref_mv` and `sad_per_bit` are
/// members of the C struct that no function in mcomp.c reads, so they are not
/// carried here.
#[derive(Clone, Copy, Debug)]
pub struct MvCostParams<'a> {
    /// C `ref_mv` — eighth-pel.
    pub ref_mv: Mv,
    pub mv_cost_type: MvCostType,
    /// C `mvjcost` + `mvcost[2]` as one bundle. `None` maps onto C's null
    /// `mvcost[i]`, which is unreachable on the `MV_COST_ENTROPY` arm — see
    /// [`mv_err_cost`]'s PORT-NOTE — and simply unused on the other four.
    pub tables: Option<&'a MvCostTables>,
    pub error_per_bit: i32,
    pub early_exit_th: i32,
}

impl MvCostParams<'_> {
    /// C `svt_mv_err_cost_` (mcomp.c:74-81).
    #[inline]
    pub fn err_cost(&self, mv: Mv) -> i32 {
        mv_err_cost(
            mv,
            self.ref_mv,
            self.tables,
            self.error_per_bit,
            self.mv_cost_type,
        )
    }
}

/// C `svt_mv_err_cost` (mcomp.c:42-72) — the full six-way dispatch, and
/// **the one body in this crate**. `port_md::pme::{mv_err_cost,
/// fp_mv_err_cost}`, [`fp_mv_err_cost`] and `intrabc::mv_err_cost` (C's
/// av1me.c `svt_aom_mv_err_cost`, which is this function's ENTROPY arm
/// under an older name) are all forwards here — `docs/WORKING-ON-THIS.md`
/// §4, folded 2026-09-04 from four transcriptions.
///
/// `diff` and `abs_diff` are C `Mv`s, i.e. pairs of `int16_t`, so both the
/// difference and its absolute value truncate to 16 bits before use. That is
/// reproduced here: `i16::wrapping_sub` for the difference and
/// `wrapping_abs` for the magnitude (C's `abs()` on `INT16_MIN` promoted to
/// `int` gives 32768, which then truncates back to `-32768` on the store into
/// the `int16_t` field — `wrapping_abs` is the same value).
///
/// PORT-NOTE, MEASURED 2026-08-31: C's `MV_COST_ENTROPY` arm reads
/// `if (mvcost) { ... } return 0;`, which LOOKS like a null-table guard. It is
/// not. The parameter is declared `const int* const mvcost[2]`, which as a
/// function parameter is adjusted to `const int* const*`, so `if (mvcost)`
/// tests the ADDRESS OF THE ARRAY. Reached through `svt_mv_err_cost_` that
/// address is `&mv_cost_params->mvcost[0]` — a member of a live struct, never
/// null — so the guard is ALWAYS TAKEN and a null `mvcost[0]` segfaults inside
/// `svt_mv_cost` instead of returning 0. Verified by calling the real
/// `svt_aom_fp_mv_err_cost` with null element pointers: it crashes (SIGSEGV),
/// it does not return 0. The `None => 0` arm below is therefore a faithful
/// transcription of a branch that is UNREACHABLE through every mcomp.c call
/// site, and `tests/c_parity_md_subpel.rs` consequently drives `None` only
/// with the four cost types that never dereference the tables.
pub fn mv_err_cost(
    mv: Mv,
    ref_mv: Mv,
    tables: Option<&MvCostTables>,
    error_per_bit: i32,
    mv_cost_type: MvCostType,
) -> i32 {
    let diff_x = mv.x.wrapping_sub(ref_mv.x);
    let diff_y = mv.y.wrapping_sub(ref_mv.y);
    let abs_x = i32::from(diff_x.wrapping_abs());
    let abs_y = i32::from(diff_y.wrapping_abs());
    match mv_cost_type {
        MvCostType::Entropy => match tables {
            Some(t) => {
                let c = i64::from(mv_table_cost(i32::from(diff_x), i32::from(diff_y), t))
                    * i64::from(error_per_bit);
                round_power_of_two_64(
                    c,
                    RDDIV_BITS + AV1_PROB_COST_SHIFT - RD_EPB_SHIFT + PIXEL_TRANSFORM_ERROR_SCALE,
                ) as i32
            }
            None => 0,
        },
        MvCostType::L1LowRes => (SSE_LAMBDA_LOWRES * (abs_y + abs_x)) >> 3,
        MvCostType::L1MidRes => (SSE_LAMBDA_MIDRES * (abs_y + abs_x)) >> 3,
        MvCostType::L1HdRes => (SSE_LAMBDA_HDRES * (abs_y + abs_x)) >> 3,
        MvCostType::Opt => round_power_of_two_64(
            i64::from((abs_y + abs_x) << 8) * i64::from(error_per_bit),
            RDDIV_BITS + AV1_PROB_COST_SHIFT - RD_EPB_SHIFT + PIXEL_TRANSFORM_ERROR_SCALE,
        ) as i32,
        MvCostType::None => 0,
    }
}

/// C `svt_aom_fp_mv_err_cost` (mcomp.c:775-777, EXPORTED) — the full-pel MD ME
/// rate term added at `product_coding_loop.c:1816/1890/2040/2920`.
///
/// It is exactly `svt_mv_err_cost_`; C keeps the wrapper because it is the one
/// spelling with external linkage.
#[inline]
pub fn fp_mv_err_cost(mv: Mv, params: &MvCostParams<'_>) -> i32 {
    params.err_cost(mv)
}

// =============================================================================
// §2. Buffer addressing and the two error metrics
// =============================================================================

/// C `svt_get_subpel_part` (mcomp.c:99-101): the q3 sub-pel phase.
#[inline]
pub fn get_subpel_part(x: i32) -> i32 {
    x & 7
}

/// C `svt_get_buf_from_mv` (mcomp.c:106-109).
///
/// `(mv.y >> 3) * stride + (mv.x >> 3)` with an ARITHMETIC shift, which floors
/// toward -inf. `-1 >> 3 == -1`, not `0` — a `/ 8` port would be wrong for
/// every negative MV component.
#[inline]
pub fn get_buf_from_mv(base: i64, stride: usize, mv: Mv) -> i64 {
    base + i64::from(i32::from(mv.y) >> 3) * stride as i64 + i64::from(i32::from(mv.x) >> 3)
}

/// C `SUBPEL_SEARCH_VAR_PARAMS` (mcomp.h:70-80) plus the `MSBuffers`
/// (mcomp.h:61-67) it owns.
///
/// C selects a size-specialised `vfp->vf` / `vfp->svf` out of
/// `svt_aom_mefn_ptr[bsize]`; those are the `_c` kernels this port's
/// [`variance_diff_sse`] / [`sub_pixel_variance`] transcribe with `w`/`h` as
/// runtime arguments. `c_parity_subpel_variance.rs` drives all 22 macro
/// instantiations, so the parameterisation is checked, not assumed.
pub struct SubpelSearchVarParams<'a> {
    /// C `ms_buffers.src->buf` and its stride.
    pub src: &'a [u8],
    pub src_base: usize,
    pub src_stride: usize,
    /// C `ms_buffers.ref->buf`, as an allocation plus the index of the block's
    /// (0, 0) — so the negative offsets [`get_buf_from_mv`] produces stay
    /// inside the slice.
    pub ref_alloc: &'a [u8],
    pub ref_base: i64,
    pub ref_stride: usize,
    pub w: usize,
    pub h: usize,
    /// C `bias_fp`: a penalty applied to fractional candidates whenever the
    /// incumbent best MV is full-pel. 0 disables it.
    pub bias_fp: i32,
    /// C `subpel_search_type` (`USE_2_TAPS` / `USE_4_TAPS` / `USE_8_TAPS`),
    /// used only by [`Self::upsampled_pref_error`].
    pub subpel_search_type: i32,
}

impl SubpelSearchVarParams<'_> {
    /// C `svt_upsampled_pref_error` (mcomp.c:112-150) — the ACCURATE metric:
    /// build an 8-tap (or 4-/2-tap) upsampled prediction, then take its
    /// variance against the source.
    ///
    /// Reuses [`crate::inter_me::obmc_search::upsampled_pred`], the existing
    /// C-gated port of `svt_aom_upsampled_pred_c`.
    ///
    /// C passes `xd`, `cm`, `mi_row`, `mi_col` and `this_mv` down to
    /// `svt_aom_upsampled_pred`, which `(void)`-ignores all five
    /// (`C_DEFAULT/variance.c:88-93`); they are omitted here.
    pub fn upsampled_pref_error(&self, this_mv: Mv) -> (u32, u32) {
        let ref_off = get_buf_from_mv(self.ref_base, self.ref_stride, this_mv);
        let subpel_x_q3 = get_subpel_part(i32::from(this_mv.x));
        let subpel_y_q3 = get_subpel_part(i32::from(this_mv.y));
        let mut pred = alloc::vec![0u8; self.w * self.h];
        crate::inter_me::obmc_search::upsampled_pred(
            &mut pred,
            self.w,
            self.h,
            subpel_x_q3,
            subpel_y_q3,
            self.ref_alloc,
            ref_off,
            self.ref_stride,
            self.subpel_search_type,
        );
        // C: `besterr = vfp->vf(pred, w, src, src_stride, sse);`
        variance_diff_sse(
            &pred,
            0,
            self.w,
            self.src,
            self.src_base,
            self.src_stride,
            self.w,
            self.h,
        )
    }

    /// C `svt_estimated_pref_error` (mcomp.c:156-168) — the FAST metric, and
    /// the number the pruned tree actually minimises: `vfp->svf`, i.e.
    /// `svt_aom_sub_pixel_variance{W}x{H}_c` (bilinear).
    pub fn estimated_pref_error(&self, this_mv: Mv) -> (u32, u32) {
        let ref_off = get_buf_from_mv(self.ref_base, self.ref_stride, this_mv);
        let subpel_x_q3 = get_subpel_part(i32::from(this_mv.x));
        let subpel_y_q3 = get_subpel_part(i32::from(this_mv.y));
        sub_pixel_variance(
            self.ref_alloc,
            ref_off as usize,
            self.ref_stride,
            subpel_x_q3 as usize,
            subpel_y_q3 as usize,
            self.src,
            self.src_base,
            self.src_stride,
            self.w,
            self.h,
        )
    }
}

/// The four mutable cells `svt_check_better{,_fast}` write through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubpelState {
    /// C `*besterr`.
    pub besterr: u32,
    /// C `*best_mv`.
    pub best_mv: Mv,
    /// C `*distortion`.
    pub distortion: i32,
    /// C `*sse1`.
    pub sse1: u32,
}

// =============================================================================
// §3. The per-candidate accept/reject pair
// =============================================================================

/// C `svt_check_better_fast` (mcomp.c:176-217).
///
/// Returns C's `cost` (`INT_MAX` as `u32` when the candidate is out of range),
/// which the caller feeds to [`get_best_diag_step`].
///
/// Three details decide the winning MV and are easy to get wrong:
/// * the `MV_COST_OPT` early exit compares `distortion + cost` against
///   `besterr * early_exit_th / 1000` and RETURNS that sum without ever
///   evaluating the error metric;
/// * `weight` is `bias_fp` only when the INCUMBENT `best_mv` is full-pel on
///   BOTH axes (C's `% 8 == 0`, truncating toward zero, so it is also true for
///   negative multiples of 8);
/// * the comparison is `(cost * weight) / 100 < besterr`, but the value STORED
///   is the unweighted `cost`.
#[allow(clippy::too_many_arguments)]
pub fn check_better_fast(
    this_mv: Mv,
    st: &mut SubpelState,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    has_better_mv: &mut bool,
    is_scaled: bool,
) -> u32 {
    if !is_subpelmv_in_range(mv_limits, this_mv) {
        return i32::MAX as u32;
    }
    let mut cost = mv_cost_params.err_cost(this_mv) as u32;
    if mv_cost_params.mv_cost_type == MvCostType::Opt {
        // C: `int64_t bestcost = *distortion + cost;` — `int + unsigned int`
        // is evaluated in `unsigned int` and only then widened.
        let bestcost = i64::from((st.distortion as u32).wrapping_add(cost));
        if bestcost > (i64::from(st.besterr) * i64::from(mv_cost_params.early_exit_th)) / 1000 {
            return bestcost as u32;
        }
    }
    let (thismse, sse) = if is_scaled {
        var_params.upsampled_pref_error(this_mv)
    } else {
        var_params.estimated_pref_error(this_mv)
    };
    cost = cost.wrapping_add(thismse);
    let mut weight: u64 = 100;
    if var_params.bias_fp != 0 && st.best_mv.x % 8 == 0 && st.best_mv.y % 8 == 0 {
        weight = var_params.bias_fp as u64;
    }
    if (u64::from(cost) * weight) / 100 < u64::from(st.besterr) {
        st.besterr = cost;
        st.best_mv = this_mv;
        st.distortion = thismse as i32;
        st.sse1 = sse;
        *has_better_mv = true;
    }
    cost
}

/// C `svt_check_better` (mcomp.c:219-246) — the UNPRUNED tree's per-candidate
/// compare.
///
/// Distinct from [`check_better_fast`] in two ways that change the search:
/// it always uses the upsampled (8-tap) error, never the bilinear estimate,
/// and it has no `MV_COST_OPT` early exit. Porting one and reusing it for the
/// other would silently change the metric.
pub fn check_better(
    this_mv: Mv,
    st: &mut SubpelState,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    is_better: &mut bool,
) -> u32 {
    if !is_subpelmv_in_range(mv_limits, this_mv) {
        return i32::MAX as u32;
    }
    let (thismse, sse) = var_params.upsampled_pref_error(this_mv);
    let mut cost = mv_cost_params.err_cost(this_mv) as u32;
    cost = cost.wrapping_add(thismse);
    let mut weight: u64 = 100;
    if var_params.bias_fp != 0 && st.best_mv.x % 8 == 0 && st.best_mv.y % 8 == 0 {
        weight = var_params.bias_fp as u64;
    }
    if (u64::from(cost) * weight) / 100 < u64::from(st.besterr) {
        st.besterr = cost;
        st.best_mv = this_mv;
        st.distortion = thismse as i32;
        st.sse1 = sse;
        *is_better = true;
    }
    cost
}

/// C `get_best_diag_step` (mcomp.c:248-254).
///
/// The `<=` comparisons are load-bearing: on a flat block where two opposite
/// costs tie, `<=` picks the NEGATIVE step, and flipping it to `<` probes the
/// other diagonal and can change the coded MV.
#[inline]
pub fn get_best_diag_step(
    step_size: i32,
    left_cost: u32,
    right_cost: u32,
    up_cost: u32,
    down_cost: u32,
) -> Mv {
    Mv {
        x: if left_cost <= right_cost {
            -step_size
        } else {
            step_size
        } as i16,
        y: if up_cost <= down_cost {
            -step_size
        } else {
            step_size
        } as i16,
    }
}

/// C's `{{this_mv.x + dx, this_mv.y + dy}}` initialiser: the fields are
/// `int16_t`, so the sum truncates.
#[inline]
fn mv_off(mv: Mv, dx: i32, dy: i32) -> Mv {
    Mv {
        x: (i32::from(mv.x) + dx) as i16,
        y: (i32::from(mv.y) + dy) as i16,
    }
}

// =============================================================================
// §4. The unpruned tree's two probes
// =============================================================================

/// C `svt_first_level_check` (mcomp.c:256-287): probe left/right/up/down, then
/// the diagonal [`get_best_diag_step`] picks. Returns that diagonal step.
///
/// C passes ONE `dummy` int to all five calls and never reads it.
pub fn first_level_check(
    this_mv: Mv,
    st: &mut SubpelState,
    hstep: i32,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
) -> Mv {
    let mut dummy = false;
    let left = check_better(
        mv_off(this_mv, -hstep, 0),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
    );
    let right = check_better(
        mv_off(this_mv, hstep, 0),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
    );
    let up = check_better(
        mv_off(this_mv, 0, -hstep),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
    );
    let down = check_better(
        mv_off(this_mv, 0, hstep),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
    );

    let diag_step = get_best_diag_step(hstep, left, right, up, down);
    let diag_mv = mv_off(this_mv, i32::from(diag_step.x), i32::from(diag_step.y));
    check_better(
        diag_mv,
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
    );
    diag_step
}

/// C `svt_second_level_check_v2` (mcomp.c:289-349).
///
/// The `diag_step` sign flips reproduce C's two `else if` arms exactly: when
/// the winner shares a row with `this_mv` the VERTICAL step is negated, when
/// it shares a column the HORIZONTAL step is negated. `is_scaled` is `(void)`d
/// by C and is not carried here.
#[allow(clippy::too_many_arguments)]
pub fn second_level_check_v2(
    this_mv: Mv,
    mut diag_step: Mv,
    st: &mut SubpelState,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
) {
    if this_mv.x == st.best_mv.x && this_mv.y == st.best_mv.y {
        return;
    } else if this_mv.y == st.best_mv.y {
        diag_step.y = diag_step.y.wrapping_neg();
    } else if this_mv.x == st.best_mv.x {
        diag_step.x = diag_step.x.wrapping_neg();
    }

    let best = st.best_mv;
    let row_bias_mv = Mv {
        x: best.x,
        y: best.y.wrapping_add(diag_step.y),
    };
    let col_bias_mv = Mv {
        x: best.x.wrapping_add(diag_step.x),
        y: best.y,
    };
    let diag_bias_mv = Mv {
        x: best.x.wrapping_add(diag_step.x),
        y: best.y.wrapping_add(diag_step.y),
    };
    let mut has_better_mv = false;
    check_better(
        row_bias_mv,
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut has_better_mv,
    );
    check_better(
        col_bias_mv,
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut has_better_mv,
    );
    if has_better_mv {
        check_better(
            diag_bias_mv,
            st,
            mv_limits,
            var_params,
            mv_cost_params,
            &mut has_better_mv,
        );
    }
}

// =============================================================================
// §5. The pruned tree's two probes
// =============================================================================

/// C `first_level_check_fast` (mcomp.c:364-420).
///
/// Same four-cardinal probe as [`first_level_check`] but with the bilinear
/// metric AND one extra gate: if the incumbent error has not improved past
/// `orgerr`, the diagonal is NOT probed and the step is returned as-is.
#[allow(clippy::too_many_arguments)]
pub fn first_level_check_fast(
    this_mv: Mv,
    st: &mut SubpelState,
    hstep: i32,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    orgerr: u32,
    is_scaled: bool,
) -> Mv {
    let mut dummy = false;
    let left = check_better_fast(
        mv_off(this_mv, -hstep, 0),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
        is_scaled,
    );
    let right = check_better_fast(
        mv_off(this_mv, hstep, 0),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
        is_scaled,
    );
    let up = check_better_fast(
        mv_off(this_mv, 0, -hstep),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
        is_scaled,
    );
    let down = check_better_fast(
        mv_off(this_mv, 0, hstep),
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
        is_scaled,
    );

    let diag_step = get_best_diag_step(hstep, left, right, up, down);
    let diag_mv = mv_off(this_mv, i32::from(diag_step.x), i32::from(diag_step.y));
    if st.besterr >= orgerr {
        return diag_step;
    }
    check_better_fast(
        diag_mv,
        st,
        mv_limits,
        var_params,
        mv_cost_params,
        &mut dummy,
        is_scaled,
    );
    diag_step
}

/// C `second_level_check_fast` (mcomp.c:422-557): two extra chess-pattern
/// probes in the winning quadrant.
///
/// Three arms, chosen by whether the winner moved off `this_mv` in both axes,
/// only the column, or only the row. C's fourth case (`tr == br && tc == bc`,
/// i.e. nothing improved) does nothing, and neither does this.
#[allow(clippy::too_many_arguments)]
pub fn second_level_check_fast(
    this_mv: Mv,
    diag_step: Mv,
    st: &mut SubpelState,
    hstep: i32,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    is_scaled: bool,
) {
    let tr = i32::from(this_mv.y);
    let tc = i32::from(this_mv.x);
    let br = i32::from(st.best_mv.y);
    let bc = i32::from(st.best_mv.x);
    let mut dummy = false;
    let probe = |mv: Mv, st: &mut SubpelState, dummy: &mut bool| {
        check_better_fast(
            mv,
            st,
            mv_limits,
            var_params,
            mv_cost_params,
            dummy,
            is_scaled,
        );
    };
    let dx = i32::from(diag_step.x);
    let dy = i32::from(diag_step.y);
    if tr != br && tc != bc {
        probe(mv16(bc + dx, br), st, &mut dummy);
        probe(mv16(bc, br + dy), st, &mut dummy);
    } else if tr == br && tc != bc {
        // Continue searching in the best direction
        probe(mv16(bc + dx, br + hstep), st, &mut dummy);
        probe(mv16(bc + dx, br - hstep), st, &mut dummy);
        // Search in the direction opposite of the best quadrant
        probe(mv16(bc, br - dy), st, &mut dummy);
    } else if tr != br && tc == bc {
        probe(mv16(bc + hstep, br + dy), st, &mut dummy);
        probe(mv16(bc - hstep, br + dy), st, &mut dummy);
        probe(mv16(bc - dx, br), st, &mut dummy);
    }
}

/// C's `{{x, y}}` `Mv` initialiser from two `int` expressions.
#[inline]
fn mv16(x: i32, y: i32) -> Mv {
    Mv {
        x: x as i16,
        y: y as i16,
    }
}

/// C `two_level_checks_fast` (mcomp.c:559-595): one iteration of the pruned
/// tree.
#[allow(clippy::too_many_arguments)]
pub fn two_level_checks_fast(
    this_mv: Mv,
    st: &mut SubpelState,
    hstep: i32,
    mv_limits: &SubpelMvLimits,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    orgerr: u32,
    iters: i32,
    is_scaled: bool,
) {
    let diag_step = first_level_check_fast(
        this_mv,
        st,
        hstep,
        mv_limits,
        var_params,
        mv_cost_params,
        orgerr,
        is_scaled,
    );
    if st.besterr < orgerr && iters > 1 {
        second_level_check_fast(
            this_mv,
            diag_step,
            st,
            hstep,
            mv_limits,
            var_params,
            mv_cost_params,
            is_scaled,
        );
    }
}

// =============================================================================
// §6. The seed
// =============================================================================

/// C `svt_upsampled_setup_center_error` (mcomp.c:351-359).
///
/// C passes the SAME pointer as both the return destination and the `sse`
/// out-parameter: `*distortion = vfp->vf(..., distortion)`. The callee writes
/// `*sse` first and the assignment of the return value happens after, so
/// `*distortion` ends up holding the VARIANCE, not the sse. Reproduced here by
/// discarding the sse.
///
/// Despite the name it does NOT upsample: it is the plain `vf` at the
/// already-full-pel start MV.
pub fn upsampled_setup_center_error(
    bestmv: Mv,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
) -> (u32, i32) {
    let ref_off = get_buf_from_mv(var_params.ref_base, var_params.ref_stride, bestmv);
    let (var, _sse) = variance_diff_sse(
        var_params.ref_alloc,
        ref_off as usize,
        var_params.ref_stride,
        var_params.src,
        var_params.src_base,
        var_params.src_stride,
        var_params.w,
        var_params.h,
    );
    let distortion = var as i32;
    (
        var.wrapping_add(mv_cost_params.err_cost(bestmv) as u32),
        distortion,
    )
}

// =============================================================================
// §7. The two entry points
// =============================================================================

/// C `SUBPEL_MOTION_SEARCH_PARAMS`'s scalar half (mcomp.h:84-104).
#[derive(Clone, Copy, Debug)]
pub struct SubpelSearchParams {
    pub allow_hp: bool,
    /// C `SUBPEL_FORCE_STOP`: [`EIGHTH_PEL`] .. [`FULL_PEL`].
    pub forced_stop: i32,
    pub iters_per_step: i32,
    pub pred_variance_th: i32,
    pub abs_th_mult: u8,
    pub round_dev_th: i32,
    pub skip_diag_refinement: u8,
    /// [`SPEL_ME`] or [`SPEL_PME`].
    pub search_stage: i32,
    pub list_idx: usize,
    pub ref_idx: usize,
    pub mv_limits: SubpelMvLimits,
}

/// The `ModeDecisionContext` fields the UNPRUNED entry point reads, and the
/// one it writes.
///
/// C reaches these through `ictx`; passing `None` is C's `ictx == NULL`, the
/// arm every caller outside `PD_PASS_1` takes.
#[derive(Clone, Copy, Debug)]
pub struct SubpelMdContext {
    /// C `ctx->pd_pass`.
    pub pd_pass: i32,
    /// C `ctx->md_subpel_me_ctrls.mvp_th`. 0 disables the whole block.
    pub mvp_th: i32,
    /// C `ctx->md_subpel_me_ctrls.hp_mv_th`.
    pub hp_mv_th: i32,
    /// C `ctx->best_fp_mvp_dist[list_idx][ref_idx]`.
    pub best_fp_mvp_dist: u32,
    /// C `ctx->mvp_array[list_idx][ref_idx][ctx->best_fp_mvp_idx[..]]`.
    pub best_fp_mvp: Mv,
    /// OUT: C `ctx->fp_me_dist[list_idx][ref_idx]`, written when
    /// `search_stage == SPEL_ME`.
    pub fp_me_dist: u32,
}

/// C `svt_av1_find_best_sub_pixel_tree_pruned` (mcomp.c:599-679, EXPORTED).
///
/// Selected when `md_subpel_me_ctrls.subpel_search_method == SUBPEL_TREE_PRUNED`
/// (`enc_mode_config.c:3620-3650`, the mid presets), and the ONLY variant
/// `src_ops_process.c:497` uses.
///
/// `is_scaled` is a C local pinned to 0 at `:611` (scaled references are not
/// reachable from this entry point in v4.2.0), so the bilinear metric is
/// always the one used; the parameter is kept so the dead-looking C arm stays
/// translated (`docs/WORKING-ON-THIS.md` §7).
///
/// Returns `(besterr, state)`; C returns `besterr` and writes `bestmv`,
/// `distortion` and `sse1` out. `ctx.fp_me_dist` is updated in place when
/// `search_stage == SPEL_ME`.
pub fn find_best_sub_pixel_tree_pruned(
    ctx: Option<&mut SubpelMdContext>,
    ms: &SubpelSearchParams,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    start_mv: Mv,
    bsize: BlockSize,
) -> (u32, SubpelState) {
    let mut hstep = INIT_SUBPEL_STEP_SIZE;
    let mut st = SubpelState {
        best_mv: start_mv,
        ..Default::default()
    };
    let is_scaled = false;

    let (mut besterr, distortion) =
        upsampled_setup_center_error(st.best_mv, var_params, mv_cost_params);
    st.distortion = distortion;

    if let Some(c) = ctx
        && ms.search_stage == SPEL_ME
    {
        c.fp_me_dist = besterr;
    }

    let th_normalizer = (var_params.w as u32)
        .wrapping_mul(var_params.h as u32)
        .wrapping_mul(u32::from(ms.abs_th_mult));
    if besterr < th_normalizer {
        st.besterr = besterr;
        return (besterr, st);
    }
    st.besterr = besterr;

    // How many steps to take: 0 = full-pel only, 1 = half-pel, and so on.
    let round = (FULL_PEL - ms.forced_stop).min(3 - i32::from(!ms.allow_hp));
    if round == 0 {
        return (besterr, st);
    }

    if ms.pred_variance_th != 0 {
        let ref_off = get_buf_from_mv(var_params.ref_base, var_params.ref_stride, st.best_mv);
        let (var, _sse) = variance_diff_sse(
            var_params.ref_alloc,
            ref_off as usize,
            var_params.ref_stride,
            &EB_AV1_VAR_OFFS,
            0,
            0,
            var_params.w,
            var_params.h,
        );
        let block_var =
            round_power_of_two(var, u32::from(NUM_PELS_LOG2_LOOKUP[bsize as usize])) as i32;
        if block_var < ms.pred_variance_th {
            return (besterr, st);
        }
    }

    let mut org_error = if ms.skip_diag_refinement >= 4 {
        0u32
    } else {
        let demo: u32 = if ms.skip_diag_refinement >= 2 {
            if var_params.w >= 64 || var_params.h >= 64 {
                2
            } else {
                1
            }
        } else {
            1
        };
        if ms.skip_diag_refinement != 0 {
            besterr / demo
        } else {
            i32::MAX as u32
        }
    };

    let mut this_mv = start_mv;
    for iter in 0..round {
        let prev_besterr = besterr;
        two_level_checks_fast(
            this_mv,
            &mut st,
            hstep,
            &ms.mv_limits,
            var_params,
            mv_cost_params,
            org_error,
            ms.iters_per_step,
            is_scaled,
        );
        besterr = st.besterr;
        hstep >>= 1;
        this_mv = st.best_mv;
        if ms.skip_diag_refinement != 0 && iter < QUARTER_PEL {
            org_error = org_error.min(besterr);
        }
        let deviation = (((i64::from(besterr.max(1))) - (i64::from(prev_besterr.max(1)))) * 100)
            / i64::from(prev_besterr.max(1));
        if deviation as i32 >= ms.round_dev_th {
            return (besterr, st);
        }
    }
    (besterr, st)
}

/// C `svt_av1_find_best_sub_pixel_tree` (mcomp.c:683-771, EXPORTED).
///
/// The unpruned tree, selected at `SUBPEL_TREE` (`enc_mode_config.c:3575/3590/
/// 3605/3721` — the slow presets and the PME control). Every candidate is
/// scored with the 8-tap upsampled error, and there is no `org_error` /
/// `round_dev_th` early exit.
///
/// The `ctx` block at `:706-723` is live only when `pd_pass == PD_PASS_1` AND
/// `md_subpel_me_ctrls.mvp_th != 0`; it can shorten `round` to 1 or 2 based on
/// how much better the ME winner is than the best MVP.
pub fn find_best_sub_pixel_tree(
    ctx: Option<&mut SubpelMdContext>,
    ms: &SubpelSearchParams,
    var_params: &SubpelSearchVarParams<'_>,
    mv_cost_params: &MvCostParams<'_>,
    start_mv: Mv,
    bsize: BlockSize,
) -> (u32, SubpelState) {
    let mut round = (FULL_PEL - ms.forced_stop).min(3 - i32::from(!ms.allow_hp));
    let mut hstep = INIT_SUBPEL_STEP_SIZE;
    let mut st = SubpelState {
        best_mv: start_mv,
        ..Default::default()
    };

    let (besterr, distortion) =
        upsampled_setup_center_error(st.best_mv, var_params, mv_cost_params);
    st.distortion = distortion;
    st.besterr = besterr;

    if let Some(c) = ctx
        && ms.search_stage == SPEL_ME
    {
        c.fp_me_dist = besterr;
        if c.pd_pass == PD_PASS_1 && c.mvp_th != 0 {
            // C: `unsigned int best_mvperr = ...; const int mvp_err =
            // best_mvperr + 1; const int me_err = besterr + 1;`. BOTH `+ 1`s
            // happen in UNSIGNED 32-bit (the operands are `unsigned int`) and
            // only then convert to `int`; the subtraction and the `* 100` are
            // then signed and wrap on overflow in every compiler that builds
            // this tree. Spelled out with wrapping ops so a large
            // `best_fp_mvp_dist` reproduces C instead of panicking.
            //
            // `me_err == 0` (i.e. `besterr == UINT32_MAX`) would divide by zero
            // in C too; it is unreachable because `besterr` is a variance plus
            // an MV cost over a <= 128x128 8-bit block.
            let mvp_err = c.best_fp_mvp_dist.wrapping_add(1) as i32;
            let me_err = besterr.wrapping_add(1) as i32;
            let deviation = me_err.wrapping_sub(mvp_err).wrapping_mul(100) / me_err;
            if deviation >= c.mvp_th {
                round = 1;
            } else if (i32::from(st.best_mv.x) - i32::from(c.best_fp_mvp.x)).abs() > c.hp_mv_th
                || (i32::from(st.best_mv.y) - i32::from(c.best_fp_mvp.y)).abs() > c.hp_mv_th
            {
                round = round.min(2);
            }
        }
    }

    let th_normalizer = (var_params.w as u32)
        .wrapping_mul(var_params.h as u32)
        .wrapping_mul(u32::from(ms.abs_th_mult));
    if besterr < th_normalizer {
        return (besterr, st);
    }
    if round == 0 {
        return (besterr, st);
    }

    if ms.pred_variance_th != 0 {
        let ref_off = get_buf_from_mv(var_params.ref_base, var_params.ref_stride, st.best_mv);
        let (var, _sse) = variance_diff_sse(
            var_params.ref_alloc,
            ref_off as usize,
            var_params.ref_stride,
            &EB_AV1_VAR_OFFS,
            0,
            0,
            var_params.w,
            var_params.h,
        );
        let block_var =
            round_power_of_two(var, u32::from(NUM_PELS_LOG2_LOOKUP[bsize as usize])) as i32;
        if block_var < ms.pred_variance_th {
            return (besterr, st);
        }
    }

    for _iter in 0..round {
        let iter_center_mv = st.best_mv;
        let diag_step = first_level_check(
            iter_center_mv,
            &mut st,
            hstep,
            &ms.mv_limits,
            var_params,
            mv_cost_params,
        );

        if !(iter_center_mv.x == st.best_mv.x && iter_center_mv.y == st.best_mv.y)
            && ms.iters_per_step > 1
        {
            second_level_check_v2(
                iter_center_mv,
                diag_step,
                &mut st,
                &ms.mv_limits,
                var_params,
                mv_cost_params,
            );
        }
        hstep >>= 1;
    }
    (st.besterr, st)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C `svt_get_buf_from_mv` uses `>>` not `/`. `-1 >> 3 == -1` floors, a
    /// truncating divide would give 0 — wrong on every negative MV.
    #[test]
    fn get_buf_from_mv_floors_negatives() {
        let stride = 32usize;
        assert_eq!(
            get_buf_from_mv(1000, stride, Mv { x: -1, y: -1 }),
            1000 - 33
        );
        assert_eq!(
            get_buf_from_mv(1000, stride, Mv { x: -8, y: -8 }),
            1000 - 33
        );
        assert_eq!(
            get_buf_from_mv(1000, stride, Mv { x: -9, y: -9 }),
            1000 - 66
        );
        assert_eq!(get_buf_from_mv(1000, stride, Mv { x: 9, y: 9 }), 1000 + 33);
    }

    /// `x & 7` pairs with the floor above: for -9 the phase is 7 and the base
    /// steps back two pixels, which together address the same sample C does.
    #[test]
    fn subpel_part_masks() {
        assert_eq!(get_subpel_part(-9), 7);
        assert_eq!(get_subpel_part(-1), 7);
        assert_eq!(get_subpel_part(0), 0);
        assert_eq!(get_subpel_part(9), 1);
    }

    /// The `<=` tie direction in `get_best_diag_step` picks the NEGATIVE step.
    #[test]
    fn diag_step_ties_negative() {
        let s = get_best_diag_step(4, 100, 100, 100, 100);
        assert_eq!((s.x, s.y), (-4, -4));
        let s = get_best_diag_step(4, 101, 100, 100, 101);
        assert_eq!((s.x, s.y), (4, -4));
    }

    /// The three table-free `MV_COST_TYPE` arms, hand-derived from
    /// mcomp.c:56-61. MIDRES's lambda is 0, so it is always 0 — that is the C
    /// value, not a stub.
    #[test]
    fn l1_arms_hand_derived() {
        let mv = Mv { x: 13, y: -20 };
        let rmv = Mv { x: 1, y: 4 };
        // |dx| = 12, |dy| = 24 -> sum 36
        assert_eq!(
            mv_err_cost(mv, rmv, None, 0, MvCostType::L1LowRes),
            (2 * 36) >> 3
        );
        assert_eq!(mv_err_cost(mv, rmv, None, 0, MvCostType::L1MidRes), 0);
        // C: `(SSE_LAMBDA_HDRES * (abs_diff.y + abs_diff.x)) >> 3` with
        // SSE_LAMBDA_HDRES == 1, so the product is written out as 36.
        assert_eq!(mv_err_cost(mv, rmv, None, 0, MvCostType::L1HdRes), 36 >> 3);
        assert_eq!(mv_err_cost(mv, rmv, None, 0, MvCostType::None), 0);
        // The MV_COST_ENTROPY `return 0` is transcribed but unreachable in C
        // (see the PORT-NOTE on `mv_err_cost`), so it is asserted here rather
        // than differentially: C crashes on the input that would reach it.
        assert_eq!(mv_err_cost(mv, rmv, None, 77, MvCostType::Entropy), 0);
    }
}
