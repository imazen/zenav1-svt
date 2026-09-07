//! Translation of SVT `grainSynthesis.c` (3115c0c1). Scratch and RNG are owned
//! by each invocation. References must never receive this output-only grain.
// Copyright (c) 2016, Alliance for Open Media. BSD-2-Clause and AOM Patent License.
// Evidence: tests/c_parity_film_grain.rs, exported pinned C on x86-64;
// production wiring: tools/film_grain_gate.py (8/10-bit 4:2:0).
// PORT-NOTE(verified): grainSynthesis.c:30-1294; compare all planes, overlaps,
// seeds, AR lags, scaling modes and 8/10-bit output against add_film_grain_run.
use crate::entropy::obu::FilmGrainParams;
use alloc::{vec, vec::Vec};
#[path = "film_grain_gaussian.rs"]
mod gaussian;

fn random(state: &mut u16, bits: u32) -> usize {
    let bit = (*state ^ (*state >> 1) ^ (*state >> 3) ^ (*state >> 12)) & 1;
    *state = (*state >> 1) | (bit << 15);
    usize::from(*state >> (16 - bits))
}
fn row_seed(line: usize, seed: u16) -> u16 {
    let row = line >> 5;
    seed ^ ((((row * 37 + 178) & 255) << 8) as u16) ^ (((row * 173 + 105) & 255) as u16)
}
fn scaling(points: &[[u8; 2]]) -> [i32; 256] {
    let mut lut = [0; 256];
    if points.is_empty() {
        return lut;
    }
    lut[..points[0][0] as usize].fill(i32::from(points[0][1]));
    for pair in points.windows(2) {
        let dx = i64::from(pair[1][0]) - i64::from(pair[0][0]);
        let dy = i64::from(pair[1][1]) - i64::from(pair[0][1]);
        let delta = dy * ((65536 + (dx >> 1)) / dx);
        for x in 0..dx {
            lut[pair[0][0] as usize + x as usize] =
                i32::from(pair[0][1]) + ((x * delta + 32768) >> 16) as i32;
        }
    }
    let last = points.last().unwrap();
    lut[last[0] as usize..].fill(i32::from(last[1]));
    lut
}
fn scale(lut: &[i32; 256], index: i32, depth: u8) -> i32 {
    let x = (index >> (depth - 8)) as usize;
    if depth == 8 || x == 255 {
        lut[x]
    } else {
        lut[x]
            + (((lut[x + 1] - lut[x]) * (index & ((1 << (depth - 8)) - 1)) + (1 << (depth - 9)))
                >> (depth - 8))
    }
}
fn templates(p: &FilmGrainParams, depth: u8) -> [Vec<i32>; 3] {
    // C's top/left pad=3, AR stabilization pad=3; 4:2:0 templates.
    let mut blocks = [vec![0; 82 * 73], vec![0; 44 * 38], vec![0; 44 * 38]];
    let lag = i32::from(p.ar_coeff_lag);
    let mut positions = Vec::new();
    for y in -lag..=0 {
        for x in -lag..=lag {
            if y == 0 && x >= 0 {
                break;
            }
            positions.push((y, x));
        }
    }
    let shift = 12 - depth + p.grain_scale_shift;
    let lo = -(128 << (depth - 8));
    let hi = (128 << (depth - 8)) - 1;
    let coeffs = [&p.ar_coeffs_y[..], &p.ar_coeffs_cb[..], &p.ar_coeffs_cr[..]];
    for plane in 0..3 {
        let active = match plane {
            0 => p.num_y_points > 0,
            1 => p.num_cb_points > 0 || p.chroma_scaling_from_luma,
            _ => p.num_cr_points > 0 || p.chroma_scaling_from_luma,
        };
        if !active {
            continue;
        }
        let (w, h) = if plane == 0 { (82, 73) } else { (44, 38) };
        let mut rng = match plane {
            0 => p.random_seed,
            1 => row_seed(7 << 5, p.random_seed),
            _ => row_seed(11 << 5, p.random_seed),
        };
        for v in &mut blocks[plane] {
            *v = (gaussian::GAUSSIAN[random(&mut rng, 11)] + ((1 << shift) >> 1)) >> shift;
        }
        for y in 3..h {
            for x in 3..w - 3 {
                let mut sum = 0;
                for (i, &(dy, dx)) in positions.iter().enumerate() {
                    let idx = (y as i32 + dy) as usize * w + (x as i32 + dx) as usize;
                    sum += i32::from(coeffs[plane][i]) * blocks[plane][idx];
                }
                if plane > 0 && p.num_y_points > 0 {
                    let ly = (y - 3) * 2 + 3;
                    let lx = (x - 3) * 2 + 3;
                    let mut avg = 0;
                    for yy in ly..ly + 2 {
                        for xx in lx..lx + 2 {
                            avg += blocks[0][yy * 82 + xx];
                        }
                    }
                    avg = (avg + 2) >> 2;
                    sum += i32::from(coeffs[plane][positions.len()]) * avg;
                }
                let idx = y * w + x;
                blocks[plane][idx] = (blocks[plane][idx]
                    + ((sum + (1 << (p.ar_coeff_shift - 1))) >> p.ar_coeff_shift))
                    .clamp(lo, hi);
            }
        }
    }
    blocks
}
fn overlap(a: i32, b: i32, i: usize, n: usize, lo: i32, hi: i32) -> i32 {
    let (wa, wb) = if n == 1 {
        (23, 22)
    } else if i == 0 {
        (27, 17)
    } else {
        (17, 27)
    };
    ((wa * a + wb * b + 16) >> 5).clamp(lo, hi)
}

