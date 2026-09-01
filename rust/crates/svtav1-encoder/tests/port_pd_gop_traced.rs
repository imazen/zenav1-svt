//! The GF-group grouping, frame-skip predicate and resize-scale reconciliation
//! left over from `Codec/pd_process.c` and `Codec/resize.c`.
//!
//! Tier 1 for `svt_aom_is_pic_skipped` (driven through the cref shim). Tier 4
//! for the rest, with the derivation above each vector.

use svtav1_cref::ref_mgmt as cref;
use svtav1_encoder::port_pd_gop as gop;
use svtav1_encoder::port_picstruct::SliceType;
use svtav1_encoder::port_superres_decision as sr;

/// TIER 1. `svt_aom_is_pic_skipped` over all eight of its inputs.
#[test]
fn c_parity_is_pic_skipped_full_domain() {
    let mut skipped = 0;
    for is_ref in [false, true] {
        for gen_pass in [false, true] {
            for first in [false, true] {
                let want = cref::is_pic_skipped(is_ref, u8::from(gen_pass), u8::from(first));
                let got = gop::is_pic_skipped(is_ref, gen_pass, first);
                assert_eq!(
                    got, want,
                    "is_ref={is_ref} gen_pass={gen_pass} first={first}"
                );
                skipped += usize::from(want);
            }
        }
    }
    assert_eq!(skipped, 1, "exactly one of the eight inputs is a skip");
}

fn pic(poc: u64, slice: SliceType, tl: u8) -> gop::GfPic {
    gop::GfPic {
        picture_number: poc,
        slice_type: slice,
        temporal_layer_index: tl,
        is_delayed_intra: false,
        is_incomp_mg_frame: false,
        idr_flag: false,
        end_of_sequence_flag: false,
    }
}

/// `store_gf_group` (`pd_process.c:4378`) — the start-of-group gate.
///
/// Derivation: the function runs for an I slice, for a non-delayed base-layer
/// picture, or for an incomplete-mini-GOP frame, and returns without touching
/// anything otherwise. A NON-base picture that is none of those is the case
/// that must produce `None`.
#[test]
fn traced_store_gf_group_only_runs_at_a_group_start() {
    let mg = [pic(1, SliceType::B, 1), pic(2, SliceType::B, 0)];

    assert!(gop::store_gf_group(&pic(0, SliceType::I, 0), &mg).is_some());
    assert!(gop::store_gf_group(&pic(4, SliceType::B, 0), &mg).is_some());

    let mut incomp = pic(5, SliceType::B, 2);
    incomp.is_incomp_mg_frame = true;
    assert!(gop::store_gf_group(&incomp, &mg).is_some());

    assert!(
        gop::store_gf_group(&pic(5, SliceType::B, 2), &mg).is_none(),
        "a non-base, non-I, non-incomplete picture starts no group"
    );

    // A DELAYED intra at temporal layer 0 still starts a group, but through
    // the I-slice arm rather than the base-layer one: the `!is_delayed_intra`
    // term switches that arm off.
    let mut delayed = pic(0, SliceType::B, 0);
    delayed.is_delayed_intra = true;
    assert!(
        gop::store_gf_group(&delayed, &mg).is_none(),
        "a delayed intra that is not an I slice fails all three arms"
    );
}

