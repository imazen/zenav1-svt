//! Differential parity: the VBR/CBR rate-control state machine
//! (`svtav1-encoder/src/port_rc_vbr_cbr_state.rs`) vs the REAL compiled C in
//! `Codec/rc_vbr_cbr.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) for the six functions
//! that have a reachable symbol:
//!
//! | C function | how it is reached |
//! |---|---|
//! | `av1_rc_regulate_q` | `--globalize-symbol` promotion of the CMake object |
//! | `av1_rc_update_rate_correction_factors` | same |
//! | `get_regulated_q_overshoot` | same |
//! | `get_regulated_q_undershoot` | same |
//! | `clamp_qindex` | same |
//! | `svt_av1_resize_reset_rc` | plain exported symbol, no promotion |
//!
//! **`calc_active_worst_quality_no_stats_cbr` is pinned INDIRECTLY, and the
//! coverage that buys is partial.** It has a `t` symbol but LLVM specialized
//! its ABI (two args, `x0` is not a PPCS — see `link_globalized_rc_vbr_statics`
//! in `svtav1-cref/build.rs` for the prologue), so it cannot be called
//! directly. `svt_av1_resize_reset_rc` calls it, and its result feeds
//! `av1_rc_regulate_q` whose qindex then selects the correction-factor nudges
//! that this test compares — so a wrong port of it shows up as a wrong
//! `rate_correction_factors[0]`. The LIMIT: `resize_reset_rc` runs
//! `svt_aom_reset_update_frame_target` first, which sets
//! `buffer_level = optimal_buffer_level`, so ONLY the middle
//! (`buffer_level > critical_level`) branch is reachable this way. The
//! above-optimal and below-critical branches stay evidence tier 4 and are
//! covered by hand-derived vectors in the port module.
//!
//! Each of the first five drives several of the file's inlined-away statics as
//! a side effect, which is how they get pinned without a symbol of their own:
//! `regulate_q` runs `get_rate_correction_factor`, `get_bits_per_mb`,
//! `find_closest_qindex_by_rate` and (in CBR) `adjust_q_cbr`;
//! `update_rate_correction_factors` runs `av1_estimate_bits_at_q` and
//! `set_rate_correction_factor`; `resize_reset_rc` runs
//! `svt_aom_reset_update_frame_target` and
//! `av1_calc_pframe_target_size_one_pass_cbr`. That is a stronger statement
//! than a hand-derived vector suite for each.
//!
//! **Not covered here, and why.** `rc_pick_q_and_bounds*`,
//! `get_active_best_quality`, `svt_av1_rc_calc_qindex_rate_control`,
//! `svt_av1_rc_postencode_update*` and the cyclic-refresh group have no symbol
//! at any linkage (`nm` on `rc_vbr_cbr.c.o` lists eight locals; the rest were
//! inlined) AND need a populated `PictureControlSet` with reference-picture
//! objects, so they stay at tier 4 with hand-derived vectors in the port's own
//! unit tests.
//!
//! **Anti-vacuity.** Every sweep counts its cells and asserts a floor, and
//! each function has a control asserting the oracle's output actually VARIES
//! across the sweep — a differential against a constant would otherwise pass
//! while proving nothing (`docs/WORKING-ON-THIS.md` §5).

use svtav1_cref::rc_vbr_cbr as cref;
use svtav1_encoder::port_rc_process::FrameUpdateType;
use svtav1_encoder::port_rc_vbr_cbr_state as rcs;

/// One test scenario, in the port's own types. Converted to the C oracle's
/// flat struct by [`to_ref`] so there is exactly one mapping to check.
#[derive(Clone)]
struct Scenario {
    rc: rcs::RateControl,
    cfg: rcs::RateControlCfg,
    scs: rcs::SeqRc,
    frame: rcs::FrameRc,
    cr: rcs::CyclicRefresh,
}

