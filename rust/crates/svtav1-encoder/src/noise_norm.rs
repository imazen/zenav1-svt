//! Noise normalization (`--noise-norm-strength`) — fork-only encode-pass
//! coefficient revival (C hybrid `svt_av1_perform_noise_normalization`,
//! full_loop.c fork block; fork default strength 1).
//!
//! After quantization, boost ONE AC coefficient back toward |1| step when
//! doing so closes most of the "energy gap" the quantizer opened:
//! * textured blocks (`eob > 1`): scan positions 1..eob for coefficients
//!   whose dequantized magnitude fell below the source magnitude; the
//!   LAST candidate whose gain ratio meets the threshold wins (C keeps
//!   overwriting `best_si` with no gap compare).
//! * flat blocks (`eob == 1`): scan positions 1..(w*h/16) for coefficients
//!   quantized to zero; the SMALLEST energy-gap candidate wins.
//!
//! Threshold by strength {1,2,3,4+} → {9,8,6,4} (of 16). 4x4 blocks and
//! strength 0 exit early. `eob` grows when a revived position lies past it.
//!
//! Differentially tested against the exported C function via a
//! minimal-struct shim (`tests/c_parity_noise_norm.rs`).

use svtav1_entropy::coeff_c;

/// C `get_qc_dqc_low` (full_loop.c:659): one quantization step DOWN from
/// `abs_qc` (caller passes target+1), returning signed (qc_low, dqc_low).
#[inline]
fn qc_dqc_low(abs_qc: i32, sign: bool, dqv: i32, shift: i32) -> (i32, i32) {
    let abs_qc_low = abs_qc - 1;
    let abs_dqc_low = (abs_qc_low * dqv) >> shift;
    if sign {
        (-abs_qc_low, -abs_dqc_low)
    } else {
        (abs_qc_low, abs_dqc_low)
    }
}

/// C `get_dqv` (full_loop.c:741) — per-position dequant with the QM
/// inverse weight applied when matrices are active.
#[inline]
fn dqv_for(dequant: &[i32; 2], coeff_idx: usize, iwt: Option<&[u8]>) -> i32 {
    crate::qm::dqv_qm(dequant, coeff_idx, iwt)
}

