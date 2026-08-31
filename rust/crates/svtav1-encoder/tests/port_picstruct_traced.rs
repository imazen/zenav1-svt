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
    let ctx = pp::PicDecisionCtx {
        mini_gop_length: {
            let mut a = [0u32; 8];
            a[0] = 4;
            a
        },
        ..Default::default()
    };

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

// ---------------------------------------------------------------------------
// Mini-GOP window map — tier 4 (all four functions are `static` in C)
// ---------------------------------------------------------------------------

fn enc_ctx(count: u32, intra: u32, idr: u32) -> pp::EncCtxPicParams {
    pp::EncCtxPicParams {
        pre_assignment_buffer_count: count,
        pre_assignment_buffer_intra_count: intra,
        pre_assignment_buffer_idr_count: idr,
        ..Default::default()
    }
}

/// `initialize_mini_gop_activity_array` + `generate_picture_window_split` +
/// `handle_incomplete_picture_window_map` on an EXACT 8-picture buffer.
///
/// Derivation. The activity array starts as `hierarchical_levels > 1` per
/// entry, so every 2-picture (L1) shape is already inactive. With
/// `count == 8` and no IDR the cascade clears `L4_0_INDEX` (entry 2, the
/// `{3, 0, 7, 8}` shape) and neither nested arm fires because `8 - 8 == 0`.
///
/// The split loop then walks: entries 0 and 1 have `end_index` 31 and 15,
/// both >= 8, so they are skipped but stay ACTIVE and stride by 1. Entry 2
/// has `end_index == 7 < 8` and is inactive, so it is emitted — and strides by
/// `mini_gop_offset[3 - 1] == 7`, jumping the whole subtree to entry 9. No
/// later entry has `end_index < 8`, so exactly ONE mini-GOP comes out.
///
/// `handle_incomplete_picture_window_map` then finds `end_index[0] == 7 ==
/// count - 1` and adds nothing.
#[test]
fn traced_mini_gop_window_split_exact_8() {
    let mut map = pp::MiniGopMap::default();
    let enc = enc_ctx(8, 1, 1);
    let needs_dg = pp::initialize_mini_gop_activity_array(&mut map, &enc, false, false, false);
    assert!(!needs_dg, "enable_dg is off in this cell");
    assert!(
        !map.activity[pp::L4_0_INDEX],
        "the {{3,0,7,8}} shape is chosen"
    );
    assert!(map.activity[pp::L6_INDEX] && map.activity[pp::L5_0_INDEX]);

    pp::generate_picture_window_split(&mut map, &enc);
    assert_eq!(map.total_number_of_mini_gops, 1);
    assert_eq!(
        (map.start_index[0], map.end_index[0], map.length[0]),
        (0, 7, 8)
    );
    assert_eq!(map.hierarchical_levels[0], 3);
    assert_eq!((map.intra_count[0], map.idr_count[0]), (1, 1));

    pp::handle_incomplete_picture_window_map(3, &mut map, &enc);
    assert_eq!(map.total_number_of_mini_gops, 1, "nothing to fix up");
}

/// The SHORT last mini-GOP — the case a 5-frame test cell hits first.
///
/// Derivation at `count == 5`: the cascade clears `L3_0_INDEX` (entry 3, the
/// `{2, 0, 3, 4}` shape) and `5 - 4 == 1` is not >= 2, so nothing nests. The
/// split emits the 4-picture shape and strides by `mini_gop_offset[2 - 1] == 3`
/// to entry 6, whose `end_index` is 7 >= 5. So one mini-GOP covering 0..3 and
/// picture 4 is left over.
///
/// `handle_incomplete_picture_window_map` sees `end_index[0] == 3 < 4` and
/// appends a second mini-GOP of length 1 at `MIN_HIERARCHICAL_LEVEL`, ZEROING
/// the counts on the previous entry as it goes.
#[test]
fn traced_mini_gop_window_split_short_tail_5() {
    let mut map = pp::MiniGopMap::default();
    let enc = enc_ctx(5, 1, 1);
    pp::initialize_mini_gop_activity_array(&mut map, &enc, false, false, false);
    assert!(!map.activity[pp::L3_0_INDEX]);

    pp::generate_picture_window_split(&mut map, &enc);
    assert_eq!(map.total_number_of_mini_gops, 1);
    assert_eq!(
        (map.start_index[0], map.end_index[0], map.length[0]),
        (0, 3, 4)
    );
    assert_eq!(map.hierarchical_levels[0], 2);

    pp::handle_incomplete_picture_window_map(3, &mut map, &enc);
    assert_eq!(map.total_number_of_mini_gops, 2);
    assert_eq!(
        (map.start_index[1], map.end_index[1], map.length[1]),
        (4, 4, 1)
    );
    assert_eq!(
        map.hierarchical_levels[1],
        u32::from(pp::MIN_HIERARCHICAL_LEVEL)
    );
    // The counts moved from the first entry to the new last one.
    assert_eq!((map.intra_count[0], map.idr_count[0]), (0, 0));
    assert_eq!((map.intra_count[1], map.idr_count[1]), (1, 1));
}

/// The IDR guard: `count >= N && !(count == N && idr_flag)`.
///
/// Derivation at `count == 4`: WITHOUT an IDR the cascade clears
/// `L3_0_INDEX`, giving one 4-picture mini-GOP. WITH an IDR the `count == 4`
/// arm is refused, control falls to the `>= 2` arm which clears
/// `L2_0_INDEX` — already inactive, so the array is unchanged — and the split
/// therefore emits the two 2-picture shapes (entries 4 and 5) instead.
///
/// So a 4-picture buffer headed by an IDR is coded as TWO mini-GOPs, not one.
/// That is the off-by-one this guard exists for and it only shows at a GOP
/// boundary.
#[test]
fn traced_mini_gop_idr_guard_splits_the_buffer() {
    let enc = enc_ctx(4, 1, 1);

    let mut no_idr = pp::MiniGopMap::default();
    pp::initialize_mini_gop_activity_array(&mut no_idr, &enc, false, false, false);
    pp::generate_picture_window_split(&mut no_idr, &enc);
    assert_eq!(no_idr.total_number_of_mini_gops, 1);
    assert_eq!((no_idr.start_index[0], no_idr.end_index[0]), (0, 3));
    assert_eq!(no_idr.hierarchical_levels[0], 2);

    let mut with_idr = pp::MiniGopMap::default();
    pp::initialize_mini_gop_activity_array(&mut with_idr, &enc, true, false, false);
    assert!(
        with_idr.activity[pp::L3_0_INDEX],
        "the 4-picture shape stays ACTIVE"
    );
    pp::generate_picture_window_split(&mut with_idr, &enc);
    assert_eq!(with_idr.total_number_of_mini_gops, 2);
    assert_eq!(
        (
            with_idr.start_index[0],
            with_idr.end_index[0],
            with_idr.length[0]
        ),
        (0, 1, 2)
    );
    assert_eq!(
        (
            with_idr.start_index[1],
            with_idr.end_index[1],
            with_idr.length[1]
        ),
        (2, 3, 2)
    );
    assert_eq!(with_idr.hierarchical_levels[0], 1);
}

/// `set_mini_gop_structure` in LOW DELAY: `pre_assignment_buffer_count` is 1,
/// so the subdivision NEVER runs and the single default mini-GOP stands.
///
/// This is the "degenerates in low delay" case named in the lane brief; the
/// point of the test is that it degenerates to a WELL-DEFINED map, not to an
/// unset one.
#[test]
fn traced_set_mini_gop_structure_low_delay_degenerates() {
    let seq = ld_flat_cqp_seq();
    let mut map = pp::MiniGopMap::for_sequence(0);
    let mut enc = enc_ctx(1, 0, 0);
    let pic = inter_frame(1, 0);
    let needs_dg =
        pp::set_mini_gop_structure(&mut map, &mut enc, &seq, &pic, 0, 0, false, true, false);
    assert!(
        !needs_dg,
        "the subdivision never runs, so the dg split cannot fire"
    );
    assert_eq!(map.total_number_of_mini_gops, 1);
    assert_eq!(
        (map.start_index[0], map.end_index[0], map.length[0]),
        (0, 0, 1)
    );
    assert_eq!(
        map.hierarchical_levels[0], 0,
        "the configured level, not a mini-GOP one"
    );
    // mini_gop_cnt_per_gop increments when the buffer holds no IDR.
    assert_eq!(enc.mini_gop_cnt_per_gop, 1);

    // With an IDR in the buffer the per-GOP counter RESETS instead.
    let mut enc = enc_ctx(1, 1, 1);
    enc.mini_gop_cnt_per_gop = 9;
    let mut map = pp::MiniGopMap::for_sequence(0);
    pp::set_mini_gop_structure(&mut map, &mut enc, &seq, &pic, 0, 0, true, false, false);
    assert_eq!(enc.mini_gop_cnt_per_gop, 0);
}

