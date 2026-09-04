//! Per-stage candidate counts (NICs) — `svt_aom_set_nics` and
//! `set_md_stage_counts`.
//!
//! | this module | C |
//! |---|---|
//! | [`MD_STAGE_NICS`] | `definitions.h:811-816` |
//! | [`qp_based_th_scaling_factors`] | `enc_mode_config.c:25-53` (EXPORTED) |
//! | [`set_nics`] | `product_coding_loop.c:1358-1391` (EXPORTED) |
//! | [`set_md_stage_counts`] | `product_coding_loop.c:1394-1413` (EXPORTED) |
//!
//! # The wrong constant this replaced (closed 2026-09-04)
//!
//! `leaf_funnel::rate_tables::nic_counts` used to hardcode the **I-slice**
//! row of `MD_STAGE_NICS` (`{64, 0, 0, 64, 64}` reduced to the 64/32/16
//! that class 0 sees) and the `min = 2` rule that only holds for
//! `pic_type < 2`, so every inter frame took I-slice stage counts. It is
//! now a front on [`set_nics`] with the picture type from
//! [`nics_pic_type`] over `FunnelFrame::{non_i_slice, is_highest_layer}`
//! (`crate::port_picstruct::is_highest_layer`, pd_process.c:5560) — ONE
//! transcription, this one, on the live path. Measured byte-inert on the
//! campaign's 96-cell grid (the I-slice row admitted more MDS1 survivors
//! than C's, none of which ever won); recorded in
//! `docs/INTER-ENCODE-PLAN.md` §1z³⁷.
//!
//! # Evidence
//!
//! Tier 1 — `tests/c_parity_md_nics.rs` drives the EXPORTED
//! `svt_aom_set_nics` and `set_md_stage_counts` (both confirmed with
//! `nm -g`; note `set_md_stage_counts` carries NO `svt_aom_` prefix and
//! is nevertheless exported, while `svt_aom_inject_inter_candidates` in
//! the same group carries the prefix and is `static`).

/// C `CAND_CLASS_TOTAL` (definitions.h:793).
pub const CAND_CLASS_TOTAL: usize = 5;
/// C `NICS_PIC_TYPE` (definitions.h:809).
pub const NICS_PIC_TYPE: usize = 3;
/// C `MD_STAGE_NICS_SCAL_DENUM` (definitions.h:817).
pub const MD_STAGE_NICS_SCAL_DENUM: u64 = 16;
/// C `MAX_QP_VALUE` (definitions.h:1662).
pub const MAX_QP_VALUE: u32 = 63;

/// C `MD_STAGE_NICS` (definitions.h:811-816).
///
/// Row 0 is I_SLICE, row 1 REF frames, row 2 NON-REF frames. The I-slice
/// row's zeros in classes 1 and 2 are C's — those classes carry no
/// candidates on an intra frame — and they survive the scaling as the
/// per-stage MINIMUM (1 or 2), never as 0.
pub const MD_STAGE_NICS: [[u32; CAND_CLASS_TOTAL]; NICS_PIC_TYPE] = [
    [64, 0, 0, 64, 64],   // I_SLICE
    [32, 32, 32, 32, 32], // REF frames
    [16, 16, 16, 16, 16], // NON-REF frames
];

/// C `MdStagingMode` (definitions.h:798-803).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MdStagingMode {
    Mode0 = 0,
    Mode1 = 1,
    Mode2 = 2,
}

/// C `NicScalingCtrls` — the three per-stage numerators over a
/// denominator of [`MD_STAGE_NICS_SCAL_DENUM`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NicScalingCtrls {
    pub stage1_scaling_num: u32,
    pub stage2_scaling_num: u32,
    pub stage3_scaling_num: u32,
}

