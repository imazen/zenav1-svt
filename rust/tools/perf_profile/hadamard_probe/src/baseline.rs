use archmage::prelude::*;
// Frozen production algorithm from 9adba930; the benchmark keeps the runtime
// entry used before the V3 rewrite, independently of the current DSP crate.
pub fn run(input: &[i16], stride: usize, output: &mut [i32]) {
    incant!(baseline_impl(input, stride, output), [v3, scalar]);
}
#[arcane]
fn baseline_impl_v3(_token: X64V3Token, input: &[i16], stride: usize, output: &mut [i32]) {
    aom_hadamard_8x8_core(input, stride, output);
}
fn baseline_impl_scalar(_token: ScalarToken, input: &[i16], stride: usize, output: &mut [i32]) {
    aom_hadamard_8x8_core(input, stride, output);
}
fn hadamard_col8(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 8]) {
    let s = |i: usize| src_diff[i * src_stride] as i32;
    let b0 = s(0) + s(1);
    let b1 = s(0) - s(1);
    let b2 = s(2) + s(3);
    let b3 = s(2) - s(3);
    let b4 = s(4) + s(5);
    let b5 = s(4) - s(5);
    let b6 = s(6) + s(7);
    let b7 = s(6) - s(7);

    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;

    coeff[0] = (c0 + c4) as i16;
    coeff[7] = (c1 + c5) as i16;
    coeff[3] = (c2 + c6) as i16;
    coeff[4] = (c3 + c7) as i16;
    coeff[2] = (c0 - c4) as i16;
    coeff[6] = (c1 - c5) as i16;
    coeff[1] = (c2 - c6) as i16;
    coeff[5] = (c3 - c7) as i16;
}

fn aom_hadamard_8x8_core(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 64];
    let mut buffer2 = [0i16; 64];
    // Column pass: one butterfly per column, walking columns left→right.
    for idx in 0..8 {
        let col = &src_diff[idx..];
        let mut out = [0i16; 8];
        hadamard_col8(col, src_stride, &mut out);
        buffer[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    // Row pass over the transposed intermediate.
    for idx in 0..8 {
        let mut out = [0i16; 8];
        hadamard_col8(&buffer[idx..], 8, &mut out);
        buffer2[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    for idx in 0..64 {
        coeff[idx] = buffer2[idx] as i32;
    }
}
