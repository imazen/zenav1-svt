//! Hand-derived vectors for the S-frame family (`Codec/pd_process.c`) —
//! evidence **tier 4** (`docs/WORKING-ON-THIS.md` §4), the weakest tier, used
//! because every one of these functions is `static` with no exported symbol.
//!
//! Verified with `nm -g Bin/Release/libSvtAv1Enc.a`: `get_dist_to_s`,
//! `get_sframe_qp`, `get_sframe_qp_offset`, `setup_sframe_qp`,
//! `sframe_position_offset`, `set_sframe_type`, `decide_sframe_mg`,
//! `set_sframe_rps`, `prune_sframe_refs` and `update_sframe_ref_order_hint`
//! are all absent from the global symbol table.
//!
//! Each expectation is derived by reading the C, and the derivation is written
//! above it — that is the only defence a tier-4 vector has against being a
//! second transcription of the same mistake.

use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_sframe as sf;

fn cfg(mode: sf::SFrameMode, dist: i32, pred: pp::PredStructure) -> sf::SFrameConfig<'static> {
    sf::SFrameConfig {
        mode,
        dist,
        qp: 0,
        qp_offset: 0,
        positions: sf::SFramePositions::default(),
        hierarchical_levels: 4,
        pred_structure: pred,
        intra_period_length: -1,
        min_qp_allowed: 1,
        max_qp_allowed: 63,
        mfmv_enabled: true,
        order_hint_bits: 7,
    }
}

fn inter(poc: u64, tl: u8) -> pp::PicParams {
    pp::PicParams {
        picture_number: poc,
        decode_order: poc,
        slice_type: pp::SliceType::B,
        temporal_layer_index: tl,
        hierarchical_levels: 4,
        pred_struct_type: pp::PredStructure::RandomAccess,
        ..Default::default()
    }
}

/// `get_dist_to_s` (`pd_process.c:1494`).
///
/// Derivation. The loop stops at the FIRST scheduled position `>=
/// picture_num` and returns that gap. `dist_to_next_s` is written only on the
/// `==` branch, so a picture that merely precedes an S-frame leaves it at -1
/// even though a following S-frame exists — that asymmetry is what makes
/// `set_sframe_type`'s `dist_to_s = dist_to_next_s` handoff fire on the
/// S-frame itself and nowhere else. With positions {16, 48, 80}:
/// picture 0 -> (16, -1), picture 16 -> (0, 32), picture 17 -> (31, -1),
/// picture 80 -> (0, -1) because 80 is the last, picture 81 -> (-1, -1).
#[test]
fn traced_dist_to_s() {
    let positions = [16u64, 48, 80];
    let posi = sf::SFramePositions {
        positions: Some(&positions),
        ..Default::default()
    };
    assert_eq!(sf::dist_to_s(&posi, 0), (16, sf::NO_SFRAME));
    assert_eq!(sf::dist_to_s(&posi, 16), (0, 32));
    assert_eq!(sf::dist_to_s(&posi, 17), (31, sf::NO_SFRAME));
    assert_eq!(sf::dist_to_s(&posi, 48), (0, 32));
    assert_eq!(sf::dist_to_s(&posi, 80), (0, sf::NO_SFRAME));
    assert_eq!(sf::dist_to_s(&posi, 81), (sf::NO_SFRAME, sf::NO_SFRAME));

    // No list at all: C dereferences nothing and the loop never runs.
    assert_eq!(
        sf::dist_to_s(&sf::SFramePositions::default(), 5),
        (sf::NO_SFRAME, sf::NO_SFRAME)
    );
}