/// `set_mini_gop_structure` in RANDOM ACCESS runs the full subdivision, and
/// reports that the dynamic-GOP split is required when `enable_dg` is set and
/// the 6L shape was chosen.
///
/// Measured (`enc_handle.c:4294-4300`): `enable_dg` is 1 for single-pass
/// CQP/CRF `RANDOM_ACCESS` below 4K, so the `true` return is the DEFAULT there,
/// not an exotic knob. `eval_sub_mini_gop` itself is not ported.
#[test]
fn traced_set_mini_gop_structure_random_access_reports_dg() {
    let seq = pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        ..ld_flat_cqp_seq()
    };
    let mut map = pp::MiniGopMap::for_sequence(5);
    let mut enc = enc_ctx(32, 0, 0);
    let pic = inter_frame(1, 0);
    let needs_dg =
        pp::set_mini_gop_structure(&mut map, &mut enc, &seq, &pic, 5, 0, false, true, false);
    // count == 32 clears L6_INDEX, so the dg evaluation is required.
    assert!(needs_dg);
    assert!(!map.activity[pp::L6_INDEX]);
    assert_eq!(map.total_number_of_mini_gops, 1);
    assert_eq!(
        (map.start_index[0], map.end_index[0], map.length[0]),
        (0, 31, 32)
    );
    assert_eq!(map.hierarchical_levels[0], 5);

    // With enable_dg off the same buffer reports no dg work.
    let mut map = pp::MiniGopMap::for_sequence(5);
    let mut enc = enc_ctx(32, 0, 0);
    assert!(!pp::set_mini_gop_structure(
        &mut map, &mut enc, &seq, &pic, 5, 0, false, false, false
    ));
}

/// `get_pred_struct_for_frame` (`pd_process.c:942-988`) — an IDR takes the
/// SEQUENCE hierarchy, everyone else takes the MINI-GOP's.
#[test]
fn traced_get_pred_struct_for_frame_idr_takes_sequence_hierarchy() {
    let mut map = pp::MiniGopMap {
        hierarchical_levels: {
            let mut a = [0u32; pp::MINI_GOP_MAX_COUNT];
            a[0] = 2;
            a
        },
        ..Default::default()
    };

    let mut idr = key_frame(0);
    pp::get_pred_struct_for_frame(
        &mut idr,
        &mut map,
        0,
        pp::PredStructure::RandomAccess,
        5,
        0,
        true,
        false,
    );
    assert_eq!(
        idr.hierarchical_levels, 5,
        "IDR -> the configured 5, not the MG's 2"
    );
    assert_eq!(idr.pred_struct_type, pp::PredStructure::RandomAccess);
    assert!(map.is_startup_gop, "an IDR at POC 0 opens the startup GOP");

    let mut b = inter_frame(1, 0);
    pp::get_pred_struct_for_frame(
        &mut b,
        &mut map,
        0,
        pp::PredStructure::RandomAccess,
        5,
        0,
        false,
        false,
    );
    assert_eq!(b.hierarchical_levels, 2, "non-IDR -> the mini-GOP's 2");
    assert!(map.is_startup_gop, "unchanged by a non-key picture");

    // A later IDR (POC != 0) CLOSES the startup GOP.
    let mut idr2 = key_frame(64);
    pp::get_pred_struct_for_frame(
        &mut idr2,
        &mut map,
        0,
        pp::PredStructure::RandomAccess,
        5,
        0,
        true,
        false,
    );
    assert!(!map.is_startup_gop);
}

/// `store_mg_picture_arrays` (`pd_process.c:4966-4985`) — display order in,
/// decode order out.
///
/// Derivation with a 4-picture random-access mini-GOP: display order
/// [P4, P1, P2, P3] carries decode orders [0, 2, 1, 3] (the base layer is
/// coded first), so the decode-order permutation is [0, 2, 1, 3].
#[test]
fn traced_store_mg_picture_arrays_sorts_by_decode_order() {
    let (decode, display) = pp::store_mg_picture_arrays(&[0, 2, 1, 3]);
    assert_eq!(display, [0, 1, 2, 3], "the display copy is the input order");
    assert_eq!(decode, [0, 2, 1, 3]);

    // A fully reversed decode order.
    let (decode, _) = pp::store_mg_picture_arrays(&[3, 2, 1, 0]);
    assert_eq!(decode, [3, 2, 1, 0]);

    // An 8-picture 3L mini-GOP: display [P8,P1..P7] has decode orders
    // [0, 3, 2, 4, 1, 6, 5, 7].
    let (decode, _) = pp::store_mg_picture_arrays(&[0, 3, 2, 4, 1, 6, 5, 7]);
    assert_eq!(decode, [0, 4, 2, 1, 3, 6, 5, 7]);

    // Degenerate sizes must not panic.
    assert_eq!(pp::store_mg_picture_arrays(&[]).0, Vec::<usize>::new());
    assert_eq!(pp::store_mg_picture_arrays(&[5]).0, [0]);
}

/// `get_pic_idx_in_mg` (`pd_process.c:4872-4893`) — two different quantities
/// out of one call.
///
/// Derivation, low delay: `pic_idx_in_mg` is 0 when `pred_struct_position` is
/// 0 and `(position - 1) % entry_count` otherwise — NOT the position itself.
/// `frame_offset` is `picture_number - last_idr_picture`, a different
/// quantity, and it is written on every low-delay call.
#[test]
fn traced_get_pic_idx_in_mg_low_delay_and_random_access() {
    let seq = ld_flat_cqp_seq();
    let map = pp::MiniGopMap::default();

    for (position, want_idx) in [(0u32, 0u32), (1, 0), (2, 1), (3, 2), (4, 3), (5, 0)] {
        let mut pic = inter_frame(10, 3);
        pic.pred_struct_entry_count = 4;
        let enc = pp::EncCtxPicParams {
            pred_struct_position: position,
            last_idr_picture: 3,
            ..Default::default()
        };
        let got = pp::get_pic_idx_in_mg(&mut pic, &seq, &enc, &map, 0, 0);
        assert_eq!(got, want_idx, "low delay, pred_struct_position {position}");
        assert_eq!(pic.frame_offset, 7, "10 - 3, written on every call");
    }

    // Random access: the index is the offset from the mini-GOP start, and
    // frame_offset is NOT touched.
    let ra = pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        ..ld_flat_cqp_seq()
    };
    let map = pp::MiniGopMap {
        start_index: {
            let mut a = [0u32; pp::MINI_GOP_MAX_COUNT];
            a[1] = 4;
            a
        },
        ..Default::default()
    };
    let mut pic = inter_frame(10, 3);
    pic.frame_offset = 999;
    assert_eq!(
        pp::get_pic_idx_in_mg(&mut pic, &ra, &Default::default(), &map, 6, 1),
        2
    );
    assert_eq!(
        pic.frame_offset, 999,
        "random access leaves frame_offset alone"
    );
}

