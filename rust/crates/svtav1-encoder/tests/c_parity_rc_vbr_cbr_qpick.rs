//! Differential parity: the VBR/CBR **qindex decision**
//! (`svtav1-encoder/src/port_rc_vbr_cbr_qpick.rs`) vs the REAL exported
//! `svt_av1_rc_calc_qindex_rate_control` in `Codec/rc_vbr_cbr.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) with NO symbol promotion
//! needed — the entry point is a plain exported symbol. That matters more than
//! usual here: every function it calls is `static` AND was inlined away by the
//! Release build, so `nm` finds none of them and this differential is the ONLY
//! way to drive them at all. One call pins, in one go:
//!
//! * `rc_pick_q_and_bounds_no_stats_cbr` (CBR) and `rc_pick_q_and_bounds` (VBR)
//! * `calc_active_worst_quality_no_stats_cbr` — including the two branches the
//!   `svt_av1_resize_reset_rc` route in `c_parity_rc_vbr_cbr_state.rs` could
//!   NOT reach, because nothing resets the buffer to optimal first here
//! * `calc_active_best_quality_no_stats_cbr`, with its three-way
//!   reference-preference scan over both lists
//! * `get_active_best_quality`, `adjust_active_best_and_worst_quality_org`,
//!   `av1_frame_type_qdelta_org`, `get_q`
//! * `find_min_ref_base_q_idx` and all four reference-qindex floors
//! * `cyclic_refresh_init`
//!
//! **The one seam, declared.** C calls `svt_aom_cyclic_refresh_setup` from
//! inside the function; that lives in `Codec/rc_aq.c`, is a different lane's
//! file, and can switch `apply_cyclic_refresh` back off. The port takes it as
//! a callback, and this test supplies a callback that copies C's post-call
//! `CyclicRefresh` — so the cells with cyclic refresh ON compare everything
//! EXCEPT that function's own outputs, and the ones with it off compare
//! everything. Both are labelled below.
//!
//! **Anti-vacuity.** Every sweep counts cells and asserts the C oracle's
//! outputs actually vary; and the DPB is deliberately built so the
//! reference-qindex floors bind on some cells and not others.

use svtav1_cref::rc_vbr_cbr as cref;
use svtav1_encoder::port_rc_process::{FrameUpdateType, SliceType};
use svtav1_encoder::port_rc_vbr_cbr_qpick as qp;
use svtav1_encoder::port_rc_vbr_cbr_state as rcs;

/// A scenario in the port's own types, plus the C-side flat mirror.
struct Cell {
    rc: rcs::RateControl,
    cfg: rcs::RateControlCfg,
    scs: rcs::SeqRc,
    twopass: qp::TwoPassRc,
    frame: rcs::FrameRc,
    slice_type: SliceType,
    l0: Vec<qp::RefPicRc>,
    l1: Vec<qp::RefPicRc>,
    l0_count_try: usize,
    l1_count_try: usize,
    me_cur: Vec<u32>,
    me_ref: Vec<u32>,
    cr_sb_end_ctx: u32,
    /// Mirrors what the C shim installs, so the two sides are built from ONE
    /// description rather than two.
    c: cref::RefRcQpickState,
}

fn mk_ref(
    tmp_layer: u8,
    slice: SliceType,
    poc: u64,
    qidx: u8,
    pcs_r0: f64,
    obj_r0: f64,
) -> qp::RefPicRc {
    qp::RefPicRc {
        tmp_layer_idx: tmp_layer,
        slice_type: slice,
        pcs_slice_type: slice,
        ref_poc: poc,
        base_q_idx: qidx,
        pcs_r0,
        obj_r0,
    }
}