/// `store_gf_group`'s three group shapes (`pd_process.c:4380-4396`).
///
/// Derivation 1, delayed intra: the picture puts ITSELF at index 0 and the
/// whole mini-GOP after it, so a 4-picture mini-GOP gives `gf_interval = 5`.
///
/// Derivation 2, the IDR trim: an INCOMPLETE mini-GOP frame whose mini-GOP
/// ends in an IDR drops that last picture, because the IDR opens the next
/// group. A picture that is not `is_incomp_mg_frame` keeps it even when the
/// IDR is there.
///
/// Derivation 3, end of sequence: an I slice with `end_of_sequence_flag`
/// collapses the whole group to itself, `gf_interval = 1`, whatever the
/// mini-GOP held.
#[test]
fn traced_store_gf_group_shapes() {
    let mg = [
        pic(1, SliceType::B, 2),
        pic(2, SliceType::B, 1),
        pic(3, SliceType::B, 2),
        pic(4, SliceType::B, 0),
    ];

    let mut delayed = pic(0, SliceType::I, 0);
    delayed.is_delayed_intra = true;
    let out = gop::store_gf_group(&delayed, &mg).unwrap();
    assert_eq!(out.len(), 5);
    let centre = out.iter().find(|(pos, _)| *pos == 0).unwrap();
    assert_eq!(centre.1.gf_interval, 5);
    assert_eq!(centre.1.gf_group[0], gop::GfGroupMember::Centre);
    assert_eq!(centre.1.gf_group[1], gop::GfGroupMember::MiniGop(0));

    // The IDR trim.
    let mut mg_idr = mg;
    mg_idr[3].idr_flag = true;
    let mut incomp = pic(9, SliceType::B, 0);
    incomp.is_incomp_mg_frame = true;
    let out = gop::store_gf_group(&incomp, &mg_idr).unwrap();
    assert_eq!(out.len(), 3, "the trailing IDR is dropped");

    // Not incomplete: the IDR stays.
    let out = gop::store_gf_group(&pic(9, SliceType::B, 0), &mg_idr).unwrap();
    assert_eq!(out.len(), 4);

    // End of sequence.
    let mut eos = pic(20, SliceType::I, 0);
    eos.end_of_sequence_flag = true;
    let out = gop::store_gf_group(&eos, &mg).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.gf_interval, 1);
    assert_eq!(out[0].1.gf_group, vec![gop::GfGroupMember::Centre]);
}

/// The per-member `gf_update_due` pass (`pd_process.c:4398-4406`), including
/// the MIXED SUBJECT that makes it surprising.
///
/// Derivation: C re-runs the start-of-group predicate on each member, but the
/// `!svt_aom_is_delayed_intra(pcs)` term reads the CENTRE picture's flag while
/// `temporal_layer_index` reads the MEMBER's. So when the centre is a delayed
/// intra, NO member qualifies through the base-layer arm — even the
/// base-layer member at the end of the mini-GOP — and only I slices and
/// incomplete-mini-GOP frames get `gf_update_due`.
#[test]
fn traced_gf_update_due_mixes_two_pictures() {
    let mg = [
        pic(1, SliceType::B, 2),
        pic(2, SliceType::B, 1),
        pic(3, SliceType::B, 2),
        pic(4, SliceType::B, 0), // a base-layer member
    ];

    // Centre is NOT a delayed intra: the base-layer member qualifies.
    let out = gop::store_gf_group(&pic(0, SliceType::I, 0), &mg).unwrap();
    let due: Vec<bool> = out.iter().map(|(_, a)| a.gf_update_due).collect();
    assert_eq!(due, vec![false, false, false, true]);

    // Centre IS a delayed intra: nothing qualifies through the base-layer arm,
    // and the centre itself only qualifies because it is an I slice.
    let mut delayed = pic(0, SliceType::I, 0);
    delayed.is_delayed_intra = true;
    let out = gop::store_gf_group(&delayed, &mg).unwrap();
    let due: Vec<bool> = out.iter().map(|(_, a)| a.gf_update_due).collect();
    assert_eq!(due, vec![true, false, false, false, false]);
}

/// The per-picture half of `get_pred_struct_for_all_frames`
/// (`pd_process.c:967-970`).
///
/// Derivation: an IDR takes the SEQUENCE's configured depth, everything else
/// takes the depth this mini-GOP was assigned. So a key frame restarts the
/// pyramid at full depth even inside a shortened GOP, which is the one thing
/// worth getting right here.
#[test]
fn traced_pred_struct_for_picture() {
    use svtav1_encoder::port_picstruct::PredStructure;
    assert_eq!(
        gop::pred_struct_for_picture(PredStructure::RandomAccess, 5, 2, true),
        (PredStructure::RandomAccess, 5)
    );
    assert_eq!(
        gop::pred_struct_for_picture(PredStructure::RandomAccess, 5, 2, false),
        (PredStructure::RandomAccess, 2)
    );
    assert_eq!(
        gop::pred_struct_for_picture(PredStructure::LowDelay, 0, 0, false),
        (PredStructure::LowDelay, 0)
    );
}

