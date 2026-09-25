use super::*;

pub(super) fn rq(poc: u64, layer: u8, q: u8, r0: f64) -> pp::RefQueueEntry {
    pp::RefQueueEntry {
        picture_number: poc,
        is_valid: true,
        temporal_layer_index: layer,
        base_q_idx: q,
        slice_type: pp::SliceType::B,
        r0,
    }
}

/// `bind_refs_and_primary_ref_frame` — the primary-ref selection rule.
///
/// TIER 4 and it has to be: `svt_aom_picture_manager_kernel_iter` IS an
/// exported symbol, but its first act is `EB_GET_FULL_OBJECT` on a fifo, so a
/// shim would block instead of returning. "A symbol exists" is not "tier 1 is
/// reachable"; the upgrade path is a byte-identity gate on the inter frame
/// header (tier 2).
///
/// Derivation. The rule is: over LAST..ALT in order, keep the reference with
/// the LARGEST temporal_layer_index that is <= the current picture's layer,
/// ties going to the FIRST (the comparison is strict `<`). With the current
/// picture at layer 2 and reference layers [0, 1, 2, 2, 1, 0, 3]:
/// LAST (0) sets max 0; LAST2 (1) raises it; LAST3 (2) raises it; GOLD (2)
/// ties and does NOT win; BWD/ALT2 are lower; ALT (3) exceeds the picture's
/// layer and is excluded. So primary_ref_frame == LAST3 == 2, stored as a
/// REF_FRAME_MINUS1.
#[test]
pub(super) fn traced_primary_ref_frame_largest_layer_not_exceeding_current() {
    let queue = [
        rq(100, 0, 40, 0.5),
        rq(101, 1, 41, 0.6),
        rq(102, 2, 42, 0.7),
        rq(103, 2, 43, 0.8),
        rq(104, 1, 44, 0.9),
        rq(105, 0, 45, 1.0),
        rq(106, 3, 46, 1.1),
    ];
    let pic = pp::PicParams {
        picture_number: 200,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 2,
        rps: pp::Av1RpsNode {
            ref_poc_array: [100, 101, 102, 103, 104, 105, 106],
            ..Default::default()
        },
        ..Default::default()
    };
    let b = pp::bind_refs_and_primary_ref_frame(&pic, &queue, true, false);
    assert_eq!(b.primary_ref_frame, 2, "LAST3 wins; GOLD ties and does not");
    assert_eq!(b.refresh_frame_context, pp::REFRESH_FRAME_CONTEXT_BACKWARD);

    // The per-reference bindings land at (list, idx) from
    // get_list_idx/get_ref_frame_idx on ref + 1: list 0 takes LAST..GOLD at
    // 0..3 and list 1 takes BWD..ALT at 0..2.
    assert_eq!(b.ref_base_q_idx[0], [40, 41, 42, 43]);
    assert_eq!(b.ref_base_q_idx[1][..3], [44, 45, 46]);
    assert_eq!(b.ref_pic_r0[0][0], 0.5);
    assert_eq!(b.ref_pic_r0[1][2], 1.1);

    // At layer 0 only the layer-0 references qualify, so LAST wins.
    let pic0 = pp::PicParams {
        temporal_layer_index: 0,
        ..pic.clone()
    };
    let b0 = pp::bind_refs_and_primary_ref_frame(&pic0, &queue, true, false);
    assert_eq!(b0.primary_ref_frame, 0);
}

/// The three ways `primary_ref_frame` becomes `PRIMARY_REF_NONE`.
#[test]
pub(super) fn traced_primary_ref_frame_none_cases() {
    let queue = [rq(100, 0, 40, 0.5); 7];
    let inter = pp::PicParams {
        picture_number: 200,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 1,
        rps: pp::Av1RpsNode {
            ref_poc_array: [100; 7],
            ..Default::default()
        },
        ..Default::default()
    };

    // 1. frame_end_cdf_update_mode off -> NONE, and the per-reference
    //    bindings still happen (the loop runs regardless).
    let b = pp::bind_refs_and_primary_ref_frame(&inter, &queue, false, false);
    assert_eq!(b.primary_ref_frame, pp::PRIMARY_REF_NONE);
    assert_eq!(b.ref_base_q_idx[0][0], 40, "bindings happen either way");

    // 2. An I slice -> NONE, and the binding loop is SKIPPED entirely.
    let intra = pp::PicParams {
        slice_type: pp::SliceType::I,
        ..inter.clone()
    };
    let b = pp::bind_refs_and_primary_ref_frame(&intra, &queue, true, false);
    assert_eq!(b.primary_ref_frame, pp::PRIMARY_REF_NONE);
    assert_eq!(b.ref_base_q_idx[0][0], 0, "no binding on an I slice");
    assert!(b.resolved.iter().all(Option::is_none));

    // 3. An S-frame -> NONE even though it is an inter slice.
    let b = pp::bind_refs_and_primary_ref_frame(&inter, &queue, true, true);
    assert_eq!(b.primary_ref_frame, pp::PRIMARY_REF_NONE);
    assert_eq!(
        b.ref_base_q_idx[0][0], 40,
        "bindings still happen for an S-frame"
    );
}