/// `update_pred_struct_and_pic_type` (`pd_process.c:4814-4871`) — the position
/// if/else CHAIN and its priority.
#[test]
fn traced_update_pred_struct_and_pic_type_position_chain() {
    let base_map = || pp::MiniGopMap {
        length: {
            let mut a = [0u32; pp::MINI_GOP_MAX_COUNT];
            a[0] = 8;
            a
        },
        ..Default::default()
    };
    let base_pic = || {
        let mut p = inter_frame(10, 0);
        p.pred_struct_entry_count = 8;
        p.pred_struct_type = pp::PredStructure::RandomAccess;
        p
    };

    // Not cutting short (length == entry_count, no IDR count), not IDR/CRA,
    // elapsed_non_cra_count > 0 -> ordinary increment, B slice.
    let mut map = base_map();
    let mut pic = base_pic();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut enc = pp::EncCtxPicParams {
        pred_struct_position: 3,
        elapsed_non_cra_count: 5,
        ..Default::default()
    };
    let st = pp::update_pred_struct_and_pic_type(
        &mut pic, &mut enc, &mut map, &mut ctx, 0, false, false, false, false, 0,
    );
    assert_eq!(st, pp::SliceType::B);
    assert_eq!(enc.pred_struct_position, 4);
    assert_eq!(ctx.cut_short_ra_mg, 0);

    // An IDR resets to init_pic_index, gives an I slice, and records the POC.
    let mut map = base_map();
    let mut pic = base_pic();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut enc = pp::EncCtxPicParams {
        pred_struct_position: 3,
        elapsed_non_cra_count: 5,
        ..Default::default()
    };
    let st = pp::update_pred_struct_and_pic_type(
        &mut pic, &mut enc, &mut map, &mut ctx, 0, false, true, false, false, 1,
    );
    assert_eq!(st, pp::SliceType::I);
    assert_eq!(enc.pred_struct_position, 1);
    assert_eq!(enc.last_idr_picture, 10);

    // Directly after a CRA (elapsed_non_cra_count == 0) -> init_pic_index + 1,
    // NOT init_pic_index. This arm sits BELOW the IDR and CRA arms in the
    // chain, so it only fires for an ordinary picture.
    let mut map = base_map();
    let mut pic = base_pic();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut enc = pp::EncCtxPicParams {
        pred_struct_position: 3,
        elapsed_non_cra_count: 0,
        ..Default::default()
    };
    pp::update_pred_struct_and_pic_type(
        &mut pic, &mut enc, &mut map, &mut ctx, 0, false, false, false, false, 1,
    );
    assert_eq!(enc.pred_struct_position, 2);

    // Cutting short a random-access mini-GOP switches the picture to LOW_DELAY
    // and forces a B slice even though the mini-GOP holds an IDR.
    let mut map = base_map();
    map.idr_count[0] = 1;
    let mut pic = base_pic();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut enc = pp::EncCtxPicParams {
        pred_struct_position: 5,
        elapsed_non_cra_count: 5,
        ..Default::default()
    };
    let st = pp::update_pred_struct_and_pic_type(
        &mut pic, &mut enc, &mut map, &mut ctx, 0, true, false, false, false, 2,
    );
    assert_eq!(st, pp::SliceType::B);
    assert_eq!(pic.pred_struct_type, pp::PredStructure::LowDelay);
    assert_eq!(ctx.cut_short_ra_mg, 1);
    // The first-pass correction subtracted init_pic_index (5 - 2 = 3), then
    // the ordinary increment made it 4.
    assert_eq!(enc.pred_struct_position, 4);

    // The wrap: position == entry_count wraps to 0.
    let mut map = base_map();
    let mut pic = base_pic();
    let mut ctx = pp::PicDecisionCtx::default();
    let mut enc = pp::EncCtxPicParams {
        pred_struct_position: 7,
        elapsed_non_cra_count: 5,
        ..Default::default()
    };
    pp::update_pred_struct_and_pic_type(
        &mut pic, &mut enc, &mut map, &mut ctx, 0, false, false, false, false, 0,
    );
    assert_eq!(enc.pred_struct_position, 0, "8 wraps to 0 at entry_count 8");
}

/// `perform_sc_detection` (`pd_process.c:4769-4813`) — inter frames INHERIT.
///
/// This is the half that matters for parity: without it a port re-detects per
/// frame and flips palette / IntraBC / SC-tuned thresholds mid-GOP.
#[test]
fn traced_perform_sc_detection_inheritance() {
    let mut last_i = pp::ScClasses::default();
    let detected = pp::ScClasses {
        class: [1, 0, 1, 0, 1, 0],
        is_luma_dominant_input: true,
    };

    // An I picture publishes its classes.
    let got = pp::perform_sc_detection(true, detected, &mut last_i);
    assert_eq!(got, detected);
    assert_eq!(last_i, detected);

    // Every following inter picture inherits them, ignoring whatever its own
    // (never-run) detection would have produced.
    let bogus = pp::ScClasses {
        class: [9; 6],
        is_luma_dominant_input: false,
    };
    for _ in 0..3 {
        assert_eq!(
            pp::perform_sc_detection(false, bogus, &mut last_i),
            detected
        );
    }
    assert_eq!(
        last_i, detected,
        "an inter picture never updates the context"
    );
}

/// `avail_past_pictures` (`pd_process.c:3592-3605`) — the temporal-filter
/// window cap at the start of a sequence.
#[test]
fn traced_avail_past_pictures() {
    assert_eq!(pp::avail_past_pictures(&[], 5), 0);
    assert_eq!(pp::avail_past_pictures(&[0, 1, 2, 3, 4, 5, 6], 4), 4);
    assert_eq!(pp::avail_past_pictures(&[5], 5), 0, "equal is not past");
    assert_eq!(pp::avail_past_pictures(&[9, 8, 7], 5), 0);
    assert_eq!(
        pp::avail_past_pictures(&[0, 1, 2], 0),
        0,
        "the sequence start"
    );
}

// ---------------------------------------------------------------------------
// TPL group — tier 4 for the `static` half of initial_rc_process.c
// ---------------------------------------------------------------------------

/// `get_tpl_params_level` (`initial_rc_process.c:307-318`) — static.
///
/// Derivation: `<= ENC_M2` -> 1, `<= ENC_M7` -> 4, else 5. Level 4 covers
/// M3..M7, which is the band the default random-access presets sit in.
#[test]
fn traced_get_tpl_params_level() {
    for m in -1i8..=2 {
        assert_eq!(pp::get_tpl_params_level(m), 1, "enc_mode {m}");
    }
    for m in 3i8..=7 {
        assert_eq!(pp::get_tpl_params_level(m), 4, "enc_mode {m}");
    }
    for m in 8i8..=13 {
        assert_eq!(pp::get_tpl_params_level(m), 5, "enc_mode {m}");
    }
}

/// `set_tpl_params` (`initial_rc_process.c:319-405`) — static.
///
/// Two things this test pins that a simplified port loses:
/// * it MUTATES an existing `TplControls`, so `enable`, `reduced_tpl_group`,
///   `r0_adjust_factor` and `synth_blk_size` from `svt_aom_set_tpl_group`
///   survive the call;
/// * `pf_shape` is resolution-dependent from level 2 up
///   (`<= INPUT_SIZE_480p_RANGE ? N2_SHAPE : N4_SHAPE`), and levels 0 and 1
///   use `DEFAULT_SHAPE` regardless.
#[test]
fn traced_set_tpl_params_mutates_and_keys_off_resolution() {
    // Start from a state svt_aom_set_tpl_group would have produced.
    let mut t = pp::TplControls {
        enable: 1,
        reduced_tpl_group: 3,
        synth_blk_size: 32,
        r0_adjust_factor: 1.6,
        ..Default::default()
    };
    pp::set_tpl_params(&mut t, 1, 5);
    assert_eq!(
        (t.enable, t.reduced_tpl_group, t.synth_blk_size),
        (1, 3, 32)
    );
    assert!(
        (t.r0_adjust_factor - 1.6).abs() < f64::EPSILON,
        "untouched by set_tpl_params"
    );
    assert_eq!(
        (t.compute_rate, t.enable_tpl_qps),
        (1, 1),
        "only level 1 computes rate"
    );
    assert_eq!(t.intra_mode_end, pp::PAETH_PRED);
    assert_eq!(t.pf_shape, pp::DEFAULT_SHAPE);
    assert_eq!(t.subpel_depth, pp::QUARTER_PEL);

    // Level 0 is the only other DEFAULT_SHAPE level, and it is FULL_PEL.
    let mut t = pp::TplControls::default();
    pp::set_tpl_params(&mut t, 0, 5);
    assert_eq!(t.pf_shape, pp::DEFAULT_SHAPE);
    assert_eq!(t.subpel_depth, pp::FULL_PEL);

    // Levels 2..5 take N2_SHAPE at <= 480p and N4_SHAPE above it.
    for level in 2u8..=5 {
        let mut small = pp::TplControls::default();
        pp::set_tpl_params(&mut small, level, pp::INPUT_SIZE_480P_RANGE);
        assert_eq!(small.pf_shape, pp::N2_SHAPE, "level {level} at <= 480p");

        let mut big = pp::TplControls::default();
        pp::set_tpl_params(&mut big, level, pp::INPUT_SIZE_480P_RANGE + 1);
        assert_eq!(big.pf_shape, pp::N4_SHAPE, "level {level} above 480p");
        assert_eq!(
            big.disable_intra_pred_nref, 1,
            "levels 2..5 disable NREF intra"
        );
        assert_eq!(big.use_sad_in_src_search, 1);
    }

    // Level 5 is the only one that raises dispenser_search_level / subsample_tx.
    let mut t = pp::TplControls::default();
    pp::set_tpl_params(&mut t, 5, 5);
    assert_eq!((t.dispenser_search_level, t.subsample_tx), (1, 2));
    let mut t = pp::TplControls::default();
    pp::set_tpl_params(&mut t, 4, 5);
    assert_eq!((t.dispenser_search_level, t.subsample_tx), (0, 0));
}

