//! Owned noise-model state translated from noise_model.c (SVT 3115c0c1).
// Copyright (c) 2017, Alliance for Open Media. BSD-2-Clause and AOM Patent License.
// Evidence: tests/c_parity_film_grain.rs, exported pinned C on x86-64;
// production wiring: tools/film_grain_gate.py (8/10-bit 4:2:0).
// PORT-NOTE(verified): noise_model.c:36-1290 and mathutils.h:22-61; compare
// flat maps, normal equations, solutions, gain, fitted LUTs and final parameters.
use crate::entropy::obu::FilmGrainParams;
use crate::port_noise_model::{NoiseShape, NoiseStrengthSolver, compare_scores};
use alloc::{vec, vec::Vec};

// AOMMIN/AOMMAX select their second operand on unordered comparisons.
// f64::min/max instead discard a NaN and are not translations of those macros.
fn cmax(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}
fn cmin(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}
/// C's out-of-range conversion has no portable language-level result. The
/// pinned x86 binary uses CVTTSD2SI (INT_MIN for NaN/overflow); AArch64 FCVTZS
/// saturates (including NaN -> 0). Spell out the target instruction behavior
/// safely. Flat, zero-noise chroma reaches this through 0/0 in y_corr.
fn c_int(value: f64) -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if !(-2147483648.0..2147483648.0).contains(&value) {
        return i32::MIN;
    }
    value as i32
}

#[derive(Clone, Debug)]
pub struct Equations {
    pub a: Vec<f64>,
    pub b: Vec<f64>,
    pub x: Vec<f64>,
}
impl Equations {
    fn new(n: usize) -> Self {
        Self {
            a: vec![0.0; n * n],
            b: vec![0.0; n],
            x: vec![0.0; n],
        }
    }
    fn clear(&mut self) {
        self.a.fill(0.0);
        self.b.fill(0.0);
        self.x.fill(0.0);
    }
    fn solve(&mut self) -> bool {
        solve(&self.a, &self.b, &mut self.x)
    }
}
/// mathutils.h linsolve: bubble-pivot elimination, including whole-row updates.
pub fn solve(a: &[f64], b: &[f64], x: &mut [f64]) -> bool {
    let n = x.len();
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    for k in 0..n.saturating_sub(1) {
        for i in (k + 1..n).rev() {
            if a[(i - 1) * n + k].abs() < a[i * n + k].abs() {
                for j in 0..n {
                    a.swap(i * n + j, (i - 1) * n + j);
                }
                b.swap(i, i - 1);
            }
        }
        for i in k..n - 1 {
            if a[k * n + k].abs() < 1e-16 {
                return false;
            }
            let c = a[(i + 1) * n + k] / a[k * n + k];
            for j in 0..n {
                a[(i + 1) * n + j] -= c * a[k * n + j];
            }
            b[i + 1] -= c * b[k];
        }
    }
    for i in (0..n).rev() {
        if a[i * n + i].abs() < 1e-16 {
            return false;
        }
        let mut c = 0.0;
        for j in i + 1..n {
            c += a[i * n + j] * x[j];
        }
        x[i] = (b[i] - c) / a[i * n + i];
    }
    true
}
impl NoiseStrengthSolver {
    pub fn clear(&mut self) {
        self.a.fill(0.0);
        self.b.fill(0.0);
        self.x.fill(0.0);
        self.num_equations = 0;
        self.total = 0.0;
    }
    /// C restores A but deliberately keeps the regularization added to b.
    pub fn solve(&mut self) -> bool {
        let n = self.num_bins as usize;
        let alpha = 2.0 * f64::from(self.num_equations) / n as f64;
        let mut a = self.a.clone();
        for i in 0..n {
            a[i * n + i.saturating_sub(1)] -= alpha;
            a[i * n + i] += 2.0 * alpha;
            a[i * n + (i + 1).min(n - 1)] -= alpha;
        }
        let mean = self.total / f64::from(self.num_equations);
        for i in 0..n {
            a[i * n + i] += 1.0 / 8192.0;
            self.b[i] += mean / 8192.0;
        }
        solve(&a, &self.b, &mut self.x)
    }
    fn residual(&self, lut: &[[f64; 2]], res: &mut [f64], start: usize, end: usize) {
        let dx = 255.0 / f64::from(self.num_bins);
        for i in start.max(1)..end.min(lut.len() - 1) {
            let lower = self.bin_index(lut[i - 1][0]).floor().max(0.0) as usize;
            let upper = (self.bin_index(lut[i + 1][0]).ceil() as usize).min(self.x.len() - 1);
            let mut r = 0.0;
            for j in lower..=upper {
                let x = self.get_center(j as i32);
                if x < lut[i - 1][0] || x >= lut[i + 1][0] {
                    continue;
                }
                let a = (x - lut[i - 1][0]) / (lut[i + 1][0] - lut[i - 1][0]);
                let estimate = lut[i - 1][1] * (1.0 - a) + lut[i + 1][1] * a;
                r += (self.x[j] - estimate).abs();
            }
            res[i] = r * dx;
        }
    }
    pub fn fit_piecewise(&self, max_points: i32) -> Vec<[f64; 2]> {
        let max_points = if max_points < 0 {
            self.num_bins
        } else {
            max_points
        } as usize;
        let tolerance = self.max_intensity * 0.00625 / 255.0;
        let mut lut: Vec<_> = (0..self.num_bins)
            .map(|i| [self.get_center(i), self.x[i as usize]])
            .collect();
        let mut residual = vec![0.0; lut.len()];
        self.residual(&lut, &mut residual, 0, lut.len());
        while lut.len() > 2 {
            let mut k = 1;
            for j in 1..lut.len() - 1 {
                if residual[j] < residual[k] {
                    k = j;
                }
            }
            let dx = lut[k + 1][0] - lut[k - 1][0];
            if lut.len() <= max_points && residual[k] / dx > tolerance {
                break;
            }
            lut.remove(k);
            // C memmoves points only, NOT residuals; keep that exact behavior.
            self.residual(&lut, &mut residual, k - 1, k + 1);
        }
        lut
    }
}

