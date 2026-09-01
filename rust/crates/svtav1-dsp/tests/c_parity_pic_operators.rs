//! Differential parity for the `Codec/pic_operators.c` pixel kernels vs the
//! real exported C symbols — evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! These four are on the hot path of every candidate the mode search
//! evaluates: the residual kernels produce the input to the forward
//! transform, and the distortion kernels produce the D of every RD cost. A
//! stride or narrowing bug in any of them is a wrong pick, not a crash, so
//! each is fuzzed over AV1 block shapes on STRIDED buffers (padded stride +
//! non-zero origin) rather than tight-packed ones.
//!
//! `picture_full_distortion32_bits_single` is the DISPATCHED entry point:
//! it reaches `svt_full_distortion_kernel32_bits` / `_cbf_zero32_bits`,
//! which are RTCD pointers on x86-64 (devirtualized `#define`s on aarch64).
//! `rtcd_ready_is_a_real_positive_control` asserts the slots are bound
//! before any of that is believed.

use svtav1_cref as cref;
use svtav1_dsp::pic_operators as po;

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
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// AV1 transform / block shapes these kernels are called on.
const SHAPES: &[(usize, usize)] = &[
    (4, 4),
    (8, 8),
    (16, 16),
    (32, 32),
    (64, 64),
    (4, 8),
    (8, 4),
    (16, 8),
    (8, 16),
    (32, 16),
    (16, 32),
    (64, 32),
    (32, 64),
    (4, 16),
    (16, 4),
    (8, 32),
    (32, 8),
];

#[test]
fn rtcd_ready_is_a_real_positive_control() {
    assert!(
        cref::pic_operators::rtcd_ready(),
        "the dispatched kernels this file compares against are unbound; every \
         match below would be meaningless (WORKING-ON-THIS §5 trap 2)"
    );
}

#[test]
fn residual_kernel8bit_matches_c() {
    let mut rng = Rng(0x5EED_1234);
    for &(w, h) in SHAPES {
        for pad in [0usize, 7] {
            let in_stride = w + pad;
            let pred_stride = w + 2 * pad;
            let res_stride = w + pad / 2;
            let input: Vec<u8> = (0..in_stride * h).map(|_| rng.next() as u8).collect();
            let pred: Vec<u8> = (0..pred_stride * h).map(|_| rng.next() as u8).collect();

            let mut mine = vec![0i16; res_stride * h];
            let mut theirs = vec![0i16; res_stride * h];
            po::residual_kernel_8bit(
                &input,
                in_stride,
                &pred,
                pred_stride,
                &mut mine,
                res_stride,
                w,
                h,
            );
            cref::pic_operators::residual_kernel8bit(
                &input,
                in_stride,
                &pred,
                pred_stride,
                &mut theirs,
                res_stride,
                w,
                h,
            );
            assert_eq!(mine, theirs, "residual8 {w}x{h} pad {pad}");
        }
    }
}

#[test]
fn residual_kernel16bit_matches_c_including_the_full_u16_range() {
    let mut rng = Rng(0xABCD_0F0F);
    for &(w, h) in SHAPES {
        // Two regimes: the encoder's own envelope (10-bit samples) and the
        // full u16 range, which is where C's two implementation-defined
        // int16_t narrowings become observable.
        for &max in &[1024u64, 65_536] {
            let in_stride = w + 5;
            let pred_stride = w + 1;
            let res_stride = w;
            let input: Vec<u16> = (0..in_stride * h).map(|_| rng.below(max) as u16).collect();
            let pred: Vec<u16> = (0..pred_stride * h)
                .map(|_| rng.below(max) as u16)
                .collect();

            let mut mine = vec![0i16; res_stride * h];
            let mut theirs = vec![0i16; res_stride * h];
            po::residual_kernel_16bit(
                &input,
                in_stride,
                &pred,
                pred_stride,
                &mut mine,
                res_stride,
                w,
                h,
            );
            cref::pic_operators::residual_kernel16bit(
                &input,
                in_stride,
                &pred,
                pred_stride,
                &mut theirs,
                res_stride,
                w,
                h,
            );
            assert_eq!(mine, theirs, "residual16 {w}x{h} max {max}");
        }
    }
}

