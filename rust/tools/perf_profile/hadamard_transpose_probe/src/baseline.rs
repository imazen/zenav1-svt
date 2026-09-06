#![forbid(unsafe_code)]
use archmage::prelude::*;

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
