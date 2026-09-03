//! The MD-level motion-search cost model and the PME SAD kernel —
//! `Source/Lib/Codec/product_coding_loop.c` + the `mcomp.c` cost helpers
//! it drives.
//!
//! | this module | C |
//! |---|---|
//! | [`MvCostType`] | `mcomp.h:29-36` |
//! | [`MvCostParams`] | `mcomp.h:38-49` |
//! | [`mv_err_cost`] | `mcomp.c:42-72` (`svt_mv_err_cost`, ALL six arms) |
//! | [`fp_mv_err_cost`] | `mcomp.c:775-777` (`svt_aom_fp_mv_err_cost`) |
//! | [`pme_sad_loop_kernel`] | `product_coding_loop.c:1775-1826` (EXPORTED) |
//! | [`get_fullmv_from_mv`] | `mv.h:60-63` |
//! | [`get_sad_per_bit`] / [`SAD_PER_BIT_LUT_8`] / [`SAD_PER_BIT_LUT_10`] | `mode_decision.c:2044-2062` + `svt_av1_init_me_luts` |
//! | [`init_mv_cost_params`] | `product_coding_loop.c:1901-1912` |
//!
//! # Why the PME kernel is not `crate::inter_me::sad`
//!
//! `svt_pme_sad_loop_kernel_c` is a **different kernel** from
//! `svt_sad_loop_kernel_c` even though the names rhyme: this one folds
//! `svt_aom_fp_mv_err_cost` into the per-position cost, and its column
//! walk is the 8-wide `col_num` ratchet (a full-rate scan of eight
//! columns, then a `search_step` jump) rather than a uniform stride. A
//! port that reused the plain SAD loop would search the same positions
//! with the wrong costs AND, at `search_step > 1`, a different position
//! set.
//!
//! # Evidence
//!
//! Tier 1 for `svt_pme_sad_loop_kernel_c` and `svt_aom_get_sad_per_bit`
//! (both EXPORTED, `nm -g`-checked) — `tests/c_parity_md_pme.rs`.
//! `svt_mv_err_cost` / `svt_aom_fp_mv_err_cost` are exercised through the
//! kernel's own cost term, which is the only way C's arithmetic reaches
//! an exported boundary here; `svt_aom_fp_mv_err_cost` is itself exported
//! and is gated directly as well.
//! [`init_mv_cost_params`] is `static` in C — **tier 4**.

use archmage::prelude::*;
use svtav1_dsp::me_sad::block_sad_scalar;
#[cfg(target_arch = "x86_64")]
use svtav1_dsp::me_sad::block_sad_v3;
#[cfg(target_arch = "aarch64")]
use svtav1_dsp::me_sad::{block_sad_arm_v2, block_sad_neon};
use svtav1_types::motion::Mv;

// ---------------------------------------------------------------------------
// Cost model (mcomp.h:29-49, mcomp.c:30-81)
// ---------------------------------------------------------------------------

/// C `MV_COST_TYPE` (mcomp.h:29-36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MvCostType {
    /// Entropy rate of the MV (the default).
    Entropy = 0,
    /// L1 norm, < 480p.
    L1LowRes = 1,
    /// L1 norm, >= 480p.
    L1MidRes = 2,
    /// L1 norm, >= 720p.
    L1HdRes = 3,
    /// Scaled L1 norm against `error_per_bit`.
    Opt = 4,
    /// Always 0.
    None = 5,
}

/// C `SSE_LAMBDA_LOWRES` / `_MIDRES` / `_HDRES` (mcomp.c:32-36).
///
/// Note `MIDRES` is **0** — the mid-resolution arm charges no MV cost at
/// all. That is C's value, not an omission.
const SSE_LAMBDA_LOWRES: i32 = 2;
const SSE_LAMBDA_MIDRES: i32 = 0;
const SSE_LAMBDA_HDRES: i32 = 1;

