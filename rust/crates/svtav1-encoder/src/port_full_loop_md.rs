//! The MD-side decisions of `Source/Lib/Codec/full_loop.c` that had no
//! counterpart — coefficient shaving, the chroma rate-estimation shortcut,
//! and the recon gate.
//!
//! # Coverage — 4 of the 34 rows the inventory lists for `full_loop.c`, and
//! what the other 30 are
//!
//! | C function | line | here |
//! |---|---|---|
//! | `ec_shave_est_zero_rate_save` | 1387 | [`shave_est_zero_rate_save`] |
//! | `shave_coeff` | 1395 | [`shave_coeff`] |
//! | `skip_chroma_rate_est` | 1942 | [`skip_chroma_rate_est`] |
//! | `svt_aom_do_md_recon` | 2739 | [`do_md_recon`] |
//!
//! **Already ported elsewhere** — checked before being dropped from the
//! queue, not assumed:
//!
//! | C function | where |
//! |---|---|
//! | `get_dqv` (:741) | `crate::qm::dqv_qm` |
//! | `get_golomb_cost` (:613) | `crate::quant::golomb_cost` |
//! | `get_br_cost` / `_with_diff` | `crate::quant::{br_cost, br_cost_with_diff}` |
//! | `get_coeff_cost_general` / `_eob` / `get_two_coeff_cost_simple` | `crate::quant::{coeff_cost_general, coeff_cost_eob, two_coeff_cost_simple}` |
//! | `get_qc_dqc_low` | `crate::quant::qc_dqc_low` |
//! | `get_lower_levels_ctx_general` | `crate::entropy::coeff_c::lower_levels_ctx_general` |
//! | `aom_av1_get_adjusted_tx_size` | `crate::entropy::coeff_c::adjusted_tx_size` |
//! | `svt_av1_compute_cul_level_c` | `crate::leaf_funnel::tx_pipeline::compute_cul_level` |
//! | `quantize_fp_helper_c` / `svt_av1_quantize_fp{,_32x32,_64x64}_c` / `_facade` | `crate::quant::quantize_fp` |
//! | `highbd_quantize_fp_helper_c` / `svt_av1_highbd_quantize_fp{,_facade}_c` | `crate::quant::quantize_fp_hbd` |
//! | `svt_aom_quantize_b_c` / `av1_quantize_b_facade_ii` | `crate::quant::quantize_b` |
//! | `svt_aom_highbd_quantize_b_c` / `svt_av1_highbd_quantize_b_facade` | `crate::quant::quantize_b_hbd` |
//! | `svt_av1_quantize_fp_qm_c` / `svt_av1_highbd_quantize_fp_qm_c` | `crate::qm::quantize_fp_qm` |
//! | `update_coeff_eob_fast` / `svt_fast_optimize_b` | `crate::port_full_loop` |
//! | `svt_av1_optimize_b` | `crate::quant::optimize_b` |
//!
//! **STILL MISSING from `full_loop.c`, named rather than left implicit** —
//! these are the four buffer-plumbing drivers, and they are a real gap, not a
//! "the port replaces this by design" exemption: each carries decision logic
//! (which components to transform, when to skip the chroma tx entirely, the
//! qindex/segment/QM selection) on top of its buffer walk.
//!
//! * `svt_aom_quantize_inv_quantize` (:1649) — the full dispatcher. The
//!   port has its still-picture arm as `crate::quant::quantize_inv_quantize_still`;
//!   the qindex derivation (delta-q vs base, the segment offset, the chroma
//!   `delta_q_dc` offset) and the inter/`is_encode_pass` arms are unported.
//! * `svt_aom_quantize_inv_quantize_light` (:1263) — the light-PD1 arm.
//! * `svt_aom_inv_transform_recon_wrapper` (:1909).
//! * `svt_aom_full_loop_uv` (:2194) and
//!   `svt_aom_full_loop_chroma_light_pd1` (:1974).
//!
//! # Reachability — measured against the C signal derivation, not assumed
//!
//! **Coefficient shaving is RTC-only.** `pcs->coeff_shaving_level` is set to
//! 0 unconditionally on the default and the second path
//! (`enc_mode_config.c:8940` and `:9915`) and takes the value 1 in exactly
//! one place — `:9550-9552`, in the low-delay RTC derivation, and only when
//! `pcs->rdoq_level == 0`, i.e. above M10 for a non-I slice. So
//! [`shave_coeff`] cannot fire on the still / allintra path this port
//! currently reaches, and `svt_aom_quantize_inv_quantize`'s call site
//! (full_loop.c:1897) is additionally gated on `component_type ==
//! COMPONENT_LUMA` and `eob > 1`.
//!
//! Per `docs/WORKING-ON-THIS.md` §7 the translation stays and the
//! reachability is written down rather than the code being called dead: the
//! RTC path is in this port's roadmap, and upstream can widen the gate in one
//! commit.
//!
//! # Evidence
//!
//! **Tier 1** for [`do_md_recon`] — `svt_aom_do_md_recon` is EXPORTED (`nm -g`
//! prints `T`) and `tests/c_parity_full_loop_md.rs` drives it over the
//! 2^7 combinations of its seven predicates.
//!
//! **Tier 4** (hand-derived vectors traced against the C source) for the
//! other three, and the reason is the same for all of them: their only
//! exported caller is `svt_aom_quantize_inv_quantize`, which needs
//! `pcs->scs->enc_ctx->quants_8bit`, `deq_8bit`, the `gqmatrix`/`giqmatrix`
//! pointer graph and a completed forward transform built in the shim before
//! the call even reaches the code under test. A shim that assembles all of
//! that is a larger, less-verified artifact than the twenty lines it would
//! gate, and `docs/WORKING-ON-THIS.md` §4 says to say tier 4 rather than
//! dress it up.

