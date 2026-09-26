use super::*;

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

fn cand(direction: u8, l0: u8, l1: u8, r0l: u8, r1l: u8) -> MeCandidateRef {
    MeCandidateRef {
        direction,
        ref_idx_l0: l0,
        ref_idx_l1: l1,
        ref0_list: r0l,
        ref1_list: r1l,
    }
}

/// TIER 4 — the SB-64 arm takes `sb_ptr->index` verbatim and computes
/// no geometry offsets; the SB-128 arm recomputes the address over
/// the b64 grid and DOES.
#[test]
fn tier4_derive_me_offsets_two_arms() {
    // SB-64: me_sb_addr is the caller's sb index, offsets stay 0.
    let a = derive_me_offsets(false, 64, 1920, 7, 64, 64, 9, true, true, 12);
    assert_eq!(a.me_sb_addr, 7);
    assert_eq!(a.geom_offset_x, 0);
    assert_eq!(a.geom_offset_y, 0);
    assert_eq!(a.me_cand_offset, a.me_block_offset * 12);

    // SB-128 at (128, 64) with b64 = 64 and aligned_width 1920:
    // pic_width_in_b64 = 30, me_sb_x = 2, me_sb_y = 1 -> addr 32,
    // geom offsets (2 & 1) * 64 = 0 and (1 & 1) * 64 = 64.
    let b = derive_me_offsets(true, 64, 1920, 7, 128, 64, 9, true, true, 12);
    // me_sb_x = 2, me_sb_y = 1, pic_width_in_b64 = 30.
    assert_eq!(b.me_sb_addr, 32);
    assert_eq!(b.geom_offset_x, 0);
    assert_eq!(b.geom_offset_y, 64);

    // An odd b64 column raises geom_offset_x instead.
    let c = derive_me_offsets(true, 64, 1920, 7, 192, 0, 9, true, true, 12);
    assert_eq!(c.geom_offset_x, 64);
    assert_eq!(c.geom_offset_y, 0);
}

/// TIER 4 — the four clamps are SEQUENTIAL and asymmetric.
#[test]
fn tier4_clip_mv_on_pic_boundary_is_sequential_and_asymmetric() {
    // Well inside: untouched.
    let (mut x, mut y) = (16i16, -16i16);
    clip_mv_on_pic_boundary(64, 64, 16, 16, 320, 240, 64, &mut x, &mut y);
    assert_eq!((x, y), (16, -16));

    // Past the right edge: replaced with (max_width - org_x) * 8,
    // which carries NO block-width term.
    let (mut x, mut y) = (4000i16, 0i16);
    clip_mv_on_pic_boundary(64, 64, 16, 16, 320, 240, 64, &mut x, &mut y);
    assert_eq!(x, (320 - 64) * 8);

    // Past the left edge: replaced with (-org_x - bwidth) * 8, which
    // DOES carry the block-width term.
    let (mut x, mut y) = (-4000i16, 0i16);
    clip_mv_on_pic_boundary(64, 64, 16, 16, 320, 240, 64, &mut x, &mut y);
    assert_eq!(x, (-64 - 16) * 8);

    // Vertical clamps are independent of the horizontal ones.
    let (mut x, mut y) = (0i16, 4000i16);
    clip_mv_on_pic_boundary(64, 64, 16, 16, 320, 240, 64, &mut x, &mut y);
    assert_eq!((x, y), (0, (240 - 64) * 8));
}

