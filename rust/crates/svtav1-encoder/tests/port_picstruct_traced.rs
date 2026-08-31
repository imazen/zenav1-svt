//! Hand-derived vectors for the `static` C functions in `Codec/pd_process.c`
//! — evidence **tier 4** (`docs/WORKING-ON-THIS.md` §4), the weakest tier,
//! used here because these functions have NO exported symbol.
//!
//! Verified with `nm -g Bin/Release/libSvtAv1Enc.a`: of the functions this
//! file covers, none is a global (`T`) symbol. Two of them
//! (`set_all_ref_frame_type`, `set_ref_list_counts`) survive in
//! `cbuild-static/.../pd_process.c.o` as LOCAL (`t`) symbols, so promoting
//! them with `llvm-objcopy --globalize-symbol` on a private copy of that
//! object is a plausible route to tier 1. That route is NOT taken here and is
//! NOT claimed to work — it has not been made to link.
//!
//! Every expectation below is derived by reading the C, step by step, with
//! the derivation written out in the comment above it. Where a value is
//! surprising, the surprise is stated — that is the only defence a tier-4
//! vector has against being a second transcription of the same mistake.

use svtav1_encoder::inter_mvp::OrderHintInfo;
use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_picstruct::{ALT, ALT2, BWD, GOLD, LAST, LAST2, LAST3};

/// The configuration of the inter campaign's first cell
/// (`tools/identity_diff_inter.sh`): low-delay P, flat, CQP.
fn ld_flat_cqp_seq() -> pp::SeqPicParams {
    pp::SeqPicParams {
        pred_structure: pp::PredStructure::LowDelay,
        rate_control_mode: pp::RcMode::CqpOrCrf,
        rtc: false,
        allintra: false,
        mrp_ctrls: pp::MrpCtrls::default(),
        order_hint_info: OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: 7,
        },
    }
}

fn key_frame(poc: u64) -> pp::PicParams {
    pp::PicParams {
        picture_number: poc,
        decode_order: poc,
        slice_type: pp::SliceType::I,
        is_key_frame: true,
        is_intra_only: true,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        aligned_width: 64,
        aligned_height: 64,
        ..Default::default()
    }
}

fn inter_frame(poc: u64, last_idr: u64) -> pp::PicParams {
    pp::PicParams {
        picture_number: poc,
        decode_order: poc,
        slice_type: pp::SliceType::B,
        is_key_frame: false,
        is_intra_only: false,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        frame_offset: poc - last_idr,
        aligned_width: 64,
        aligned_height: 64,
        ..Default::default()
    }
}

/// `set_frame_update_type` (`pd_process.c:4591-4611`) over a flat low-delay
/// GOP, plus `set_layer_depth`.
///
/// Derivation: `hierarchical_levels == 0`, so the first two arms are skipped
/// and the modulus is `MAX(4, 1 << 0)` = **4** — the `1 << hierarchical_levels`
/// term never wins at level 0. So offset 0/4/8 are GF, odd offsets are LF, and
/// the remaining even offsets (2, 6, 10) are INTNL_ARF.
#[test]
fn traced_set_frame_update_type_flat_gop() {
    use pp::FrameUpdateType::{Gf, IntnlArf, Lf};
    let expect = [
        Gf, Lf, IntnlArf, Lf, Gf, Lf, IntnlArf, Lf, Gf, Lf, IntnlArf, Lf,
    ];
    for (offset, want) in expect.iter().enumerate() {
        let mut pic = inter_frame(offset as u64, 0);
        pp::set_gf_group_param(&mut pic);
        assert_eq!(pic.update_type, *want, "frame_offset {offset}");
        // set_layer_depth: not a key frame -> temporal_layer + 1.
        assert_eq!(pic.layer_depth, 1, "frame_offset {offset}");
    }
    // A key frame is KF_UPDATE with layer_depth 0 regardless of offset.
    let mut kf = key_frame(0);
    pp::set_gf_group_param(&mut kf);
    assert_eq!(kf.update_type, pp::FrameUpdateType::Kf);
    assert_eq!(kf.layer_depth, 0);
}