fn c_ref(r: &qp::RefPicRc) -> cref::RefRcQpickRef {
    cref::RefRcQpickRef {
        present: 1,
        tmp_layer_idx: i32::from(r.tmp_layer_idx),
        slice_type: r.slice_type as i32,
        pcs_slice_type: r.pcs_slice_type as i32,
        ref_poc: r.ref_poc,
        base_q_idx: i32::from(r.base_q_idx),
        pcs_r0: r.pcs_r0,
        obj_r0: r.obj_r0,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_cell(
    mode: rcs::AomRcMode,
    bit_depth: u8,
    frame_type: rcs::FrameType,
    slice_type: SliceType,
    update_type: FrameUpdateType,
    tl: u8,
    layer_depth: i32,
    sc: bool,
    buffer_level: i64,
    avg_qi: [i32; 2],
    refs_l0: Vec<qp::RefPicRc>,
    refs_l1: Vec<qp::RefPicRc>,
    intra_period: i32,
    r0: f64,
    extend: (i32, i32, i32),
    b64: u16,
    this_frame_target: i32,
) -> Cell {
    let rc = rcs::RateControl {
        avg_frame_bandwidth: 83_333,
        prev_avg_frame_bandwidth: 80_000,
        max_frame_bandwidth: 833_330,
        optimal_buffer_level: 625_000,
        maximum_buffer_size: 900_000,
        buffer_level,
        bits_off_target: buffer_level,
        rate_correction_factors: [0.4, 0.6, 0.8, 1.0, 1.2, 1.4, 1.6],
        avg_frame_qindex: avg_qi,
        last_q: [40, 45],
        worst_quality: 255,
        best_quality: 0,
        active_worst_quality: 180,
        active_best_quality: [30, 40, 50, 60, 70, 80, 90],
        last_boosted_qindex: 96,
        kf_boost: 2300,
        gfu_boost: 2000,
        arf_q: 0,
        frames_to_key: 40,
        frames_since_key: 30,
        this_key_frame_forced: false,
        avg_frame_low_motion: 70,
        rc_1_frame: -1,
        rc_2_frame: 1,
        q_1_frame: 130,
        q_2_frame: 128,
        percent_refresh_adjustment: 2,
        rate_ratio_qdelta_adjustment: 0.1,
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
        encoder_bit_depth: bit_depth,
        intra_period_length: intra_period,
        hierarchical_levels: 4,
        min_qp_allowed: 1,
        max_qp_allowed: 63,
        super_block_size: 64,
        sb_total_count: 510,
        passes: 1,
        qp_scale_compress_strength: 2,
        input_resolution: 2,
        is_short_clip: false,
        gop_constraint_rc: false,
        ..Default::default()
    };
    let twopass = qp::TwoPassRc {
        extend_minq: extend.0,
        extend_maxq: extend.1,
        extend_minq_fast: extend.2,
        kf_zeromotion_pct: 50,
        total_stats_count: 300,
    };
    let frame = rcs::FrameRc {
        picture_number: 64,
        frame_offset: 64,
        frame_type,
        showable_frame: true,
        base_q_idx: 130,
        update_type,
        is_overlay: false,
        is_ref: true,
        sc_class1: sc,
        transition_present: -1,
        temporal_layer_index: tl,
        hierarchical_levels: 4,
        layer_depth,
        frame_width: 1920,
        frame_height: 1080,
        superres_upscaled_width: 1920,
        this_frame_target,
        projected_frame_size: 80_000,
        b64_total_count: b64,
        r0,
        ..Default::default()
    };

    let n = usize::from(b64);
    let me_cur: Vec<u32> = (0..64).map(|i| 1_000 + (i as u32) * 37).collect();
    let me_ref: Vec<u32> = (0..64).map(|i| 900 + (i as u32) * 11).collect();

    let l0_count_try = refs_l0.len();
    let l1_count_try = refs_l1.len();

    let mut c = cref::RefRcQpickState {
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
            is_overlay: 0,
            sc_class1: i32::from(frame.sc_class1),
            temporal_layer_index: i32::from(frame.temporal_layer_index),
            hierarchical_levels: i32::from(frame.hierarchical_levels),
            frame_width: frame.frame_width,
            frame_height: frame.frame_height,
            this_frame_target: frame.this_frame_target,
            projected_frame_size: frame.projected_frame_size,
            base_q_idx: frame.base_q_idx,
            b64_total_count: i32::from(b64),
            apply_cyclic_refresh: 0,
            qindex_delta: [0; 3],
            actual_num_seg1_sbs: 0,
            actual_num_seg2_sbs: 0,
            encoder_bit_depth: i32::from(bit_depth),
        },
        active_worst_quality: rc.active_worst_quality,
        active_best_quality: rc.active_best_quality,
        last_boosted_qindex: rc.last_boosted_qindex,
        kf_boost: rc.kf_boost,
        gfu_boost: rc.gfu_boost,
        arf_q: rc.arf_q,
        frames_to_key: rc.frames_to_key,
        this_key_frame_forced: i32::from(rc.this_key_frame_forced),
        avg_frame_low_motion: rc.avg_frame_low_motion,
        picture_number: frame.picture_number,
        frame_offset: frame.frame_offset,
        slice_type: slice_type as i32,
        layer_depth: frame.layer_depth,
        is_ref: i32::from(frame.is_ref),
        transition_present: i32::from(frame.transition_present),
        r0: frame.r0,
        intra_period_length: scs.intra_period_length,
        gop_constraint_rc: i32::from(scs.gop_constraint_rc),
        is_short_clip: i32::from(scs.is_short_clip),
        super_block_size: i32::from(scs.super_block_size),
        sb_total_count: i32::from(scs.sb_total_count),
        passes: scs.passes,
        qp_scale_compress_strength: i32::from(scs.qp_scale_compress_strength),
        input_resolution: scs.input_resolution,
        min_qp_allowed: i32::from(scs.min_qp_allowed),
        max_qp_allowed: i32::from(scs.max_qp_allowed),
        seq_hierarchical_levels: i32::from(scs.hierarchical_levels),
        extend_minq: twopass.extend_minq,
        extend_maxq: twopass.extend_maxq,
        extend_minq_fast: twopass.extend_minq_fast,
        kf_zeromotion_pct: twopass.kf_zeromotion_pct,
        l0_count_try: l0_count_try as i32,
        l1_count_try: l1_count_try as i32,
        b64_total_count: i32::from(b64),
        norm_me_dist: 1_500,
        cr_sb_end_ctx: 0,
        ..Default::default()
    };
    for (i, r) in refs_l0.iter().enumerate().take(cref::QPICK_MAX_REFS) {
        c.l0[i] = c_ref(r);
    }
    for (i, r) in refs_l1.iter().enumerate().take(cref::QPICK_MAX_REFS) {
        c.l1[i] = c_ref(r);
    }
    for i in 0..64 {
        c.me_cur_64x64[i] = me_cur[i];
        c.me_ref_l0_64x64[i] = me_ref[i];
        // A mix of static and moving blocks so the cyclic-refresh motion gate
        // classifies some into each segment rather than rejecting all of them.
        c.me_cur_8x8[i] = 800 + (i as u32) * 29;
        c.me_mv_x[i] = if i % 3 == 0 { 0 } else { 4 };
        c.me_mv_y[i] = if i % 5 == 0 { 1 } else { 0 };
    }
    let _ = n;

    Cell {
        rc,
        cfg,
        scs,
        twopass,
        frame,
        slice_type,
        l0: refs_l0,
        l1: refs_l1,
        l0_count_try,
        l1_count_try,
        me_cur,
        me_ref,
        cr_sb_end_ctx: 0,
        c,
    }
}

fn cells() -> Vec<Cell> {
    let mut out = Vec::new();
    let refs_a = vec![
        mk_ref(0, SliceType::B, 60, 100, 0.30, 0.30),
        mk_ref(1, SliceType::B, 56, 118, 0.28, 0.28),
    ];
    let refs_b = vec![
        mk_ref(0, SliceType::I, 32, 80, 0.45, 0.45),
        mk_ref(2, SliceType::B, 63, 140, 0.31, 0.31),
        mk_ref(1, SliceType::B, 48, 90, 0.20, 0.20),
    ];
    let refs_l1 = vec![mk_ref(1, SliceType::B, 72, 110, 0.33, 0.33)];
    // A HIGH-qindex reference set. Without it the reference-qindex floors
    // never bind (measured: the `floors_bound` control below failed on a
    // sweep that had only the two sets above), so the whole floor block
    // could have been deleted with this test still green.
    let refs_high = vec![
        mk_ref(0, SliceType::B, 60, 236, 0.30, 0.30),
        mk_ref(1, SliceType::B, 56, 244, 0.28, 0.28),
    ];
    let refs_high_l1 = vec![mk_ref(0, SliceType::B, 72, 240, 0.33, 0.33)];

    for &mode in &[rcs::AomRcMode::Cbr, rcs::AomRcMode::Vbr] {
        for &bd in &[8u8, 10u8] {
            for &(frame_type, slice_type) in &[
                (rcs::FrameType::Inter, SliceType::B),
                (rcs::FrameType::Key, SliceType::I),
            ] {
                for &ut in &[
                    FrameUpdateType::LfUpdate,
                    FrameUpdateType::GfUpdate,
                    FrameUpdateType::ArfUpdate,
                    FrameUpdateType::IntnlArfUpdate,
                ] {
                    for &(tl, layer_depth) in &[(0u8, 1i32), (1, 2), (2, 3), (3, 5)] {
                        for &sc in &[false, true] {
                            for &buf in &[625_000i64, 200_000, 800_000, 40_000] {
                                for &ip in &[-1i32, 64] {
                                    for (refs0, refs1, tgt) in [
                                        (refs_a.clone(), Vec::new(), 83_333),
                                        (refs_b.clone(), refs_l1.clone(), 83_333),
                                        // Big target => low regulated q, and
                                        // high-qindex refs => high floor.
                                        (refs_high.clone(), refs_high_l1.clone(), 700_000),
                                    ] {
                                        out.push(build_cell(
                                            mode,
                                            bd,
                                            frame_type,
                                            slice_type,
                                            ut,
                                            tl,
                                            layer_depth,
                                            sc,
                                            buf,
                                            [120, 130],
                                            refs0,
                                            refs1,
                                            ip,
                                            0.25,
                                            (4, 6, 2),
                                            32,
                                            tgt,
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
fn calc_qindex_rate_control_matches_c() {
    let mut cells_run = 0usize;
    let mut cr_on = 0usize;
    let mut qindexes = std::collections::BTreeSet::new();
    let mut floors_bound = 0usize;

    for mut cell in cells() {
        let mut c = cell.c;
        let want_q = cref::calc_qindex_rate_control(&mut c);

        let refs = qp::RefLists {
            l0: &cell.l0,
            l1: &cell.l1,
            l0_count_try: cell.l0_count_try,
            l1_count_try: cell.l1_count_try,
        };
        let me = qp::MeDistortion {
            ref_l0: &cell.me_ref,
            cur: &cell.me_cur,
        };
        let mut cr = rcs::CyclicRefresh::default();
        let mut cr_sb_end = cell.cr_sb_end_ctx;
        // The AQ seam: hand the port exactly what C's
        // `svt_aom_cyclic_refresh_setup` produced, since that function belongs
        // to `Codec/rc_aq.c` and not to this port.
        let aq = |dst: &mut rcs::CyclicRefresh| {
            dst.apply_cyclic_refresh = c.cr_apply != 0;
            dst.rate_ratio_qdelta_seg2 = c.cr_rate_ratio_qdelta_seg2;
            dst.actual_num_seg1_sbs = c.cr_actual_num_seg1_sbs;
            dst.actual_num_seg2_sbs = c.cr_actual_num_seg2_sbs;
        };
        let got_q = qp::rc_calc_qindex_rate_control(
            &mut cell.rc,
            &cell.cfg,
            &cell.scs,
            &cell.twopass,
            &mut cell.frame,
            &refs,
            cell.slice_type,
            Some(me),
            &mut cr_sb_end,
            &mut cr,
            aq,
        )
        .expect("the DPB always has an L0 slot 0 in this sweep");

        let ctx = format!(
            "mode={:?} bd={} ft={:?} slice={:?} ut={:?} tl={} ld={} sc={} buf={} ip={} nrefs={}/{}",
            cell.cfg.mode,
            cell.scs.encoder_bit_depth,
            cell.frame.frame_type,
            cell.slice_type,
            cell.frame.update_type,
            cell.frame.temporal_layer_index,
            cell.frame.layer_depth,
            cell.frame.sc_class1,
            cell.rc.buffer_level,
            cell.scs.intra_period_length,
            cell.l0_count_try,
            cell.l1_count_try,
        );
        assert_eq!(got_q, want_q, "base_q_idx mismatch: {ctx}");
        assert_eq!(
            cell.frame.base_q_idx, c.out_base_q_idx,
            "stored base_q_idx: {ctx}"
        );
        assert_eq!(cell.frame.top_index, c.out_top_index, "top_index: {ctx}");
        assert_eq!(
            cell.frame.bottom_index, c.out_bottom_index,
            "bottom_index: {ctx}"
        );
        assert_eq!(cell.rc.arf_q, c.arf_q, "arf_q: {ctx}");

        // cyclic_refresh_init's own outputs, comparable in every CBR cell
        // regardless of what the AQ setup then did.
        if cell.cfg.mode == rcs::AomRcMode::Cbr {
            assert_eq!(
                cr.percent_refresh, c.cr_percent_refresh,
                "cr.percent_refresh: {ctx}"
            );
            assert_eq!(cr.sb_start, c.cr_sb_start, "cr.sb_start: {ctx}");
            assert_eq!(cr.sb_end, c.cr_sb_end, "cr.sb_end: {ctx}");
            assert_eq!(cr_sb_end, c.cr_sb_end_ctx, "enc_ctx->cr_sb_end: {ctx}");
            if c.cr_apply != 0 {
                cr_on += 1;
                assert_eq!(
                    cr.max_qdelta_perc, c.cr_max_qdelta_perc,
                    "cr.max_qdelta_perc: {ctx}"
                );
                assert_eq!(
                    cr.rate_boost_fac, c.cr_rate_boost_fac,
                    "cr.rate_boost_fac: {ctx}"
                );
                assert_eq!(
                    cr.rate_ratio_qdelta, c.cr_rate_ratio_qdelta,
                    "cr.rate_ratio_qdelta: {ctx}"
                );
                assert_eq!(cr.qindex_delta, c.cr_qindex_delta, "cr.qindex_delta: {ctx}");
            }
        }

        // Did a reference floor actually bind on this cell? MEASURED, not
        // assumed: re-run just the pick on a clone and see whether the final
        // qindex differs from the clamped pick. Without this the sweep could
        // be green with the whole reference-floor block deleted.
        {
            let mut rc2 = cell.rc.clone();
            let mut frame2 = cell.frame;
            let pick = if cell.cfg.mode == rcs::AomRcMode::Cbr {
                qp::rc_pick_q_and_bounds_no_stats_cbr(
                    &mut rc2,
                    &cell.cfg,
                    &cell.scs,
                    &mut frame2,
                    &refs,
                    cell.slice_type,
                )
                .expect("L0 slot 0 present")
            } else {
                qp::rc_pick_q_and_bounds(
                    &mut rc2,
                    &cell.cfg,
                    &cell.scs,
                    &cell.twopass,
                    &mut frame2,
                    &refs,
                )
            };
            if rcs::clamp_qindex(&cell.scs, pick) != want_q {
                floors_bound += 1;
            }
        }
        qindexes.insert(want_q);
        cells_run += 1;
    }

    println!(
        "qpick differential: {cells_run} cells, {} distinct qindexes, {cr_on} with cyclic \
         refresh enabled, {floors_bound} where a reference floor bound",
        qindexes.len()
    );
    assert!(cells_run >= 2000, "sweep collapsed to {cells_run} cells");
    assert!(
        qindexes.len() > 20,
        "the C oracle produced only {} distinct qindexes over {cells_run} cells",
        qindexes.len()
    );
    assert!(
        cr_on > 0,
        "no cell reached cyclic_refresh_init's ENABLED path — the CBR arm's \
         qindex_delta comparison never ran"
    );
    assert!(
        floors_bound > 0,
        "no cell's final qindex differed from the clamped pick — the \
         reference-qindex floor block is inert in this sweep, so deleting it \
         would not fail this test"
    );
}
