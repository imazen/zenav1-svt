#![forbid(unsafe_code)]
use archmage::prelude::*;
static DR_INTRA_DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

fn get_dx(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(180 - angle) as usize] as i32
    } else {
        1
    }
}

fn get_dy(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE[(270 - angle) as usize] as i32
    } else {
        1
    }
}

#[arcane]
pub fn row_split(
    _token: X64V3Token,
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    ua: bool,
    ul: bool,
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);
    match (ua, ul) {
        (false, false) => {
            split::<false, false>(dst, stride, above, left, origin, width, height, dx, dy)
        }
        (false, true) => {
            split::<false, true>(dst, stride, above, left, origin, width, height, dx, dy)
        }
        (true, false) => {
            split::<true, false>(dst, stride, above, left, origin, width, height, dx, dy)
        }
        (true, true) => {
            split::<true, true>(dst, stride, above, left, origin, width, height, dx, dy)
        }
    }
}

#[inline(always)]
fn split<const UA: bool, const UL: bool>(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    w: usize,
    h: usize,
    dx: i32,
    dy: i32,
) {
    let step = 1usize << (UA as usize);
    for r in 0..h {
        let x = -((r as i32 + 1) * dx);
        let base = x >> (6 - UA as u32);
        let first =
            ((-(step as i32) - base + step as i32 - 1) >> (UA as u32)).clamp(0, w as i32) as usize;
        let row = &mut dst[r * stride..r * stride + w];
        for (c, out) in row[..first].iter_mut().enumerate() {
            let y = ((r as i32) << 6) - (c as i32 + 1) * dy;
            let i = (origin as i32 + (y >> (6 - UL as u32))) as usize;
            let shift = ((y << (UL as u32)) & 63) >> 1;
            *out = ((i32::from(left[i]) * (32 - shift) + i32::from(left[i + 1]) * shift + 16) >> 5)
                as u8;
        }
        if first < w {
            let begin = (origin as i32 + base + (first * step) as i32) as usize;
            let len = w - first;
            let source = &above[begin..begin + (len - 1) * step + 2];
            let shift = ((x << (UA as u32)) & 63) >> 1;
            for (out, pair) in row[first..].iter_mut().zip(source.windows(2).step_by(step)) {
                *out = ((i32::from(pair[0]) * (32 - shift) + i32::from(pair[1]) * shift + 16) >> 5)
                    as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use svtav1_dsp::intra_pred as ip;
    #[test]
    fn split_matches_c_all_zone2_angles() {
        let token = X64V3Token::summon().expect("native AVX2 probe");
        let mut cases = 0;
        for (w, h) in [
            (4, 4),
            (8, 8),
            (16, 16),
            (32, 32),
            (64, 64),
            (4, 8),
            (8, 4),
            (4, 16),
            (16, 4),
            (8, 16),
            (16, 8),
            (8, 32),
            (32, 8),
            (16, 32),
            (32, 16),
            (16, 64),
            (64, 16),
            (32, 64),
            (64, 32),
        ] {
            for base in [113, 135, 157] {
                for delta in -3..=3 {
                    let angle = base + 3 * delta;
                    for filt in 0..=1 {
                        let ua = ip::use_intra_edge_upsample(w as i32, h as i32, angle - 90, filt);
                        let ul = ip::use_intra_edge_upsample(h as i32, w as i32, angle - 180, filt);
                        for seed in 0..3usize {
                            let mut a = [0u8; 160];
                            let mut l = [0u8; 160];
                            for i in 0..160 {
                                a[i] = (i * 31 + seed * 79) as u8;
                                l[i] = (i * 53 + seed * 19) as u8;
                            }
                            l[15] = a[15];
                            let stride = w + 7;
                            let mut actual = vec![219; stride * h + 3];
                            let mut expected = actual.clone();
                            row_split(token, &mut actual, stride, &a, &l, 16, ua, ul, w, h, angle);
                            svtav1_cref::dr_predictor_edged(
                                &mut expected,
                                stride,
                                &a,
                                &l,
                                16,
                                ua,
                                ul,
                                w,
                                h,
                                angle,
                            );
                            assert_eq!(
                                actual, expected,
                                "{w}x{h} angle{angle} ua{ua} ul{ul} seed{seed}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 19 * 3 * 7 * 2 * 3);
    }
}
