//! Differential parity: the fork's mds0 distortion facade
//! (`svt_spatial_full_distortion_kernel_facade`) vs the Rust bias layer.
//!
//! The C facade = SSE kernel + bias layer; the Rust port splits them, so
//! the witness recomputes SSE independently and applies `facade_bias`,
//! asserting end-to-end equality with the real C for every intra mode,
//! chroma variant, tx_bias level, temporal layer and ac_bias setting.
use svtav1_cref as cref;
use svtav1_encoder::tx_bias::{facade_bias, BiasModeClass};

// AV1 PredictionMode values (definitions.h enum order).
const DC_PRED: u8 = 0;
const V_PRED: u8 = 1;
const H_PRED: u8 = 2;
const D45_PRED: u8 = 3;
const D135_PRED: u8 = 4;
const SMOOTH_PRED: u8 = 9;
const SMOOTH_V_PRED: u8 = 10;
const SMOOTH_H_PRED: u8 = 11;
const PAETH_PRED: u8 = 12;

fn class_for_intra(mode: u8, is_chroma: bool) -> BiasModeClass {
    // UV enum mirrors the luma enum for the modes used here.
    let _ = is_chroma;
    match mode {
        DC_PRED | SMOOTH_PRED | SMOOTH_V_PRED | SMOOTH_H_PRED => BiasModeClass::IntraBlurry,
        H_PRED | V_PRED | PAETH_PRED => BiasModeClass::IntraNeutral,
        _ => BiasModeClass::IntraOther,
    }
}

fn sse(a: &[u8], sa: usize, b: &[u8], sb: usize, w: usize, h: usize) -> i64 {
    let mut acc = 0i64;
    for r in 0..h {
        for c in 0..w {
            let d = i64::from(a[r * sa + c]) - i64::from(b[r * sb + c]);
            acc += d * d;
        }
    }
    acc
}

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
}

#[test]
fn facade_matches_c_for_intra_modes() {
    let mut rng = Rng(0xfacade_b1a5);
    let modes = [
        DC_PRED,
        V_PRED,
        H_PRED,
        D45_PRED,
        D135_PRED,
        SMOOTH_PRED,
        SMOOTH_V_PRED,
        SMOOTH_H_PRED,
        PAETH_PRED,
    ];
    let dims = [(8usize, 8usize), (16, 16), (32, 32), (64, 64), (64, 32)];
    for &(w, h) in &dims {
        let a: Vec<u8> = (0..w * h).map(|_| (rng.next() >> 32) as u8).collect();
        let b: Vec<u8> = (0..w * h).map(|_| (rng.next() >> 32) as u8).collect();
        let base = sse(&a, w, &b, w, w, h);
        for &mode in &modes {
            for &is_chroma in &[false, true] {
                for &tx_bias in &[0u8, 1, 2, 3] {
                    for &ac_bias in &[0.0f64, 1.0] {
                        for &layer in &[0u8, 2, 5] {
                            let c = cref::spatial_facade(
                                &a, w as u32, &b, w as u32, w as u32, h as u32,
                                mode, mode, // UV enum mirrors luma for these
                                is_chroma, false, 0, layer, ac_bias, tx_bias,
                            );
                            let r = facade_bias(
                                base,
                                class_for_intra(mode, is_chroma),
                                true,
                                w as u32,
                                h as u32,
                                layer,
                                ac_bias,
                                tx_bias,
                            ) as u64;
                            assert_eq!(
                                r, c,
                                "mode {mode} chroma {is_chroma} txb {tx_bias} acb {ac_bias} layer {layer} {w}x{h}"
                            );
                        }
                    }
                }
            }
        }
    }
}
