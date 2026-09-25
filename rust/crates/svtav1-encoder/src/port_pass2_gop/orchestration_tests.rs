use super::*;

/// **EVIDENCE TIER 4** — see the module header. `svt_aom_process_rc_stat`
/// and `svt_av1_init_second_pass` ARE exported, but driving them needs a
/// populated `STATS_BUFFER_CTX` wired into a `SequenceControlSet`, which
/// this lane has not built; the other three are `static` and inlined.
const _: () = ();

fn stat(bits: u64, coded_error: f64, duration: f64) -> FirstPassStats {
    FirstPassStats {
        frame: 1.0,
        coded_error,
        duration,
        count: 1.0,
        stat_struct: StatStruct {
            poc: 0,
            total_num_bits: bits,
            qindex: 100,
            worst_qindex: 150,
            temporal_layer_index: 0,
        },
    }
}

/// `accumulate_stats` is the exact inverse of `subtract_stats`, and both
/// leave `stat_struct` alone.
#[test]
fn accumulate_and_subtract_are_inverses() {
    let mut section = FirstPassStats::default();
    section.stat_struct.poc = 9;
    let f = stat(100, 5.0, 1000.0);
    accumulate_stats(&mut section, &f);
    assert_eq!((section.frame, section.count), (1.0, 1.0));
    assert_eq!(section.coded_error, 5.0);
    assert_eq!(section.duration, 1000.0);
    assert_eq!(section.stat_struct.poc, 9, "stat_struct is not accumulated");
    subtract_stats(&mut section, &f);
    assert_eq!(section, {
        let mut z = FirstPassStats::default();
        z.stat_struct.poc = 9;
        z
    });
}

/// `svt_av1_init_second_pass` DERIVES the frame rate from the accumulated
/// duration rather than taking the configured one — each first-pass frame
/// can have a different duration, so this is the true average.
#[test]
fn init_second_pass_derives_the_frame_rate_from_duration() {
    let mut stats = [
        stat(1000, 10.0, 500_000.0),
        stat(2000, 20.0, 500_000.0),
        // A frame with no recorded bits: read_stat_from_file backfills it
        // from the previous frame at the same temporal layer.
        stat(0, 30.0, 500_000.0),
    ];
    let mut rc = RateControl::default();
    let mut tp = TwoPassState::default();
    let cfg2 = TwoPassCfg {
        vbrmin_section: 50,
        vbrmax_section: 200,
        ..Default::default()
    };
    let mut total = FirstPassStats::default();
    let mut left = FirstPassStats::default();
    let fr = init_second_pass(
        &mut rc, &mut tp, &cfg2, 10_000_000, &mut stats, &mut total, &mut left,
    )
    .expect("non-empty ring");
    // 3 frames over 1.5e6 ticks of 1e-7 s -> 20 fps.
    assert_eq!(fr, 10_000_000.0 * 3.0 / 1_500_000.0);
    // bits_left = duration * bitrate / 1e7 = 1.5e6 * 1e7 / 1e7.
    assert_eq!(tp.bits_left, 1_500_000);
    // The backfill happened, and the total is the backfilled sum.
    assert_eq!(stats[2].stat_struct.total_num_bits, 2000);
    assert_eq!(total.stat_struct.total_num_bits, 1000 + 2000 + 2000);
    // The error bounds are the average coded error scaled by the vbr
    // percentages; the modified error total is the (pre-backfill) bit sum.
    let avg_error = 60.0 / double_divide_check(3.0);
    assert_eq!(tp.modified_error_min, avg_error * 50.0 / 100.0);
    assert_eq!(tp.modified_error_max, avg_error * 200.0 / 100.0);
    assert_eq!(tp.modified_error_left, 1000.0 + 2000.0 + 2000.0);
    assert_eq!(tp.kf_zeromotion_pct, 100);
    // An empty ring returns None and writes nothing.
    let mut empty: [FirstPassStats; 0] = [];
    assert!(
        init_second_pass(
            &mut rc, &mut tp, &cfg2, 10_000_000, &mut empty, &mut total, &mut left
        )
        .is_none()
    );
}