pub struct FlatBlockFinder {
    pub block_size: usize,
    a: Vec<f64>,
    inv: [f64; 9],
    normalization: f64,
}
impl FlatBlockFinder {
    pub fn new(bs: usize, depth: u8) -> Self {
        let mut a = vec![0.0; bs * bs * 3];
        let mut eq = Equations::new(3);
        for y in 0..bs {
            let yd = (y as f64 - bs as f64 / 2.0) / (bs as f64 / 2.0);
            for x in 0..bs {
                let xd = (x as f64 - bs as f64 / 2.0) / (bs as f64 / 2.0);
                let coords = [yd, xd, 1.0];
                a[(y * bs + x) * 3..(y * bs + x) * 3 + 3].copy_from_slice(&coords);
                for i in 0..3 {
                    for j in 0..3 {
                        eq.a[i * 3 + j] += coords[i] * coords[j];
                    }
                }
            }
        }
        let mut inv = [0.0; 9];
        for i in 0..3 {
            eq.b.fill(0.0);
            eq.b[i] = 1.0;
            assert!(eq.solve());
            for j in 0..3 {
                inv[j * 3 + i] = eq.x[j];
            }
        }
        Self {
            block_size: bs,
            a,
            inv,
            normalization: ((1u32 << depth) - 1) as f64,
        }
    }
    pub fn extract(
        &self,
        data: &[u16],
        w: usize,
        h: usize,
        stride: usize,
        ox: i32,
        oy: i32,
        plane: &mut [f64],
        block: &mut [f64],
    ) {
        let bs = self.block_size;
        let norm = 1.0 / self.normalization;
        for yi in 0..bs {
            let y = (oy + yi as i32).clamp(0, h as i32 - 1) as usize;
            for xi in 0..bs {
                let x = (ox + xi as i32).clamp(0, w as i32 - 1) as usize;
                block[yi * bs + xi] = f64::from(data[y * stride + x]) * norm;
            }
        }
        let mut b = [0.0; 3];
        for i in 0..bs * bs {
            for j in 0..3 {
                b[j] += block[i] * self.a[i * 3 + j];
            }
        }
        let mut coords = [0.0; 3];
        for j in 0..3 {
            coords[j] =
                (self.inv[j * 3] * b[0] + self.inv[j * 3 + 1] * b[1]) + self.inv[j * 3 + 2] * b[2];
        }
        for i in 0..bs * bs {
            plane[i] = (self.a[i * 3] * coords[0] + self.a[i * 3 + 1] * coords[1])
                + self.a[i * 3 + 2] * coords[2];
            block[i] -= plane[i];
        }
    }
    pub fn run(&self, data: &[u16], w: usize, h: usize, stride: usize) -> Vec<u8> {
        let bs = self.block_size;
        let nw = w.div_ceil(bs);
        let nh = h.div_ceil(bs);
        let mut flat = vec![0; nw * nh];
        let mut scores = Vec::with_capacity(nw * nh);
        let mut plane = vec![0.0; bs * bs];
        let mut block = plane.clone();
        let var_threshold = 0.005 / (bs * bs) as f64;
        for by in 0..nh {
            for bx in 0..nw {
                self.extract(
                    data,
                    w,
                    h,
                    stride,
                    (bx * bs) as i32,
                    (by * bs) as i32,
                    &mut plane,
                    &mut block,
                );
                let (mut gxx, mut gxy, mut gyy, mut var, mut mean) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for y in 1..bs - 1 {
                    for x in 1..bs - 1 {
                        let i = y * bs + x;
                        let gx = (block[i + 1] - block[i - 1]) / 2.0;
                        let gy = (block[i + bs] - block[i - bs]) / 2.0;
                        gxx += gx * gx;
                        gxy += gx * gy;
                        gyy += gy * gy;
                        mean += block[i];
                        var += block[i] * block[i];
                    }
                }
                let n = ((bs - 2) * (bs - 2)) as f64;
                mean /= n;
                gxx /= n;
                gxy /= n;
                gyy /= n;
                var = var / n - mean * mean;
                let trace = gxx + gyy;
                let det = gxx * gyy - gxy * gxy;
                let root = (trace * trace - 4.0 * det).sqrt();
                let e1 = (trace + root) / 2.0;
                let e2 = (trace - root) / 2.0;
                let ratio = e1 / e2.max(1e-6);
                let is_flat = trace < 0.15 / 1024.0
                    && ratio < 1.25
                    && e1 < 0.08 / 1024.0
                    && var > var_threshold;
                let score = (1.0
                    / (1.0
                        + (-(-6682.0 * var - 0.2056 * ratio + 13087.0 * trace - 12434.0 * e1
                            + 2.5694))
                            .exp())) as f32;
                let idx = by * nw + bx;
                flat[idx] = if is_flat { 255 } else { 0 };
                scores.push((idx, if var > var_threshold { score } else { 0.0 }));
            }
        }
        // Only the threshold value matters: tied blocks are all included.
        scores.sort_by(|a, b| compare_scores(a.1, b.1));
        let threshold = scores[nw * nh * 90 / 100].1;
        for (i, s) in scores {
            if s >= threshold {
                flat[i] |= 1;
            }
        }
        flat
    }
}