use crate::quant::TX_SCALE_TAB;

/// C `AV1_PROB_COST_SHIFT` / `RDDIV_BITS` (rd_cost.h:34-36).
const AV1_PROB_COST_SHIFT: u32 = 9;
const RDDIV_BITS: u32 = 7;

/// C `av1_cost_literal(1)`.
const BIT_COST: i32 = 1 << AV1_PROB_COST_SHIFT;

/// C `get_coeff_dist` (full_loop.c:735).
#[inline]
fn coeff_dist(tcoeff: i32, dqcoeff: i32, shift: u32) -> i64 {
    let d = (i64::from(tcoeff) - i64::from(dqcoeff)) * (1i64 << shift);
    d * d
}

/// C `ctx->coeff_shaving_ctrls` (md_process.h:443).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoeffShavingCtrls {
    /// C `enabled`.
    pub enabled: bool,
    /// C `level_threshold` — the largest `|level|` eligible for removal.
    pub level_threshold: i32,
    /// C `zero_gap_threshold` — the shortest run of zeros that justifies
    /// retracting the trailing coefficient.
    pub zero_gap_threshold: i32,
    /// C `rd_zero_strength`, 0..=10. Zero disables phase 2 entirely.
    pub rd_zero_strength: i32,
}

/// C `ec_shave_est_zero_rate_save` (full_loop.c:1387).
///
/// The symbol-count knees of the coefficient alphabet: the base-range
/// escape fires once per level of 4, 7, 10 and 13, and above 14 the golomb
/// tail costs extra. Note the thresholds are `>`, so a level of exactly 3
/// saves nothing.
#[inline]
pub fn shave_est_zero_rate_save(ref_level: i32, bit_cost: i32) -> i32 {
    let knees = i32::from(ref_level > 3)
        + i32::from(ref_level > 6)
        + i32::from(ref_level > 9)
        + i32::from(ref_level > 12);
    let mut save = knees * bit_cost;
    if ref_level > 14 {
        save += crate::quant::golomb_cost(ref_level);
    }
    save
}