/// The two startup latches (`pd_process.c:975-988`).
///
/// Derivation: `enable_startup_mg` is armed by any IDR or CRA and cleared by
/// the NEXT picture, so it is true for exactly one picture at a time and only
/// when `startup_mg_size != 0`. `is_startup_gop` is set only by the IDR at
/// picture 0 and cleared by any later IDR or CRA, so it identifies the first
/// GOP of the stream and stays true through it.
#[test]
fn traced_startup_latches() {
    let mut s = gop::StartupState::default();

    s = gop::advance_startup_state(s, 4, true, false, 0);
    assert_eq!(
        s,
        gop::StartupState {
            enable_startup_mg: true,
            is_startup_gop: true
        }
    );

    s = gop::advance_startup_state(s, 4, false, false, 1);
    assert_eq!(
        s,
        gop::StartupState {
            enable_startup_mg: false,
            is_startup_gop: true
        },
        "the very next picture clears the mini-GOP latch, not the GOP one"
    );

    s = gop::advance_startup_state(s, 4, false, false, 2);
    assert!(!s.enable_startup_mg);
    assert!(s.is_startup_gop);

    // A later IDR re-arms the mini-GOP latch and ends the startup GOP.
    s = gop::advance_startup_state(s, 4, true, false, 64);
    assert_eq!(
        s,
        gop::StartupState {
            enable_startup_mg: true,
            is_startup_gop: false
        }
    );

    // A CRA does the same.
    let mut s2 = gop::StartupState {
        enable_startup_mg: false,
        is_startup_gop: true,
    };
    s2 = gop::advance_startup_state(s2, 4, false, true, 32);
    assert_eq!(
        s2,
        gop::StartupState {
            enable_startup_mg: true,
            is_startup_gop: false
        }
    );

    // startup_mg_size == 0 disables the first latch entirely.
    let s3 = gop::advance_startup_state(gop::StartupState::default(), 0, true, false, 0);
    assert_eq!(
        s3,
        gop::StartupState {
            enable_startup_mg: false,
            is_startup_gop: true
        }
    );
}

// ---------------------------------------------------------------------------
// resize.c size-scale reconciliation
// ---------------------------------------------------------------------------

/// `dimension_is_ok` (`resize.c:1906`) — the AV1 conformance floor.
///
/// Derivation: `resized * 8 >= orig * denom / 2`, with a TRUNCATING integer
/// divide on the right. At denominator 16 (half width) and an original width
/// of 1920, the floor is `1920 * 16 / 2 = 15360`, so a coded width of 1920
/// gives `1920 * 8 = 15360` — exactly at the limit and therefore legal, while
/// 1919 is not.
#[test]
fn traced_dimension_is_ok() {
    assert!(sr::dimension_is_ok(1920, 1920, 16));
    assert!(!sr::dimension_is_ok(1920, 1919, 16));
    assert!(
        sr::dimension_is_ok(1920, 960, 8),
        "no scaling is always legal"
    );
    // The truncating divide: at an odd product the floor rounds DOWN, so one
    // more coded pixel than the rational half is not required.
    assert!(
        sr::dimension_is_ok(3, 1, 5),
        "3 * 5 / 2 = 7, and 1 * 8 = 8 >= 7"
    );
}

