use super::*;

fn inter(rf0: i8, rf1: i8) -> NeighborMi {
    NeighborMi {
        ref_frame: [rf0, rf1],
        use_intrabc: false,
    }
}
fn intra() -> NeighborMi {
    NeighborMi {
        ref_frame: [INTRA_FRAME, NONE_FRAME],
        use_intrabc: false,
    }
}

/// TIER 4 — an INTRA neighbour contributes NOTHING to the counts, and
/// an intrabc one increments slot 0, which no context reads.
#[test]
fn tier4_collect_neighbors_ref_counts_skips_intra() {
    let c = NeighborRefCounts::collect(Some(intra()), Some(intra()));
    assert_eq!(c.0, [0; TOTAL_REFS_PER_FRAME]);

    let c = NeighborRefCounts::collect(Some(inter(LAST_FRAME, NONE_FRAME)), None);
    assert_eq!(c.0[LAST_FRAME as usize], 1);

    // A compound neighbour counts BOTH references.
    let c = NeighborRefCounts::collect(Some(inter(LAST_FRAME, BWDREF_FRAME)), None);
    assert_eq!(c.0[LAST_FRAME as usize], 1);
    assert_eq!(c.0[BWDREF_FRAME as usize], 1);

    // An intrabc neighbour IS inter and lands in slot 0.
    let ibc = NeighborMi {
        ref_frame: [INTRA_FRAME, NONE_FRAME],
        use_intrabc: true,
    };
    let c = NeighborRefCounts::collect(Some(ibc), None);
    assert_eq!(c.0[0], 1);
    assert_eq!(c.0[1..], [0; 7]);
}

/// TIER 4 — the shared three-way vote.
#[test]
fn tier4_vote_is_equal_then_less_then_greater() {
    assert_eq!(vote(0, 0), 1);
    assert_eq!(vote(0, 1), 0);
    assert_eq!(vote(1, 0), 2);
}

/// TIER 4 — `uni_comp_ref_p1` is NOT `comp_ref_p1`: one votes LAST2
/// against LAST3+GOLDEN, the other LAST against LAST2.
#[test]
fn tier4_uni_comp_ref_p1_differs_from_comp_ref_p1() {
    let mut c = NeighborRefCounts::default();
    c.0[LAST_FRAME as usize] = 2;
    c.0[LAST2_FRAME as usize] = 0;
    c.0[LAST3_FRAME as usize] = 1;
    // comp_ref_p1: LAST(2) vs LAST2(0) -> 2.
    assert_eq!(comp_ref_p1(&c), 2);
    // uni_comp_ref_p1: LAST2(0) vs LAST3+GOLDEN(1) -> 0.
    assert_eq!(uni_comp_ref_p1(&c), 0);
}

/// TIER 4 — an INTRA neighbour in the "one of two edges uses comp
/// pred" arm lands in context 3, not 2.
#[test]
fn tier4_reference_mode_context_intra_neighbour() {
    // Above intra (single ref), left compound.
    let ctx = reference_mode_context(Some(intra()), Some(inter(LAST_FRAME, BWDREF_FRAME)));
    assert_eq!(ctx, 3);
    // Above a FORWARD single ref, left compound -> 2.
    let ctx = reference_mode_context(
        Some(inter(LAST_FRAME, NONE_FRAME)),
        Some(inter(LAST_FRAME, BWDREF_FRAME)),
    );
    assert_eq!(ctx, 2);
    // Both compound -> 4.
    assert_eq!(
        reference_mode_context(
            Some(inter(LAST_FRAME, BWDREF_FRAME)),
            Some(inter(LAST_FRAME, ALTREF_FRAME))
        ),
        4
    );
    // No edges -> 1.
    assert_eq!(reference_mode_context(None, None), 1);
    // One edge, single forward -> 0; single backward -> 1.
    assert_eq!(
        reference_mode_context(Some(inter(LAST_FRAME, NONE_FRAME)), None),
        0
    );
    assert_eq!(
        reference_mode_context(Some(inter(BWDREF_FRAME, NONE_FRAME)), None),
        1
    );
}

/// TIER 4 — `has_uni_comp_refs` is a same-side test, so
/// (LAST, LAST2) is unidirectional and (LAST, BWDREF) is not.
#[test]
fn tier4_has_uni_comp_refs() {
    assert!(inter(LAST_FRAME, LAST2_FRAME).has_uni_comp_refs());
    assert!(inter(BWDREF_FRAME, ALTREF_FRAME).has_uni_comp_refs());
    assert!(!inter(LAST_FRAME, BWDREF_FRAME).has_uni_comp_refs());
    // A single reference is never unidirectional-compound.
    assert!(!inter(LAST_FRAME, NONE_FRAME).has_uni_comp_refs());
    // ref_frame[1] == INTRA_FRAME also means "no second ref".
    assert!(!inter(LAST_FRAME, INTRA_FRAME).has_second_ref());
}