#[test]
fn full_distortion_kernel32_bits_matches_c() {
    let mut rng = Rng(0x1357_9BDF);
    for &(w, h) in SHAPES {
        let stride = w + 3;
        // Coefficient magnitudes: AV1 TranLow is int32 but the encoder's
        // range after quantization is far smaller. `1 << 30` is the largest
        // magnitude for which C's own `(int64_t)d * d` stays DEFINED
        // (|d| <= 2^31 => d*d <= 2^62); beyond it C is signed-overflow UB
        // and there is no oracle to compare against, so the differential
        // deliberately stops there. The port's `wrapping_mul` covers the
        // rest without a panic (see the module doc).
        for &mag in &[1i64 << 10, 1i64 << 30] {
            let coeff: Vec<i32> = (0..stride * h)
                .map(|_| (rng.below(2 * mag as u64) as i64 - mag) as i32)
                .collect();
            let recon: Vec<i32> = (0..stride * h)
                .map(|_| (rng.below(2 * mag as u64) as i64 - mag) as i32)
                .collect();

            let mine = po::full_distortion_kernel32_bits(&coeff, &recon, stride, w, h);
            let (res, pred) =
                cref::pic_operators::full_distortion_kernel32_bits(&coeff, &recon, stride, w, h);
            assert_eq!(
                (mine.residual, mine.prediction),
                (res, pred),
                "fulldist32 {w}x{h} mag {mag}"
            );

            let mine_z = po::full_distortion_kernel_cbf_zero32_bits(&coeff, stride, w, h);
            let (rz, pz) =
                cref::pic_operators::full_distortion_kernel_cbf_zero32_bits(&coeff, stride, w, h);
            assert_eq!(
                (mine_z.residual, mine_z.prediction),
                (rz, pz),
                "cbfzero32 {w}x{h} mag {mag}"
            );
        }
    }
}

#[test]
fn picture_full_distortion32_bits_single_matches_c_on_both_arms() {
    let mut rng = Rng(0x2468_ACE0);
    for &(w, h) in SHAPES {
        let stride = w + 1;
        let coeff: Vec<i32> = (0..stride * h)
            .map(|_| (rng.below(8192) as i64 - 4096) as i32)
            .collect();
        let recon: Vec<i32> = (0..stride * h)
            .map(|_| (rng.below(8192) as i64 - 4096) as i32)
            .collect();
        for &nz in &[0u32, 1, 37] {
            let mine =
                po::picture_full_distortion32_bits_single(&coeff, &recon, stride, w, h, nz != 0);
            let (res, pred) = cref::pic_operators::picture_full_distortion32_bits_single(
                &coeff, &recon, stride, w, h, nz,
            );
            assert_eq!(
                (mine.residual, mine.prediction),
                (res, pred),
                "single {w}x{h} nz {nz}"
            );
        }
    }
}

#[test]
fn spatial_full_distortion_kernel_matches_c() {
    let mut rng = Rng(0x0BAD_F00D);
    for &(w, h) in SHAPES {
        let in_stride = w + 9;
        let recon_stride = w + 4;
        let in_off = in_stride + 3;
        let recon_off = recon_stride * 2;
        let input: Vec<u8> = (0..in_off + in_stride * h + w)
            .map(|_| rng.next() as u8)
            .collect();
        let recon: Vec<u8> = (0..recon_off + recon_stride * h + w)
            .map(|_| rng.next() as u8)
            .collect();

        let mine = po::spatial_full_distortion_kernel(
            &input,
            in_off,
            in_stride,
            &recon,
            recon_off,
            recon_stride,
            w,
            h,
        );
        let theirs = cref::pic_operators::spatial_full_distortion_kernel_c(
            &input,
            in_off,
            in_stride,
            &recon,
            recon_off,
            recon_stride,
            w,
            h,
        );
        assert_eq!(mine, theirs, "spatial {w}x{h}");
    }
}
