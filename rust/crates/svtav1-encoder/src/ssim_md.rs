//! [SVT_HDR_MODE] tune-SSIM MD distortion — the block-SSIM full-loop
//! distortion the fork's `--alt-ssim-tuning` activates at PD_PASS_1
//! (SSIM_LVL_1, I-slices INCLUDED — product_coding_loop.c:10316; the
//! regular tune=SSIM/IQ arm excludes I-slices, so this knob is what makes
//! the machinery reachable on stills).
//!
//! Pieces (8-bit path — the port's envelope):
//! * [`similarity`] — C `svt_aom_similarity` (enc_dec_process.c:645), the
//!   classic SSIM ratio with count-scaled c1/c2.
//! * [`ssim_blocks`] — the 8x8 (or 4x4 for thin blocks) tiling walker
//!   (mode_decision.c ssim_8x8_blocks / ssim_4x4_blocks): per-tile score
//!   CLIP3(0,1), mean over tiles.
//! * [`spatial_full_distortion_ssim`] — C
//!   `svt_spatial_full_distortion_ssim_kernel` (mode_decision.c:4430):
//!   `(1 - ssim) * count * 100 * 7` (+ `psy_distortion * ac_bias`,
//!   truncated, when ac_bias != 0).
//!
//! Differentially tested against the exported C kernel
//! (tests/c_parity_ssim_md.rs). The per-candidate `full_cost_ssim`
//! assembly and the two-pass winner selection live in the leaf funnel.

use svtav1_dsp::ac_bias::psy_distortion;

/// C `svt_aom_similarity` for bd=8: c1/c2 are the SSIM stabilizers scaled
/// by count^2 >> 12 (cc1 = 64^2*(.01*255)^2, cc2 = 64^2*(.03*255)^2).
fn similarity(sum_s: u32, sum_r: u32, sum_sq_s: u32, sum_sq_r: u32, sum_sxr: u32, count: i64) -> f64 {
    const CC1: i64 = 26634;
    const CC2: i64 = 239708;
    let c1 = ((CC1 * count * count) >> 12) as f64;
    let c2 = ((CC2 * count * count) >> 12) as f64;
    let (sum_s, sum_r) = (f64::from(sum_s), f64::from(sum_r));
    let (sum_sq_s, sum_sq_r, sum_sxr) =
        (f64::from(sum_sq_s), f64::from(sum_sq_r), f64::from(sum_sxr));
    let count = count as f64;
    let ssim_n = (2.0 * sum_s * sum_r + c1) * (2.0 * count * sum_sxr - 2.0 * sum_s * sum_r + c2);
    let ssim_d = (sum_s * sum_s + sum_r * sum_r + c1)
        * (count * sum_sq_s - sum_s * sum_s + count * sum_sq_r - sum_r * sum_r + c2);
    ssim_n / ssim_d
}

/// One n x n tile's SSIM score (C `svt_ssim_8x8_c` / `svt_ssim_4x4_c`).
fn ssim_nxn(s: &[u8], sp: usize, r: &[u8], rp: usize, n: usize) -> f64 {
    let (mut sum_s, mut sum_r, mut sum_sq_s, mut sum_sq_r, mut sum_sxr) = (0u32, 0u32, 0u32, 0u32, 0u32);
    for i in 0..n {
        for j in 0..n {
            let sv = u32::from(s[i * sp + j]);
            let rv = u32::from(r[i * rp + j]);
            sum_s += sv;
            sum_r += rv;
            sum_sq_s += sv * sv;
            sum_sq_r += rv * rv;
            sum_sxr += sv * rv;
        }
    }
    similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, (n * n) as i64)
}

/// C `ssim()` (mode_decision.c): 8x8 tiling when both dims are multiples
/// of 8, else 4x4; per-tile CLIP3(0,1), mean over tiles.
pub fn ssim_blocks(s: &[u8], sp: usize, r: &[u8], rp: usize, width: usize, height: usize) -> f64 {
    debug_assert!(width % 4 == 0 && height % 4 == 0);
    let n = if width % 8 == 0 && height % 8 == 0 { 8 } else { 4 };
    let mut total = 0.0f64;
    let mut samples = 0u32;
    let mut i = 0;
    while i + n <= height {
        let mut j = 0;
        while j + n <= width {
            let v = ssim_nxn(&s[i * sp + j..], sp, &r[i * rp + j..], rp, n);
            total += v.clamp(0.0, 1.0);
            samples += 1;
            j += n;
        }
        i += n;
    }
    debug_assert!(samples > 0);
    total / f64::from(samples)
}

/// C `svt_spatial_full_distortion_ssim_kernel` (mode_decision.c:4430),
/// 8-bit (`m = 1`). `ac_bias` is the EFFECTIVE ac bias (the MD call sites
/// pass `get_effective_ac_bias(...)`, full_loop.c:2496).
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_ssim(
    input: &[u8],
    input_offset: usize,
    input_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
    ac_bias: f64,
) -> u64 {
    let count = (area_width * area_height) as f64;
    let ssim_score = ssim_blocks(
        &input[input_offset..],
        input_stride,
        &recon[recon_offset..],
        recon_stride,
        area_width,
        area_height,
    );
    let mut psy: u64 = 0;
    if ac_bias != 0.0 {
        let ac = psy_distortion(
            &input[input_offset..],
            input_stride,
            &recon[recon_offset..],
            recon_stride,
            area_width,
            area_height,
        );
        psy = (ac as f64 * ac_bias) as u64;
    }
    ((1.0 - ssim_score) * count * 100.0 * 7.0) as u64 + psy
}

/// C `derive_ssim_threshold_factor_for_full_md` (mode_decision.c:3862):
/// pass-two SSD-cost envelope for the SSIM winner re-pick.
pub fn ssim_threshold_factor(width: u32, height: u32) -> f64 {
    // INPUT_SIZE_1080p_RANGE: SVT classes resolutions by luma samples;
    // the 1080p range starts above 1665x1120-ish (INPUT_SIZE_720p ends at
    // 1665*1120). Use the sample-count boundary C uses.
    if u64::from(width) * u64::from(height) > 1_665 * 1_120 {
        1.02
    } else {
        1.03
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_planes_score_one() {
        let img: alloc::vec::Vec<u8> = (0..64 * 64).map(|i| (i % 251) as u8).collect();
        assert_eq!(spatial_full_distortion_ssim(&img, 0, 64, &img, 0, 64, 64, 64, 0.0), 0);
        let s = ssim_blocks(&img, 64, &img, 64, 16, 16);
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn thin_blocks_use_4x4_tiling() {
        let a: alloc::vec::Vec<u8> = (0..4 * 16).map(|i| (i * 7 % 256) as u8).collect();
        let b: alloc::vec::Vec<u8> = (0..4 * 16).map(|i| (i * 7 % 256) as u8).collect();
        let s = ssim_blocks(&a, 16, &b, 16, 16, 4);
        assert!((s - 1.0).abs() < 1e-12);
    }
}
