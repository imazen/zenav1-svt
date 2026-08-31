//! Differential tests for the TPL-group SELECTION against the REAL exported C
//! symbols `validate_pic_for_tpl` and `store_extended_group` — evidence
//! **tier 1** (`docs/WORKING-ON-THIS.md` §4).
//!
//! Why these two get their own file: TPL group MEMBERSHIP sets `r0` and the
//! per-SB qindex offsets in random access, so a different membership is a
//! different qindex on every superblock. They were ported at tier 4 first
//! (`port_picstruct_traced.rs`); these gates upgrade them.
//!
//! The shim re-declares `LadQueue` / `LadQueueEntry` /
//! `InitialRateControlContext`, which `initial_rc_process.c` defines in the .c
//! file rather than a header. That is a layout dependency and it is called out
//! in the shim; the shapes are trivial (a dctor plus a pointer, a buffer plus
//! three counters).

use svtav1_cref::picstruct as cref;
use svtav1_encoder::port_picstruct as pp;

/// `validate_pic_for_tpl` over the de-duplication, the `reduced_tpl_group`
/// cutoff and the `svt_aom_is_pic_skipped` gate.
///
/// `is_pic_skipped` is `!is_ref && rc_stat_gen_pass_mode && !first_frame_in_minigop`
/// (`pd_process.c`), so all three of its inputs are swept.
#[test]
fn c_parity_validate_pic_for_tpl() {
    let groups: [(&[u64], &[u8]); 5] = [
        (&[10, 11, 12, 13], &[0, 1, 2, 3]),
        (&[10, 11, 10, 13], &[0, 1, 2, 3]),
        (&[10, 10, 10, 10], &[0, 0, 0, 0]),
        (&[10, 11, 12, 11], &[0, 3, 1, 3]),
        (&[7], &[0]),
    ];
    let mut saw_valid = false;
    let mut saw_invalid = false;
    for (pocs, layers) in groups {
        for reduced in [-1i8, 0, 1, 2, 3] {
            for rc_stat in [0u8, 1] {
                for first_in_mg in [0u8, 1] {
                    for is_ref_all in [0u8, 1] {
                        let is_ref: Vec<u8> = vec![is_ref_all; pocs.len()];
                        for idx in 0..pocs.len() {
                            // The port's helper takes the accumulated prefix,
                            // which is what C's `pcs->tpl_group[0..pic_index]`
                            // holds at that point.
                            let skipped = is_ref_all == 0 && rc_stat == 1 && first_in_mg == 0;
                            let got = pp::validate_pic_for_tpl(pocs, layers, idx, reduced, skipped);
                            let (want_valid, _) = cref::validate_pic_for_tpl(
                                pocs,
                                layers,
                                &is_ref,
                                idx as u32,
                                reduced,
                                rc_stat,
                                first_in_mg,
                            );
                            assert_eq!(
                                u8::from(got),
                                want_valid,
                                "validate_pic_for_tpl(pocs={pocs:?}, layers={layers:?}, \
                                 idx={idx}, reduced={reduced}, rc_stat={rc_stat}, \
                                 first_in_mg={first_in_mg}, is_ref={is_ref_all})"
                            );
                            if got {
                                saw_valid = true;
                            } else {
                                saw_invalid = true;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        saw_valid && saw_invalid,
        "the sweep must reach both verdicts"
    );
}

/// `store_extended_group`'s membership selection.
///
/// The grid is built to reach every arm of the C loop: a delayed intra at
/// index 0 and at a later index, a non-delayed intra at both, a group that
/// runs past `limited_tpl_group_size`, and an `ext_mg_id` change after a
/// GOP-closing intra.
#[test]
fn c_parity_store_extended_group_membership() {
    fn pic(
        poc: u64,
        i_slice: bool,
        layer: u8,
        mg: i64,
        idr: bool,
        cra: bool,
        pred_ra: bool,
        intra_period: i32,
        entry_count: u32,
        pab: u32,
    ) -> cref::TplPicDesc {
        cref::TplPicDesc {
            picture_number: poc,
            ext_mg_id: mg,
            slice_type: u8::from(i_slice),
            temporal_layer_index: layer,
            hierarchical_levels: 3,
            idr_flag: u8::from(idr),
            cra_flag: u8::from(cra),
            pred_structure: if pred_ra { 2 } else { 1 },
            end_of_sequence_flag: 0,
            is_ref: 1,
            first_frame_in_minigop: 1,
            tpl_params_ready: 0,
            pre_assignment_buffer_count: pab,
            pred_struct_entry_count: entry_count,
            intra_period_length: intra_period,
        }
    }

    // Cases, each a whole extended group.
    let cases: Vec<Vec<cref::TplPicDesc>> = vec![
        // 1. All inter, longer than any limited size.
        (0..12)
            .map(|i| pic(i, false, 1, 0, false, false, true, 32, 8, 8))
            .collect(),
        // 2. A DELAYED intra at index 3 (RA + idr + intra_period != 0 +
        //    !end_of_sequence -> delayed): the loop must BREAK before it.
        (0..8)
            .map(|i| {
                if i == 3 {
                    pic(i, true, 0, 1, true, false, true, 32, 8, 8)
                } else {
                    pic(i, false, 1, 0, false, false, true, 32, 8, 8)
                }
            })
            .collect(),
        // 3. A NON-delayed intra at index 3 (intra_period 0 -> not delayed):
        //    it is ADDED and closes the GOP; only same-ext_mg_id members follow.
        (0..8)
            .map(|i| {
                if i == 3 {
                    pic(i, true, 0, 1, true, false, true, 0, 8, 8)
                } else if i > 3 {
                    pic(
                        i,
                        false,
                        1,
                        if i < 6 { 1 } else { 2 },
                        false,
                        false,
                        true,
                        32,
                        8,
                        8,
                    )
                } else {
                    pic(i, false, 1, 0, false, false, true, 32, 8, 8)
                }
            })
            .collect(),
        // 4. A delayed intra at index 0: ADDED, and the loop continues.
        (0..8)
            .map(|i| {
                if i == 0 {
                    pic(i, true, 0, 0, true, false, true, 32, 8, 8)
                } else {
                    pic(i, false, 1, 0, false, false, true, 32, 8, 8)
                }
            })
            .collect(),
        // 5. A single picture.
        vec![pic(0, false, 0, 0, false, false, true, 32, 8, 8)],
    ];

    let mut saw_break = false;
    let mut saw_full = false;
    for (case_i, members) in cases.iter().enumerate() {
        for centre_slice in [0u8, 1] {
            for hier in [0u8, 2, 3] {
                for lad in [0u8, 1, 2] {
                    for reduced in [-1i8, 0, 2] {
                        let want = cref::store_extended_group(
                            members,
                            centre_slice,
                            hier,
                            lad,
                            0, // startup_mg_size 0: the override arm is off
                            hier,
                            0,
                            0,
                            reduced,
                            5,
                            0,
                            i64::MAX,
                        );

                        // Port side: the caller precomputes is_delayed_intra
                        // and is_pic_skipped, exactly as the module documents.
                        let ext: Vec<pp::ExtGroupPic> = members
                            .iter()
                            .map(|m| pp::ExtGroupPic {
                                picture_number: m.picture_number,
                                slice_type: if m.slice_type == 0 {
                                    pp::SliceType::B
                                } else {
                                    pp::SliceType::I
                                },
                                temporal_layer_index: m.temporal_layer_index,
                                ext_mg_id: m.ext_mg_id,
                                is_delayed_intra: pp::is_delayed_intra(
                                    m.idr_flag != 0,
                                    m.cra_flag != 0,
                                    if m.pred_structure == 2 {
                                        pp::PredStructure::RandomAccess
                                    } else {
                                        pp::PredStructure::LowDelay
                                    },
                                    m.intra_period_length,
                                    m.end_of_sequence_flag != 0,
                                    m.pre_assignment_buffer_count,
                                    m.pred_struct_entry_count,
                                ),
                                is_skipped: false,
                            })
                            .collect();
                        let got = pp::store_extended_group(
                            &ext,
                            if centre_slice == 0 {
                                pp::SliceType::B
                            } else {
                                pp::SliceType::I
                            },
                            hier,
                            u32::from(lad),
                            reduced,
                        );

                        let ctx = format!(
                            "store_extended_group(case={case_i}, centre_slice={centre_slice}, \
                             hier={hier}, lad={lad}, reduced={reduced})"
                        );
                        let got_pocs: Vec<u64> = got
                            .members
                            .iter()
                            .map(|&i| members[i].picture_number)
                            .collect();
                        assert_eq!(got_pocs, want.group_pocs, "{ctx} membership");
                        assert_eq!(
                            u32::from(got.used_tpl_frame_num as u8),
                            u32::from(want.used_tpl_frame_num),
                            "{ctx} used_tpl_frame_num"
                        );
                        assert_eq!(
                            &got.valid[..members.len()],
                            &want.valid[..members.len()],
                            "{ctx} tpl_valid_pic"
                        );

                        if want.group_pocs.len() < members.len() {
                            saw_break = true;
                        } else {
                            saw_full = true;
                        }
                    }
                }
            }
        }
    }
    // Positive controls: the grid must reach BOTH a truncated group and a full
    // one, or a port that always returned the whole extended group would pass.
    assert!(saw_break, "the grid never truncated a group");
    assert!(saw_full, "the grid never selected the whole extended group");
}
