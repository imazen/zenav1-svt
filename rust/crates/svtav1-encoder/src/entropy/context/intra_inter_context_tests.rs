/// TIER 4 — the two transcriptions of
/// `svt_av1_get_intra_inter_context` must agree over the quadrant they
/// share (both neighbours available).
///
/// It exists because they did NOT: this module's table was inverted
/// (both-intra 0 / both-inter 3, where C says 3 / 0) and gave 2 for one
/// of the mixed cases, which is the value C reserves for a SINGLE
/// available intra neighbour. The writer used
/// `port_entropy_inter::intra_inter_context` and mode decision used this
/// one, so the encoder priced a context it did not code — 1207 rate
/// units per inter candidate, measured against C's own
/// `svt_aom_inter_fast_cost`. Pinning them is the counterpart of
/// `docs/WORKING-ON-THIS.md` §4: when you find a second transcription,
/// PIN IT to the first.
#[test]
fn agrees_with_the_oracle_transcription_when_both_neighbours_exist() {
    use crate::port_entropy_inter::{NeighborMi, Neighbors, intra_inter_context};
    // `ref_frame[0] > INTRA_FRAME(0)` is what `is_inter_block` reads.
    let mi = |inter: bool| NeighborMi {
        mode: if inter { 13 } else { 0 },
        ref_frame: if inter { [1, -1] } else { [0, -1] },
        interp_filters: 0,
        use_intrabc: false,
        skip_mode: false,
        skip: false,
        comp_group_idx: 0,
        compound_idx: 0,
        bsize: 3,
    };
    for above_inter in [false, true] {
        for left_inter in [false, true] {
            let nb = Neighbors {
                above: Some(mi(above_inter)),
                left: Some(mi(left_inter)),
                up_available: true,
                left_available: true,
            };
            assert_eq!(
                super::get_intra_inter_context(!above_inter, !left_inter),
                intra_inter_context(&nb),
                "above_inter={above_inter} left_inter={left_inter}",
            );
        }
    }
}

/// TIER 4 — C's own values, read out of `svt_av1_get_intra_inter_context`
/// (entropy_coding.c:1127): `left_intra && above_intra ? 3 : left_intra
/// || above_intra`. BOTH-INTER is 0 and BOTH-INTRA is 3, which is the
/// direction the old table had backwards.
#[test]
fn matches_c_for_both_available() {
    assert_eq!(super::get_intra_inter_context(true, true), 3);
    assert_eq!(super::get_intra_inter_context(true, false), 1);
    assert_eq!(super::get_intra_inter_context(false, true), 1);
    assert_eq!(super::get_intra_inter_context(false, false), 0);
}
