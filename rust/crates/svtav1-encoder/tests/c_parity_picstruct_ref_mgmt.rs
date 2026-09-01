//! Long-term reference management (`Codec/pd_process.c:1162-1478`) against the
//! REAL exported C symbols — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4) for the two functions that have one, plus
//! the traced coverage for the `static` dispatcher around them.
//!
//! `svt_aom_ref_mgmt_storeable_slots_mask` is the interesting one: it calls
//! the FILE-STATIC `exclusive_write_slots_mask_ld_cbr`, so driving the
//! exported wrapper exhaustively over its whole input domain drives that
//! helper too — the static gets tier-1 coverage without a symbol of its own.

use core::num::NonZeroU32;

use svtav1_cref::ref_mgmt as cref;
use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_ref_mgmt as rm;

fn seq(rtc: bool, hier: u8, pred: pp::PredStructure, ld_reduce: u8) -> pp::SeqPicParams {
    pp::SeqPicParams {
        pred_structure: pred,
        rtc,
        hierarchical_levels: hier,
        max_managed_refs: 4,
        mrp_ctrls: pp::MrpCtrls {
            ld_reduce_ref_buffs: ld_reduce,
            ..pp::MrpCtrls::default()
        },
        ..pp::SeqPicParams::default()
    }
}

/// TIER 1. `svt_aom_ref_mgmt_storeable_slots_mask` over its ENTIRE input
/// domain: every `rtc`, every `hierarchical_levels` C accepts (0..=6), every
/// `PredStructure`, every `ld_reduce_ref_buffs` the field can hold.
///
/// The out-of-range `ld_reduce_ref_buffs` values matter as much as the valid
/// ones: C's `default:` arm is a bare `assert(0)`, which under `NDEBUG` — the
/// Release build this oracle links — adds no bits. A port that "helpfully"
/// panicked or picked a fallback there would diverge from the shipping
/// library, so the differential covers 0..=255.
#[test]
fn c_parity_storeable_slots_mask_full_domain() {
    let mut cells = 0usize;
    let mut distinct = std::collections::HashSet::new();
    for rtc in [false, true] {
        for hier in 0u8..=6 {
            for (pred_idx, pred) in [
                pp::PredStructure::AllIntra,
                pp::PredStructure::LowDelay,
                pp::PredStructure::RandomAccess,
            ]
            .into_iter()
            .enumerate()
            {
                for ld_reduce in 0u8..=255 {
                    let s = seq(rtc, hier, pred, ld_reduce);
                    let got = rm::storeable_slots_mask(&s);
                    let want = cref::storeable_slots_mask(
                        rtc,
                        hier,
                        u8::try_from(pred_idx).unwrap(),
                        ld_reduce,
                    );
                    assert_eq!(
                        got, want,
                        "rtc={rtc} hier={hier} pred={pred_idx} ld_reduce={ld_reduce}"
                    );
                    cells += 1;
                    distinct.insert(want);
                }
            }
        }
    }
    assert_eq!(cells, 2 * 7 * 3 * 256);
    // Anti-vacuity: a stub returning one constant would pass a comparison that
    // never varies.
    assert!(
        distinct.len() >= 4,
        "only {} distinct masks over the whole domain",
        distinct.len()
    );
}

/// TIER 1. `svt_aom_is_pic_skipped` over its entire input domain — three
/// booleans, so all eight combinations.
#[test]
fn c_parity_is_pic_skipped_full_domain() {
    let mut any_true = false;
    for is_ref in [false, true] {
        for gen_pass in [0u8, 1] {
            for first in [0u8, 1] {
                let want = cref::is_pic_skipped(is_ref, gen_pass, first);
                // C: `!is_ref && rc_stat_gen_pass_mode && !first_frame_in_minigop`.
                let got = !is_ref && gen_pass != 0 && first == 0;
                assert_eq!(
                    got, want,
                    "is_ref={is_ref} gen_pass={gen_pass} first={first}"
                );
                any_true |= want;
            }
        }
    }
    assert!(
        any_true,
        "no input makes C skip a picture — the shim is inert"
    );
}

// ---------------------------------------------------------------------------
// The `static` dispatcher — tier 4, derivations written out
// ---------------------------------------------------------------------------

fn id(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("test ids are nonzero")
}

fn base_pic() -> pp::PicParams {
    pp::PicParams {
        picture_number: 10,
        slice_type: pp::SliceType::B,
        temporal_layer_index: 0,
        hierarchical_levels: 2,
        pred_struct_type: pp::PredStructure::LowDelay,
        is_ref: true,
        ..Default::default()
    }
}

