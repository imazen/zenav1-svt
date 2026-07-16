//! Differential parity for the delta-q L2 building blocks:
//! the sub-sampled 8x8 mean/mean-square producers vs the exported C
//! functions, plus a whole-SB variance cross-check assembled from them.
use svtav1_cref as cref;
use svtav1_encoder::sb_qindex;

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
fn sub_mean_producers_match_c() {
    let mut rng = Rng(0x5b91dec5);
    for _ in 0..200 {
        let stride = 8 + (rng.next() as usize % 24);
        let buf: Vec<u8> = (0..stride * 8).map(|_| (rng.next() >> 32) as u8).collect();
        // Rust-side recomputation with the sb_qindex internal formula via
        // compute_sb_variances on an 8x8-only view is indirect; instead
        // recompute here exactly and pin BOTH against C.
        let mut s: u64 = 0;
        let mut sq: u64 = 0;
        for vi in 0..4 {
            for hi in 0..8 {
                let p = u64::from(buf[(2 * vi) * stride + hi]);
                s += p;
                sq += p * p;
            }
        }
        assert_eq!(s << 3, cref::sub_mean_8x8(&buf, stride as u16));
        assert_eq!(sq << 11, cref::sub_mean_squared_8x8(&buf, stride as u32));
    }
}

#[test]
fn sb_variance_producer_consistent_with_c_blocks() {
    // Assemble a full 64x64 SB and verify compute_sb_variances' 8x8 level
    // equals the C producers combined with the fork SVT_VAR_STORE formula.
    let mut rng = Rng(0xfeed5b);
    let stride = 80usize;
    let buf: Vec<u8> = (0..stride * 64).map(|_| (rng.next() >> 32) as u8).collect();
    let v = sb_qindex::compute_sb_variances(&buf, stride, 64, 64, 0, 0);
    for row in 0..8 {
        for col in 0..8 {
            let blk = &buf[row * 8 * stride + col * 8..];
            let m = cref::sub_mean_8x8(blk, stride as u16);
            let sq = cref::sub_mean_squared_8x8(blk, stride as u32);
            let expect = (sq as i64 - (m * m) as i64) as f64 / 65536.0;
            assert_eq!(v.var_8x8[row * 8 + col], expect, "blk ({row},{col})");
        }
    }
}
