//! Differential parity: the mode-decision predicate layer
//! (`svtav1-encoder/src/port_md/predicates.rs`) vs the REAL exported C
//! functions in `Source/Lib/Codec/mode_decision.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) for the seven
//! exported oracles, driven through `shims/mode_decision_shims.c`:
//!
//! | oracle | C |
//! |---|---|
//! | `svt_get_ref_frame_type` | mode_decision.c:265 |
//! | `svt_aom_get_max_drl_index` | mode_decision.c:269 |
//! | `svt_is_interintra_allowed` | mode_decision.c:96 |
//! | `svt_aom_get_wedge_params_bits` | inter_prediction.c:2053 |
//! | `svt_aom_get_me_block_offset` | mode_decision.c:117 |
//! | `svt_aom_is_valid_unipred_ref` | mode_decision.c:762 |
//! | `svt_aom_is_me_data_present` | mode_decision.c:179 |
//! | `svt_aom_obmc_motion_mode_allowed` | mode_decision.c:214 |
//!
//! Linkage for each was re-checked with `nm -g libSvtAv1Enc.a`, not
//! inferred from the `svt_aom_` prefix.
//!
//! `is_valid_bipred_ref`, `check_mv_validity`, `is_valid_mv_diff`,
//! `mv_is_already_injected` and `warped_motion_mode_allowed` are `static`
//! in the C with no exported symbol; their tests live in the module's own
//! `#[cfg(test)]` block and are labelled **tier 4**.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::predicates as rmd;
use svtav1_types::motion::TransformationType;
use svtav1_types::prediction::PredictionMode;

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

/// C `BLOCK_SIZES_ALL`.
const BLOCK_SIZES_ALL: u8 = 22;

fn mode_from_u8(v: u8) -> PredictionMode {
    // SAFETY-free: an explicit table rather than a transmute.
    const MODES: [PredictionMode; 25] = [
        PredictionMode::DcPred,
        PredictionMode::VPred,
        PredictionMode::HPred,
        PredictionMode::D45Pred,
        PredictionMode::D135Pred,
        PredictionMode::D113Pred,
        PredictionMode::D157Pred,
        PredictionMode::D203Pred,
        PredictionMode::D67Pred,
        PredictionMode::SmoothPred,
        PredictionMode::SmoothVPred,
        PredictionMode::SmoothHPred,
        PredictionMode::PaethPred,
        PredictionMode::NearestMv,
        PredictionMode::NearMv,
        PredictionMode::GlobalMv,
        PredictionMode::NewMv,
        PredictionMode::NearestNearestMv,
        PredictionMode::NearNearMv,
        PredictionMode::NearestNewMv,
        PredictionMode::NewNearestMv,
        PredictionMode::NearNewMv,
        PredictionMode::NewNearMv,
        PredictionMode::GlobalGlobalMv,
        PredictionMode::NewNewMv,
    ];
    MODES[v as usize]
}

// ---------------------------------------------------------------------------
// svt_get_ref_frame_type (mode_decision.c:265) — EXHAUSTIVE
// ---------------------------------------------------------------------------

#[test]
fn ref_frame_type_table_matches_c_exhaustively() {
    for list in 0u8..2 {
        for ref_idx in 0u8..4 {
            let c = cmd::get_ref_frame_type(list, ref_idx);
            let r = rmd::get_ref_frame_type(list, ref_idx);
            assert_eq!(
                c, r,
                "svt_get_ref_frame_type(list={list}, ref_idx={ref_idx})"
            );
        }
    }
    // The [1][3] hole is INVALID_REF, not a reference type — the check
    // that the port did not silently narrow it to a valid ref.
    assert_eq!(rmd::get_ref_frame_type(1, 3), rmd::INVALID_REF);
}

// ---------------------------------------------------------------------------
// svt_aom_get_max_drl_index (mode_decision.c:269) — EXHAUSTIVE
// ---------------------------------------------------------------------------