/// `get_sframe_qp` / `get_sframe_qp_offset` (`pd_process.c:1509`, `:1525`).
///
/// Derivation, and the surprise worth stating: with a QP array but NO position
/// list C returns `qps[0]` for EVERY picture — that is how a single
/// `--sframe-qp` applies everywhere. With a position list it returns a value
/// only on an exact match, and 0 otherwise, so 0 doubles as "not scheduled"
/// and as "scheduled with no QP".
#[test]
fn traced_sframe_qp_lookup() {
    let positions = [16u64, 48];
    let qps = [30u8, 40];
    let offsets = [-3i8, 5];

    let no_list = sf::SFramePositions {
        positions: None,
        qps: Some(&qps),
        qp_offsets: Some(&offsets),
    };
    for poc in [0u64, 16, 99] {
        assert_eq!(sf::sframe_qp(&no_list, poc), 30, "poc {poc}");
        assert_eq!(sf::sframe_qp_offset(&no_list, poc), -3, "poc {poc}");
    }

    let with_list = sf::SFramePositions {
        positions: Some(&positions),
        qps: Some(&qps),
        qp_offsets: Some(&offsets),
    };
    assert_eq!(sf::sframe_qp(&with_list, 16), 30);
    assert_eq!(sf::sframe_qp(&with_list, 48), 40);
    assert_eq!(sf::sframe_qp(&with_list, 17), 0);
    assert_eq!(sf::sframe_qp_offset(&with_list, 48), 5);
    assert_eq!(sf::sframe_qp_offset(&with_list, 17), 0);

    // No arrays: 0 regardless.
    assert_eq!(sf::sframe_qp(&sf::SFramePositions::default(), 16), 0);
    assert_eq!(sf::sframe_qp_offset(&sf::SFramePositions::default(), 16), 0);
}

/// `setup_sframe_qp` (`pd_process.c:1541`) — the two traps.
///
/// Derivation 1: the schedule is looked up by DECODE order under
/// `SFRAME_DEC_POSI_BASE` and by DISPLAY order otherwise. A picture with
/// `picture_number = 20` and `decode_order = 16` therefore matches a schedule
/// entry at 16 only in that mode.
///
/// Derivation 2: C clips through `int8_t` —
/// `CLIP3((int8_t)min, (int8_t)max, (int8_t)sframe_qp)`. A configured QP of
/// 200 becomes -56 as an `int8_t`, which the clip raises to `min_qp_allowed`,
/// NOT to `max_qp_allowed`. That is the opposite of what an unsigned reading
/// predicts, so it is pinned here.
#[test]
fn traced_setup_sframe_qp_decode_order_and_int8_clip() {
    let positions = [16u64];
    let qps = [30u8];
    let posi = sf::SFramePositions {
        positions: Some(&positions),
        qps: Some(&qps),
        qp_offsets: None,
    };

    let mut c = cfg(
        sf::SFrameMode::DecPosiBase,
        0,
        pp::PredStructure::RandomAccess,
    );
    c.positions = posi;
    let mut pic = inter(20, 0);
    pic.decode_order = 16;
    sf::setup_sframe_qp(&mut pic, &c);
    assert_eq!((pic.picture_qp, pic.qp_on_the_fly), (30, true));

    // The same picture in a display-order mode finds nothing.
    let mut c2 = cfg(
        sf::SFrameMode::FlexibleBase,
        0,
        pp::PredStructure::RandomAccess,
    );
    c2.positions = posi;
    let mut pic = inter(20, 0);
    pic.decode_order = 16;
    sf::setup_sframe_qp(&mut pic, &c2);
    assert_eq!((pic.picture_qp, pic.qp_on_the_fly), (0, false));

    // The int8 clip.
    let mut c3 = cfg(
        sf::SFrameMode::StrictBase,
        0,
        pp::PredStructure::RandomAccess,
    );
    c3.qp = 200;
    let mut pic = inter(20, 0);
    sf::setup_sframe_qp(&mut pic, &c3);
    assert_eq!(
        pic.picture_qp, 1,
        "200 as int8 is -56, which clips UP to min_qp_allowed"
    );
}

