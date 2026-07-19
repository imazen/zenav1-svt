//! Differential parity: the HIGHBD (`is_16bit` / 10-bit) loop-restoration
//! kernels vs the C reference.
//!
//! The bd8 twin (`c_parity_wiener.rs`) pins the 8-bit family. C keeps a
//! parallel highbd implementation of every kernel the Wiener SEARCH touches,
//! selected by `cm->use_highbitdepth` (restoration_pick.c:1243), and at
//! `encoder_bit_depth == 10` those — not the 8-bit ones — are what decides
//! `lr_type` and the coded taps. The risky deltas they carry:
//!
//! * `svt_av1_compute_stats_highbd_c` accumulates `int32` windowed diffs into
//!   `int64` (bd8 uses `int16`/`int32`) and then integer-DIVIDES every M and H
//!   entry by `bit_depth_divider` (4 at 10-bit) — applied after accumulation,
//!   so it is not equivalent to scaling the inputs, and it is where a naive
//!   "reuse the 8-bit stats" port silently diverges.
//! * `svt_av1_highbd_wiener_convolve_add_src_c` carries `bd` in three places
//!   (both rounding offsets and the intermediate clamp) plus the final
//!   `clip_pixel_highbd`.
//! * `svt_aom_highbd_get_sse` truncates each partial sum to `uint32_t` before
//!   accumulating; at 10 bits a tall right-edge strip can genuinely exceed
//!   2^32, so the truncation is observable.

use svtav1_cref as cref;
use svtav1_dsp::restoration as rst;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// A 10-bit pixel.
    fn px10(&mut self) -> u16 {
        (self.next() % 1024) as u16
    }
    fn tap(&mut self, min: i32, max: i32) -> i16 {
        (min + self.range((max - min + 1) as u64) as i32) as i16
    }
}

/// A signalable Wiener filter row (same construction as the bd8 twin):
/// taps 0..2 in their syntax ranges, tap 3 derived so the row sums to 128.
fn random_filter(rng: &mut Rng, win5: bool) -> [i16; 8] {
    let t0 = if win5 { 0 } else { rng.tap(-5, 10) };
    let t1 = rng.tap(-23, 8);
    let t2 = rng.tap(-17, 46);
    let mid = 128 - 2 * (t0 + t1 + t2);
    [t0, t1, t2, mid, t2, t1, t0, 0]
}

fn fill10(rng: &mut Rng, buf: &mut [u16], class_: u64) {
    match class_ {
        0 => {
            for v in buf.iter_mut() {
                *v = rng.px10();
            }
        }
        1 => {
            // Smooth ramp — the near-flat content where the tap solve is
            // most sensitive.
            for (i, v) in buf.iter_mut().enumerate() {
                *v = ((i as u64 * 7 + rng.range(4)) % 1024) as u16;
            }
        }
        _ => {
            // Full-range extremes (0 and 1023) — pins the clamps.
            for v in buf.iter_mut() {
                *v = if rng.range(2) == 0 { 0 } else { 1023 };
            }
        }
    }
}

/// `svt_av1_highbd_wiener_convolve_add_src_c` (convolve.c:200) at bd10.
#[test]
fn highbd_wiener_convolve_matches_c() {
    let mut rng = Rng(0x5EED_1001);
    for iter in 0..400 {
        let w = 1 + rng.range(64) as usize;
        let h = 1 + rng.range(64) as usize;
        let b = 4usize; // top3/left3/bottom3/right4 + slack
        let stride = w + 2 * b + rng.range(9) as usize;
        let rows = h + 2 * b;
        let origin = b * stride + b;
        let mut src = vec![0u16; stride * rows];
        fill10(&mut rng, &mut src, iter % 3);

        let win5 = iter % 2 == 0;
        let hf = random_filter(&mut rng, win5);
        let vf = random_filter(&mut rng, win5);

        let mut dst_c = vec![0u16; stride * rows];
        let mut dst_r = vec![0u16; stride * rows];
        cref::highbd_wiener_convolve_add_src(
            &src, origin, stride, &mut dst_c, origin, stride, &hf, &vf, w, h, 10,
        );
        rst::wiener_convolve_add_src_hbd(
            &src, origin, stride, &mut dst_r, origin, stride, &hf, &vf, w, h, 10,
        );
        for y in 0..h {
            for x in 0..w {
                assert_eq!(
                    dst_c[origin + y * stride + x],
                    dst_r[origin + y * stride + x],
                    "iter {iter} px ({x},{y}) w{w} h{h} win5={win5} hf={hf:?} vf={vf:?}"
                );
            }
        }
    }
}

