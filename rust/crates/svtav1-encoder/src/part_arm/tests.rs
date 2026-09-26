use super::*;

/// The predicate `pipeline.rs` carried inline before this module existed,
/// kept VERBATIM as the regression oracle for the still path.
fn old_flattened_cap(preset: i8, full_sb: bool) -> bool {
    preset >= 8 && full_sb
}

/// Ditto for the NSQ-geometry predicate (two sites, same expression).
fn old_flattened_geom_enabled(preset: i8) -> bool {
    preset <= 6
}

/// Ditto for `NsqCfg::for_preset_qp`'s level derivation — the base table
/// plus the seq-qp-mod offsets, transcribed from the function as it stood.
fn old_flattened_search_level(preset: i8, cli_qp: u32) -> u8 {
    let base: i32 = match preset {
        0 => 3,
        1 => 10,
        2 => 14,
        3 => 16,
        _ => 0,
    };
    if base == 0 {
        return 0;
    }
    let mut level = base;
    if cli_qp <= 39 {
        level = if level + 3 > 19 { 0 } else { level + 3 };
    } else if cli_qp <= 45 {
        level = if level + 2 > 19 { 0 } else { level + 2 };
    } else if cli_qp <= 48 {
        level = if level + 1 > 19 { 0 } else { level + 1 };
    } else if cli_qp > 59 {
        level = (level - 1).max(1);
    }
    level as u8
}

#[test]
fn allintra_flattening_matches_the_ladder() {
    for preset in 0i8..=13 {
        for full_sb in [false, true] {
            assert_eq!(
                max_block_cap_active(ScArm::Allintra, preset, full_sb),
                old_flattened_cap(preset, full_sb),
                "max-block cap p{preset} full_sb={full_sb}"
            );
        }
        assert_eq!(
            nsq_geom_enabled(
                ScArm::Allintra,
                preset,
                crate::reference::SvtReference::Hybrid3115
            ),
            old_flattened_geom_enabled(preset),
            "nsq geom p{preset}"
        );
        // The flattened tail assumed factors of 1/1 unconditionally. That
        // is only sound where the still path actually builds an NsqCfg —
        // presets 0..=3, the band where the allintra search ladder is
        // non-zero. Pin BOTH halves: the flag is off there, and the ladder
        // is off wherever the flag is on.
        if nsq_search_level(ScArm::Allintra, preset, 40) != 0 {
            assert!(
                !nsq_qp_based_th_scaling(ScArm::Allintra, preset),
                "still tail must stay unscaled at p{preset}"
            );
        }
        for cli_qp in 0u32..=63 {
            assert_eq!(
                nsq_search_level(ScArm::Allintra, preset, cli_qp),
                old_flattened_search_level(preset, cli_qp),
                "nsq search p{preset} q{cli_qp}"
            );
        }
    }
}

/// The allintra geom levels the flattening implied: 2 at presets 0..=3
/// (`allow_HV4 = 1`, `min_nsq = 0` — exactly the pair `NsqCfg` hardcoded),
/// 3 at 4..=6, 0 above.
#[test]
fn allintra_geom_ctrls_match_the_hardcoded_pair() {
    for preset in 0i8..=3 {
        assert_eq!(
            nsq_geom_level(
                ScArm::Allintra,
                preset,
                crate::reference::SvtReference::Hybrid3115
            ),
            2
        );
        assert_eq!(nsq_geom_shape_ctrls(2), (true, 0));
    }
    for preset in 4i8..=6 {
        assert_eq!(
            nsq_geom_level(
                ScArm::Allintra,
                preset,
                crate::reference::SvtReference::Hybrid3115
            ),
            3
        );
    }
    for preset in 7i8..=13 {
        assert_eq!(
            nsq_geom_level(
                ScArm::Allintra,
                preset,
                crate::reference::SvtReference::Hybrid3115
            ),
            0
        );
    }
}