/// C `PIXEL_TRANSFORM_ERROR_SCALE` (mcomp.c:40).
const PIXEL_TRANSFORM_ERROR_SCALE: u32 = 4;
/// C `RDDIV_BITS` (rd_cost.h:34) / `AV1_PROB_COST_SHIFT`
/// (md_rate_estimation.h:29) / `RD_EPB_SHIFT` (restoration.h:342).
const RDDIV_BITS: u32 = 7;
const AV1_PROB_COST_SHIFT: u32 = 9;
const RD_EPB_SHIFT: u32 = 6;

/// The shift `svt_mv_err_cost`'s ENTROPY and OPT arms round by.
const MV_ERR_COST_SHIFT: u32 =
    RDDIV_BITS + AV1_PROB_COST_SHIFT - RD_EPB_SHIFT + PIXEL_TRANSFORM_ERROR_SCALE;

/// C `ROUND_POWER_OF_TWO_64`.
#[inline]
fn round_power_of_two_64(value: i64, n: u32) -> i64 {
    (value + (1i64 << (n - 1))) >> n
}

/// C `MV_MAX` (cabac_context_model.h:194) = `(1 << 14) - 1`.
pub const MV_MAX: i32 = (1 << 14) - 1;
/// C `MV_VALS` (cabac_context_model.h:195) = `(MV_MAX << 1) + 1`.
pub const MV_VALS: usize = ((MV_MAX << 1) + 1) as usize;
/// C `MV_LOW` / `MV_UPP` (cabac_context_model.h:198-199).
const MV_LOW: i32 = -(1 << 14);
const MV_UPP: i32 = 1 << 14;

/// C's `mvjcost` + `mvcost[2]` triple as
/// `svt_mv_cost` (mcomp.h:138-142) reads them.
///
/// Deliberately NOT [`crate::intrabc::MvCostTables`]: that type clamps the
/// component index to `+-MV_MAX`, while `svt_mv_cost` clips to
/// `CLIP3(MV_LOW, MV_UPP, .)` — i.e. `+-16384`, one wider. The two agree
/// everywhere except a per-component diff of exactly `+16384`, where C
/// indexes `nmv_costs[i][MV_MAX + 16384]` = element **32767** of a
/// `MV_VALS = 32767`-long row, one past its end. See
/// [`MvCostTable::comp_cost`] for what this port does there instead.
#[derive(Debug, Clone)]
pub struct MvCostTable {
    /// C `mvjcost`, 4 entries (`MV_JOINTS`).
    pub joint: [i32; 4],
    /// C `mvcost[0]` (the ROW/y component, coded first) and `mvcost[1]`
    /// (the column/x component), each `MV_VALS` long and indexed
    /// `MV_MAX + value`.
    pub comp: [Vec<i32>; 2],
}

impl MvCostTable {
    /// C `comp_cost[i][CLIP3(MV_LOW, MV_UPP, v)]` with the index kept
    /// inside the table.
    ///
    /// C's clip admits `+16384`, whose index is one past the row. The
    /// port clamps the INDEX to the last valid element rather than
    /// reproducing the read; the value C would get there is
    /// `nmv_costs[1][0]` for component 0 and unrelated struct memory for
    /// component 1, i.e. not a defined cost at all. Callers never reach
    /// it: `is_valid_mv_diff` (mode_decision.c:776) rejects any candidate
    /// whose per-component MV-minus-predmv exceeds `1 << 14` BEFORE
    /// injection, and the search operates on smaller diffs still.
    #[inline]
    pub fn comp_cost(&self, i: usize, v: i32) -> i32 {
        let clipped = v.clamp(MV_LOW, MV_UPP);
        let idx = ((MV_MAX + clipped) as usize).min(MV_VALS - 1);
        self.comp[i][idx]
    }

    /// C `svt_mv_cost` (mcomp.h:138-142). `diff` is `mv - ref_mv`.
    #[inline]
    pub fn mv_cost(&self, diff: Mv) -> i32 {
        self.joint[mv_joint_index(i32::from(diff.x), i32::from(diff.y))]
            + self.comp_cost(0, i32::from(diff.y))
            + self.comp_cost(1, i32::from(diff.x))
    }
}