/// `is_frame_already_exists` + `validate_pic_for_tpl`
/// (`initial_rc_process.c:161-189`).
///
/// Trap: `reduced_tpl_group == 0` means "base layer only", not "no
/// reduction" — that is -1. A port that treated 0 as the off value would admit
/// every layer.
#[test]
fn traced_validate_pic_for_tpl_reduced_group_zero_is_base_only() {
    let pocs = [10u64, 11, 12, 11];
    let layers = [0u8, 1, 2, 1];

    // Duplicate at index 3 (POC 11 already at index 1) -> rejected.
    assert!(pp::is_frame_already_exists(&pocs, 3, pocs[3]));
    assert!(!pp::validate_pic_for_tpl(&pocs, &layers, 3, -1, false));

    // reduced_tpl_group == -1 admits every layer.
    for i in 0..3 {
        assert!(
            pp::validate_pic_for_tpl(&pocs, &layers, i, -1, false),
            "index {i}"
        );
    }

    // reduced_tpl_group == 0 admits ONLY temporal layer 0.
    assert!(pp::validate_pic_for_tpl(&pocs, &layers, 0, 0, false));
    assert!(!pp::validate_pic_for_tpl(&pocs, &layers, 1, 0, false));
    assert!(!pp::validate_pic_for_tpl(&pocs, &layers, 2, 0, false));

    // reduced_tpl_group == 1 admits layers 0 and 1.
    assert!(pp::validate_pic_for_tpl(&pocs, &layers, 1, 1, false));
    assert!(!pp::validate_pic_for_tpl(&pocs, &layers, 2, 1, false));

    // A skipped picture is rejected whatever the group setting.
    assert!(!pp::validate_pic_for_tpl(&pocs, &layers, 0, -1, true));
}

/// `store_extended_group`'s group-selection half
/// (`initial_rc_process.c:439-497`).
///
/// Derivation for the asymmetric intra arms, which is the part worth pinning:
/// a NON-delayed intra at `i != 0` is ADDED and then closes the GOP, while a
/// DELAYED intra at `i != 0` breaks WITHOUT being added. After the close, only
/// pictures carrying the same `ext_mg_id` as that intra continue.
#[test]
fn traced_store_extended_group_intra_arms_are_asymmetric() {
    let mk = |poc: u64, i_slice: bool, layer: u8, mg: i64, delayed: bool| pp::ExtGroupPic {
        picture_number: poc,
        slice_type: if i_slice {
            pp::SliceType::I
        } else {
            pp::SliceType::B
        },
        temporal_layer_index: layer,
        ext_mg_id: mg,
        is_delayed_intra: delayed,
        is_skipped: false,
    };

    // A DELAYED intra at index 3 breaks BEFORE being added: members are 0..2.
    let ext = [
        mk(0, false, 0, 0, false),
        mk(1, false, 1, 0, false),
        mk(2, false, 1, 0, false),
        mk(3, true, 0, 1, true),
        mk(4, false, 1, 1, false),
    ];
    let g = pp::store_extended_group(&ext, pp::SliceType::B, 2, 1, -1);
    assert_eq!(g.members, [0, 1, 2]);

    // A NON-delayed intra at the same index IS added, closes the GOP, and the
    // following picture continues only because it shares ext_mg_id 1.
    let ext = [
        mk(0, false, 0, 0, false),
        mk(1, false, 1, 0, false),
        mk(2, false, 1, 0, false),
        mk(3, true, 0, 1, false),
        mk(4, false, 1, 1, false),
        mk(5, false, 1, 2, false),
    ];
    let g = pp::store_extended_group(&ext, pp::SliceType::B, 2, 1, -1);
    assert_eq!(
        g.members,
        [0, 1, 2, 3, 4],
        "index 5 has a different ext_mg_id"
    );

    // limited_tpl_group_size: a B slice takes (tpl_lad_mg + 1) * (1 << hier).
    // At hier 2 and tpl_lad_mg 0 that is 4, so only four members are walked.
    let ext: Vec<_> = (0..8).map(|i| mk(i, false, 1, 0, false)).collect();
    let g = pp::store_extended_group(&ext, pp::SliceType::B, 2, 0, -1);
    assert_eq!(g.members, [0, 1, 2, 3]);

    // An I slice gets ONE extra: 1 + (tpl_lad_mg + 1) * mg_size = 5.
    let g = pp::store_extended_group(&ext, pp::SliceType::I, 2, 0, -1);
    assert_eq!(g.members, [0, 1, 2, 3, 4]);

    // tpl_valid_pic[0] is forced to 1 before the loop even when the picture
    // would not validate -- here every picture is layer 1 with a reduced group
    // of 0, so nothing validates, yet slot 0 is still marked.
    let g = pp::store_extended_group(&ext, pp::SliceType::B, 2, 0, 0);
    assert_eq!(g.valid[0], 1);
    assert_eq!(g.used_tpl_frame_num, 0, "no picture passed validation");
    assert_eq!(&g.valid[1..4], &[0, 0, 0]);

    // With a group that admits layer 1, every walked picture validates.
    let g = pp::store_extended_group(&ext, pp::SliceType::B, 2, 0, 1);
    assert_eq!(g.used_tpl_frame_num, 4);

    // Degenerate: an empty extended group must not panic.
    let g = pp::store_extended_group(&[], pp::SliceType::B, 2, 0, -1);
    assert!(g.members.is_empty());
}

// ---------------------------------------------------------------------------
// primary_ref_frame + the send_picture_out count adjustment — tier 4
// ---------------------------------------------------------------------------

