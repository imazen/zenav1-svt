use super::*;

pub(super) fn enc_ctx(count: u32, intra: u32, idr: u32) -> pp::EncCtxPicParams {
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
pub(super) fn traced_mini_gop_window_split_exact_8() {
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
pub(super) fn traced_mini_gop_window_split_short_tail_5() {
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
pub(super) fn traced_mini_gop_idr_guard_splits_the_buffer() {
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
pub(super) fn traced_set_mini_gop_structure_low_delay_degenerates() {
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
pub(super) fn traced_set_mini_gop_structure_random_access_reports_dg() {
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
pub(super) fn traced_get_pred_struct_for_frame_idr_takes_sequence_hierarchy() {
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
pub(super) fn traced_store_mg_picture_arrays_sorts_by_decode_order() {
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
pub(super) fn traced_get_pic_idx_in_mg_low_delay_and_random_access() {
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
pub(super) fn traced_update_pred_struct_and_pic_type_position_chain() {
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
pub(super) fn traced_perform_sc_detection_inheritance() {
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
pub(super) fn traced_avail_past_pictures() {
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
pub(super) fn traced_get_tpl_params_level() {
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
pub(super) fn traced_set_tpl_params_mutates_and_keys_off_resolution() {
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
pub(super) fn traced_validate_pic_for_tpl_reduced_group_zero_is_base_only() {
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
pub(super) fn traced_store_extended_group_intra_arms_are_asymmetric() {
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