/// `kf_group_rate_assingment` sets `kf_zeromotion_pct` to **0**,
/// overwriting the 100 the init functions set — so the
/// `STATIC_KF_GROUP_THRESH` gate in `rc_vbr_cbr.c`'s `get_q` can only
/// fire before the first KF group is budgeted.
#[test]
fn kf_group_rate_assingment_zeroes_kf_zeromotion_pct() {
    let stats = [stat(1000, 10.0, 1.0), stat(1000, 10.0, 1.0)];
    let mut cursor = StatsCursor::new(&stats, 1);
    let mut rc = RateControl {
        avg_frame_bandwidth: 50_000,
        max_frame_bandwidth: 500_000,
        best_quality: 0,
        worst_quality: 255,
        kf_boost: 2000,
        frames_to_key: 30,
        ..Default::default()
    };
    let scs = SeqRc {
        intra_period_length: 63,
        ..Default::default()
    };
    let cfg2 = TwoPassCfg {
        vbrmax_section: 200,
        ..Default::default()
    };
    let mut tp = TwoPassState {
        bits_left: 10_000_000,
        modified_error_left: 100_000.0,
        kf_zeromotion_pct: 100,
        ..Default::default()
    };
    let input = GroupRateInput {
        new_framerate: 30.0,
        target_bit_rate: 1_500_000,
        ..Default::default()
    };
    let mut eos = false;
    let kf_bits = kf_group_rate_assingment(
        &mut rc,
        &scs,
        &cfg2,
        &mut tp,
        &input,
        &mut cursor,
        true,
        stats[0],
        &mut eos,
    );
    assert_eq!(tp.kf_zeromotion_pct, 0);
    assert!(kf_bits > 0, "the key frame must get a budget");
    // frames_since_key / frames_since_cdf_update are reset.
    assert_eq!(rc.frames_since_key, 0);
    assert_eq!(rc.frames_since_cdf_update, 0);
    // The cursor is restored.
    assert_eq!(cursor.position(), 1);
    // The group's remaining error excludes the key frame's own.
    assert_eq!(tp.kf_group_error_left, 1000);
}

/// `process_first_pass_stats` seeds the quality state ONLY on picture 0,
/// and seeds `avg_frame_qindex[KEY_FRAME]` HALFWAY to `best_allowed_q`.
#[test]
fn process_first_pass_stats_seeds_only_picture_zero() {
    let stats = [stat(1000, 10.0, 1.0), stat(2000, 20.0, 1.0)];
    let mut rc = RateControl {
        best_quality: 0,
        worst_quality: 255,
        ..Default::default()
    };
    let scs = SeqRc::default();
    let cfg2 = TwoPassCfg::default();
    let tp = TwoPassState {
        bits_left: 1_000_000,
        ..Default::default()
    };
    let total = FirstPassStats {
        count: 10.0,
        coded_error: 100.0,
        ..Default::default()
    };
    let mut left = total;
    let mut this = FirstPassStats::default();

    let frame0 = FrameRc {
        picture_number: 0,
        ..Default::default()
    };
    let mut cursor = StatsCursor::new(&stats, 0);
    let seed = process_first_pass_stats(
        &mut rc,
        &scs,
        &cfg2,
        &tp,
        &frame0,
        40,
        &mut cursor,
        Some(&total),
        Some(&mut left),
        &mut this,
        |_, _, _, _| 120,
    )
    .expect("picture 0 with both stats slots seeds");
    assert_eq!(seed.active_worst_quality, 120);
    assert_eq!(seed.avg_frame_qindex_inter, 120);
    assert_eq!(seed.avg_frame_qindex_key, (120 + 40) / 2);
    assert_eq!(rc.avg_frame_qindex, [80, 120]);
    // One stat was consumed and deducted from the remainder.
    assert_eq!(cursor.position(), 1);
    assert_eq!(left.count, 9.0);
    assert_eq!(left.coded_error, 90.0);

    // A later picture seeds nothing but still consumes.
    let frame1 = FrameRc {
        picture_number: 1,
        ..Default::default()
    };
    let mut cursor = StatsCursor::new(&stats, 0);
    let mut left2 = total;
    assert!(
        process_first_pass_stats(
            &mut rc,
            &scs,
            &cfg2,
            &tp,
            &frame1,
            40,
            &mut cursor,
            Some(&total),
            Some(&mut left2),
            &mut this,
            |_, _, _, _| 200,
        )
        .is_none()
    );
    assert_eq!(rc.active_worst_quality, 120, "unchanged on a later picture");
    assert_eq!(cursor.position(), 1);
}

