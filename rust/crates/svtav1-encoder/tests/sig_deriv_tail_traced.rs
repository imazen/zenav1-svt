//! **Evidence tier 4** (`docs/WORKING-ON-THIS.md` §4 — the WEAKEST tier):
//! hand-derived vectors traced against `Source/Lib/Codec/enc_mode_config.c`
//! for the five remaining tables, every one of which is file-`static` in C
//! with NO exported symbol to differential against.
//!
//! Why tier 4 here and not tier 1 as elsewhere in this lane: the exported entry
//! points that reach these tables are
//! `svt_aom_sig_deriv_multi_processes_default` (`set_cdef_recon_controls`, the
//! SGR level) and `svt_aom_sig_deriv_enc_dec_pd0` (the two PD0 thresholds),
//! neither of which this lane has ported — driving one of them would need a
//! synthetic PPCS with the whole picture-level state populated, and a shim
//! built on guessed state would be a worse oracle than the source itself.
//! When either entry point lands, these move to tier 1.

use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::tail;

/// TIER 4. `svt_aom_set_sg_filter_ctrls` (`enc_mode_config.c:1295`).
#[test]
fn sg_filter_ctrls_traced_vectors() {
    let l0 = tail::set_sg_filter_ctrls(0).expect("level 0");
    assert!(!l0.enabled);
    // Level 0 writes ONLY `enabled`; everything else stays zeroed.
    assert_eq!(l0.start_ep, [0, 0]);
    assert_eq!(l0.end_ep, [0, 0]);
    assert_eq!(l0.ep_inc, [0, 0]);
    assert_eq!(l0.refine, [0, 0]);
    assert!(!l0.use_chroma);

    let l1 = tail::set_sg_filter_ctrls(1).expect("level 1");
    assert!(l1.enabled && l1.use_chroma);
    assert_eq!(
        (l1.start_ep, l1.end_ep, l1.ep_inc, l1.refine),
        ([0, 0], [16, 16], [1, 1], [1, 1])
    );

    let l2 = tail::set_sg_filter_ctrls(2).expect("level 2");
    assert_eq!(
        (l2.start_ep, l2.end_ep, l2.ep_inc, l2.refine),
        ([0, 4], [16, 5], [1, 1], [1, 0])
    );

    // Level 3 differs from level 2 ONLY in the luma increment.
    let l3 = tail::set_sg_filter_ctrls(3).expect("level 3");
    assert_eq!(l3.ep_inc, [8, 1]);
    assert_eq!(
        (l3.start_ep, l3.end_ep, l3.refine),
        (l2.start_ep, l2.end_ep, l2.refine)
    );
    assert!(l3.use_chroma);

    // Level 4 differs from level 3 ONLY in use_chroma.
    let l4 = tail::set_sg_filter_ctrls(4).expect("level 4");
    assert!(!l4.use_chroma);
    assert_eq!(
        (l4.start_ep, l4.end_ep, l4.ep_inc, l4.refine),
        (l3.start_ep, l3.end_ep, l3.ep_inc, l3.refine)
    );

    assert!(tail::set_sg_filter_ctrls(5).is_none());
}

/// TIER 4. `set_cdef_recon_controls` (`enc_mode_config.c:1200`).
#[test]
fn cdef_recon_controls_traced_vectors() {
    let expect: [(u8, u16, u8, u16); 5] = [
        (0, 0, 0, 0),
        (1, 61, 2, 10),
        (2, 61, 3, 10),
        (3, 60, 3, 10),
        (4, 58, 3, 10),
    ];
    for &(lvl, bias, strength, dist) in &expect {
        let c = tail::set_cdef_recon_controls(lvl).expect("level in range");
        assert_eq!(
            (
                c.zero_fs_cost_bias,
                c.zero_filter_strength_lvl,
                c.prev_cdef_dist_th
            ),
            (bias, strength, dist),
            "cdef recon level={lvl}"
        );
    }
    assert!(tail::set_cdef_recon_controls(5).is_none());
}

/// TIER 4. The level ladder at `enc_mode_config.c:2102` that selects the table
/// above. Note the fast-decode-2 arm INVERTS the preset test: `<= M8` gives 2
/// there and 0 on the fast-decode-0 arm.
#[test]
fn cdef_recon_level_ladder_traced() {
    for m in -1i8..=13 {
        // fast_decode 0: off up to M8, then 1, then 2.
        let want0 = if m <= 8 {
            0
        } else if m <= 10 {
            1
        } else {
            2
        };
        assert_eq!(
            tail::cdef_recon_level_default(m, 0, ResolutionRange::R1080p),
            want0,
            "fast_decode=0 enc_mode={m}"
        );
        // fast_decode 1 is a flat 1 at every preset...
        assert_eq!(
            tail::cdef_recon_level_default(m, 1, ResolutionRange::R1080p),
            1,
            "fast_decode=1 enc_mode={m}"
        );
        // ...but at 360p and below, ANY fast_decode takes the first arm.
        assert_eq!(
            tail::cdef_recon_level_default(m, 1, ResolutionRange::R360p),
            want0,
            "fast_decode=1 at 360p enc_mode={m}"
        );
        // fast_decode 2 inverts: 2 at <= M8, 1 above.
        assert_eq!(
            tail::cdef_recon_level_default(m, 2, ResolutionRange::R1080p),
            if m <= 8 { 2 } else { 1 },
            "fast_decode=2 enc_mode={m}"
        );
    }
}