/// C `svt_av1_get_mv_joint` (rd_cost.c:41-53).
#[inline]
fn mv_joint_index(diff_x: i32, diff_y: i32) -> usize {
    if diff_y == 0 {
        if diff_x == 0 { 0 } else { 1 }
    } else if diff_x == 0 {
        2
    } else {
        3
    }
}

/// C `svt_mv_cost_param` (mcomp.h:38-49).
///
/// `mv_cost_tables` stands in for C's `mvjcost` + `mvcost[2]` triple: C
/// tests `if (mvcost)` and returns 0 when the pointer is NULL, which is
/// `None` here. `full_ref_mv` is carried because C stores it even though
/// `svt_mv_err_cost` does not read it — the SAD-domain callers do.
#[derive(Debug, Clone)]
pub struct MvCostParams<'a> {
    /// C `ref_mv` — eighth-pel.
    pub ref_mv: Mv,
    /// C `full_ref_mv` — `get_fullmv_from_mv(ref_mv)`.
    pub full_ref_mv: Mv,
    /// C `mv_cost_type`.
    pub mv_cost_type: MvCostType,
    /// C `mvjcost` + `mvcost[2]`; `None` is C's NULL `mvcost`.
    pub mv_cost_tables: Option<&'a MvCostTable>,
    /// C `error_per_bit`.
    pub error_per_bit: i32,
    /// C `early_exit_th`.
    pub early_exit_th: i32,
    /// C `sad_per_bit`.
    pub sad_per_bit: i32,
}

/// C `svt_mv_err_cost` (mcomp.c:42-72, `static INLINE`).
///
/// All six arms. The three L1 arms shift the weighted sum right by 3
/// AFTER the multiply, so `MID_RES`'s zero lambda makes them free and
/// `HDRES`'s 1 makes them `(|dy| + |dx|) >> 3`.
pub fn mv_err_cost(mv: Mv, params: &MvCostParams<'_>) -> i32 {
    // C stores the difference into an `Mv` — int16_t fields — BEFORE
    // anything reads it, so both the cost lookup and the `abs_diff` term
    // see the TRUNCATED value, not a widened one.
    let diff = Mv {
        x: mv.x.wrapping_sub(params.ref_mv.x),
        y: mv.y.wrapping_sub(params.ref_mv.y),
    };
    // C's `abs_diff` is likewise an `Mv`, so `abs(i16::MIN)` wraps back to
    // `i16::MIN` rather than becoming 32768.
    let abs_sum = i32::from(diff.y.wrapping_abs()) + i32::from(diff.x.wrapping_abs());

    match params.mv_cost_type {
        MvCostType::Entropy => match params.mv_cost_tables {
            Some(t) => {
                let cost = i64::from(t.mv_cost(diff)) * i64::from(params.error_per_bit);
                round_power_of_two_64(cost, MV_ERR_COST_SHIFT) as i32
            }
            // C guards this arm with `if (mvcost)` and returns 0
            // otherwise — but `mvcost` is `const int* mvcost[2]`, an
            // ARRAY member, so the test is a tautology and the zero
            // return is DEAD CODE: with NULL component pointers C
            // dereferences them and crashes (measured: SIGSEGV driving
            // svt_aom_fp_mv_err_cost with a zeroed svt_mv_cost_param).
            // `None` here therefore means "no table configured", a state
            // C's ENTROPY arm cannot survive; the port returns 0 rather
            // than reproducing the crash.
            None => 0,
        },
        MvCostType::L1LowRes => (SSE_LAMBDA_LOWRES * abs_sum) >> 3,
        MvCostType::L1MidRes => (SSE_LAMBDA_MIDRES * abs_sum) >> 3,
        MvCostType::L1HdRes => (SSE_LAMBDA_HDRES * abs_sum) >> 3,
        MvCostType::Opt => {
            let cost = i64::from(abs_sum << 8) * i64::from(params.error_per_bit);
            round_power_of_two_64(cost, MV_ERR_COST_SHIFT) as i32
        }
        MvCostType::None => 0,
    }
}

/// C `svt_aom_fp_mv_err_cost` (mcomp.c:775-777, EXPORTED) — a thin
/// forward to [`mv_err_cost`].
#[inline]
pub fn fp_mv_err_cost(mv: Mv, params: &MvCostParams<'_>) -> i32 {
    mv_err_cost(mv, params)
}

