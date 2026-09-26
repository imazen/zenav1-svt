use super::*;
use alloc::vec;

/// A literal transcription of C `svt_av1_convolve_2d_sr_c` restricted
/// to the BILINEAR kernel at subpel {0,8} — the oracle the shortcut
/// arithmetic above must match bit-for-bit. 8-tap loop with the real
/// zero taps, real offsets (fo=3), real two-stage rounding.
fn convolve_oracle(
    src: &[u8],
    stride: usize,
    sx: usize,
    sy: usize,
    w: usize,
    h: usize,
    subpel_x: bool,
    subpel_y: bool,
) -> vec::Vec<u8> {
    const FILTER_BITS: i32 = 7;
    const ROUND_0: i32 = 3;
    const ROUND_1: i32 = 11;
    let taps_at = |half: bool| -> [i32; 8] {
        if half {
            [0, 0, 0, 64, 64, 0, 0, 0]
        } else {
            [0, 0, 0, 128, 0, 0, 0, 0]
        }
    };
    let round_pot = |v: i32, n: i32| -> i32 { (v + (1 << (n - 1))) >> n };
    let xf = taps_at(subpel_x);
    let yf = taps_at(subpel_y);
    let px = |x: i64, y: i64| -> i32 { i32::from(src[y as usize * stride + x as usize]) };
    let mut out = vec![0u8; w * h];
    if subpel_x && subpel_y {
        // 2D: horizontal into im (h + 7 rows from sy - 3), then vertical.
        let bd = 8;
        let im_h = h + 7;
        let mut im = vec![0i32; im_h * w];
        for y in 0..im_h {
            for x in 0..w {
                let mut sum = 1 << (bd + FILTER_BITS - 1);
                for (k, &t) in xf.iter().enumerate() {
                    if t != 0 {
                        sum += t * px(
                            sx as i64 + x as i64 - 3 + k as i64,
                            sy as i64 + y as i64 - 3,
                        );
                    }
                }
                im[y * w + x] = round_pot(sum, ROUND_0);
            }
        }
        let offset_bits = bd + 2 * FILTER_BITS - ROUND_0;
        for y in 0..h {
            for x in 0..w {
                let mut sum = 1 << offset_bits;
                for (k, &t) in yf.iter().enumerate() {
                    if t != 0 {
                        sum += t * im[(y + k) * w + x];
                    }
                }
                let res = round_pot(sum, ROUND_1)
                    - ((1 << (offset_bits - ROUND_1)) + (1 << (offset_bits - ROUND_1 - 1)));
                // bits = 2*FILTER_BITS - ROUND_0 - ROUND_1 = 0.
                out[y * w + x] = res.clamp(0, 255) as u8;
            }
        }
    } else if subpel_x {
        for y in 0..h {
            for x in 0..w {
                let mut res = 0i32;
                for (k, &t) in xf.iter().enumerate() {
                    if t != 0 {
                        res += t * px(sx as i64 + x as i64 - 3 + k as i64, sy as i64 + y as i64);
                    }
                }
                let res = round_pot(res, ROUND_0);
                out[y * w + x] = round_pot(res, FILTER_BITS - ROUND_0).clamp(0, 255) as u8;
            }
        }
    } else if subpel_y {
        for y in 0..h {
            for x in 0..w {
                let mut res = 0i32;
                for (k, &t) in yf.iter().enumerate() {
                    if t != 0 {
                        res += t * px(sx as i64 + x as i64, sy as i64 + y as i64 - 3 + k as i64);
                    }
                }
                out[y * w + x] = round_pot(res, FILTER_BITS).clamp(0, 255) as u8;
            }
        }
    } else {
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = px(sx as i64 + x as i64, sy as i64 + y as i64) as u8;
            }
        }
    }
    out
}

fn lcg_frame(seed: &mut u32, n: usize) -> vec::Vec<u8> {
    (0..n)
        .map(|_| {
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (*seed >> 24) as u8
        })
        .collect()
}

#[test]
fn luma_copy_exact() {
    let mut seed = 7u32;
    let stride = 64;
    let frame = lcg_frame(&mut seed, stride * 64);
    let mut dst = vec![0u8; 8 * 8];
    // Block at (32, 32), DV (-128, -64) eighth-pel = (-16, -8) px.
    predict_intrabc_luma(
        &frame,
        stride,
        32,
        32,
        8,
        8,
        Mv { x: -128, y: -64 },
        &mut dst,
    );
    for r in 0..8 {
        for c in 0..8 {
            assert_eq!(dst[r * 8 + c], frame[(24 + r) * stride + 16 + c]);
        }
    }
}

/// The chunk-7 pin for map §F.12: every subpel case of the chroma
/// predictor must match the literal C convolve transcription over
/// randomized content — including the "odd DV" (half-pel) cases whose
/// kernel row-select C hardcodes to 8.
#[test]
fn chroma_halfpel_matches_convolve_oracle() {
    let mut seed = 42u32;
    let c_stride = 128;
    let (fcw, fch) = (128usize, 128usize);
    let plane = lcg_frame(&mut seed, c_stride * fch);
    // (dv.x, dv.y) eighth-pel; odd/even full-pel combinations.
    let cases: [(i16, i16); 6] = [
        (-64, -32), // even/even -> copy
        (-56, -32), // odd/even  -> x half-pel
        (-64, -40), // even/odd  -> y half-pel
        (-56, -40), // odd/odd   -> 2d
        (-8, -8),   // minimal odd/odd
        (40, -104), // positive x odd, negative y odd
    ];
    for (dvx, dvy) in cases {
        let dv = Mv { x: dvx, y: dvy };
        let (cw, ch) = (8usize, 8usize);
        let (ccx, ccy) = (32usize, 32usize);
        let pos_x = (ccx as i64 + i64::from(dvx >> 4)) as usize;
        let pos_y = (ccy as i64 + i64::from(dvy >> 4)) as usize;
        let mut dst = vec![0u8; cw * ch];
        predict_intrabc_chroma(&plane, c_stride, ccx, ccy, cw, ch, fcw, fch, dv, &mut dst);
        let oracle = convolve_oracle(
            &plane,
            c_stride,
            pos_x,
            pos_y,
            cw,
            ch,
            (dvx & 15) != 0,
            (dvy & 15) != 0,
        );
        assert_eq!(dst, oracle, "dv=({dvx},{dvy})");
    }
}

#[test]
fn chroma_neg_dv_floors_like_c_shift() {
    // dv.x = -8 (one odd luma pel left): C `-8 >> 4` = -1 (arithmetic
    // floor), subpel 8 — i.e. sample columns (org-1, org) averaged.
    let c_stride = 32;
    let mut plane = vec![0u8; c_stride * 32];
    for (i, p) in plane.iter_mut().enumerate() {
        *p = (i % 251) as u8;
    }
    let mut dst = vec![0u8; 4 * 4];
    predict_intrabc_chroma(
        &plane,
        c_stride,
        8,
        8,
        4,
        4,
        32,
        32,
        Mv { x: -8, y: 0 },
        &mut dst,
    );
    let a = u16::from(plane[8 * c_stride + 7]);
    let b = u16::from(plane[8 * c_stride + 8]);
    assert_eq!(dst[0], ((a + b + 1) >> 1) as u8);
}