/// TIER 4. `compute_intra_pd0_th` / `compute_subres_th`
/// (`enc_mode_config.c:6279` and `:6290`), through the `RDCOST` macro.
#[test]
fn pd0_thresholds_traced_vectors() {
    // RDCOST(RM, R, D) = ROUND_POWER_OF_TWO(R*RM, 9) + (D << 7), with
    // R = 1 << 13 and D = sb_size * 6.
    for &(fast_lambda, sb) in &[(0u32, 64u32), (1, 64), (100, 64), (5000, 128), (65535, 64)] {
        let sb_size = i64::from(sb) * i64::from(sb);
        let want = tail::round_power_of_two((1i64 << 13) * i64::from(fast_lambda), 9)
            + ((sb_size * 6) << 7);
        assert_eq!(
            tail::compute_intra_pd0_th(fast_lambda, sb),
            want as u64,
            "intra_pd0_th lambda={fast_lambda} sb={sb}"
        );
        assert_eq!(
            tail::compute_subres_th(fast_lambda, sb),
            want as u64,
            "subres_th lambda={fast_lambda} sb={sb}"
        );
    }
    // The rounding is C's add-then-shift, not a round-half-away-from-zero:
    // ROUND_POWER_OF_TWO(x, n) == (x + (1<<n >> 1)) >> n.
    assert_eq!(tail::round_power_of_two(0, 9), 0);
    assert_eq!(tail::round_power_of_two(255, 9), 0);
    assert_eq!(tail::round_power_of_two(256, 9), 1);
    assert_eq!(tail::round_power_of_two(-256, 9), 0);
}

/// TIER 4. `mfmv_controls` (`enc_mode_config.c:8853`).
#[test]
fn mfmv_controls_traced_vectors() {
    let base = tail::MfmvInputs {
        mfmv_level: 0,
        is_base: true,
        tpl: true,
        r0_gen: true,
        r0: 0.05,
        is_b_slice: true,
        ref_list1_count_try: 1,
        ref_l0_is_mfmv_used: false,
        ref_l1_is_mfmv_used: false,
    };
    // Level 0 is a flat off, level 1 a flat on, regardless of everything else.
    assert_eq!(
        tail::mfmv_controls(tail::MfmvInputs {
            mfmv_level: 0,
            ..base
        }),
        Some(0)
    );
    assert_eq!(
        tail::mfmv_controls(tail::MfmvInputs {
            mfmv_level: 1,
            ..base
        }),
        Some(1)
    );

    // Levels 2/3/4 use an r0 threshold of 0.15 / 0.13 / 0.10 WHEN TPL IS ON.
    for &(lvl, th) in &[(2u8, 0.15f64), (3, 0.13), (4, 0.10)] {
        // r0 below the threshold on a base picture turns it on.
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th - 0.01,
                ..base
            }),
            Some(1),
            "level={lvl} r0 below th"
        );
        // r0 above it does not.
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th + 0.01,
                ..base
            }),
            Some(0),
            "level={lvl} r0 above th"
        );
        // A non-base picture never gets it from r0...
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th - 0.01,
                is_base: false,
                ..base
            }),
            Some(0),
            "level={lvl} non-base"
        );
        // ...but inherits it from either closest reference.
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th + 0.01,
                is_base: false,
                ref_l0_is_mfmv_used: true,
                ..base
            }),
            Some(1),
            "level={lvl} inherits from L0"
        );
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th + 0.01,
                is_base: false,
                ref_l1_is_mfmv_used: true,
                ..base
            }),
            Some(1),
            "level={lvl} inherits from L1"
        );
        // L1 inheritance needs a B slice AND a nonzero ref_list1_count_try.
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th + 0.01,
                is_base: false,
                is_b_slice: false,
                ref_l1_is_mfmv_used: true,
                ..base
            }),
            Some(0),
            "level={lvl} L1 needs a B slice"
        );
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                r0: th + 0.01,
                is_base: false,
                ref_list1_count_try: 0,
                ref_l1_is_mfmv_used: true,
                ..base
            }),
            Some(0),
            "level={lvl} L1 needs ref_list1_count_try"
        );
        // With TPL OFF the threshold is 0, C's `if (r0_th)` is false, and the
        // whole reference-inheritance block is skipped -- so even a reference
        // that used mfmv does not turn it on.
        assert_eq!(
            tail::mfmv_controls(tail::MfmvInputs {
                mfmv_level: lvl,
                tpl: false,
                r0: 0.0,
                ref_l0_is_mfmv_used: true,
                ..base
            }),
            Some(0),
            "level={lvl} TPL off zeroes the threshold"
        );
    }
    assert_eq!(
        tail::mfmv_controls(tail::MfmvInputs {
            mfmv_level: 5,
            ..base
        }),
        None
    );
}