fn rq(poc: u64, layer: u8, q: u8, r0: f64) -> pp::RefQueueEntry {
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
fn traced_primary_ref_frame_largest_layer_not_exceeding_current() {
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
fn traced_primary_ref_frame_none_cases() {
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
fn traced_primary_ref_overlay_uses_own_poc() {
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
fn traced_primary_ref_missing_reference_panics() {
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
fn traced_send_picture_out_ref_counts() {
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
fn traced_copy_tf_params_low_delay_and_random_access() {
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
fn traced_list_and_ref_idx_tables() {
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

fn flat_hist(f: impl Fn(usize, usize, usize) -> u32) -> Box<pp::RegionHistograms> {
    let mut h = Box::new([[[0u32; 256]; 4]; 4]);
    for w in 0..4 {
        for y in 0..4 {
            for b in 0..256 {
                h[w][y][b] = f(w, y, b);
            }
        }
    }
    h
}

/// `calc_ahd` (`pd_process.c:55-84`) — the sum AND the active-region count.
///
/// Derivation: with a constant per-bin difference of `d` over 256 bins, each
/// region contributes `256 * d`, and the active-region test is
/// `ahd_per_region > region_width * region_height`. At 64x64 with a 4x4 grid
/// each region is 16x16 = 256 pixels, so `d = 1` gives exactly 256 which is
/// NOT `> 256` (no region counts), while `d = 2` gives 512 which is.
#[test]
fn traced_calc_ahd_sum_and_active_region_threshold() {
    let a = flat_hist(|_, _, _| 10);
    let b1 = flat_hist(|_, _, _| 9);
    let (ahd, active) = pp::calc_ahd(&a, &b1, 64, 64, 4, 4);
    assert_eq!(ahd, 16 * 256, "16 regions x 256 bins x |10-9|");
    assert_eq!(active, 0, "256 is not > 256");

    let b2 = flat_hist(|_, _, _| 8);
    let (ahd, active) = pp::calc_ahd(&a, &b2, 64, 64, 4, 4);
    assert_eq!(ahd, 16 * 512);
    assert_eq!(active, 16, "512 > 256 in every region");

    // A smaller active region grid changes the pixel count per region and so
    // the threshold: at 2x2 over 64x64 each region is 32x32 = 1024 pixels, so
    // 512 no longer qualifies -- and only 4 regions are visited at all.
    let (ahd, active) = pp::calc_ahd(&a, &b2, 64, 64, 2, 2);
    assert_eq!(ahd, 4 * 512);
    assert_eq!(active, 0);

    // Identical histograms: zero, and no active region.
    assert_eq!(pp::calc_ahd(&a, &a, 64, 64, 4, 4), (0, 0));
}

/// `calc_ahd_pd` (`pd_process.c:5192-5215`) — the SIMPLER of the two, with no
/// region-size test at all.
///
/// The two functions are easy to conflate; this pins that `calc_ahd_pd`
/// returns only the sum and never looks at the picture dimensions.
#[test]
fn traced_calc_ahd_pd_is_sum_only() {
    let a = flat_hist(|w, h, b| (w * 4 + h + b) as u32);
    let b = flat_hist(|w, h, b| (w * 4 + h + b) as u32 + 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 4, 4), 16 * 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 2, 2), 4 * 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 1, 1), 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &a, 4, 4), 0);
}

/// `copy_histograms` (`pd_process.c:4703-4719`) — it copies the FULL 4x4 grid.
///
/// The loops run over `MAX_NUMBER_OF_REGIONS_IN_{WIDTH,HEIGHT}`, not over the
/// sequence's active region counts, so regions the detector never reads are
/// still refreshed. A port that copied only the active regions would leave
/// stale data behind and diverge the moment the region count changed.
#[test]
fn traced_copy_histograms_copies_the_full_grid() {
    let mut state = pp::SceneDetectState::default();
    let h = flat_hist(|w, y, b| (w * 1000 + y * 100 + b) as u32);
    let mut inten = [[0u64; 4]; 4];
    for w in 0..4 {
        for y in 0..4 {
            inten[w][y] = (w * 10 + y) as u64;
        }
    }
    pp::copy_histograms(&mut state, &h, &inten);
    for w in 0..4 {
        for y in 0..4 {
            assert_eq!(
                state.prev_picture_histogram[w][y][255], h[w][y][255],
                "[{w}][{y}]"
            );
            assert_eq!(state.prev_average_intensity_per_region[w][y], inten[w][y]);
        }
    }
    // Including the corner a 2x2 detector would never touch.
    assert_eq!(state.prev_picture_histogram[3][3][0], h[3][3][0]);
}

/// `num_64x64_in_pic` — the `NUM64x64INPIC` macro's shift.
///
/// `svt_log2f(BLOCK_SIZE_64) << 1` is `6 << 1` = 12, so the macro is
/// `(w * h) >> 12`, not a division by 64.
#[test]
fn traced_num_64x64_in_pic_shift() {
    assert_eq!(pp::num_64x64_in_pic(64, 64), 1);
    assert_eq!(pp::num_64x64_in_pic(1920, 1080), (1920 * 1080) >> 12);
    assert_eq!(
        pp::num_64x64_in_pic(63, 63),
        0,
        "a sub-64x64 region rounds to zero"
    );
    assert_eq!(pp::num_64x64_in_pic(0, 0), 0);
}

/// `scene_transition_detector` (`pd_process.c:256-378`) — the accumulating
/// region size, which is the quirk that makes this function hard to port.
///
/// Derivation. `region_width` and `region_height` are declared OUTSIDE the
/// region loops and updated inside with `+=`. At 1918x1078 with a 4x4 grid the
/// base sizes are 479 and 269, and the remainders are
/// `1918 - 4*479 = 2` and `1078 - 4*269 = 2`. The height remainder is added on
/// EVERY last-height iteration, i.e. once per width column, so `region_height`
/// is 269, 271, 273, 275 in successive columns; the width remainder is added
/// on every iteration of the FINAL column, so `region_width` walks 479 -> 487
/// there. The threshold therefore differs between regions of identical actual
/// size.
///
/// This test does not re-derive the whole detector; it pins the observable
/// consequence: with a picture size that divides evenly the verdict is
/// insensitive to which region carries a given difference, and with one that
/// does not, it is not.
#[test]
fn traced_scene_transition_detector_region_size_accumulates() {
    // A difference big enough to trip the threshold in a SMALL region but not
    // in a large one, placed in the last region (largest accumulated size).
    let make = |spike_w: usize, spike_h: usize, amount: u32| {
        (
            flat_hist(move |w, y, _| {
                if w == spike_w && y == spike_h {
                    amount
                } else {
                    0
                }
            }),
            flat_hist(|_, _, _| 0),
        )
    };

    // Evenly divisible: 1024x1024 over 4x4 gives 256x256 regions with zero
    // remainder, so no accumulation happens and every region has the same
    // threshold. The same spike must give the same per-region verdict wherever
    // it sits.
    let mut verdicts_even = Vec::new();
    for (sw, sh) in [(0usize, 0usize), (3, 3), (1, 2)] {
        let (cur, prev) = make(sw, sh, 40_000);
        let mut st = pp::SceneDetectState {
            prev_picture_histogram: prev,
            ..Default::default()
        };
        let cur_i = [[100u64; 4]; 4];
        let fut_i = [[200u64; 4]; 4];
        let r = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
        verdicts_even.push((r, st.ahd_running_avg[sw][sh]));
    }
    assert!(
        verdicts_even.iter().all(|v| *v == verdicts_even[0]),
        "with no remainder the region position must not matter: {verdicts_even:?}"
    );

    // The running average update: when NO abrupt change is seen, the region's
    // average moves to (3*avg + ahd)/4. Starting from 0 with ahd 0, it stays 0.
    let (cur, prev) = make(0, 0, 0);
    let mut st = pp::SceneDetectState {
        prev_picture_histogram: prev,
        ..Default::default()
    };
    let flat_i = [[100u64; 4]; 4];
    let changed = pp::scene_transition_detector(&mut st, &cur, &flat_i, &flat_i, 1024, 1024, 4, 4);
    assert!(!changed, "identical histograms are not a scene change");
    assert_eq!(st.ahd_running_avg, [[0u32; 4]; 4]);
    assert!(!st.reset_running_avg, "no region was abrupt");

    // reset_running_avg latches when at least half the regions are abrupt, and
    // on the NEXT call it seeds the running average with the raw ahd instead
    // of blending. Drive two frames to reach it.
    let (cur, prev) = (flat_hist(|_, _, _| 5000), flat_hist(|_, _, _| 0));
    let mut st = pp::SceneDetectState {
        prev_picture_histogram: prev,
        ..Default::default()
    };
    let cur_i = [[10u64; 4]; 4];
    let fut_i = [[200u64; 4]; 4];
    let _ = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
    assert!(
        st.reset_running_avg,
        "every region difference is huge -> all abrupt"
    );
    // My first hand-derivation of the next two steps was WRONG and the test
    // caught it. On frame 1 `reset_running_avg` is still FALSE (C's ctor
    // zeroes it), so the seed does not happen; and the region IS abrupt, so
    // the blend in the else-branch does not happen either. The average is
    // therefore untouched at 0 after frame 1 — the latch only takes effect on
    // the NEXT call.
    assert_eq!(
        st.ahd_running_avg[0][0], 0,
        "frame 1 neither seeds nor blends"
    );
    let _ = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
    // Each region's ahd is 256 bins x |5000 - 0|.
    assert_eq!(
        st.ahd_running_avg[0][0],
        256 * 5000,
        "with reset latched the average is SEEDED with ahd, not blended"
    );
}

/// `perform_scene_change_detection` (`pd_process.c:4682-4700`) — which arm runs.
///
/// Measured settings, not guesses: `static_config.scene_change_detection` is
/// force-zeroed (`enc_settings.c:839-843`) so the first arm is dead in
/// mainline, while `vq_ctrls.sharpness_ctrls.scene_transition` is 1 in both
/// arms of `derive_vq_params` and zeroed only for LOW_DELAY
/// (`enc_handle.c:3282, 3291, 3324-3326`), so the SECOND arm is live in random
/// access.
#[test]
fn traced_perform_scene_change_detection_arms() {
    // Arm 1 (dead in mainline): the detector result becomes scene_change_flag
    // and cra_flag is forced true.
    let o = pp::perform_scene_change_detection(true, true, -1, false, || true);
    assert!(o.scene_change_flag && o.cra_flag && o.is_scene_change_detected);
    assert_eq!(
        o.transition_detected, -1,
        "arm 1 never touches transition_detected"
    );

    // Arm 2 (live in RA): scene_change_flag stays false and the result lands in
    // transition_detected instead.
    let o = pp::perform_scene_change_detection(false, true, -1, false, || true);
    assert!(!o.scene_change_flag && !o.cra_flag);
    assert_eq!(o.transition_detected, 1);

    let o = pp::perform_scene_change_detection(false, true, 0, false, || false);
    assert_eq!(o.transition_detected, 0);

    // Already latched at 1: the detector is NOT re-run and the value stands.
    let o = pp::perform_scene_change_detection(false, true, 1, false, || {
        panic!("the detector must not run while transition_detected is latched")
    });
    assert_eq!(o.transition_detected, 1);

    // Sharpness transition off (LOW_DELAY): neither arm runs.
    let o = pp::perform_scene_change_detection(false, false, -1, true, || {
        panic!("the detector must not run with both gates off")
    });
    assert!(!o.scene_change_flag);
    assert!(o.cra_flag, "an incoming cra_flag survives");
    assert_eq!(o.transition_detected, -1);
}

// ---------------------------------------------------------------------------
// Dynamic-GOP split decision — tier 4 (the HME half is tier 1, see
// c_parity_picstruct_dg.rs)
// ---------------------------------------------------------------------------

/// `early_hme`'s reduction (`pd_process.c:669-684`).
///
/// Derivation, and the trap: the block counts here use a HARDCODED 64
/// (`(aligned_width + 63) / 64`), while `dg_detector_hme_level0` divides by
/// `scs->b64_size`. The two agree at the default b64_size of 64 and would not
/// at 128 — reproduced rather than unified.
#[test]
fn traced_early_hme_reduce() {
    let m = pp::DgDetectorMetrics {
        tot_dist: 40_000,
        tot_cplx: 5,
        tot_active: 15,
        sum_in_vectors: -10,
        seg_completed: 1,
    };
    // 320x192 -> 5 x 3 = 20 blocks (192/64 = 3 exactly, 320/64 = 5).
    let r = pp::early_hme_reduce(&m, 320, 192);
    assert_eq!(r.norm_dist, 40_000 / 15);
    assert_eq!(u32::from(r.perc_cplx), (5u32 * 100) / 15);
    assert_eq!(u32::from(r.perc_active), (15u32 * 100) / 15);
    assert_eq!(i32::from(r.mv_in_out_count), (-10i32 * 100) / 15);

    // A partial block row/column rounds UP: 65x65 is 2x2 blocks, not 1x1.
    let r = pp::early_hme_reduce(&m, 65, 65);
    assert_eq!(r.norm_dist, 40_000 / 4);

    // sum_in_vectors is signed, and the percentage keeps its sign.
    let neg = pp::DgDetectorMetrics {
        sum_in_vectors: -400,
        ..m
    };
    assert!(pp::early_hme_reduce(&neg, 640, 640).mv_in_out_count < 0);
}

/// `calc_mini_gop_activity` (`pd_process.c:686-712`) — the split predicate.
///
/// Derivation of the three conditions:
/// * `cond1` requires the TOP layer to be ≥ 95 % active AND rules out the two
///   lopsided sub-layer combinations (one ≥ 95 while the other < 75). It gates
///   everything: without it neither `cond2` nor `cond3` can fire.
/// * `cond2` is the distortion test, and its `bias` is 25 when the previous
///   mini-GOP in this GOP was 5L and this is not the first mini-GOP, else 75 —
///   a hysteresis toward staying at 6L.
/// * `cond3` is the motion test: MIN of the two sub-layer counts > 40 AND MAX
///   > 55.
#[test]
fn traced_calc_mini_gop_activity_conditions() {
    // A baseline that satisfies cond1 and cond2 with bias 75.
    let base = |bias_prev_5l: bool| {
        pp::calc_mini_gop_activity(
            if bias_prev_5l { 2 } else { 0 },
            if bias_prev_5l { 5 } else { 4 },
            10_000, // top dist, > LOW_DIST_TH
            95,     // top active
            10,     // top cplx, > 0
            1_000,  // sub0 dist, < HIGH_DIST_TH
            95,     // sub0 active
            10,     // sub0 cplx, < 25
            1_000,  // sub1 dist
            95,     // sub1 active
            10,     // sub1 cplx
            0,      // top mv count (UNUSED in C)
            0,      // sub mv count 1
            0,      // sub mv count 2
        )
    };
    // bias 75: (1000 + 1000)/2 = 1000 < (75 * 10000)/100 = 7500 -> splits.
    assert!(base(false));
    // bias 25: 1000 < (25 * 10000)/100 = 2500 -> still splits.
    assert!(base(true));

    // Raise the sub-layer distortion so bias 75 passes and bias 25 does not.
    // The value must stay UNDER HIGH_DIST_TH (16*16*18 = 4608) or cond2 fails
    // for a different reason — my first attempt used 5000 and failed on
    // exactly that, which is why the number is spelled out here.
    // (4000 + 4000)/2 = 4000; 75% of 10000 is 7500, 25% is 2500.
    let with_dist = |bias_prev_5l: bool| {
        pp::calc_mini_gop_activity(
            if bias_prev_5l { 2 } else { 0 },
            if bias_prev_5l { 5 } else { 4 },
            10_000,
            95,
            10,
            4_000,
            95,
            10,
            4_000,
            95,
            10,
            0,
            0,
            0,
        )
    };
    assert!(with_dist(false), "bias 75 admits the split");
    assert!(
        !with_dist(true),
        "bias 25 is the hysteresis toward staying at 6L"
    );

    // cond1 gates everything: drop the top-layer activity below 95.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 94, 10, 1_000, 95, 10, 1_000, 95, 10, 0, 0, 0
    ));
    // The lopsided sub-layer cases: (>=95, <75) and (<75, >=95).
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 10, 1_000, 95, 10, 1_000, 74, 10, 0, 0, 0
    ));
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 10, 1_000, 74, 10, 1_000, 95, 10, 0, 0, 0
    ));

    // cond3 alone: cond2 fails (top cplx 0) but the motion test carries it.
    assert!(pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 56
    ));
    // MIN must exceed 40 AND MAX must exceed 55 — 41/55 fails on the MAX.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 55
    ));
    // 40/56 fails on the MIN.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 40, 56
    ));

    // The top-layer mv count is (void)-cast unused in C: changing it alone
    // must change nothing.
    let a =
        pp::calc_mini_gop_activity(0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 56);
    let b = pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 32_767, 41, 56,
    );
    assert_eq!(a, b, "top_layer_mv_in_out_count is unused");

    // HIGH_DIST_TH / LOW_DIST_TH are exclusive bounds.
    assert!(
        !pp::calc_mini_gop_activity(
            0,
            4,
            pp::LOW_DIST_TH,
            95,
            10,
            1_000,
            95,
            10,
            1_000,
            95,
            10,
            0,
            0,
            0
        ),
        "top_layer_dist must be strictly greater than LOW_DIST_TH"
    );
    assert!(
        !pp::calc_mini_gop_activity(
            0,
            4,
            10_000,
            95,
            10,
            pp::HIGH_DIST_TH,
            95,
            10,
            1_000,
            95,
            10,
            0,
            0,
            0
        ),
        "sub_layer_dist0 must be strictly less than HIGH_DIST_TH"
    );
}