/// C `shave_coeff` (full_loop.c:1395): retract the EOB by dropping trailing
/// low-magnitude coefficients.
///
/// Two phases, and C's comment says why the order matters: a cheap
/// structural pass first (gap and level only, no RD arithmetic) shortens the
/// tail, then the RD-gated pass runs on what is left.
///
/// Three details that are easy to lose:
///
/// * **phase 1 RETURNS on an ineligible trailing coefficient, phase 2
///   BREAKS.** C returns early from phase 1 (`abs_val > level_th` ->
///   `return`), skipping phase 2 altogether; phase 2's identical test only
///   leaves the loop. So a block whose last coefficient is large is never
///   RD-shaved, even if the one behind it would qualify.
/// * the `level_threshold == 1` fast path is not merely an optimization —
///   `ec_shave_est_zero_rate_save(1, ..)` is 0, so its rate saving is
///   `bit_cost * strength` where the generic path's is
///   `(save + bit_cost) * strength`. The two agree at level 1 and the fast
///   path exists to skip the call.
/// * the comparison is `dist_term >= rate_term`, so a TIE keeps the
///   coefficient.
///
/// Returns the new eob and zeroes the retracted positions of both buffers,
/// as C does.
#[allow(clippy::too_many_arguments)]
pub fn shave_coeff(
    quant_buf: &mut [i32],
    recon_buf: &mut [i32],
    tcoeff: &[i32],
    eob: u16,
    tx_size: usize,
    tx_type: usize,
    lambda: u32,
    ctrls: &CoeffShavingCtrls,
) -> u16 {
    let scan = crate::entropy::scan_tables::scan(
        tx_size,
        crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let level_th = ctrls.level_threshold;
    let gap_th = ctrls.zero_gap_threshold;
    let mut updated_eob = i32::from(eob);
    let mut prev_nz_scan_idx = updated_eob - 2;

    // Phase 1 — structural: retract a trailing coefficient that is small AND
    // separated from the previous non-zero by at least `gap_th` zeros.
    while updated_eob > 1 {
        let last_scan_idx = updated_eob - 1;
        let last_pos = scan[last_scan_idx as usize] as usize;
        if quant_buf[last_pos].abs() > level_th {
            // C RETURNS here — phase 2 never runs. See the doc above.
            return updated_eob as u16;
        }
        while prev_nz_scan_idx >= 0 && quant_buf[scan[prev_nz_scan_idx as usize] as usize] == 0 {
            prev_nz_scan_idx -= 1;
        }
        if prev_nz_scan_idx < 0 {
            break;
        }
        let gap = last_scan_idx - prev_nz_scan_idx - 1;
        if gap < gap_th {
            break;
        }
        quant_buf[last_pos] = 0;
        recon_buf[last_pos] = 0;
        updated_eob = prev_nz_scan_idx + 1;
        prev_nz_scan_idx -= 1;
    }

    if ctrls.rd_zero_strength <= 0 || updated_eob <= 1 {
        return updated_eob as u16;
    }

    let shift = TX_SCALE_TAB[tx_size] as u32;
    let rd_rate_scale = i64::from(ctrls.rd_zero_strength);

    // Phase 2 — RD-gated. The two arms differ only in the rate saving.
    while updated_eob > 1 {
        let last_scan_idx = updated_eob - 1;
        let last_pos = scan[last_scan_idx as usize] as usize;
        let abs_val = quant_buf[last_pos].abs();

        let rate_save = if level_th == 1 {
            if abs_val > 1 {
                break;
            }
            // `ec_shave_est_zero_rate_save(1, ..)` is 0 by construction.
            i64::from(BIT_COST) * rd_rate_scale
        } else {
            if abs_val > level_th {
                break;
            }
            i64::from(shave_est_zero_rate_save(abs_val, BIT_COST) + BIT_COST) * rd_rate_scale
        };

        let dist_cur = coeff_dist(tcoeff[last_pos], recon_buf[last_pos], shift);
        let dist_new = coeff_dist(tcoeff[last_pos], 0, shift);
        let dist_term = (dist_new - dist_cur) * (1i64 << RDDIV_BITS);
        // C `ROUND_POWER_OF_TWO(rate_save * lambda, AV1_PROB_COST_SHIFT)`.
        let rate_term = (rate_save * i64::from(lambda) + (1 << (AV1_PROB_COST_SHIFT - 1)))
            >> AV1_PROB_COST_SHIFT;
        if dist_term >= rate_term {
            break;
        }

        quant_buf[last_pos] = 0;
        recon_buf[last_pos] = 0;
        let mut next_eob = last_scan_idx;
        while next_eob > 0 && quant_buf[scan[(next_eob - 1) as usize] as usize] == 0 {
            next_eob -= 1;
        }
        updated_eob = next_eob;
    }

    updated_eob as u16
}

/// C `COMPONENT_TYPE` as this function sees it.
pub use crate::port_rd_cost::full_cost::ComponentType;

/// The chroma coefficient-rate estimates `skip_chroma_rate_est` may write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChromaCoeffBits {
    pub cb: u64,
    pub cr: u64,
}

