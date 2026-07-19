//! Differential parity: AC-bias psychovisual kernels vs the exported C
//! reference (`svt_psy_distortion`, `svt_psy_adjust_rate_light`,
//! `get_effective_ac_bias` — ac_bias.c). Feature code compiled into BOTH
//! SVT_HDR_MODE libs, so this runs under the standard differential setup.
use svtav1_cref as cref;
use svtav1_dsp::ac_bias;

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

#[test]
fn psy_distortion_matches_c_across_dims() {
    let mut rng = Rng(0x5eed_acb1_a500);
    // The C walker's dim classes: >=8x8 tiles and the thin 4-tile shapes.
    let dims = [
        (4usize, 4usize),
        (4, 8),
        (8, 4),
        (4, 16),
        (16, 4),
        (8, 8),
        (8, 16),
        (16, 8),
        (16, 16),
        (32, 32),
        (64, 32),
        (64, 64),
    ];
    for &(w, h) in &dims {
        for trial in 0..8 {
            let stride_a = w + (rng.next() as usize % 17);
            let stride_b = w + (rng.next() as usize % 9);
            let a: Vec<u8> = (0..stride_a * h).map(|_| rng.byte()).collect();
            // trial 0: identical content (gap 0); others: independent noise
            // or source +- small perturbation (realistic recon).
            let b: Vec<u8> = if trial == 0 {
                let mut b = vec![0u8; stride_b * h];
                for r in 0..h {
                    for c in 0..w {
                        b[r * stride_b + c] = a[r * stride_a + c];
                    }
                }
                b
            } else if trial % 2 == 0 {
                (0..stride_b * h).map(|_| rng.byte()).collect()
            } else {
                let mut b = vec![0u8; stride_b * h];
                for r in 0..h {
                    for c in 0..w {
                        let p = i16::from(a[r * stride_a + c]) + (rng.next() as i16 % 9) - 4;
                        b[r * stride_b + c] = p.clamp(0, 255) as u8;
                    }
                }
                b
            };
            assert_eq!(
                ac_bias::psy_distortion(&a, stride_a, &b, stride_b, w, h),
                cref::psy_distortion(&a, stride_a as u32, &b, stride_b as u32, w as u32, h as u32),
                "dims {w}x{h} trial {trial}"
            );
        }
    }
}

#[test]
fn psy_adjust_rate_light_matches_c() {
    let mut rng = Rng(0xf00d_1157);
    for &(w, h) in &[(4usize, 4usize), (8, 8), (16, 16), (32, 32), (16, 4)] {
        for &bias in &[0.0f64, 0.25, 1.0, 1.5, 4.0, 8.0] {
            for _ in 0..6 {
                let coeff: Vec<i32> = (0..w * h)
                    .map(|_| ((rng.next() as i32) % 4001) - 2000)
                    .collect();
                let bits = rng.next() % 5_000_000;
                assert_eq!(
                    ac_bias::psy_adjust_rate_light(&coeff, bits, w, h, bias),
                    cref::psy_adjust_rate_light(&coeff, bits, w as u32, h as u32, bias),
                    "dims {w}x{h} bias {bias} bits {bits}"
                );
            }
        }
    }
}

#[test]
fn effective_ac_bias_matches_c() {
    for &bias in &[0.0f64, 0.3, 1.0, 2.5, 8.0] {
        for layer in 0u8..=5 {
            for &isl in &[true, false] {
                let r = ac_bias::effective_ac_bias(bias, isl, layer);
                let c = cref::effective_ac_bias(bias, isl, layer);
                assert!(
                    (r - c).abs() < 1e-15 || r == c,
                    "bias {bias} islice {isl} layer {layer}: {r} vs {c}"
                );
            }
        }
    }
}
