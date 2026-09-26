use super::*;

/// Tier 4 vectors: every assertion below was traced by hand against
/// `product_coding_loop.c`. The functions are `static` in C.
fn ctrls() -> NicPruningCtrls {
    NicPruningCtrls {
        mds1_class_th: Some(100),
        mds1_band_cnt: 3,
        mds2_class_th: Some(50),
        mds2_band_cnt: 3,
        mds3_class_th: Some(50),
        i_mds3_class_th_mult: 1,
        mds3_band_cnt: 3,
        mds1_cand_base_th_intra: Some(100),
        mds1_cand_base_th_inter: Some(200),
        mds1_cand_th_rank_factor: 0,
        mds2_cand_base_th: Some(100),
        mds2_cand_th_rank_factor: 0,
        mds2_relative_dev_th: 0,
        mds3_cand_base_th: Some(100),
        enable_skipping_mds1: true,
        merge_inter_cands_mult: 0,
    }
}

#[test]
fn exchange_sort_differs_from_a_stable_sort_on_a_tie() {
    // costs indexed by buffer id: 0 -> 10, 1 -> 5, 2 -> 5.
    let costs = [10u64, 5, 5];
    let got = sort_fast_cost_based_candidates(0, 3, |i| costs[i as usize]);
    // C's i=0 pass swaps in buffer 1, then compares j=2 (5) against 5:
    // strict `<` fails, so 2 stays behind. i=1 pass compares 0 (10)
    // against 2 (5) and swaps.
    assert_eq!(got, vec![1, 2, 0]);
    // A stable sort_by_key would give [1, 2, 0] here too; the difference
    // shows when the tie straddles the leading element.
    let costs2 = [5u64, 7, 5];
    let got2 = sort_fast_cost_based_candidates(0, 3, |i| costs2[i as usize]);
    let mut stable: Vec<u32> = vec![0, 1, 2];
    stable.sort_by_key(|&i| costs2[i as usize]);
    assert_eq!(got2, vec![0, 2, 1]);
    assert_eq!(stable, vec![0, 2, 1]);
}

#[test]
fn class_threshold_is_disabled_on_an_i_slice_and_live_otherwise() {
    // Class 1 (inter) sits 200% above the global best -> dev 200 >= 100,
    // which kills the class on a P slice and does nothing on an I slice.
    let c0 = [100u64];
    let c1 = [300u64];
    let empty: &[u64] = &[];
    let costs = [&c0[..], &c1[..], empty, empty, empty];
    let s0 = [1u32, 1, 0, 0, 0];

    let mut s1 = [1u32, 1, 0, 0, 0];
    let out = post_mds0_nic_pruning(&ctrls(), (1, 1), false, &costs, &s0, &mut s1, 100);
    assert_eq!(s1, [1, 0, 0, 0, 0]);
    assert_eq!(out.total, 1);
    // Exactly one survivor and skipping enabled -> MDS1 is skipped.
    assert!(!out.perform_mds1);

    let mut s1i = [1u32, 1, 0, 0, 0];
    let outi = post_mds0_nic_pruning(&ctrls(), (1, 1), true, &costs, &s0, &mut s1i, 100);
    assert_eq!(s1i, [1, 1, 0, 0, 0]);
    assert_eq!(outi.total, 2);
    assert!(outi.perform_mds1);
}

#[test]
fn intra_and_inter_classes_take_different_candidate_thresholds() {
    // Second candidate deviates 150% from its class best: under the
    // intra threshold (100) it is dropped, under the inter one (200) kept.
    let intra = [100u64, 250];
    let inter = [100u64, 250];
    let empty: &[u64] = &[];
    let costs = [&intra[..], &inter[..], empty, empty, empty];
    let s0 = [2u32, 2, 0, 0, 0];
    let mut s1 = [2u32, 2, 0, 0, 0];
    // best_md_stage_cost == both class bests, so the class prune is inert.
    post_mds0_nic_pruning(&ctrls(), (1, 1), false, &costs, &s0, &mut s1, 100);
    assert_eq!(s1[0], 1, "intra class prunes at 100");
    assert_eq!(s1[1], 2, "inter class keeps at 200");
}

#[test]
fn band_reduction_halves_the_count_in_the_middle_band() {
    // dev 40 with class_th 100 and band_cnt 3 -> band_idx 0 -> unchanged;
    // dev 60 -> band_idx 1 -> DIVIDE_AND_ROUND(4, 2) = 2.
    assert_eq!(band_reduce(4, 40, 3, 100), 4);
    assert_eq!(band_reduce(4, 60, 3, 100), 2);
    // band_cnt < 3 disables it entirely, and a single candidate is never
    // reduced.
    assert_eq!(band_reduce(4, 60, 2, 100), 4);
    assert_eq!(band_reduce(1, 60, 3, 100), 1);
}

