//! Differential parity: per-stage candidate counts
//! (`svtav1-encoder/src/port_md/nics.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `svt_aom_set_nics` | product_coding_loop.c:1358 |
//! | `set_md_stage_counts` | product_coding_loop.c:1394 |
//!
//! `set_md_stage_counts` carries no `svt_aom_` prefix and IS exported;
//! `svt_aom_inject_inter_candidates`, in the same C file, carries the
//! prefix and is `static`. Linkage came from `nm -g`, not the name.
//!
//! What this pins in particular: **`pic_type` is not always 0.**
//! `leaf_funnel::rate_tables::nic_counts` hardcodes the I-slice row of
//! `MD_STAGE_NICS`, so an inter frame gets I-slice counts today. The
//! `all_three_pic_types_differ` case below asserts the three rows really
//! do produce different counts — without it, a port that ignored
//! `pic_type` entirely would still pass every other case that happens to
//! use `pic_type == 0`.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::nics as rnic;

fn staging_mode(v: u8) -> rnic::MdStagingMode {
    match v {
        0 => rnic::MdStagingMode::Mode0,
        1 => rnic::MdStagingMode::Mode1,
        _ => rnic::MdStagingMode::Mode2,
    }
}

#[test]
fn set_nics_matches_c_exhaustively_over_the_reachable_grid() {
    let mut checked = 0usize;
    for pic_type in 0u8..3 {
        for s1 in [0u8, 1, 2, 4, 8, 12, 16, 20, 32] {
            for s2 in [0u8, 1, 8, 16, 32] {
                for s3 in [0u8, 1, 8, 16, 32] {
                    for qp in [0u32, 10, 20, 35, 45, 46, 47, 55, 63] {
                        for scale in [false, true] {
                            let c = cmd::set_nics((s1, s2, s3), pic_type, qp, scale);
                            let r = rnic::set_nics(
                                &rnic::NicScalingCtrls {
                                    stage1_scaling_num: u32::from(s1),
                                    stage2_scaling_num: u32::from(s2),
                                    stage3_scaling_num: u32::from(s3),
                                },
                                pic_type,
                                qp,
                                scale,
                            );
                            assert_eq!(
                                c.mds1, r.mds1,
                                "mds1: pic_type={pic_type} s=({s1},{s2},{s3}) qp={qp} scale={scale}"
                            );
                            assert_eq!(
                                c.mds2, r.mds2,
                                "mds2: pic_type={pic_type} s=({s1},{s2},{s3}) qp={qp} scale={scale}"
                            );
                            assert_eq!(
                                c.mds3, r.mds3,
                                "mds3: pic_type={pic_type} s=({s1},{s2},{s3}) qp={qp} scale={scale}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3 * 9 * 5 * 5 * 9 * 2);
}

/// Positive control on the axis this module exists for: the three
/// `pic_type` rows must produce DIFFERENT counts, or a port that ignored
/// `pic_type` would pass the sweep above by accident.
#[test]
fn all_three_pic_types_differ() {
    let scaling = (16u8, 16, 16);
    let a = cmd::set_nics(scaling, 0, 35, false);
    let b = cmd::set_nics(scaling, 1, 35, false);
    let c = cmd::set_nics(scaling, 2, 35, false);
    assert_ne!(a.mds1, b.mds1);
    assert_ne!(b.mds1, c.mds1);
    assert_eq!(a.mds1[0], 64);
    assert_eq!(b.mds1[0], 32);
    assert_eq!(c.mds1[0], 16);
}

#[test]
fn set_md_stage_counts_matches_c() {
    let mut checked = 0usize;
    let mut bypass1_seen = [false; 2];
    let mut bypass2_seen = [false; 2];
    for mode in 0u8..3 {
        for is_i in [false, true] {
            for is_highest in [false, true] {
                for (s1, s2, s3) in [(0u8, 0u8, 0u8), (16, 16, 16), (4, 8, 12), (32, 1, 1)] {
                    for qp in [0u32, 20, 35, 46, 63] {
                        for scale in [false, true] {
                            let (c, cb1, cb2) = cmd::set_md_stage_counts(
                                (s1, s2, s3),
                                mode,
                                is_i,
                                is_highest,
                                qp,
                                scale,
                            );
                            let r = rnic::set_md_stage_counts(
                                &rnic::NicScalingCtrls {
                                    stage1_scaling_num: u32::from(s1),
                                    stage2_scaling_num: u32::from(s2),
                                    stage3_scaling_num: u32::from(s3),
                                },
                                staging_mode(mode),
                                is_i,
                                is_highest,
                                qp,
                                scale,
                            );
                            assert_eq!(
                                c.mds1, r.counts.mds1,
                                "mds1: mode={mode} i={is_i} hi={is_highest} \
                                 s=({s1},{s2},{s3}) qp={qp} scale={scale}"
                            );
                            assert_eq!(c.mds2, r.counts.mds2);
                            assert_eq!(c.mds3, r.counts.mds3);
                            assert_eq!(cb1, r.bypass_md_stage_1, "bypass1: mode={mode}");
                            assert_eq!(cb2, r.bypass_md_stage_2, "bypass2: mode={mode}");
                            bypass1_seen[usize::from(cb1)] = true;
                            bypass2_seen[usize::from(cb2)] = true;
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3 * 2 * 2 * 4 * 5 * 2);
    // Positive control: both flags must take both values, or the
    // comparison of a constant against a constant proves nothing.
    assert_eq!(bypass1_seen, [true, true]);
    assert_eq!(bypass2_seen, [true, true]);
}
