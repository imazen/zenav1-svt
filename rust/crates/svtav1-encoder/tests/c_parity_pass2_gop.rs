//! Differential parity: `pass2_strategy.c`'s two-pass post-encode feedback
//! (`svtav1-encoder/src/port_pass2_gop.rs`) vs the REAL exported symbols.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4), no promotion:
//! `svt_av1_twopass_postencode_update` and
//! `svt_av1_twopass_postencode_update_gop_const`. These are the two functions
//! in the file that run on EVERY frame of a VBR encode — everything else in
//! `pass2_strategy.c` runs once per GF or KF group.
//!
//! **NOT covered here, named rather than implied.** The GOP/KF bit allocation
//! (`gf_group_rate_assingment`, `kf_group_rate_assingment`,
//! `calculate_gf_stats`, `allocate_gf_group_bits`,
//! `calculate_active_worst_quality`, `lap_rc_init`, …) is `static` and inlined
//! away, and its only exported entry points — `svt_aom_process_rc_stat`,
//! `svt_av1_init_second_pass`, `svt_av1_init_single_pass_lap` — need a
//! populated `STATS_BUFFER_CTX` ring wired into a `SequenceControlSet`. That
//! harness does not exist yet, so those functions are evidence tier 4 and the
//! port says so per function.

use svtav1_cref::rc_vbr_cbr as cref;
use svtav1_encoder::port_pass2_gop as p2;
use svtav1_encoder::port_rc_process::FrameUpdateType;
use svtav1_encoder::port_rc_vbr_cbr_state as rcs;
use svtav1_encoder::port_rc_vbr_cbr_update::RcIntervalParams;

struct Cell {
    rc: rcs::RateControl,
    cfg: rcs::RateControlCfg,
    scs: rcs::SeqRc,
    frame: rcs::FrameRc,
    twopass: p2::TwoPassState,
    params: RcIntervalParams,
    c: cref::RefRcUpdateState,
}

#[allow(clippy::too_many_arguments)]
fn build(
    update_type: FrameUpdateType,
    is_overlay: bool,
    is_short_clip: bool,
    layer_depth: i32,
    base_frame_target: i32,
    projected: i32,
    vbr_off: i64,
    vbr_off_fast: i64,
    total_actual: i64,
    total_target: i64,
    rolling_target: i32,
    rolling_actual: i32,
    extend: (i32, i32, i32),
    active_worst: i32,
    under_pct: i32,
    over_pct: i32,
) -> Cell {
    let rc = rcs::RateControl {
        avg_frame_bandwidth: 83_333,
        max_frame_bandwidth: 833_330,
        worst_quality: 255,
        best_quality: 0,
        active_worst_quality: active_worst,
        active_best_quality: [10, 20, 30, 40, 50, 60, 70],
        rolling_target_bits: rolling_target,
        rolling_actual_bits: rolling_actual,
        total_actual_bits: total_actual,
        total_target_bits: total_target,
        vbr_bits_off_target: vbr_off,
        vbr_bits_off_target_fast: vbr_off_fast,
        rate_error_estimate: 0,
        rate_correction_factors: [1.0; 7],
        avg_frame_qindex: [120, 130],
        ..Default::default()
    };
    let cfg = rcs::RateControlCfg {
        mode: rcs::AomRcMode::Vbr,
        under_shoot_pct: under_pct,
        over_shoot_pct: over_pct,
        ..Default::default()
    };
    let scs = rcs::SeqRc {
        encoder_bit_depth: 8,
        is_short_clip,
        ..Default::default()
    };
    let frame = rcs::FrameRc {
        picture_number: 64,
        frame_type: rcs::FrameType::Inter,
        showable_frame: true,
        base_q_idx: 111,
        update_type,
        is_overlay,
        layer_depth,
        base_frame_target,
        projected_frame_size: projected,
        frame_width: 1920,
        frame_height: 1080,
        superres_upscaled_width: 1920,
        this_frame_target: 83_333,
        b64_total_count: 510,
        ..Default::default()
    };
    let twopass = p2::TwoPassState {
        extend_minq: extend.0,
        extend_maxq: extend.1,
        extend_minq_fast: extend.2,
        ..Default::default()
    };
    let params = RcIntervalParams {
        rolling_target_bits: rolling_target,
        rolling_actual_bits: rolling_actual,
        total_actual_bits: total_actual,
        total_target_bits: total_target,
        vbr_bits_off_target: vbr_off,
        vbr_bits_off_target_fast: vbr_off_fast,
        rate_error_estimate: 0,
        extend_minq: extend.0,
        extend_maxq: extend.1,
        extend_minq_fast: extend.2,
        ..Default::default()
    };

    let c = cref::RefRcUpdateState {
        base: cref::RefRcVbrState {
            avg_frame_bandwidth: rc.avg_frame_bandwidth,
            max_frame_bandwidth: rc.max_frame_bandwidth,
            rate_correction_factors: rc.rate_correction_factors,
            avg_frame_qindex: rc.avg_frame_qindex,
            worst_quality: rc.worst_quality,
            best_quality: rc.best_quality,
            rc_mode: cfg.mode as i32,
            under_shoot_pct: cfg.under_shoot_pct,
            over_shoot_pct: cfg.over_shoot_pct,
            frame_type: frame.frame_type as i32,
            update_type: frame.update_type as i32,
            is_overlay: i32::from(frame.is_overlay),
            frame_width: frame.frame_width,
            frame_height: frame.frame_height,
            this_frame_target: frame.this_frame_target,
            projected_frame_size: frame.projected_frame_size,
            base_q_idx: frame.base_q_idx,
            b64_total_count: i32::from(frame.b64_total_count),
            encoder_bit_depth: i32::from(scs.encoder_bit_depth),
            ..Default::default()
        },
        active_worst_quality: rc.active_worst_quality,
        active_best_quality: rc.active_best_quality,
        rolling_target_bits: rc.rolling_target_bits,
        rolling_actual_bits: rc.rolling_actual_bits,
        total_actual_bits: rc.total_actual_bits,
        total_target_bits: rc.total_target_bits,
        vbr_bits_off_target: rc.vbr_bits_off_target,
        vbr_bits_off_target_fast: rc.vbr_bits_off_target_fast,
        rate_error_estimate: rc.rate_error_estimate,
        extend_minq: twopass.extend_minq,
        extend_maxq: twopass.extend_maxq,
        extend_minq_fast: twopass.extend_minq_fast,
        base_frame_target: frame.base_frame_target,
        layer_depth: frame.layer_depth,
        is_short_clip: i32::from(scs.is_short_clip),
        picture_number: frame.picture_number,
        showable_frame: 1,
        min_qp_allowed: i32::from(scs.min_qp_allowed),
        max_qp_allowed: i32::from(scs.max_qp_allowed),
        param_rolling_target_bits: params.rolling_target_bits,
        param_rolling_actual_bits: params.rolling_actual_bits,
        param_total_actual_bits: params.total_actual_bits,
        param_total_target_bits: params.total_target_bits,
        param_vbr_bits_off_target: params.vbr_bits_off_target,
        param_vbr_bits_off_target_fast: params.vbr_bits_off_target_fast,
        param_rate_error_estimate: params.rate_error_estimate,
        param_extend_minq: params.extend_minq,
        param_extend_maxq: params.extend_maxq,
        param_extend_minq_fast: params.extend_minq_fast,
        ..Default::default()
    };

    Cell {
        rc,
        cfg,
        scs,
        frame,
        twopass,
        params,
        c,
    }
}

