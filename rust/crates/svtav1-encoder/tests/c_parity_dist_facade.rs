//! Differential parity for the two tx-bias distortion facades of
//! `Codec/pic_operators.c` vs the real exported C symbols — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! This is the test `tx_bias`'s module doc claimed already existed. It did
//! not: `c_parity_ac_bias.rs` covers `psy_distortion`,
//! `psy_adjust_rate_light` and `effective_ac_bias` and never calls either
//! facade, so the bias layer had no differential at all before this file.
//!
//! Both facades are swept over EVERY (mode, uv_mode) pair rather than a
//! representative sample, because the classification nest is the part that
//! is easy to get subtly wrong — C reads `mode` for luma and `uv_mode` for
//! chroma, checks `is_interintra_used` BEFORE the compound type, and gives
//! single-reference inter modes no mode bias at all.

use svtav1_cref::pic_operators as cref_po;
use svtav1_dsp::pic_operators::{self as po, FullDistortion};
use svtav1_encoder::dist_facade::{
    FacadeMode, picture_full_distortion32_facade, spatial_full_distortion_facade,
};
use svtav1_types::prediction::{CompoundType, PredictionMode, UvPredictionMode};

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
}

const MODES: [PredictionMode; 25] = {
    use PredictionMode::*;
    [
        DcPred,
        VPred,
        HPred,
        D45Pred,
        D135Pred,
        D113Pred,
        D157Pred,
        D203Pred,
        D67Pred,
        SmoothPred,
        SmoothVPred,
        SmoothHPred,
        PaethPred,
        NearestMv,
        NearMv,
        GlobalMv,
        NewMv,
        NearestNearestMv,
        NearNearMv,
        NearestNewMv,
        NewNearestMv,
        NearNewMv,
        NewNearMv,
        GlobalGlobalMv,
        NewNewMv,
    ]
};

const UV_MODES: [UvPredictionMode; 14] = {
    use UvPredictionMode::*;
    [
        UvDcPred,
        UvVPred,
        UvHPred,
        UvD45Pred,
        UvD135Pred,
        UvD113Pred,
        UvD157Pred,
        UvD203Pred,
        UvD67Pred,
        UvSmoothPred,
        UvSmoothVPred,
        UvSmoothHPred,
        UvPaethPred,
        UvCflPred,
    ]
};

const COMPOUNDS: [CompoundType; 4] = [
    CompoundType::Average,
    CompoundType::DistWtd,
    CompoundType::Wedge,
    CompoundType::DiffWtd,
];

/// `(width, height)` pairs chosen to hit all three transform-size arms:
/// the 64x64 strong bias, the `<= 32*32` mild bias, and neither.
const DIMS: &[(usize, usize)] = &[(64, 64), (32, 32), (16, 8), (64, 32), (32, 64), (8, 8)];

