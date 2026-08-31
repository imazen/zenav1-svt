//! Differential tests for `port_picstruct` against the REAL exported C
//! symbols in `Codec/pd_process.c` — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Five of the module's functions have an exported symbol in
//! `Bin/Release/libSvtAv1Enc.a` (verified with `nm -g`, not from headers) and
//! all five are gated here exhaustively or near-exhaustively:
//!
//! | Rust | C symbol |
//! |---|---|
//! | `is_pic_used_as_ref` | `svt_aom_is_pic_used_as_ref` |
//! | `get_gm_needed_resolutions` | `svt_aom_get_gm_needed_resolutions` |
//! | `is_incomp_mg_frame` | `svt_aom_is_incomp_mg_frame` |
//! | `update_count_try` | `update_count_try` |
//! | `setup_skip_mode_allowed` | `svt_av1_setup_skip_mode_allowed` |
//!
//! The remaining `port_picstruct` functions are `static` in C with no
//! exported symbol; they are gated at tier 4 in `port_picstruct_traced.rs`.

use svtav1_cref::picstruct as cref;
use svtav1_encoder::port_picstruct as pp;

/// `svt_aom_is_pic_used_as_ref` over its ENTIRE input domain.
///
/// `hierarchical_levels` is swept past the 0..=5 the C switch names so the
/// `default:` arm (which asserts under a debug build and returns true under
/// the Release oracle) is covered too.
#[test]
fn c_parity_is_pic_used_as_ref_exhaustive() {
    let mut checked = 0usize;
    for hier in 0u32..=7 {
        for tl in 0u32..=7 {
            for pic_idx in 0u32..=16 {
                for scheme in 0u32..=2 {
                    for overlay in [false, true] {
                        let got = pp::is_pic_used_as_ref(hier, tl, pic_idx, scheme, overlay);
                        let want = cref::is_pic_used_as_ref(hier, tl, pic_idx, scheme, overlay);
                        assert_eq!(
                            got, want,
                            "is_pic_used_as_ref(hier={hier}, tl={tl}, idx={pic_idx}, \
                             scheme={scheme}, overlay={overlay})"
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    // Positive control: the probe must actually vary, or an all-`true` port
    // would pass an all-`true` oracle (WORKING-ON-THIS.md §5).
    assert_eq!(checked, 8 * 8 * 17 * 3 * 2);
    assert!(pp::is_pic_used_as_ref(0, 0, 0, 0, false));
    assert!(!pp::is_pic_used_as_ref(5, 5, 0, 1, false));
}

/// `svt_aom_get_gm_needed_resolutions` over every downsample level, plus
/// out-of-range levels (all three outputs false).
#[test]
fn c_parity_get_gm_needed_resolutions_exhaustive() {
    for ds in 0u8..=8 {
        assert_eq!(
            pp::get_gm_needed_resolutions(ds),
            cref::get_gm_needed_resolutions(ds),
            "get_gm_needed_resolutions(ds_lvl={ds})"
        );
    }
    // Positive control: the three flags are not all-constant across the sweep.
    assert_eq!(pp::get_gm_needed_resolutions(0), (true, false, false));
    assert_eq!(pp::get_gm_needed_resolutions(2), (false, false, true));
}

/// `svt_aom_is_incomp_mg_frame` over every (picture pred type, sequence pred
/// structure) pair.
#[test]
fn c_parity_is_incomp_mg_frame_exhaustive() {
    use pp::PredStructure::{AllIntra, LowDelay, RandomAccess};
    for pic_ps in [AllIntra, LowDelay, RandomAccess] {
        for seq_ps in [AllIntra, LowDelay, RandomAccess] {
            let pic = pp::PicParams {
                pred_struct_type: pic_ps,
                ..Default::default()
            };
            let seq = pp::SeqPicParams {
                pred_structure: seq_ps,
                ..Default::default()
            };
            let got = pp::is_incomp_mg_frame(&pic, &seq);
            let want = cref::is_incomp_mg_frame(pic_ps as u8, seq_ps as u8);
            assert_eq!(
                got, want,
                "is_incomp_mg_frame(pic={pic_ps:?}, seq={seq_ps:?})"
            );
        }
    }
    // Positive control: exactly one of the nine pairs is true.
    let pic = pp::PicParams {
        pred_struct_type: LowDelay,
        ..Default::default()
    };
    let seq = pp::SeqPicParams {
        pred_structure: RandomAccess,
        ..Default::default()
    };
    assert!(pp::is_incomp_mg_frame(&pic, &seq));
}

/// `update_count_try` over the frame types, update types and count/cap grid.
///
/// `frame_is_boosted` is `frame_is_intra_only || ARF || GF`, so BOTH the frame
/// type and the update type are swept — a port that keyed the base-vs-non-base
/// cap off `temporal_layer == 0` (the natural wrong guess) fails here.
#[test]
fn c_parity_update_count_try_grid() {
    // FrameType: KEY_FRAME=0, INTER_FRAME=1, INTRA_ONLY_FRAME=2, S_FRAME=3.
    const FRAME_TYPES: [(u8, bool); 4] = [(0, true), (1, false), (2, true), (3, false)];
    // SvtAv1FrameUpdateType 0..=6.
    const UPDATE_TYPES: [(u8, pp::FrameUpdateType); 7] = [
        (0, pp::FrameUpdateType::Kf),
        (1, pp::FrameUpdateType::Lf),
        (2, pp::FrameUpdateType::Gf),
        (3, pp::FrameUpdateType::Arf),
        (4, pp::FrameUpdateType::Overlay),
        (5, pp::FrameUpdateType::IntnlOverlay),
        (6, pp::FrameUpdateType::IntnlArf),
    ];
    let mut saw_base_cap = false;
    let mut saw_nonbase_cap = false;
    for (ft, intra_only) in FRAME_TYPES {
        for (ut, rust_ut) in UPDATE_TYPES {
            for l0 in 0u8..=4 {
                for l1 in 0u8..=3 {
                    for (b0, b1, n0, n1) in [(4u8, 3u8, 2u8, 1u8), (1, 1, 4, 3), (3, 2, 3, 2)] {
                        let mut pic = pp::PicParams {
                            is_intra_only: intra_only,
                            update_type: rust_ut,
                            ref_list0_count: l0,
                            ref_list1_count: l1,
                            ..Default::default()
                        };
                        let seq = pp::SeqPicParams {
                            mrp_ctrls: pp::MrpCtrls {
                                base_ref_list0_count: b0,
                                base_ref_list1_count: b1,
                                non_base_ref_list0_count: n0,
                                non_base_ref_list1_count: n1,
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        pp::update_count_try(&mut pic, &seq);
                        let want = cref::update_count_try(ft, ut, l0, l1, b0, b1, n0, n1);
                        assert_eq!(
                            (pic.ref_list0_count_try, pic.ref_list1_count_try),
                            want,
                            "update_count_try(ft={ft}, ut={ut}, l0={l0}, l1={l1}, \
                             caps=({b0},{b1},{n0},{n1}))"
                        );
                        if pp::frame_is_boosted(&pic) {
                            saw_base_cap = true;
                        } else {
                            saw_nonbase_cap = true;
                        }
                    }
                }
            }
        }
    }
    // Positive control: both arms of frame_is_boosted were exercised.
    assert!(saw_base_cap && saw_nonbase_cap);
}

/// `svt_av1_setup_skip_mode_allowed` over a hint grid that reaches all three
/// C outcomes: not allowed, bi-directional, and forward-only (second-nearest).
#[test]
fn c_parity_setup_skip_mode_allowed_grid() {
    let hint_sets: [[u32; 7]; 8] = [
        // Pure low-delay P: all references in the past, strictly decreasing.
        [9, 8, 7, 6, 9, 9, 9],
        // Bi-directional: LAST..GOLD past, BWD..ALT future.
        [8, 7, 6, 5, 12, 14, 16],
        // All references equal to the current hint (no forward, no backward).
        [10, 10, 10, 10, 10, 10, 10],
        // Forward-only with a single distinct hint (no second-nearest).
        [9, 9, 9, 9, 9, 9, 9],
        // Forward-only with two distinct hints (second-nearest exists).
        [9, 5, 9, 9, 9, 9, 9],
        // Wrap-around: hints straddle the order-hint modulus.
        [126, 127, 0, 1, 2, 3, 4],
        // Backward-only.
        [12, 13, 14, 15, 16, 17, 18],
        // Mixed with duplicates on both sides.
        [9, 9, 6, 6, 12, 12, 20],
    ];
    let mut saw_allowed = false;
    let mut saw_disallowed = false;
    for bits in [4u8, 7, 8] {
        for hints in hint_sets {
            for cur in [0u32, 1, 10, 100] {
                for slice_type in [0u8, 1] {
                    for ref_mode in [0u8, 2] {
                        for enable in [true, false] {
                            let mut pic = pp::PicParams {
                                slice_type: if slice_type == 0 {
                                    pp::SliceType::B
                                } else {
                                    pp::SliceType::I
                                },
                                reference_mode: if ref_mode == 0 {
                                    pp::ReferenceMode::Single
                                } else {
                                    pp::ReferenceMode::Select
                                },
                                ref_order_hint: hints,
                                cur_order_hint: cur,
                                ..Default::default()
                            };
                            let seq = pp::SeqPicParams {
                                order_hint_info: svtav1_encoder::inter_mvp::OrderHintInfo {
                                    enable_order_hint: enable,
                                    order_hint_bits: u32::from(bits),
                                },
                                ..Default::default()
                            };
                            pp::setup_skip_mode_allowed(&mut pic, &seq);
                            let want = cref::setup_skip_mode_allowed(
                                enable, bits, slice_type, ref_mode, &hints, cur,
                            );
                            assert_eq!(
                                (
                                    pic.skip_mode.skip_mode_allowed,
                                    pic.skip_mode.ref_frame_idx_0,
                                    pic.skip_mode.ref_frame_idx_1
                                ),
                                want,
                                "setup_skip_mode_allowed(bits={bits}, hints={hints:?}, \
                                 cur={cur}, slice={slice_type}, mode={ref_mode}, \
                                 enable={enable})"
                            );
                            if pic.skip_mode.skip_mode_allowed != 0 {
                                saw_allowed = true;
                            } else {
                                saw_disallowed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    // Positive control: the grid reaches BOTH outcomes. Without this an
    // always-return-0 port would pass against an always-0 probe.
    assert!(saw_allowed, "grid never produced skip_mode_allowed = 1");
    assert!(saw_disallowed);
}

/// `svt_aom_get_mini_gop_stats` over ALL 31 entries of `mini_gop_stats_array`.
///
/// The table is transcribed into the port, so this is the differential that
/// makes the transcription evidence rather than a claim.
#[test]
fn c_parity_get_mini_gop_stats_all_31_entries() {
    for i in 0..31u32 {
        let r = pp::get_mini_gop_stats(i as usize);
        let want = cref::get_mini_gop_stats(i);
        assert_eq!(
            (r.hierarchical_levels, r.start_index, r.end_index, r.length),
            want,
            "mini_gop_stats_array[{i}]"
        );
    }
    // Positive control: the table is not a constant row.
    assert_ne!(cref::get_mini_gop_stats(0), cref::get_mini_gop_stats(30));
}

/// `is_pic_cutting_short_ra_mg` over the whole predicate lattice.
///
/// The C condition is
/// `(length < entry_count || idr_count > 0) && pred_type == RANDOM_ACCESS &&
/// !idr_flag && !cra_flag`, so the grid sweeps both disjuncts and all three
/// conjuncts.
#[test]
fn c_parity_is_pic_cutting_short_ra_mg_lattice() {
    let mut saw_true = false;
    let mut saw_false = false;
    for mg_len in [0u32, 1, 4, 8] {
        for idr_count in [0u32, 1] {
            for entry_count in [1u32, 4, 8] {
                for pred_type in [0u8, 1, 2] {
                    for idr in [false, true] {
                        for cra in [false, true] {
                            let map = pp::MiniGopMap {
                                length: {
                                    let mut a = [0u32; pp::MINI_GOP_MAX_COUNT];
                                    a[0] = mg_len;
                                    a
                                },
                                idr_count: {
                                    let mut a = [0u32; pp::MINI_GOP_MAX_COUNT];
                                    a[0] = idr_count;
                                    a
                                },
                                ..Default::default()
                            };
                            let pic = pp::PicParams {
                                pred_struct_entry_count: entry_count,
                                pred_struct_type: match pred_type {
                                    0 => pp::PredStructure::AllIntra,
                                    1 => pp::PredStructure::LowDelay,
                                    _ => pp::PredStructure::RandomAccess,
                                },
                                ..Default::default()
                            };
                            let got = pp::is_pic_cutting_short_ra_mg(&map, &pic, 0, idr, cra);
                            let want = cref::is_pic_cutting_short_ra_mg(
                                mg_len,
                                idr_count,
                                entry_count,
                                pred_type,
                                idr,
                                cra,
                            );
                            assert_eq!(
                                got, want,
                                "is_pic_cutting_short_ra_mg(len={mg_len}, idr_count={idr_count}, \
                                 entry={entry_count}, pred={pred_type}, idr={idr}, cra={cra})"
                            );
                            if got {
                                saw_true = true;
                            } else {
                                saw_false = true;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(saw_true && saw_false, "the grid must reach both verdicts");
}

/// `svt_aom_is_delayed_intra` over the whole predicate lattice.
#[test]
fn c_parity_is_delayed_intra_lattice() {
    let mut saw_true = false;
    let mut saw_false = false;
    for idr in [false, true] {
        for cra in [false, true] {
            for pred in [0u8, 1, 2] {
                for period in [-1i32, 0, 1, 32] {
                    for eos in [false, true] {
                        for pab in [0u32, 1, 4, 8] {
                            for entry in [1u32, 4, 8] {
                                let got = pp::is_delayed_intra(
                                    idr,
                                    cra,
                                    match pred {
                                        0 => pp::PredStructure::AllIntra,
                                        1 => pp::PredStructure::LowDelay,
                                        _ => pp::PredStructure::RandomAccess,
                                    },
                                    period,
                                    eos,
                                    pab,
                                    entry,
                                );
                                let want =
                                    cref::is_delayed_intra(idr, cra, pred, period, eos, pab, entry);
                                assert_eq!(
                                    got, want,
                                    "is_delayed_intra(idr={idr}, cra={cra}, pred={pred}, \
                                     period={period}, eos={eos}, pab={pab}, entry={entry})"
                                );
                                if got {
                                    saw_true = true;
                                } else {
                                    saw_false = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(saw_true && saw_false, "the grid must reach both verdicts");
}

/// `search_this_pic` — found, not found, first-match-wins and the empty buffer.
#[test]
fn c_parity_search_this_pic() {
    let buffers: [&[u64]; 5] = [
        &[],
        &[7],
        &[3, 1, 4, 1, 5, 9, 2, 6],
        &[0, 0, 0, 0],
        &[100, 99, 98, 97, 96],
    ];
    let mut saw_hit = false;
    let mut saw_miss = false;
    for buf in buffers {
        for probe in 0u64..12 {
            let got = pp::search_this_pic(buf, probe);
            let want = cref::search_this_pic(buf, probe);
            assert_eq!(got, want, "search_this_pic({buf:?}, {probe})");
            if got >= 0 {
                saw_hit = true;
            } else {
                saw_miss = true;
            }
        }
        // A probe that is definitely present when the buffer is non-empty.
        if let Some(&first) = buf.first() {
            assert_eq!(
                pp::search_this_pic(buf, first),
                cref::search_this_pic(buf, first)
            );
        }
    }
    assert!(saw_hit && saw_miss);
    // First match wins on a duplicate (buffer [3,1,4,1,...]: probe 1 -> 1).
    assert_eq!(pp::search_this_pic(&[3, 1, 4, 1, 5], 1), 1);
}