/// C `DIVIDE_AND_ROUND` (utility.h:96): `(x + (y >> 1)) / y`.
#[inline]
fn divide_and_round(x: u64, y: u64) -> u64 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_get_qp_based_th_scaling_factors` (enc_mode_config.c:25-53,
/// EXPORTED).
///
/// Disabled returns `(1, 1)`. Enabled is `(max(10, qp), 63)` below qp 46
/// and `((1.05 - exp(-(max(40, qp) - 35) / 10)) * 10000, 10000)` at or
/// above it. The `max(40, .)` inside the exponent is dead for the branch
/// that reaches it (`qp >= 46` already exceeds 40) but is transcribed
/// because it is what C computes.
pub fn qp_based_th_scaling_factors(enabled: bool, qp: u32) -> (u32, u32) {
    if !enabled {
        return (1, 1);
    }
    if qp >= 46 {
        let ex = -((qp.max(40) as f64) - 35.0) / 10.0;
        let w = (1.05 - ex.exp()) * 10000.0;
        (w as u32, 10000)
    } else {
        (qp.max(10), MAX_QP_VALUE)
    }
}

/// The three per-class stage counts `svt_aom_set_nics` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdStageCounts {
    pub mds1: [u32; CAND_CLASS_TOTAL],
    pub mds2: [u32; CAND_CLASS_TOTAL],
    pub mds3: [u32; CAND_CLASS_TOTAL],
}

/// C `svt_aom_set_nics` (product_coding_loop.c:1358-1391, EXPORTED).
///
/// Two details a paraphrase loses:
///
/// * **The minimum is per stage and depends on `pic_type`.**
///   `min = (pic_type < 2 && stageN_scaling_num) ? 2 : 1`. At the highest
///   temporal layer (`pic_type == 2`) the minimum is 1 even with scaling
///   on.
/// * **The scaling runs TWICE, each with its own clamp to the minimum.**
///   First `* stageN_num / 16`, then `* q_weight / q_weight_denom`, with
///   `MAX(min, .)` applied after each. Folding them into one multiply
///   changes the result whenever the first clamp bites.
pub fn set_nics(
    scaling: &NicScalingCtrls,
    pic_type: u8,
    qp: u32,
    nic_max_qp_based_th_scaling: bool,
) -> MdStageCounts {
    let pt = pic_type as usize;
    let mut out = MdStageCounts::default();
    for cidx in 0..CAND_CLASS_TOTAL {
        out.mds1[cidx] = MD_STAGE_NICS[pt][cidx];
        out.mds2[cidx] = MD_STAGE_NICS[pt][cidx] >> 1;
        out.mds3[cidx] = MD_STAGE_NICS[pt][cidx] >> 2;
    }

    let min_of = |num: u32| -> u64 { if pic_type < 2 && num != 0 { 2 } else { 1 } };
    let min1 = min_of(scaling.stage1_scaling_num);
    let min2 = min_of(scaling.stage2_scaling_num);
    let min3 = min_of(scaling.stage3_scaling_num);

    for cidx in 0..CAND_CLASS_TOTAL {
        out.mds1[cidx] = min1.max(divide_and_round(
            u64::from(out.mds1[cidx]) * u64::from(scaling.stage1_scaling_num),
            MD_STAGE_NICS_SCAL_DENUM,
        )) as u32;
        out.mds2[cidx] = min2.max(divide_and_round(
            u64::from(out.mds2[cidx]) * u64::from(scaling.stage2_scaling_num),
            MD_STAGE_NICS_SCAL_DENUM,
        )) as u32;
        out.mds3[cidx] = min3.max(divide_and_round(
            u64::from(out.mds3[cidx]) * u64::from(scaling.stage3_scaling_num),
            MD_STAGE_NICS_SCAL_DENUM,
        )) as u32;
    }

    let (q_weight, q_weight_denom) = qp_based_th_scaling_factors(nic_max_qp_based_th_scaling, qp);
    for cidx in 0..CAND_CLASS_TOTAL {
        out.mds1[cidx] = min1.max(divide_and_round(
            u64::from(out.mds1[cidx]) * u64::from(q_weight),
            u64::from(q_weight_denom),
        )) as u32;
        out.mds2[cidx] = min2.max(divide_and_round(
            u64::from(out.mds2[cidx]) * u64::from(q_weight),
            u64::from(q_weight_denom),
        )) as u32;
        out.mds3[cidx] = min3.max(divide_and_round(
            u64::from(out.mds3[cidx]) * u64::from(q_weight),
            u64::from(q_weight_denom),
        )) as u32;
    }

    out
}

/// C's `pic_type` derivation from `set_md_stage_counts`
/// (product_coding_loop.c:1398): `I_SLICE ? 0 : !is_highest_layer ? 1 : 2`.
#[inline]
pub fn nics_pic_type(is_i_slice: bool, is_highest_layer: bool) -> u8 {
    if is_i_slice {
        0
    } else if !is_highest_layer {
        1
    } else {
        2
    }
}

/// What [`set_md_stage_counts`] produces: the counts plus the two bypass
/// flags C writes onto the MD context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MdStagePlan {
    pub counts: MdStageCounts,
    /// C `ctx->bypass_md_stage_1`.
    pub bypass_md_stage_1: bool,
    /// C `ctx->bypass_md_stage_2`.
    pub bypass_md_stage_2: bool,
}

/// C `set_md_stage_counts` (product_coding_loop.c:1394-1413, EXPORTED).
///
/// The bypass flags are NOT "staging mode >= N": stage 1 runs for modes 1
/// AND 2, stage 2 only for mode 2, and mode 0 bypasses both.
pub fn set_md_stage_counts(
    scaling: &NicScalingCtrls,
    md_staging_mode: MdStagingMode,
    is_i_slice: bool,
    is_highest_layer: bool,
    qp: u32,
    nic_max_qp_based_th_scaling: bool,
) -> MdStagePlan {
    let pic_type = nics_pic_type(is_i_slice, is_highest_layer);
    let counts = set_nics(scaling, pic_type, qp, nic_max_qp_based_th_scaling);
    MdStagePlan {
        counts,
        bypass_md_stage_1: !matches!(md_staging_mode, MdStagingMode::Mode1 | MdStagingMode::Mode2),
        bypass_md_stage_2: md_staging_mode != MdStagingMode::Mode2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim this module exists to correct, stated as a test rather
    /// than only in prose: an inter frame's class-0 stage-1 count is NOT
    /// the I-slice one.
    #[test]
    fn inter_frames_do_not_get_i_slice_stage_counts() {
        let scaling = NicScalingCtrls {
            stage1_scaling_num: 16,
            stage2_scaling_num: 16,
            stage3_scaling_num: 16,
        };
        let i = set_nics(&scaling, 0, 35, false);
        let refr = set_nics(&scaling, 1, 35, false);
        let nonref = set_nics(&scaling, 2, 35, false);
        assert_eq!(i.mds1[0], 64);
        assert_eq!(refr.mds1[0], 32);
        assert_eq!(nonref.mds1[0], 16);
        // Classes 1 and 2 are zero on an I slice and clamp to the
        // minimum, not to zero.
        assert_eq!(i.mds1[1], 2);
        assert_eq!(refr.mds1[1], 32);
    }

    /// `pic_type == 2` loses the minimum-of-2 rule even with scaling on.
    #[test]
    fn highest_layer_minimum_is_one() {
        let scaling = NicScalingCtrls {
            stage1_scaling_num: 1,
            stage2_scaling_num: 1,
            stage3_scaling_num: 1,
        };
        // 16 * 1 / 16 = 1 for mds1; mds3 base is 16 >> 2 = 4, 4/16 -> 0.
        assert_eq!(set_nics(&scaling, 2, 35, false).mds3[0], 1);
        assert_eq!(set_nics(&scaling, 1, 35, false).mds3[0], 2);
    }

    #[test]
    fn staging_mode_bypass_flags() {
        let s = NicScalingCtrls::default();
        let p = |m| set_md_stage_counts(&s, m, true, false, 35, false);
        assert!(p(MdStagingMode::Mode0).bypass_md_stage_1);
        assert!(p(MdStagingMode::Mode0).bypass_md_stage_2);
        assert!(!p(MdStagingMode::Mode1).bypass_md_stage_1);
        assert!(p(MdStagingMode::Mode1).bypass_md_stage_2);
        assert!(!p(MdStagingMode::Mode2).bypass_md_stage_1);
        assert!(!p(MdStagingMode::Mode2).bypass_md_stage_2);
    }
}