/// `sframe_position_offset` (`pd_process.c:1563`).
///
/// Derivation: 1 only for `SFRAME_DEC_POSI_BASE` in RANDOM_ACCESS. Low delay
/// decodes in display order, so no adjustment is needed there even in that
/// mode.
#[test]
fn traced_position_offset() {
    for (mode, pred, want) in [
        (
            sf::SFrameMode::DecPosiBase,
            pp::PredStructure::RandomAccess,
            1,
        ),
        (sf::SFrameMode::DecPosiBase, pp::PredStructure::LowDelay, 0),
        (
            sf::SFrameMode::FlexibleBase,
            pp::PredStructure::RandomAccess,
            0,
        ),
        (
            sf::SFrameMode::StrictBase,
            pp::PredStructure::RandomAccess,
            0,
        ),
        (
            sf::SFrameMode::NearestBase,
            pp::PredStructure::RandomAccess,
            0,
        ),
    ] {
        assert_eq!(
            sf::position_offset(&cfg(mode, 32, pred)),
            want,
            "{mode:?} {pred:?}"
        );
    }
}

/// `set_sframe_type`, `SFRAME_STRICT_BASE` (`pd_process.c:1578-1582`).
///
/// Derivation: a base-layer inter frame exactly `sframe_dist` from the last
/// key frame. With `key_poc = 0` and `dist = 32`, POC 32 and 64 qualify and
/// POC 33 does not; a non-base frame at POC 32 never does.
#[test]
fn traced_strict_base() {
    let c = cfg(
        sf::SFrameMode::StrictBase,
        32,
        pp::PredStructure::RandomAccess,
    );
    for (poc, tl, want) in [
        (32u64, 0u8, true),
        (64, 0, true),
        (33, 0, false),
        (32, 2, false),
    ] {
        let mut ctx = pp::PicDecisionCtx::new();
        let mut pic = inter(poc, tl);
        sf::set_sframe_type(&mut pic, &c, &mut ctx);
        assert_eq!(pic.is_switch_frame, want, "poc {poc} tl {tl}");
    }
}

/// `set_sframe_type`, `SFRAME_NEAREST_BASE` — its two arms differ
/// (`pd_process.c:1583-1598`).
///
/// Derivation, random access: pictures arrive in DECODE order, so a base-layer
/// frame whose `frames_since_key % dist` is anywhere inside the mini-GOP is
/// the next S-frame. With `dist = 32` and `mg_size = 16`, POC 32 (remainder 0)
/// and POC 40 (remainder 8) both qualify; POC 48 (remainder 16) does not,
/// because 16 is not `< 16`.
///
/// Derivation, low delay: the schedule sets a STICKY `sframe_due` flag at the
/// exact multiple, and the next base-layer frame consumes it. So a non-base
/// frame at the multiple arms the flag without switching, and the following
/// base-layer frame switches even though its own remainder is not 0.
#[test]
fn traced_nearest_base_two_arms() {
    let ra = cfg(
        sf::SFrameMode::NearestBase,
        32,
        pp::PredStructure::RandomAccess,
    );
    for (poc, want) in [(32u64, true), (40, true), (48, false)] {
        let mut ctx = pp::PicDecisionCtx::new();
        ctx.mg_size = 16;
        let mut pic = inter(poc, 0);
        sf::set_sframe_type(&mut pic, &ra, &mut ctx);
        assert_eq!(pic.is_switch_frame, want, "RA poc {poc}");
    }

    let ld = cfg(sf::SFrameMode::NearestBase, 32, pp::PredStructure::LowDelay);
    let mut ctx = pp::PicDecisionCtx::new();
    let mut non_base = inter(32, 1);
    sf::set_sframe_type(&mut non_base, &ld, &mut ctx);
    assert!(
        !non_base.is_switch_frame,
        "a non-base frame only arms the flag"
    );
    assert!(ctx.sframe_due);

    let mut later = inter(35, 0);
    sf::set_sframe_type(&mut later, &ld, &mut ctx);
    assert!(
        later.is_switch_frame,
        "the next base frame consumes the flag"
    );
    assert!(!ctx.sframe_due);
}