#[test]
fn mds3_class_threshold_is_refloored_on_an_i_slice() {
    // mds3_class_th 50 * mult 4 = 200 on an I slice; a class deviating
    // 100% survives there and dies on a P slice.
    let mut c = ctrls();
    c.i_mds3_class_th_mult = 4;
    let c0 = [100u64];
    let c3 = [200u64];
    let empty: &[u64] = &[];
    let costs = [&c0[..], empty, empty, &c3[..], empty];
    let s2 = [1u32, 0, 0, 1, 0];

    let mut s3 = [1u32, 0, 0, 1, 0];
    assert_eq!(
        post_mds2_nic_pruning(&c, (1, 1), true, &costs, &s2, &mut s3, 100),
        2
    );
    let mut s3p = [1u32, 0, 0, 1, 0];
    assert_eq!(
        post_mds2_nic_pruning(&c, (1, 1), false, &costs, &s2, &mut s3p, 100),
        1
    );
    assert_eq!(s3p[3], 0);
}

#[test]
fn mds3_class_threshold_floors_at_25_on_an_i_slice() {
    // scaled 5 * mult 1 = 5, floored to 25: a class 10% off the best
    // survives even though the unfloored threshold would kill it.
    let mut c = ctrls();
    c.mds3_class_th = Some(5);
    let c0 = [100u64];
    let c3 = [110u64];
    let empty: &[u64] = &[];
    let costs = [&c0[..], empty, empty, &c3[..], empty];
    let s2 = [1u32, 0, 0, 1, 0];
    let mut s3 = [1u32, 0, 0, 1, 0];
    assert_eq!(
        post_mds2_nic_pruning(&c, (1, 1), true, &costs, &s2, &mut s3, 100),
        2
    );
}

#[test]
fn mds1_rank_factor_staging_penalises_a_losing_class() {
    // cand_th 100, rank factor 1. Candidate 1 deviates 60%.
    // Winning class: rank 1 + 2 (mds0 == mds1 winner) = 3 -> 100/3 = 33,
    //   60 >= 33 -> dropped.
    // With the rank factor off the raw 100 applies -> kept.
    let mut c = ctrls();
    c.mds2_cand_th_rank_factor = 1;
    let c0 = [100u64, 160];
    let empty: &[u64] = &[];
    let costs = [&c0[..], empty, empty, empty, empty];
    let s1 = [2u32, 0, 0, 0, 0];
    let w = StageWinner {
        idx: 0,
        class: CandClass::Intra,
    };
    let mut s2 = [2u32, 0, 0, 0, 0];
    post_mds1_nic_pruning(&c, (1, 1), false, &costs, &s1, &mut s2, 100, w, w);
    assert_eq!(s2[0], 1);

    let mut c_off = c;
    c_off.mds2_cand_th_rank_factor = 0;
    let mut s2b = [2u32, 0, 0, 0, 0];
    post_mds1_nic_pruning(&c_off, (1, 1), false, &costs, &s1, &mut s2b, 100, w, w);
    assert_eq!(s2b[0], 2);
}

#[test]
fn relative_deviation_threshold_stops_a_jump() {
    // devs 10, 20, 90. With mds2_relative_dev_th = 15 the third
    // candidate's jump (90 > 20 + 15) stops the walk even though 90 is
    // still under the flat threshold of 100.
    let mut c = ctrls();
    c.mds2_relative_dev_th = 15;
    let c0 = [100u64, 110, 120, 190];
    let empty: &[u64] = &[];
    let costs = [&c0[..], empty, empty, empty, empty];
    let s1 = [4u32, 0, 0, 0, 0];
    let w = StageWinner {
        idx: 0,
        class: CandClass::Intra,
    };
    let mut s2 = [4u32, 0, 0, 0, 0];
    post_mds1_nic_pruning(&c, (1, 1), false, &costs, &s1, &mut s2, 100, w, w);
    assert_eq!(s2[0], 3);
}

#[test]
fn union_is_class_concatenated_not_cost_sorted() {
    let i0 = [7u32, 8];
    let i1 = [1u32];
    let empty: &[u32] = &[];
    let buffers = [&i0[..], &i1[..], empty, empty, empty];
    let counts = [2u32, 1, 0, 0, 0];
    assert_eq!(
        construct_best_sorted_arrays_md_stage_3(&counts, &buffers),
        vec![7, 8, 1]
    );
}

#[test]
fn disabled_thresholds_leave_every_count_untouched() {
    let mut c = ctrls();
    c.mds1_class_th = None;
    c.mds1_cand_base_th_intra = None;
    c.mds1_cand_base_th_inter = None;
    let c0 = [100u64, 100_000];
    let empty: &[u64] = &[];
    let costs = [&c0[..], empty, empty, empty, empty];
    let s0 = [2u32, 0, 0, 0, 0];
    let mut s1 = [2u32, 0, 0, 0, 0];
    let out = post_mds0_nic_pruning(&c, (1, 1), false, &costs, &s0, &mut s1, 1);
    assert_eq!(s1[0], 2);
    assert_eq!(out.total, 2);
}
