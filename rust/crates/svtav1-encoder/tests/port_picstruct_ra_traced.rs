//! Hand-derived vectors for the random-access **hierarchical** RPS branches
//! (`Codec/pd_process.c:2270-3482`) — evidence **tier 4**
//! (`docs/WORKING-ON-THIS.md` §4), because `av1_generate_rps_info` is `static`
//! with no exported symbol (`nm -g Bin/Release/libSvtAv1Enc.a` has no entry).
//!
//! Every expectation is derived by reading the C, with the derivation written
//! out above it. That is the only defence a tier-4 vector has against being a
//! second transcription of the same mistake — so where a value is surprising,
//! the surprise is stated.
//!
//! The end-to-end reference structure is checked at a stronger tier by
//! `c_parity_picstruct_ra_rps.rs`, which reads `refresh_frame_flags` and
//! `ref_frame_idx[]` out of the REAL C encoder's bitstream.

use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_picstruct::{ALT, ALT2, BWD, GOLD, LAST, LAST2, LAST3};
use svtav1_encoder::port_picstruct_ra as ra;
use svtav1_encoder::port_picstruct_ra::Slot;

fn ra_seq() -> pp::SeqPicParams {
    pp::SeqPicParams {
        pred_structure: pp::PredStructure::RandomAccess,
        rate_control_mode: pp::RcMode::CqpOrCrf,
        rtc: false,
        allintra: false,
        mrp_ctrls: pp::MrpCtrls::default(),
        order_hint_info: svtav1_encoder::inter_mvp::OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: 7,
        },
        hierarchical_levels: 0,
        max_managed_refs: 0,
    }
}

