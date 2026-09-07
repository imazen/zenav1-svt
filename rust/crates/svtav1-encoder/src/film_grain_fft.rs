//! C fft.c's real 2D transform, packing and noise_util.c's Wiener filter.
// Copyright (c) 2017-2018, Alliance for Open Media. BSD-2-Clause and AOM Patent License.
// Evidence: tests/c_parity_film_grain.rs, exported pinned C on x86-64;
// production wiring: tools/film_grain_gate.py (8/10-bit 4:2:0).
// PORT-NOTE(verified): fft.c:22-237 and noise_util.c:20-110; differential
// forward/inverse/filter against all five exported C transform sizes.
use alloc::{vec, vec::Vec};
#[path = "film_grain_fft_1d.rs"]
mod d1;
fn transform(n: usize, inverse: bool) -> fn(&[f32], &mut [f32], usize) {
    match (n, inverse) {
        (2, false) => d1::fft2,
        (4, false) => d1::fft4,
        (8, false) => d1::fft8,
        (16, false) => d1::fft16,
        (32, false) => d1::fft32,
        (2, true) => d1::ifft2,
        (4, true) => d1::ifft4,
        (8, true) => d1::ifft8,
        (16, true) => d1::ifft16,
        (32, true) => d1::ifft32,
        _ => panic!("unsupported noise transform size"),
    }
}
fn transpose(a: &[f32], b: &mut [f32], n: usize) {
    for y in 0..n {
        for x in 0..n {
            b[y * n + x] = a[x * n + y];
        }
    }
}
pub fn forward(input: &[f32], temp: &mut [f32], output: &mut [f32], n: usize) {
    let tx = transform(n, false);
    for x in 0..n {
        tx(&input[x..], &mut output[x..], n);
    }
    transpose(output, temp, n);
    for x in 0..n {
        tx(&temp[x..], &mut output[x..], n);
    }
    transpose(output, temp, n);
    for y in 0..=n / 2 {
        let y2 = y + n / 2;
        let ye = y2 > n / 2 && y2 < n;
        for x in 0..=n / 2 {
            let x2 = x + n / 2;
            let xe = x2 > n / 2 && x2 < n;
            output[2 * (y * n + x)] =
                temp[y * n + x] - if xe && ye { temp[y2 * n + x2] } else { 0.0 };
            output[2 * (y * n + x) + 1] =
                (if ye { temp[y2 * n + x] } else { 0.0 }) + if xe { temp[y * n + x2] } else { 0.0 };
            if ye {
                output[2 * ((n - y) * n + x)] =
                    temp[y * n + x] + if xe { temp[y2 * n + x2] } else { 0.0 };
                output[2 * ((n - y) * n + x) + 1] =
                    -temp[y2 * n + x] + if xe { temp[y * n + x2] } else { 0.0 };
            }
        }
    }
}
pub fn inverse(input: &[f32], temp: &mut [f32], output: &mut [f32], n: usize) {
    let fft = transform(n, false);
    let ifft = transform(n, true);
    for y in 0..=n / 2 {
        output[y * n] = input[2 * y * n];
        output[y * n + 1] = input[2 * (y * n + n / 2)];
    }
    for y in n / 2 + 1..n {
        output[y * n] = input[2 * (y - n / 2) * n + 1];
        output[y * n + 1] = input[2 * ((y - n / 2) * n + n / 2) + 1];
    }
    for i in 0..2 {
        ifft(&output[i..], &mut temp[i..], n);
    }
    for y in 0..n {
        for x in 1..n / 2 {
            output[y * n + x + 1] = input[2 * (y * n + x)];
        }
        for x in 1..n / 2 {
            output[y * n + x + n / 2] = input[2 * (y * n + x) + 1];
        }
    }
    for y in 2..n {
        fft(&output[y..], &mut temp[y..], n);
    }
    for x in 0..n {
        output[x] = temp[x * n];
        output[(n / 2) * n + x] = temp[x * n + 1];
    }
    for y in 1..n / 2 {
        for x in 0..=n / 2 {
            output[x + y * n] = temp[y + 1 + x * n]
                + if x > 0 && x < n / 2 {
                    temp[y + n / 2 + (x + n / 2) * n]
                } else {
                    0.0
                };
        }
        for x in n / 2 + 1..n {
            output[x + y * n] = temp[y + 1 + (n - x) * n] - temp[y + n / 2 + (n - x + n / 2) * n];
        }
        for x in 0..=n / 2 {
            output[x + (y + n / 2) * n] = temp[y + n / 2 + x * n]
                - if x > 0 && x < n / 2 {
                    temp[y + 1 + (x + n / 2) * n]
                } else {
                    0.0
                };
        }
        for x in n / 2 + 1..n {
            output[x + (y + n / 2) * n] =
                temp[y + 1 + (n - x + n / 2) * n] + temp[y + n / 2 + (n - x) * n];
        }
    }
    for y in 0..n {
        ifft(&output[y..], &mut temp[y..], n);
    }
    transpose(temp, output, n);
}
pub fn filter(block: &mut [f32], psd: f32) {
    let beta = 1.1_f32;
    let low = (beta - 1.0) / beta;
    let threshold = beta * psd;
    for z in block.chunks_exact_mut(2) {
        let p = z[0] * z[0] + z[1] * z[1];
        // C compares its float p to an unsuffixed double 1e-6 literal.
        let factor = if p > threshold && f64::from(p) > 1e-6 {
            (p - psd) / p.max(1e-6_f32)
        } else {
            low
        };
        z[0] *= factor;
        z[1] *= factor;
    }
}
pub(crate) struct NoiseTransform {
    n: usize,
    temp: Vec<f32>,
    spectrum: Vec<f32>,
}
impl NoiseTransform {
    pub fn new(n: usize) -> Self {
        Self {
            n,
            temp: vec![0.0; 2 * n * n],
            spectrum: vec![0.0; 2 * n * n],
        }
    }
    pub fn denoise(&mut self, block: &mut [f32], psd: f32) {
        forward(block, &mut self.temp, &mut self.spectrum, self.n);
        filter(&mut self.spectrum, psd);
        inverse(&self.spectrum, &mut self.temp, block, self.n);
        for v in &mut block[..self.n * self.n] {
            *v /= (self.n * self.n) as f32;
        }
    }
}