/// Where the video arm actually departs from the still one — the whole
/// point of the chunk, recorded so a future edit that flattens it back is
/// a test failure rather than a silent regression.
#[test]
fn video_arm_departs_where_expected() {
    let v = ScArm::Video { is_islice: true };
    // max_block_size: the still arm caps at M8+, the video arm never caps.
    for preset in 0i8..=13 {
        assert!(
            !max_block_cap_active(v, preset, true),
            "video cap p{preset}"
        );
    }
    assert!(max_block_cap_active(ScArm::Allintra, 8, true));

    // NSQ geometry: the still arm switches OFF above M6, the video arm
    // never does (`get_nsq_geom_level_default` returns 1/2/3 only).
    for preset in 0i8..=13 {
        assert!(
            nsq_geom_enabled(v, preset, crate::reference::SvtReference::Hybrid3115),
            "video geom p{preset}"
        );
    }
    assert!(!nsq_geom_enabled(
        ScArm::Allintra,
        7,
        crate::reference::SvtReference::Hybrid3115
    ));

    // NSQ search: the still arm is OFF from M4 up at EVERY qp (the
    // allintra base table is 0 there and the offsets short-circuit on 0),
    // while the video arm keeps searching.
    for preset in 4i8..=13 {
        for qp in 0u32..=63 {
            assert_eq!(
                nsq_search_level(ScArm::Allintra, preset, qp),
                0,
                "still search p{preset} q{qp}"
            );
        }
    }
    // At q40 the video arm's `qp <= 43` offset is +2, which pushes M7's
    // base 18 and M8+'s 19 over 19 and back to 0 — NSQ search off. That is
    // C's own saturation rule (`level + 2 > 19 ? 0 : ...`), not a port
    // shortcut, so the departure shows at 4..=6 here...
    for preset in 4i8..=6 {
        assert_ne!(nsq_search_level(v, preset, 40), 0, "video search p{preset}");
    }
    // ...and at q55, where no offset applies, it extends to the top.
    for preset in 4i8..=13 {
        assert_ne!(
            nsq_search_level(v, preset, 55),
            0,
            "video search p{preset} q55"
        );
    }
}

/// The video ladder resolved at cli qp 40 (seq_qp_mod 2), spelled out so a
/// change to either the base table or the offset arm shows up as a diff in
/// a table rather than in behaviour only.
///
/// Base (`svt_aom_get_nsq_search_level_default`, :8254): M0 2
/// (`temporal_layer_index == 0` -> `is_base`), M1..M2 7, M3 9, M4 12,
/// M5..M6 15, M7 18, M8+ 19. Then the seq-qp offset: M0..M6 take the
/// `qp <= 45` arm (+2), M7+ the `qp <= 43` arm (also +2 at 40) — and the
/// `level + 2 > 19 ? 0` saturation turns M7's 18 and M8+'s 19 into NSQ
/// search OFF.
#[test]
fn video_search_levels_at_q40() {
    let v = ScArm::Video { is_islice: true };
    let expect = [4u8, 9, 9, 11, 14, 17, 17, 0, 0, 0, 0, 0, 0, 0];
    for (preset, want) in expect.iter().enumerate() {
        assert_eq!(nsq_search_level(v, preset as i8, 40), *want, "p{preset}");
    }
}

/// The same ladder at cli qp 55, where NO seq-qp offset applies (55 is
/// above every `<=` bound and not `> 56`), so the base table shows
/// through unmodified.
#[test]
fn video_search_levels_at_q55() {
    let v = ScArm::Video { is_islice: true };
    let expect = [2u8, 7, 7, 9, 12, 15, 15, 18, 19, 19, 19, 19, 19, 19];
    for (preset, want) in expect.iter().enumerate() {
        assert_eq!(nsq_search_level(v, preset as i8, 55), *want, "p{preset}");
    }
}

#[test]
fn video_search_level_qp_arm_split_at_q44() {
    let v = ScArm::Video { is_islice: true };
    // M6 takes the `qp <= 45` arm: 15 + 2 = 17.
    assert_eq!(nsq_search_level(v, 6, 44), 17);
    // M7 takes the `qp <= 43` arm, so 44 falls through to `qp <= 48`:
    // 18 + 1 = 19.
    assert_eq!(nsq_search_level(v, 7, 44), 19);
}
