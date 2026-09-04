//! The MD-level motion-search cost model and the PME SAD kernel —
//! `Source/Lib/Codec/product_coding_loop.c` + the `mcomp.c` cost helpers
//! it drives.
//!
//! | this module | C |
//! |---|---|
//! | [`MvCostType`] / [`MvCostParams`] | `mcomp.h:29-49` — re-exported from [`crate::md_subpel`], the one transcription |
//! | [`mv_err_cost`] | `mcomp.c:42-72` (`svt_mv_err_cost`) — a forward to [`crate::md_subpel::mv_err_cost`] |
//! | [`fp_mv_err_cost`] | `mcomp.c:775-777` (`svt_aom_fp_mv_err_cost`) — the same forward |
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
// Cost model (mcomp.h:29-49, mcomp.c:30-81) — ONE transcription, which lives
// in `crate::md_subpel`
// ---------------------------------------------------------------------------
//
// This module used to carry its own `MvCostType`, `MvCostParams`,
// `MvCostTable` and a second body of `svt_mv_err_cost`, pinned to
// `md_subpel`'s by a 576-cell sweep. `docs/WORKING-ON-THIS.md` §4: two
// transcriptions of one C function drift. As of 2026-09-04 the mcomp.c cost
// model has exactly one body, [`crate::md_subpel::mv_err_cost`], and every
// name below is a re-export of, or a one-line forward to, that one. The
// table type is [`crate::intrabc::MvCostTables`], built by the single
// transcription of `svt_av1_build_nmv_cost_table`
// ([`crate::intrabc::build_nmv_cost_table`]).

pub use crate::intrabc::{MV_MAX, MV_VALS};
pub use crate::md_subpel::{MvCostParams, MvCostType};

/// C's `mvjcost` + `mvcost[2]` triple as `svt_mv_cost` (mcomp.h:138-142)
/// reads them — the same type the IntraBC and sub-pel searches use.
///
/// C clips the component index to `CLIP3(MV_LOW, MV_UPP, v)` = `±16384` and
/// then reads `mvcost[i][MV_MAX + v]`, which at exactly `+16384` is one past
/// a `MV_VALS`-long row and at `-16384` one before it — out of bounds in C
/// either way, and unreachable: `is_valid_mv_diff` (mode_decision.c:776)
/// rejects any per-component diff past `1 << 14` before injection. The port
/// clamps to the populated `±MV_MAX` instead
/// ([`crate::intrabc::MvComponentCost::cost`]). The retired `port_md` table
/// type mapped `-16384` onto the `+16383` entry, so the two copies only ever
/// agreed there when the sign CDF was flat — one more reason for one type.
pub type MvCostTable = crate::intrabc::MvCostTables;

/// C `RD_EPB_SHIFT` (restoration.h:342).
const RD_EPB_SHIFT: u32 = crate::intrabc::RD_EPB_SHIFT;

/// C `svt_mv_err_cost` (mcomp.c:42-72) reached through a [`MvCostParams`]
/// — a forward to the one body, [`crate::md_subpel::mv_err_cost`].
#[inline]
pub fn mv_err_cost(mv: Mv, params: &MvCostParams<'_>) -> i32 {
    params.err_cost(mv)
}

/// C `svt_aom_fp_mv_err_cost` (mcomp.c:775-777, EXPORTED) — the same forward
/// under the name C exports; gated against the real symbol in
/// `tests/c_parity_md_pme.rs` and `tests/c_parity_md_subpel.rs`.
#[inline]
pub fn fp_mv_err_cost(mv: Mv, params: &MvCostParams<'_>) -> i32 {
    params.err_cost(mv)
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
/// `static`) — **tier 4**. The ONE transcription: the MD sub-pel driver
/// (`port_md::md_search`) and the full-pel ME cost
/// (`inter_search_arm::full_pel_mv_cost_params`) both build their params
/// here instead of re-deriving them.
///
/// `skip_diag_refinement` is **the ME controls'** value
/// (`ctx->md_subpel_me_ctrls.skip_diag_refinement >= 3`, :1906) whichever
/// search is being set up — a PME call still takes its cost TYPE from the
/// ME controls. `>= 3` picks `MV_COST_OPT`, everything else
/// `MV_COST_ENTROPY`; that single comparison is what decides whether the
/// whole MD search prices MVs by entropy or by a scaled L1 norm.
///
/// C also stores `full_ref_mv` and `sad_per_bit` (the latter from
/// `svt_aom_get_sad_per_bit(base_q_idx, hbd_md)`); no function in mcomp.c
/// reads either, so [`MvCostParams`] does not carry them and this takes
/// neither `base_q_idx` nor `hbd_md`.
pub fn init_mv_cost_params<'a>(
    ref_mv: Mv,
    sq_size: u16,
    me_skip_diag_refinement: u8,
    rdmult: u32,
    tables: Option<&'a MvCostTable>,
) -> MvCostParams<'a> {
    MvCostParams {
        ref_mv,
        mv_cost_type: if me_skip_diag_refinement >= 3 {
            MvCostType::Opt
        } else {
            MvCostType::Entropy
        },
        tables,
        // C `AOMMAX(rdmult >> RD_EPB_SHIFT, 1)`.
        error_per_bit: ((rdmult >> RD_EPB_SHIFT).max(1)) as i32,
        // C `1020 - (ctx->blk_geom->sq_size >> 2)`.
        early_exit_th: 1020 - i32::from(sq_size >> 2),
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
        let p = init_mv_cost_params(Mv { x: 8, y: -16 }, 32, 3, 1 << 10, None);
        assert_eq!(p.mv_cost_type, MvCostType::Opt);
        assert_eq!(p.early_exit_th, 1020 - 8);
        assert_eq!(p.error_per_bit, (1 << 10) >> RD_EPB_SHIFT);

        let p = init_mv_cost_params(Mv::ZERO, 8, 2, 0, None);
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
            mv_cost_type: t,
            tables: None,
            error_per_bit: 64,
            early_exit_th: 0,
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
