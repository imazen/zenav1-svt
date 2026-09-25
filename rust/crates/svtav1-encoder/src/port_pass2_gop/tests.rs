use super::*;

/// **EVIDENCE TIER 4** for every test in this module: hand-derived vectors
/// traced against `Codec/pass2_strategy.c`. These functions are `static`
/// in C and were inlined away by the Release build (`nm` on
/// `pass2_strategy.c.o` shows ONE local symbol, `get_twopass_worst_quality`,
/// which is already covered by `port_pass2_strategy`), so there is no
/// symbol to link and no exported caller that reaches them without a
/// first-pass stats ring. Each vector below states the C line it was
/// traced from.
const _: () = ();

fn stat(bits: u64, coded_error: f64, tl: u8) -> FirstPassStats {
    FirstPassStats {
        frame: 1.0,
        coded_error,
        duration: 10_000.0,
        count: 1.0,
        stat_struct: StatStruct {
            poc: 0,
            total_num_bits: bits,
            qindex: 100,
            worst_qindex: 150,
            temporal_layer_index: tl,
        },
    }
}

/// `DOUBLE_DIVIDE_CHECK` (firstpass.h:23) preserves the divisor's SIGN.
/// `max(x, eps)` would flip a negative divisor's quotient.
#[test]
fn double_divide_check_preserves_sign() {
    assert_eq!(double_divide_check(0.0), 0.000_001);
    assert_eq!(double_divide_check(-0.0), 0.000_001);
    assert_eq!(double_divide_check(-1.0), -1.000_001);
    assert_eq!(double_divide_check(2.0), 2.000_001);
}

/// `init_gf_stats` (pass2_strategy.c:326) is NOT a zero-init: the qindexes
/// start at 172 and `total_num_bits` at 1.
#[test]
fn init_gf_stats_is_not_default() {
    let g = init_gf_stats();
    assert_eq!(g.gf_stat_struct.total_num_bits, 1);
    assert_eq!(g.gf_stat_struct.qindex, 172);
    assert_eq!(g.gf_stat_struct.worst_qindex, 172);
    assert_eq!(g.gf_group_err, 0.0);
    assert_ne!(g, GfGroupStats::default());
}

/// `subtract_stats` (pass2_strategy.c:47) touches four fields and leaves
/// `stat_struct` alone.
#[test]
fn subtract_stats_leaves_stat_struct() {
    let mut section = FirstPassStats {
        frame: 10.0,
        coded_error: 500.0,
        duration: 100.0,
        count: 10.0,
        stat_struct: StatStruct {
            poc: 7,
            total_num_bits: 999,
            qindex: 1,
            worst_qindex: 2,
            temporal_layer_index: 3,
        },
    };
    let frame = stat(4, 50.0, 1);
    subtract_stats(&mut section, &frame);
    assert_eq!(section.frame, 9.0);
    assert_eq!(section.coded_error, 450.0);
    assert_eq!(section.count, 9.0);
    // 100 - 10000: `duration` can legitimately go negative here, because
    // `subtract_stats` is used to draw a frame OUT of an accumulated
    // section whose duration may already have been consumed.
    assert_eq!(section.duration, -9_900.0);
    assert_eq!(section.stat_struct.total_num_bits, 999);
    assert_eq!(section.stat_struct.poc, 7);
}

/// `frame_max_bits` (pass2_strategy.c:55): the `CLIP3` upper bound is
/// `max_frame_bandwidth`, so a huge `vbrmax_section` cannot exceed it.
#[test]
fn frame_max_bits_clips_to_max_frame_bandwidth() {
    let rc = RateControl {
        avg_frame_bandwidth: 100_000,
        max_frame_bandwidth: 250_000,
        ..Default::default()
    };
    assert_eq!(frame_max_bits(&rc, 100), 100_000);
    assert_eq!(frame_max_bits(&rc, 400), 250_000); // 400_000 clipped
    assert_eq!(frame_max_bits(&rc, 0), 0);
}