/// `eval_sub_mini_gop` (`pd_process.c:713-758`) — the pass-to-layer mapping.
///
/// The three `early_hme` passes run in the order (end,start), (end,mid),
/// (mid,start), and are handed to `calc_mini_gop_activity` as
/// TOP=(end,start), SUB0=(mid,start), SUB1=(end,mid) — NOT in run order.
///
/// **A finding, and it explains a C oddity.** `calc_mini_gop_activity` is
/// FULLY SYMMETRIC in its two sub layers: the distortion enters as
/// `(d0 + d1) / 2`, the two activity guards are mirror images, the two
/// complexity guards are identical, and cond3 takes a MIN and a MAX. So
/// swapping SUB0 and SUB1 can never change the verdict — which is why C's call
/// site can pass `sub_layer_mv_in_out_count1` the (end,mid) value and
/// `..._count2` the (mid,start) value, inverted relative to the layer indices
/// they are named after, without anyone noticing. What DOES matter is the
/// TOP-vs-SUB assignment, and that is asserted below.
#[test]
fn traced_eval_sub_mini_gop_layer_mapping() {
    let r = |dist: u64, active: u8, cplx: u8, mv: i16| pp::EarlyHmeResult {
        mv_in_out_count: mv,
        norm_dist: dist,
        perc_cplx: cplx,
        perc_active: active,
    };
    let end_start = r(10_000, 95, 10, 10);
    let end_mid = r(4_000, 95, 10, 20);
    let mid_start = r(1_000, 95, 10, 30);

    // Sub-layer symmetry: swapping the two sub inputs is inert.
    assert_eq!(
        pp::eval_sub_mini_gop(0, 4, end_start, end_mid, mid_start),
        pp::eval_sub_mini_gop(0, 4, end_start, mid_start, end_mid),
        "calc_mini_gop_activity is symmetric in its two sub layers"
    );

    // TOP-vs-SUB is NOT symmetric. With (end,start) as the top layer the
    // distortion test passes: (1000 + 4000)/2 = 2500 < 75% of 10000.
    assert!(pp::eval_sub_mini_gop(0, 4, end_start, end_mid, mid_start));
    // Promote (mid,start) to the top layer instead and it fails: the top
    // distortion is now 1000, and (10000 + 4000)/2 = 7000 is not < 750 — and
    // 10000 also exceeds HIGH_DIST_TH as a sub layer.
    assert!(!pp::eval_sub_mini_gop(0, 4, mid_start, end_mid, end_start));
}