/// C `svt_av1_perform_noise_normalization` body. `c_tx_size` is the REAL
/// TxSize index (shift table + capped dims are derived like C).
/// `coeff`/`qcoeff`/`dqcoeff` are packed rasters in the same layout the
/// quantizer used; `scan` maps scan index → raster index.
#[allow(clippy::too_many_arguments)]
pub fn perform_noise_normalization(
    dequant: &[i32; 2],
    iwt: Option<&[u8]>,
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    scan: &[u16],
    c_tx_size: usize,
    strength: u8,
) {
    let shift = crate::quant::TX_SCALE_TAB[c_tx_size];
    let width = coeff_c::txb_wide(c_tx_size);
    let height = coeff_c::txb_high(c_tx_size);

    if width == 4 && height == 4 {
        return;
    }
    if strength < 1 {
        return;
    }

    let thresh: i32 = match strength {
        1 => 9,
        2 => 8,
        3 => 6,
        _ => 4,
    };

    let mut best_si: i32 = -1;
    let mut best_smallest_energy_gap = i32::MAX;
    let mut best_qc_low = 0i32;
    let mut best_dqc_low = 0i32;

    if *eob > 1 {
        // Textured: boost the most suitable AC coefficient within eob.
        for si in 1..usize::from(*eob) {
            let ci = scan[si] as usize;
            let tqc = coeff[ci];
            let qc = qcoeff[ci];
            let dqc = dqcoeff[ci];
            let sign = tqc < 0;

            if dqc != 0 && (tqc.abs() - dqc.abs()) > 0 {
                let dqv = dqv_for(dequant, ci, iwt);
                let abs_qc = (qc.abs() + 1) + 1; // +1: qc_dqc_low expects it
                let (qc_low, dqc_low) = qc_dqc_low(abs_qc, sign, dqv, shift);

                let energy_gap = (dqc_low - tqc).abs();
                let dq_step_size = (dqc_low - dqc).abs();
                let ratio = ((dq_step_size - energy_gap) << 4) / dq_step_size;

                // C takes the LAST candidate meeting the threshold here
                // (no energy-gap compare in the textured branch).
                if ratio >= thresh {
                    best_si = si as i32;
                    best_qc_low = qc_low;
                    best_dqc_low = dqc_low;
                }
            }
        }
    } else if *eob == 1 {
        // Flat: revive the best zeroed AC coefficient near DC.
        for si in 1..(width * height / 16) {
            let ci = scan[si] as usize;
            let tqc = coeff[ci];
            let dqc = dqcoeff[ci];
            let sign = tqc < 0;

            if dqc == 0 && tqc != 0 {
                let dqv = dqv_for(dequant, ci, iwt);
                let abs_qc = 1 + 1;
                let (qc_low, dqc_low) = qc_dqc_low(abs_qc, sign, dqv, shift);

                let energy_gap = (dqc_low - tqc).abs();
                let dq_step_size = (dqc_low - dqc).abs();
                let ratio = ((dq_step_size - energy_gap) << 4) / dq_step_size;

                if ratio >= thresh && energy_gap < best_smallest_energy_gap {
                    best_smallest_energy_gap = energy_gap;
                    best_si = si as i32;
                    best_qc_low = qc_low;
                    best_dqc_low = dqc_low;
                }
            }
        }
    }

    if best_si > 0 {
        let best_ci = scan[best_si as usize] as usize;
        qcoeff[best_ci] = best_qc_low;
        dqcoeff[best_ci] = best_dqc_low;
        if best_si as u16 >= *eob {
            *eob = best_si as u16 + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use svtav1_entropy::scan_tables;

    fn scan_for(c_tx: usize) -> &'static [u16] {
        // DCT_DCT scan for the tests.
        scan_tables::scan(c_tx, scan_tables::TX_TYPE_TO_SCAN_INDEX[0] as usize)
    }

    #[test]
    fn early_exits() {
        let scan = scan_for(0); // TX_4X4
        let coeff = [100i32; 16];
        let mut q = [1i32; 16];
        let mut dq = [50i32; 16];
        let mut eob = 5u16;
        let q0 = q;
        perform_noise_normalization(&[100, 100], None, &coeff, &mut q, &mut dq, &mut eob, scan, 0, 4);
        assert_eq!(q, q0, "4x4 must early-exit");
        let scan = scan_for(1); // TX_8X8
        let coeff = [100i32; 64];
        let mut q = [1i32; 64];
        let mut dq = [50i32; 64];
        let q0 = q;
        perform_noise_normalization(&[100, 100], None, &coeff, &mut q, &mut dq, &mut eob, scan, 1, 0);
        assert_eq!(q, q0, "strength 0 must early-exit");
    }

    #[test]
    fn flat_block_revives_and_grows_eob() {
        // TX_8X8 DC-only block with one strong zeroed AC at scan pos 1.
        let scan = scan_for(1);
        let mut coeff = [0i32; 64];
        let mut q = [0i32; 64];
        let mut dq = [0i32; 64];
        coeff[scan[0] as usize] = 400;
        q[scan[0] as usize] = 4;
        dq[scan[0] as usize] = 400;
        // Source AC close to one dequant step (dqv=100, shift=0 -> dqc_low
        // = 100): gap 4, step 100, ratio = (96<<4)/100 = 15 >= 9.
        coeff[scan[1] as usize] = 96;
        let mut eob = 1u16;
        perform_noise_normalization(&[100, 100], None, &coeff, &mut q, &mut dq, &mut eob, scan, 1, 1);
        assert_eq!(q[scan[1] as usize], 1);
        assert_eq!(dq[scan[1] as usize], 100);
        assert_eq!(eob, 2);
    }
}
