use super::*;

/// The two ladders disagree at every preset a still encode can reach —
/// which is the whole reason the video arm needed its own wiring. KEY
/// frame, so the default arm's `is_base` is true.
#[test]
fn default_and_allintra_ladders_differ_on_a_key_frame() {
    let allintra: Vec<u8> = (0..=13)
        .map(|p| cdef_search_level_allintra(p, 0, ResolutionRange::R240p, 1, false, CONFIG_DEFAULT))
        .collect();
    let default: Vec<u8> = (0..=13)
        .map(|p| cdef_search_level_default(p, true, 1, false, CONFIG_DEFAULT))
        .collect();
    assert_eq!(
        allintra,
        vec![2, 3, 3, 3, 5, 5, 7, 10, 10, 10, 10, 10, 10, 10]
    );
    assert_eq!(default, vec![2, 2, 2, 5, 5, 5, 5, 5, 7, 7, 7, 7, 7, 7]);
}

/// The reference cell of chunk C1a (`docs/INTER-ENCODE-PLAN.md`): a
/// video-mode key frame at preset 6 searches level 5 (three primary
/// candidates, no row subsampling), where the still path searches level 7
/// (two candidates, every 4th row).
#[test]
fn video_key_frame_p6_is_level_5() {
    let lvl = cdef_search_level_default(6, true, 1, false, CONFIG_DEFAULT);
    assert_eq!(lvl, 5);
    let c = set_cdef_search_controls(lvl, true, true, crate::reference::SvtReference::Hybrid3115)
        .unwrap();
    assert_eq!(c.enabled, 1);
    assert!(!c.use_qp_strength);
    assert_eq!(c.first_pass_fs_num, 3);
    assert_eq!(&c.default_first_pass_fs[..3], &[0, 28, 60]);
    assert_eq!(c.default_second_pass_fs_num, 3);
    assert_eq!(&c.default_second_pass_fs[..3], &[2, 30, 62]);
    assert_eq!(c.subsampling_factor, 1);
    assert_eq!(c.search_best_ref_fs, 0);
    assert_eq!(c.skip_th, 0);
}

/// Every level C can assign is representable; 11+ is C's `assert(0)`.
#[test]
fn level_domain_is_zero_through_ten() {
    for lvl in 0..=10u8 {
        assert!(
            set_cdef_search_controls(lvl, true, true, crate::reference::SvtReference::Hybrid3115)
                .is_some(),
            "level {lvl}"
        );
    }
    assert!(
        set_cdef_search_controls(11, true, true, crate::reference::SvtReference::Hybrid3115)
            .is_none()
    );
}

/// Level 1 is the only one whose chroma second pass carries real
/// candidates.
#[test]
fn only_level_one_keeps_a_real_chroma_second_pass() {
    for lvl in 1..=9u8 {
        let c =
            set_cdef_search_controls(lvl, true, true, crate::reference::SvtReference::Hybrid3115)
                .unwrap();
        let n = c.default_second_pass_fs_num as usize;
        let real = (0..n).any(|i| c.default_second_pass_fs_uv[i] >= 0);
        assert_eq!(real, lvl == 1, "level {lvl}");
    }
}

// -----------------------------------------------------------------
// update_cdef_filters_on_ref_info (md_config_process.c:681-772)
// -----------------------------------------------------------------