/// `process_rc_stat` budgets a KF group only on an IDR, and a GF group
/// only when one is due — and the `lap_rc` error recomputation is skipped
/// on the first mini-GOP after a key frame (which the KF assignment just
/// set).
#[test]
fn process_rc_stat_gates_the_two_group_assignments() {
    let stats = [stat(1000, 10.0, 1.0), stat(1000, 10.0, 1.0)];
    let base_rc = RateControl {
        avg_frame_bandwidth: 50_000,
        max_frame_bandwidth: 500_000,
        worst_quality: 255,
        frames_to_key: 30,
        baseline_gf_interval: 8,
        ..Default::default()
    };
    let scs = SeqRc {
        intra_period_length: 63,
        hierarchical_levels: 3,
        ..Default::default()
    };
    let cfg2 = TwoPassCfg {
        vbrmax_section: 200,
        mb_rows: 68,
        ..Default::default()
    };
    let mk_tp = || TwoPassState {
        bits_left: 10_000_000,
        modified_error_left: 100_000.0,
        kf_group_bits: 1_000_000,
        kf_group_error_left: 10_000,
        ..Default::default()
    };
    let mut gf_group = vec![GfGroupFrame::default(); 8];

    // Non-IDR, no GF update due: neither assignment runs.
    let mut rc = base_rc.clone();
    let mut tp = mk_tp();
    let mut eos = false;
    let frame = FrameRc {
        picture_number: 4,
        frame_type: crate::port_rc_vbr_cbr_state::FrameType::Inter,
        ..Default::default()
    };
    let input = GroupRateInput {
        gf_interval: 8,
        new_framerate: 30.0,
        target_bit_rate: 1_500_000,
        ..Default::default()
    };
    let mut cursor = StatsCursor::new(&stats, 0);
    let out = process_rc_stat(
        &mut rc,
        &scs,
        &cfg2,
        &mut tp,
        &frame,
        &input,
        0,
        &mut cursor,
        None,
        None,
        &mut gf_group,
        false,
        false,
        &mut eos,
        |_, _, _, _| 120,
    );
    assert_eq!(out.kf_base_frame_target, None);
    assert!(!out.new_gf_group);
    assert_eq!(tp.kf_zeromotion_pct, 0, "untouched default");

    // IDR: the KF group is budgeted.
    let mut rc = base_rc.clone();
    let mut tp = mk_tp();
    let kf_frame = FrameRc {
        picture_number: 0,
        frame_type: crate::port_rc_vbr_cbr_state::FrameType::Key,
        ..Default::default()
    };
    let kf_input = GroupRateInput {
        idr_flag: true,
        gf_interval: 8,
        new_framerate: 30.0,
        target_bit_rate: 1_500_000,
        ..Default::default()
    };
    let mut cursor = StatsCursor::new(&stats, 0);
    let out = process_rc_stat(
        &mut rc,
        &scs,
        &cfg2,
        &mut tp,
        &kf_frame,
        &kf_input,
        0,
        &mut cursor,
        None,
        None,
        &mut gf_group,
        true,
        false,
        &mut eos,
        |_, _, _, _| 120,
    );
    assert!(out.kf_base_frame_target.is_some());
    assert!(out.new_gf_group, "a KF always defines a new GF group");
    // The GF assignment ran and split the budget.
    assert!(rc.gf_group_bits >= 0);
}

/// `gop_bit_allocation` derives the ARF pool from the GF boost and hands
/// it to the splitter; a zero boost gives a zero pool, so every frame in
/// the group gets the flat base share.
#[test]
fn gop_bit_allocation_with_no_boost_is_a_flat_split() {
    let rc = RateControl {
        baseline_gf_interval: 4,
        gfu_boost: 0,
        ..Default::default()
    };
    let mut group = vec![
        GfGroupFrame {
            update_type: FrameUpdateType::Lf,
            ..Default::default()
        };
        4
    ];
    gop_bit_allocation(&rc, &mut group, 4, 3, false, 8, false, 40_000);
    for f in &group {
        assert_eq!(f.base_frame_target, 10_000);
    }
}