/// `validate_size_scales` (`resize.c:1916`) — which knob it is allowed to turn.
///
/// Derivation: the early return fires when the dimensions are already legal.
/// Otherwise exactly one of four arms runs, selected by which of resize and
/// superres was configured RANDOM — a denominator the application asked for
/// explicitly is never silently altered, so the "neither is random" arm
/// returns false without changing anything.
#[test]
fn traced_validate_size_scales_arms() {
    // Already legal: nothing is touched.
    let mut rsz = sr::SuperresParams {
        encoding_width: 1920,
        encoding_height: 1080,
        superres_denom: 8,
    };
    let before = rsz;
    let mut resize_denom = 8u8;
    assert!(sr::validate_size_scales(
        sr::ResizeMode::None,
        sr::SuperresMode::None,
        1920,
        1080,
        &mut rsz,
        &mut resize_denom
    ));
    assert_eq!(rsz, before);

    // Illegal, and neither knob may move: refuse without changing the params.
    let mut rsz = sr::SuperresParams {
        encoding_width: 400,
        encoding_height: 1080,
        superres_denom: 16,
    };
    let before = rsz;
    let mut resize_denom = 8u8;
    assert!(!sr::validate_size_scales(
        sr::ResizeMode::Fixed,
        sr::SuperresMode::Fixed,
        1920,
        1080,
        &mut rsz,
        &mut resize_denom
    ));
    assert_eq!(
        rsz, before,
        "a fixed configuration is never silently altered"
    );

    // Illegal with a RANDOM superres: the superres denominator is recomputed
    // from the resize one, and the result must be legal.
    let mut rsz = sr::SuperresParams {
        encoding_width: 400,
        encoding_height: 1080,
        superres_denom: 16,
    };
    let mut resize_denom = 8u8;
    let ok = sr::validate_size_scales(
        sr::ResizeMode::Fixed,
        sr::SuperresMode::Random,
        1920,
        1080,
        &mut rsz,
        &mut resize_denom,
    );
    assert_ne!(rsz.superres_denom, 16, "the random knob was turned");
    assert_eq!(
        ok,
        sr::dimensions_are_ok(1920, &rsz),
        "the return value is the final verdict"
    );
}

/// `calculate_next_resize_scale` (`resize.c:1855`) — mode by mode, and the
/// refusal.
///
/// Derivation: FIXED splits key-frame vs not exactly like the superres FIXED
/// mode; DYNAMIC takes the rate control's pending denominator verbatim;
/// RANDOM_ACCESS delegates to the event's own nested mode, and a nested mode
/// C does not handle hits `svt_aom_assert_err(0, ...)` — which in a Release
/// build does NOT abort and silently returns the unscaled denominator. The
/// port returns `None` instead.
#[test]
fn traced_calculate_next_resize_scale() {
    let ev = sr::ResizeEvent {
        scale_mode: sr::ResizeMode::Fixed,
        scale_denom: 11,
        scale_kf_denom: 13,
    };
    let base = sr::ResizeConfig {
        mode: sr::ResizeMode::Fixed,
        denom: 10,
        kf_denom: 12,
        pending_denom: 15,
        event: ev,
    };
    let mut seed = 56789u32;

    assert_eq!(
        sr::calculate_next_resize_scale(
            &sr::ResizeConfig {
                mode: sr::ResizeMode::None,
                ..base
            },
            true,
            &mut seed
        ),
        Some(8)
    );
    assert_eq!(
        sr::calculate_next_resize_scale(&base, true, &mut seed),
        Some(12)
    );
    assert_eq!(
        sr::calculate_next_resize_scale(&base, false, &mut seed),
        Some(10)
    );
    assert_eq!(
        sr::calculate_next_resize_scale(
            &sr::ResizeConfig {
                mode: sr::ResizeMode::Dynamic,
                ..base
            },
            false,
            &mut seed
        ),
        Some(15)
    );

    let ra = sr::ResizeConfig {
        mode: sr::ResizeMode::RandomAccess,
        ..base
    };
    assert_eq!(
        sr::calculate_next_resize_scale(&ra, true, &mut seed),
        Some(13)
    );
    assert_eq!(
        sr::calculate_next_resize_scale(&ra, false, &mut seed),
        Some(11)
    );

    // RANDOM stays in range whichever level it is reached through.
    for mode in [sr::ResizeMode::Random, sr::ResizeMode::RandomAccess] {
        let c = sr::ResizeConfig {
            mode,
            event: sr::ResizeEvent {
                scale_mode: sr::ResizeMode::Random,
                ..ev
            },
            ..base
        };
        for _ in 0..32 {
            let d = sr::calculate_next_resize_scale(&c, false, &mut seed).unwrap();
            assert!((8..=16).contains(&d), "denominator {d} out of range");
        }
    }

    // A nested mode C does not handle.
    let bad = sr::ResizeConfig {
        mode: sr::ResizeMode::RandomAccess,
        event: sr::ResizeEvent {
            scale_mode: sr::ResizeMode::Dynamic,
            ..ev
        },
        ..base
    };
    assert_eq!(
        sr::calculate_next_resize_scale(&bad, false, &mut seed),
        None
    );
}
