use super::*;
use crate::port_enc_mode_config::ctrls::set_dlf_controls;

fn base<'a>(refs: &'a [RefDlfState], lvl: u8) -> DlfPickInputs<'a> {
    DlfPickInputs {
        ctrls: set_dlf_controls(lvl).expect("a level in 0..=7"),
        frame_type_is_key: false,
        is_intra_slice: false,
        frame_is_boosted: false,
        frame_is_leaf: true,
        hierarchical_levels: 0,
        temporal_layer_index: 0,
        input_resolution: ResolutionRange::R240p,
        refs,
        avg_me_sad: 1_000_000,
        base_qindex: 160,
        bit_depth: 8,
    }
}

/// The campaign's level-3 arm: the reference's levels are COPIED, and no
/// search runs. Measured counterpart: C's `diag 72x72 q40 p6` frame 1
/// carries frame 0's 12 (`docs/INTER-ENCODE-PLAN.md` §1z²¹).
#[test]
fn level3_copies_the_reference_levels_without_searching() {
    let refs = [RefDlfState {
        filter_level: [12, 12, 0, 0],
        dlf_dist_dev: 40,
    }];
    let i = base(&refs, 3);
    assert_eq!(i.ctrls.dlf_avg, true);
    assert_eq!(i.ctrls.use_ref_avg_y, true);
    let mut searched = 0;
    let p = pick_filter_level_full_image(&i, |_, _, _| {
        searched += 1;
        (63, 0, 0)
    });
    assert_eq!(p.levels.levels, [12, 12, 0, 0]);
    assert_eq!(searched, 0, "the ref-average arms must not call the search");
    assert_eq!((p.zero_filt_sse, p.best_filt_sse), (-1, -1));
}

/// The same arm with a reference whose own deblock barely helped: C
/// declines to filter. Measured counterpart: `gradient 72x72 q40 p6`,
/// frame 0 level 8 -> frame 1 level 0.
#[test]
fn level3_zeroes_when_the_references_dlf_gain_was_under_five() {
    let refs = [RefDlfState {
        filter_level: [8, 8, 4, 4],
        dlf_dist_dev: 4,
    }];
    let p =
        pick_filter_level_full_image(&base(&refs, 3), |_, _, _| panic!("no search on this arm"));
    assert_eq!(p.levels.levels, [0, 0, 0, 0], "chroma follows luma to 0");

    // The sentinel is NOT a small number: a reference coded with
    // sb_based_dlf never computed the deviation, and C skips it rather
    // than reading -1 as "under 5".
    let refs = [RefDlfState {
        filter_level: [8, 8, 4, 4],
        dlf_dist_dev: -1,
    }];
    let p =
        pick_filter_level_full_image(&base(&refs, 3), |_, _, _| panic!("no search on this arm"));
    assert_eq!(p.levels.levels, [8, 8, 4, 4]);
}

/// A KEY frame reduces to the still path: no references, so `dlf_avg` is
/// inert, `frame_is_boosted` forces the luma search, and chroma searches
/// whenever luma is nonzero. This is what lets
/// `deblock::pick_filter_levels_full_search` delegate here without
/// moving a still byte.
#[test]
fn a_key_frame_takes_the_search_arms_only() {
    let mut i = base(&[], 3);
    i.frame_type_is_key = true;
    i.is_intra_slice = true;
    i.frame_is_boosted = true;
    i.frame_is_leaf = false;
    let mut seen: alloc::vec::Vec<(usize, i32, [i32; 4])> = alloc::vec::Vec::new();
    let p = pick_filter_level_full_image(&i, |plane, dir, last| {
        seen.push((plane, dir, last));
        (7, 99, 11)
    });
    assert_eq!(p.levels.levels, [7, 7, 7, 7]);
    assert_eq!(seen, [(0, 2, [0; 4]), (1, 0, [0; 4]), (2, 0, [0; 4])]);
    assert_eq!((p.zero_filt_sse, p.best_filt_sse), (99, 11));
}

/// Level 6's `zero_filter_strength_lvl = 2` is what makes the by-q arm
/// motion-sensitive, and the two thresholds are `th` and `2 * th` — the
/// state C shows on `screen 16x16 q40 p8`, luma 9 with chroma 0.
#[test]
fn level6_by_q_has_a_luma_threshold_and_a_separate_chroma_one() {
    let refs = [RefDlfState {
        filter_level: [3, 3, 1, 1],
        dlf_dist_dev: -1,
    }];
    // disable_dlf_th[2][R240p] = 900, mult = 3 (leaf, flat) -> 2700.
    let mut i = base(&refs, 6);
    assert_eq!(i.ctrls.zero_filter_strength_lvl, 2);

    i.avg_me_sad = 2699;
    assert_eq!(pick_filter_level_by_q(&i).levels, [0, 0, 0, 0], "below th");

    i.avg_me_sad = 2700;
    let l = pick_filter_level_by_q(&i).levels;
    assert_eq!((l[0], l[2]), (9, 0), "luma on, chroma still below 2*th");

    i.avg_me_sad = 5400;
    assert_eq!(pick_filter_level_by_q(&i).levels, [9, 9, 4, 4], "both on");
}