#[test]
fn spatial_facade_matches_c_over_every_mode_pair() {
    let mut rng = Rng(0x5A17_C0DE);
    let mut cases = 0usize;
    for &(w, h) in DIMS {
        let stride = w + 7;
        let input: Vec<u8> = (0..stride * h + stride).map(|_| rng.next() as u8).collect();
        let recon: Vec<u8> = (0..stride * h + stride).map(|_| rng.next() as u8).collect();
        let sse = po::spatial_full_distortion_kernel(&input, 0, stride, &recon, 0, stride, w, h);

        for (mi_idx, &mode) in MODES.iter().enumerate() {
            let uv_mode = UV_MODES[mi_idx % UV_MODES.len()];
            for &compound_type in &COMPOUNDS {
                for &is_interintra_used in &[false, true] {
                    for &is_chroma in &[false, true] {
                        for &ac_bias in &[0.0f64, 0.75] {
                            for tx_bias in 0..=2u8 {
                                for temporal_layer_index in [0u8, 2, 5] {
                                    let mine = spatial_full_distortion_facade(
                                        sse,
                                        FacadeMode {
                                            mode,
                                            uv_mode,
                                            is_interintra_used,
                                            compound_type,
                                        },
                                        is_chroma,
                                        w as u32,
                                        h as u32,
                                        temporal_layer_index,
                                        ac_bias,
                                        tx_bias,
                                    );
                                    let theirs = cref_po::spatial_full_distortion_kernel_facade(
                                        &input,
                                        0,
                                        stride,
                                        &recon,
                                        0,
                                        stride,
                                        w,
                                        h,
                                        cref_po::FacadeMode {
                                            mode: mode as i32,
                                            uv_mode: uv_mode as i32,
                                            is_interintra_used,
                                            compound_type: compound_type as i32,
                                        },
                                        is_chroma,
                                        temporal_layer_index,
                                        ac_bias,
                                        tx_bias,
                                    );
                                    assert_eq!(
                                        mine, theirs,
                                        "spatial facade {mode:?}/{uv_mode:?} ct {compound_type:?} \
                                         ii {is_interintra_used} chroma {is_chroma} ac {ac_bias} \
                                         txb {tx_bias} tl {temporal_layer_index} {w}x{h}"
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
    assert!(cases >= 10_000, "coverage collapsed to {cases} cases");
}

#[test]
fn full_distortion32_facade_matches_c_over_every_mode_pair() {
    let mut rng = Rng(0x3A17_F00D);
    let mut cases = 0usize;
    for &(w, h) in DIMS {
        let stride = w + 2;
        let coeff: Vec<i32> = (0..stride * h)
            .map(|_| (rng.next() % 8192) as i64 as i32 - 4096)
            .collect();
        let recon: Vec<i32> = (0..stride * h)
            .map(|_| (rng.next() % 8192) as i64 as i32 - 4096)
            .collect();

        for &nz in &[0u32, 5] {
            let base: FullDistortion =
                po::picture_full_distortion32_bits_single(&coeff, &recon, stride, w, h, nz != 0);

            for (mi_idx, &mode) in MODES.iter().enumerate() {
                let uv_mode = UV_MODES[(mi_idx + 3) % UV_MODES.len()];
                for &compound_type in &COMPOUNDS {
                    for &is_interintra_used in &[false, true] {
                        for &is_chroma in &[false, true] {
                            for &ac_bias in &[0.0f64, 0.75] {
                                for tx_bias in 0..=2u8 {
                                    for temporal_layer_index in [0u8, 3] {
                                        let mine = picture_full_distortion32_facade(
                                            base,
                                            FacadeMode {
                                                mode,
                                                uv_mode,
                                                is_interintra_used,
                                                compound_type,
                                            },
                                            is_chroma,
                                            w as u32,
                                            h as u32,
                                            temporal_layer_index,
                                            ac_bias,
                                            tx_bias,
                                        );
                                        let theirs =
                                            cref_po::picture_full_distortion32_bits_single_facade(
                                                &coeff,
                                                &recon,
                                                stride,
                                                w,
                                                h,
                                                w,
                                                h,
                                                nz,
                                                cref_po::FacadeMode {
                                                    mode: mode as i32,
                                                    uv_mode: uv_mode as i32,
                                                    is_interintra_used,
                                                    compound_type: compound_type as i32,
                                                },
                                                is_chroma,
                                                temporal_layer_index,
                                                ac_bias,
                                                tx_bias,
                                            );
                                        assert_eq!(
                                            (mine.residual, mine.prediction),
                                            theirs,
                                            "fulldist facade {mode:?}/{uv_mode:?} ct \
                                             {compound_type:?} ii {is_interintra_used} chroma \
                                             {is_chroma} ac {ac_bias} txb {tx_bias} tl \
                                             {temporal_layer_index} nz {nz} {w}x{h}"
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
    assert!(cases >= 10_000, "coverage collapsed to {cases} cases");
}