/// C `skip_chroma_rate_est` (full_loop.c:1942): substitute a closed-form
/// estimate for the chroma coefficient rate instead of pricing the txb.
///
/// `None` means C returned `false` — the caller must run the real
/// estimation. `Some` carries the values C wrote through its two out
/// pointers, and ONLY for the components `component_type` covers; the other
/// keeps the caller's previous value, exactly as C's untouched pointer does.
///
/// The level gating is C's and reads backwards on purpose: level **1** is
/// the one that always does full estimation, while 0 and >= 2 approximate.
/// The two approximations differ — `3000 + 500 * eob` for a small eob, and
/// (level 0 only) `1500 + 50 * eob` for a large one — and an eob of zero
/// costs nothing in both.
pub fn skip_chroma_rate_est(
    coeff_rate_est_lvl: u8,
    component_type: ComponentType,
    tx_width_uv: u32,
    tx_height_uv: u32,
    eob_u: u16,
    eob_v: u16,
) -> Option<ChromaCoeffBits> {
    if !(coeff_rate_est_lvl >= 2 || coeff_rate_est_lvl == 0) {
        return None;
    }
    let th = (u64::from(tx_width_uv) * u64::from(tx_height_uv)) >> 6;
    let approx = |eob: u16| -> Option<u64> {
        let e = u64::from(eob);
        if e < th {
            Some(if e != 0 { 3000 + e * 500 } else { 0 })
        } else if coeff_rate_est_lvl == 0 {
            Some(if e != 0 { 1500 + e * 50 } else { 0 })
        } else {
            None
        }
    };

    let mut out = ChromaCoeffBits::default();
    if matches!(
        component_type,
        ComponentType::Chroma | ComponentType::ChromaCb
    ) {
        out.cb = approx(eob_u)?;
    }
    if matches!(
        component_type,
        ComponentType::Chroma | ComponentType::ChromaCr
    ) {
        out.cr = approx(eob_v)?;
    }
    Some(out)
}

/// The seven predicates `svt_aom_do_md_recon` reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MdReconInputs {
    /// C `ctx->bypass_encdec`.
    pub bypass_encdec: bool,
    /// C `ctx->pd_pass == PD_PASS_1`.
    pub pd_pass_1: bool,
    /// C `ctx->skip_intra`.
    pub skip_intra: bool,
    /// C `ctx->inter_intra_comp_ctrls.enabled`.
    pub inter_intra_enabled: bool,
    /// C `pcs->is_ref`.
    pub is_ref: bool,
    /// C `pcs->scs->static_config.recon_enabled`.
    pub recon_enabled: bool,
    /// C `pcs->dlf_ctrls.enabled`.
    pub dlf_enabled: bool,
    /// C `pcs->cdef_search_ctrls.enabled`.
    pub cdef_enabled: bool,
    /// C `pcs->cdef_search_ctrls.use_qp_strength`.
    pub cdef_use_qp_strength: bool,
    /// C `pcs->cdef_search_ctrls.use_reference_cdef_fs`.
    pub cdef_use_reference_fs: bool,
    /// C `pcs->enable_restoration`.
    pub enable_restoration: bool,
    /// C `pcs->compute_psnr`.
    pub compute_psnr: bool,
    /// C `pcs->compute_ssim`.
    pub compute_ssim: bool,
}