/// The cursor's two boundary rules (see [`StatsCursor`]): the loops test
/// `<= end` while `input_stats` stops at `>= end`, so a loop body runs
/// once with the cursor ON the end sentinel.
#[test]
fn stats_cursor_boundary_rules() {
    let stats = [stat(10, 1.0, 0), stat(20, 2.0, 1)];
    let mut c = StatsCursor::new(&stats, 0);
    assert!(c.at_or_before_end());
    assert_eq!(
        c.input_stats().map(|s| s.stat_struct.total_num_bits),
        Some(10)
    );
    assert_eq!(
        c.input_stats().map(|s| s.stat_struct.total_num_bits),
        Some(20)
    );
    // pos == len: still "at or before end", but input_stats is EOF.
    assert!(c.at_or_before_end());
    assert_eq!(c.input_stats(), None);
    assert_eq!(c.position(), 2);
    c.reset_fpf_position(1);
    assert_eq!(
        c.input_stats().map(|s| s.stat_struct.total_num_bits),
        Some(20)
    );
    // `previous()` is C's `(stats_in - 1)`; None rather than a read before
    // the ring.
    let c0 = StatsCursor::new(&stats, 0);
    assert!(c0.previous().is_none());
}

/// `calculate_modified_err` (pass2_strategy.c:22) returns the frame's bit
/// count, or 0 when the accumulated `total_stats` slot is absent.
#[test]
fn calculate_modified_err_gates_on_total_stats() {
    let s = stat(1234, 5.0, 0);
    assert_eq!(calculate_modified_err(true, &s), 1234.0);
    assert_eq!(calculate_modified_err(false, &s), 0.0);
}

/// `read_stat_from_file` (pass2_strategy.c:955) carries the last non-zero
/// bit count forward PER TEMPORAL LAYER, not globally.
#[test]
fn read_stat_from_file_carries_forward_per_layer() {
    let mut stats = [
        stat(100, 1.0, 0),
        stat(200, 1.0, 1),
        stat(0, 1.0, 0), // inherits 100, not 200
        stat(0, 1.0, 1), // inherits 200
        stat(0, 1.0, 2), // no predecessor at layer 2 -> 0
    ];
    let total = read_stat_from_file(&mut stats);
    assert_eq!(stats[2].stat_struct.total_num_bits, 100);
    assert_eq!(stats[3].stat_struct.total_num_bits, 200);
    assert_eq!(stats[4].stat_struct.total_num_bits, 0);
    assert_eq!(total, 100 + 200 + 100 + 200);
}

fn arf(layer_depth: i32) -> GfGroupFrame {
    GfGroupFrame {
        update_type: FrameUpdateType::IntnlArfUpdate,
        layer_depth,
        ..Default::default()
    }
}

/// `allocate_gf_group_bits` (pass2_strategy.c:240): the ARF pool is
/// CONSUMED as it descends the pyramid, and the TOP level's fraction is
/// forced to 1.0 regardless of `layer_fraction`. Traced by hand against
/// the C loop at :356-363.
#[test]
fn allocate_gf_group_bits_consumes_the_arf_pool() {
    let rc = RateControl {
        baseline_gf_interval: 8,
        ..Default::default()
    };
    let mut group = vec![
        GfGroupFrame {
            update_type: FrameUpdateType::LfUpdate,
            ..Default::default()
        },
        arf(1),
        arf(2),
        GfGroupFrame {
            update_type: FrameUpdateType::OverlayUpdate,
            ..Default::default()
        },
    ];
    // gf_group_bits 80_000, gf_arf_bits 8_000, hierarchical_levels 2 so
    // max_arf_layer == 2 and level 2 takes fraction 1.0.
    allocate_gf_group_bits(&rc, &mut group, 4, 2, 80_000, 8_000, 8, false, true);
    let base = (80_000 - 8_000) / 8; // 9_000
    assert_eq!(group[0].base_frame_target, base);
    // Level 1 takes layer_fraction[1] = 0.80 of 8_000 over 1 frame.
    assert_eq!(group[1].base_frame_target, base + 6_400);
    // Level 2 (== max_arf_layer) takes ALL of what is left: 8_000 - 6_400.
    assert_eq!(group[2].base_frame_target, base + 1_600);
    // Overlays get nothing.
    assert_eq!(group[3].base_frame_target, 0);
}

/// `is_new_gf_group` (pass2_strategy.c:823): for a complete mini-GOP the
/// answer is this frame's own flag and NOTHING is cleared; for an
/// incomplete one a hit clears `gf_update_due` across the whole group.
#[test]
fn is_new_gf_group_clears_only_on_the_incomplete_path() {
    let mut group = vec![
        GfGroupFrame {
            picture_number: 10,
            gf_update_due: true,
            is_incomp_mg_frame: true,
            ..Default::default()
        },
        GfGroupFrame {
            picture_number: 11,
            gf_update_due: true,
            is_incomp_mg_frame: true,
            ..Default::default()
        },
    ];
    // Complete mini-GOP: returns the frame's own flag, clears nothing.
    assert!(is_new_gf_group(&mut group, 2, 10, false, true));
    assert!(group[0].gf_update_due && group[1].gf_update_due);
    // Incomplete: finds a due neighbour and clears every entry.
    assert!(is_new_gf_group(&mut group, 2, 10, true, false));
    assert!(!group[0].gf_update_due && !group[1].gf_update_due);
    // Now nothing is due, so no new group and still nothing to clear.
    assert!(!is_new_gf_group(&mut group, 2, 10, true, false));
}