#[test]
fn max_drl_index_matches_c_exhaustively() {
    for refmv_cnt in 0u8..=8 {
        for m in 0u8..25 {
            let mode = mode_from_u8(m);
            let c = cmd::get_max_drl_index(refmv_cnt, m);
            let r = rmd::get_max_drl_index(refmv_cnt, mode);
            assert_eq!(
                c, r,
                "svt_aom_get_max_drl_index(refmv_cnt={refmv_cnt}, mode={m})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// svt_is_interintra_allowed (mode_decision.c:96) — EXHAUSTIVE over
// (enable, bsize, mode) x a spread of ref pairs.
// ---------------------------------------------------------------------------

#[test]
fn interintra_allowed_matches_c_exhaustively() {
    let ref_pairs: [[i8; 2]; 8] = [
        [0, -1],
        [1, -1],
        [7, -1],
        [1, 0],
        [1, 1],
        [1, 7],
        [-1, -1],
        [0, 0],
    ];
    for enable in [0u8, 1] {
        for bsize in 0..BLOCK_SIZES_ALL {
            for m in 0u8..25 {
                for rf in ref_pairs {
                    let c = cmd::is_interintra_allowed(enable, bsize, m, rf) != 0;
                    let r = rmd::is_interintra_allowed(enable != 0, bsize, m, rf);
                    assert_eq!(
                        c, r,
                        "svt_is_interintra_allowed(enable={enable}, bsize={bsize}, \
                         mode={m}, rf={rf:?})"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_get_wedge_params_bits (inter_prediction.c:2053) — EXHAUSTIVE.
// The port carries the table inline; this proves the transcription.
// ---------------------------------------------------------------------------

#[test]
fn wedge_params_bits_matches_c_exhaustively() {
    for bsize in 0..BLOCK_SIZES_ALL {
        assert_eq!(
            cmd::get_wedge_params_bits(bsize),
            rmd::wedge_params_bits(bsize),
            "svt_aom_get_wedge_params_bits(bsize={bsize})"
        );
    }
}

/// `get_tot_comp_types_bsize` is `static` in C (mode_decision.c:111) —
/// **tier 4**. It is nevertheless pinned against the tier-1
/// `wedge_params_bits` oracle so the only untested step is the two-line
/// `MIN`.
#[test]
fn tot_comp_types_bsize_tracks_the_c_wedge_table() {
    for bsize in 0..BLOCK_SIZES_ALL {
        for tot in 0u8..6 {
            let expect = if cmd::get_wedge_params_bits(bsize) == 0 {
                tot.min(3) // MD_COMP_WEDGE
            } else {
                tot
            };
            assert_eq!(
                expect,
                rmd::get_tot_comp_types_bsize(tot, bsize),
                "get_tot_comp_types_bsize(tot={tot}, bsize={bsize})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_get_me_block_offset (mode_decision.c:117) — EXHAUSTIVE over the
// SB64 grid x every block size x both enable flags.
// ---------------------------------------------------------------------------

#[test]
fn me_block_offset_matches_c_exhaustively() {
    for bsize in 0..BLOCK_SIZES_ALL {
        for enable_8x8 in [0u8, 1] {
            for enable_16x16 in [0u8, 1] {
                for org_y in (0..64u32).step_by(4) {
                    for org_x in (0..64u32).step_by(4) {
                        let c =
                            cmd::get_me_block_offset(org_x, org_y, bsize, enable_8x8, enable_16x16);
                        let r = rmd::get_me_block_offset(
                            org_x,
                            org_y,
                            bsize,
                            enable_8x8 != 0,
                            enable_16x16 != 0,
                        );
                        assert_eq!(
                            c, r,
                            "svt_aom_get_me_block_offset(x={org_x}, y={org_y}, \
                             bsize={bsize}, me8={enable_8x8}, me16={enable_16x16})"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_is_valid_unipred_ref (mode_decision.c:762) — randomized over the
// full ref_filtering_res table.
// ---------------------------------------------------------------------------

#[test]
fn is_valid_unipred_ref_matches_c() {
    const N: usize = rmd::TOT_INTER_GROUP * rmd::MAX_NUM_OF_REF_PIC_LIST * rmd::REF_LIST_MAX_DEPTH;
    let groups = [
        rmd::InterCandGroup::PaMe,
        rmd::InterCandGroup::Uni3x3,
        rmd::InterCandGroup::Bi3x3,
        rmd::InterCandGroup::NrstNewNear,
        rmd::InterCandGroup::NrstNear,
        rmd::InterCandGroup::PredMe,
        rmd::InterCandGroup::Global,
        rmd::InterCandGroup::Warp,
        rmd::InterCandGroup::Obmc,
        rmd::InterCandGroup::InterIntra,
        rmd::InterCandGroup::InterComp,
    ];
    let mut rng = Rng(0x51ED_0B0E_2026_0831);
    let mut checked = 0usize;
    for trial in 0..64 {
        let mut do_ref_flat = [0u8; N];
        let mut closest = [0u8; rmd::TOT_INTER_GROUP];
        for b in do_ref_flat.iter_mut() {
            *b = (rng.below(2)) as u8;
        }
        for b in closest.iter_mut() {
            *b = (rng.below(2)) as u8;
        }
        let enabled = trial % 2 == 0;

        let mut state = rmd::RefPruningState {
            enabled,
            ..Default::default()
        };
        for g in 0..rmd::TOT_INTER_GROUP {
            state.closest_refs[g] = closest[g] != 0;
            for l in 0..rmd::MAX_NUM_OF_REF_PIC_LIST {
                for r in 0..rmd::REF_LIST_MAX_DEPTH {
                    state.do_ref[g][l][r] = do_ref_flat
                        [(g * rmd::MAX_NUM_OF_REF_PIC_LIST + l) * rmd::REF_LIST_MAX_DEPTH + r]
                        != 0;
                }
            }
        }

        for (gi, group) in groups.iter().enumerate() {
            for l in 0..rmd::MAX_NUM_OF_REF_PIC_LIST {
                for r in 0..rmd::REF_LIST_MAX_DEPTH {
                    let c = cmd::is_valid_unipred_ref(
                        enabled,
                        &do_ref_flat,
                        &closest,
                        gi as u8,
                        l as u8,
                        r as u8,
                    );
                    let got = rmd::is_valid_unipred_ref(&state, *group, l, r);
                    assert_eq!(
                        c, got,
                        "svt_aom_is_valid_unipred_ref(enabled={enabled}, group={gi}, \
                         list={l}, ref={r}) trial {trial}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 64 * 11 * 2 * 4, "positive control: cases compared");
}

// ---------------------------------------------------------------------------
// svt_aom_is_me_data_present (mode_decision.c:179) — randomized candidate
// arrays.
// ---------------------------------------------------------------------------

#[test]
fn is_me_data_present_matches_c() {
    let mut rng = Rng(0x4D45_0DA7_2026_0831);
    let mut hits = 0usize;
    let mut checked = 0usize;
    for _ in 0..400 {
        let n_blocks = 1 + rng.below(4) as usize;
        let n_cands = 1 + rng.below(12) as usize;
        let totals: Vec<u8> = (0..n_blocks).map(|_| rng.below(5) as u8).collect();
        let c_cands: Vec<cmd::RefMeCandidate> = (0..n_cands)
            .map(|_| cmd::RefMeCandidate {
                direction: rng.below(3) as u8,
                ref_idx_l0: rng.below(4) as u8,
                ref_idx_l1: rng.below(4) as u8,
                ref0_list: rng.below(2) as u8,
                ref1_list: rng.below(2) as u8,
            })
            .collect();
        let r_cands: Vec<rmd::MeCandidateRef> = c_cands
            .iter()
            .map(|c| rmd::MeCandidateRef {
                direction: c.direction,
                ref_idx_l0: c.ref_idx_l0,
                ref_idx_l1: c.ref_idx_l1,
                ref0_list: c.ref0_list,
                ref1_list: c.ref1_list,
            })
            .collect();

        let blk = rng.below(n_blocks as u64) as usize;
        // C reads me_candidate_array + me_cand_offset and walks
        // totals[blk] entries; keep the offset in range of the array.
        let max_off = n_cands.saturating_sub(totals[blk] as usize);
        let off = if max_off == 0 {
            0
        } else {
            rng.below(max_off as u64 + 1) as usize
        };
        if off + totals[blk] as usize > n_cands {
            continue;
        }
        for list_idx in 0u8..2 {
            for ref_idx in 0u8..4 {
                let c = cmd::is_me_data_present(
                    blk as u32, off as u32, &totals, &c_cands, list_idx, ref_idx,
                ) != 0;
                let r = rmd::is_me_data_present(blk, &totals, &r_cands[off..], list_idx, ref_idx);
                assert_eq!(
                    c, r,
                    "svt_aom_is_me_data_present(blk={blk}, off={off}, list={list_idx}, \
                     ref={ref_idx})"
                );
                if c {
                    hits += 1;
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 1000, "positive control: {checked} cases compared");
    assert!(hits > 100, "positive control: only {hits} true results");
}

// ---------------------------------------------------------------------------
// svt_aom_obmc_motion_mode_allowed (mode_decision.c:214) — randomized over
// every field the predicate reads.
// ---------------------------------------------------------------------------

#[test]
fn obmc_motion_mode_allowed_matches_c() {
    let mut rng = Rng(0x0B3C_2026_0831_0001);
    let mut obmc_seen = 0usize;
    let mut checked = 0usize;
    for _ in 0..20000 {
        let trans_face_off = rng.below(2) as u8;
        let obmc_enabled = rng.below(2) as u8;
        let obmc_max_blk_size = [8u8, 16, 32, 64, 128][rng.below(5) as usize];
        let situation = rng.below(3) as u8;
        let switchable = rng.below(2) as u8;
        let force_integer_mv = rng.below(2) as u8;
        let mut gm_wmtype = [0i32; 8];
        for g in gm_wmtype.iter_mut() {
            *g = rng.below(4) as i32;
        }
        let overlappable = rng.below(3) as u32;
        let bsize = rng.below(u64::from(BLOCK_SIZES_ALL)) as u8;
        let rf0 = (rng.below(8)) as i8;
        // rf1 spans NONE_FRAME (-1), INTRA_FRAME (0) and real refs, which
        // is exactly the three-way split C's predicate makes. It is BIASED
        // toward -1, and the mode toward the single-ref inter modes,
        // because a uniform draw reaches C's OBMC_CAUSAL arm in well under
        // 1% of cases — the run's own positive control (below) is what
        // caught that.
        let rf1 = if rng.below(2) == 0 {
            -1
        } else {
            (rng.below(9) as i8) - 1
        };
        let m = if rng.below(2) == 0 {
            13 + rng.below(4) as u8
        } else {
            rng.below(25) as u8
        };

        let c = cmd::obmc_motion_mode_allowed(&cmd::ObmcAllowedInput {
            trans_face_off,
            obmc_enabled,
            obmc_max_blk_size,
            situation,
            is_motion_mode_switchable: switchable,
            force_integer_mv,
            gm_wmtype,
            overlappable_neighbors: overlappable,
            bsize,
            rf0,
            rf1,
            mode: m,
        });

        let ctx = rmd::MotionModeCtx {
            trans_face_off: trans_face_off != 0,
            obmc_enabled: obmc_enabled != 0,
            obmc_max_blk_size,
            is_motion_mode_switchable: switchable != 0,
            force_integer_mv,
            has_overlappable_candidates: overlappable != 0,
            allow_warped_motion: false,
            wm_enabled: false,
            blk_width: 0,
            blk_height: 0,
        };
        let wm = match gm_wmtype[rf0 as usize] {
            0 => TransformationType::Identity,
            1 => TransformationType::Translation,
            2 => TransformationType::RotZoom,
            _ => TransformationType::Affine,
        };
        let r = rmd::obmc_motion_mode_allowed(&ctx, bsize, situation, wm, rf0, rf1, m) as i32;
        assert_eq!(
            c, r,
            "svt_aom_obmc_motion_mode_allowed(tfo={trans_face_off}, en={obmc_enabled}, \
             max={obmc_max_blk_size}, sit={situation}, sw={switchable}, \
             fim={force_integer_mv}, ovl={overlappable}, bsize={bsize}, rf0={rf0}, \
             rf1={rf1}, mode={m})"
        );
        if c == 1 {
            obmc_seen += 1;
        }
        checked += 1;
    }
    assert_eq!(checked, 20000);
    assert!(
        obmc_seen > 200,
        "positive control: only {obmc_seen} OBMC_CAUSAL results — the probe \
         may never be reaching the non-SIMPLE arm"
    );
}