// ---------------------------------------------------------------------------
// MV precision helpers (mv.h:57-68)
// ---------------------------------------------------------------------------

/// C `GET_MV_RAWPEL` (mv.h:57): `((x) + 3 + ((x) >= 0)) >> 3`.
///
/// The `+ ((x) >= 0)` term makes this an asymmetric round — do NOT
/// substitute a plain `(x + 4) >> 3`.
#[inline]
pub fn get_mv_rawpel(x: i16) -> i16 {
    let x = i32::from(x);
    ((x + 3 + i32::from(x >= 0)) >> 3) as i16
}

/// C `get_fullmv_from_mv` (mv.h:60-63).
#[inline]
pub fn get_fullmv_from_mv(subpel_mv: Mv) -> Mv {
    Mv {
        x: get_mv_rawpel(subpel_mv.x),
        y: get_mv_rawpel(subpel_mv.y),
    }
}

// ---------------------------------------------------------------------------
// SAD-per-bit LUT (mode_decision.c:2044-2062)
// ---------------------------------------------------------------------------

/// C `QINDEX_RANGE`.
pub const QINDEX_RANGE: usize = 256;

/// C `svt_av1_convert_qindex_to_q` (rc_process.c:186-198) for
/// `EB_EIGHT_BIT` / `EB_TEN_BIT`.
fn convert_qindex_to_q(qindex: usize, ten_bit: bool) -> f64 {
    if ten_bit {
        f64::from(crate::bd10::ac_qlookup_10(qindex as u8)) / 16.0
    } else {
        f64::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex]) / 4.0
    }
}

/// C `init_me_luts_bd` (mode_decision.c:2052-2062): `0.0418 * q + 2.4107`,
/// truncated toward zero by C's `(int)` cast.
fn init_me_lut(ten_bit: bool) -> [i32; QINDEX_RANGE] {
    let mut lut = [0i32; QINDEX_RANGE];
    let mut i = 0;
    while i < QINDEX_RANGE {
        lut[i] = (0.0418 * convert_qindex_to_q(i, ten_bit) + 2.4107) as i32;
        i += 1;
    }
    lut
}

/// C `sad_per_bit_lut_8`, built by `svt_av1_init_me_luts`.
pub fn sad_per_bit_lut_8() -> [i32; QINDEX_RANGE] {
    init_me_lut(false)
}

/// C `sad_per_bit_lut_10`.
pub fn sad_per_bit_lut_10() -> [i32; QINDEX_RANGE] {
    init_me_lut(true)
}

/// C `svt_aom_get_sad_per_bit` (mode_decision.c:2048-2050, EXPORTED).
///
/// **C's second parameter is declared `EbBitDepth` but used as a
/// BOOLEAN** (`is_hbd ? lut_10 : lut_8`), and `EB_EIGHT_BIT` is **8** —
/// truthy. Passing the enum selects the TEN-bit table for eight-bit
/// content. Every C call site passes a 0/1 flag instead
/// (mode_decision.c:2109 a literal 0; product_coding_loop.c:1908 the
/// `uint8_t hbd_md`), so the port takes a `bool`. This cost a
/// tier-1 red at qidx=50 before the shim was corrected.
#[inline]
pub fn get_sad_per_bit(qidx: usize, is_hbd: bool) -> i32 {
    if is_hbd {
        sad_per_bit_lut_10()[qidx]
    } else {
        sad_per_bit_lut_8()[qidx]
    }
}

// ---------------------------------------------------------------------------
// svt_init_mv_cost_params (product_coding_loop.c:1901-1912)
// ---------------------------------------------------------------------------