/// `set_sframe_type`, `SFRAME_DEC_POSI_BASE` — the deferral
/// (`pd_process.c:1604-1610`, `:1622-1626`).
///
/// Derivation: `sframe_position_offset` is 1 in random access, so the schedule
/// is probed at `picture_number + 1`. When that hits, the frame does NOT
/// become the S-frame; `ctx.next_arf_is_s` is armed and the NEXT base-layer
/// frame takes it. That deferral is the whole point of the decode-order mode,
/// and it is invisible in any single-frame test.
///
/// With `dist = 32` and `key_poc = 0`: POC 31 probes `(31 + 1) % 32 == 0`, so
/// it arms; POC 32 then switches. The arming frame also resets
/// `sframe_hier_lvls` to the full configured depth.
#[test]
fn traced_dec_posi_defers_to_the_next_base_frame() {
    let c = cfg(
        sf::SFrameMode::DecPosiBase,
        32,
        pp::PredStructure::RandomAccess,
    );
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.sframe_hier_lvls = 1;

    let mut arming = inter(31, 0);
    sf::set_sframe_type(&mut arming, &c, &mut ctx);
    assert!(
        !arming.is_switch_frame,
        "the arming frame is NOT the S-frame"
    );
    assert!(ctx.next_arf_is_s);
    assert_eq!(
        ctx.sframe_hier_lvls, 4,
        "the pyramid restarts at full depth"
    );

    let mut switching = inter(32, 0);
    sf::set_sframe_type(&mut switching, &c, &mut ctx);
    assert!(switching.is_switch_frame);
    assert!(!ctx.next_arf_is_s);

    // In the same mode under LOW_DELAY the offset is 0, so there is no
    // deferral at all and POC 32 switches directly.
    let ld = cfg(sf::SFrameMode::DecPosiBase, 32, pp::PredStructure::LowDelay);
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.sframe_hier_lvls = 4;
    let mut pic = inter(32, 0);
    sf::set_sframe_type(&mut pic, &ld, &mut ctx);
    assert!(pic.is_switch_frame);
    assert!(!ctx.next_arf_is_s);
}

/// The mini-GOP downgrade (`pd_process.c:1630-1638` and its two twins).
///
/// Derivation: the loop takes the SMALLEST `lvl` with `dist < 2^(lvl+1)`, so a
/// distance of 1 gives level 0, 2 and 3 give level 1, 4..7 give level 2, and a
/// distance at or above `2^hier_lvls` leaves the level alone because the loop
/// runs out.
///
/// Driven here through `set_sframe_type` with an explicit schedule: at
/// `picture_number = 10` with an S-frame at 13, `dist_to_s` is 3 and
/// `next_mg_size` is 16, so 3 < 16 arms the downgrade and level 1 results.
#[test]
fn traced_mini_gop_downgrade_from_a_schedule() {
    for (sframe_at, want_lvl) in [(11u64, 0i32), (12, 1), (13, 1), (14, 2), (17, 2), (25, 3)] {
        let positions = [sframe_at];
        let mut c = cfg(
            sf::SFrameMode::FlexibleBase,
            0,
            pp::PredStructure::RandomAccess,
        );
        c.positions = sf::SFramePositions {
            positions: Some(&positions),
            ..Default::default()
        };
        let mut ctx = pp::PicDecisionCtx::new();
        ctx.sframe_hier_lvls = 4;
        let mut pic = inter(10, 0);
        sf::set_sframe_type(&mut pic, &c, &mut ctx);
        assert!(!pic.is_switch_frame, "picture 10 is not itself scheduled");
        assert_eq!(
            ctx.sframe_hier_lvls,
            want_lvl,
            "S-frame at {sframe_at}: distance {} downgrades to level {want_lvl}",
            sframe_at - 10
        );
    }

    // Control: a distance of EXACTLY the mini-GOP size does not downgrade —
    // the guard is `dist_to_s < next_mg_size`, and 16 < 16 is false. So is a
    // distance beyond it.
    for sframe_at in [26u64, 42] {
        let positions = [sframe_at];
        let mut c = cfg(
            sf::SFrameMode::FlexibleBase,
            0,
            pp::PredStructure::RandomAccess,
        );
        c.positions = sf::SFramePositions {
            positions: Some(&positions),
            ..Default::default()
        };
        let mut ctx = pp::PicDecisionCtx::new();
        ctx.sframe_hier_lvls = 4;
        let mut pic = inter(10, 0);
        sf::set_sframe_type(&mut pic, &c, &mut ctx);
        assert_eq!(ctx.sframe_hier_lvls, 4, "S-frame at {sframe_at}");
    }

    let positions = [42u64];
    let mut c = cfg(
        sf::SFrameMode::FlexibleBase,
        0,
        pp::PredStructure::RandomAccess,
    );
    c.positions = sf::SFramePositions {
        positions: Some(&positions),
        ..Default::default()
    };
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.sframe_hier_lvls = 4;
    let mut pic = inter(10, 0);
    sf::set_sframe_type(&mut pic, &c, &mut ctx);
    assert_eq!(ctx.sframe_hier_lvls, 4);
}