/// An overlay picture binds ITS OWN POC as every reference
/// (`pic_manager_process.c:808-809` hardcodes it).
#[test]
pub(super) fn traced_primary_ref_overlay_uses_own_poc() {
    let queue = [rq(200, 1, 55, 2.0)];
    let pic = pp::PicParams {
        picture_number: 200,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 1,
        is_overlay: true,
        // Deliberately WRONG POCs: the overlay path must ignore them.
        rps: pp::Av1RpsNode {
            ref_poc_array: [999; 7],
            ..Default::default()
        },
        ..Default::default()
    };
    let b = pp::bind_refs_and_primary_ref_frame(&pic, &queue, true, false);
    assert!(b.resolved.iter().all(|r| *r == Some(0)));
    assert_eq!(
        b.primary_ref_frame, 0,
        "LAST is the first layer-1 reference"
    );
}

/// A reference POC missing from the queue is a hard error, not a fallback.
///
/// C asserts and raises `EB_ENC_PM_ERROR10`; continuing would bind a wrong
/// picture, which is exactly the plausible-but-wrong output
/// `docs/WORKING-ON-THIS.md` §6 forbids.
#[test]
#[should_panic(expected = "is not in the reference queue")]
pub(super) fn traced_primary_ref_missing_reference_panics() {
    let queue = [rq(100, 0, 40, 0.5)];
    let pic = pp::PicParams {
        picture_number: 200,
        slice_type: pp::SliceType::B,
        rps: pp::Av1RpsNode {
            ref_poc_array: [777; 7],
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = pp::bind_refs_and_primary_ref_frame(&pic, &queue, true, false);
}

/// `send_picture_out`'s reference-count adjustment, both limiters.
///
/// Trap C flags itself with a TODO and this test pins: the `safe_limit_nref`
/// limiter lowers the try counts AFTER `set_all_ref_frame_type` has already
/// run, so `ref_frame_type_arr` keeps candidates for references MD will not
/// enumerate. Reproduced, not fixed.
#[test]
pub(super) fn traced_send_picture_out_ref_counts() {
    let mut seq = ld_flat_cqp_seq();
    seq.rtc = true;
    seq.mrp_ctrls.early_hme_l0_prune_th = 100;

    // Flat RTC: LAST2 is pruned when last2_dist * 100 >= last_dist * 100,
    // i.e. when it is no better than LAST.
    let mut pic = pp::PicParams {
        slice_type: pp::SliceType::B,
        hierarchical_levels: 0,
        ref_list0_count: 4,
        ref_list1_count: 1,
        ref_list0_count_try: 4,
        ref_list1_count_try: 1,
        ..Default::default()
    };
    let (arr, tot) = pp::set_all_ref_frame_type(&pic);
    pic.ref_frame_type_arr = arr;
    pic.tot_ref_frame_types = tot;
    pp::send_picture_out_ref_counts(&mut pic, &seq, Some((1000, 1000)), false);
    assert_eq!(pic.ref_list0_count_try, 1, "LAST2 pruned");
    // The candidate set was RE-RUN, so it now reflects the lowered count.
    assert_eq!(pic.tot_ref_frame_types, 3, "LAST, BWD, (LAST,BWD)");

    // A clearly better LAST2 is NOT pruned.
    let mut pic2 = pp::PicParams {
        ref_list0_count_try: 4,
        ..pic.clone()
    };
    pic2.ref_list0_count_try = 4;
    pp::send_picture_out_ref_counts(&mut pic2, &seq, Some((1000, 500)), false);
    assert_eq!(pic2.ref_list0_count_try, 4);

    // Hierarchical RTC prunes LAST3 instead, and only at base layer with
    // at least three list-0 references to try.
    let mut pic3 = pp::PicParams {
        slice_type: pp::SliceType::B,
        hierarchical_levels: 3,
        temporal_layer_index: 0,
        ref_list0_count: 4,
        ref_list1_count: 3,
        ref_list0_count_try: 4,
        ref_list1_count_try: 3,
        ..Default::default()
    };
    pp::send_picture_out_ref_counts(&mut pic3, &seq, Some((1000, 1000)), false);
    assert_eq!(pic3.ref_list0_count_try, 2, "LAST3 pruned");

    // safe_limit_nref == 2 caps BOTH lists at 1 on the top two layers when
    // the references have similar brightness -- and it runs after the
    // candidate set was built, so tot_ref_frame_types is now STALE.
    let mut seq2 = ld_flat_cqp_seq();
    seq2.mrp_ctrls.safe_limit_nref = 2;
    let mut pic4 = pp::PicParams {
        slice_type: pp::SliceType::B,
        hierarchical_levels: 3,
        temporal_layer_index: 2,
        ref_list0_count: 4,
        ref_list1_count: 3,
        ref_list0_count_try: 4,
        ref_list1_count_try: 3,
        ..Default::default()
    };
    let (arr, tot) = pp::set_all_ref_frame_type(&pic4);
    pic4.ref_frame_type_arr = arr;
    pic4.tot_ref_frame_types = tot;
    let before = pic4.tot_ref_frame_types;
    pp::send_picture_out_ref_counts(&mut pic4, &seq2, None, true);
    assert_eq!((pic4.ref_list0_count_try, pic4.ref_list1_count_try), (1, 1));
    assert_eq!(
        pic4.tot_ref_frame_types, before,
        "STALE on purpose -- C's own TODO"
    );

    // Without similar brightness nothing changes.
    let mut pic5 = pp::PicParams {
        ref_list0_count_try: 4,
        ref_list1_count_try: 3,
        ..pic4.clone()
    };
    pp::send_picture_out_ref_counts(&mut pic5, &seq2, None, false);
    assert_eq!((pic5.ref_list0_count_try, pic5.ref_list1_count_try), (4, 3));

    // Below the top two layers the limiter does not apply either.
    let mut pic6 = pp::PicParams {
        temporal_layer_index: 1,
        ref_list0_count_try: 4,
        ref_list1_count_try: 3,
        ..pic4.clone()
    };
    pp::send_picture_out_ref_counts(&mut pic6, &seq2, None, true);
    assert_eq!((pic6.ref_list0_count_try, pic6.ref_list1_count_try), (4, 3));
}

/// `copy_tf_params` (`pd_process.c:4468-4497`).
///
/// MEASURED and reproduced as a NEGATIVE result: in LOW_DELAY the only
/// picture that maps to a parameter set at all is a non-I base-layer picture,
/// and `tf_level` is forced to 0 for all LOW_DELAY before any preset logic
/// runs (`enc_handle.c:3339-3343`), so the entry it selects is itself
/// disabled. The mapping is what this function owns; the disabling is one
/// level up in the parameter table.
#[test]
pub(super) fn traced_copy_tf_params_low_delay_and_random_access() {
    use pp::TfParamsChoice::{Base, DelayedIntra, Disabled, L1};
    let ld = pp::PredStructure::LowDelay;
    let ra = pp::PredStructure::RandomAccess;

    // LOW DELAY: base-layer inter -> entry 1; everything else disabled.
    assert_eq!(
        pp::copy_tf_params(ld, pp::SliceType::B, false, 0, 0, false, true, false),
        Base
    );
    assert_eq!(
        pp::copy_tf_params(ld, pp::SliceType::I, true, 0, 0, false, true, false),
        Disabled
    );
    assert_eq!(
        pp::copy_tf_params(ld, pp::SliceType::B, false, 1, 3, false, true, false),
        Disabled
    );

    // RANDOM ACCESS, in C's if/else order.
    // A key frame with enable_tf_key off is disabled even when delayed.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::I, true, 0, 4, false, false, true),
        Disabled
    );
    // With enable_tf_key on, a delayed intra takes entry 0.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::I, true, 0, 4, false, true, true),
        DelayedIntra
    );
    // An overlay is disabled whatever else is true.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 0, 4, true, true, true),
        Disabled
    );
    // The HIGHEST layer is disabled -- the check is temporal_layer ==
    // hierarchical_levels, which is what makes 2L's layer 1 ineligible.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 2, 2, false, true, false),
        Disabled
    );
    // BASE -> entry 1, L1 -> entry 2, deeper layers disabled.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 0, 4, false, true, false),
        Base
    );
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 1, 4, false, true, false),
        L1
    );
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 2, 4, false, true, false),
        Disabled
    );
}