/// C `apply_ref_mgmt_events` phase 3 (`pd_process.c:1436-1462`) — the reason
/// this module is NOT dead code with no events queued.
///
/// Derivation: the guard runs unconditionally. With slot 5 held by an earlier
/// STORE and a branch that chose `refresh_frame_mask = 0x21` (slots 0 and 5),
/// the mask comes out `0x01` — slot 5 is preserved. With nothing held the
/// derived mask is 0 and `& !0` leaves the branch's choice untouched, which is
/// what made omitting this whole file byte-inert until an anchor exists.
#[test]
fn traced_phase3_refresh_guard_runs_without_events() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);

    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x21;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(pic.rps.refresh_frame_mask, 0x21, "no anchors held: no-op");
    assert!(report.diagnostics.is_empty());

    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[5] = Some(id(7));
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x21;
    rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(pic.rps.refresh_frame_mask, 0x01, "slot 5 is preserved");
}

/// The collapse diagnostic (`pd_process.c:1451-1461`): the branch wanted only
/// slots a STORE holds, so the frame codes normally but never enters the DPB.
#[test]
fn traced_phase3_reports_a_collapsed_mask() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[5] = Some(id(7));
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x20;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(pic.rps.refresh_frame_mask, 0);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::RefreshMaskCollapsed {
                wanted: 0x20,
                preserved: 0x20,
            })
    );

    // Control: a branch that wanted nothing does NOT report a collapse, because
    // C guards the diagnostic on `orig != 0`.
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[5] = Some(id(7));
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(report.diagnostics.is_empty());
}

/// Phase order (`pd_process.c:1424-1470`): CLEAR frees a slot before STORE
/// looks for one, so a picture may release and re-claim the same slot.
///
/// Derivation: low delay, `hierarchical_levels = 2`, `ld_reduce = 0`. The
/// exclusive mask is `0x07 | (1<<3 | 1<<4) | (1<<5)` = `0x3F`, so the
/// storeable pool is `0xC0` — slots 6 and 7. With slot 6 held by id 1 and slot
/// 7 held by id 2, a STORE would find no free slot; CLEARing id 1 first frees
/// slot 6, and `trailing_zeros` picks the LOWEST free slot, which is 6 again.
#[test]
fn traced_clear_runs_before_store() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);
    assert_eq!(
        rm::storeable_slots_mask(&s),
        0xC0,
        "the derivation's premise"
    );

    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[6] = Some(id(1));
    ctx.pic_id_per_dpb_slot[7] = Some(id(2));
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x02;
    pic.ref_mgmt = rm::RefMgmtEvents {
        clear_id: Some(id(1)),
        store_id: Some(id(3)),
        use_id: None,
    };
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(report.new_store_slot, Some(6));
    assert_eq!(ctx.pic_id_per_dpb_slot[6], Some(id(3)));
    assert_eq!(ctx.pic_id_per_dpb_slot[7], Some(id(2)));
    // The STORE force-refreshes its own slot; slot 7 stays preserved.
    assert_eq!(pic.rps.refresh_frame_mask, 0x42);
    assert!(report.diagnostics.is_empty());
}

/// A failed STORE scrubs `store_id` (`pd_process.c:1430-1434`), so
/// packetization does not stamp the "reference stored" flag for an anchor that
/// does not exist. Three ways to fail, each also a no-op.
#[test]
fn traced_failed_store_is_a_noop_and_scrubs_the_id() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);

    // (a) duplicate id.
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[6] = Some(id(1));
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x02;
    pic.ref_mgmt.store_id = Some(id(1));
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(report.new_store_slot, None);
    assert_eq!(pic.ref_mgmt.store_id, None);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::StoreIdAlreadyHeld(id(1)))
    );

    // (b) the pool is full — both storeable slots are held.
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[6] = Some(id(1));
    ctx.pic_id_per_dpb_slot[7] = Some(id(2));
    let mut pic = base_pic();
    pic.ref_mgmt.store_id = Some(id(3));
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(report.new_store_slot, None);
    assert!(report.diagnostics.contains(rm::RefMgmtDiag::StorePoolFull {
        storeable_mask: 0xC0
    }));

    // (c) the simultaneous-hold cap. Random access makes every slot storeable,
    // so the pool is not the limit; `max_managed_refs = 2` is.
    let mut s2 = seq(false, 2, pp::PredStructure::RandomAccess, 0);
    s2.max_managed_refs = 2;
    assert_eq!(rm::storeable_slots_mask(&s2), 0xFF);
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[0] = Some(id(1));
    ctx.pic_id_per_dpb_slot[1] = Some(id(2));
    let mut pic = base_pic();
    pic.ref_mgmt.store_id = Some(id(3));
    let report = rm::apply_events(&mut pic, &s2, &mut ctx);
    assert_eq!(report.new_store_slot, None);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::StoreCapReached { held: 2, cap: 2 })
    );
}

