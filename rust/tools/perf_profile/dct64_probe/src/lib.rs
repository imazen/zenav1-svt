#![forbid(unsafe_code)]
use archmage::prelude::*;
use svtav1_dsp::fwd_txfm::{cospi_arr, fwd_txfm_shift, FWD_COS_BIT_COL, FWD_COS_BIT_ROW};
macro_rules! c {
    ($t:expr, $cospi:expr, $k:expr) => {
        splat($t, $cospi[$k])
    };
}
/// `splat(-cospi[k])`.
macro_rules! cn {
    ($t:expr, $cospi:expr, $k:expr) => {
        splat($t, -$cospi[$k])
    };
}
macro_rules! add {
    ($a:expr, $b:expr) => {
        _mm256_add_epi32($a, $b)
    };
}
macro_rules! sub {
    ($a:expr, $b:expr) => {
        _mm256_sub_epi32($a, $b)
    };
}
#[rite]
fn splat(_t: Desktop64, v: i32) -> __m256i {
        _mm256_set1_epi32(v)
    }
#[rite]
fn hbtf(
        _t: Desktop64,
        w0: __m256i,
        n0: __m256i,
        w1: __m256i,
        n1: __m256i,
        rnd: __m256i,
        sh: __m128i,
    ) -> __m256i {
        let x = _mm256_mullo_epi32(w0, n0);
        let y = _mm256_mullo_epi32(w1, n1);
        _mm256_sra_epi32(_mm256_add_epi32(_mm256_add_epi32(x, y), rnd), sh)
    }
#[rite]
fn round_shift_v(_t: Desktop64, v: __m256i, bit: i32) -> __m256i {
        if bit > 0 {
            let b = bit as u32;
            let rnd = _mm256_set1_epi32(1 << (b - 1));
            _mm256_sra_epi32(_mm256_add_epi32(v, rnd), _mm_cvtsi32_si128(bit))
        } else if bit < 0 {
            _mm256_sll_epi32(v, _mm_cvtsi32_si128(-bit))
        } else {
            v
        }
    }
#[rite]
fn transpose8(t: Desktop64, inp: &[__m256i; 8]) -> [__m256i; 8] {
        let a0 = _mm256_unpacklo_epi32(inp[0], inp[1]);
        let a1 = _mm256_unpackhi_epi32(inp[0], inp[1]);
        let a2 = _mm256_unpacklo_epi32(inp[2], inp[3]);
        let a3 = _mm256_unpackhi_epi32(inp[2], inp[3]);
        let a4 = _mm256_unpacklo_epi32(inp[4], inp[5]);
        let a5 = _mm256_unpackhi_epi32(inp[4], inp[5]);
        let a6 = _mm256_unpacklo_epi32(inp[6], inp[7]);
        let a7 = _mm256_unpackhi_epi32(inp[6], inp[7]);
        let b0 = _mm256_unpacklo_epi64(a0, a2);
        let b1 = _mm256_unpackhi_epi64(a0, a2);
        let b2 = _mm256_unpacklo_epi64(a1, a3);
        let b3 = _mm256_unpackhi_epi64(a1, a3);
        let b4 = _mm256_unpacklo_epi64(a4, a6);
        let b5 = _mm256_unpackhi_epi64(a4, a6);
        let b6 = _mm256_unpacklo_epi64(a5, a7);
        let b7 = _mm256_unpackhi_epi64(a5, a7);
        let _ = t;
        [
            _mm256_permute2x128_si256::<0x20>(b0, b4),
            _mm256_permute2x128_si256::<0x20>(b1, b5),
            _mm256_permute2x128_si256::<0x20>(b2, b6),
            _mm256_permute2x128_si256::<0x20>(b3, b7),
            _mm256_permute2x128_si256::<0x31>(b0, b4),
            _mm256_permute2x128_si256::<0x31>(b1, b5),
            _mm256_permute2x128_si256::<0x31>(b2, b6),
            _mm256_permute2x128_si256::<0x31>(b3, b7),
        ]
    }
#[rite]
fn load8(_t: Desktop64, buf: &[i32], off: usize) -> __m256i {
        let a: &[i32; 8] = buf[off..off + 8].try_into().unwrap();
        _mm256_loadu_si256(a)
    }
#[rite]
fn store8(_t: Desktop64, buf: &mut [i32], off: usize, v: __m256i) {
        let a: &mut [i32; 8] = (&mut buf[off..off + 8]).try_into().unwrap();
        _mm256_storeu_si256(a, v);
    }
mod baseline;
mod specialized;
#[arcane]
pub fn baseline(t: X64V3Token, input: &[i32], output: &mut [i32], stride: usize) { baseline::driver(t,input,output,stride); }
#[arcane]
pub fn candidate(t: X64V3Token, input: &[i32], output: &mut [i32], stride: usize) { specialized::driver(t,input,output,stride); }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_match_real_c() {
        let t=X64V3Token::summon().unwrap();
        let mut seed=7654321u32;
        for stride in [64,71] {
            for pattern in 0..80 {
                let mut input=vec![0i32;stride*64+3];
                for x in &mut input {
                    seed=seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    *x=match pattern {0=>0,1=>255,2=>-255,_=>(seed%511) as i32-255};
                }
                let packed:Vec<i16>=(0..4096).map(|i|input[3+(i/64)*stride+i%64] as i16).collect();
                let expected=svtav1_cref::fwd_txfm2d(64,&packed,0);
                let mut a=vec![123456;4100];let mut b=a.clone();
                baseline(t,&input[3..],&mut a[2..4098],stride);
                candidate(t,&input[3..],&mut b[2..4098],stride);
                assert_eq!(a,b,"stride={stride} pattern={pattern}");
                assert_eq!(&a[2..4098],expected.as_slice());
                assert_eq!(&a[..2],&[123456;2]);assert_eq!(&a[4098..],&[123456;2]);
            }
        }
    }
}