/// TIER 4. `get_sb_tpl_intra_stats` (`enc_mode_config.c:6480`).
///
/// The live arm depends on the unported TPL vertical; this exercises the gate
/// and the buffer walk, NOT a production path.
#[test]
fn tpl_intra_stats_traced_vectors() {
    let blocks: Vec<tail::TplSrcBlock> = (0..64)
        .map(|i| tail::TplSrcBlock {
            // Cycle DC(0), V(1, directional), PAETH(12, intra non-directional),
            // NEARESTMV(13, inter).
            best_mode: [0u8, 1, 12, 13][i % 4],
        })
        .collect();
    let base = tail::TplIntraStatsInputs {
        tpl_enable: true,
        tpl_src_data_ready: true,
        temporal_layer_index: 0,
        hierarchical_levels: 4,
        disable_intra_pred_nref: false,
        aligned_width: 64,
        sb_origin_x: 0,
        sb_origin_y: 0,
        dispenser_search_level: 0,
        sb_width: 64,
        sb_height: 64,
    };
    // TPL off -> no stats at all (C returns 0).
    assert!(
        tail::get_sb_tpl_intra_stats(
            tail::TplIntraStatsInputs {
                tpl_enable: false,
                ..base
            },
            &blocks
        )
        .is_none()
    );
    assert!(
        tail::get_sb_tpl_intra_stats(
            tail::TplIntraStatsInputs {
                tpl_src_data_ready: false,
                ..base
            },
            &blocks
        )
        .is_none()
    );
    // The third conjunct: a non-reference picture at the highest layer is
    // excluded only when disable_intra_pred_nref is set.
    assert!(
        tail::get_sb_tpl_intra_stats(
            tail::TplIntraStatsInputs {
                temporal_layer_index: 4,
                disable_intra_pred_nref: true,
                ..base
            },
            &blocks
        )
        .is_none()
    );
    assert!(
        tail::get_sb_tpl_intra_stats(
            tail::TplIntraStatsInputs {
                temporal_layer_index: 4,
                disable_intra_pred_nref: false,
                ..base
            },
            &blocks
        )
        .is_some()
    );

    // 64x64 SB at dispenser level 0 -> 16x16 blocks -> 4x4 = 16 samples, step 1,
    // aligned16_width = 4. Rows start at 0, 4, 8, 12 -> the modes at those
    // indices with the 4-cycle above are all DC/V/PAETH/NEARESTMV in order.
    let s = tail::get_sb_tpl_intra_stats(base, &blocks).expect("stats available");
    assert_eq!(s.intra_count, 12, "3 of every 4 sampled modes are intra");
    assert_eq!(s.ang_intra_count, 4, "1 of every 4 is V_PRED");
    assert_eq!(s.max_intra, 12, "PAETH_PRED is the highest intra seen");

    // Dispenser level 2 -> 64x64 blocks -> a single sample at index 0 (DC).
    let s2 = tail::get_sb_tpl_intra_stats(
        tail::TplIntraStatsInputs {
            dispenser_search_level: 2,
            ..base
        },
        &blocks,
    )
    .expect("stats available");
    assert_eq!(
        (s2.intra_count, s2.ang_intra_count, s2.max_intra),
        (1, 0, 0)
    );

    // An SB narrower than the block size still samples one column (C's MAX(1,..)).
    let s3 = tail::get_sb_tpl_intra_stats(
        tail::TplIntraStatsInputs {
            sb_width: 8,
            sb_height: 8,
            ..base
        },
        &blocks,
    )
    .expect("stats available");
    assert_eq!(s3.intra_count, 1);
}

/// The two mode predicates the stat reader uses, against the C enum values
/// (`definitions.h:1188`): DC_PRED 0, V_PRED 1, D67_PRED 8, PAETH_PRED 12,
/// NEARESTMV 13 == INTRA_MODE_END.
#[test]
fn mode_predicates_match_the_c_enum() {
    for m in 0u8..=12 {
        assert!(tail::is_intra_mode(m), "mode {m} is intra");
    }
    assert!(!tail::is_intra_mode(13), "NEARESTMV is not intra");
    for m in 1u8..=8 {
        assert!(tail::av1_is_directional_mode(m), "mode {m} is directional");
    }
    assert!(
        !tail::av1_is_directional_mode(0),
        "DC_PRED is not directional"
    );
    assert!(
        !tail::av1_is_directional_mode(9),
        "SMOOTH_PRED is not directional"
    );
    assert!(
        !tail::av1_is_directional_mode(12),
        "PAETH_PRED is not directional"
    );
}