/// `get_list_idx` / `get_ref_frame_idx` (`inter_prediction.h:531-541`) over
/// every `MvReferenceFrame` the picture manager passes them.
#[test]
pub(super) fn traced_list_and_ref_idx_tables() {
    // ref_type = REF_FRAME_MINUS1 + 1, i.e. LAST_FRAME(1)..ALTREF_FRAME(7).
    let list: Vec<u8> = (1u8..=7).map(pp::get_list_idx).collect();
    let idx: Vec<u8> = (1u8..=7).map(pp::get_ref_frame_idx).collect();
    assert_eq!(list, [0, 0, 0, 0, 1, 1, 1]);
    assert_eq!(idx, [0, 1, 2, 3, 0, 1, 2]);
    // Slot 0 is INTRA_FRAME and both tables carry a 0 there.
    assert_eq!((pp::get_list_idx(0), pp::get_ref_frame_idx(0)), (0, 0));
}

// ---------------------------------------------------------------------------
// Histogram scene detection — tier 4
// ---------------------------------------------------------------------------
//
// `scene_transition_detector` is `static` AND its globalized symbol is
// unusable: LLVM promoted its `PictureParentControlSet** window` parameter to
// the current PPCS, so calling the promoted symbol with the source signature
// segfaults (the finding is written up in `c_parity_picstruct_statics.rs`).
// So this one is tier 4 with no upgrade path short of a byte-identity gate.
