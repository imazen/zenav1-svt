//! Differential parity: intra edge filter / upsample kernels + the
//! upsample-capable directional predictors vs the C reference
//! (`svt_av1_filter_intra_edge_c`, `svt_av1_upsample_intra_edge_c`,
//! `svt_aom_intra_edge_filter_strength`, `svt_aom_use_intra_edge_upsample`,
//! `svt_av1_dr_prediction_z1/z2/z3_c`).
//!
//! These are the kernels the M5 leaf funnel's directional predictions run
//! when SH `enable_intra_edge_filter = 1`; any drift is a recon-parity
//! (and therefore bitstream-identity) break.

use svtav1_cref as cref;
use svtav1_dsp::intra_pred as ip;

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
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
}

/// Exhaustive: strength + upsample decisions over every production
/// (bs0, bs1, delta, type) combination.
#[test]
fn edge_strength_and_upsample_decisions_match_c() {
    let dims = [4i32, 8, 16, 32, 64];
    for &bs0 in &dims {
        for &bs1 in &dims {
            for delta in -180..=180 {
                for filt_type in 0..=1 {
                    assert_eq!(
                        ip::intra_edge_filter_strength(bs0, bs1, delta, filt_type),
                        cref::intra_edge_filter_strength(bs0, bs1, delta, filt_type),
                        "strength bs0={bs0} bs1={bs1} delta={delta} type={filt_type}"
                    );
                    assert_eq!(
                        ip::use_intra_edge_upsample(bs0, bs1, delta, filt_type),
                        cref::use_intra_edge_upsample(bs0, bs1, delta, filt_type),
                        "upsample bs0={bs0} bs1={bs1} delta={delta} type={filt_type}"
                    );
                }
            }
        }
    }
}

/// Random-edge fuzz of the 5-tap edge filter at every size/strength.
#[test]
fn filter_intra_edge_matches_c() {
    let mut rng = Rng(0xed6ef117_0001);
    for sz in 1usize..=129 {
        for strength in 0..=3 {
            for _ in 0..20 {
                let mut buf_r = vec![0u8; 140];
                for b in buf_r.iter_mut() {
                    *b = rng.byte();
                }
                let mut buf_c = buf_r.clone();
                ip::filter_intra_edge(&mut buf_r, 4, sz, strength);
                cref::filter_intra_edge(&mut buf_c, 4, sz, strength);
                assert_eq!(buf_r, buf_c, "sz={sz} strength={strength}");
            }
        }
    }
}

/// Random-edge fuzz of the 2x edge upsampler at every legal size.
#[test]
fn upsample_intra_edge_matches_c() {
    let mut rng = Rng(0x0b5a_317e_0001);
    for sz in 1usize..=16 {
        for _ in 0..100 {
            let mut buf_r = vec![0u8; 64];
            for b in buf_r.iter_mut() {
                *b = rng.byte();
            }
            let mut buf_c = buf_r.clone();
            ip::upsample_intra_edge(&mut buf_r, 16, sz);
            cref::upsample_intra_edge(&mut buf_c, 16, sz);
            assert_eq!(buf_r, buf_c, "sz={sz}");
        }
    }
}

/// dr z1/z2/z3 with every production (angle, upsample) combination over
/// random edged buffers: angles are the M5 candidate set (8 base angles
/// x deltas {-3, 0, +3} x ANGLE_STEP 3), upsample flags derived exactly
/// like production (`use_intra_edge_upsample` per side).
#[test]
fn dr_prediction_kernels_match_c() {
    let mut rng = Rng(0xd41d_ed6e_u64 ^ 0x9E3779B97F4A7C15);
    let base_angles = [45i32, 67, 90, 113, 135, 157, 203, 180];
    let sizes = [(4usize, 4usize), (8, 8), (16, 16), (32, 32), (64, 64)];
    for &(w, h) in &sizes {
        for &base in &base_angles {
            for delta in [-3i32, 0, 3] {
                let angle = base + delta * 3;
                if angle <= 0 || angle >= 270 || angle == 90 || angle == 180 {
                    continue; // exact V/H route to predict_v/h (no dr kernel)
                }
                for filt_type in 0..=1 {
                    let upsample_above =
                        ip::use_intra_edge_upsample(w as i32, h as i32, angle - 90, filt_type);
                    let upsample_left =
                        ip::use_intra_edge_upsample(h as i32, w as i32, angle - 180, filt_type);
                    for _ in 0..30 {
                        let mut above = vec![0u8; ip::EDGE_BUF_LEN];
                        let mut left = vec![0u8; ip::EDGE_BUF_LEN];
                        for b in above.iter_mut() {
                            *b = rng.byte();
                        }
                        for b in left.iter_mut() {
                            *b = rng.byte();
                        }
                        // Shared top-left sample like the C builders.
                        left[ip::EDGE_ORIGIN - 1] = above[ip::EDGE_ORIGIN - 1];
                        let mut dst_r = vec![0u8; w * h];
                        let mut dst_c = vec![0u8; w * h];
                        ip::dr_predictor_edged(
                            &mut dst_r,
                            w,
                            &above,
                            &left,
                            ip::EDGE_ORIGIN,
                            upsample_above,
                            upsample_left,
                            w,
                            h,
                            angle,
                        );
                        cref::dr_predictor_edged(
                            &mut dst_c,
                            w,
                            &above,
                            &left,
                            ip::EDGE_ORIGIN,
                            upsample_above,
                            upsample_left,
                            w,
                            h,
                            angle,
                        );
                        assert_eq!(
                            dst_r, dst_c,
                            "dr {w}x{h} angle={angle} up_a={upsample_above} up_l={upsample_left}"
                        );
                    }
                }
            }
        }
    }
}