/// C's column and line overlap buffers, expressed once for luma and chroma.
/// Bottom strips retain vertical overlap before top overlap is applied; right
/// strips retain raw template samples. This preserves the corner operation order.
fn render(
    template: &[i32],
    width: usize,
    height: usize,
    chroma: bool,
    p: &FilmGrainParams,
    depth: u8,
) -> Vec<i32> {
    let (bs, edge, ts, pad) = if chroma {
        (16, 1, 44, 6)
    } else {
        (32, 2, 82, 9)
    };
    let lo = -(128 << (depth - 8));
    let hi = (128 << (depth - 8)) - 1;
    let mut out = vec![0; width * height];
    let mut line = vec![0; width * edge];
    let mut col = vec![0; (bs + edge) * edge];
    let ps = bs + edge;
    let mut patch = vec![0; ps * ps];
    for by in (0..height).step_by(bs) {
        let mut rng = row_seed(if chroma { by * 2 } else { by }, p.random_seed);
        let mut next_line = vec![0; width * edge];
        for bx in (0..width).step_by(bs) {
            let offset = random(&mut rng, 8);
            let ox = pad + (offset >> 4) * edge;
            let oy = pad + (offset & 15) * edge;
            let bw = bs.min(width - bx);
            let bh = bs.min(height - by);
            for y in 0..ps {
                for x in 0..ps {
                    patch[y * ps + x] = template[(oy + y) * ts + ox + x];
                }
            }
            if p.overlap_flag && bx > 0 {
                for y in 0..ps {
                    for x in 0..edge {
                        patch[y * ps + x] =
                            overlap(col[y * edge + x], patch[y * ps + x], x, edge, lo, hi);
                    }
                }
            }
            // Next row sees the vertically overlapped bottom, never the top blend.
            for y in 0..edge {
                for x in 0..bw {
                    next_line[y * width + bx + x] = patch[(bs + y) * ps + x];
                }
            }
            if p.overlap_flag && by > 0 {
                for y in 0..edge.min(bh) {
                    for x in 0..bw {
                        patch[y * ps + x] =
                            overlap(line[y * width + bx + x], patch[y * ps + x], y, edge, lo, hi);
                    }
                }
            }
            for y in 0..bh {
                out[(by + y) * width + bx..(by + y) * width + bx + bw]
                    .copy_from_slice(&patch[y * ps..y * ps + bw]);
            }
            for y in 0..ps {
                for x in 0..edge {
                    col[y * edge + x] = template[(oy + y) * ts + ox + bs + x];
                }
            }
        }
        line = next_line;
    }
    out
}

/// Add C reconstruction-output film grain to strided 4:2:0 planes. The C
/// runner processes width/2 and height/2 blocks; an odd trailing row/column
/// is left unchanged. `depth` is 8 or 10, with samples stored as u16.
pub fn add_grain(
    p: &FilmGrainParams,
    planes: [&mut [u16]; 3],
    strides: [usize; 3],
    width: usize,
    height: usize,
    depth: u8,
) {
    add_grain_impl(p, planes, strides, width, height, depth, false);
}