/// `set_kf_interval_variables` (pass2_strategy.c:594): a
/// `num_frames_to_detect_scenecut` of 0 returns having written NOTHING,
/// not even `frames_to_key`.
#[test]
fn set_kf_interval_variables_zero_lookahead_writes_nothing() {
    let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0)];
    let mut cursor = StatsCursor::new(&stats, 0);
    let mut rc = RateControl {
        frames_to_key: 77,
        ..Default::default()
    };
    let scs = SeqRc {
        intra_period_length: 63,
        ..Default::default()
    };
    let mut this_frame = stats[0];
    let (err, eos) = set_kf_interval_variables(
        &mut rc,
        &scs,
        &mut cursor,
        true,
        &mut this_frame,
        true,
        0,
        false,
        false,
    );
    assert_eq!(err, 0.0);
    assert!(!eos);
    assert_eq!(rc.frames_to_key, 77, "frames_to_key must be untouched");
    assert_eq!(cursor.position(), 0);
}

/// `set_kf_interval_variables`, the normal path: it walks the ring and
/// clamps `frames_to_key` to `intra_period_length + 1`. Note the loop runs
/// once with the cursor on the end sentinel (see [`StatsCursor`]), so
/// three stats yield `frames_to_key == 3` and not 2.
#[test]
fn set_kf_interval_variables_counts_through_the_sentinel() {
    let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0), stat(30, 1.0, 0)];
    // The caller invariant: `this_frame` is stats[0] and the cursor is
    // already past it. See `StatsCursor`.
    let mut cursor = StatsCursor::new(&stats, 1);
    let mut rc = RateControl::default();
    let scs = SeqRc {
        intra_period_length: 63,
        ..Default::default()
    };
    let mut this_frame = stats[0];
    let (err, _) = set_kf_interval_variables(
        &mut rc,
        &scs,
        &mut cursor,
        true,
        &mut this_frame,
        true,
        100,
        false,
        false,
    );
    // Bodies run for stats[0], [1], [2]; the third advance hits EOF and
    // breaks, so the sentinel iteration does not add a fourth term here.
    assert_eq!(err, 10.0 + 20.0 + 30.0);
    assert_eq!(rc.frames_to_key, 3);
}

/// `lap_rc_group_error_calc` (pass2_strategy.c:562) sums EXACTLY
/// `frames_to_key` entries and restores the cursor.
#[test]
fn lap_rc_group_error_calc_sums_frames_to_key_entries() {
    let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0), stat(30, 1.0, 0)];
    // Caller invariant: this_frame == stats[0], cursor already past it.
    let mut cursor = StatsCursor::new(&stats, 1);
    let rc = RateControl {
        frames_to_key: 2,
        ..Default::default()
    };
    let err = lap_rc_group_error_calc(&rc, &mut cursor, true, stats[0]);
    assert_eq!(err, 10.0 + 20.0);
    assert_eq!(cursor.position(), 1, "the cursor must be restored");
}

/// `calculate_total_gf_group_bits` (pass2_strategy.c:186) SUBTRACTS what
/// it hands out, so it is a state mutator: calling it twice with the same
/// arguments does not return the same number.
#[test]
fn calculate_total_gf_group_bits_consumes_the_kf_budget() {
    let rc = RateControl {
        avg_frame_bandwidth: 100_000,
        max_frame_bandwidth: 10_000_000,
        baseline_gf_interval: 16,
        frames_to_key: 40,
        ..Default::default()
    };
    let scs = SeqRc {
        hierarchical_levels: 4,
        intra_period_length: 63,
        ..Default::default()
    };
    let cfg2 = TwoPassCfg {
        vbrmax_section: 2000,
        ..Default::default()
    };
    let mut twopass = TwoPassState {
        kf_group_bits: 1_000_000,
        kf_group_error_left: 4_000,
        ..Default::default()
    };
    let first = calculate_total_gf_group_bits(&rc, &scs, &cfg2, &mut twopass, 0, 1_000.0);
    assert_eq!(first, 250_000);
    assert_eq!(twopass.kf_group_bits, 750_000);
    let second = calculate_total_gf_group_bits(&rc, &scs, &cfg2, &mut twopass, 0, 1_000.0);
    assert_eq!(second, 187_500);
    assert_eq!(twopass.kf_group_bits, 562_500);
}