fn ones() -> RefFrameFacBits {
    RefFrameFacBits {
        comp_inter: [[1i32; 2]; COMP_INTER_CONTEXTS],
        comp_ref_type: [[1i32; 2]; COMP_REF_TYPE_CONTEXTS],
        uni_comp_ref: [[[1i32; 2]; 3]; REF_CONTEXTS],
        comp_ref: [[[1i32; 2]; 3]; REF_CONTEXTS],
        comp_bwd_ref: [[[1i32; 2]; 2]; REF_CONTEXTS],
        single_ref: [[[1i32; 2]; 6]; REF_CONTEXTS],
    }
}

/// TIER 4 — a unidirectional compound pair pays strictly FEWER
/// symbols than a bidirectional one, because C returns early.
#[test]
fn tier4_unidir_compound_returns_before_the_bwdref_bits() {
    let t = ones();
    let c = NeighborRefCounts::default();
    // (LAST, LAST2): type + uni_comp_ref[0] + uni_comp_ref[1] = 3.
    let uni = estimate_ref_frame_type_bits(&c, None, None, [LAST_FRAME, LAST2_FRAME], true, &t);
    assert_eq!(uni, 3);
    // (LAST, BWDREF): type + comp_ref[0] + comp_ref[1] +
    // comp_bwd_ref[0] + comp_bwd_ref[1] = 5.
    let bi = estimate_ref_frame_type_bits(&c, None, None, [LAST_FRAME, BWDREF_FRAME], true, &t);
    assert_eq!(bi, 5);
    // (BWDREF, ALTREF) is unidirectional too, and takes the
    // `bit == 1` arm, so it stops after ONE uni_comp_ref symbol.
    let uni_bwd =
        estimate_ref_frame_type_bits(&c, None, None, [BWDREF_FRAME, ALTREF_FRAME], true, &t);
    assert_eq!(uni_bwd, 2);
}

/// TIER 4 — every single reference costs two or three symbols, and
/// exactly which ones depends on the reference.
#[test]
fn tier4_single_ref_symbol_counts() {
    let t = ones();
    let c = NeighborRefCounts::default();
    let n = |rf: i8| estimate_ref_frame_type_bits(&c, None, None, [rf, NONE_FRAME], false, &t);
    // LAST: p1 + p3 + p4 = 3.
    assert_eq!(n(LAST_FRAME), 3);
    assert_eq!(n(LAST2_FRAME), 3);
    assert_eq!(n(LAST3_FRAME), 3);
    assert_eq!(n(GOLDEN_FRAME), 3);
    // BWDREF: p1 + p2 + p6 = 3.
    assert_eq!(n(BWDREF_FRAME), 3);
    assert_eq!(n(ALTREF2_FRAME), 3);
    // ALTREF: p1 + p2 = 2 (bit1 == 1 skips p6).
    assert_eq!(n(ALTREF_FRAME), 2);
}

/// TIER 4 — the comp_inter term is gated on BOTH the frame-header
/// mode and `MIN(bwidth, bheight) >= 8`.
#[test]
fn tier4_estimate_ref_frames_num_bits_comp_inter_gate() {
    let t = ones();
    let c = NeighborRefCounts::default();
    let decode = |p: i8| {
        if p < 8 {
            [p, NONE_FRAME]
        } else {
            [LAST_FRAME, BWDREF_FRAME]
        }
    };

    let with =
        estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, true, 16, 16, &t, decode);
    let without =
        estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, false, 16, 16, &t, decode);
    assert_eq!(with[0].1, without[0].1 + 1);

    // A 4-wide block loses the term even under REFERENCE_MODE_SELECT.
    let narrow =
        estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, true, 4, 16, &t, decode);
    assert_eq!(narrow[0].1, without[0].1);

    // The result is keyed by reference TYPE: a single ref by rf[0],
    // a compound pair by the pair id.
    let mixed =
        estimate_ref_frames_num_bits(&[LAST_FRAME, 8], &c, None, None, false, 16, 16, &t, decode);
    assert_eq!(mixed[0].0, LAST_FRAME);
    assert_eq!(mixed[1].0, 8);
}