fn cells() -> Vec<Cell> {
    let mut out = Vec::new();
    for &ut in &[
        FrameUpdateType::LfUpdate,
        FrameUpdateType::GfUpdate,
        FrameUpdateType::ArfUpdate,
        FrameUpdateType::IntnlArfUpdate,
        FrameUpdateType::KfUpdate,
    ] {
        for &overlay in &[false, true] {
            for &short_clip in &[false, true] {
                for &ld in &[0i32, 1, 3, 6] {
                    for &(bft, proj) in &[
                        (83_333i32, 83_000i32),
                        (83_333, 10_000),  // deep undershoot -> fast pool
                        (83_333, 400_000), // extreme local overshoot
                        (200_000, 90_000),
                    ] {
                        for &(vbr_off, vbr_fast) in &[
                            (0i64, 0i64),
                            (900_000, 0),
                            (-900_000, 0),
                            (400_000, 120_000),
                        ] {
                            for &(rt, ra) in &[(70_000i32, 72_000i32), (72_000, 70_000)] {
                                for &aw in &[100i32, 200, 255] {
                                    // The extend_* start values matter: from
                                    // (3,4,1) a single call can never drive
                                    // extend_minq below zero, and the
                                    // short_clip_unwind control below FAILS.
                                    // Starting at 0 lets one decrement land on
                                    // -1, which the gop_const clamp keeps and
                                    // the plain one floors at 0.
                                    for &extend in &[(3i32, 4i32, 1i32), (0, 0, 0), (-10, -20, 0)] {
                                        out.push(build(
                                            ut, overlay, short_clip, ld, bft, proj, vbr_off,
                                            vbr_fast, 4_000_000, 4_100_000, rt, ra, extend, aw, 25,
                                            25,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

#[test]
fn twopass_postencode_update_matches_c() {
    let mut n = 0usize;
    let mut minq_moved = 0usize;
    let mut fast_pool_used = 0usize;
    for mut cell in cells() {
        let mut c = cell.c;
        cref::twopass_postencode_update(&mut c);

        let before_minq = cell.twopass.extend_minq;
        p2::twopass_postencode_update(
            &mut cell.rc,
            &cell.cfg,
            &cell.scs,
            &cell.frame,
            &mut cell.twopass,
            &mut cell.params,
            p2::DriftStateOwner::RateControlAndTwoPass,
        );

        let ctx = format!(
            "ut={:?} overlay={} short={} ld={} bft={} proj={} vbr_off={} vbr_fast={} \
             rolling=({},{}) aw={}",
            cell.frame.update_type,
            cell.frame.is_overlay,
            cell.scs.is_short_clip,
            cell.frame.layer_depth,
            cell.frame.base_frame_target,
            cell.frame.projected_frame_size,
            c.vbr_bits_off_target,
            c.vbr_bits_off_target_fast,
            cell.rc.rolling_target_bits,
            cell.rc.rolling_actual_bits,
            cell.rc.active_worst_quality,
        );
        assert_eq!(
            cell.rc.vbr_bits_off_target, c.vbr_bits_off_target,
            "vbr_bits_off_target: {ctx}"
        );
        assert_eq!(
            cell.rc.vbr_bits_off_target_fast, c.vbr_bits_off_target_fast,
            "vbr_bits_off_target_fast: {ctx}"
        );
        assert_eq!(
            cell.rc.rate_error_estimate, c.rate_error_estimate,
            "rate_error_estimate: {ctx}"
        );
        assert_eq!(
            cell.rc.active_best_quality, c.active_best_quality,
            "active_best_quality pyramid: {ctx}"
        );
        assert_eq!(
            cell.twopass.extend_minq, c.extend_minq,
            "extend_minq: {ctx}"
        );
        assert_eq!(
            cell.twopass.extend_maxq, c.extend_maxq,
            "extend_maxq: {ctx}"
        );
        assert_eq!(
            cell.twopass.extend_minq_fast, c.extend_minq_fast,
            "extend_minq_fast: {ctx}"
        );
        if cell.twopass.extend_minq != before_minq {
            minq_moved += 1;
        }
        if c.vbr_bits_off_target_fast != 0 {
            fast_pool_used += 1;
        }
        n += 1;
    }
    assert!(n >= 2000, "sweep collapsed to {n} cells");
    assert!(
        minq_moved > 0,
        "no cell moved extend_minq — the drift adjustment is inert here"
    );
    assert!(
        fast_pool_used > 0,
        "no cell touched the fast undershoot pool — that whole block is inert"
    );
}

#[test]
fn twopass_postencode_update_gop_const_matches_c() {
    let mut n = 0usize;
    let mut short_clip_unwind = 0usize;
    for mut cell in cells() {
        let mut c = cell.c;
        cref::twopass_postencode_update_gop_const(&mut c);

        p2::twopass_postencode_update(
            &mut cell.rc,
            &cell.cfg,
            &cell.scs,
            &cell.frame,
            &mut cell.twopass,
            &mut cell.params,
            p2::DriftStateOwner::IntervalParams,
        );

        let ctx = format!(
            "ut={:?} overlay={} short={} ld={} bft={} proj={}",
            cell.frame.update_type,
            cell.frame.is_overlay,
            cell.scs.is_short_clip,
            cell.frame.layer_depth,
            cell.frame.base_frame_target,
            cell.frame.projected_frame_size,
        );
        assert_eq!(
            cell.params.vbr_bits_off_target, c.param_vbr_bits_off_target,
            "params.vbr_bits_off_target: {ctx}"
        );
        assert_eq!(
            cell.params.vbr_bits_off_target_fast, c.param_vbr_bits_off_target_fast,
            "params.vbr_bits_off_target_fast: {ctx}"
        );
        assert_eq!(
            cell.params.rate_error_estimate, c.param_rate_error_estimate,
            "params.rate_error_estimate: {ctx}"
        );
        assert_eq!(
            cell.params.extend_minq, c.param_extend_minq,
            "params.extend_minq: {ctx}"
        );
        assert_eq!(
            cell.params.extend_maxq, c.param_extend_maxq,
            "params.extend_maxq: {ctx}"
        );
        assert_eq!(
            cell.params.extend_minq_fast, c.param_extend_minq_fast,
            "params.extend_minq_fast: {ctx}"
        );
        assert_eq!(
            cell.rc.active_best_quality, c.active_best_quality,
            "active_best_quality pyramid: {ctx}"
        );
        // This variant leaves the RATE_CONTROL / TWO_PASS copies alone.
        assert_eq!(
            cell.rc.vbr_bits_off_target, c.vbr_bits_off_target,
            "the gop_const variant must not touch rc->vbr_bits_off_target: {ctx}"
        );
        assert_eq!(
            cell.twopass.extend_minq, c.extend_minq,
            "the gop_const variant must not touch twopass->extend_minq: {ctx}"
        );
        if cell.scs.is_short_clip && cell.params.extend_minq < 0 {
            short_clip_unwind += 1;
        }
        n += 1;
    }
    assert!(n >= 2000, "sweep collapsed to {n} cells");
    // Anti-vacuity for the divergence this variant exists to express: only
    // the gop_const path can drive extend_minq NEGATIVE (its short-clip clamp
    // has a -minq_adj_limit/4 floor where the plain one clamps at 0).
    assert!(
        short_clip_unwind > 0,
        "no short-clip cell drove extend_minq negative — the clamp that \
         DISTINGUISHES this variant from the plain one is never exercised"
    );
}
