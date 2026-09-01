//! Differential parity for the loop-filter level derivations of
//! `Codec/deblocking_common.c` vs the real exported C symbols — evidence
//! tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! `svt_av1_loop_filter_frame_init` and `svt_aom_get_filter_level_delta_lf`
//! decide the level EVERY deblocked edge is filtered at. The three axes the
//! encoder does not currently signal (ref deltas, mode deltas, delta_lf) are
//! swept anyway, because the port's job is to be correct when they are
//! turned on, and a table that is only ever exercised in its degenerate
//! configuration proves nothing about the rest.
//!
//! The `lvl` table is compared including the cells C leaves UNWRITTEN: both
//! sides start from a 0xFF-filled struct, so a port that helpfully filled
//! `[INTRA_FRAME][1]` would fail here rather than pass by accident.

use svtav1_cref::pic_operators as cref_po;
use svtav1_encoder::lf_levels::{
    EdgeDir, LoopFilterLevels, LoopFilterParams, MAX_MODE_LF_DELTAS, MAX_PLANES, REF_FRAMES,
    SbDeltaLf, filter_level_delta_lf, loop_filter_frame_init,
};
use svtav1_types::restoration::MAX_SEGMENTS;
use svtav1_types::segmentation::{SEG_LVL_MAX, SegmentationParams};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Mirror a port-side `(LoopFilterParams, SegmentationParams)` pair into the
/// flat state the C shim rebuilds a `FrameHeader` from.
fn to_cref(lf: &LoopFilterParams, seg: &SegmentationParams) -> cref_po::LfFrameState {
    let mut st = cref_po::LfFrameState {
        filter_levels: [
            lf.filter_level[0],
            lf.filter_level[1],
            lf.filter_level_u,
            lf.filter_level_v,
        ],
        sharpness: lf.sharpness_level,
        mode_ref_delta_enabled: lf.mode_ref_delta_enabled,
        ref_deltas: lf.ref_deltas,
        mode_deltas: lf.mode_deltas,
        segmentation_enabled: seg.segmentation_enabled,
        ..cref_po::LfFrameState::default()
    };
    for s in 0..MAX_SEGMENTS {
        for f in 0..SEG_LVL_MAX {
            st.seg_enabled[s][f] = u8::from(seg.feature_enabled[s][f] != 0);
            st.seg_data[s][f] = i32::from(seg.feature_data[s][f]);
        }
    }
    st
}

/// The AV1 default ref/mode deltas the C encoder initializes
/// (`resource_coordination_process.c:394-401`), plus deliberately extreme
/// ones so the `scale` and the clamps are both exercised.
const REF_DELTA_SETS: &[[i8; REF_FRAMES]] = &[
    [0, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, -1, 0, -1, -1],
    [63, -63, 31, -31, 7, -7, 3, -3],
    [-128, 127, -128, 127, -128, 127, -128, 127],
];

const MODE_DELTA_SETS: &[[i8; MAX_MODE_LF_DELTAS]] = &[[0, 0], [-2, 3], [127, -128]];