/// `decide_sframe_mg` (`pd_process.c:1689`).
///
/// Derivation: a key frame always restarts at full depth and clears the
/// deferral, and then the first mini-GOP is shrunk if an S-frame lands inside
/// it. With `hierarchical_levels = 4` the full mini-GOP is 16; a schedule
/// entry 5 pictures away gives `5 < 16`, and the smallest `lvl` with
/// `5 < 2^(lvl+1)` is 2.
///
/// The early return matters too: with a schedule whose entries are all in the
/// past, C returns BEFORE the downgrade, leaving the full depth.
#[test]
fn traced_decide_sframe_mg() {
    let positions = [5u64];
    let mut c = cfg(
        sf::SFrameMode::FlexibleBase,
        0,
        pp::PredStructure::RandomAccess,
    );
    c.positions = sf::SFramePositions {
        positions: Some(&positions),
        ..Default::default()
    };
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.sframe_hier_lvls = 0;
    ctx.next_arf_is_s = true;
    let pic = pp::PicParams {
        picture_number: 0,
        slice_type: pp::SliceType::I,
        is_key_frame: true,
        ..Default::default()
    };
    sf::decide_sframe_mg(&pic, &c, &mut ctx);
    assert!(!ctx.next_arf_is_s, "a key frame clears the deferral");
    assert_eq!(ctx.sframe_hier_lvls, 2);

    // All entries expired: the early return leaves the restored full depth.
    let expired = [3u64];
    c.positions = sf::SFramePositions {
        positions: Some(&expired),
        ..Default::default()
    };
    let mut ctx = pp::PicDecisionCtx::new();
    let pic = pp::PicParams {
        picture_number: 10,
        slice_type: pp::SliceType::I,
        is_key_frame: true,
        ..Default::default()
    };
    sf::decide_sframe_mg(&pic, &c, &mut ctx);
    assert_eq!(ctx.sframe_hier_lvls, 4);

    // With no schedule, `sframe_dist` itself sizes the first mini-GOP.
    let mut c2 = cfg(
        sf::SFrameMode::FlexibleBase,
        3,
        pp::PredStructure::RandomAccess,
    );
    c2.hierarchical_levels = 4;
    let mut ctx = pp::PicDecisionCtx::new();
    sf::decide_sframe_mg(&pic, &c2, &mut ctx);
    assert_eq!(ctx.sframe_hier_lvls, 1, "3 < 2^2, so level 1");
}

/// `set_sframe_rps` (`pd_process.c:1726`) — what makes the frame a tune-in
/// point, and the reason its ORDER against the ref-management dispatcher
/// matters.
#[test]
fn traced_set_sframe_rps() {
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.lay0_toggle = 2;
    ctx.lay1_toggle = 1;
    let mut enc = pp::EncCtxPicParams {
        elapsed_non_cra_count: 9,
        ..Default::default()
    };
    let mut pic = inter(64, 0);
    pic.rps.refresh_frame_mask = 0x08;

    sf::set_sframe_rps(&mut pic, &mut ctx, &mut enc);

    assert!(pic.error_resilient_mode);
    assert_eq!(pic.rps.refresh_frame_mask, 0xFF);
    assert_eq!((ctx.lay0_toggle, ctx.lay1_toggle), (0, 0));
    assert_eq!(ctx.sframe_poc, 64);
    assert_eq!(enc.elapsed_non_cra_count, 0);
}