/// `prune_refs` (`pd_process.c:1100-1131`) — the fold ORDER.
///
/// Derivation, with distinct slot ids so a mis-ordered fold is visible:
/// start `dpb_index = [0,1,2,3,4,5,6]`. At `(l0, l1) = (1, 0)`:
/// GOLD<-LAST, LAST3<-LAST, LAST2<-LAST give `[0,0,0,0,...]`; then
/// `l1 < 1` gives BWD<-LAST = 0; then `l1 < 3` gives ALT<-BWD, and BWD is
/// ALREADY 0, so ALT = 0 (not the original 4); then `l1 < 2` gives ALT2 = 0.
/// A port that folded ALT2 before ALT, or that read the pre-fold BWD, gets 4.
#[test]
fn traced_prune_refs_fold_order() {
    let mut rps = pp::Av1RpsNode {
        refresh_frame_mask: 0,
        ref_dpb_index: [0, 1, 2, 3, 4, 5, 6],
        ref_poc_array: [10, 11, 12, 13, 14, 15, 16],
    };
    pp::prune_refs(&mut rps, 1, 0);
    assert_eq!(rps.ref_dpb_index, [0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(rps.ref_poc_array, [10, 10, 10, 10, 10, 10, 10]);

    // (l0, l1) = (2, 1): LAST2 survives; BWD survives; ALT and ALT2 fold onto
    // the SURVIVING BWD (4), not onto LAST.
    let mut rps = pp::Av1RpsNode {
        refresh_frame_mask: 0,
        ref_dpb_index: [0, 1, 2, 3, 4, 5, 6],
        ref_poc_array: [10, 11, 12, 13, 14, 15, 16],
    };
    pp::prune_refs(&mut rps, 2, 1);
    assert_eq!(rps.ref_dpb_index, [0, 1, 0, 0, 4, 4, 4]);
    assert_eq!(rps.ref_poc_array, [10, 11, 10, 10, 14, 14, 14]);

    // (l0, l1) = (4, 3): nothing folds.
    let mut rps = pp::Av1RpsNode {
        refresh_frame_mask: 0,
        ref_dpb_index: [0, 1, 2, 3, 4, 5, 6],
        ref_poc_array: [10, 11, 12, 13, 14, 15, 16],
    };
    pp::prune_refs(&mut rps, 4, 3);
    assert_eq!(rps.ref_dpb_index, [0, 1, 2, 3, 4, 5, 6]);
}

/// `update_dpb` (`pd_process.c:5179-5191`) — only the masked slots move.
#[test]
fn traced_update_dpb_masked_slots_only() {
    let mut ctx = pp::PicDecisionCtx::default();
    let pic = pp::PicParams {
        picture_number: 7,
        decode_order: 5,
        temporal_layer_index: 2,
        rps: pp::Av1RpsNode {
            refresh_frame_mask: 0b1010_0001,
            ..Default::default()
        },
        ..Default::default()
    };
    pp::update_dpb(&pic, &mut ctx);
    for slot in 0..8 {
        let want_written = (0b1010_0001u8 >> slot) & 1 == 1;
        let e = ctx.dpb[slot];
        if want_written {
            assert_eq!(
                (e.picture_number, e.decode_order, e.temporal_layer_index),
                (7, 5, 2)
            );
        } else {
            assert_eq!(
                (e.picture_number, e.decode_order, e.temporal_layer_index),
                (0, 0, 0)
            );
        }
    }
    // A zero mask writes nothing at all (C guards the whole loop).
    let mut ctx = pp::PicDecisionCtx::default();
    let pic = pp::PicParams {
        picture_number: 9,
        rps: pp::Av1RpsNode {
            refresh_frame_mask: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    pp::update_dpb(&pic, &mut ctx);
    assert!(ctx.dpb.iter().all(|e| e.picture_number == 0));
}

/// `set_ref_list_counts` (`pd_process.c:1804-1900`) — the de-duplication.
///
/// Derivation for the list-1 loop, which is the half that is easy to get
/// wrong. With `ref_poc_array = [4, 3, 2, 1, 4, 4, 4]` and
/// `ref_list0_count == 4`:
/// * `i = BWD(4)`: the inner loop STARTS at LAST2, not LAST — BWD is allowed
///   to equal LAST, and here it does (both 4). j = 1,2,3 give 3,2,1 vs 4: no
///   match, so list1_count becomes 1.
/// * `i = ALT2(5)`: the inner loop starts at LAST. j = 0 gives poc 4 == 4 ->
///   breakout.
///
/// So list1_count == 1 even though BWD == LAST. A port that started the BWD
/// row at LAST would get 0 and signal no backward reference at all.
#[test]
fn traced_set_ref_list_counts_bwd_may_equal_last() {
    let seq = ld_flat_cqp_seq();
    let ctx = pp::PicDecisionCtx::default();
    let mut pic = pp::PicParams {
        slice_type: pp::SliceType::B,
        update_type: pp::FrameUpdateType::Lf, // not boosted -> non_base caps
        rps: pp::Av1RpsNode {
            refresh_frame_mask: 0,
            ref_dpb_index: [0; 7],
            ref_poc_array: [4, 3, 2, 1, 4, 4, 4],
        },
        ..Default::default()
    };
    pp::set_ref_list_counts(&mut pic, &seq, &ctx);
    assert_eq!(pic.ref_list0_count, 4, "all four list-0 POCs are distinct");
    assert_eq!(
        pic.ref_list1_count, 1,
        "BWD == LAST is allowed; ALT2 == LAST is not"
    );

    // All POCs identical. list 0 breaks out at LAST2 -> count 1. list 1 is
    // the subtle one, and my first hand-derivation of it was WRONG (I wrote 0,
    // the test failed, and re-reading the C gave 1): with ref_list0_count == 1,
    // BWD's inner loop skips j = LAST2/LAST3/GOLD entirely via the
    // `j + 1 > ref_list0_count` guard, so it never reaches the equality test
    // and BWD counts. ALT2 then starts at LAST, where j = 0 passes the guard
    // (1 > 1 is false) and the POCs match, so it breaks out. Result (1, 1) --
    // a frame whose only backward reference is the same picture as LAST.
    // Recorded because "all POCs equal => no list 1" is the natural wrong
    // guess and it is what a simplified port would produce.
    let mut pic = pp::PicParams {
        slice_type: pp::SliceType::B,
        update_type: pp::FrameUpdateType::Lf,
        rps: pp::Av1RpsNode {
            ref_poc_array: [9; 7],
            ..Default::default()
        },
        ..Default::default()
    };
    pp::set_ref_list_counts(&mut pic, &seq, &ctx);
    assert_eq!((pic.ref_list0_count, pic.ref_list1_count), (1, 1));

    // An I slice zeroes both without looking at the POCs.
    let mut pic = pp::PicParams {
        slice_type: pp::SliceType::I,
        rps: pp::Av1RpsNode {
            ref_poc_array: [7, 6, 5, 4, 3, 2, 1],
            ..Default::default()
        },
        ..Default::default()
    };
    pp::set_ref_list_counts(&mut pic, &seq, &ctx);
    assert_eq!((pic.ref_list0_count, pic.ref_list1_count), (0, 0));
}

/// `set_ref_list_counts` caps against the BOOSTED row of `MrpCtrls` only when
/// `frame_is_boosted` — which is `update_type in {ARF, GF}` OR intra-only,
/// NOT `temporal_layer == 0`.
///
/// Both pictures below are temporal layer 0 with seven distinct POCs; only the
/// GF one takes the base cap.
#[test]
fn traced_set_ref_list_counts_boosted_is_update_type_not_layer() {
    let mut seq = ld_flat_cqp_seq();
    seq.mrp_ctrls.base_ref_list0_count = 4;
    seq.mrp_ctrls.non_base_ref_list0_count = 2;
    let ctx = pp::PicDecisionCtx::default();
    let poc = [7, 6, 5, 4, 3, 2, 1];

    let mut gf = pp::PicParams {
        slice_type: pp::SliceType::B,
        temporal_layer_index: 0,
        update_type: pp::FrameUpdateType::Gf,
        rps: pp::Av1RpsNode {
            ref_poc_array: poc,
            ..Default::default()
        },
        ..Default::default()
    };
    pp::set_ref_list_counts(&mut gf, &seq, &ctx);
    assert_eq!(gf.ref_list0_count, 4, "GF_UPDATE is boosted -> base cap 4");

    let mut lf = pp::PicParams {
        slice_type: pp::SliceType::B,
        temporal_layer_index: 0,
        update_type: pp::FrameUpdateType::Lf,
        rps: pp::Av1RpsNode {
            ref_poc_array: poc,
            ..Default::default()
        },
        ..Default::default()
    };
    pp::set_ref_list_counts(&mut lf, &seq, &ctx);
    assert_eq!(
        lf.ref_list0_count, 2,
        "LF_UPDATE at layer 0 is NOT boosted -> cap 2"
    );
}

/// `set_all_ref_frame_type` (`pd_process.c:1044-1099`) — the exact ordered set.
///
/// Derivation at `(l0_try, l1_try) = (2, 1)`, B slice:
/// single list 0 -> LAST(1), LAST2(2); single list 1 -> BWDREF(5);
/// compound bi-dir -> (LAST,BWD) then (LAST2,BWD);
/// uni-dir -> `l0_try > 1` adds (LAST,LAST2); `l1_try > 2` is false.
/// `av1_ref_frame_type` is the C-gated helper in `inter_mvp`
/// (`c_parity_inter_mvp.rs` round-trips it against `av1_set_ref_frame`), so
/// the compound codes are computed, not re-transcribed:
/// (LAST,BWD) -> 8 + (1-1) + (5-5)*4 = 8; (LAST2,BWD) -> 8 + 1 + 0 = 9;
/// (LAST,LAST2) is uni-comp index 0 -> 8 + 12 + 0 = 20.
#[test]
fn traced_set_all_ref_frame_type_ordered_set() {
    let pic = pp::PicParams {
        slice_type: pp::SliceType::B,
        ref_list0_count_try: 2,
        ref_list1_count_try: 1,
        ..Default::default()
    };
    let (arr, tot) = pp::set_all_ref_frame_type(&pic);
    assert_eq!(tot, 6);
    assert_eq!(&arr[..6], &[1, 2, 5, 8, 9, 20]);

    // An I slice with zero try counts produces the empty set — and the
    // uni-dir block is gated on B_SLICE, so it never runs.
    let pic = pp::PicParams {
        slice_type: pp::SliceType::I,
        ..Default::default()
    };
    let (_, tot) = pp::set_all_ref_frame_type(&pic);
    assert_eq!(tot, 0);

    // Full (4, 3): 4 + 3 singles + 12 bi-dir + 3 uni list-0 + 1 uni list-1.
    let pic = pp::PicParams {
        slice_type: pp::SliceType::B,
        ref_list0_count_try: 4,
        ref_list1_count_try: 3,
        ..Default::default()
    };
    let (arr, tot) = pp::set_all_ref_frame_type(&pic);
    assert_eq!(tot, 23);
    assert_eq!(
        &arr[..7],
        &[1, 2, 3, 4, 5, 6, 7],
        "the seven singles, in list order"
    );
    // The three uni-dir list-0 codes are (LAST,LAST2)=20, (LAST,LAST3)=21,
    // (LAST,GOLD)=22 and the uni-dir list-1 code is (BWD,ALT)=23.
    assert_eq!(&arr[19..23], &[20, 21, 22, 23]);
}

/// The full first cell: a five-frame low-delay flat CQP sequence, driven
/// through [`pp::picture_decision_per_picture`] in C's order.
///
/// Every expectation is hand-derived from `pd_process.c` and the derivation is
/// spelled out per frame. This is the vector that would catch a toggle that
/// advances at the wrong point, a DPB write in the wrong place, or a
/// `prune_refs` call ordered against a stale count.
#[test]
fn traced_low_delay_flat_cqp_five_frames() {
    let seq = ld_flat_cqp_seq();
    let mut ctx = pp::PicDecisionCtx::default();

    // ---- frame 0: KEY ----
    // set_gf_group_param -> KF_UPDATE, layer_depth 0.
    // generate_rps_info: I slice + key -> refresh_frame_mask = 0xFF, toggles
    // reset to 0, counts zeroed, EARLY RETURN (no branch runs).
    let mut f0 = key_frame(0);
    pp::picture_decision_per_picture(&mut f0, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(f0.rps.refresh_frame_mask, 0xFF);
    assert_eq!((f0.ref_list0_count, f0.ref_list1_count), (0, 0));
    assert_eq!(f0.reference_mode, pp::ReferenceMode::IntraSentinel);
    assert!(f0.show_frame && !f0.has_show_existing);
    // update_dpb with 0xFF fills every slot with POC 0.
    assert!(ctx.dpb.iter().all(|e| e.picture_number == 0));
    assert_eq!((ctx.lay0_toggle, ctx.lay1_toggle), (0, 0));
    // mi_cols/mi_rows = aligned dims >> MI_SIZE_LOG2 = 64 >> 2 = 16.
    assert_eq!((f0.mi_cols, f0.mi_rows), (16, 16));

    // ---- frame 1: POC 1 ----
    // base2 = lay0_toggle = 0; base1 = CIRC_DEC(0,0,2) = 2; base0 = 1.
    // lay1_offset = 3 (ld_reduce_ref_buffs 0); lay1_2 = 3; lay1_1 = 5; lay1_0 = 4.
    // lay1_pic_idx = 0 at hier 0, and pic_idx (0) is not > 0, so LAST = base2.
    // Pre-prune: [0, 5, 7, 1, 3, 4, 2].
    // Toggle advances FIRST (lay0 0 -> 1), so the mask is 1 << 1 = 0x02.
    // Every DPB slot still holds POC 0, so all seven ref POCs are 0; list 0
    // breaks out at LAST2 (count 1) and list 1 counts BWD only (count 1),
    // because BWD's row starts at LAST2 and every j is skipped by the
    // `j + 1 > ref_list0_count` guard.
    // prune_refs(1, 1) then folds LAST2/LAST3/GOLD onto LAST and ALT/ALT2
    // onto BWD.
    let mut f1 = inter_frame(1, 0);
    pp::picture_decision_per_picture(&mut f1, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(
        f1.update_type,
        pp::FrameUpdateType::Lf,
        "offset 1 is odd -> LF"
    );
    assert!(f1.is_ref);
    assert_eq!(f1.rps.refresh_frame_mask, 0x02);
    assert_eq!(f1.rps.ref_dpb_index, [0, 0, 0, 0, 3, 3, 3]);
    assert_eq!(f1.rps.ref_poc_array, [0; 7]);
    assert_eq!((f1.ref_list0_count, f1.ref_list1_count), (1, 1));
    assert_eq!((f1.ref_list0_count_try, f1.ref_list1_count_try), (1, 1));
    assert_eq!(f1.reference_mode, pp::ReferenceMode::Select);
    assert_eq!(f1.cur_order_hint, 1);
    assert_eq!(f1.ref_order_hint, [0; 7]);
    // Every reference is BEHIND the current frame, so no backward reference
    // exists and there is only ONE distinct forward hint -> no skip mode.
    assert_eq!(f1.skip_mode.skip_mode_allowed, 0);
    assert_eq!(f1.skip_mode.skip_mode_flag, 0);
    // Candidate set: LAST, BWDREF, (LAST,BWDREF).
    assert_eq!(f1.tot_ref_frame_types, 3);
    assert_eq!(&f1.ref_frame_type_arr[..3], &[1, 5, 8]);
    // update_dpb wrote slot 1 only.
    assert_eq!(ctx.dpb[1].picture_number, 1);
    assert_eq!(ctx.dpb[0].picture_number, 0);
    assert_eq!(ctx.lay0_toggle, 1);

    // ---- frame 2: POC 2 ----
    // base2 = 1; base1 = CIRC_DEC(1,0,2) = 0; base0 = 2.
    // Pre-prune dpb: LAST=1, LAST2=5, LAST3=7, GOLD=2, BWD=3, ALT2=4, ALT=0.
    // Toggle 1 -> 2, mask 0x04.
    // POCs read out of the DPB: slot 1 holds POC 1, every other slot POC 0
    // -> [1, 0, 0, 0, 0, 0, 0].
    // list 0: LAST2 (0) != LAST (1) -> count 2; LAST3 (0) == LAST2 (0) ->
    // breakout. list 1: BWD's j = LAST2 is now in range (2 > 2 is false) and
    // poc[BWD] == poc[LAST2] -> breakout at count 0.
    // prune_refs(2, 0) keeps LAST2 and folds everything else onto LAST/BWD,
    // with BWD itself already folded onto LAST.
    let mut f2 = inter_frame(2, 0);
    pp::picture_decision_per_picture(&mut f2, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(
        f2.update_type,
        pp::FrameUpdateType::IntnlArf,
        "offset 2: even, not %4"
    );
    assert_eq!(f2.rps.refresh_frame_mask, 0x04);
    assert_eq!(f2.rps.ref_dpb_index, [1, 5, 1, 1, 1, 1, 1]);
    assert_eq!(f2.rps.ref_poc_array, [1, 0, 1, 1, 1, 1, 1]);
    assert_eq!((f2.ref_list0_count, f2.ref_list1_count), (2, 0));
    assert_eq!((f2.ref_list0_count_try, f2.ref_list1_count_try), (2, 0));
    // SURPRISE, and it is C's behaviour: skip mode IS allowed here even though
    // ref_list1_count is 0. svt_av1_setup_skip_mode_allowed reads
    // ref_order_hint[] and cur_order_hint ONLY -- never the list counts. Two
    // distinct forward hints (1 and 0) exist, so the forward-only arm fires
    // with the nearest at index 0 and the second-nearest at index 1.
    assert_eq!(f2.skip_mode.skip_mode_allowed, 1);
    assert_eq!(
        (f2.skip_mode.ref_frame_idx_0, f2.skip_mode.ref_frame_idx_1),
        (1, 2)
    );
    // Candidate set: LAST, LAST2, and the uni-dir compound (LAST,LAST2) = 20.
    assert_eq!(f2.tot_ref_frame_types, 3);
    assert_eq!(&f2.ref_frame_type_arr[..3], &[1, 2, 20]);
    assert_eq!(ctx.dpb[2].picture_number, 2);
    assert_eq!(ctx.lay0_toggle, 2);

    // ---- frame 3: POC 3 ----
    // base2 = 2; base1 = 1; base0 = 0. Toggle 2 -> 0 (CIRC_INC wraps at 2),
    // mask = 1 << 0 = 0x01.
    // POCs: slot 2 -> 2, slot 5/7 -> 0, slot 0 -> 0, slot 3/4 -> 0, slot 1 -> 1.
    // Pre-prune: LAST=2(poc 2), LAST2=5(0), LAST3=7(0), GOLD=0(0), BWD=3(0),
    // ALT2=4(0), ALT=1(poc 1).
    // list 0: LAST2 0 != 2 -> 2; LAST3 0 == LAST2 0 -> breakout. count 2.
    // list 1: BWD row starts LAST2; j=1 in range, poc equal (0 == 0) ->
    // breakout, count 0.
    let mut f3 = inter_frame(3, 0);
    pp::picture_decision_per_picture(&mut f3, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(f3.rps.refresh_frame_mask, 0x01);
    assert_eq!(ctx.lay0_toggle, 0, "CIRC_INC(2, 0, 2) wraps to 0");
    assert_eq!(f3.rps.ref_poc_array[LAST], 2);
    assert_eq!((f3.ref_list0_count, f3.ref_list1_count), (2, 0));
    // Slot 0 was refreshed, so it now holds POC 3 while slot 1 still holds 1.
    assert_eq!(ctx.dpb[0].picture_number, 3);
    assert_eq!(ctx.dpb[1].picture_number, 1);

    // ---- frame 4: POC 4 ----
    // offset 4 is a multiple of 4 -> GF_UPDATE, which IS boosted, so
    // set_ref_list_counts and update_count_try switch to the BASE caps.
    // base2 = lay0_toggle = 0 (POC 3); base1 = 2 (POC 2); base0 = 1 (POC 1).
    // Pre-prune POCs: LAST=slot0=3, LAST2=slot5=0, LAST3=slot7=0,
    // GOLD=slot1=1, BWD=slot3=0, ALT2=slot4=0, ALT=slot2=2.
    // list 0: LAST2 0 != 3 -> 2; LAST3 0 == LAST2 -> breakout. count 2.
    let mut f4 = inter_frame(4, 0);
    pp::picture_decision_per_picture(&mut f4, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(
        f4.update_type,
        pp::FrameUpdateType::Gf,
        "offset 4 -> GF_UPDATE"
    );
    assert_eq!(f4.rps.ref_poc_array[LAST], 3);
    assert_eq!(f4.rps.ref_dpb_index[LAST], 0);
    assert_eq!((f4.ref_list0_count, f4.ref_list1_count), (2, 0));
    assert_eq!(f4.rps.refresh_frame_mask, 0x02, "toggle 0 -> 1");
    assert_eq!(
        ctx.dpb[1].picture_number, 4,
        "slot 1's POC-1 content is replaced"
    );

    // The long-term slot 7 is refreshed only every 128 base pictures; five
    // frames in, it must still hold the key frame.
    assert_eq!(ctx.dpb[7].picture_number, 0);
    assert_eq!(ctx.last_long_base_pic, 0);
}

/// The long-term base reference (`long_base_idx = 7`, `long_base_pic = 128`)
/// in the low-delay CQP branch.
///
/// Derivation: the guard is `picture_number - last_long_base_pic >= 128 &&
/// temporal_layer_index == 0`, applied AFTER `set_ref_list_counts` and BEFORE
/// `prune_refs`, so it ORs bit 7 into whatever mask the toggle produced.
#[test]
fn traced_long_base_ref_refresh_at_128() {
    let seq = ld_flat_cqp_seq();
    let mut ctx = pp::PicDecisionCtx::default();

    // POC 127: 127 - 0 < 128, so bit 7 is NOT set.
    let mut p127 = inter_frame(127, 0);
    pp::picture_decision_per_picture(&mut p127, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(p127.rps.refresh_frame_mask & 0x80, 0);
    assert_eq!(ctx.last_long_base_pic, 0);

    // POC 128: 128 - 0 >= 128, so bit 7 is ORed in and the marker moves.
    let mut ctx = pp::PicDecisionCtx::default();
    let mut p128 = inter_frame(128, 0);
    pp::picture_decision_per_picture(&mut p128, &seq, &mut ctx, 0, 0).unwrap();
    assert_ne!(p128.rps.refresh_frame_mask & 0x80, 0);
    assert_eq!(ctx.last_long_base_pic, 128);

    // A non-base picture never refreshes it, however far past the interval.
    let mut ctx = pp::PicDecisionCtx::default();
    let mut nb = inter_frame(500, 0);
    nb.temporal_layer_index = 1;
    nb.hierarchical_levels = 1;
    pp::picture_decision_per_picture(&mut nb, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(nb.rps.refresh_frame_mask & 0x80, 0);
    assert_eq!(ctx.last_long_base_pic, 0);
}

/// The random-access FLAT branch (`pd_process.c:2238-2269`) — the toggle
/// advances AFTER `prune_refs`, not before.
///
/// Derivation on the first inter frame with a DPB freshly filled by a key
/// frame: `base0 = lay0_toggle = 0`, and CIRC_DEC over the FULL 0..7 range
/// gives base1 = 7, base2 = 6, base3 = 5, base4 = 4, base5 = 3, base7 = 2.
/// So pre-prune `[0, 6, 4, 2, 7, 5, 3]`, and the mask is `1 << CIRC_INC(0)`
/// = 0x02 — computed from the toggle AFTER the slot reads, which is why the
/// LAST slot is 0 and not 1.
#[test]
fn traced_random_access_flat_toggle_order() {
    let seq = pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        rate_control_mode: pp::RcMode::CqpOrCrf,
        ..ld_flat_cqp_seq()
    };
    let mut ctx = pp::PicDecisionCtx::default();
    let mut kf = key_frame(0);
    kf.pred_struct_type = pp::PredStructure::RandomAccess;
    pp::picture_decision_per_picture(&mut kf, &seq, &mut ctx, 0, 0).unwrap();

    let mut f1 = inter_frame(1, 0);
    f1.pred_struct_type = pp::PredStructure::RandomAccess;
    pp::picture_decision_per_picture(&mut f1, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(f1.rps.refresh_frame_mask, 0x02);
    assert_eq!(ctx.lay0_toggle, 1);
    // Flat RA always shows the frame (C overrides set_frame_display_params).
    assert!(f1.show_frame && !f1.has_show_existing);
    // Every DPB slot holds POC 0, so all counts collapse the same way the
    // low-delay branch does and LAST keeps slot 0.
    assert_eq!(f1.rps.ref_dpb_index[LAST], 0);

    // Second inter frame: base0 = 1 (POC 1), base1 = CIRC_DEC(1,0,7) = 0.
    let mut f2 = inter_frame(2, 0);
    f2.pred_struct_type = pp::PredStructure::RandomAccess;
    pp::picture_decision_per_picture(&mut f2, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(
        f2.rps.ref_poc_array[LAST], 1,
        "slot 1 was refreshed by frame 1"
    );
    assert_eq!(f2.rps.refresh_frame_mask, 0x04);
}

/// An unported random-access hierarchical branch REFUSES rather than guessing.
///
/// `docs/WORKING-ON-THIS.md` §6: a plausible-but-wrong reference structure is
/// indistinguishable from a correct one at the seam, so the port returns a
/// typed error for `hierarchical_levels` 1..=5 under random access.
#[test]
fn unported_random_access_hierarchies_refuse() {
    let seq = pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        ..ld_flat_cqp_seq()
    };
    for hier in 1u8..=5 {
        let mut ctx = pp::PicDecisionCtx::default();
        let mut pic = inter_frame(1, 0);
        pic.pred_struct_type = pp::PredStructure::RandomAccess;
        pic.hierarchical_levels = hier;
        pic.temporal_layer_index = 0;
        let r = pp::picture_decision_per_picture(&mut pic, &seq, &mut ctx, 0, 0);
        assert_eq!(
            r,
            Err(pp::RpsBranchUnsupported {
                hierarchical_levels: hier,
                temporal_layer: 0
            }),
            "RA hierarchical level {hier} must refuse, not guess"
        );
    }
    // The four branches that ARE ported must not refuse -- a positive control
    // so a blanket "always Err" cannot pass this test.
    let ld = ld_flat_cqp_seq();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut pic = inter_frame(1, 0);
    assert!(pp::picture_decision_per_picture(&mut pic, &ld, &mut ctx, 0, 0).is_ok());
}

/// The RTC flat branch (`pd_process.c:1954-1986`) — the refresh mask includes
/// the unused slots.
///
/// Derivation at `flat_max_refs = 4`: slots 0..3 rotate, so the mask is
/// `(1 << CIRC_INC(lay0_toggle, 0, 3)) | 0xf0`, and the `for (i = 3; i >=
/// max_refs; --i)` loop adds nothing. At `flat_max_refs = 2` the loop adds
/// bits 3 and 2, giving `(1 << toggle) | 0xfc`.
#[test]
fn traced_rtc_flat_refresh_mask_covers_unused_slots() {
    let mut seq = ld_flat_cqp_seq();
    seq.rtc = true;
    seq.mrp_ctrls.flat_max_refs = 4;
    let mut ctx = pp::PicDecisionCtx::default();
    let mut f1 = inter_frame(1, 0);
    pp::picture_decision_per_picture(&mut f1, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(f1.rps.refresh_frame_mask, 0x02 | 0xf0);
    // Slots 0..3 in decreasing recency; list 1 mirrors LAST pre-prune.
    assert_eq!(ctx.lay0_toggle, 1);

    seq.mrp_ctrls.flat_max_refs = 2;
    let mut ctx = pp::PicDecisionCtx::default();
    let mut f1 = inter_frame(1, 0);
    pp::picture_decision_per_picture(&mut f1, &seq, &mut ctx, 0, 0).unwrap();
    assert_eq!(f1.rps.refresh_frame_mask, 0x02 | 0xfc);
    assert_eq!(ctx.lay0_toggle, 1, "CIRC_INC(0, 0, 1) = 1");
}

/// `set_frame_display_params` (`pd_process.c:1132-1161`) — all four outcomes.
#[test]
fn traced_set_frame_display_params() {
    let mut ctx = pp::PicDecisionCtx::default();
    ctx.mini_gop_length[0] = 4;

    // Low delay: always shown, never show-existing, returns true.
    let mut pic = pp::PicParams {
        pred_struct_type: pp::PredStructure::LowDelay,
        slice_type: pp::SliceType::B,
        show_frame: false,
        ..Default::default()
    };
    assert!(pp::set_frame_display_params(&mut pic, &ctx, 0));
    assert!(pic.show_frame && !pic.has_show_existing);

    // Random access, I slice, BROKEN mini-GOP (length < entry count): shown.
    let mut pic = pp::PicParams {
        pred_struct_type: pp::PredStructure::RandomAccess,
        slice_type: pp::SliceType::I,
        pred_struct_entry_count: 8,
        show_frame: false,
        ..Default::default()
    };
    assert!(pp::set_frame_display_params(&mut pic, &ctx, 0));
    assert!(pic.show_frame);

    // Random access, I slice, COMPLETE mini-GOP: hidden.
    let mut pic = pp::PicParams {
        pred_struct_type: pp::PredStructure::RandomAccess,
        slice_type: pp::SliceType::I,
        pred_struct_entry_count: 4,
        show_frame: true,
        ..Default::default()
    };
    assert!(pp::set_frame_display_params(&mut pic, &ctx, 0));
    assert!(!pic.show_frame && !pic.has_show_existing);

    // Random access, B slice: returns FALSE -- the caller must decide from
    // the picture index. C leaves show_frame untouched in this case.
    let mut pic = pp::PicParams {
        pred_struct_type: pp::PredStructure::RandomAccess,
        slice_type: pp::SliceType::B,
        pred_struct_entry_count: 4,
        show_frame: false,
        ..Default::default()
    };
    assert!(!pp::set_frame_display_params(&mut pic, &ctx, 0));
    assert!(!pic.show_frame);

    // An OVERLAY takes the low-delay arm regardless of the pred structure.
    let mut pic = pp::PicParams {
        pred_struct_type: pp::PredStructure::RandomAccess,
        slice_type: pp::SliceType::B,
        is_overlay: true,
        show_frame: false,
        ..Default::default()
    };
    assert!(pp::set_frame_display_params(&mut pic, &ctx, 0));
    assert!(pic.show_frame);
}

/// `set_ref_frame_sign_bias` (`pd_process.c:4894-4909`) — the off-by-one
/// between the two index spaces, and the disable path.
///
/// `ref_frame_sign_bias` is indexed by `MvReferenceFrame` (1..7) while
/// `ref_order_hint` is indexed by `ref_frame - 1`. Slot 0 (INTRA_FRAME) is
/// never written.
#[test]
fn traced_set_ref_frame_sign_bias_index_spaces() {
    let seq = ld_flat_cqp_seq();
    // cur = 10; hints 8 and 9 are behind (bias 0), 11..14 ahead (bias 1),
    // and a hint EQUAL to cur has distance 0, which is not > 0 -> bias 0.
    let mut pic = pp::PicParams {
        cur_order_hint: 10,
        ref_order_hint: [8, 9, 10, 11, 12, 13, 14],
        ..Default::default()
    };
    pp::set_ref_frame_sign_bias(&mut pic, &seq);
    assert_eq!(pic.ref_frame_sign_bias, [0, 0, 0, 0, 1, 1, 1, 1]);

    // With order hints disabled the whole array stays zero.
    let seq_off = pp::SeqPicParams {
        order_hint_info: OrderHintInfo {
            enable_order_hint: false,
            order_hint_bits: 7,
        },
        ..ld_flat_cqp_seq()
    };
    let mut pic = pp::PicParams {
        cur_order_hint: 10,
        ref_order_hint: [11, 12, 13, 14, 15, 16, 17],
        ..Default::default()
    };
    pp::set_ref_frame_sign_bias(&mut pic, &seq_off);
    assert_eq!(pic.ref_frame_sign_bias, [0; 8]);
}

/// `circ_inc` / `circ_dec` (`pd_process.c:167-168`) over the ranges the RPS
/// branches actually use.
#[test]
fn traced_circ_inc_dec() {
    assert_eq!([0, 1, 2].map(|v| pp::circ_inc(v, 0, 2)), [1, 2, 0]);
    assert_eq!([0, 1, 2].map(|v| pp::circ_dec(v, 0, 2)), [2, 0, 1]);
    // The layer-1 window is offset, not zero-based.
    assert_eq!([3, 4, 5].map(|v| pp::circ_dec(v, 3, 5)), [5, 3, 4]);
    assert_eq!(pp::circ_inc(7, 0, 7), 0);
    assert_eq!(pp::circ_dec(0, 0, 7), 7);
    // A degenerate single-slot window is a fixed point in both directions.
    assert_eq!(pp::circ_inc(0, 0, 0), 0);
    assert_eq!(pp::circ_dec(0, 0, 0), 0);
}

/// Silence the unused-import lint if a constant is only used in a comment.
#[test]
fn ref_index_constants_are_the_c_order() {
    assert_eq!(
        [LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT],
        [0, 1, 2, 3, 4, 5, 6]
    );
}
