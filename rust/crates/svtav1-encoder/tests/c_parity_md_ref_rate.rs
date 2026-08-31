//! Differential parity: the reference-signalling rate
//! (`svtav1-encoder/src/port_md/ref_frame_rate.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `estimate_ref_frame_type_bits` | rd_cost.c:643 |
//! | `svt_aom_collect_neighbors_ref_counts_new` | entropy_coding.c:1877 |
//! | `svt_aom_get_reference_mode_context_new` | entropy_coding.c:1833 |
//!
//! The first oracle reaches EVERY prediction-context function in the
//! family — the five vote helpers, `single_ref_p1`..`p6`, `comp_ref_p`
//! /`_p1`/`_p2`, `comp_bwdref_p`/`_p1`, `uni_comp_ref_p`/`_p1`/`_p2` and
//! `comp_reference_type_context` — because its only inputs beyond the
//! rate tables are the two neighbours, and each reference type takes a
//! different path through them. The tables are randomized per trial so a
//! wrong CONTEXT index (not just a wrong branch) shows up as a wrong
//! total.
//!
//! `estimate_ref_frames_num_bits` (the loop around this) is `static` and
//! is covered at tier 4 in the module's own tests.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::ref_frame_rate as rrf;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Every neighbour shape the context functions distinguish: unavailable,
/// intra, intrabc, single forward, single backward, bidirectional
/// compound, and both flavours of unidirectional compound.
fn neighbor_cases() -> Vec<Option<(i8, i8, bool)>> {
    vec![
        None,
        Some((0, -1, false)), // intra
        Some((0, -1, true)),  // intrabc (counts as inter)
        Some((1, -1, false)), // LAST
        Some((4, -1, false)), // GOLDEN
        Some((5, -1, false)), // BWDREF
        Some((7, -1, false)), // ALTREF
        Some((1, 5, false)),  // LAST + BWDREF  (bidir)
        Some((1, 2, false)),  // LAST + LAST2   (unidir fwd)
        Some((5, 7, false)),  // BWDREF + ALTREF (unidir bwd)
    ]
}

fn to_port(n: Option<(i8, i8, bool)>) -> Option<rrf::NeighborMi> {
    n.map(|(a, b, ibc)| rrf::NeighborMi {
        ref_frame: [a, b],
        use_intrabc: ibc,
    })
}

fn tables(rng: &mut Rng) -> (cmd::RefRateTables, rrf::RefFrameFacBits) {
    let mut port = rrf::RefFrameFacBits {
        comp_inter: [[0; 2]; rrf::COMP_INTER_CONTEXTS],
        comp_ref_type: [[0; 2]; rrf::COMP_REF_TYPE_CONTEXTS],
        uni_comp_ref: [[[0; 2]; 3]; rrf::REF_CONTEXTS],
        comp_ref: [[[0; 2]; 3]; rrf::REF_CONTEXTS],
        comp_bwd_ref: [[[0; 2]; 2]; rrf::REF_CONTEXTS],
        single_ref: [[[0; 2]; 6]; rrf::REF_CONTEXTS],
    };
    let mut comp_ref_type = Vec::new();
    for c in 0..rrf::COMP_REF_TYPE_CONTEXTS {
        for b in 0..2 {
            let v = rng.below(4096) as i32;
            port.comp_ref_type[c][b] = v;
            comp_ref_type.push(v);
        }
    }
    let mut uni_comp_ref = Vec::new();
    let mut comp_ref = Vec::new();
    let mut comp_bwd_ref = Vec::new();
    let mut single_ref = Vec::new();
    for c in 0..rrf::REF_CONTEXTS {
        for i in 0..3 {
            for b in 0..2 {
                let v = rng.below(4096) as i32;
                port.uni_comp_ref[c][i][b] = v;
                uni_comp_ref.push(v);
            }
        }
        for i in 0..3 {
            for b in 0..2 {
                let v = rng.below(4096) as i32;
                port.comp_ref[c][i][b] = v;
                comp_ref.push(v);
            }
        }
        for i in 0..2 {
            for b in 0..2 {
                let v = rng.below(4096) as i32;
                port.comp_bwd_ref[c][i][b] = v;
                comp_bwd_ref.push(v);
            }
        }
        for i in 0..6 {
            for b in 0..2 {
                let v = rng.below(4096) as i32;
                port.single_ref[c][i][b] = v;
                single_ref.push(v);
            }
        }
    }
    (
        cmd::RefRateTables {
            comp_ref_type,
            uni_comp_ref,
            comp_ref,
            comp_bwd_ref,
            single_ref,
        },
        port,
    )
}

/// Every reference type the encoder can name: the seven singles, and the
/// compound pairs C's `av1_set_ref_frame` decodes from ids 8..
fn ref_cases() -> Vec<(i32, [i8; 2], bool)> {
    let mut v: Vec<(i32, [i8; 2], bool)> = (1..=7).map(|r| (r, [r as i8, -1], false)).collect();
    // Bidirectional pairs: every (fwd, bwd) combination.
    for f in 1..=4i8 {
        for b in 5..=7i8 {
            v.push((0, [f, b], true));
        }
    }
    // Unidirectional pairs.
    for pair in [[1i8, 2], [1, 3], [1, 4], [5, 7]] {
        v.push((0, pair, true));
    }
    v
}

#[test]
fn estimate_ref_frame_type_bits_matches_c_over_every_neighbour_and_ref() {
    let mut rng = Rng(0x9EF1_2026_0831_0009);
    let mut checked = 0usize;
    let mut nonzero = 0usize;
    for trial in 0..8 {
        let (ct, pt) = tables(&mut rng);
        for above in neighbor_cases() {
            for left in neighbor_cases() {
                let counts = rrf::NeighborRefCounts::collect(to_port(above), to_port(left));
                for (single_id, rf, is_compound) in ref_cases() {
                    // C's `ref_frame_type` argument matters only for the
                    // single-ref path (it is re-decoded into rf); for the
                    // compound path the shim's decode gives rf directly,
                    // so pass the single id there and the pair's own
                    // encoding here.
                    let type_arg = if is_compound {
                        // av1_ref_frame_type for a (fwd, bwd) or unidir
                        // pair; the C shim re-derives rf from it.
                        svtav1_encoder::inter_mvp::av1_ref_frame_type(rf) as i32
                    } else {
                        single_id
                    };
                    let (c_bits, c_mode_ctx, c_counts) =
                        cmd::estimate_ref_frame_type_bits(above, left, type_arg, is_compound, &ct);
                    assert_eq!(
                        c_counts, counts.0,
                        "collect_neighbors_ref_counts: above={above:?} left={left:?}"
                    );
                    assert_eq!(
                        c_mode_ctx as usize,
                        rrf::reference_mode_context(to_port(above), to_port(left)),
                        "reference_mode_context: above={above:?} left={left:?}"
                    );
                    let p_bits = rrf::estimate_ref_frame_type_bits(
                        &counts,
                        to_port(above),
                        to_port(left),
                        rf,
                        is_compound,
                        &pt,
                    );
                    assert_eq!(
                        c_bits, p_bits,
                        "estimate_ref_frame_type_bits: trial {trial} above={above:?} \
                         left={left:?} rf={rf:?} compound={is_compound}"
                    );
                    if c_bits != 0 {
                        nonzero += 1;
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 8 * 10 * 10 * ref_cases().len());
    // Positive control: a port and an oracle that both returned 0 would
    // agree trivially.
    assert!(
        nonzero > checked * 9 / 10,
        "positive control: only {nonzero} of {checked} results were non-zero"
    );
}