/// `commit_sub_mini_gop_split` (`pd_process.c:707-711`) — the array write.
#[test]
fn traced_commit_sub_mini_gop_split() {
    let mut map = pp::MiniGopMap::for_sequence(5);
    map.activity[pp::L6_INDEX] = false;
    map.activity[pp::L5_0_INDEX] = true;
    map.activity[pp::L5_1_INDEX] = true;
    pp::commit_sub_mini_gop_split(&mut map, true, pp::L6_INDEX, pp::L5_0_INDEX, pp::L5_1_INDEX);
    assert!(
        map.activity[pp::L6_INDEX],
        "the 6L shape becomes ACTIVE (to be split)"
    );
    assert!(!map.activity[pp::L5_0_INDEX] && !map.activity[pp::L5_1_INDEX]);

    // A false verdict writes NOTHING — C's whole block is inside the `if`.
    let mut map = pp::MiniGopMap::for_sequence(5);
    map.activity[pp::L6_INDEX] = false;
    map.activity[pp::L5_0_INDEX] = true;
    pp::commit_sub_mini_gop_split(
        &mut map,
        false,
        pp::L6_INDEX,
        pp::L5_0_INDEX,
        pp::L5_1_INDEX,
    );
    assert!(!map.activity[pp::L6_INDEX]);
    assert!(map.activity[pp::L5_0_INDEX]);
}

// ---------------------------------------------------------------------------
// Temporal-filter window — tier 4
// ---------------------------------------------------------------------------

/// `ref_pics_modulation` (`pd_process.c:3642-3745`).
///
/// Derivation of the three shapes:
/// * I slice: three Q16 log1p thresholds, 26572 / 45426 / 71998, yielding
///   6 / 4 / 2 / 0. LOWER noise buys MORE frames — the opposite of the
///   intuitive direction, and the C comment says why.
/// * base layer: five `modulate_pics` tables keyed on
///   `ratio = filt_to_unfilt_diff * 100 / noise`.
/// * non-base: three tables, each a single threshold yielding 0 or 1, and
///   there is NO `case 4` — `modulate_pics == 4` falls through to 0 there
///   while base layer gives 0/1/2.
#[test]
fn traced_ref_pics_modulation_three_shapes() {
    let c = |modulate: u8| pp::TfWindowCtrls {
        modulate_pics: modulate,
        ..Default::default()
    };

    // I slice: the three noise bands and the fall-through.
    for (noise, want) in [
        (0i32, 6),
        (26571, 6),
        (26572, 4),
        (45425, 4),
        (45426, 2),
        (71997, 2),
        (71998, 0),
    ] {
        assert_eq!(
            pp::ref_pics_modulation(true, 0, &c(1), noise, 0, 1, 1),
            want,
            "I slice at noise {noise}"
        );
    }
    // The I-slice arm ignores modulate_pics entirely.
    for m in 0u8..=4 {
        assert_eq!(pp::ref_pics_modulation(true, 0, &c(m), 0, 0, 1, 1), 6);
    }

    // Base layer. ratio = diff * 100 / noise; pick noise = 100 so ratio == diff.
    let base = |m: u8, ratio: u32| pp::ref_pics_modulation(false, 0, &c(m), 100, ratio, 1, 1);
    assert_eq!(base(0, 999), 0, "modulate_pics 0 is always zero");
    assert_eq!((base(1, 99), base(1, 100)), (5, pp::TF_MAX_EXTENSION));
    assert_eq!(
        (base(2, 49), base(2, 99), base(2, 100)),
        (3, 5, pp::TF_MAX_EXTENSION)
    );
    assert_eq!((base(3, 49), base(3, 99), base(3, 100)), (3, 4, 5));
    assert_eq!((base(4, 49), base(4, 99), base(4, 100)), (0, 1, 2));
    assert_eq!(base(5, 100), 0, "an unknown level falls through to zero");

    // Non-base: single thresholds, and NO case 4.
    let nb = |m: u8, ratio: u32| pp::ref_pics_modulation(false, 1, &c(m), 100, ratio, 1, 1);
    assert_eq!((nb(1, 24), nb(1, 25)), (0, 1));
    assert_eq!((nb(2, 49), nb(2, 50)), (0, 1));
    assert_eq!((nb(3, 74), nb(3, 75)), (0, 1));
    assert_eq!(nb(4, 999), 0, "non-base has no case 4 -- it falls through");
    assert_eq!(nb(0, 999), 0);

    // Zero noise short-circuits the ratio to 0 rather than dividing.
    assert_eq!(pp::ref_pics_modulation(false, 0, &c(1), 0, 9_999, 1, 1), 5);

    // qp_opt applies DIVIDE_AND_ROUND(offset * q_weight, q_weight_denom).
    let qp = pp::TfWindowCtrls {
        modulate_pics: 1,
        qp_opt: true,
        ..Default::default()
    };
    // offset 5, weight 3/4 -> (15 + 2) / 4 = 4 (rounds, not truncates).
    assert_eq!(pp::ref_pics_modulation(false, 0, &qp, 100, 99, 3, 4), 4);
    // weight 1/1 is the identity.
    assert_eq!(pp::ref_pics_modulation(false, 0, &qp, 100, 99, 1, 1), 5);
}

/// `derive_tf_window_params`' count derivation, all four arms.
///
/// The arms differ in ways that blur together on a quick read; each assertion
/// below names the difference it pins.
#[test]
fn traced_derive_tf_window_counts_per_arm() {
    let ctrls = pp::TfWindowCtrls {
        enabled: true,
        modulate_pics: 1,
        num_past_pics: 2,
        num_future_pics: 2,
        max_num_past_pics: 8,
        max_num_future_pics: 8,
        ..Default::default()
    };

    // LOW DELAY: offset applies (modulate_pics != 0), no per-struct cap.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &ctrls, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (5, 5));
    // With modulate_pics 0 the offset is dropped even if the caller passes one.
    let no_mod = pp::TfWindowCtrls {
        modulate_pics: 0,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &no_mod, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (2, 2));
    // max_num_* caps.
    let capped = pp::TfWindowCtrls {
        max_num_past_pics: 3,
        max_num_future_pics: 3,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &capped, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (3, 3));

    // DELAYED INTRA: NO past pictures, and the future cap is the I_SLICE row
    // of tf_max_ref_per_struct, i.e. 1 << hierarchical_levels.
    for (hier, want) in [(0u32, 1i32), (1, 2), (2, 4), (3, 5)] {
        let c = pp::derive_tf_window_counts(pp::TfWindowArm::DelayedIntra, &ctrls, 3, hier, 0);
        assert_eq!(c.num_past_pics, 0, "delayed intra never filters the past");
        assert_eq!(c.num_future_pics, want, "hier {hier}");
    }
    // RANDOM-ACCESS IDR takes the same shape.
    let a = pp::derive_tf_window_counts(pp::TfWindowArm::DelayedIntra, &ctrls, 3, 2, 0);
    let b = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessIdr, &ctrls, 3, 2, 0);
    assert_eq!(a, b);

    // RANDOM-ACCESS INTER: the MAX(1, ...) floor exists only here, so a
    // negative modulation cannot empty the window.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &ctrls, -10, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (1, 1));
    // The low-delay arm has NO such floor: the same negative offset drives it
    // below zero.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &ctrls, -10, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (-8, -8));

    // The inter arm's per-struct cap keys off the temporal layer: BASE takes
    // row 1 (7 each side), non-base takes row 2 (1 below 6L, 2 at 6L).
    let wide = pp::TfWindowCtrls {
        num_past_pics: 9,
        num_future_pics: 9,
        max_num_past_pics: 30,
        max_num_future_pics: 30,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 4, 0);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_BASE_REF_PICS),
            i32::from(pp::TF_MAX_BASE_REF_PICS)
        )
    );
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 4, 1);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_L1_REF_PICS_SUB_6L),
            i32::from(pp::TF_MAX_L1_REF_PICS_SUB_6L)
        )
    );
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 5, 1);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_L1_REF_PICS_6L),
            i32::from(pp::TF_MAX_L1_REF_PICS_6L)
        )
    );
}

