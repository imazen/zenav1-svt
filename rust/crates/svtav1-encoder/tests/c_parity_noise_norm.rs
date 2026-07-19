//! Differential parity: fork noise normalization vs the exported C
//! function (`svt_av1_perform_noise_normalization`, full_loop.c fork
//! block; present in both hybrid libs). Random quantizer-shaped inputs
//! across tx sizes, strengths, and both branches (textured eob>1, flat
//! eob==1), asserting identical qcoeff/dqcoeff/eob afterwards.
use svtav1_cref as cref;
use svtav1_encoder::noise_norm;
use svtav1_entropy::{coeff_c, scan_tables};

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
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Build a coherent (coeff, qcoeff, dqcoeff, eob) set the way a real
/// quantizer would: pick qcoeff by rounding coeff/dqv toward zero with
/// noise, dqcoeff = qcoeff * dqv >> shift.
fn synth(
    rng: &mut Rng,
    n: usize,
    scan: &[u16],
    dequant: [i16; 2],
    shift: i32,
    eob_target: usize,
) -> (Vec<i32>, Vec<i32>, Vec<i32>, u16) {
    let mut coeff = vec![0i32; n];
    let mut q = vec![0i32; n];
    let mut dq = vec![0i32; n];
    for si in 0..eob_target {
        let ci = scan[si] as usize;
        let dqv = i32::from(dequant[usize::from(ci != 0)]);
        let mag = rng.range(3 * (dqv as u64).max(1)) as i32 + 1;
        let sign = if rng.next() & 1 == 1 { -1 } else { 1 };
        coeff[ci] = sign * mag;
        let qc = mag / dqv.max(1);
        q[ci] = sign * qc;
        dq[ci] = sign * ((qc * dqv) >> shift);
    }
    // eob = last nonzero q in scan order + 1 (fall back to eob_target for
    // the flat case where the AC positions quantized to zero).
    let mut eob = 1u16;
    for si in (0..eob_target).rev() {
        if q[scan[si] as usize] != 0 {
            eob = si as u16 + 1;
            break;
        }
    }
    (coeff, q, dq, eob)
}

#[test]
fn noise_norm_matches_c() {
    let mut rng = Rng(0x5eed_2077);
    // (c_tx_size, name dims) — REAL TxSize indexes; 4x4 (0) pins the
    // early-exit, others exercise both branches.
    for &c_tx in &[0usize, 1, 2, 5, 9, 12] {
        let w = coeff_c::txb_wide(c_tx);
        let h = coeff_c::txb_high(c_tx);
        let n = w * h;
        let scan = scan_tables::scan(c_tx, scan_tables::TX_TYPE_TO_SCAN_INDEX[0] as usize);
        let shift = svtav1_encoder::quant::TX_SCALE_TAB[c_tx];
        for strength in 0u8..=4 {
            for trial in 0..40 {
                let dequant = [
                    (rng.range(2000) + 8) as i16,
                    (rng.range(2000) + 8) as i16,
                ];
                let eob_target = if trial % 3 == 0 {
                    1
                } else {
                    (rng.range((n as u64 / 2).max(2)) + 2) as usize
                };
                let (coeff, q0, dq0, eob0) =
                    synth(&mut rng, n, scan, dequant, shift, eob_target.min(n));

                let mut q_r = q0.clone();
                let mut dq_r = dq0.clone();
                let mut eob_r = eob0;
                noise_norm::perform_noise_normalization(
                    &[i32::from(dequant[0]), i32::from(dequant[1])],
                    None, // iqmatrix: the C shim passes NULL too
                    &coeff,
                    &mut q_r,
                    &mut dq_r,
                    &mut eob_r,
                    scan,
                    c_tx,
                    strength,
                );

                let mut q_c = q0.clone();
                let mut dq_c = dq0.clone();
                let mut eob_c = eob0;
                cref::noise_normalization(
                    dequant, &coeff, &mut q_c, &mut dq_c, &mut eob_c, c_tx as i32,
                    0, // DCT_DCT
                    strength,
                );

                assert_eq!(eob_r, eob_c, "tx {c_tx} s {strength} t {trial} eob");
                assert_eq!(q_r, q_c, "tx {c_tx} s {strength} t {trial} qcoeff");
                assert_eq!(dq_r, dq_c, "tx {c_tx} s {strength} t {trial} dqcoeff");
            }
        }
    }
}