/// TIER 4 — the C comparisons are SIGNED against positive thresholds,
/// so a large NEGATIVE MV raises nothing. Reproduced deliberately.
#[test]
fn tier4_check_spatial_mv_size_is_signed_not_absolute() {
    assert_eq!(check_spatial_mv_size(&[mv(0, 0)], 0, 0), 0);
    assert_eq!(check_spatial_mv_size(&[mv(300, 0)], 0, 0), 1);
    assert_eq!(check_spatial_mv_size(&[mv(600, 0)], 0, 0), 2);
    assert_eq!(check_spatial_mv_size(&[mv(3000, 0)], 0, 0), 3);
    // Same magnitudes, negative: category stays 0.
    assert_eq!(check_spatial_mv_size(&[mv(-3000, -3000)], 0, 0), 0);
    // The ME MV participates in every arm.
    assert_eq!(check_spatial_mv_size(&[mv(0, 0)], 3000, 0), 3);
    // Thresholds are STRICT: exactly LOW_SPATIAL_MV_TH is not above it.
    assert_eq!(check_spatial_mv_size(&[mv(LOW_SPATIAL_MV_TH, 0)], 0, 0), 0);
    assert_eq!(
        check_spatial_mv_size(&[mv(LOW_SPATIAL_MV_TH + 1, 0)], 0, 0),
        1
    );
    // An empty MVP array never enters the loop -> 0 even for a huge
    // ME MV, because the ME MV is only tested INSIDE the loop.
    assert_eq!(check_spatial_mv_size(&[], 30000, 30000), 0);
    // The running max survives a later small MVP.
    assert_eq!(check_spatial_mv_size(&[mv(600, 0), mv(0, 0)], 0, 0), 2);
}

/// TIER 4 — unlike its spatial twin, the temporal check DOES use ABS,
/// and it never returns 3.
#[test]
fn tier4_check_temporal_mv_size_uses_abs() {
    assert_eq!(check_temporal_mv_size(mv(0, 0)), 0);
    assert_eq!(check_temporal_mv_size(mv(1500, 0)), 1);
    assert_eq!(check_temporal_mv_size(mv(-1500, 0)), 1);
    assert_eq!(check_temporal_mv_size(mv(0, 3000)), 2);
    assert_eq!(check_temporal_mv_size(mv(0, -3000)), 2);
    assert_eq!(check_temporal_mv_size(mv(30000, 30000)), 2);
    // INVALID_MV short-circuits to 0.
    assert_eq!(check_temporal_mv_size(Mv::from_int(INVALID_MV_AS_INT)), 0);
}

#[test]
fn tier4_temporal_mv_tpl_index_is_half_resolution() {
    // mi_row = 64 >> 2 = 16, mi_col = 128 >> 2 = 32,
    // index = (16 >> 1) * (mi_stride >> 1) + (32 >> 1).
    assert_eq!(temporal_mv_tpl_index(128, 64, 128), 8 * 64 + 16);
}

/// TIER 4 — order is load-bearing, and C does not deduplicate.
#[test]
fn tier4_determine_best_references_order_and_backfill() {
    // Two list-0 candidates naming DIFFERENT refs, then a bi-pred.
    let cands = [
        cand(0, 0, 0, 0, 0), // LAST
        cand(0, 2, 0, 0, 0), // LAST3
        cand(2, 0, 0, 0, 1), // LAST + BWDREF -> LAST_BWD_FRAME
    ];
    let out = determine_best_references(&cands, 3, true, true, true);
    // LAST(1), LAST3(3), LAST_BWD(8), then only the BWD backfill —
    // LAST and LAST_BWD were already added.
    assert_eq!(out, vec![1, 3, LAST_BWD_FRAME, 5]);
    // The literal constant and the computed compound type must agree,
    // or the is_last_bwd_added guard and the backfill would disagree.
    assert_eq!(crate::inter_mvp::av1_ref_frame_type([1, 5]), LAST_BWD_FRAME);

    // No candidates on a B slice: all three backfills, in C's order.
    let out = determine_best_references(&[], 0, true, true, true);
    assert_eq!(out, vec![1, 5, LAST_BWD_FRAME]);

    // Non-B slice gets no backfill at all.
    let out = determine_best_references(&[], 0, false, true, true);
    assert!(out.is_empty());

    // Each backfill is independently gated on its list's count_try.
    let out = determine_best_references(&[], 0, true, true, false);
    assert_eq!(out, vec![1]);
    let out = determine_best_references(&[], 0, true, false, true);
    assert_eq!(out, vec![5]);

    // No dedup: the same reference twice stays twice.
    let out = determine_best_references(&[cand(0, 0, 0, 0, 0); 2], 2, false, true, true);
    assert_eq!(out, vec![1, 1]);
}