/// The past-window compaction (`pd_process.c:3914-3920`, `:4091-4098`).
///
/// **This block is DEAD CODE in C** and the port keeps it anyway
/// (`docs/WORKING-ON-THIS.md` §7; written up as
/// `docs/SUSPECTED-C-BUGS.md` #18). `actual_past_pics` is initialised to
/// `num_past_pics` and never modified — only `actual_future_pics` is
/// incremented — so the guard `actual_past_pics != num_past_pics` is always
/// false. Verified by `grep -n actual_past_pics Codec/pd_process.c`: two
/// initialisations, two `past_altref_nframes` assignments, two comparisons, no
/// decrement.
///
/// The first assertion below is the one that found it. My hand-derivation
/// expected a front-holed list (past slots 2 and 3 filled, 0 and 1 NULL — the
/// shape the block was written for) to compact to the front. It does not:
/// C's loop is `while (list[pic_i] != NULL)` starting at index 0, so on
/// exactly that shape it stops immediately. Fixing the counter alone would
/// not fix the block.
#[test]
fn traced_compact_tf_past_window() {
    // The shape the block was WRITTEN for: holes at the front. C's loop bound
    // makes it a no-op, and so does the port's.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[2] = Some(20);
    list[3] = Some(30);
    list[4] = Some(40); // centre
    list[5] = Some(50);
    list[6] = Some(60);
    let before = list;
    pp::compact_tf_past_window(&mut list, 4, 2);
    assert_eq!(list, before, "the `while != NULL` bound stops at index 0");

    // With no leading hole the shift does happen, which is what the block
    // would do if the counter ever differed.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[0] = Some(10);
    list[1] = Some(20);
    list[2] = Some(30);
    list[3] = Some(40);
    list[4] = Some(50);
    pp::compact_tf_past_window(&mut list, 4, 2);
    assert_eq!(&list[..3], &[Some(30), Some(40), Some(50)]);

    // No shortfall: untouched, and the guard is not even entered.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[0] = Some(1);
    list[1] = Some(2);
    let before = list;
    pp::compact_tf_past_window(&mut list, 1, 1);
    assert_eq!(list, before);
}

/// The `tf_avg_luma` / `tf_avg_ahd_error` reduction
/// (`pd_process.c:4101-4118`) — the CENTRE picture is excluded.
#[test]
fn traced_tf_window_averages_excludes_the_centre() {
    // past 2, centre at index 2, future 1: indices 0, 1, 3 contribute.
    let luma = [100u64, 200, 999_999, 300, 0];
    let err = [10i32, 20, 999_999, 30, 0];
    let (avg_luma, avg_err) = pp::tf_window_averages(&luma, &err, 2, 1);
    assert_eq!(avg_luma, (100 + 200 + 300) / 3);
    assert_eq!(avg_err, (10 + 20 + 30) / 3);

    // An empty window returns zeros without dividing.
    assert_eq!(pp::tf_window_averages(&luma, &err, 0, 0), (0, 0));

    // Centre at index 0 (no past pictures) excludes index 0.
    let (avg_luma, _) = pp::tf_window_averages(&[999_999, 10, 20], &[0, 1, 2], 0, 2);
    assert_eq!(avg_luma, (10 + 20) / 2);
}

/// `low_delay_store_tf_pictures`' store predicate (`pd_process.c:4132`).
///
/// Derivation at hierarchical_levels 3 (mg_size 8) with one past picture:
/// the predicate is `pic_idx_in_mg + 1 + 1 >= 8`, so only index 6 and 7
/// qualify — the last two non-base pictures of the mini-GOP.
#[test]
fn traced_low_delay_store_tf_picture_predicate() {
    for idx in 0u32..8 {
        let want = idx >= 6;
        assert_eq!(
            pp::low_delay_should_store_tf_picture(1, idx, 1, 3),
            want,
            "pic_idx_in_mg {idx}"
        );
    }
    // A BASE picture never joins the ring, however late it sits.
    assert!(!pp::low_delay_should_store_tf_picture(0, 7, 1, 3));
    // More past pictures widen the window backwards.
    assert!(pp::low_delay_should_store_tf_picture(1, 4, 3, 3));
    assert!(!pp::low_delay_should_store_tf_picture(1, 3, 3, 3));
}

/// `mctf_frame`'s decision half (`pd_process.c:4194-4250`).
///
/// The trap this pins: the STORE and RELEASE gates are NOT symmetric — RELEASE
/// additionally requires `temporal_layer_index == 0`, because the ring is
/// filled by non-base pictures and drained by the base picture that consumed
/// it.
#[test]
fn traced_mctf_frame_decision_store_and_release_are_asymmetric() {
    let ld = pp::PredStructure::LowDelay;
    let ra = pp::PredStructure::RandomAccess;

    // Low delay, base TF enabled, at a NON-base picture: store but do not
    // release.
    let d = pp::mctf_frame_decision(ld, true, true, 1, 0);
    assert!(d.store_ld_tf_pictures && !d.release_ld_tf_pictures);

    // The same at the BASE picture: both.
    let d = pp::mctf_frame_decision(ld, true, true, 0, 0);
    assert!(d.store_ld_tf_pictures && d.release_ld_tf_pictures);

    // Random access: neither, whatever the layer.
    for tl in [0u8, 1] {
        let d = pp::mctf_frame_decision(ra, true, true, tl, 0);
        assert!(!d.store_ld_tf_pictures && !d.release_ld_tf_pictures);
    }

    // tf_ctrls disabled clears do_tf and skips the filter, but the ring
    // operations are gated on the SEQUENCE params, not on tf_ctrls -- so they
    // still run.
    let d = pp::mctf_frame_decision(ld, true, false, 0, 0);
    assert!(!d.run_tf && d.do_tf_cleared);
    assert!(d.store_ld_tf_pictures && d.release_ld_tf_pictures);

    // is_noise_level is a plain threshold on the LAST I picture's noise.
    assert!(!pp::mctf_frame_decision(ra, false, true, 0, pp::VQ_NOISE_LVL_TH - 1).is_noise_level);
    assert!(pp::mctf_frame_decision(ra, false, true, 0, pp::VQ_NOISE_LVL_TH).is_noise_level);
}

/// `mctf_frame`'s motion-direction verdict (`pd_process.c:4232-4238`).
///
/// The margin is `other * 6 / 4` with INTEGER division on the right, so at
/// `vert == 1` the threshold is 1 (not 1.5) and `horz == 2` already wins.
#[test]
fn traced_tf_motion_direction() {
    assert_eq!(pp::tf_motion_direction(0, 0), -1);
    assert_eq!(pp::tf_motion_direction(10, 10), -1);
    // 1.5x margin at larger counts.
    assert_eq!(pp::tf_motion_direction(151, 100), 0);
    assert_eq!(pp::tf_motion_direction(150, 100), -1);
    assert_eq!(pp::tf_motion_direction(100, 151), 1);
    // The truncation at small counts: vert 1 -> threshold 6/4 = 1.
    assert_eq!(pp::tf_motion_direction(2, 1), 0);
    assert_eq!(pp::tf_motion_direction(1, 1), -1);
    // Zero on one side always picks the other, if it is non-zero.
    assert_eq!(pp::tf_motion_direction(1, 0), 0);
    assert_eq!(pp::tf_motion_direction(0, 1), 1);
}

/// `svt_aom_tf_max_ref_per_struct` — the port copy, gated at tier 1 in
/// `c_parity_picstruct.rs`. Repeated here only for the two shape facts a
/// reader needs while reading the window derivation above.
#[test]
fn traced_tf_max_ref_per_struct_shape() {
    // Only the I_SLICE row grows with the hierarchy.
    assert_eq!(pp::tf_max_ref_per_struct(3, 0, false), 8);
    assert_eq!(
        pp::tf_max_ref_per_struct(3, 1, false),
        pp::TF_MAX_BASE_REF_PICS
    );
    assert_eq!(
        pp::tf_max_ref_per_struct(4, 2, false),
        pp::TF_MAX_L1_REF_PICS_SUB_6L
    );
    assert_eq!(
        pp::tf_max_ref_per_struct(5, 2, false),
        pp::TF_MAX_L1_REF_PICS_6L
    );
    // `direction` is (void)-cast in C.
    for ty in 0u8..=2 {
        assert_eq!(
            pp::tf_max_ref_per_struct(4, ty, false),
            pp::tf_max_ref_per_struct(4, ty, true)
        );
    }
}
