#![forbid(unsafe_code)]
use archmage::prelude::*;
pub static DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

#[inline(always)]
fn old_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    let up = upsample_above as i32;
    let max_base_x = ((bw + bh) as i32 - 1) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut x = dx;
    for r in 0..bh {
        let mut base = x >> frac_bits;
        let shift = ((x << up) & 0x3F) >> 1;
        if base >= max_base_x {
            let fill = above[origin + max_base_x as usize];
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        for c in 0..bw {
            let v = if base < max_base_x {
                let val = above[origin + base as usize] as i32 * (32 - shift)
                    + above[origin + base as usize + 1] as i32 * shift;
                ((val + 16) >> 5).clamp(0, 255) as u8
            } else {
                above[origin + max_base_x as usize]
            };
            dst[r * dst_stride + c] = v;
            base += base_inc;
        }
        x += dx;
    }
}

pub fn baseline(
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    edge: &[u8],
    origin: usize,
    dx: i32,
) {
    old_core(dst, stride, w, h, edge, origin, false, dx);
}
pub fn candidate(
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    edge: &[u8],
    origin: usize,
    dx: i32,
) {
    incant!(split(dst, stride, w, h, edge, origin, dx), [v3, scalar]);
}
fn split_scalar(
    _t: ScalarToken,
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    edge: &[u8],
    origin: usize,
    dx: i32,
) {
    baseline(dst, stride, w, h, edge, origin, dx);
}
#[arcane]
fn split_v3(
    _t: X64V3Token,
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    edge: &[u8],
    origin: usize,
    dx: i32,
) {
    let max_base = (w + h - 1) as i32;
    let fill = edge[origin + max_base as usize];
    let mut x = dx;
    for r in 0..h {
        let base = x >> 6;
        let shift = ((x & 63) >> 1) as u16;
        let valid = if base >= max_base {
            0
        } else {
            w.min((max_base - base) as usize)
        };
        let row = &mut dst[r * stride..r * stride + w];
        if valid > 0 {
            let bi = origin + base as usize;
            for (out, (&a, &b)) in row[..valid].iter_mut().zip(
                edge[bi..bi + valid]
                    .iter()
                    .zip(&edge[bi + 1..bi + valid + 1]),
            ) {
                *out = ((u16::from(a) * (32 - shift) + u16::from(b) * shift + 16) >> 5) as u8;
            }
        }
        row[valid..].fill(fill);
        x += dx;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zone1_matches_c() {
        let _lock = archmage::testing::lock_token_testing();
        X64V3Token::summon().expect("V3 probe");
        let mut checked = 0;
        for (w, h) in [
            (4, 4),
            (8, 8),
            (16, 4),
            (16, 8),
            (16, 16),
            (32, 8),
            (32, 16),
            (32, 32),
            (64, 16),
            (64, 32),
            (64, 64),
        ] {
            for stride in [w, w + 7] {
                for angle in 1..90 {
                    let dx = DERIVATIVE[angle];
                    if dx == 0 {
                        continue;
                    }
                    for pattern in 0..5 {
                        let edge: Vec<u8> = (0..16 + w + h + 1)
                            .map(|i| match pattern {
                                0 => 0,
                                1 => 255,
                                2 => {
                                    if i % 2 == 0 {
                                        0
                                    } else {
                                        255
                                    }
                                }
                                _ => ((i * 7919 + pattern * 997) % 256) as u8,
                            })
                            .collect();
                        let mut c = vec![77; stride * h + 9];
                        let mut b = c.clone();
                        let mut a = c.clone();
                        svtav1_cref::dr_predictor_edged(
                            &mut c[3..],
                            stride,
                            &edge,
                            &edge,
                            16,
                            false,
                            false,
                            w,
                            h,
                            angle as i32,
                        );
                        baseline(&mut b[3..], stride, w, h, &edge, 16, dx as i32);
                        candidate(&mut a[3..], stride, w, h, &edge, 16, dx as i32);
                        assert_eq!(
                            a, c,
                            "candidate {w}x{h} s={stride} angle={angle} pattern={pattern}"
                        );
                        assert_eq!(b, c, "baseline");
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 2000);
        eprintln!("{checked} C cases");
    }
}