/// USE (`pd_process.c:1336-1356` + `:1464-1477`): all seven reference
/// positions point at the anchor, the list counts clamp to (1, 0), and the
/// refresh mask becomes every NON-held slot — a recovery point.
///
/// Derivation with slot 6 holding id 1 and slot 7 holding id 2: the anchor is
/// slot 6, `ref_dpb_index` is `[6; 7]`, `ref_poc_array` is the POC the shadow
/// DPB records for slot 6, and the mask is `0xFF & ~0xC0` = `0x3F`.
#[test]
fn traced_use_redirects_and_sets_a_recovery_point() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[6] = Some(id(1));
    ctx.pic_id_per_dpb_slot[7] = Some(id(2));
    ctx.dpb[6].picture_number = 42;
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x02;
    pic.ref_list0_count = 3;
    pic.ref_list1_count = 2;
    pic.ref_mgmt.use_id = Some(id(1));

    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(report.use_applied);
    assert_eq!(pic.rps.ref_dpb_index, [6; 7]);
    assert_eq!(pic.rps.ref_poc_array, [42; 7]);
    assert_eq!((pic.ref_list0_count, pic.ref_list1_count), (1, 0));
    assert_eq!(pic.rps.refresh_frame_mask, 0x3F);

    // An unknown id is a diagnostic and a full no-op.
    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.rps.refresh_frame_mask = 0x02;
    pic.ref_mgmt.use_id = Some(id(9));
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(!report.use_applied);
    assert_eq!(pic.rps.refresh_frame_mask, 0x02);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::UseIdNotFound(id(9)))
    );
}

/// The three gates that reject a whole event set (`pd_process.c:1383-1417`):
/// an overlay frame, a non-base frame, and a `pic_id` reused across two events
/// of the same picture. Each scrubs all three ids.
#[test]
fn traced_event_gates_reject_the_whole_set() {
    let s = seq(false, 2, pp::PredStructure::LowDelay, 0);
    let events = rm::RefMgmtEvents {
        store_id: Some(id(3)),
        clear_id: None,
        use_id: None,
    };

    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.is_overlay = true;
    pic.ref_mgmt = events;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::EventsOnOverlay)
    );
    assert_eq!(pic.ref_mgmt, rm::RefMgmtEvents::default());

    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.temporal_layer_index = 1;
    pic.ref_mgmt = events;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::EventsOnNonBase { temporal_layer: 1 })
    );

    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.ref_mgmt = rm::RefMgmtEvents {
        store_id: Some(id(3)),
        clear_id: Some(id(3)),
        use_id: None,
    };
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert!(
        report
            .diagnostics
            .contains(rm::RefMgmtDiag::DuplicateIdAcrossEvents)
    );
    assert_eq!(ctx.pic_id_per_dpb_slot, [None; 8]);

    // Control: the same STORE on a legal frame succeeds, so the gates are
    // rejecting the frame and not the event.
    let mut ctx = pp::PicDecisionCtx::new();
    let mut pic = base_pic();
    pic.ref_mgmt = events;
    let report = rm::apply_events(&mut pic, &s, &mut ctx);
    assert_eq!(report.new_store_slot, Some(6));
}

/// A key frame destroys every anchor (`set_key_frame_rps` ->
/// `ref_mgmt_reset_state`, `pd_process.c:1483`), and the port's
/// `generate_rps_info` runs the dispatcher on that path too, so a STORE queued
/// on the key frame still lands.
#[test]
fn traced_key_frame_resets_state_then_still_stores() {
    let s = pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        hierarchical_levels: 4,
        max_managed_refs: 4,
        ..pp::SeqPicParams::default()
    };
    let mut ctx = pp::PicDecisionCtx::new();
    ctx.pic_id_per_dpb_slot[3] = Some(id(1));
    ctx.lay0_toggle = 2;

    let mut kf = pp::PicParams {
        picture_number: 0,
        slice_type: pp::SliceType::I,
        is_key_frame: true,
        is_intra_only: true,
        hierarchical_levels: 4,
        pred_struct_type: pp::PredStructure::RandomAccess,
        ..Default::default()
    };
    kf.ref_mgmt.store_id = Some(id(5));

    pp::generate_rps_info(&mut kf, &s, &mut ctx, 0, 0).unwrap();

    assert_eq!(ctx.lay0_toggle, 0, "the key frame resets the toggles");
    assert_eq!(ctx.pic_id_per_dpb_slot[3], None, "the old anchor is gone");
    assert_eq!(
        ctx.pic_id_per_dpb_slot[0],
        Some(id(5)),
        "the new one landed"
    );
    // A key frame already refreshes all eight slots, so the forced STORE bit
    // changes nothing and the guard preserves only the slot it just claimed.
    assert_eq!(kf.rps.refresh_frame_mask, 0xFF);
}
