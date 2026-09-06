#![forbid(unsafe_code)]
use archmage::prelude::*;
pub mod baseline;

use magetypes::simd::generic::i16x8;
type V = i16x8<X64V3Token>;

#[rite]
fn vertical(_token: X64V3Token, s: [V; 8]) -> [V; 8] {
    let b0 = s[0] + s[1];
    let b1 = s[0] - s[1];
    let b2 = s[2] + s[3];
    let b3 = s[2] - s[3];
    let b4 = s[4] + s[5];
    let b5 = s[4] - s[5];
    let b6 = s[6] + s[7];
    let b7 = s[6] - s[7];
    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;
    [
        c0 + c4,
        c2 - c6,
        c0 - c4,
        c2 + c6,
        c3 + c7,
        c3 - c7,
        c1 - c5,
        c1 + c5,
    ]
}

#[rite]
fn transpose(token: X64V3Token, v: [V; 8]) -> [V; 8] {
    let rows = v.map(|r| r.to_array());
    core::array::from_fn(|c| V::from_array(token, core::array::from_fn(|r| rows[r][c])))
}

#[arcane]
pub fn candidate(token: X64V3Token, input: &[i16], stride: usize, output: &mut [i32]) {
    let rows = core::array::from_fn(|r| {
        V::load(token, input[r * stride..r * stride + 8].try_into().unwrap())
    });
    let first = transpose(token, vertical(token, rows));
    let result = transpose(token, vertical(token, first));
    let output: &mut [i32; 64] = (&mut output[..64]).try_into().unwrap();
    for (r, row) in result.into_iter().enumerate() {
        output[r * 8..r * 8 + 8].copy_from_slice(&row.to_array().map(i32::from));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hadamard_matches_real_c_with_padding_and_wrap() {
        let token = X64V3Token::summon().expect("AVX2 probe");
        let mut seed = 0x9827aa65u32;
        let mut cases = 0;
        for stride in [8, 11, 16, 32] {
            for pattern in 0..300 {
                let mut source = vec![0i16; stride * 8 + 9];
                for (i, v) in source.iter_mut().enumerate() {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *v = match pattern {
                        0 => 0,
                        1 => i16::MAX,
                        2 => i16::MIN,
                        3 => {
                            if i % 2 == 0 {
                                i16::MIN
                            } else {
                                i16::MAX
                            }
                        }
                        4..=67 => {
                            if i == pattern - 4 {
                                1023
                            } else {
                                0
                            }
                        }
                        68..=99 => (seed % 511) as i16 - 255,
                        100..=199 => (seed % 2047) as i16 - 1023,
                        _ => seed as i16,
                    };
                }
                let mut got = [123456i32; 72];
                let mut expected = got;
                candidate(token, &source[3..], stride, &mut got[2..]);
                let mut original = expected;
                baseline::run(&source[3..], stride, &mut original[2..]);
                assert_eq!(got, original);
                svtav1_cref::hadamard(8, &source[3..], stride, &mut expected[2..]);
                assert_eq!(got, expected, "stride={stride} pattern={pattern}");
                cases += 1;
            }
        }
        assert_eq!(cases, 1200);
    }
}
