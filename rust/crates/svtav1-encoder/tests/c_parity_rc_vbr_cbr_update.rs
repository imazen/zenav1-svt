//! Differential parity: the VBR/CBR **post-encode, recode and resize** group
//! (`svtav1-encoder/src/port_rc_vbr_cbr_update.rs`) vs the REAL exported
//! symbols in `Codec/rc_vbr_cbr.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4), no symbol promotion:
//! `svt_av1_rc_postencode_update`, `svt_av1_rc_postencode_update_gop_const`,
//! `recode_loop_update_q` and `svt_aom_dynamic_resize_decision` are all
//! exported. Between them they drive `update_buffer_level`,
//! `av1_rc_update_rate_correction_factors`,
//! `av1_rc_compute_frame_size_bounds`, `recode_loop_test`, `av1_find_qindex`,
//! `av1_get_compression_ratio`, `get_regulated_q_overshoot`/`_undershoot`,
//! `dynamic_resize_one_pass_cbr`, `svt_av1_resize_reset_rc` and
//! `set_gf_interval_update_onepass_rt` — none of which the sweeps could reach
//! before.
//!
//! **NOT covered here, named so it is not mistaken for covered:**
//! * the `AOM_Q && max_bit_rate != 0` arm of `recode_loop_update_q`, which
//!   calls `capped_crf_reencode` in `rc_crf_cqp.c`. Every cell keeps
//!   `max_bit_rate == 0`, so that branch is not entered on either side. The
//!   port takes it as a callback for the same file-boundary reason as the
//!   cyclic-refresh seam in chunk 2.
//! * `svt_aom_one_pass_rt_rate_alloc` and
//!   `svt_av1_rc_process_rate_allocation`'s VBR arm, which need a populated
//!   first-pass stats ring. Tier 4 for now, and their doc comments say so.

use svtav1_cref::rc_vbr_cbr as cref;
use svtav1_encoder::port_rc_process::FrameUpdateType;
use svtav1_encoder::port_rc_vbr_cbr_state as rcs;
use svtav1_encoder::port_rc_vbr_cbr_update as up;

struct Cell {
    rc: rcs::RateControl,
    cfg: rcs::RateControlCfg,
    scs: rcs::SeqRc,
    frame: rcs::FrameRc,
    cr: rcs::CyclicRefresh,
    params: up::RcIntervalParams,
    total_num_bits: u64,
    avg_cnt_zeromv: u64,
    c: cref::RefRcUpdateState,
}

