//! [SVT_HDR_MODE] tune-SSIM MD distortion differential: the Rust
//! `ssim_md::spatial_full_distortion_ssim` vs the REAL exported C
//! `svt_spatial_full_distortion_ssim_kernel` across block dims (8x8-tiled
//! and thin 4x4-tiled shapes), randomized content, and ac_bias values
//! (0 = pure SSIM; nonzero adds the truncated psy term).

use svtav1_cref as cref;
use svtav1_encoder::ssim_md;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
}

#[test]
fn ssim_md_kernel_matches_c() {
    let mut rng = Rng(0x551_2026_0717);
    let mut cells = 0usize;
    // (w, h): square 8x8-tiled, rectangular, and thin 4x4-tiled shapes.
    for (w, h) in [
        (8usize, 8usize),
        (16, 16),
        (32, 32),
        (64, 64),
        (16, 8),
        (8, 32),
        (4, 16),
        (16, 4),
        (4, 4),
        (32, 8),
    ] {
        let stride_in = w + 11; // strided rows, not tight
        let stride_rec = w + 5;
        for &ac_bias in &[0.0f64, 0.3, 1.0, 2.5] {
            for _ in 0..6 {
                let input: Vec<u8> = (0..stride_in * h).map(|_| rng.byte()).collect();
                // recon = input +- small noise so ssim is in a realistic range
                let recon: Vec<u8> = (0..stride_rec * h)
                    .map(|i| {
                        let r = i % stride_rec;
                        if r < w {
                            let v = input[(i / stride_rec) * stride_in + r];
                            v.saturating_add((rng.next() % 7) as u8).saturating_sub(3)
                        } else {
                            rng.byte()
                        }
                    })
                    .collect();
                let c = cref::spatial_full_distortion_ssim(
                    &input, 0, stride_in, &recon, 0, stride_rec, w, h, ac_bias,
                );
                let r = ssim_md::spatial_full_distortion_ssim(
                    &input, 0, stride_in, &recon, 0, stride_rec, w, h, ac_bias,
                );
                assert_eq!(r, c, "{w}x{h} ac_bias {ac_bias}");
                cells += 1;
            }
        }
    }
    println!("ssim-md parity: {cells} cells");
}