/// C `svt_aom_do_md_recon` (full_loop.c:2739, EXPORTED): does MD have to
/// produce reconstructed samples for this block?
///
/// Six independent reasons, OR-ed. Two of them are gated on
/// `pd_pass == PD_PASS_1` and two on a sub-flag of their own feature — the
/// CDEF search only needs recon when it is NOT reusing a qp-derived or a
/// reference frame's filter strength.
pub fn do_md_recon(i: &MdReconInputs) -> bool {
    let encdec_bypass = i.bypass_encdec && i.pd_pass_1;
    let need_for_intra_pred = !i.skip_intra || i.inter_intra_enabled;
    let need_for_ref = (i.is_ref || i.recon_enabled) && encdec_bypass;
    let need_for_dlf_search = i.dlf_enabled;
    let need_for_cdef_search =
        i.cdef_enabled && !i.cdef_use_qp_strength && !i.cdef_use_reference_fs;
    let need_for_restoration_search = i.enable_restoration;
    let need_for_quality = (i.compute_psnr || i.compute_ssim) && i.pd_pass_1;

    need_for_intra_pred
        || need_for_ref
        || need_for_dlf_search
        || need_for_cdef_search
        || need_for_restoration_search
        || need_for_quality
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TIER 4 (hand-derived, traced against full_loop.c:1387). The knees are
    /// STRICT `>` tests, so 3/6/9/12 themselves save nothing, and the golomb
    /// tail starts above 14 — not at 14.
    #[test]
    fn shave_rate_save_knees_are_strict() {
        assert_eq!(shave_est_zero_rate_save(1, 512), 0);
        assert_eq!(shave_est_zero_rate_save(3, 512), 0);
        assert_eq!(shave_est_zero_rate_save(4, 512), 512);
        assert_eq!(shave_est_zero_rate_save(6, 512), 512);
        assert_eq!(shave_est_zero_rate_save(7, 512), 1024);
        assert_eq!(shave_est_zero_rate_save(10, 512), 1536);
        assert_eq!(shave_est_zero_rate_save(13, 512), 2048);
        // 14 is still four knees and NO golomb term.
        assert_eq!(shave_est_zero_rate_save(14, 512), 2048);
        // 15 adds `get_golomb_cost(15)`, which is
        // av1_cost_literal(2 * (msb(15 - 12 - 2) + 1) - 1) =
        // av1_cost_literal(2 * 1 - 1) = 512.
        assert_eq!(
            shave_est_zero_rate_save(15, 512),
            2048 + crate::quant::golomb_cost(15)
        );
    }

    /// TIER 4. Phase 1 with a gap wide enough to retract the tail, on a 4x4
    /// DCT_DCT block whose scan is the default zig-zag.
    #[test]
    fn shave_phase1_retracts_a_gapped_trailing_one() {
        let ctrls = CoeffShavingCtrls {
            enabled: true,
            level_threshold: 1,
            zero_gap_threshold: 8,
            // Phase 2 off, so this test observes phase 1 alone.
            rd_zero_strength: 0,
        };
        let scan = crate::entropy::scan_tables::scan(
            0,
            crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[0] as usize,
        );
        // Non-zero at scan positions 0 and 12; scan 1..11 are zero, a gap of
        // 11 >= 8, so the trailing one is retracted and the eob falls to 1.
        let mut q = [0i32; 16];
        let mut r = [0i32; 16];
        let t = [0i32; 16];
        q[scan[0] as usize] = 5;
        r[scan[0] as usize] = 5;
        q[scan[12] as usize] = 1;
        r[scan[12] as usize] = 4;
        let eob = shave_coeff(&mut q, &mut r, &t, 13, 0, 0, 1000, &ctrls);
        assert_eq!(eob, 1);
        assert_eq!(q[scan[12] as usize], 0);
        assert_eq!(r[scan[12] as usize], 0);
    }

    /// TIER 4. A trailing coefficient ABOVE the level threshold makes C
    /// `return` from phase 1, so phase 2 never runs even with a strength
    /// set. That early return (rather than a `break`) is the detail this
    /// pins.
    #[test]
    fn shave_returns_when_the_tail_is_too_large() {
        let ctrls = CoeffShavingCtrls {
            enabled: true,
            level_threshold: 1,
            zero_gap_threshold: 1,
            rd_zero_strength: 10,
        };
        let scan = crate::entropy::scan_tables::scan(
            0,
            crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[0] as usize,
        );
        let mut q = [0i32; 16];
        let mut r = [0i32; 16];
        let t = [0i32; 16];
        q[scan[0] as usize] = 9;
        q[scan[3] as usize] = 7;
        r[scan[3] as usize] = 7;
        let eob = shave_coeff(&mut q, &mut r, &t, 4, 0, 0, 1_000_000, &ctrls);
        assert_eq!(eob, 4);
        assert_eq!(q[scan[3] as usize], 7);
    }

    /// TIER 4. `eob <= 1` and a zero strength both short-circuit phase 2.
    #[test]
    fn shave_leaves_a_single_coefficient_alone() {
        let ctrls = CoeffShavingCtrls {
            enabled: true,
            level_threshold: 1,
            zero_gap_threshold: 1,
            rd_zero_strength: 10,
        };
        let mut q = [0i32; 16];
        let mut r = [0i32; 16];
        let t = [0i32; 16];
        q[0] = 1;
        r[0] = 4;
        assert_eq!(shave_coeff(&mut q, &mut r, &t, 1, 0, 0, 1_000, &ctrls), 1);
        assert_eq!(q[0], 1);
    }

    /// TIER 4 (skip_chroma_rate_est, full_loop.c:1942). Level 1 ALWAYS does
    /// full estimation; 0 and >= 2 approximate, and only level 0 has the
    /// large-eob fallback.
    #[test]
    fn skip_chroma_rate_est_level_gating() {
        // level 1 -> None (caller must do the real estimation).
        assert!(skip_chroma_rate_est(1, ComponentType::Chroma, 8, 8, 0, 0).is_none());

        // 8x8 -> th = 64 >> 6 = 1, so eob 0 is "small" and costs nothing.
        assert_eq!(
            skip_chroma_rate_est(2, ComponentType::Chroma, 8, 8, 0, 0),
            Some(ChromaCoeffBits { cb: 0, cr: 0 })
        );

        // 32x32 -> th = 16. eob 4 < 16 -> 3000 + 4 * 500.
        assert_eq!(
            skip_chroma_rate_est(2, ComponentType::ChromaCb, 32, 32, 4, 99),
            Some(ChromaCoeffBits { cb: 5000, cr: 0 })
        );

        // eob >= th at level >= 2 -> None; at level 0 -> the second formula.
        assert!(skip_chroma_rate_est(2, ComponentType::ChromaCb, 32, 32, 20, 0).is_none());
        assert_eq!(
            skip_chroma_rate_est(0, ComponentType::ChromaCb, 32, 32, 20, 0),
            Some(ChromaCoeffBits {
                cb: 1500 + 20 * 50,
                cr: 0
            })
        );

        // A component the type does not cover is left at zero, not estimated.
        assert_eq!(
            skip_chroma_rate_est(2, ComponentType::ChromaCr, 32, 32, 4, 4),
            Some(ChromaCoeffBits { cb: 0, cr: 5000 })
        );
    }
}