#[allow(clippy::too_many_arguments)]
fn build(
    mode: rcs::AomRcMode,
    bd: u8,
    frame_type: rcs::FrameType,
    update_type: FrameUpdateType,
    showable: bool,
    is_overlay: bool,
    sc: bool,
    tl: u8,
    buffer_level: i64,
    base_q_idx: i32,
    total_num_bits: u64,
    avg_cnt_zeromv: u64,
    avg_low_motion: i32,
    cr_on: bool,
    resize_state: rcs::ResizeState,
    resize_count: i32,
    resize_underflow: i32,
    frames_since_key: i32,
    scene_change: bool,
    rtc: bool,
) -> Cell {
    let rc = rcs::RateControl {
        avg_frame_bandwidth: 83_333,
        prev_avg_frame_bandwidth: 80_000,
        max_frame_bandwidth: 833_330,
        starting_buffer_level: 625_000,
        optimal_buffer_level: 625_000,
        maximum_buffer_size: 900_000,
        buffer_level,
        bits_off_target: buffer_level,
        rate_correction_factors: [0.4, 0.6, 0.8, 1.0, 1.2, 1.4, 1.6],
        avg_frame_qindex: [120, 130],
        last_q: [40, 45],
        worst_quality: 255,
        best_quality: 0,
        active_worst_quality: 180,
        last_boosted_qindex: 96,
        kf_boost: 2300,
        gfu_boost: 2000,
        baseline_gf_interval: 16,
        constrained_gf_group: 0,
        frames_to_key: 40,
        frames_since_key,
        frames_since_cdf_update: 7,
        rolling_target_bits: 70_000,
        rolling_actual_bits: 72_000,
        total_actual_bits: 4_000_000,
        total_target_bits: 4_100_000,
        avg_frame_low_motion: avg_low_motion,
        rc_1_frame: -1,
        rc_2_frame: 1,
        q_1_frame: 130,
        q_2_frame: 128,
        percent_refresh_adjustment: 2,
        rate_ratio_qdelta_adjustment: 0.1,
        resize_state,
        resize_avg_qp: 4_000,
        resize_buffer_underflow: resize_underflow,
        resize_count,
        frame_updated: 6,
        cur_avg_base_me_dist: 5_000,
        prev_avg_base_me_dist: 6_000,
        ..Default::default()
    };
    let cfg = rcs::RateControlCfg {
        mode,
        ..Default::default()
    };
    let scs = rcs::SeqRc {
        encoder_bit_depth: bd,
        intra_period_length: 64,
        hierarchical_levels: 4,
        min_qp_allowed: 1,
        max_qp_allowed: 63,
        super_block_size: 64,
        sb_total_count: 510,
        new_framerate: 30.0,
        max_input_luma_width: 1920,
        max_input_luma_height: 1080,
        rtc,
        recode_tolerance: 25,
        seq_profile: 0,
        ..Default::default()
    };
    let frame = rcs::FrameRc {
        picture_number: 64,
        frame_offset: 64,
        frame_type,
        showable_frame: showable,
        base_q_idx,
        update_type,
        is_overlay,
        is_ref: true,
        sc_class1: sc,
        scene_change_flag: scene_change,
        transition_present: -1,
        temporal_layer_index: tl,
        hierarchical_levels: 4,
        layer_depth: 2,
        frame_width: 1920,
        frame_height: 1080,
        superres_upscaled_width: 1920,
        src_frame_width: 1920,
        src_frame_height: 1080,
        this_frame_target: 83_333,
        projected_frame_size: 0,
        max_frame_size: 100_000,
        top_index: 200,
        bottom_index: 40,
        loop_count: 1,
        b64_total_count: 510,
        r0: 0.25,
        ..Default::default()
    };
    let cr = rcs::CyclicRefresh {
        apply_cyclic_refresh: cr_on,
        actual_num_seg1_sbs: if cr_on { 120 } else { 0 },
        actual_num_seg2_sbs: if cr_on { 60 } else { 0 },
        qindex_delta: if cr_on { [0, -20, -40] } else { [0; 3] },
        max_qdelta_perc: 60,
        percent_refresh: 22,
        rate_ratio_qdelta: 1.5,
        rate_ratio_qdelta_seg2: 2.0,
        rate_boost_fac: 15,
        sb_start: 0,
        sb_end: 100,
    };
    let params = up::RcIntervalParams {
        rolling_target_bits: 60_000,
        rolling_actual_bits: 61_000,
        total_actual_bits: 3_000_000,
        total_target_bits: 3_100_000,
        ..Default::default()
    };

    let c = cref::RefRcUpdateState {
        base: cref::RefRcVbrState {
            avg_frame_bandwidth: rc.avg_frame_bandwidth,
            prev_avg_frame_bandwidth: rc.prev_avg_frame_bandwidth,
            max_frame_bandwidth: rc.max_frame_bandwidth,
            optimal_buffer_level: rc.optimal_buffer_level,
            maximum_buffer_size: rc.maximum_buffer_size,
            buffer_level: rc.buffer_level,
            bits_off_target: rc.bits_off_target,
            rate_correction_factors: rc.rate_correction_factors,
            avg_frame_qindex: rc.avg_frame_qindex,
            worst_quality: rc.worst_quality,
            best_quality: rc.best_quality,
            q_1_frame: rc.q_1_frame,
            q_2_frame: rc.q_2_frame,
            rc_1_frame: rc.rc_1_frame,
            rc_2_frame: rc.rc_2_frame,
            frames_since_key: rc.frames_since_key,
            percent_refresh_adjustment: rc.percent_refresh_adjustment,
            rate_ratio_qdelta_adjustment: rc.rate_ratio_qdelta_adjustment,
            cur_avg_base_me_dist: rc.cur_avg_base_me_dist,
            prev_avg_base_me_dist: rc.prev_avg_base_me_dist,
            frame_updated: rc.frame_updated,
            rc_mode: cfg.mode as i32,
            under_shoot_pct: cfg.under_shoot_pct,
            over_shoot_pct: cfg.over_shoot_pct,
            frame_type: frame.frame_type as i32,
            update_type: frame.update_type as i32,
            is_overlay: i32::from(frame.is_overlay),
            sc_class1: i32::from(frame.sc_class1),
            temporal_layer_index: i32::from(frame.temporal_layer_index),
            hierarchical_levels: i32::from(frame.hierarchical_levels),
            frame_width: frame.frame_width,
            frame_height: frame.frame_height,
            this_frame_target: frame.this_frame_target,
            projected_frame_size: frame.projected_frame_size,
            base_q_idx: frame.base_q_idx,
            b64_total_count: i32::from(frame.b64_total_count),
            apply_cyclic_refresh: i32::from(cr.apply_cyclic_refresh),
            qindex_delta: cr.qindex_delta,
            actual_num_seg1_sbs: cr.actual_num_seg1_sbs,
            actual_num_seg2_sbs: cr.actual_num_seg2_sbs,
            encoder_bit_depth: i32::from(bd),
        },
        last_boosted_qindex: rc.last_boosted_qindex,
        last_q: rc.last_q,
        rolling_target_bits: rc.rolling_target_bits,
        rolling_actual_bits: rc.rolling_actual_bits,
        total_actual_bits: rc.total_actual_bits,
        total_target_bits: rc.total_target_bits,
        avg_frame_low_motion: rc.avg_frame_low_motion,
        constrained_gf_group: rc.constrained_gf_group,
        frames_since_cdf_update: rc.frames_since_cdf_update,
        frames_to_key: rc.frames_to_key,
        this_key_frame_forced: i32::from(rc.this_key_frame_forced),
        active_worst_quality: rc.active_worst_quality,
        kf_boost: rc.kf_boost,
        gfu_boost: rc.gfu_boost,
        baseline_gf_interval: rc.baseline_gf_interval,
        resize_state: rc.resize_state as i32,
        resize_avg_qp: rc.resize_avg_qp,
        resize_buffer_underflow: rc.resize_buffer_underflow,
        resize_count: rc.resize_count,
        picture_number: frame.picture_number,
        frame_offset: frame.frame_offset,
        total_num_bits,
        pcs_total_rate: 40_000_000,
        avg_cnt_zeromv,
        showable_frame: i32::from(frame.showable_frame),
        loop_count: frame.loop_count,
        max_frame_size: frame.max_frame_size,
        src_frame_width: frame.src_frame_width,
        src_frame_height: frame.src_frame_height,
        scene_change_flag: i32::from(frame.scene_change_flag),
        min_qp_allowed: i32::from(scs.min_qp_allowed),
        max_qp_allowed: i32::from(scs.max_qp_allowed),
        recode_loop: 3, // ALLOW_RECODE
        recode_tolerance: scs.recode_tolerance,
        min_cr: 0,
        max_bit_rate: 0,
        rtc: i32::from(scs.rtc),
        max_input_luma_width: scs.max_input_luma_width,
        max_input_luma_height: scs.max_input_luma_height,
        intra_period_length: scs.intra_period_length,
        seq_profile: i32::from(scs.seq_profile),
        new_framerate: scs.new_framerate,
        param_rolling_target_bits: params.rolling_target_bits,
        param_rolling_actual_bits: params.rolling_actual_bits,
        param_total_actual_bits: params.total_actual_bits,
        param_total_target_bits: params.total_target_bits,
        recode_q: base_q_idx,
        recode_q_low: 40,
        recode_q_high: 200,
        recode_undershoot_seen: 0,
        recode_overshoot_seen: 0,
        recode_low_cr_seen: 0,
        recode_loop_again: 0,
        top_index: frame.top_index,
        bottom_index: frame.bottom_index,
        pending_resize_state: 0,
        pending_resize_denom: 8,
        ..Default::default()
    };

    Cell {
        rc,
        cfg,
        scs,
        frame,
        cr,
        params,
        total_num_bits,
        avg_cnt_zeromv,
        c,
    }
}