/// A reference whose loop filter was OFF forces this frame's off too,
/// unless the frame is boosted. C's "loop-filter is shut for one of the
/// sub-layer reference frames" rule (`deblocking_filter.c:1097`).
#[test]
fn by_q_min_ref_zero_shuts_the_filter_off() {
    let refs = [RefDlfState {
        filter_level: [0, 0, 0, 0],
        dlf_dist_dev: -1,
    }];
    let mut i = base(&refs, 6);
    assert_eq!(pick_filter_level_by_q(&i).levels, [0, 0, 0, 0]);
    i.frame_is_boosted = true;
    assert_eq!(
        pick_filter_level_by_q(&i).levels,
        [9, 9, 4, 4],
        "boosted overrides the reference's zero"
    );
}

/// `mult` is a function of the update type on a FLAT GOP and of the
/// temporal layer otherwise — C's own comment says so, and getting it
/// wrong scales the threshold by 3x.
#[test]
fn me_skip_mult_follows_the_update_type_only_when_the_gop_is_flat() {
    let refs = [RefDlfState {
        filter_level: [3, 3, 1, 1],
        dlf_dist_dev: -1,
    }];
    // Every assertion below is at a sad value that DISCRIMINATES the
    // mult it is testing from the neighbouring one — a threshold test
    // that passes at every mult tests nothing.
    let mut i = base(&refs, 6);
    // flat + leaf -> mult 3 -> th 2700, 2*th 5400.
    i.avg_me_sad = 2699;
    assert_eq!(me_based_dlf_skip(&i), (false, false));
    i.avg_me_sad = 2700;
    assert_eq!(me_based_dlf_skip(&i), (true, false));
    i.avg_me_sad = 5400;
    assert_eq!(me_based_dlf_skip(&i), (true, true));
    // flat + non-leaf, non-boosted -> mult 2 -> th 1800. 1800 is BELOW
    // the leaf threshold, so this fails if `frame_is_leaf` is ignored.
    i.frame_is_leaf = false;
    i.avg_me_sad = 1800;
    assert_eq!(me_based_dlf_skip(&i), (true, false));
    i.avg_me_sad = 1799;
    assert_eq!(me_based_dlf_skip(&i), (false, false));
    // flat + boosted -> mult 1 -> th 900, below the non-leaf threshold.
    i.frame_is_boosted = true;
    i.avg_me_sad = 900;
    assert_eq!(me_based_dlf_skip(&i), (true, false));
    i.avg_me_sad = 899;
    assert_eq!(me_based_dlf_skip(&i), (false, false));
    // hierarchy -> mult = temporal_layer_index + 1, and the update type
    // is IGNORED: boosted would be mult 1 on a flat GOP, this is mult 4.
    i.hierarchical_levels = 3;
    i.temporal_layer_index = 3;
    i.avg_me_sad = 3599;
    assert_eq!(me_based_dlf_skip(&i), (false, false), "th 3600");
    i.avg_me_sad = 3600;
    assert_eq!(me_based_dlf_skip(&i), (true, false));
    i.avg_me_sad = 7200;
    assert_eq!(me_based_dlf_skip(&i), (true, true), "2*th 7200");
}

/// An I_SLICE returns before the table lookup, which is what keeps every
/// still cell out of this function's reach.
#[test]
fn an_i_slice_never_skips() {
    let mut i = base(&[], 6);
    i.is_intra_slice = true;
    i.avg_me_sad = 0;
    assert_eq!(me_based_dlf_skip(&i), (true, true));
}

/// `dlf_dist_dev`'s two zero-producing corners and its arithmetic.
#[test]
fn dlf_dist_dev_matches_cs_three_arms() {
    let off = LfLevels { levels: [0; 4] };
    let on = LfLevels {
        levels: [8, 8, 0, 0],
    };
    assert_eq!(dlf_dist_dev(off, 1000, 500), 0, "no filtering -> 0");
    assert_eq!(dlf_dist_dev(on, 0, 0), 0, "zero SSE -> 0");
    assert_eq!(dlf_dist_dev(on, 1000, 900), 100);
    assert_eq!(dlf_dist_dev(on, 1000, 996), 4, "the < 5 boundary");
    assert_eq!(dlf_dist_dev(on, 1000, 995), 5);
}
