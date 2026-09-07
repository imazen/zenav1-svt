//! C noise_model.c's overlapped Wiener denoiser and per-picture model driver.
// Copyright (c) 2017, Alliance for Open Media. BSD-2-Clause and AOM Patent License.
// Evidence: tests/c_parity_film_grain.rs, exported pinned C on x86-64;
// production wiring: tools/film_grain_gate.py (8/10-bit 4:2:0).
// PORT-NOTE(verified): noise_model.c:1968-2367; retain float accumulation order.
use crate::entropy::obu::FilmGrainParams;
use crate::film_grain_fft::NoiseTransform;
use crate::film_grain_model::{FlatBlockFinder, NoiseModel, NoiseStatus};
use crate::port_noise_model::{NoiseShape, apply_window_function_to_plane, pointwise_multiply};
use alloc::{vec, vec::Vec};
#[path = "film_grain_windows.rs"]
mod windows;

pub fn block_size(width: usize, height: usize, adaptive: bool) -> usize {
    if !adaptive {
        32
    } else if width * height < 0x535200 {
        8
    } else if width * height < 0x140a000 {
        16
    } else {
        32
    }
}
/// Packed 8/10-bit samples; strides are in samples. Chroma is 4:2:0.
/// Output padding is zero, as in C's calloc'ed denoised planes.
pub fn wiener_denoise(
    data: [&[u16]; 3],
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
    bs: usize,
    psd: [f32; 3],
) -> [Vec<u16>; 3] {
    let nw = w.div_ceil(bs);
    let nh = h.div_ceil(bs);
    let rs = (nw + 2) * bs;
    let norm = ((1u32 << depth) - 1) as f32;
    core::array::from_fn(|c| {
        let sub = usize::from(c > 0);
        let n = bs >> sub;
        let pw = w >> sub;
        let ph = h >> sub;
        let mut output = vec![0; strides[c] * h.div_ceil(1 << sub)];
        if pw == 0 || ph == 0 {
            return output;
        }
        let finder = FlatBlockFinder::new(n, depth);
        let window = windows::window(n);
        let mut tx = NoiseTransform::new(n);
        let mut result = vec![0.0; rs * (nh + 2) * bs];
        let mut plane = vec![0.0; n * n];
        let mut block = vec![0.0; n * n];
        let mut plane_d = vec![0.0; n * n];
        let mut block_d = vec![0.0; n * n];
        for oy in [0, n / 2] {
            for ox in [0, n / 2] {
                for by in -1..nh as i32 {
                    for bx in -1..nw as i32 {
                        finder.extract(
                            data[c],
                            pw,
                            ph,
                            strides[c],
                            bx * n as i32 + ox as i32,
                            by * n as i32 + oy as i32,
                            &mut plane_d,
                            &mut block_d,
                        );
                        pointwise_multiply(window, &mut plane, &mut block, &plane_d, &block_d);
                        tx.denoise(&mut block, psd[c]);
                        let offset = ((by + 1) as usize * n + oy) * rs + (bx + 1) as usize * n + ox;
                        apply_window_function_to_plane(
                            n,
                            n,
                            &mut result[offset..],
                            rs,
                            &block,
                            &plane,
                            window,
                        );
                    }
                }
            }
        }
        for y in 0..ph {
            for x in 0..pw {
                let i = (y + n) * rs + x + n;
                let value = (result[i] * norm + 0.5).clamp(0.0, norm) as u16;
                let err = -(f32::from(value) / norm - result[i]);
                output[y * strides[c] + x] = value;
                if x + 1 < pw {
                    result[i + 1] += err * 7.0 / 16.0;
                }
                if y + 1 < ph {
                    if x > 0 {
                        result[i + rs - 1] += err * 3.0 / 16.0;
                    }
                    result[i + rs] += err * 5.0 / 16.0;
                    if x + 1 < pw {
                        result[i + rs + 1] += err * 1.0 / 16.0;
                    }
                }
            }
        }
        output
    })
}
/// Mirrors C's fresh denoise/model context for every picture. The caller may
/// replace source pixels only when the returned parameters apply grain.
pub fn denoise_and_model(
    data: [&[u16]; 3],
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
    strength: u8,
    adaptive: bool,
    seed: u16,
) -> ([Vec<u16>; 3], FilmGrainParams, NoiseStatus) {
    let bs = block_size(w, h, adaptive);
    let finder = FlatBlockFinder::new(bs, depth);
    let flat = finder.run(data[0], w, h, strides[0]);
    let level = (f64::from(strength) / 10.0) as f32;
    let psd = core::array::from_fn(|c| {
        let n = (bs >> usize::from(c > 0)) as f32;
        level * level / 10000.0 * n * n / 8.0
    });
    let denoised = wiener_denoise(data, w, h, strides, depth, bs, psd);
    let mut model = NoiseModel::new(NoiseShape::Square, 3, depth);
    let status = model.update(
        data,
        denoised.each_ref().map(Vec::as_slice),
        w,
        h,
        strides,
        &flat,
        bs,
    );
    let mut params = FilmGrainParams {
        random_seed: seed,
        ..Default::default()
    };
    if status == NoiseStatus::Ok {
        model.save_latest();
        if let Some(fg) = model.grain_parameters(seed) {
            params = fg;
        }
    }
    (denoised, params, status)
}