/// C `svt_init_mv_cost_params` (product_coding_loop.c:1901-1912,
/// `static`) — **tier 4**.
///
/// `skip_diag_refinement >= 3` picks `MV_COST_OPT`, everything else
/// `MV_COST_ENTROPY`; that single comparison is what decides whether the
/// whole MD search prices MVs by entropy or by a scaled L1 norm.
pub fn init_mv_cost_params<'a>(
    ref_mv: Mv,
    sq_size: u16,
    skip_diag_refinement: u8,
    rdmult: u32,
    base_q_idx: usize,
    hbd_md: bool,
    mv_cost_tables: Option<&'a MvCostTable>,
) -> MvCostParams<'a> {
    MvCostParams {
        ref_mv,
        full_ref_mv: get_fullmv_from_mv(ref_mv),
        mv_cost_type: if skip_diag_refinement >= 3 {
            MvCostType::Opt
        } else {
            MvCostType::Entropy
        },
        mv_cost_tables,
        error_per_bit: ((rdmult >> RD_EPB_SHIFT).max(1)) as i32,
        early_exit_th: 1020 - i32::from(sq_size >> 2),
        sad_per_bit: get_sad_per_bit(base_q_idx, hbd_md),
    }
}

// ---------------------------------------------------------------------------
// svt_pme_sad_loop_kernel_c (product_coding_loop.c:1775-1826, EXPORTED)
// ---------------------------------------------------------------------------

/// The running best of [`pme_sad_loop_kernel`], C's three out-params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmeBest {
    pub cost: u32,
    pub mvx: i16,
    pub mvy: i16,
}

/// C `svt_pme_sad_loop_kernel_c` (product_coding_loop.c:1775-1826,
/// EXPORTED).
///
/// A full-pel SAD scan over a search rectangle that folds the MV rate
/// ([`fp_mv_err_cost`]) into every position's cost.
///
/// Three details that a naive rewrite gets wrong:
///
/// * **The column walk is an 8-wide ratchet, not a uniform stride.**
///   `col_num` counts 0..7; while it is below 7 the x step is 1, and on
///   the eighth column it resets and the step becomes `search_step`. So
///   positions are visited in runs of eight at full rate separated by
///   `search_step` jumps.
/// * **The `(search_area_width - x) < 8 && col_num == 0` guard
///   `continue`s WITHOUT advancing x**, because the `continue` skips the
///   `col_num` update too and `search_step_x` keeps its previous value.
///   With `search_step_x == 1` that is a plain skip of the tail; the port
///   reproduces the control flow rather than the intent.
/// * **`ref` advances by `search_step * ref_stride` per OUTER row**, not
///   per y position — the y loop already steps by `search_step`, so the
///   pointer and the index stay in sync only because both use the same
///   step.
///
/// `src` is indexed from 0; `ref_base` is indexed from 0 and the caller
/// has already applied C's `ref_origin_index`.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn pme_sad_loop_kernel_generic<F>(
    sad: &F,
    params: &MvCostParams<'_>,
    src: &[u8],
    src_stride: usize,
    ref_buf: &[u8],
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    best: &mut PmeBest,
    search_position_start_x: i16,
    search_position_start_y: i16,
    search_area_width: i16,
    search_area_height: i16,
    search_step: i16,
    mvx: i16,
    mvy: i16,
) where
    F: Fn(&[u8], usize, &[u8], usize, usize, usize) -> u32,
{
    let mut col_num: i16 = 0;
    let mut search_step_x: i16 = 1;
    // C advances the `ref` POINTER by search_step * ref_stride per outer
    // iteration; the port keeps the equivalent row base as an index.
    let mut ref_row_base: isize = 0;

    let mut y_search_index: i16 = 0;
    while y_search_index < search_area_height {
        let mut x_search_index: i16 = 0;
        while x_search_index < search_area_width {
            if (search_area_width - x_search_index) < 8 && col_num == 0 {
                // C's `continue` re-enters the for-loop's increment, which
                // is `xSearchIndex += search_step_x` with the value
                // search_step_x had on entry to this iteration.
                x_search_index += search_step_x;
                continue;
            }
            if col_num == 7 {
                col_num = 0;
                search_step_x = search_step;
            } else {
                col_num += 1;
                search_step_x = 1;
            }

            // C walks the block with two scalar loops; the sum is a plain
            // integer SAD, so the SIMD kernel is bit-identical (see
            // `svtav1_dsp::me_sad`). `ref_row_base` only ever grows from 0.
            let r_base = (ref_row_base + isize::from(x_search_index)) as usize;
            let mut cost: u32 = sad(
                src,
                src_stride,
                &ref_buf[r_base..],
                ref_stride,
                block_width,
                block_height,
            );

            // C computes the refinement position into `uint32_t` and then
            // multiplies by 8 BEFORE adding mvx/mvy, so a negative start
            // position wraps through unsigned arithmetic and the sum is
            // taken modulo 2^16 on assignment to an int16_t. Reproduced
            // with wrapping i32 math narrowed the same way.
            let refinement_pos_x =
                (search_position_start_x as i32).wrapping_add(x_search_index as i32);
            let refinement_pos_y =
                (search_position_start_y as i32).wrapping_add(y_search_index as i32);
            let cand_x = (mvx as i32).wrapping_add(refinement_pos_x.wrapping_mul(8)) as i16;
            let cand_y = (mvy as i32).wrapping_add(refinement_pos_y.wrapping_mul(8)) as i16;
            let best_mv = Mv {
                x: cand_x,
                y: cand_y,
            };
            cost = cost.wrapping_add(fp_mv_err_cost(best_mv, params) as u32);
            if cost < best.cost {
                best.mvx = cand_x;
                best.mvy = cand_y;
                best.cost = cost;
            }

            x_search_index += search_step_x;
        }
        ref_row_base += isize::from(search_step) * ref_stride as isize;
        y_search_index += search_step;
    }
}