/// `svt_extend_frame(.., highbd = 1)` -> `extend_frame_highbd`.
#[test]
fn highbd_extend_frame_matches_c() {
    let mut rng = Rng(0x5EED_1002);
    for iter in 0..100 {
        let w = 1 + rng.range(70) as usize;
        let h = 1 + rng.range(70) as usize;
        let bh = 1 + rng.range(4) as usize;
        let bv = 1 + rng.range(4) as usize;
        let stride = w + 2 * bh + rng.range(5) as usize;
        let rows = h + 2 * bv;
        let origin = bv * stride + bh;
        let mut base = vec![0u16; stride * rows];
        fill10(&mut rng, &mut base, iter % 3);

        let mut c = base.clone();
        let mut r = base.clone();
        cref::extend_frame_highbd(&mut c, origin, w, h, stride, bh, bv);
        rst::extend_frame(&mut r, origin, w, h, stride, bh, bv);
        assert_eq!(c, r, "iter {iter} w{w} h{h} bh{bh} bv{bv}");
    }
}

/// `svt_av1_compute_stats_highbd_c` (restoration_pick.c:692) at bd10 and
/// bd12 — the M/H moments the tap solve consumes.
#[test]
fn highbd_compute_stats_matches_c() {
    let mut rng = Rng(0x5EED_1003);
    let mut saw_divider_effect = 0u32;
    for iter in 0..80 {
        let w = 8 + rng.range(72) as usize;
        let h = 8 + rng.range(72) as usize;
        let b = 4usize;
        let stride = w + 2 * b;
        let rows = h + 2 * b;
        let origin = b * stride + b;
        let mut dgd = vec![0u16; stride * rows];
        let mut src = vec![0u16; stride * rows];
        fill10(&mut rng, &mut dgd, iter % 3);
        fill10(&mut rng, &mut src, (iter + 1) % 3);
        rst::extend_frame(&mut dgd, origin, w, h, stride, 4, 3);

        let win = if iter % 2 == 0 { 5 } else { 7 };
        let win2 = win * win;
        let bd: u8 = if iter % 5 == 4 { 12 } else { 10 };
        let mut m_c = vec![0i64; win2];
        let mut h_c = vec![0i64; win2 * win2];
        let mut m_r = vec![0i64; win2];
        let mut h_r = vec![0i64; win2 * win2];
        let h_start = rng.range(4) as i32;
        let v_start = rng.range(4) as i32;
        let h_end = w as i32 - rng.range(4) as i32;
        let v_end = h as i32 - rng.range(4) as i32;

        cref::compute_stats_highbd(
            win,
            &dgd,
            origin,
            stride,
            &src,
            origin,
            stride,
            h_start,
            h_end,
            v_start,
            v_end,
            &mut m_c,
            &mut h_c,
            bd as i32,
        );
        rst::compute_stats_hbd(
            win, &dgd, origin, stride, &src, origin, stride, h_start, h_end, v_start, v_end,
            &mut m_r, &mut h_r, bd,
        );
        assert_eq!(m_c, m_r, "M diverges iter {iter} win {win} bd {bd}");
        assert_eq!(h_c, h_r, "H diverges iter {iter} win {win} bd {bd}");

        // Non-vacuity: the bit_depth_divider must actually be doing work —
        // if it were dropped, at least one entry would differ from the
        // divided result. Compare against a bd8-divider (=1) run.
        let mut m_nodiv = vec![0i64; win2];
        let mut h_nodiv = vec![0i64; win2 * win2];
        rst::compute_stats_hbd(
            win,
            &dgd,
            origin,
            stride,
            &src,
            origin,
            stride,
            h_start,
            h_end,
            v_start,
            v_end,
            &mut m_nodiv,
            &mut h_nodiv,
            8,
        );
        if m_nodiv != m_r || h_nodiv != h_r {
            saw_divider_effect += 1;
        }
    }
    assert!(
        saw_divider_effect > 70,
        "bit_depth_divider had no observable effect in {} of 80 rounds",
        80 - saw_divider_effect
    );
}