/// Synthesize the parameters a 4:2:0 decoder receives. Keep the direct C
/// entry point above for oracle comparisons, including C's HBD CfL omission.
pub(crate) fn add_grain_for_output(
    p: &FilmGrainParams,
    planes: [&mut [u16]; 3],
    strides: [usize; 3],
    width: usize,
    height: usize,
    depth: u8,
) {
    let mut signaled = p.clone();
    // write_film_grain_params omits these counts under the 4:2:0 rule.
    if signaled.chroma_scaling_from_luma || signaled.num_y_points == 0 {
        signaled.num_cb_points = 0;
        signaled.num_cr_points = 0;
    }
    add_grain_impl(&signaled, planes, strides, width, height, depth, true);
}

fn add_grain_impl(
    p: &FilmGrainParams,
    planes: [&mut [u16]; 3],
    strides: [usize; 3],
    width: usize,
    height: usize,
    depth: u8,
    decoder_chroma: bool,
) {
    assert!(depth == 8 || depth == 10);
    if !p.apply_grain {
        return;
    }
    let [y, cb, cr] = planes;
    let [ys, us, vs] = strides;
    let w = width & !1;
    let h = height & !1;
    if w == 0 || h == 0 {
        return;
    }
    let blocks = templates(p, depth);
    let grain = [
        render(&blocks[0], w, h, false, p, depth),
        render(&blocks[1], w / 2, h / 2, true, p, depth),
        render(&blocks[2], w / 2, h / 2, true, p, depth),
    ];
    let yl = scaling(&p.scaling_points_y[..p.num_y_points]);
    let cl = if p.chroma_scaling_from_luma {
        [yl, yl]
    } else {
        [
            scaling(&p.scaling_points_cb[..p.num_cb_points]),
            scaling(&p.scaling_points_cr[..p.num_cr_points]),
        ]
    };
    let max_sample = (1 << depth) - 1;
    let shift = depth - 8;
    let (lmin, lmax, cmin, cmax) = if p.clip_to_restricted_range {
        (16 << shift, 235 << shift, 16 << shift, 240 << shift)
    } else {
        (0, max_sample, 0, max_sample)
    };
    // The oracle entry preserves C's HBD point-count-only predicate.
    // Production output also applies CfL at high bit depth, as signaled.
    for (ci, (dst, stride, count, mult, lmult, off)) in [
        (
            cb,
            us,
            p.num_cb_points,
            p.cb_mult,
            p.cb_luma_mult,
            p.cb_offset,
        ),
        (
            cr,
            vs,
            p.num_cr_points,
            p.cr_mult,
            p.cr_luma_mult,
            p.cr_offset,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if count == 0 && !((depth == 8 || decoder_chroma) && p.chroma_scaling_from_luma) {
            continue;
        }
        let (mult, lmult, off) = if p.chroma_scaling_from_luma {
            (0, 64, 0)
        } else {
            (
                i32::from(mult) - 128,
                i32::from(lmult) - 128,
                (i32::from(off) << shift) - (1 << depth),
            )
        };
        for yy in 0..h / 2 {
            for xx in 0..w / 2 {
                let l = (i32::from(y[yy * 2 * ys + xx * 2])
                    + i32::from(y[yy * 2 * ys + xx * 2 + 1])
                    + 1)
                    >> 1;
                let i = yy * stride + xx;
                let old = i32::from(dst[i]);
                let index = (((l * lmult + mult * old) >> 6) + off).clamp(0, max_sample);
                dst[i] = (old
                    + ((scale(&cl[ci], index, depth) * grain[ci + 1][yy * (w / 2) + xx]
                        + (1 << (p.scaling_shift - 1)))
                        >> p.scaling_shift))
                    .clamp(cmin, cmax) as u16;
            }
        }
    }
    if p.num_y_points > 0 {
        for yy in 0..h {
            for xx in 0..w {
                let i = yy * ys + xx;
                let old = i32::from(y[i]);
                y[i] = (old
                    + ((scale(&yl, old, depth) * grain[0][yy * w + xx]
                        + (1 << (p.scaling_shift - 1)))
                        >> p.scaling_shift))
                    .clamp(lmin, lmax) as u16;
            }
        }
    }
}