// ---------------------------------------------------------------------------
// Tier wrappers for `pme_sad_loop_kernel`.
//
// The generic body above is `#[inline(always)]` and takes the block SAD as a
// closure; the closure inherits the `#[arcane]` wrapper's target features, so
// the search loop runs with ONE target-feature boundary per call instead of
// one per search position.
// ---------------------------------------------------------------------------

macro_rules! pme_sad_loop_variant {
    ($(#[$m:meta])* $name:ident, $tok:ident, $k:ident) => {
        $(#[$m])*
        #[allow(clippy::too_many_arguments)]
        fn $name(
            token: $tok,
            params: &MvCostParams<'_>,
            src: &[u8],
            src_stride: usize,
            ref_buf: &[u8],
            ref_stride: usize,
            block_height: usize,
            block_width: usize,
            best: &mut PmeBest,
            search_position_start_x: i16,
            search_position_start_y: i16,
            search_area_width: i16,
            search_area_height: i16,
            search_step: i16,
            mvx: i16,
            mvy: i16,
        ) {
            let sad = |a: &[u8], sa: usize, b: &[u8], sb: usize, w: usize, h: usize| {
                $k(token, a, sa, b, sb, w, h)
            };
            pme_sad_loop_kernel_generic(
                &sad,
                params,
                src,
                src_stride,
                ref_buf,
                ref_stride,
                block_height,
                block_width,
                best,
                search_position_start_x,
                search_position_start_y,
                search_area_width,
                search_area_height,
                search_step,
                mvx,
                mvy,
            );
        }
    };
}

pme_sad_loop_variant!(pme_sad_loop_dispatch_scalar, ScalarToken, block_sad_scalar);
#[cfg(target_arch = "aarch64")]
pme_sad_loop_variant!(
    #[arcane]
    pme_sad_loop_dispatch_neon,
    NeonToken,
    block_sad_neon
);
#[cfg(target_arch = "aarch64")]
pme_sad_loop_variant!(
    #[arcane]
    pme_sad_loop_dispatch_arm_v2,
    Arm64V2Token,
    block_sad_arm_v2
);
#[cfg(target_arch = "x86_64")]
pme_sad_loop_variant!(
    #[arcane]
    pme_sad_loop_dispatch_v3,
    Desktop64,
    block_sad_v3
);

/// C `svt_pme_sad_loop_kernel_c` (product_coding_loop.c:1775-1826, EXPORTED).
///
/// See [`pme_sad_loop_kernel_generic`] for the three control-flow details a
/// naive rewrite gets wrong; this is the dispatching entry point.
#[allow(clippy::too_many_arguments)]
pub fn pme_sad_loop_kernel(
    params: &MvCostParams<'_>,
    src: &[u8],
    src_stride: usize,
    ref_buf: &[u8],
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    best: &mut PmeBest,
    search_position_start_x: i16,
    search_position_start_y: i16,
    search_area_width: i16,
    search_area_height: i16,
    search_step: i16,
    mvx: i16,
    mvy: i16,
) {
    incant!(
        pme_sad_loop_dispatch(
            params,
            src,
            src_stride,
            ref_buf,
            ref_stride,
            block_height,
            block_width,
            best,
            search_position_start_x,
            search_position_start_y,
            search_area_width,
            search_area_height,
            search_step,
            mvx,
            mvy
        ),
        [arm_v2, v3, neon, scalar]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TIER 4 — `get_fullmv_from_mv` / `GET_MV_RAWPEL` (mv.h:57) is a
    /// macro; the asymmetric `+ ((x) >= 0)` term is what this pins.
    #[test]
    fn tier4_get_mv_rawpel_is_asymmetric() {
        assert_eq!(get_mv_rawpel(0), 0);
        assert_eq!(get_mv_rawpel(4), 1);
        // (-4 + 3 + 0) >> 3 is (-1) >> 3 = -1 under C's arithmetic shift,
        // NOT 0 — a truncating-toward-zero port would give 0 here.
        assert_eq!(get_mv_rawpel(-4), -1);
        assert_eq!(get_mv_rawpel(-1), 0);
        // The rounding boundary sits between -3 and -4, not -4 and -5.
        assert_eq!(get_mv_rawpel(-3), 0);
        assert_eq!(get_mv_rawpel(-5), -1);
        assert_eq!(get_mv_rawpel(8), 1);
        assert_eq!(get_mv_rawpel(-8), -1);
    }

    /// TIER 4 — `svt_init_mv_cost_params` (product_coding_loop.c:1901).
    #[test]
    fn tier4_init_mv_cost_params_thresholds() {
        let p = init_mv_cost_params(Mv { x: 8, y: -16 }, 32, 3, 1 << 10, 100, false, None);
        assert_eq!(p.mv_cost_type, MvCostType::Opt);
        assert_eq!(p.early_exit_th, 1020 - 8);
        assert_eq!(p.error_per_bit, (1 << 10) >> RD_EPB_SHIFT);
        assert_eq!(p.full_ref_mv, get_fullmv_from_mv(Mv { x: 8, y: -16 }));

        let p = init_mv_cost_params(Mv::ZERO, 8, 2, 0, 0, false, None);
        assert_eq!(p.mv_cost_type, MvCostType::Entropy);
        // AOMMAX(rdmult >> 6, 1) floors at 1, never 0.
        assert_eq!(p.error_per_bit, 1);
    }

    /// TIER 4 — the three L1 arms of `svt_mv_err_cost`. `MIDRES`'s lambda
    /// is 0, so that arm is free; a port that "fixed" it to 1 would
    /// change every mid-resolution MD search.
    #[test]
    fn tier4_mv_err_cost_l1_arms() {
        let mk = |t: MvCostType| MvCostParams {
            ref_mv: Mv::ZERO,
            full_ref_mv: Mv::ZERO,
            mv_cost_type: t,
            mv_cost_tables: None,
            error_per_bit: 64,
            early_exit_th: 0,
            sad_per_bit: 0,
        };
        let mv = Mv { x: 40, y: -24 };
        assert_eq!(mv_err_cost(mv, &mk(MvCostType::L1LowRes)), (2 * 64) >> 3);
        assert_eq!(mv_err_cost(mv, &mk(MvCostType::L1MidRes)), 0);
        assert_eq!(mv_err_cost(mv, &mk(MvCostType::L1HdRes)), 64 >> 3);
        assert_eq!(mv_err_cost(mv, &mk(MvCostType::None)), 0);
        // A NULL mvcost table is C's `return 0`, not a panic.
        assert_eq!(mv_err_cost(mv, &mk(MvCostType::Entropy)), 0);
    }
}