/// `svt_av1_loop_restoration_filter_unit(.., highbd = 1, bit_depth = 10)`
/// with `need_boundaries = 0` — the SEARCH-path unit filter
/// (`use_boundaries_in_rest_search = 0`), including the stripe split.
#[test]
fn highbd_filter_unit_search_matches_c() {
    let mut rng = Rng(0x5EED_1004);
    for iter in 0..200 {
        let ss = (iter % 2) as i32;
        let plane_w = (8 + rng.range(140) as i32) >> ss << ss.max(0);
        let plane_h = (8 + rng.range(140) as i32) >> ss << ss.max(0);
        let pw = (plane_w >> ss).max(1);
        let ph = (plane_h >> ss).max(1);

        let b = 4usize;
        let stride = pw as usize + 2 * b;
        let rows = ph as usize + 2 * b;
        let origin = b * stride + b;

        let mut data = vec![0u16; stride * rows];
        fill10(&mut rng, &mut data, iter % 3);
        rst::extend_frame(&mut data, origin, pw as usize, ph as usize, stride, 3, 3);

        let tile_rect = (0i32, 0i32, pw, ph);
        let rtype = if iter % 8 == 7 {
            rst::RESTORE_NONE
        } else {
            rst::RESTORE_WIENER
        };
        let win5 = iter % 3 == 0;
        let vf = random_filter(&mut rng, win5);
        let hf = random_filter(&mut rng, win5);
        let limits = (0i32, pw, 0i32, ph);

        let mut data_c = data.clone();
        let mut dst_c = vec![0u16; stride * rows];
        let mut dst_r = vec![0u16; stride * rows];

        cref::loop_restoration_filter_unit_highbd(
            limits,
            rtype as i32,
            &vf,
            &hf,
            tile_rect,
            0,
            ss,
            ss,
            10,
            &mut data_c,
            origin,
            stride,
            &mut dst_c,
            origin,
            stride,
        );
        rst::loop_restoration_filter_unit_search_hbd(
            &rst::TileLimits {
                h_start: limits.0,
                h_end: limits.1,
                v_start: limits.2,
                v_end: limits.3,
            },
            rtype,
            &rst::WienerInfo {
                vfilter: vf,
                hfilter: hf,
            },
            &rst::PixelRect {
                left: tile_rect.0,
                top: tile_rect.1,
                right: tile_rect.2,
                bottom: tile_rect.3,
            },
            0,
            ss,
            ss,
            &data,
            origin,
            stride,
            &mut dst_r,
            origin,
            stride,
            10,
        );
        // `need_boundaries = 0` must not touch `data`.
        assert_eq!(data_c, data, "iter {iter}: C mutated data at need_boundaries=0");
        for y in 0..ph as usize {
            for x in 0..pw as usize {
                assert_eq!(
                    dst_c[origin + y * stride + x],
                    dst_r[origin + y * stride + x],
                    "iter {iter} px ({x},{y}) pw{pw} ph{ph} ss{ss} rtype {rtype} win5={win5}"
                );
            }
        }
    }
}

/// `svt_aom_highbd_get_sse` (svt_psnr.c:93) — `sse_restoration_unit` at
/// `highbd = 1`. Covers the 16x16 tiling, both edge strips, and (via tall
/// narrow-strip geometry at full 10-bit contrast) the `(uint32_t)` partial-sum
/// truncation.
#[test]
fn highbd_sse_region_matches_c() {
    let mut rng = Rng(0x5EED_1005);
    for iter in 0..300 {
        // Bias toward `width % 16 != 0` with a large height so the right
        // strip is tall — the geometry where C's u32 truncation can bite.
        let (w, h) = if iter % 3 == 0 {
            (1 + rng.range(31) as usize, 200 + rng.range(200) as usize)
        } else {
            (1 + rng.range(200) as usize, 1 + rng.range(200) as usize)
        };
        let a_stride = w + rng.range(8) as usize;
        let b_stride = w + rng.range(8) as usize;
        let mut a = vec![0u16; a_stride * (h + 2)];
        let mut b = vec![0u16; b_stride * (h + 2)];
        // Class 2 = extremes: maximizes the per-pixel squared error (1023^2)
        // so tall strips reach the truncation threshold.
        let class_ = if iter % 3 == 0 { 2 } else { iter % 3 };
        fill10(&mut rng, &mut a, class_);
        fill10(&mut rng, &mut b, if class_ == 2 { 2 } else { (class_ + 1) % 3 });

        let ours = rst::sse_region_hbd(&a, 0, a_stride, &b, 0, b_stride, w, h);
        let theirs = cref::highbd_get_sse(&a, 0, a_stride, &b, 0, b_stride, w, h);
        assert_eq!(ours, theirs, "iter {iter} w{w} h{h} class {class_}");
    }
}
