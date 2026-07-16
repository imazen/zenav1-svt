//! AC Bias (`--ac-bias`) — psychovisual distortion/rate adjustments.
//!
//! Port of `Source/Lib/Codec/ac_bias.c` (mainline v4.2, upstreamed from
//! SVT-AV1-PSY; the svt-av1-hdr fork defaults `ac_bias` to 1.0 and routes
//! it through its mds0 distortion facade). 8-bit path only (the port's
//! envelope); the HBD variant mirrors trivially when 10-bit lands.
//!
//! Three pieces:
//! * [`psy_distortion`] — the "energy gap": per 8x8 (or 4x4 for thin
//!   blocks) |(SATD−DC)_src − (SATD−DC)_recon| summed over the block.
//! * [`psy_adjust_rate_light`] — Light-PD variant: block rate is reduced
//!   by `energy * ac_bias * 100` (floored at 1) so high-energy blocks look
//!   cheaper to RDO.
//! * [`effective_ac_bias`] — slice/temporal-layer scaling of the config
//!   value (I-slice ×0.3; layers 0/1/2 ×0.6/0.8/0.9).
//!
//! All three are differentially tested against the exported C functions
//! (`tests/c_parity_ac_bias.rs`).

use crate::hadamard::{aom_hadamard_4x4, aom_hadamard_8x8, aom_satd};

/// C `svt_psy_distortion` (ac_bias.c:22), 8-bit.
///
/// `width`/`height` are the block dims; strides in pixels. Blocks with
/// both dims >= 8 walk 8x8 tiles; anything thinner walks 4x4 tiles
/// (4x4, 4x8, 4x16, 8x4, 16x4 — C comment).
pub fn psy_distortion(
    input: &[u8],
    input_stride: usize,
    recon: &[u8],
    recon_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut energy_gap: u64 = 0;

    if width >= 8 && height >= 8 {
        let mut coeffs = [0i32; 64];
        let mut block = [0i16; 64];
        let mut j = 0;
        while j < height {
            let mut i = 0;
            while i < width {
                for h in 0..8 {
                    let row = &input[(j + h) * input_stride + i..][..8];
                    for w in 0..8 {
                        block[h * 8 + w] = i16::from(row[w]);
                    }
                }
                aom_hadamard_8x8(&block, 8, &mut coeffs);
                let input_energy = ((aom_satd(&coeffs) + 2) >> 2) - ((coeffs[0] + 2) >> 2);

                for h in 0..8 {
                    let row = &recon[(j + h) * recon_stride + i..][..8];
                    for w in 0..8 {
                        block[h * 8 + w] = i16::from(row[w]);
                    }
                }
                aom_hadamard_8x8(&block, 8, &mut coeffs);
                let recon_energy = ((aom_satd(&coeffs) + 2) >> 2) - ((coeffs[0] + 2) >> 2);

                energy_gap += u64::from(input_energy.abs_diff(recon_energy));
                i += 8;
            }
            j += 8;
        }
    } else {
        let mut coeffs = [0i32; 16];
        let mut block = [0i16; 16];
        let mut j = 0;
        while j < height {
            let mut i = 0;
            while i < width {
                for h in 0..4 {
                    let row = &input[(j + h) * input_stride + i..][..4];
                    for w in 0..4 {
                        block[h * 4 + w] = i16::from(row[w]);
                    }
                }
                aom_hadamard_4x4(&block, 4, &mut coeffs);
                let input_energy = (aom_satd(&coeffs) << 1) - coeffs[0];

                for h in 0..4 {
                    let row = &recon[(j + h) * recon_stride + i..][..4];
                    for w in 0..4 {
                        block[h * 4 + w] = i16::from(row[w]);
                    }
                }
                aom_hadamard_4x4(&block, 4, &mut coeffs);
                let recon_energy = (aom_satd(&coeffs) << 1) - coeffs[0];

                energy_gap += u64::from(input_energy.abs_diff(recon_energy));
                i += 4;
            }
            j += 4;
        }
    }

    energy_gap
}

/// C `get_svt_psy_full_dist` (ac_bias.c:150), 8-bit path:
/// `llrint(psy_distortion(...) * ac_bias)`.
#[allow(clippy::too_many_arguments)]
pub fn psy_full_dist(
    src: &[u8],
    src_offset: usize,
    src_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    width: usize,
    height: usize,
    ac_bias: f64,
) -> u64 {
    let d = psy_distortion(
        &src[src_offset..],
        src_stride,
        &recon[recon_offset..],
        recon_stride,
        width,
        height,
    );
    // llrint with a non-negative product == round-half-to-even; C's
    // default rounding mode. Rust f64::round() rounds half AWAY from zero
    // — use round_ties_even to match llrint exactly.
    (d as f64 * ac_bias).round_ties_even() as u64
}

/// C `svt_psy_adjust_rate_light` (ac_bias.c:174): subtract
/// `energy * ac_bias * 100` (as-int) from `coeff_bits`, floored at 1.
/// `coeff` is the width*height coefficient raster; DC (index 0) skipped.
pub fn psy_adjust_rate_light(
    coeff: &[i32],
    coeff_bits: u64,
    width: usize,
    height: usize,
    ac_bias: f64,
) -> u64 {
    let mut energy: u64 = 0;
    for j in 0..height {
        let row = &coeff[j * width..][..width];
        let start = usize::from(j == 0);
        for &c in &row[start..] {
            energy += u64::from(c.unsigned_abs());
        }
    }
    if energy > 0 {
        // C: `(int)(energy * ac_bias * 100)` — f64 product truncated to
        // int, then widened. Mirror the (int) cast semantics.
        let coeff_bits_adj = (energy as f64 * ac_bias * 100.0) as i32 as u64;
        if coeff_bits > coeff_bits_adj {
            coeff_bits - coeff_bits_adj
        } else {
            1
        }
    } else {
        coeff_bits
    }
}

/// C `get_effective_ac_bias` (ac_bias.c:197).
pub fn effective_ac_bias(ac_bias: f64, is_islice: bool, temporal_layer_index: u8) -> f64 {
    if is_islice {
        return ac_bias * 0.3;
    }
    match temporal_layer_index {
        0 => ac_bias * 0.6,
        1 => ac_bias * 0.8,
        2 => ac_bias * 0.9,
        _ => ac_bias,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn zero_gap_when_identical() {
        let img: Vec<u8> = (0..64 * 64).map(|i| (i % 251) as u8).collect();
        assert_eq!(psy_distortion(&img, 64, &img, 64, 64, 64), 0);
        assert_eq!(psy_distortion(&img, 64, &img, 64, 4, 16), 0);
    }

    #[test]
    fn effective_bias_table() {
        assert_eq!(effective_ac_bias(1.0, true, 5), 0.3);
        assert_eq!(effective_ac_bias(1.0, false, 0), 0.6);
        assert_eq!(effective_ac_bias(1.0, false, 1), 0.8);
        assert_eq!(effective_ac_bias(1.0, false, 2), 0.9);
        assert_eq!(effective_ac_bias(1.0, false, 3), 1.0);
    }

    #[test]
    fn rate_light_floors_at_one() {
        let coeff = vec![100i32; 16];
        assert_eq!(psy_adjust_rate_light(&coeff, 10, 4, 4, 1.0), 1);
        // zero energy (DC-only) leaves rate untouched
        let mut dc_only = vec![0i32; 16];
        dc_only[0] = 500;
        assert_eq!(psy_adjust_rate_light(&dc_only, 777, 4, 4, 1.0), 777);
    }
}