/// `update_sframe_ref_order_hint` (`pd_process.c:4521`) — the low-delay
/// relative-hint trap.
///
/// Derivation: low delay publishes `ref_order_hint[i] - key_poc`, random
/// access publishes the raw value. With slot hints {100, 101, ...} and
/// `key_poc = 96`, low delay yields {4, 5, ...} and random access {100, 101,
/// ...}. Then every slot the refresh mask touches takes this picture's own
/// hint, `picture_number mod 2^order_hint_bits`.
#[test]
fn traced_update_sframe_ref_order_hint() {
    let hints = [100u32, 101, 102, 103, 104, 105, 106, 107];

    let ld = cfg(sf::SFrameMode::StrictBase, 32, pp::PredStructure::LowDelay);
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.ref_order_hint = hints;
    ctx.key_poc = 96;
    let mut pic = inter(200, 0);
    pic.rps.refresh_frame_mask = 0;
    sf::update_sframe_ref_order_hint(&mut pic, &ld, &mut ctx);
    assert_eq!(pic.dpb_order_hint, [4, 5, 6, 7, 8, 9, 10, 11]);
    assert_eq!(ctx.ref_order_hint, hints, "a zero mask updates nothing");

    let ra = cfg(
        sf::SFrameMode::StrictBase,
        32,
        pp::PredStructure::RandomAccess,
    );
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.ref_order_hint = hints;
    ctx.key_poc = 96;
    let mut pic = inter(200, 0);
    pic.rps.refresh_frame_mask = 0b0000_1010;
    sf::update_sframe_ref_order_hint(&mut pic, &ra, &mut ctx);
    assert_eq!(pic.dpb_order_hint, hints);
    // 200 mod 128 = 72, written into slots 1 and 3 only.
    assert_eq!(ctx.ref_order_hint, [100, 72, 102, 72, 104, 105, 106, 107]);
}

/// `prune_sframe_refs` (`pd_process.c:1003`).
///
/// Derivation. The gate is `sframe_poc > 0 && picture_number < sframe_poc &&
/// mfmv_enabled`, so it fires only for pictures BEFORE a pending switch. A
/// candidate is dropped when either half of its reference pair is a list-0
/// reference (`< BWDREF_FRAME`) whose order hint equals the S-frame's.
///
/// The compaction is the interesting half: C does NOT advance the index after
/// a removal, because the entry that shifted down into the slot has not been
/// examined. Dropping two ADJACENT entries is therefore the case that a
/// naive `for` loop gets wrong, and it is what this drives.
///
/// Setup, chosen around C's own index (`ref_order_hint[rf]`, NOT `rf - 1` —
/// see the port's doc comment and `docs/SUSPECTED-C-BUGS.md`): hints
/// `[7, 40, 40, 9, ...]` with `sframe_poc = 40` make single-reference LAST (1)
/// and LAST2 (2) match at indices 1 and 2, while LAST3 (3) reads 9 and BWDREF
/// (5) fails the `< BWDREF_FRAME` test outright. `ref_order_hint[0]` is
/// deliberately NOT 40, so the port's `rf < 0` no-match is not doing the work
/// here.
#[test]
fn traced_prune_sframe_refs_compaction() {
    let c = cfg(
        sf::SFrameMode::StrictBase,
        32,
        pp::PredStructure::RandomAccess,
    );
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.sframe_poc = 40;

    let mut pic = inter(30, 0);
    pic.ref_order_hint = [7, 40, 40, 9, 10, 11, 12];

    let mut arr = [1i8, 2, 3, 5, 0, 0, 0, 0];
    let mut tot = 4u8;
    let pruned = sf::prune_sframe_refs(&pic, &c, &ctx, &mut arr, &mut tot);
    assert!(pruned);
    assert_eq!(tot, 2, "two ADJACENT entries removed");
    assert_eq!(&arr[..2], &[3i8, 5]);

    // Nothing matches: a full no-op, so the compaction above is not an
    // artefact of the loop always removing.
    let mut pic2 = inter(30, 0);
    pic2.ref_order_hint = [7, 8, 9, 10, 11, 12, 13];
    let mut arr2 = [1i8, 2, 3, 5, 0, 0, 0, 0];
    let mut tot2 = 4u8;
    assert!(!sf::prune_sframe_refs(
        &pic2, &c, &ctx, &mut arr2, &mut tot2
    ));
    assert_eq!(tot2, 4);

    // The three gates, each a full no-op.
    for (poc, sframe_poc, mfmv) in [(30u64, 0u64, true), (50, 40, true), (30, 40, false)] {
        let mut c2 = c;
        c2.mfmv_enabled = mfmv;
        let mut ctx2 = pp::PicDecisionCtx::new();
        ctx2.sframe_poc = sframe_poc;
        let mut pic2 = inter(poc, 0);
        pic2.ref_order_hint = [7, 40, 40, 9, 10, 11, 12];
        let mut arr2 = [1i8, 2, 3, 5, 0, 0, 0, 0];
        let mut tot2 = 4u8;
        assert!(!sf::prune_sframe_refs(
            &pic2, &c2, &ctx2, &mut arr2, &mut tot2
        ));
        assert_eq!(tot2, 4, "poc {poc} sframe_poc {sframe_poc} mfmv {mfmv}");
    }
}