/// `rc_update_framerate` (pass2_strategy.c:884): `max_frame_bandwidth` is
/// the MAXIMUM of three terms, so a small `vbrmax_section` cannot lower it
/// below the 1080p floor.
#[test]
fn rc_update_framerate_floors_at_maxrate_1080p() {
    let mut rc = RateControl::default();
    let cfg2 = TwoPassCfg {
        vbrmax_section: 100,
        num_mbs: 100,
        ..Default::default()
    };
    rc_update_framerate(&mut rc, &cfg2, 3_000_000, 60.0);
    assert_eq!(rc.avg_frame_bandwidth, 50_000);
    assert_eq!(rc.max_frame_bandwidth, MAXRATE_1080P as i32);
    // A big MB count wins instead.
    let cfg_big = TwoPassCfg {
        vbrmax_section: 100,
        num_mbs: 100_000,
        ..Default::default()
    };
    rc_update_framerate(&mut rc, &cfg_big, 3_000_000, 60.0);
    assert_eq!(rc.max_frame_bandwidth, 100_000 * 250);
}

/// `svt_av1_new_framerate` (pass2_strategy.c:901) maps anything under 0.1
/// to **30**, not to 0.1.
#[test]
fn new_framerate_maps_tiny_to_thirty() {
    let mut rc = RateControl::default();
    let cfg2 = TwoPassCfg {
        vbrmax_section: 100,
        num_mbs: 1,
        ..Default::default()
    };
    assert_eq!(new_framerate(&mut rc, &cfg2, 3_000_000, 0.05), 30.0);
    assert_eq!(rc.avg_frame_bandwidth, 100_000);
    assert_eq!(new_framerate(&mut rc, &cfg2, 3_000_000, 24.0), 24.0);
    assert_eq!(rc.avg_frame_bandwidth, 125_000);
}

/// `get_section_target_bandwidth` (pass2_strategy.c:746) divides by
/// `total_stats->count - picture_number` with no guard in C; the port
/// returns `None` there instead of dividing by zero.
#[test]
fn get_section_target_bandwidth_refuses_zero_frames_left() {
    let rc = RateControl {
        avg_frame_bandwidth: 1_234,
        ..Default::default()
    };
    let twopass = TwoPassState {
        bits_left: 1_000_000,
        ..Default::default()
    };
    assert_eq!(
        get_section_target_bandwidth(&rc, &twopass, false, 100.0, 60),
        Some(25_000)
    );
    assert_eq!(
        get_section_target_bandwidth(&rc, &twopass, false, 60.0, 60),
        None
    );
    // lap_rc ignores the ring entirely.
    assert_eq!(
        get_section_target_bandwidth(&rc, &twopass, true, 60.0, 60),
        Some(1_234)
    );
}

/// `calculate_gf_stats` (pass2_strategy.c:341): an intra frame's error is
/// SUBTRACTED before the loop adds it back, and the cursor is restored.
#[test]
fn calculate_gf_stats_pre_subtracts_the_intra_frame() {
    let stats = [stat(10, 1.0, 0), stat(20, 2.0, 0), stat(30, 3.0, 0)];
    let mut rc = RateControl {
        frames_to_key: 100,
        ..Default::default()
    };
    let inter = FrameRc {
        frame_type: crate::port_rc_vbr_cbr_state::FrameType::Inter,
        ..Default::default()
    };
    let key = FrameRc {
        frame_type: crate::port_rc_vbr_cbr_state::FrameType::Key,
        ..Default::default()
    };

    let mut cursor = StatsCursor::new(&stats, 0);
    let mut this = stats[0];
    let r_inter = calculate_gf_stats(&mut rc, &inter, &mut cursor, true, &mut this, 3, false);
    assert_eq!(cursor.position(), 0);
    assert_eq!(r_inter.arf_position, 3);
    assert!(r_inter.use_alt_ref);

    let mut cursor = StatsCursor::new(&stats, 0);
    let mut this = stats[0];
    let r_key = calculate_gf_stats(&mut rc, &key, &mut cursor, true, &mut this, 3, false);
    // The key-frame run is exactly the inter run minus this_frame's error.
    assert_eq!(
        r_key.gf_stats.gf_group_err,
        r_inter.gf_stats.gf_group_err - 10.0
    );
    assert_eq!(
        r_key.gf_stats.gf_group_raw_error,
        r_inter.gf_stats.gf_group_raw_error - 1.0
    );
}
