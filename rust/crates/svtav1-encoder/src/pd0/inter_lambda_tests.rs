use super::*;
use crate::port_rc_process::FrameUpdateType as U;

/// C's OWN `full_sb_lambda_md[EB_8_BIT_MD]`, read off the `lambda`
/// argument of `svt_aom_full_cost_pd0` through the `SVT_PD0COST_OUT`
/// interposer on `diag 64x64 q40 p8 frames=2` (evidence tier 2 — the real
/// encoder, not a transcription):
///
/// ```text
/// PD0COST org=(0,0) 64x64 dist=64712 ybits=1519 cost=9142574 lambda=241378
/// ```
///
/// with `base_q_idx = 160` on that frame (`tools/fh_fields.py --index 1`).
/// The KEY frame of the same cell is `base_q_idx = 67`, `lambda=18500`,
/// which the KF builder already reproduced.
#[test]
fn the_low_delay_p_inter_lambda_matches_cs_measured_value() {
    // The two selectors C actually uses on that frame. A flat low-delay
    // GOP's frame 1 is `temporal_layer_index == 0`, so the
    // LAMBDA_MOD_INTRA arm does not fire — the 128 identity.
    assert_eq!(
        inter_full_lambda_8bit(160, U::LfUpdate, U::ArfUpdate, false, 0, 128, 150),
        241_378
    );
    // The KEY frame, for the same cell, from the same dump.
    assert_eq!(kf_full_lambda_8bit_lw(67, 150), 18_500);
}

/// The LAMBDA_MOD_INTRA arm (`lambda_mod_intra == 138`) pinned to a
/// measured C value: `diag 64x64 q40 p8 hier=3`, picture poc=4
/// (`tl=1` → the INTNL_ARF factor row, `ref_intra_percentage` below the
/// 50 threshold on that content, `stats_based_sb_lambda_modulation` on at
/// M8). C's trellis `rdmult` for that frame is 206 584 — recovered from
/// `av1_optimize_b`'s RDCOST on identical rate/distortion inputs — which
/// is `(51646 * 16 + 2) >> 2` under `rdoq_rdmult_full`'s video/inter/luma
/// weight. Without the arm the port computed 47 905 / 191 620 and kept a
/// marginal coefficient C dropped (`eob` 7 vs 6).
#[test]
fn the_lambda_mod_intra_arm_matches_cs_measured_value() {
    assert_eq!(
        inter_full_lambda_8bit(106, U::LfUpdate, U::IntnlArfUpdate, false, 0, 138, 150),
        51_646
    );
    // Same frame with the arm NOT taken — the value the port computed
    // before the arm was wired.
    assert_eq!(
        inter_full_lambda_8bit(106, U::LfUpdate, U::IntnlArfUpdate, false, 0, 128, 150),
        47_905
    );
}

/// **The port already HAD a correct transcription, and this function is a
/// SECOND one that diverged from it.**
/// `port_rc_process::compute_rd_mult` takes `LambdaContext::update_type`
/// for the base and lets `update_lambda` derive its own `gf_update_type`
/// for the factor — exactly C's split — and has done since it was ported.
/// `inter_full_lambda_8bit` re-transcribed the same C chain and collapsed
/// the two, which is the failure mode a duplicate transcription always
/// has. This test PINS THEM TOGETHER over a sweep so they cannot diverge
/// again: the only thing this function adds is the `lambda_weight`
/// multiply `av1_lambda_assign_md` (md_process.c:747) applies afterwards.
#[test]
fn it_agrees_with_port_rc_process_compute_rd_mult_over_a_sweep() {
    use crate::port_rc_process::{LambdaContext, compute_rd_mult};
    for &qindex in &[0u8, 1, 20, 67, 100, 160, 200, 255] {
        for &(base, factor_tl) in &[
            (U::LfUpdate, 0u8),
            (U::ArfUpdate, 0),
            (U::GfUpdate, 0),
            (U::IntnlArfUpdate, 0),
            (U::LfUpdate, 3),
        ] {
            for &lw in &[0u32, 128, 150, 175] {
                // Every qdiff that selects a DIFFERENT factor, plus both
                // boundaries of each threshold. Sweeping this axis is what
                // lets the test SEE the wrong-arm transcription corrected
                // on 2026-09-02: the `delta_q_present` arm and the one an
                // inter frame takes agree at qdiff 0 and at no other point
                // in this list, so a revert fails here.
                for &qd in &[0i32, -3, -4, -5, -9, 3, 4, 5, 9] {
                    let ctx = LambdaContext {
                        frame_type: 1, // not KEY_FRAME
                        temporal_layer_index: factor_tl,
                        hierarchical_levels: 3,
                        update_type: base,
                        alt_lambda_factors: false,
                        rtc: false,
                        stats_based_sb_lambda_modulation: true,
                        base_q_idx: i32::from(qindex),
                        delta_q_present: false,
                        r0_delta_qp_md: false,
                        lambda_scale_factors: [128; 7],
                    };
                    let me_q = (i32::from(qindex) + qd).clamp(0, 255) as u8;
                    let qd = i32::from(me_q) - i32::from(qindex);
                    let rc = compute_rd_mult(&ctx, qindex, me_q, 8);
                    let want = if lw == 0 {
                        rc
                    } else {
                        ((u64::from(rc) * u64::from(lw)) >> 7) as u32
                    };
                    let factor = crate::port_rc_process::lambda_gf_update_type(
                        false,
                        ctx.hierarchical_levels,
                        factor_tl,
                    );
                    assert_eq!(
                        inter_full_lambda_8bit(qindex, base, factor, false, qd, 128, lw),
                        want,
                        "qindex {qindex} base {base:?} tl {factor_tl} lw {lw} qdiff {qd}"
                    );
                }
            }
        }
    }
}

/// The NEGATIVE control: conflating the two update types — which is what
/// the port did until 2026-09-02 — gives a DIFFERENT number, so the test
/// above cannot pass with the split reverted.
#[test]
fn one_update_type_for_both_halves_does_not_reproduce_c() {
    // ARF for both: the value `docs/INTER-ENCODE-PLAN.md` §1y recorded.
    assert_eq!(
        inter_full_lambda_8bit(160, U::ArfUpdate, U::ArfUpdate, false, 0, 128, 150),
        244_792
    );
    // LF for both: factor 180 instead of 150.
    assert_eq!(
        inter_full_lambda_8bit(160, U::LfUpdate, U::LfUpdate, false, 0, 128, 150),
        289_654
    );
    // And the KF chain at the same qindex, which is what a caller that
    // forgot the frame type entirely would produce.
    assert_eq!(kf_full_lambda_8bit_lw(160, 150), 248_207);
}