fn ra_pic(hier: u8, tl: u8) -> pp::PicParams {
    pp::PicParams {
        picture_number: 1,
        decode_order: 1,
        slice_type: pp::SliceType::B,
        is_key_frame: false,
        is_intra_only: false,
        temporal_layer_index: tl,
        hierarchical_levels: hier,
        pred_struct_type: pp::PredStructure::RandomAccess,
        pred_struct_entry_count: 1u32 << hier,
        is_ref: true,
        aligned_width: 64,
        aligned_height: 64,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// toggles_for_picture — the per-branch toggle preamble
// ---------------------------------------------------------------------------

/// C `pd_process.c:2271-2288` (and its four copies).
///
/// Derivation. In RANDOM_ACCESS the guard
/// `pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS` is false, so the local
/// toggles equal the context's. With `lay0_toggle = 0`:
/// `base2 = 0`, `base1 = CIRC_DEC(0, 0, 2) = 2`, `base0 = CIRC_DEC(2, 0, 2) =
/// 1` — note the base indices WRAP, so "oldest" is slot 1, not slot 2.
/// With `lay1_toggle = 0`: `lay1_1 = LAY1_OFF + 0 = 3`,
/// `lay1_0 = CIRC_DEC(3, 3, 4) = 4` — which wraps to the HIGHER slot, because
/// the layer-1 pair is a two-slot ring at {3, 4}.
#[test]
fn traced_toggles_random_access_no_adjustment() {
    let ctx = pp::PicDecisionCtx::default();
    for hier in 1u8..=5 {
        for tl in 0..=hier {
            let pic = ra_pic(hier, tl);
            let idx = ra::toggles_for_picture(&pic, &ctx, 0);
            assert_eq!(
                (idx.base2, idx.base1, idx.base0, idx.lay1_cur, idx.lay1_prev),
                (0, 2, 1, 3, 4),
                "HL{hier} TL{tl}: random access must not adjust the toggles"
            );
        }
    }
}

/// The low-delay adjustment, and the `(1 << (hier - 1)) - 1` threshold.
///
/// Derivation. A picture whose own pred struct is LOW_DELAY inside an RA
/// sequence is decoded in display order, so C advances the LOCAL layer-0
/// toggle by one (`CIRC_INC(0, 0, 2) = 1`) for every non-base picture, and
/// flips the LOCAL layer-1 toggle only for the first half of the mini-GOP:
/// `pic_idx == 0` at HL2, `< 3` at HL3, `< 7` at HL4, `< 15` at HL5, and never
/// at HL1 (which has no such line at all).
///
/// Surprise worth stating: the adjustment is keyed on `temporal_layer != 0`,
/// NOT on the picture being non-base in some other sense, so a base picture in
/// an incomplete mini-GOP is left alone.
#[test]
fn traced_toggles_low_delay_adjustment_threshold() {
    let ctx = pp::PicDecisionCtx::default();
    // Base pictures never adjust, whatever the pred struct.
    for hier in 1u8..=5 {
        let mut pic = ra_pic(hier, 0);
        pic.pred_struct_type = pp::PredStructure::LowDelay;
        let idx = ra::toggles_for_picture(&pic, &ctx, 0);
        assert_eq!((idx.base2, idx.lay1_cur), (0, 3), "HL{hier} base");
    }

    // Non-base: layer 0 always advances 0 -> 1.
    for (hier, first_half) in [(1u8, 0u32), (2, 1), (3, 3), (4, 7), (5, 15)] {
        for pic_idx in [
            0u32,
            first_half.saturating_sub(1),
            first_half,
            first_half + 1,
        ] {
            let mut pic = ra_pic(hier, 1);
            pic.pred_struct_type = pp::PredStructure::LowDelay;
            let idx = ra::toggles_for_picture(&pic, &ctx, pic_idx);
            assert_eq!(idx.base2, 1, "HL{hier} pic_idx {pic_idx}: layer-0 advance");
            let want_lay1 = if pic_idx < first_half { 4 } else { 3 };
            assert_eq!(
                idx.lay1_cur, want_lay1,
                "HL{hier} pic_idx {pic_idx}: layer-1 flip iff pic_idx < {first_half}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// slot_table — the rows whose entries are CONDITIONAL
// ---------------------------------------------------------------------------

/// HL1 layer 1 (`pd_process.c:2335-2342`) — the only row `referencing_scheme`
/// moves.
///
/// Derivation, read off the two ternaries:
/// `LAST2 = referencing_scheme == 0 ? base0_idx : lay1_1_idx` and
/// `ALT2 = referencing_scheme == 0 ? ref_dpb_index[BWD] : lay1_0_idx`, where
/// `BWD` was assigned `base2_idx` two lines earlier. Everything else is fixed:
/// `LAST = base1`, `LAST3 = base0`, `GOLD = LAST`, `ALT = BWD`.
#[test]
fn traced_hl1_layer1_referencing_scheme() {
    let scheme0 = ra::slot_table(1, 1, 0, 0, false, false).expect("HL1 TL1 is a coded position");
    assert_eq!(
        scheme0,
        [
            Slot::Base1,
            Slot::Base0,
            Slot::Base0,
            Slot::Base1,
            Slot::Base2,
            Slot::Base2,
            Slot::Base2
        ]
    );
    let scheme1 = ra::slot_table(1, 1, 0, 1, false, false).unwrap();
    assert_eq!(scheme1[LAST2], Slot::Lay1Cur);
    assert_eq!(scheme1[ALT2], Slot::Lay1Prev);
    // Only those two entries move.
    for i in [LAST, LAST3, GOLD, BWD, ALT] {
        assert_eq!(scheme0[i], scheme1[i], "entry {i} is not scheme-dependent");
    }
}

/// The five rows `mrp_ctrls.more_5L_refs` moves — HL4 only
/// (`pd_process.c:2763`, `:2769`, `:2807`, `:2819`, `:2848`, `:2927`).
///
/// Derivation: each is `more_5L_refs ? <a deeper-layer slot> : <the mirrored
/// neighbour>`. With the knob OFF every one collapses onto the entry it
/// mirrors, so the OFF row must contain no `Lay3`/`Lay4` in those positions.
#[test]
fn traced_hl4_more_5l_refs_rows() {
    /// `(index, more_5L-on value, more_5L-off value)`.
    type Cond = (usize, Slot, Slot);
    // (temporal_layer, pic_idx, conditional entries). Layer 0 is the one row
    // with TWO of them.
    let cases: &[(u8, u32, &[Cond])] = &[
        (
            0,
            0,
            &[
                (LAST3, Slot::Lay1Cur, Slot::Base2),
                (ALT, Slot::Lay1Prev, Slot::Base2),
            ],
        ),
        (2, 3, &[(ALT, Slot::Lay3, Slot::Lay1Cur)]),
        (2, 11, &[(ALT, Slot::Lay1Prev, Slot::Base2)]),
        (3, 5, &[(ALT, Slot::Lay4, Slot::Lay1Cur)]),
        (4, 6, &[(ALT, Slot::Lay1Prev, Slot::Lay1Cur)]),
    ];
    for &(tl, pic_idx, entries) in cases {
        let row_on = ra::slot_table(4, tl, pic_idx, 1, true, false).unwrap();
        let row_off = ra::slot_table(4, tl, pic_idx, 1, false, false).unwrap();
        for &(i, on, off) in entries {
            assert_eq!(
                row_on[i], on,
                "HL4 TL{tl} pic {pic_idx} entry {i}, more_5L on"
            );
            assert_eq!(
                row_off[i], off,
                "HL4 TL{tl} pic {pic_idx} entry {i}, more_5L off"
            );
        }
        for j in 0..7 {
            if !entries.iter().any(|&(i, _, _)| i == j) {
                assert_eq!(row_on[j], row_off[j], "HL4 TL{tl} pic {pic_idx} entry {j}");
            }
        }
    }
    // No other hierarchy reads the knob.
    for hier in [1u8, 2, 3, 5] {
        for tl in 0..=hier {
            for pic_idx in 0..(1u32 << hier) {
                let a = ra::slot_table(hier, tl, pic_idx, 1, true, false);
                let b = ra::slot_table(hier, tl, pic_idx, 1, false, false);
                assert_eq!(a, b, "HL{hier} TL{tl} pic {pic_idx} must ignore more_5L");
            }
        }
    }
}

/// Which `(temporal_layer, pic_idx)` pairs each branch codes at all.
///
/// Derivation, from the `if (pic_idx == …)` chains: a picture at temporal
/// layer `t` of an `H`-level pyramid sits at the display positions whose
/// `pic_idx + 1` is divisible by `2^(H - t)` but not by `2^(H - t + 1)`. So
/// layer H occupies the even `pic_idx`, layer H-1 the pic_idx ≡ 1 (mod 4), and
/// so on; layers 0 and 1 have a single position each and C does not switch on
/// `pic_idx` for them at all (it accepts any).
///
/// Anything else is C's `SVT_LOG("Error in MG indexing …")` fall-through,
/// which this port turns into `None`.
#[test]
fn traced_coded_positions_per_layer() {
    for hier in 1u8..=5 {
        let mg = 1u32 << hier;
        for tl in 0..=hier {
            for pic_idx in 0..mg {
                let coded = ra::slot_table(hier, tl, pic_idx, 1, false, false).is_some();
                let want = if tl <= 1 {
                    true // C does not switch on pic_idx for layers 0 and 1
                } else {
                    let period = 1u32 << (u32::from(hier) - u32::from(tl));
                    (pic_idx + 1) % period == 0 && (pic_idx + 1) % (period * 2) != 0
                };
                assert_eq!(
                    coded, want,
                    "HL{hier} TL{tl} pic_idx {pic_idx}: coded position mismatch"
                );
            }
        }
    }
    // Positive control on the negative half: the table really does reject.
    assert!(ra::slot_table(5, 5, 1, 1, false, false).is_none());
    assert!(ra::slot_table(4, 2, 5, 1, false, false).is_none());
    assert!(ra::slot_table(6, 0, 0, 1, false, false).is_none());
}

/// An overlay frame at the top layer references only the newest base picture
/// (`pd_process.c:2325`, `:2438`, `:2617`, `:2879`, `:3251`).
#[test]
fn traced_overlay_rows_are_all_base2() {
    for hier in 1u8..=5 {
        let row = ra::slot_table(hier, hier, 0, 1, false, true).unwrap();
        assert_eq!(row, [Slot::Base2; 7], "HL{hier} overlay");
    }
}

// ---------------------------------------------------------------------------
// show_existing_slot
// ---------------------------------------------------------------------------

/// C's `show_existing_frame` slot per top-layer position (`pd_process.c:2352`,
/// `:2494`, `:2694`, `:2957`, `:3402`).
///
/// Derivation: only the EVEN `pic_idx` are top-layer positions, and each one
/// re-displays the deepest hidden picture that just became displayable. The
/// tables read as a ruler sequence — `pic_idx` 30 shows the base, 14 shows
/// layer 1, 6 and 22 show layer 2, and so on — but they are transcribed
/// literally, and this test states the transcription, not the pattern.
#[test]
fn traced_show_existing_slots() {
    assert_eq!(ra::show_existing_slot(1, 0), Some(Slot::Base2));

    assert_eq!(ra::show_existing_slot(2, 0), Some(Slot::Lay1Cur));
    assert_eq!(ra::show_existing_slot(2, 2), Some(Slot::Base2));

    assert_eq!(ra::show_existing_slot(3, 0), Some(Slot::Lay2));
    assert_eq!(ra::show_existing_slot(3, 2), Some(Slot::Lay1Cur));
    assert_eq!(ra::show_existing_slot(3, 4), Some(Slot::Lay2));
    assert_eq!(ra::show_existing_slot(3, 6), Some(Slot::Base2));

    let hl4 = [
        Slot::Lay3,
        Slot::Lay2,
        Slot::Lay3,
        Slot::Lay1Cur,
        Slot::Lay3,
        Slot::Lay2,
        Slot::Lay3,
        Slot::Base2,
    ];
    for (k, want) in hl4.into_iter().enumerate() {
        assert_eq!(
            ra::show_existing_slot(4, 2 * k as u32),
            Some(want),
            "HL4 {k}"
        );
    }

    let hl5 = [
        Slot::Lay4,
        Slot::Lay3,
        Slot::Lay4,
        Slot::Lay2,
        Slot::Lay4,
        Slot::Lay3,
        Slot::Lay4,
        Slot::Lay1Cur,
        Slot::Lay4,
        Slot::Lay3,
        Slot::Lay4,
        Slot::Lay2,
        Slot::Lay4,
        Slot::Lay3,
        Slot::Lay4,
        Slot::Base2,
    ];
    for (k, want) in hl5.into_iter().enumerate() {
        assert_eq!(
            ra::show_existing_slot(5, 2 * k as u32),
            Some(want),
            "HL5 {k}"
        );
    }

    // Odd positions are not top-layer positions at any level.
    for hier in 1u8..=5 {
        for pic_idx in (1..(1u32 << hier)).step_by(2) {
            assert_eq!(
                ra::show_existing_slot(hier, pic_idx),
                None,
                "HL{hier} pic_idx {pic_idx}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// rps_random_access_hier — masks, toggles, show flags
// ---------------------------------------------------------------------------

/// The refresh mask per layer, and which arms are gated on `is_ref`.
///
/// Derivation, one line each:
/// * layer 0 — `ctx->lay0_toggle = CIRC_INC(…, 0, 2); mask = 1 << toggle`,
///   never gated (`:2306`, `:2414`, `:2568`, `:2771`, `:3060`);
/// * layer 1 — `ctx->lay1_toggle = 1 - toggle; mask = 1 << (LAY1_OFF +
///   toggle)`, gated on `is_ref` ONLY at HL1, where layer 1 is the top layer
///   (`:2341` vs `:2431`, `:2581`, `:2790`, `:3075`);
/// * layers 2..4 — a fixed slot, gated on `is_ref` only when the layer IS the
///   top layer;
/// * HL5's layer 5 — `mask = 0` **unconditionally** (`:3424`), not through
///   `is_ref`. That asymmetry against HL1..HL4's top layers is C's, and is the
///   surprise this test exists to pin.
#[test]
fn traced_refresh_masks_and_gating() {
    let seq = ra_seq();
    // (hier, tl, pic_idx, is_ref, expected mask)
    let cases: &[(u8, u8, u32, bool, u8)] = &[
        // layer 0: toggle 0 -> 1, mask = 1 << 1
        (1, 0, 0, true, 0b0000_0010),
        (5, 0, 0, true, 0b0000_0010),
        // layer 1: toggle 0 -> 1, mask = 1 << (3 + 1) = 1 << 4
        (2, 1, 0, true, 0b0001_0000),
        (5, 1, 0, true, 0b0001_0000),
        // HL1 layer 1 IS the top layer -> is_ref-gated
        (1, 1, 0, true, 0b0001_0000),
        (1, 1, 0, false, 0),
        // layer 2 below the top: unconditional 1 << LAY2_OFF (= 1 << 5)
        (3, 2, 1, false, 0b0010_0000),
        (5, 2, 7, false, 0b0010_0000),
        // layer 2 AS the top layer (HL2): gated
        (2, 2, 0, true, 0b0010_0000),
        (2, 2, 0, false, 0),
        // layer 3 below the top vs as the top
        (4, 3, 1, false, 0b0100_0000),
        (3, 3, 0, true, 0b0100_0000),
        (3, 3, 0, false, 0),
        // layer 4 below the top vs as the top
        (5, 4, 1, false, 0b1000_0000),
        (4, 4, 0, true, 0b1000_0000),
        (4, 4, 0, false, 0),
        // HL5 layer 5: always zero, is_ref or not
        (5, 5, 0, true, 0),
        (5, 5, 0, false, 0),
    ];
    for &(hier, tl, pic_idx, is_ref, want) in cases {
        let mut ctx = pp::PicDecisionCtx::default();
        let mut pic = ra_pic(hier, tl);
        pic.is_ref = is_ref;
        ra::rps_random_access_hier(&mut pic, &seq, &mut ctx, pic_idx, 0).unwrap();
        assert_eq!(
            pic.rps.refresh_frame_mask, want,
            "HL{hier} TL{tl} pic {pic_idx} is_ref={is_ref}"
        );
    }
}

/// The layer-1 toggle is advanced by every layer-1 picture EXCEPT an HL1
/// overlay (`pd_process.c:2325-2334` takes the overlay arm and never reaches
/// the `ctx->lay1_toggle` line).
///
/// Getting this wrong is invisible on the overlay frame itself — its mask is 0
/// either way — and desynchronises every later layer-1 slot assignment.
#[test]
fn traced_hl1_overlay_does_not_toggle_layer1() {
    let seq = ra_seq();
    for (is_overlay, want_toggle) in [(false, 1u8), (true, 0u8)] {
        let mut ctx = pp::PicDecisionCtx::default();
        let mut pic = ra_pic(1, 1);
        pic.is_overlay = is_overlay;
        pic.is_ref = !is_overlay;
        ra::rps_random_access_hier(&mut pic, &seq, &mut ctx, 0, 0).unwrap();
        assert_eq!(ctx.lay1_toggle, want_toggle, "is_overlay = {is_overlay}");
        assert_eq!(pic.rps.refresh_frame_mask, if is_overlay { 0 } else { 16 });
    }
}

/// `show_frame` / `has_show_existing` / `show_existing_frame` for a complete
/// random-access mini-GOP (`pd_process.c:2496-2507` and its four copies).
///
/// Derivation. `set_frame_display_params` returns false for a B frame of a
/// complete RA mini-GOP, so the branch's own tail runs: a picture BELOW the
/// top layer is coded hidden (`show_frame = false`), and a top-layer picture
/// is shown and additionally re-displays a previously hidden frame.
///
/// At HL3 with the toggles at 0, `lay1_1_idx = 3` and `base2_idx = 0`, so
/// pic_idx 2 re-displays slot 3 and pic_idx 6 re-displays slot 0.
#[test]
fn traced_show_flags_hl3_mini_gop() {
    let seq = ra_seq();

    for (tl, pic_idx) in [(0u8, 7u32), (1, 3), (2, 1), (2, 5)] {
        let mut ctx = pp::PicDecisionCtx::default();
        let mut pic = ra_pic(3, tl);
        ra::rps_random_access_hier(&mut pic, &seq, &mut ctx, pic_idx, 0).unwrap();
        assert!(!pic.show_frame, "HL3 TL{tl} pic {pic_idx} is coded hidden");
        assert!(!pic.has_show_existing);
    }

    for (pic_idx, want_slot) in [(0u32, 5u8), (2, 3), (4, 5), (6, 0)] {
        let mut ctx = pp::PicDecisionCtx::default();
        let mut pic = ra_pic(3, 3);
        ra::rps_random_access_hier(&mut pic, &seq, &mut ctx, pic_idx, 0).unwrap();
        assert!(pic.show_frame, "HL3 TL3 pic {pic_idx}");
        assert!(pic.has_show_existing);
        assert_eq!(
            pic.show_existing_frame, want_slot,
            "HL3 TL3 pic {pic_idx} re-displays"
        );
    }
}

/// The HL2 low-delay long-term base reference (`pd_process.c:2403-2407` and
/// `:2484-2489`) — the one place a branch reads the SEQUENCE's prediction
/// structure rather than the picture's.
///
/// Derivation. Under LOW_DELAY the layer-0 row's `LAST3` is slot 7 instead of
/// `LAST`, and slot 7 is refreshed whenever `picture_number -
/// last_long_base_pic >= 128` on a base picture. Under RANDOM_ACCESS neither
/// happens.
#[test]
fn traced_hl2_long_base_is_low_delay_only() {
    let ra_s = ra_seq();
    let ld_s = pp::SeqPicParams {
        pred_structure: pp::PredStructure::LowDelay,
        ..ra_seq()
    };

    // Random access: no long-term slot, no slot-7 refresh even past 128.
    let mut ctx = pp::PicDecisionCtx::default();
    let mut pic = ra_pic(2, 0);
    pic.picture_number = 200;
    ra::rps_random_access_hier(&mut pic, &ra_s, &mut ctx, 3, 0).unwrap();
    assert_eq!(pic.rps.refresh_frame_mask & (1 << 7), 0);
    assert_eq!(ctx.last_long_base_pic, 0);

    // Low delay, picture 200 with the last long base at 0: 200 >= 128, so slot
    // 7 joins the mask and the marker advances.
    let mut ctx = pp::PicDecisionCtx::default();
    let mut pic = ra_pic(2, 0);
    pic.pred_struct_type = pp::PredStructure::LowDelay;
    pic.picture_number = 200;
    ra::rps_random_access_hier(&mut pic, &ld_s, &mut ctx, 3, 0).unwrap();
    assert_eq!(pic.rps.refresh_frame_mask & (1 << 7), 1 << 7);
    assert_eq!(ctx.last_long_base_pic, 200);

    // Low delay at picture 100: below the 128 threshold, no refresh.
    let mut ctx = pp::PicDecisionCtx::default();
    let mut pic = ra_pic(2, 0);
    pic.pred_struct_type = pp::PredStructure::LowDelay;
    pic.picture_number = 100;
    ra::rps_random_access_hier(&mut pic, &ld_s, &mut ctx, 3, 0).unwrap();
    assert_eq!(pic.rps.refresh_frame_mask & (1 << 7), 0);
    assert_eq!(ctx.last_long_base_pic, 0);
}

/// The resolved DPB indices of one hand-walked HL2 random-access mini-GOP.
///
/// This is the only test here that walks the whole per-picture chain, so the
/// derivation is written out in full.
///
/// Setup: a key frame has just run, so `set_key_frame_rps` left both toggles at
/// 0 and `update_dpb` (mask 0xFF) put POC 0 in all eight slots. The mini-GOP
/// covers display POC 1..4 and is decoded base-first: (TL0, pic_idx 3),
/// (TL1, pic_idx 1), (TL2, pic_idx 0), (TL2, pic_idx 2).
///
/// Frame 1 — TL0, pic_idx 3. RA, so no toggle adjustment: base2 = 0,
/// base1 = 2, base0 = 1. Row `[base2, base0, base2, base2, base2, base1,
/// base2]` = `[0, 1, 0, 0, 0, 2, 0]`. Then `ctx->lay0_toggle` advances
/// 0 -> 1 and the mask is `1 << 1`.
///
/// Frame 2 — TL1, pic_idx 1. `lay0_toggle` is now 1: base2 = 1, base1 = 0,
/// base0 = 2; `lay1_toggle` still 0: lay1_1 = 3, lay1_0 = 4. Row
/// `[base1, lay1_1, base0, base1, base2, base2, base2]` = `[0, 3, 2, 0, 1, 1,
/// 1]`. Then `lay1_toggle` flips 0 -> 1, mask `1 << 4`.
///
/// Frame 3 — TL2, pic_idx 0. `lay1_toggle` is now 1: lay1_1 = 4,
/// lay1_0 = CIRC_DEC(4, 3, 4) = 3. Row `[base1, lay1_0, base0, base1, lay1_1,
/// base2, lay1_1]` = `[0, 3, 2, 0, 4, 1, 4]`.
///
/// Frame 4 — TL2, pic_idx 2. Same slot map. Row `[lay1_1, base1, lay2,
/// lay1_1, base2, base2, base2]` = `[4, 0, 5, 4, 1, 1, 1]`.
///
/// The DPB POCs are seeded distinct here so `prune_refs` cannot fire and
/// obscure the mapping; the POC-driven pruning is covered by
/// `traced_prune_refs_fold_order` in `port_picstruct_traced.rs`.
#[test]
fn traced_hl2_random_access_mini_gop_slots() {
    let seq = ra_seq();
    let mut ctx = pp::PicDecisionCtx::default();
    // Distinct POCs in every slot: list0/list1 counts saturate, prune is inert.
    for (i, e) in ctx.dpb.iter_mut().enumerate() {
        e.picture_number = 1000 + i as u64;
    }

    let expect: [(u8, u32, [u8; 7], u8); 4] = [
        (0, 3, [0, 1, 0, 0, 0, 2, 0], 0b0000_0010),
        (1, 1, [0, 3, 2, 0, 1, 1, 1], 0b0001_0000),
        (2, 0, [0, 3, 2, 0, 4, 1, 4], 0b0010_0000),
        (2, 2, [4, 0, 5, 4, 1, 1, 1], 0b0010_0000),
    ];
    for (tl, pic_idx, want_idx, want_mask) in expect {
        let mut pic = ra_pic(2, tl);
        pic.picture_number = u64::from(pic_idx) + 1;
        ra::rps_random_access_hier(&mut pic, &seq, &mut ctx, pic_idx, 0).unwrap();
        assert_eq!(
            pic.rps.ref_dpb_index, want_idx,
            "HL2 TL{tl} pic_idx {pic_idx} slots"
        );
        assert_eq!(
            pic.rps.refresh_frame_mask, want_mask,
            "HL2 TL{tl} pic_idx {pic_idx} mask"
        );
    }
    assert_eq!((ctx.lay0_toggle, ctx.lay1_toggle), (1, 1));
}

/// The reference-index constants are the order `slot_table`'s rows are written
/// in — the premise that makes a positional array literal equal to C's seven
/// named assignments.
#[test]
fn ref_index_order_matches_the_row_layout() {
    assert_eq!(
        [LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT],
        [0, 1, 2, 3, 4, 5, 6]
    );
}
