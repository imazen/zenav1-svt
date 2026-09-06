#![forbid(unsafe_code)]
use archmage::prelude::*;
#[inline(always)]
fn wide_core(input: &[i32]) -> i64 {
    input.iter().map(|&x| i64::from(x.unsigned_abs())).sum()
}
#[inline(always)]
fn narrow_core(input: &[i32]) -> i32 {
    input.iter().map(|&x| x.abs()).sum()
}
#[inline(never)]
pub fn wide_baseline(_: X64V3Token, input: &[i32]) -> i64 {
    wide_core(input)
}
#[inline(never)]
pub fn narrow_baseline(_: X64V3Token, input: &[i32]) -> i64 {
    i64::from(narrow_core(input))
}
pub fn wide(_: X64V3Token, input: &[i32]) -> i64 {
    incant!(wide_impl(input), [v3, scalar])
}
pub fn narrow(_: X64V3Token, input: &[i32]) -> i64 {
    i64::from(incant!(narrow_impl(input), [v3, scalar]))
}
#[arcane]
fn wide_impl_v3(_: X64V3Token, input: &[i32]) -> i64 {
    wide_core(input)
}
fn wide_impl_scalar(_: ScalarToken, input: &[i32]) -> i64 {
    wide_core(input)
}
#[arcane]
fn narrow_impl_v3(_: X64V3Token, input: &[i32]) -> i32 {
    narrow_core(input)
}
fn narrow_impl_scalar(_: ScalarToken, input: &[i32]) -> i32 {
    narrow_core(input)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c_range_and_wide_edges() {
        let t = X64V3Token::summon().unwrap();
        let mut seed = 7654321u32;
        for n in [0, 1, 7, 8, 15, 16, 17, 32, 63, 64, 65, 256, 1024] {
            for _ in 0..30 {
                let input: Vec<i32> = (0..n + 3)
                    .map(|_| {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        (seed % 65281) as i32 - 32640
                    })
                    .collect();
                let input = &input[3..];
                let expected = i64::from(svtav1_cref::satd(input));
                assert_eq!(wide(t, input), expected);
                assert_eq!(narrow(t, input), expected);
                assert_eq!(wide_baseline(t, input), expected);
                assert_eq!(narrow_baseline(t, input), expected);
            }
        }
        for n in [1, 4, 16, 64, 256, 1024] {
            let input: Vec<i32> = [i32::MIN, i32::MAX, -1, 0].repeat(n);
            assert_eq!(wide(t, &input), 4_294_967_296i64 * n as i64);
            assert_eq!(wide_baseline(t, &input), 4_294_967_296i64 * n as i64);
        }
    }
}