fn pruning_ctrls(max_dev: u32, closest_mult: u32) -> RefPruningCtrls {
    RefPruningCtrls {
        enabled: true,
        max_dev_to_best: [max_dev; TOT_INTER_GROUP],
        check_closest_multiplier: closest_mult,
        closest_refs: [false; TOT_INTER_GROUP],
    }
}

/// TIER 4 — the documented C off-by-one: entry 7 is never written, so
/// (list 1, ref 3) is enabled for every group with a non-zero
/// threshold no matter how bad its distortion is.
#[test]
fn tier4_reference_pruning_last_entry_is_never_scored() {
    let ctrls = pruning_ctrls(50, 0);
    // Every reference is catastrophically worse than the best...
    let mut early = [1_000_000u32; MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH];
    early[0] = 10;
    let offsets = [[0u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST];
    let state = perform_md_reference_pruning(&ctrls, &early, &offsets, 10, 16, 16, 30);

    // ...so only the best one survives on merit,
    assert!(state.do_ref[0][0][0]);
    assert!(!state.do_ref[0][0][1]);
    assert!(!state.do_ref[0][1][2]);
    // ...plus entry 7 = (list 1, ref 3), whose dev stayed 0.
    assert!(
        state.do_ref[0][1][3],
        "C's `i < n - 1` fill loop leaves entry 7 at 0, which is below \
             every non-zero pruning threshold"
    );
}

/// TIER 4 — the `check_closest_multiplier` arm bypasses the
/// distortion sort entirely and enables ref 0 for every group (and
/// every ref for groups whose threshold is `~0`).
#[test]
fn tier4_reference_pruning_closest_arm() {
    let ctrls = pruning_ctrls(50, 4);
    let early = [1u32; MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH];
    let offsets = [[0u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST];
    // th = 4 * 256 * 30 / 24 = 1280 > 1, so the arm is taken.
    let state = perform_md_reference_pruning(&ctrls, &early, &offsets, 1, 16, 16, 30);
    for li in 0..MAX_NUM_OF_REF_PIC_LIST {
        assert!(state.do_ref[0][li][0]);
        for ri in 1..REF_LIST_MAX_DEPTH {
            assert!(!state.do_ref[0][li][ri]);
        }
    }

    // With max_dev_to_best == ~0 the same arm enables EVERY ref.
    let ctrls = pruning_ctrls(u32::MAX, 4);
    let state = perform_md_reference_pruning(&ctrls, &early, &offsets, 1, 16, 16, 30);
    for li in 0..MAX_NUM_OF_REF_PIC_LIST {
        for ri in 0..REF_LIST_MAX_DEPTH {
            assert!(state.do_ref[0][li][ri]);
        }
    }
}

/// TIER 4 — a zero `max_dev_to_best` forces `pruning_th = 0`, and
/// `dev < 0` is impossible, so the group enables NOTHING.
#[test]
fn tier4_reference_pruning_zero_threshold_enables_nothing() {
    let ctrls = pruning_ctrls(0, 0);
    let early = [10u32; MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH];
    let offsets = [[0u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST];
    let state = perform_md_reference_pruning(&ctrls, &early, &offsets, 10, 16, 16, 30);
    for li in 0..MAX_NUM_OF_REF_PIC_LIST {
        for ri in 0..REF_LIST_MAX_DEPTH {
            assert!(!state.do_ref[0][li][ri]);
        }
    }
    assert!(enabled_groups(&state, 0, 0).is_empty());
}

/// TIER 4 — the noise floor quantises the distortion to multiples of
/// `area`, because `variance / area` is an integer divide taken
/// BEFORE the clamp.
#[test]
fn tier4_compute_lpd0_cost_from_variance_quantises() {
    let area = 16u64 * 16;
    // lambda >> 10 = 8, var_pp = 100000/256 = 390 -> clamped to 8.
    let c = compute_lpd0_cost_from_variance(100_000, 16, 16, 8192, 40);
    assert_eq!(c, ((40u64 * 8192 + 256) >> 9) + ((8 * area) << 7));
    // Below the noise floor the variance passes through, still
    // quantised: 1000/256 = 3.
    let c = compute_lpd0_cost_from_variance(1000, 16, 16, 8192, 40);
    assert_eq!(c, ((40u64 * 8192 + 256) >> 9) + ((3 * area) << 7));
}

/// TIER 4 — BI_PRED is skipped and does NOT count toward the cap, and
/// the cap admits THREE candidates.
#[test]
fn tier4_lpd0_inter_candidate_walk_skips_bipred_and_caps_at_three() {
    let cands = [
        cand(2, 0, 0, 0, 1), // BI_PRED — skipped, not counted
        cand(0, 0, 0, 0, 0),
        cand(1, 0, 1, 0, 0),
        cand(0, 2, 0, 0, 0),
        cand(0, 3, 0, 0, 0), // beyond the cap
    ];
    let got = lpd0_inter_candidate_walk(&cands, 5, |_l, _r| mv(1, 1), 0, 0, 16, 16, 4096, 4096, 64);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].list_idx, 0);
    assert_eq!(got[0].ref_idx, 0);
    assert_eq!(got[1].list_idx, 1);
    // direction 1 selects ref_idx_l1.
    assert_eq!(got[1].ref_idx, 1);
    assert_eq!(got[2].ref_idx, 2);
    // Full-pel MVs are multiplied by 8 on the way in.
    assert_eq!(got[0].mv_full_pel, mv(8, 8));
}

#[test]
fn tier4_is_intra_bordered_needs_both_neighbours() {
    assert!(is_intra_bordered(true, true, false, false));
    assert!(!is_intra_bordered(true, true, true, false));
    assert!(!is_intra_bordered(true, true, false, true));
    assert!(!is_intra_bordered(false, true, false, false));
    assert!(!is_intra_bordered(true, false, false, false));
}

/// TIER 4 — the outer guard is `||`, so ONE valid distortion is
/// enough, and the flag is only ever raised.
#[test]
fn tier4_eliminate_candidate_based_on_pme_me_results() {
    let mut f = false;
    // Both invalid: never enters.
    eliminate_candidate_based_on_pme_me_results(u32::MAX, u32::MAX, 1000, 16, 16, &mut f);
    assert!(!f);

    // One valid and below th = 1000 * 256 = 256000.
    eliminate_candidate_based_on_pme_me_results(u32::MAX, 1000, 1000, 16, 16, &mut f);
    assert!(f);

    // Above the threshold does NOT clear an already-set flag.
    eliminate_candidate_based_on_pme_me_results(u32::MAX, 999_999, 1000, 16, 16, &mut f);
    assert!(f);

    let mut g = false;
    eliminate_candidate_based_on_pme_me_results(u32::MAX, 999_999, 1000, 16, 16, &mut g);
    assert!(!g);
}

#[test]
fn tier4_ssim_threshold_factor_for_tx_type_search() {
    assert_eq!(ssim_threshold_factor_for_tx_type_search(true), 1.06);
    assert_eq!(ssim_threshold_factor_for_tx_type_search(false), 1.05);
    // Distinct from the full-MD twin (1.02 / 1.03) — the two must not
    // be shared.
    assert_ne!(ssim_threshold_factor_for_tx_type_search(true), 1.03);
}

#[test]
fn tier4_get_enable_use_best_me_levels() {
    // Base layer is always off, whatever the level.
    assert_eq!(get_enable_use_best_me(3, 0, 1_000_000), Some(false));
    assert_eq!(get_enable_use_best_me(0, 2, 1_000_000), Some(false));
    // Level 1 is a distortion threshold, strict.
    assert_eq!(get_enable_use_best_me(1, 1, 45_000), Some(false));
    assert_eq!(get_enable_use_best_me(1, 1, 45_001), Some(true));
    // Level 3 is unconditional above the base layer.
    assert_eq!(get_enable_use_best_me(3, 1, 0), Some(true));
    // Level 2 needs TPL, which this port does not have.
    assert_eq!(get_enable_use_best_me(2, 1, 0), None);
}

#[test]
fn tier4_b64_index() {
    // aligned_width 1920, b64 64 -> 30 b64 columns.
    assert_eq!(b64_index(0, 0, 1920, 64), 0);
    assert_eq!(b64_index(128, 64, 1920, 64), 30 + 2);
    // Non-multiple widths round UP.
    assert_eq!(b64_index(0, 64, 100, 64), 2);
}