fn to_ref(s: &Scenario) -> cref::RefRcVbrState {
    cref::RefRcVbrState {
        avg_frame_bandwidth: s.rc.avg_frame_bandwidth,
        prev_avg_frame_bandwidth: s.rc.prev_avg_frame_bandwidth,
        max_frame_bandwidth: s.rc.max_frame_bandwidth,
        optimal_buffer_level: s.rc.optimal_buffer_level,
        maximum_buffer_size: s.rc.maximum_buffer_size,
        buffer_level: s.rc.buffer_level,
        bits_off_target: s.rc.bits_off_target,
        rate_correction_factors: s.rc.rate_correction_factors,
        avg_frame_qindex: s.rc.avg_frame_qindex,
        worst_quality: s.rc.worst_quality,
        best_quality: s.rc.best_quality,
        q_1_frame: s.rc.q_1_frame,
        q_2_frame: s.rc.q_2_frame,
        rc_1_frame: s.rc.rc_1_frame,
        rc_2_frame: s.rc.rc_2_frame,
        frames_since_key: s.rc.frames_since_key,
        percent_refresh_adjustment: s.rc.percent_refresh_adjustment,
        rate_ratio_qdelta_adjustment: s.rc.rate_ratio_qdelta_adjustment,
        cur_avg_base_me_dist: s.rc.cur_avg_base_me_dist,
        prev_avg_base_me_dist: s.rc.prev_avg_base_me_dist,
        frame_updated: s.rc.frame_updated,
        rc_mode: s.cfg.mode as i32,
        under_shoot_pct: s.cfg.under_shoot_pct,
        over_shoot_pct: s.cfg.over_shoot_pct,
        frame_type: s.frame.frame_type as i32,
        update_type: s.frame.update_type as i32,
        is_overlay: i32::from(s.frame.is_overlay),
        sc_class1: i32::from(s.frame.sc_class1),
        temporal_layer_index: i32::from(s.frame.temporal_layer_index),
        hierarchical_levels: i32::from(s.frame.hierarchical_levels),
        frame_width: s.frame.frame_width,
        frame_height: s.frame.frame_height,
        this_frame_target: s.frame.this_frame_target,
        projected_frame_size: s.frame.projected_frame_size,
        base_q_idx: s.frame.base_q_idx,
        b64_total_count: i32::from(s.frame.b64_total_count),
        apply_cyclic_refresh: i32::from(s.cr.apply_cyclic_refresh),
        qindex_delta: s.cr.qindex_delta,
        actual_num_seg1_sbs: s.cr.actual_num_seg1_sbs,
        actual_num_seg2_sbs: s.cr.actual_num_seg2_sbs,
        encoder_bit_depth: i32::from(s.scs.encoder_bit_depth),
    }
}

/// A plausible mid-encode CBR state: 5 Mbit/s at 60 fps, buffer at optimal.
fn base_scenario() -> Scenario {
    let mut rc = rcs::RateControl {
        avg_frame_bandwidth: 83_333,
        prev_avg_frame_bandwidth: 83_333,
        max_frame_bandwidth: 833_330,
        optimal_buffer_level: 625_000,
        maximum_buffer_size: 625_000,
        buffer_level: 625_000,
        bits_off_target: 625_000,
        avg_frame_qindex: [120, 130],
        worst_quality: 255,
        best_quality: 0,
        q_1_frame: 130,
        q_2_frame: 128,
        frames_since_key: 30,
        ..Default::default()
    };
    rc.rate_correction_factors = [1.0; 7];
    Scenario {
        rc,
        cfg: rcs::RateControlCfg {
            mode: rcs::AomRcMode::Cbr,
            ..Default::default()
        },
        scs: rcs::SeqRc::default(),
        frame: rcs::FrameRc {
            frame_type: rcs::FrameType::Inter,
            update_type: FrameUpdateType::LfUpdate,
            frame_width: 1920,
            frame_height: 1080,
            superres_upscaled_width: 1920,
            this_frame_target: 83_333,
            projected_frame_size: 80_000,
            base_q_idx: 130,
            b64_total_count: 510,
            hierarchical_levels: 4,
            temporal_layer_index: 0,
            ..Default::default()
        },
        cr: rcs::CyclicRefresh::default(),
    }
}