/// The hooks wired into `generate_rps_info_sframe`, and the ORDER that makes
/// them correct.
///
/// `set_sframe_rps` forces `refresh_frame_mask` to `0xFF` and must run BEFORE
/// the long-term-reference dispatcher, whose phase 3 masks held anchors back
/// out of it. Driven here with slot 6 held: the S-frame's mask must come out
/// `0xBF`, not `0xFF` and not the branch's own choice.
#[test]
fn traced_sframe_hooks_run_before_the_ref_mgmt_guard() {
    use core::num::NonZeroU32;

    let seq = pp::SeqPicParams {
        pred_structure: pp::PredStructure::LowDelay,
        rate_control_mode: pp::RcMode::CqpOrCrf,
        hierarchical_levels: 0,
        max_managed_refs: 4,
        ..pp::SeqPicParams::default()
    };
    let c = cfg(sf::SFrameMode::StrictBase, 32, pp::PredStructure::LowDelay);
    let mut enc = pp::EncCtxPicParams::default();
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[6] = NonZeroU32::new(11);

    let mut pic = pp::PicParams {
        picture_number: 32,
        decode_order: 32,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        ..Default::default()
    };
    pp::generate_rps_info_sframe(
        &mut pic,
        &seq,
        &mut ctx,
        0,
        0,
        Some(pp::SFrameHooks {
            cfg: &c,
            enc_ctx: &mut enc,
        }),
    )
    .unwrap();

    assert!(pic.is_switch_frame, "POC 32 at dist 32 is the switch point");
    assert!(pic.error_resilient_mode);
    assert_eq!(
        pic.rps.refresh_frame_mask, 0xBF,
        "0xFF from the S-frame, minus the held anchor in slot 6"
    );
    assert_eq!(ctx.sframe_poc, 32);

    // Without the hooks the same picture is an ordinary inter frame and the
    // branch's own refresh mask survives, so the difference above is the
    // S-frame path and not the guard.
    let mut ctx2 = pp::PicDecisionCtx::new();
    let mut pic2 = pp::PicParams {
        picture_number: 32,
        decode_order: 32,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        ..Default::default()
    };
    pp::generate_rps_info(&mut pic2, &seq, &mut ctx2, 0, 0).unwrap();
    assert!(!pic2.is_switch_frame);
    assert_ne!(pic2.rps.refresh_frame_mask, 0xFF);
}