/// The arm the campaign's first INTER frame takes, and the exact numbers
/// it takes it with.
///
/// `gradient 64x64 q40 p6`, frame 1 of a 2-frame low-delay-P GOP: level 5,
/// `is_not_highest_layer = false` (an LF_UPDATE), so
/// `search_best_ref_fs = 1`. Every DPB slot still holds the key frame, so
/// list 0 and list 1 are the SAME picture and both report the key frame's
/// signalled strengths: packed y = 2 (`pri 0, sec 2`) and uv = 28
/// (`pri 7, sec 0`). C must then take `use_reference_cdef_fs` and hand the
/// frame exactly those, with no search — which is what the C encoder's own
/// frame-1 header says (`docs/INTER-ENCODE-PLAN.md` §1r).
#[test]
fn ref_info_takes_the_use_reference_arm_when_both_lists_agree() {
    let mut c = set_cdef_search_controls(
        5,
        /*is_base=*/ false,
        /*is_not_highest_layer=*/ false,
        crate::reference::SvtReference::Hybrid3115,
    )
    .expect("level 5");
    assert_eq!(c.search_best_ref_fs, 1, "level 5 on a non-leaf-layer frame");
    let r = RefCdefStrengths {
        y0: 2,
        uv0: 28,
        y_min: 2,
        y_max: 2,
    };
    let out = update_cdef_filters_on_ref_info(&mut c, r, Some(r));
    assert_eq!(c.use_reference_cdef_fs, 1);
    assert_eq!(c.pred_y_f, 2, "the reference's own luma strength");
    assert_eq!(c.pred_uv_f, 28, "the MEAN of the two lists' chroma");
    assert_eq!(c.first_pass_fs_num, 0, "no search runs");
    assert_eq!(c.default_second_pass_fs_num, 0);
    assert!(!out.force_cdef_off);
}

/// A KEY frame can never reach the function: both flags are derived from
/// `!is_base` / `!is_not_highest_layer`, and a key frame has both true.
/// This is what makes the whole change byte-inert for the still envelope
/// BY CONSTRUCTION rather than only by measurement.
#[test]
fn a_key_frame_never_asks_for_a_reference_derived_set() {
    for lvl in 0..=10u8 {
        let c = set_cdef_search_controls(
            lvl,
            /*is_base=*/ true,
            /*is_not_highest_layer=*/ true,
            crate::reference::SvtReference::Hybrid3115,
        )
        .unwrap_or_else(|| panic!("level {lvl}"));
        assert_eq!(c.search_best_ref_fs, 0, "level {lvl}");
        assert_eq!(c.use_reference_cdef_fs, 0, "level {lvl}");
    }
}

/// Two DIFFERENT references: the candidate list grows to three (the
/// level's default plus each list's), the search still runs, and
/// `use_reference_cdef_fs` stays off.
#[test]
fn ref_info_grows_the_candidate_list_when_the_lists_disagree() {
    let mut c =
        set_cdef_search_controls(5, false, false, crate::reference::SvtReference::Hybrid3115)
            .expect("level 5");
    let default0 = c.default_first_pass_fs[0];
    let a = RefCdefStrengths {
        y0: default0.wrapping_add(1),
        uv0: 5,
        y_min: default0.wrapping_add(1),
        y_max: default0.wrapping_add(1),
    };
    let b = RefCdefStrengths {
        y0: default0.wrapping_add(2),
        uv0: 9,
        y_min: default0.wrapping_add(2),
        y_max: default0.wrapping_add(2),
    };
    let out = update_cdef_filters_on_ref_info(&mut c, a, Some(b));
    assert_eq!(c.use_reference_cdef_fs, 0);
    assert_eq!(c.first_pass_fs_num, 3);
    assert_eq!(c.default_first_pass_fs[1], a.y0);
    assert_eq!(c.default_first_pass_fs[2], b.y0);
    assert!(!out.force_cdef_off);
}

/// One list only, and its filter IS the level's default: nothing is added,
/// the count stays 1, and C switches CDEF off for the frame
/// ("Set cdef to off if pred luma is", `md_config_process.c:768`). The
/// test is on the candidate COUNT, not on a strength value.
#[test]
fn ref_info_forces_cdef_off_when_only_the_default_survives() {
    let mut c =
        set_cdef_search_controls(5, false, false, crate::reference::SvtReference::Hybrid3115)
            .expect("level 5");
    let same = RefCdefStrengths {
        y0: c.default_first_pass_fs[0],
        uv0: 3,
        y_min: c.default_first_pass_fs[0],
        y_max: c.default_first_pass_fs[0],
    };
    let out = update_cdef_filters_on_ref_info(&mut c, same, None);
    assert_eq!(c.first_pass_fs_num, 1);
    assert!(out.force_cdef_off);
}