/// The scenario grid every sweep walks. Deliberately spans both RC modes, both
/// bit depths, key and inter frames, screen/non-screen, all five temporal
/// layers of a 4-level hierarchy, an under- and an over-full buffer, and the
/// GF/ARF update types that select a different rate-correction-factor slot.
fn scenarios() -> Vec<Scenario> {
    let mut out = Vec::new();
    for &mode in &[rcs::AomRcMode::Cbr, rcs::AomRcMode::Vbr] {
        for &bd in &[8u8, 10u8] {
            for &frame_type in &[rcs::FrameType::Inter, rcs::FrameType::Key] {
                for &sc in &[false, true] {
                    for &tl in &[0u8, 1, 3] {
                        for &(buf, off) in &[
                            (625_000i64, 625_000i64),
                            (100_000, 100_000),
                            (1_100_000, 1_100_000),
                            (-40_000, -40_000),
                        ] {
                            for &ut in &[
                                FrameUpdateType::LfUpdate,
                                FrameUpdateType::GfUpdate,
                                FrameUpdateType::ArfUpdate,
                            ] {
                                let mut s = base_scenario();
                                s.cfg.mode = mode;
                                s.scs.encoder_bit_depth = bd;
                                s.frame.frame_type = frame_type;
                                s.frame.sc_class1 = sc;
                                s.frame.temporal_layer_index = tl;
                                s.frame.update_type = ut;
                                s.rc.buffer_level = buf;
                                s.rc.bits_off_target = off;
                                // Give the slots distinct values so a wrong
                                // index picks a visibly wrong factor.
                                s.rc.rate_correction_factors = [0.4, 0.6, 0.8, 1.0, 1.2, 1.4, 1.6];
                                out.push(s);
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
fn regulate_q_matches_c() {
    if !cref::rc_vbr_statics_oracle_is_available() {
        // The caller decides (SVT_CREF_REQUIRE_RC_VBR_STATICS=1 makes the call
        // above panic); this branch cannot be reached under CI's setting.
        return;
    }
    let mut cells = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for s in scenarios() {
        for &(best, worst) in &[(0i32, 255i32), (40, 200), (100, 140), (130, 131)] {
            let mut st = to_ref(&s);
            let want = cref::regulate_q(
                &mut st,
                best,
                worst,
                s.frame.frame_width,
                s.frame.frame_height,
            )
            .expect("oracle available");
            let got = rcs::regulate_q(
                &s.rc,
                &s.cfg,
                &s.scs,
                &s.frame,
                best,
                worst,
                s.frame.frame_width,
                s.frame.frame_height,
            );
            assert_eq!(
                got,
                want,
                "regulate_q mismatch: mode={:?} bd={} ft={:?} sc={} tl={} buf={} ut={:?} \
                 best={best} worst={worst}",
                s.cfg.mode,
                s.scs.encoder_bit_depth,
                s.frame.frame_type,
                s.frame.sc_class1,
                s.frame.temporal_layer_index,
                s.rc.buffer_level,
                s.frame.update_type,
            );
            seen.insert(want);
            cells += 1;
        }
    }
    assert!(cells >= 2000, "sweep collapsed to {cells} cells");
    // Anti-vacuity: a differential against a constant proves nothing. The
    // floor is deliberately low — `regulate_q` clamps hard to
    // `[active_best, active_worst]` and three of the four bound pairs are
    // narrow, so a handful of distinct qindexes is the CORRECT behaviour here
    // and a high floor would be a false alarm rather than a real check.
    assert!(
        seen.len() >= 8,
        "the C oracle returned only {} distinct qindexes over {cells} cells",
        seen.len()
    );
}

#[test]
fn clamp_qindex_matches_c() {
    if !cref::rc_vbr_statics_oracle_is_available() {
        return;
    }
    let mut cells = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for min_qp in 0..=63i32 {
        for &max_qp in &[min_qp, 63.min(min_qp + 1), 63.min(min_qp + 20), 63] {
            for qindex in -8..=263i32 {
                let want = cref::clamp_qindex(min_qp, max_qp, qindex).expect("oracle");
                let scs = rcs::SeqRc {
                    min_qp_allowed: min_qp as u8,
                    max_qp_allowed: max_qp as u8,
                    ..Default::default()
                };
                let got = rcs::clamp_qindex(&scs, qindex);
                assert_eq!(got, want, "clamp_qindex({min_qp}, {max_qp}, {qindex})");
                seen.insert(want);
                cells += 1;
            }
        }
    }
    assert!(cells >= 60_000, "sweep collapsed to {cells} cells");
    assert!(
        seen.len() > 40,
        "oracle returned {} distinct values",
        seen.len()
    );
}

#[test]
fn update_rate_correction_factors_matches_c() {
    if !cref::rc_vbr_statics_oracle_is_available() {
        return;
    }
    let mut cells = 0usize;
    let mut moved = 0usize;
    for mut s in scenarios() {
        for &projected in &[1_000i32, 40_000, 80_000, 200_000] {
            for &cr_on in &[false, true] {
                s.frame.projected_frame_size = projected;
                s.cr.apply_cyclic_refresh = cr_on;
                s.cr.actual_num_seg1_sbs = if cr_on { 120 } else { 0 };
                s.cr.actual_num_seg2_sbs = if cr_on { 60 } else { 0 };
                s.cr.qindex_delta = if cr_on { [0, -20, -40] } else { [0; 3] };

                let mut st = to_ref(&s);
                assert!(cref::update_rate_correction_factors(
                    &mut st,
                    s.frame.frame_width,
                    s.frame.frame_height
                ));

                let mut rc = s.rc.clone();
                rcs::update_rate_correction_factors(
                    &mut rc,
                    &s.cfg,
                    &s.scs,
                    &s.frame,
                    &s.cr,
                    s.frame.frame_width,
                    s.frame.frame_height,
                );

                assert_eq!(
                    rc.rate_correction_factors,
                    st.rate_correction_factors,
                    "rate_correction_factors mismatch: mode={:?} ft={:?} projected={projected} \
                     cr={cr_on} tl={} ut={:?}",
                    s.cfg.mode,
                    s.frame.frame_type,
                    s.frame.temporal_layer_index,
                    s.frame.update_type,
                );
                assert_eq!((rc.q_1_frame, rc.q_2_frame), (st.q_1_frame, st.q_2_frame));
                assert_eq!(
                    (rc.rc_1_frame, rc.rc_2_frame),
                    (st.rc_1_frame, st.rc_2_frame)
                );
                assert_eq!(rc.percent_refresh_adjustment, st.percent_refresh_adjustment);
                assert_eq!(
                    rc.rate_ratio_qdelta_adjustment, st.rate_ratio_qdelta_adjustment,
                    "rate_ratio_qdelta_adjustment mismatch"
                );
                if rc.rate_correction_factors != s.rc.rate_correction_factors {
                    moved += 1;
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 2000, "sweep collapsed to {cells} cells");
    // Positive control: the whole point of the function is to MOVE the stored
    // factor. If no cell moved it, the sweep proved nothing.
    assert!(
        moved > cells / 10,
        "only {moved} of {cells} cells changed a rate correction factor — the sweep is not \
         exercising the feedback path"
    );
}

#[test]
fn regulated_q_overshoot_and_undershoot_match_c() {
    if !cref::rc_vbr_statics_oracle_is_available() {
        return;
    }
    let mut cells = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for s in scenarios() {
        for &(q_low, q_high, top, bottom) in &[
            (60i32, 200i32, 210i32, 40i32),
            (100, 120, 130, 90),
            (0, 255, 255, 0),
            (150, 160, 140, 20),
        ] {
            let mut st_o = to_ref(&s);
            let want_o = cref::get_regulated_q_overshoot(&mut st_o, q_low, q_high, top, bottom)
                .expect("oracle");
            let mut rc_o = s.rc.clone();
            let got_o = rcs::get_regulated_q_overshoot(
                &mut rc_o, &s.cfg, &s.scs, &s.frame, &s.cr, q_low, q_high, top, bottom,
            );
            assert_eq!(
                got_o, want_o,
                "get_regulated_q_overshoot mismatch: q_low={q_low} q_high={q_high} top={top} \
                 bottom={bottom} mode={:?} ft={:?}",
                s.cfg.mode, s.frame.frame_type,
            );
            assert_eq!(
                rc_o.rate_correction_factors, st_o.rate_correction_factors,
                "get_regulated_q_overshoot left different rate correction factors behind"
            );

            let mut st_u = to_ref(&s);
            let want_u =
                cref::get_regulated_q_undershoot(&mut st_u, q_high, top, bottom).expect("oracle");
            let mut rc_u = s.rc.clone();
            let got_u = rcs::get_regulated_q_undershoot(
                &mut rc_u, &s.cfg, &s.scs, &s.frame, &s.cr, q_high, top, bottom,
            );
            assert_eq!(
                got_u, want_u,
                "get_regulated_q_undershoot mismatch: q_high={q_high} top={top} bottom={bottom}"
            );
            assert_eq!(rc_u.rate_correction_factors, st_u.rate_correction_factors);

            seen.insert(want_o);
            seen.insert(want_u);
            cells += 1;
        }
    }
    assert!(cells >= 2000, "sweep collapsed to {cells} cells");
    assert!(
        seen.len() > 10,
        "oracle returned {} distinct values",
        seen.len()
    );
}

#[test]
fn resize_reset_rc_matches_c() {
    // No promotion needed — `svt_av1_resize_reset_rc` is an exported symbol,
    // so this cell is tier 1 on every host.
    //
    // It is ALSO the only route to `calc_active_worst_quality_no_stats_cbr`
    // (see the module header), so the sweep deliberately varies exactly the
    // state that function reads: `avg_frame_qindex[KEY]`/`[INTER]`,
    // `worst_quality`, `frame_updated` across its `< 4` boundary, and
    // `optimal_buffer_level` (which sets the critical level). If the port's
    // copy of it were wrong, the regulated qindex would differ and the
    // `rate_correction_factors[0]` nudges below would diverge.
    let mut cells = 0usize;
    let mut targets = std::collections::BTreeSet::new();
    let mut factors = std::collections::BTreeSet::new();
    for mut s in scenarios() {
        s.cfg.mode = rcs::AomRcMode::Cbr;
        for &(avg_key, avg_inter, worst) in &[
            (120i32, 130i32, 255i32),
            (30, 40, 255),
            (200, 210, 255),
            (60, 200, 220),
            (10, 12, 60),
        ] {
            for &fu in &[0i32, 5] {
                s.rc.avg_frame_qindex = [avg_key, avg_inter];
                s.rc.worst_quality = worst;
                s.rc.frame_updated = fu;
                for &(nw, nh, pw, ph) in &[
                    (960i32, 540i32, 1920i32, 1080i32),
                    (1920, 1080, 960, 540),
                    (1440, 810, 1920, 1080),
                    (1920, 1080, 480, 270),
                ] {
                    let mut st = to_ref(&s);
                    cref::resize_reset_rc(&mut st, nw, nh, pw, ph);

                    let mut rc = s.rc.clone();
                    // C reads `rc->last_q[INTER_FRAME]`; the shim mirrors it
                    // from `avg_frame_qindex[1]`, so the port reads the same.
                    rc.last_q[1] = s.rc.avg_frame_qindex[1];
                    let mut frame = s.frame;
                    rcs::resize_reset_rc(&mut rc, &s.cfg, &s.scs, &mut frame, nw, nh, pw, ph);

                    assert_eq!(
                        frame.this_frame_target, st.this_frame_target,
                        "resize_reset_rc this_frame_target mismatch: {nw}x{nh} from {pw}x{ph}"
                    );
                    assert_eq!(rc.buffer_level, st.buffer_level);
                    assert_eq!(rc.bits_off_target, st.bits_off_target);
                    assert_eq!(
                        rc.avg_frame_qindex, st.avg_frame_qindex,
                        "resize_reset_rc avg_frame_qindex mismatch: {nw}x{nh} from {pw}x{ph}"
                    );
                    assert_eq!(
                        rc.rate_correction_factors, st.rate_correction_factors,
                        "resize_reset_rc rate_correction_factors mismatch \
                         (this is where a wrong calc_active_worst_quality_no_stats_cbr shows \
                         up): {nw}x{nh} from {pw}x{ph}, avg_qi=[{avg_key},{avg_inter}], \
                         worst={worst}, frame_updated={fu}"
                    );
                    targets.insert(frame.this_frame_target);
                    factors.insert(rc.rate_correction_factors[0].to_bits());
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 20_000, "sweep collapsed to {cells} cells");
    // `this_frame_target` is by construction the same in every cell: the
    // function resets the buffer to optimal first, so the pframe target
    // reduces to `avg_frame_bandwidth`. That is the CORRECT answer, so the
    // anti-vacuity control is on the FACTORS, which is where the indirect
    // pin on calc_active_worst_quality_no_stats_cbr actually lands.
    assert_eq!(
        targets.len(),
        1,
        "unexpectedly variable target: {targets:?}"
    );
    // HOW COARSE THIS PIN IS, measured rather than assumed. `resize_reset_rc`
    // exposes `calc_active_worst_quality_no_stats_cbr` only through THREE
    // threshold tests on the regulated qindex (`> 90 % worst_quality`,
    // `> 130 % last_q`, `<= 120 % last_q`), each of which multiplies
    // `rate_correction_factors[0]` by a constant. So the reachable outcome set
    // is the base value plus a handful of products, and 23 040 cells produce
    // exactly 3 distinct values — that is the ceiling of this route, not a
    // harness defect. It proves the nudges are REACHED (trap #6: a control
    // that produces no change is only evidence once the code is shown to be
    // reached) and therefore that a `calc_active_worst_quality_no_stats_cbr`
    // wrong enough to move the qindex across one of those thresholds would be
    // caught. A wrong-by-one port would NOT be, which is why the function also
    // carries hand-derived tier-4 vectors for all three of its branches in
    // `port_rc_vbr_cbr_state`.
    assert!(
        factors.len() >= 3,
        "only {} distinct rate_correction_factors[0] over {cells} cells — the sweep is not \
         reaching the qindex-dependent nudges at all",
        factors.len()
    );
}