#[derive(Clone, Debug)]
pub struct NoiseState {
    pub eq: Equations,
    pub strength: NoiseStrengthSolver,
    pub observations: i32,
    pub gain: f64,
}
impl NoiseState {
    fn new(n: usize, depth: u8) -> Self {
        Self {
            eq: Equations::new(n),
            strength: NoiseStrengthSolver::new(20, u32::from(depth)),
            observations: 0,
            gain: 1.0,
        }
    }
    fn solve(&mut self, chroma: bool) -> bool {
        self.gain = 1.0;
        if !self.eq.solve() {
            return false;
        }
        let n = self.eq.x.len();
        let nc = n - usize::from(chroma);
        let mut var = 0.0;
        for i in 0..nc {
            var += self.eq.a[i * n + i] / f64::from(self.observations);
        }
        var /= nc as f64;
        let mut covar = 0.0;
        for i in 0..nc {
            let mut b = self.eq.b[i];
            if chroma {
                b -= self.eq.a[i * n + n - 1] * self.eq.x[n - 1];
            }
            covar += (b * self.eq.x[i]) / f64::from(self.observations);
        }
        self.gain = (var / (var - covar).max(1e-6)).max(1e-6).sqrt().max(1.0);
        true
    }
    fn chroma_fallback(&mut self) {
        let n = self.eq.x.len();
        self.eq.x.fill(0.0);
        if self.eq.a[n * n - 1].abs() > 1e-6 {
            self.eq.x[n - 1] = self.eq.b[n - 1] / self.eq.a[n * n - 1];
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseStatus {
    Ok,
    InvalidArgument,
    InsufficientFlatBlocks,
    InsufficientNoisePixels,
    InternalError,
}
pub struct NoiseModel {
    pub latest: [NoiseState; 3],
    pub combined: [NoiseState; 3],
    coords: Vec<(i32, i32)>,
    lag: usize,
    depth: u8,
}
impl NoiseModel {
    pub fn new(shape: NoiseShape, lag: usize, depth: u8) -> Self {
        assert!((1..=4).contains(&lag));
        let mut coords = Vec::new();
        let l = lag as i32;
        for y in -l..=0 {
            for x in -l..=if y == 0 { -1 } else { l } {
                if shape == NoiseShape::Square || x.abs() <= y + l {
                    coords.push((x, y));
                }
            }
        }
        let states =
            core::array::from_fn(|c| NoiseState::new(coords.len() + usize::from(c > 0), depth));
        Self {
            latest: states.clone(),
            combined: states,
            coords,
            lag,
            depth,
        }
    }
    pub fn update(
        &mut self,
        data: [&[u16]; 3],
        denoised: [&[u16]; 3],
        w: usize,
        h: usize,
        strides: [usize; 3],
        flat: &[u8],
        bs: usize,
    ) -> NoiseStatus {
        if bs <= 1 || bs < 2 * self.lag + 1 {
            return NoiseStatus::InvalidArgument;
        }
        for state in &mut self.latest {
            state.eq.clear();
            state.observations = 0;
            state.strength.clear();
        }
        if flat.iter().filter(|&&v| v != 0).count() <= 1 {
            return NoiseStatus::InsufficientFlatBlocks;
        }
        let nw = w.div_ceil(bs);
        let nh = h.div_ceil(bs);
        let norm = ((1u32 << self.depth) - 1) as f64;
        let recp = 1.0 / (norm * norm);
        let nc = self.coords.len();
        for c in 0..3 {
            if data[c].is_empty() || denoised[c].is_empty() {
                break;
            }
            let sub = usize::from(c > 0);
            let pw = w >> sub;
            let ph = h >> sub;
            let stride = strides[c];
            let bsz = bs >> sub;
            let n = nc + sub;
            let mut buf = vec![0.0; n];
            for by in 0..nh {
                for bx in 0..nw {
                    if flat[by * nw + bx] == 0 {
                        continue;
                    }
                    let yo = by * bsz;
                    let xo = bx * bsz;
                    let ys = if by > 0 && flat[(by - 1) * nw + bx] != 0 {
                        0
                    } else {
                        self.lag
                    };
                    let xs = if bx > 0 && flat[by * nw + bx - 1] != 0 {
                        0
                    } else {
                        self.lag
                    };
                    let ye = ph.saturating_sub(yo).min(bsz);
                    let xe = pw.saturating_sub(xo + self.lag).min(
                        if bx + 1 < nw && flat[by * nw + bx + 1] != 0 {
                            bsz
                        } else {
                            bsz - self.lag
                        },
                    );
                    for yy in ys..ye {
                        for xx in xs..xe {
                            let y = yo + yy;
                            let x = xo + xx;
                            for (i, &(dx, dy)) in self.coords.iter().enumerate() {
                                let k =
                                    (y as i32 + dy) as usize * stride + (x as i32 + dx) as usize;
                                buf[i] = f64::from(data[c][k]) - f64::from(denoised[c][k]);
                            }
                            let k = y * stride + x;
                            let value = f64::from(data[c][k]) - f64::from(denoised[c][k]);
                            if c > 0 {
                                let mut a = 0.0;
                                let mut b = 0.0;
                                for dy in 0..2 {
                                    for dx in 0..2 {
                                        let k = (y * 2 + dy) * strides[0] + x * 2 + dx;
                                        a += f64::from(data[0][k]);
                                        b += f64::from(denoised[0][k]);
                                    }
                                }
                                buf[nc] = (a - b) / 4.0;
                            }
                            let eq = &mut self.latest[c].eq;
                            for i in 0..n {
                                let bnorm = buf[i] * recp;
                                eq.b[i] += bnorm * value;
                                for j in 0..n {
                                    eq.a[i * n + j] += bnorm * buf[j];
                                }
                            }
                        }
                    }
                    self.latest[c].observations +=
                        (ye.saturating_sub(ys) * xe.saturating_sub(xs)) as i32;
                }
            }
            if !self.latest[c].solve(c > 0) {
                if c > 0 {
                    self.latest[c].chroma_fallback();
                } else {
                    return NoiseStatus::InsufficientNoisePixels;
                }
            }
            for by in 0..nh {
                for bx in 0..nw {
                    if flat[by * nw + bx] == 0 {
                        continue;
                    }
                    let yo = by * bsz;
                    let xo = bx * bsz;
                    let bh = ph.saturating_sub(yo).min(bsz);
                    let bw = pw.saturating_sub(xo).min(bsz);
                    if bw * bh <= bs {
                        continue;
                    }
                    let mx = xo << sub;
                    let my = yo << sub;
                    let mw = (w - mx).min(bs);
                    let mh = (h - my).min(bs);
                    let mut mean = 0.0;
                    for y in 0..mh {
                        for x in 0..mw {
                            mean += f64::from(data[0][(my + y) * strides[0] + mx + x]);
                        }
                    }
                    mean /= (mw * mh) as f64;
                    let mut var = 0.0;
                    let mut noise_mean = 0.0;
                    for y in 0..bh {
                        for x in 0..bw {
                            let k = (yo + y) * stride + xo + x;
                            let v = f64::from(data[c][k]) - f64::from(denoised[c][k]);
                            noise_mean += v;
                            var += v * v;
                        }
                    }
                    noise_mean /= (bw * bh) as f64;
                    var = var / (bw * bh) as f64 - noise_mean * noise_mean;
                    let strength = if c > 0 {
                        self.latest[0].gain * self.latest[0].strength.value_at(mean)
                    } else {
                        0.0
                    };
                    let corr = if c > 0 { self.latest[c].eq.x[nc] } else { 0.0 };
                    let adjusted = (var / 16.0).max(var - (corr * strength).powi(2)).sqrt()
                        / self.latest[c].gain;
                    self.latest[c].strength.add_measurement(mean, adjusted);
                }
            }
            if !self.latest[c].strength.solve() {
                return NoiseStatus::InternalError;
            }
            self.combined[c].observations = self.latest[c].observations;
            self.combined[c].eq = self.latest[c].eq.clone();
            if !self.combined[c].solve(c > 0) {
                if c > 0 {
                    self.combined[c].chroma_fallback();
                } else {
                    return NoiseStatus::InternalError;
                }
            }
            self.combined[c].strength = self.latest[c].strength.clone();
            if !self.combined[c].strength.solve() {
                return NoiseStatus::InternalError;
            }
        }
        NoiseStatus::Ok
    }
    pub fn save_latest(&mut self) {
        for c in 0..3 {
            // C save_latest copies the equation arrays and count, but not total.
            let total = self.combined[c].strength.total;
            self.combined[c] = self.latest[c].clone();
            self.combined[c].strength.total = total;
        }
    }
    pub fn grain_parameters(&self, seed: u16) -> Option<FilmGrainParams> {
        if self.lag > 3 {
            return None;
        }
        let mut p = FilmGrainParams {
            apply_grain: true,
            random_seed: seed,
            ar_coeff_lag: self.lag as u8,
            ..Default::default()
        };
        let mut luts = core::array::from_fn::<_, 3, _>(|c| {
            self.combined[c]
                .strength
                .fit_piecewise(if c == 0 { 14 } else { 10 })
        });
        let divisor = (1u32 << (self.depth - 8)) as f64;
        let mut max: f64 = 1e-4;
        for lut in &mut luts {
            for point in lut {
                point[0] = (point[0] / divisor).min(255.0);
                point[1] = (point[1] / divisor).min(255.0);
                max = max.max(point[1]);
            }
        }
        let log = (max.log2() + 1.0).floor().clamp(2.0, 5.0) as u8;
        p.scaling_shift = 13 - log;
        let factor = (1u32 << (8 - log)) as f64;
        p.num_y_points = luts[0].len();
        p.num_cb_points = luts[1].len();
        p.num_cr_points = luts[2].len();
        for (c, out) in [
            &mut p.scaling_points_y[..],
            &mut p.scaling_points_cb[..],
            &mut p.scaling_points_cr[..],
        ]
        .into_iter()
        .enumerate()
        {
            for (dst, src) in out.iter_mut().zip(&luts[c]) {
                dst[0] = (src[0] + 0.5) as u8;
                dst[1] = ((factor * src[1] + 0.5) as i32).clamp(0, 255) as u8;
            }
        }
        let nc = self.coords.len();
        let mut max: f64 = 1e-4;
        let mut min: f64 = -1e-4;
        let mut ycorr = [0.0; 2];
        let mut avg_luma = 0.0;
        for c in 0..3 {
            let state = &self.combined[c];
            for &x in &state.eq.x[..nc] {
                max = cmax(max, x);
                min = cmin(min, x);
            }
            let solver = &state.strength;
            let n = solver.x.len();
            let mut avg = 0.0;
            let mut total = 0.0;
            for i in 0..n {
                let mut weight = 0.0;
                for j in 0..n {
                    weight += solver.a[i * n + j];
                }
                weight = weight.sqrt();
                avg += solver.x[i] * weight;
                total += weight;
            }
            avg = if total == 0.0 { 1.0 } else { avg / total };
            if c == 0 {
                avg_luma = avg;
            } else {
                ycorr[c - 1] = avg_luma * state.eq.x[nc] / avg;
                max = cmax(max, ycorr[c - 1]);
                min = cmin(min, ycorr[c - 1]);
            }
        }
        p.ar_coeff_shift = 7_i32
            .wrapping_sub(c_int(cmax(1.0 + max.log2().floor(), (-min).log2().ceil())))
            .clamp(6, 9) as u8;
        let scale = (1u32 << p.ar_coeff_shift) as f64;
        for (c, out) in [
            &mut p.ar_coeffs_y[..],
            &mut p.ar_coeffs_cb[..],
            &mut p.ar_coeffs_cr[..],
        ]
        .into_iter()
        .enumerate()
        {
            for i in 0..nc {
                out[i] = c_int((scale * self.combined[c].eq.x[i]).round()).clamp(-128, 127) as i16;
            }
            if c > 0 {
                out[nc] = c_int((scale * ycorr[c - 1]).round()).clamp(-128, 127) as i16;
            }
        }
        p.cb_mult = 128;
        p.cr_mult = 128;
        p.cb_luma_mult = 192;
        p.cr_luma_mult = 192;
        p.cb_offset = 256;
        p.cr_offset = 256;
        p.overlap_flag = true;
        Some(p)
    }
}