#[test]
fn loop_filter_frame_init_matches_c() {
    let mut rng = Rng(0xC0FF_EE01);
    let mut cases = 0usize;
    for &ref_deltas in REF_DELTA_SETS {
        for &mode_deltas in MODE_DELTA_SETS {
            for &mrd in &[false, true] {
                for &levels in &[
                    [0, 0, 0, 0],
                    [1, 2, 3, 4],
                    [31, 32, 63, 0],
                    [63, 63, 63, 63],
                ] {
                    for &seg_on in &[false, true] {
                        let lf = LoopFilterParams {
                            filter_level: [levels[0], levels[1]],
                            filter_level_u: levels[2],
                            filter_level_v: levels[3],
                            sharpness_level: 0,
                            mode_ref_delta_enabled: mrd,
                            ref_deltas,
                            mode_deltas,
                        };
                        let mut seg = SegmentationParams {
                            segmentation_enabled: seg_on,
                            ..SegmentationParams::default()
                        };
                        if seg_on {
                            for s in 0..MAX_SEGMENTS {
                                for f in 0..SEG_LVL_MAX {
                                    seg.feature_enabled[s][f] = i16::from(rng.below(2) as i8);
                                    seg.feature_data[s][f] = rng.below(129) as i16 - 64;
                                }
                            }
                        }

                        for (start, end) in [(0usize, 3usize), (0, 1), (1, 3), (2, 3)] {
                            let mine = loop_filter_frame_init(
                                &lf,
                                &seg,
                                start,
                                end,
                                LoopFilterLevels::filled(0xFF),
                            );
                            let theirs = cref_po::loop_filter_frame_init(
                                &to_cref(&lf, &seg),
                                start as i32,
                                end as i32,
                                0xFF,
                            );
                            assert_eq!(
                                mine.as_flat().as_slice(),
                                theirs.as_slice(),
                                "frame_init levels {levels:?} mrd {mrd} seg {seg_on} \
                                 refd {ref_deltas:?} moded {mode_deltas:?} planes {start}..{end}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cases >= 500, "coverage collapsed to {cases} cases");
}

#[test]
fn get_filter_level_delta_lf_matches_c() {
    let mut rng = Rng(0x1234_5678);
    let mut cases = 0usize;
    for &ref_deltas in REF_DELTA_SETS {
        for &mode_deltas in MODE_DELTA_SETS {
            for &mrd in &[false, true] {
                for &seg_on in &[false, true] {
                    let lf = LoopFilterParams {
                        filter_level: [rng.below(64) as i32, rng.below(64) as i32],
                        filter_level_u: rng.below(64) as i32,
                        filter_level_v: rng.below(64) as i32,
                        sharpness_level: rng.below(8) as i32,
                        mode_ref_delta_enabled: mrd,
                        ref_deltas,
                        mode_deltas,
                    };
                    let mut seg = SegmentationParams {
                        segmentation_enabled: seg_on,
                        ..SegmentationParams::default()
                    };
                    if seg_on {
                        for s in 0..MAX_SEGMENTS {
                            for f in 0..SEG_LVL_MAX {
                                seg.feature_enabled[s][f] = i16::from(rng.below(2) as i8);
                                seg.feature_data[s][f] = rng.below(129) as i16 - 64;
                            }
                        }
                    }
                    let st = to_cref(&lf, &seg);

                    for &multi in &[false, true] {
                        let values = [
                            rng.below(129) as i32 - 64,
                            rng.below(129) as i32 - 64,
                            rng.below(129) as i32 - 64,
                            rng.below(129) as i32 - 64,
                        ];
                        let sb = SbDeltaLf { values, multi };
                        for plane in 0..MAX_PLANES {
                            for dir in [EdgeDir::Vert, EdgeDir::Horz] {
                                for seg_id in [0usize, 3, 7] {
                                    for ref_frame in 0..REF_FRAMES {
                                        for mode_delta in 0..MAX_MODE_LF_DELTAS {
                                            // C reads `mode_lf_lut[pred_mode]`; the shim is
                                            // handed a pred_mode that maps to `mode_delta`.
                                            // mode_lf_lut is 0 for every intra mode and for
                                            // GLOBALMV/NEARESTMV..., 1 for NEWMV-class modes;
                                            // DC_PRED (0) gives 0 and NEWMV gives 1.
                                            let pred_mode = if mode_delta == 0 { 0 } else { 16 };
                                            let mine = filter_level_delta_lf(
                                                &lf, &seg, dir, plane, sb, seg_id, mode_delta,
                                                ref_frame,
                                            );
                                            let mut c_delta = values;
                                            let theirs = cref_po::get_filter_level_delta_lf(
                                                &st,
                                                multi,
                                                dir.index() as i32,
                                                plane as i32,
                                                &mut c_delta,
                                                seg_id as u8,
                                                pred_mode,
                                                ref_frame as i32,
                                            );
                                            assert_eq!(
                                                mine, theirs,
                                                "delta_lf plane {plane} dir {dir:?} seg {seg_id} \
                                                 ref {ref_frame} mode {mode_delta} multi {multi} \
                                                 mrd {mrd} seg_on {seg_on}"
                                            );
                                            cases += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cases >= 5000, "coverage collapsed to {cases} cases");
}

/// `svt_aom_update_sharpness` is ported as
/// `svtav1_dsp::loop_filter::lf_thresholds`, which states the same
/// arithmetic per level instead of filling a 64-entry table. This pins that
/// equivalence against the real C function for every (level, sharpness).
#[test]
fn update_sharpness_matches_lf_thresholds_for_every_level_and_sharpness() {
    for sharpness in 0..8i32 {
        for level in 0..=63i32 {
            let (lim, mblim) = cref_po::update_sharpness(sharpness, level);
            let t = svtav1_dsp::loop_filter::lf_thresholds(level as u8, sharpness as u8);
            assert_eq!(t.lim, lim, "lim level {level} sharpness {sharpness}");
            assert_eq!(t.mblim, mblim, "mblim level {level} sharpness {sharpness}");
        }
    }
}
