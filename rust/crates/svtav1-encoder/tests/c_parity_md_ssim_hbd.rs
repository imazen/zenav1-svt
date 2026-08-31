//! Differential parity: the high-bit-depth tune-SSIM distortion
//! (`svtav1-encoder/src/port_md/ssim_hbd.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `svt_aom_similarity` | enc_dec_process.c:645 |
//! | `svt_ssim_4x4_hbd_c` | mode_decision.c:4220 |
//! | `svt_ssim_8x8_hbd_c` | mode_decision.c:4245 |
//! | `svt_spatial_full_distortion_ssim_kernel` (hbd arm) | mode_decision.c:4372 |
//!
//! The last one covers the `static` `ssim_hbd`, `ssim_8x8_blocks_hbd` and
//! `ssim_4x4_blocks_hbd` through the exported entry point, so all seven C
//! functions in this group reach an oracle.
//!
//! `ac_bias` is compared at 0 only: a non-zero bias makes C call
//! `svt_psy_distortion_hbd`, which needs the high-bit-depth Hadamard
//! kernels this port does not have (see the module doc). The kernel's
//! `+ psy` step IS covered — by a unit test in the module that pins the
//! addition given a supplied AC distortion.
//!
//! Comparisons are BIT-EXACT on the `f64`s: both sides do the same
//! sequence of double operations, so anything less would hide a real
//! divergence.

use svtav1_cref::mode_decision as cmd;
use svtav1_encoder::port_md::ssim_hbd as rss;

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

#[test]
fn similarity_matches_c_at_every_bit_depth() {
    let mut rng = Rng(0x5511_2026_0831_0007);
    let mut checked = 0usize;
    for _ in 0..4000 {
        // Sums in the range real 8/10/12-bit tiles produce.
        let n = [16i32, 64][rng.below(2) as usize];
        let maxv = [255u32, 1023, 4095][rng.below(3) as usize];
        let mut sum_s = 0u32;
        let mut sum_r = 0u32;
        let mut sum_sq_s = 0u32;
        let mut sum_sq_r = 0u32;
        let mut sum_sxr = 0u32;
        for _ in 0..n {
            let a = rng.below(u64::from(maxv) + 1) as u32;
            let b = rng.below(u64::from(maxv) + 1) as u32;
            sum_s += a;
            sum_r += b;
            sum_sq_s += a * a;
            sum_sq_r += b * b;
            sum_sxr += a * b;
        }
        for bd in [8u32, 10, 12] {
            let c = cmd::similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, n, bd);
            let r = rss::similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, i64::from(n), bd);
            assert_eq!(
                c.to_bits(),
                r.to_bits(),
                "svt_aom_similarity(sums={sum_s},{sum_r},{sum_sq_s},{sum_sq_r},{sum_sxr}, \
                 count={n}, bd={bd})"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4000 * 3);
}

#[test]
fn ssim_tile_kernels_match_c() {
    let mut rng = Rng(0x7711_2026_0831_0007);
    let mut nonzero_spread = 0usize;
    for trial in 0..500 {
        let stride = 24usize;
        let s: Vec<u16> = (0..stride * 16).map(|_| rng.below(1024) as u16).collect();
        let r: Vec<u16> = if trial % 3 == 0 {
            s.clone()
        } else {
            (0..stride * 16).map(|_| rng.below(1024) as u16).collect()
        };
        let c4 = cmd::ssim_4x4_hbd(&s, stride, &r, stride);
        let p4 = rss::ssim_4x4_hbd(&s, stride, &r, stride);
        assert_eq!(
            c4.to_bits(),
            p4.to_bits(),
            "svt_ssim_4x4_hbd_c trial {trial}"
        );

        let c8 = cmd::ssim_8x8_hbd(&s, stride, &r, stride);
        let p8 = rss::ssim_8x8_hbd(&s, stride, &r, stride);
        assert_eq!(
            c8.to_bits(),
            p8.to_bits(),
            "svt_ssim_8x8_hbd_c trial {trial}"
        );

        if (c4 - c8).abs() > 1e-9 {
            nonzero_spread += 1;
        }
    }
    // Positive control: the two kernels must actually disagree on the
    // same data, or a port that confused them would pass.
    assert!(
        nonzero_spread > 200,
        "positive control: 4x4 and 8x8 agreed on {} of 500 cases",
        500 - nonzero_spread
    );
}

#[test]
fn spatial_full_distortion_ssim_hbd_matches_c() {
    let mut rng = Rng(0x9911_2026_0831_0007);
    // Both tiling arms: 8-multiple dims take ssim_8x8_blocks_hbd, the
    // rest take ssim_4x4_blocks_hbd.
    let sizes: [(usize, usize); 8] = [
        (4, 4),
        (8, 8),
        (12, 8),
        (8, 12),
        (16, 16),
        (16, 4),
        (32, 32),
        (64, 64),
    ];
    let mut nonzero = 0usize;
    let mut checked = 0usize;
    for (w, h) in sizes {
        for trial in 0..24 {
            let stride = w + 16;
            let n = stride * (h + 8);
            let input: Vec<u16> = (0..n).map(|_| rng.below(1024) as u16).collect();
            let recon: Vec<u16> = if trial % 4 == 0 {
                input.clone()
            } else {
                input
                    .iter()
                    .map(|&v| {
                        let d = rng.below(64) as i32 - 32;
                        (i32::from(v) + d).clamp(0, 1023) as u16
                    })
                    .collect()
            };
            let in_off = stride * 2 + 3;
            let rec_off = stride + 5;

            let c = cmd::spatial_full_distortion_ssim_hbd(
                &input, in_off, stride, &recon, rec_off, stride, w, h, 0.0,
            );
            let p = rss::spatial_full_distortion_ssim_hbd(
                &input, in_off, stride, &recon, rec_off, stride, w, h, 0.0, 0,
            );
            assert_eq!(
                c, p,
                "svt_spatial_full_distortion_ssim_kernel(hbd) {w}x{h} trial {trial}"
            );
            if c != 0 {
                nonzero += 1;
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 8 * 24);
    // Positive control: a kernel that always returned 0 would match a
    // port that always returned 0.
    assert!(
        nonzero > checked / 2,
        "only {nonzero} of {checked} were non-zero"
    );
}