fn cells() -> Vec<Cell> {
    let mut out = Vec::new();
    for &mode in &[rcs::AomRcMode::Cbr, rcs::AomRcMode::Vbr] {
        for &bd in &[8u8, 10u8] {
            for &(ft, ut) in &[
                (rcs::FrameType::Inter, FrameUpdateType::LfUpdate),
                (rcs::FrameType::Inter, FrameUpdateType::GfUpdate),
                (rcs::FrameType::Inter, FrameUpdateType::ArfUpdate),
                (rcs::FrameType::Inter, FrameUpdateType::IntnlArfUpdate),
                (rcs::FrameType::Key, FrameUpdateType::KfUpdate),
            ] {
                for &showable in &[true, false] {
                    for &overlay in &[false, true] {
                        for &sc in &[false, true] {
                            for &bits in &[20_000u64, 83_000, 400_000, 1_200_000] {
                                for &qi in &[20i32, 96, 160, 240] {
                                    for &lm in &[0i32, 40, 90] {
                                        for &cr_on in &[false, true] {
                                            out.push(build(
                                                mode,
                                                bd,
                                                ft,
                                                ut,
                                                showable,
                                                overlay,
                                                sc,
                                                if ut == FrameUpdateType::LfUpdate {
                                                    2
                                                } else {
                                                    0
                                                },
                                                if bits > 400_000 { 100_000 } else { 625_000 },
                                                qi,
                                                bits,
                                                (bits / 1000).min(100),
                                                lm,
                                                cr_on,
                                                rcs::ResizeState::Orig,
                                                0,
                                                0,
                                                30,
                                                false,
                                                false,
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
    }
    out
}

#[test]
fn postencode_update_matches_c() {
    let mut n = 0usize;
    let mut moved_buffer = 0usize;
    for mut cell in cells() {
        let mut c = cell.c;
        cref::postencode_update(&mut c);

        up::postencode_update(
            &mut cell.rc,
            &cell.cfg,
            &cell.scs,
            &mut cell.frame,
            &cell.cr,
            cell.total_num_bits,
            cell.avg_cnt_zeromv,
        );

        let ctx = format!(
            "mode={:?} bd={} ft={:?} ut={:?} showable={} overlay={} sc={} bits={} qi={} lm={} cr={}",
            cell.cfg.mode,
            cell.scs.encoder_bit_depth,
            cell.frame.frame_type,
            cell.frame.update_type,
            cell.frame.showable_frame,
            cell.frame.is_overlay,
            cell.frame.sc_class1,
            cell.total_num_bits,
            cell.frame.base_q_idx,
            cell.rc.avg_frame_low_motion,
            cell.cr.apply_cyclic_refresh,
        );
        assert_eq!(
            cell.frame.projected_frame_size, c.out_projected_frame_size,
            "projected_frame_size: {ctx}"
        );
        assert_eq!(
            cell.rc.rate_correction_factors, c.base.rate_correction_factors,
            "rate_correction_factors: {ctx}"
        );
        assert_eq!(
            cell.rc.avg_frame_qindex, c.base.avg_frame_qindex,
            "avg_frame_qindex: {ctx}"
        );
        assert_eq!(cell.rc.last_q, c.last_q, "last_q: {ctx}");
        assert_eq!(
            cell.rc.last_boosted_qindex, c.last_boosted_qindex,
            "last_boosted_qindex: {ctx}"
        );
        assert_eq!(
            cell.rc.buffer_level, c.base.buffer_level,
            "buffer_level: {ctx}"
        );
        assert_eq!(
            cell.rc.bits_off_target, c.base.bits_off_target,
            "bits_off_target: {ctx}"
        );
        assert_eq!(
            cell.rc.prev_avg_frame_bandwidth, c.base.avg_frame_bandwidth,
            "prev_avg_frame_bandwidth: {ctx}"
        );
        assert_eq!(
            (cell.rc.rolling_target_bits, cell.rc.rolling_actual_bits),
            (c.rolling_target_bits, c.rolling_actual_bits),
            "rolling bits: {ctx}"
        );
        assert_eq!(
            (cell.rc.total_actual_bits, cell.rc.total_target_bits),
            (c.total_actual_bits, c.total_target_bits),
            "total bits: {ctx}"
        );
        assert_eq!(
            cell.rc.avg_frame_low_motion, c.avg_frame_low_motion,
            "avg_frame_low_motion: {ctx}"
        );
        assert_eq!(
            (cell.rc.frames_since_key, cell.rc.frames_since_cdf_update),
            (c.base.frames_since_key, c.frames_since_cdf_update),
            "frames_since_*: {ctx}"
        );
        assert_eq!(
            (cell.rc.q_1_frame, cell.rc.q_2_frame),
            (c.base.q_1_frame, c.base.q_2_frame),
            "q_1/q_2: {ctx}"
        );
        if cell.rc.buffer_level != 625_000 && cell.rc.buffer_level != 100_000 {
            moved_buffer += 1;
        }
        n += 1;
    }
    assert!(n >= 2000, "sweep collapsed to {n} cells");
    assert!(
        moved_buffer > n / 2,
        "only {moved_buffer} of {n} cells moved the buffer level — the leaky \
         bucket is not being exercised"
    );
}

#[test]
fn postencode_update_gop_const_matches_c() {
    let mut n = 0usize;
    for mut cell in cells() {
        let mut c = cell.c;
        cref::postencode_update_gop_const(&mut c);

        up::postencode_update_gop_const(
            &mut cell.rc,
            &cell.cfg,
            &cell.scs,
            &mut cell.frame,
            &cell.cr,
            &mut cell.params,
            cell.total_num_bits,
        );

        let ctx = format!(
            "mode={:?} ft={:?} ut={:?} bits={} qi={}",
            cell.cfg.mode,
            cell.frame.frame_type,
            cell.frame.update_type,
            cell.total_num_bits,
            cell.frame.base_q_idx,
        );
        assert_eq!(
            (
                cell.params.rolling_target_bits,
                cell.params.rolling_actual_bits
            ),
            (c.param_rolling_target_bits, c.param_rolling_actual_bits),
            "per-GOP rolling bits: {ctx}"
        );
        assert_eq!(
            (cell.params.total_actual_bits, cell.params.total_target_bits),
            (c.param_total_actual_bits, c.param_total_target_bits),
            "per-GOP total bits: {ctx}"
        );
        // The variant that does NOT touch avg_frame_low_motion. If the port
        // updated it here the assert would fire, which is the point.
        assert_eq!(
            cell.rc.avg_frame_low_motion, c.avg_frame_low_motion,
            "avg_frame_low_motion must be untouched by the gop_const variant: {ctx}"
        );
        assert_eq!(cell.rc.rolling_target_bits, c.rolling_target_bits);
        assert_eq!(cell.rc.rolling_actual_bits, c.rolling_actual_bits);
        assert_eq!(
            cell.rc.buffer_level, c.base.buffer_level,
            "buffer_level: {ctx}"
        );
        assert_eq!(
            cell.rc.rate_correction_factors, c.base.rate_correction_factors,
            "rate_correction_factors: {ctx}"
        );
        n += 1;
    }
    assert!(n >= 2000, "sweep collapsed to {n} cells");
}

#[test]
fn recode_loop_update_q_matches_c() {
    let mut n = 0usize;
    let mut looped = 0usize;
    let mut q_moved = std::collections::BTreeSet::new();
    for cell in cells() {
        for &recode_loop in &[
            rcs::RecodeLoop::Disallow,
            rcs::RecodeLoop::AllowKfMaxBw,
            rcs::RecodeLoop::Allow,
        ] {
            for &loop_count in &[0i32, 2, 3] {
                for &min_cr in &[0i32, 200] {
                    // `pcs_total_rate` is in 1/512-bit units; these four
                    // values put the derived projected size far BELOW, just
                    // inside, and far ABOVE the +/-25% recode bracket around
                    // this_frame_target (83 333 bits). Without the outer two
                    // the `looped` control below FAILS — measured, and the
                    // reason the axis exists.
                    for &pcs_total_rate in &[4_000_000u64, 40_000_000, 60_000_000, 400_000_000] {
                        let mut c = cell.c;
                        c.recode_loop = recode_loop as i32;
                        c.loop_count = loop_count;
                        c.min_cr = min_cr;
                        c.pcs_total_rate = pcs_total_rate;
                        cref::recode_loop_update_q(&mut c);

                        let mut rc = cell.rc.clone();
                        let mut frame = cell.frame;
                        frame.loop_count = loop_count;
                        let mut cfg = cell.cfg;
                        cfg.min_cr = min_cr as u32;
                        let mut state = up::RecodeState {
                            q: cell.frame.base_q_idx,
                            q_low: 40,
                            q_high: 200,
                            undershoot_seen: false,
                            overshoot_seen: false,
                            low_cr_seen: false,
                            loop_again: false,
                        };
                        up::recode_loop_update_q(
                            &mut rc,
                            &cfg,
                            &cell.scs,
                            &mut frame,
                            &cell.cr,
                            &mut state,
                            recode_loop,
                            0,
                            pcs_total_rate,
                            cell.frame.top_index,
                            cell.frame.bottom_index,
                            loop_count,
                            |_q| {
                                unreachable!(
                                    "max_bit_rate is 0 in every cell, so capped CRF is off"
                                )
                            },
                        );

                        let ctx = format!(
                            "mode={:?} ft={:?} recode_loop={recode_loop:?} loop_count={loop_count} \
                         min_cr={min_cr} pcs_total_rate={pcs_total_rate} bits={} qi={}",
                            cell.cfg.mode,
                            cell.frame.frame_type,
                            cell.total_num_bits,
                            cell.frame.base_q_idx,
                        );
                        assert_eq!(
                            frame.projected_frame_size, c.out_projected_frame_size,
                            "projected_frame_size: {ctx}"
                        );
                        assert_eq!(state.q, c.recode_q, "q: {ctx}");
                        assert_eq!(state.q_low, c.recode_q_low, "q_low: {ctx}");
                        assert_eq!(state.q_high, c.recode_q_high, "q_high: {ctx}");
                        assert_eq!(
                            i32::from(state.undershoot_seen),
                            c.recode_undershoot_seen,
                            "undershoot_seen: {ctx}"
                        );
                        assert_eq!(
                            i32::from(state.overshoot_seen),
                            c.recode_overshoot_seen,
                            "overshoot_seen: {ctx}"
                        );
                        assert_eq!(
                            i32::from(state.low_cr_seen),
                            c.recode_low_cr_seen,
                            "low_cr_seen: {ctx}"
                        );
                        assert_eq!(
                            i32::from(state.loop_again),
                            c.recode_loop_again,
                            "loop: {ctx}"
                        );
                        assert_eq!(
                            rc.rate_correction_factors, c.base.rate_correction_factors,
                            "rate_correction_factors: {ctx}"
                        );
                        if state.loop_again {
                            looped += 1;
                            q_moved.insert(state.q);
                        }
                        n += 1;
                    }
                }
            }
        }
    }
    assert!(n >= 2000, "sweep collapsed to {n} cells");
    // Anti-vacuity: a recode-loop differential where no cell ever decides to
    // recode would pass with the whole bisection deleted.
    assert!(
        looped > 0,
        "no cell asked for a re-encode — the bisection block is inert here"
    );
    assert!(
        q_moved.len() > 3,
        "only {} distinct recode qindexes over {n} cells",
        q_moved.len()
    );
}

#[test]
fn dynamic_resize_decision_matches_c() {
    let mut n = 0usize;
    let mut transitions = 0usize;
    for &rtc in &[false, true] {
        for &state in &[
            rcs::ResizeState::Orig,
            rcs::ResizeState::ThreeQuarter,
            rcs::ResizeState::OneHalf,
        ] {
            for &count in &[0i32, 60, 90, 180] {
                for &underflow in &[0i32, 10, 50, 120] {
                    for &fsk in &[10i32, 100] {
                        for &scene in &[false, true] {
                            for &buf in &[625_000i64, 100_000, 880_000] {
                                for &avg_qp in &[100i32, 4_000, 20_000] {
                                    let mut cell = build(
                                        rcs::AomRcMode::Cbr,
                                        8,
                                        rcs::FrameType::Inter,
                                        FrameUpdateType::LfUpdate,
                                        true,
                                        false,
                                        false,
                                        0,
                                        buf,
                                        96,
                                        83_000,
                                        50,
                                        60,
                                        false,
                                        state,
                                        count,
                                        underflow,
                                        fsk,
                                        scene,
                                        rtc,
                                    );
                                    cell.rc.resize_avg_qp = avg_qp;
                                    cell.c.resize_avg_qp = avg_qp;

                                    let mut c = cell.c;
                                    cref::dynamic_resize_decision(&mut c);

                                    let mut pending = up::ResizePendingParams {
                                        resize_state: rcs::ResizeState::Orig,
                                        resize_denom: 8,
                                    };
                                    up::dynamic_resize_decision(
                                        &mut cell.rc,
                                        &cell.cfg,
                                        &cell.scs,
                                        &mut cell.frame,
                                        &mut pending,
                                    );

                                    let ctx = format!(
                                        "rtc={rtc} state={state:?} count={count} \
                                         underflow={underflow} fsk={fsk} scene={scene} \
                                         buf={buf} avg_qp={avg_qp}"
                                    );
                                    assert_eq!(
                                        cell.rc.resize_state as i32, c.resize_state,
                                        "resize_state: {ctx}"
                                    );
                                    assert_eq!(
                                        pending.resize_state as i32, c.pending_resize_state,
                                        "pending.resize_state: {ctx}"
                                    );
                                    assert_eq!(
                                        i32::from(pending.resize_denom),
                                        c.pending_resize_denom,
                                        "pending.resize_denom: {ctx}"
                                    );
                                    assert_eq!(
                                        (
                                            cell.rc.resize_avg_qp,
                                            cell.rc.resize_count,
                                            cell.rc.resize_buffer_underflow
                                        ),
                                        (
                                            c.resize_avg_qp,
                                            c.resize_count,
                                            c.resize_buffer_underflow
                                        ),
                                        "resize counters: {ctx}"
                                    );
                                    assert_eq!(
                                        cell.rc.buffer_level, c.base.buffer_level,
                                        "buffer_level: {ctx}"
                                    );
                                    assert_eq!(
                                        cell.rc.rate_correction_factors,
                                        c.base.rate_correction_factors,
                                        "rate_correction_factors: {ctx}"
                                    );
                                    assert_eq!(
                                        cell.frame.this_frame_target, c.out_this_frame_target,
                                        "this_frame_target: {ctx}"
                                    );
                                    if cell.rc.resize_state as i32 != state as i32 {
                                        transitions += 1;
                                    }
                                    n += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(n >= 500, "sweep collapsed to {n} cells");
    // Anti-vacuity: without a real transition the whole step-4 reset (and
    // svt_av1_resize_reset_rc with it) is never entered.
    assert!(
        transitions > 0,
        "no cell changed resize_state — the ladder and its RC reset are inert"
    );
}
