//! Differential parity: Wiener loop-restoration kernel + statistics + the
//! full per-unit stripe machinery vs the C reference
//! (`svt_av1_wiener_convolve_add_src_c`, `svt_av1_compute_stats_c`,
//! `svt_av1_loop_restoration_filter_unit`, `svt_extend_frame`).
//!
//! The unit filter is what the DECODER runs on the post-CDEF frame; the
//! kernel + stats feed the encoder's tap search. A single bit of divergence
//! breaks recon parity (encoder recon vs aomdec) frame-wide.

use svtav1_cref as cref;
use svtav1_dsp::restoration as rst;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn tap(&mut self, min: i32, max: i32) -> i16 {
        (min + self.range((max - min + 1) as u64) as i32) as i16
    }
}

/// Build a signalable Wiener filter row: taps 0..2 random within their
/// signalable bounds (tap0 forced 0 for 5-tap filters), center derived,
/// mirrored, tap[7] = 0 — exactly the post-`finalize_sym_filter` shape.
fn random_filter(rng: &mut Rng, win5: bool) -> [i16; 8] {
    let t0 = if win5 {
        0
    } else {
        rng.tap(rst::WIENER_FILT_TAP0_MINV, rst::WIENER_FILT_TAP0_MAXV)
    };
    let t1 = rng.tap(rst::WIENER_FILT_TAP1_MINV, rst::WIENER_FILT_TAP1_MAXV);
    let t2 = rng.tap(rst::WIENER_FILT_TAP2_MINV, rst::WIENER_FILT_TAP2_MAXV);
    [t0, t1, t2, -2 * (t0 + t1 + t2), t2, t1, t0, 0]
}

/// Random content classes: pure noise, smooth gradient + noise, flat.
fn fill_content(rng: &mut Rng, buf: &mut [u8], stride: usize, w: usize, h: usize, class_: u64) {
    for y in 0..h {
        for x in 0..w {
            let v = match class_ {
                0 => rng.byte(),
                1 => ((x * 255 / w.max(1)) as u8).wrapping_add(rng.byte() & 15),
                _ => 128u8.wrapping_add((rng.byte() & 7).wrapping_sub(3)),
            };
            buf[y * stride + x] = v;
        }
    }
}

/// Kernel parity: random blocks x random signalable filters, 7- and 5-tap,
/// odd sizes and strides, content classes. Also proves the exact access
/// pattern (3/3/3/4 margins) is what C touches: the planes only carry those
/// margins plus the C-side sanitizer slack.
#[test]
fn wiener_convolve_matches_c() {
    let mut rng = Rng(0x5EED_0001);
    for iter in 0..400 {
        let w = 1 + rng.range(64) as usize;
        let h = 1 + rng.range(64) as usize;
        let b = 4usize; // enough for top3/left3/bottom3/right4
        let stride = w + 2 * b + rng.range(9) as usize;
        let rows = h + 2 * b;
        let origin = b * stride + b;
        let mut src = vec![0u8; stride * rows];
        let n = src.len();
        fill_content(&mut rng, &mut src, 1, n, 1, iter % 3);

        let win5 = iter % 2 == 0;
        let hf = random_filter(&mut rng, win5);
        let vf = random_filter(&mut rng, win5);

        let mut dst_c = vec![0u8; stride * rows];
        let mut dst_r = vec![0u8; stride * rows];
        cref::wiener_convolve_add_src(
            &src, origin, stride, &mut dst_c, origin, stride, &hf, &vf, w, h,
        );
        rst::wiener_convolve_add_src(
            &src, origin, stride, &mut dst_r, origin, stride, &hf, &vf, w, h,
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

/// extend_frame parity: the border replication both the search and the
/// application run before filtering.
#[test]
fn extend_frame_matches_c() {
    let mut rng = Rng(0x5EED_0002);
    for iter in 0..100 {
        let w = 1 + rng.range(70) as usize;
        let h = 1 + rng.range(70) as usize;
        let bh = 1 + rng.range(4) as usize;
        let bv = 1 + rng.range(4) as usize;
        let stride = w + 2 * bh + rng.range(5) as usize;
        let rows = h + 2 * bv;
        let origin = bv * stride + bh;
        let mut base = vec![0u8; stride * rows];
        let n = base.len();
        fill_content(&mut rng, &mut base, 1, n, 1, iter % 3);

        let mut c = base.clone();
        let mut r = base.clone();
        cref::extend_frame(&mut c, origin, w, h, stride, bh, bv);
        rst::extend_frame(&mut r, origin, w, h, stride, bh, bv);
        assert_eq!(c, r, "iter {iter} w{w} h{h} bh{bh} bv{bv}");
    }
}

/// compute_stats parity: M/H moments over random regions for both window
/// sizes (the search uses win=5 at the M6 controls; win=7 covers
/// filter_tap_lvl=1 presets).
#[test]
fn compute_stats_matches_c() {
    let mut rng = Rng(0x5EED_0003);
    for iter in 0..60 {
        let w = 8 + rng.range(72) as usize;
        let h = 8 + rng.range(72) as usize;
        let b = 4usize;
        let stride = w + 2 * b;
        let rows = h + 2 * b;
        let origin = b * stride + b;
        let mut dgd = vec![0u8; stride * rows];
        let mut src = vec![0u8; stride * rows];
        let n = dgd.len();
        fill_content(&mut rng, &mut dgd, 1, n, 1, iter % 3);
        let n2 = src.len();
        fill_content(&mut rng, &mut src, 1, n2, 1, (iter + 1) % 3);
        // Borders must be defined for dgd (the window reads +-halfwin).
        rst::extend_frame(&mut dgd, origin, w, h, stride, 4, 3);

        let win = if iter % 2 == 0 { 5 } else { 7 };
        let win2 = win * win;
        let mut m_c = vec![0i64; win2];
        let mut h_c = vec![0i64; win2 * win2];
        let mut m_r = vec![0i64; win2];
        let mut h_r = vec![0i64; win2 * win2];
        // Random sub-region (mimics RU limits).
        let h_start = rng.range(4) as i32;
        let v_start = rng.range(4) as i32;
        let h_end = w as i32 - rng.range(4) as i32;
        let v_end = h as i32 - rng.range(4) as i32;

        cref::compute_stats(
            win, &dgd, origin, stride, &src, origin, stride, h_start, h_end, v_start, v_end,
            &mut m_c, &mut h_c,
        );
        rst::compute_stats(
            win, &dgd, origin, stride, &src, origin, stride, h_start, h_end, v_start, v_end,
            &mut m_r, &mut h_r,
        );
        assert_eq!(m_c, m_r, "M diverges iter {iter} win {win}");
        assert_eq!(h_c, h_r, "H diverges iter {iter} win {win}");
    }
}

/// compute_stats — EVERY dispatch tier byte-identical to real C (and hence to
/// each other). The suite above runs whatever tier this host picks (v3 here),
/// pinning v3==C; this forces both the AVX2 (`_v3`) kernel and the scalar
/// reference (`_scalar`, also the aarch64 `_neon` fallback) and asserts each
/// == real C AND == the first tier's output. Since the scalar tier == C and
/// the v3 tier == C, the SIMD path == the scalar path (the task's "SIMD ==
/// real-C AND == scalar"). Covers both window sizes, all content classes, and
/// edge regions: widths < 8 (scalar-tail only), widths 8/16 (whole-vector),
/// off-by-one tails (9, 17, …), 1-row / 1-column regions, and tall regions
/// (many flush rows). The scalar-tail path (win2 = 25 and 49 are both not
/// multiples of 8) is exercised on every case.
#[test]
fn compute_stats_all_tiers_match_c() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut rng = Rng(0x5EED_57A7);
    // Explicit region widths/heights hitting the SIMD boundary conditions,
    // plus random sizes. (w, h) are the plane content extents; the region
    // (h_start..h_end, v_start..v_end) is carved inside with small margins.
    let widths = [1usize, 2, 5, 7, 8, 9, 15, 16, 17, 24, 25, 33, 48, 64, 80];
    for iter in 0..220usize {
        let (w, h) = if iter < widths.len() * 2 {
            let wv = widths[iter % widths.len()];
            let hv = if iter < widths.len() { 1 + (iter % 6) } else { widths[(iter + 3) % widths.len()] };
            (wv, hv)
        } else {
            (1 + rng.range(90) as usize, 1 + rng.range(90) as usize)
        };
        let b = 5usize; // >= halfwin(3) + slack for +-window reads
        let stride = w + 2 * b + rng.range(9) as usize;
        let rows = h + 2 * b;
        let origin = b * stride + b;
        let mut dgd = vec![0u8; stride * rows];
        let mut src = vec![0u8; stride * rows];
        let nd = dgd.len();
        fill_content(&mut rng, &mut dgd, 1, nd, 1, (iter % 3) as u64);
        let ns = src.len();
        fill_content(&mut rng, &mut src, 1, ns, 1, ((iter + 2) % 3) as u64);
        // Window reads +-halfwin around every region pixel; define those borders.
        rst::extend_frame(&mut dgd, origin, w, h, stride, 4, 4);

        let win = if iter % 2 == 0 { 5 } else { 7 };
        let win2 = win * win;

        // Region: full plane on some iters, a carved sub-region on others.
        let (h_start, h_end, v_start, v_end) = if iter % 3 == 0 {
            (0i32, w as i32, 0i32, h as i32)
        } else {
            let hs = rng.range((w as u64).max(1)) as i32 * (w > 1) as i32;
            let vs = rng.range((h as u64).max(1)) as i32 * (h > 1) as i32;
            let he = (w as i32).max(hs + 1);
            let ve = (h as i32).max(vs + 1);
            (hs.min(he - 1).max(0), he, vs.min(ve - 1).max(0), ve)
        };

        let mut m_c = vec![0i64; win2];
        let mut h_c = vec![0i64; win2 * win2];
        cref::compute_stats(
            win, &dgd, origin, stride, &src, origin, stride, h_start, h_end, v_start, v_end,
            &mut m_c, &mut h_c,
        );

        // Reference tier snapshot (asserted equal across all tiers too).
        let mut m_first: Option<Vec<i64>> = None;
        let mut h_first: Option<Vec<i64>> = None;
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut m_r = vec![0i64; win2];
            let mut h_r = vec![0i64; win2 * win2];
            rst::compute_stats(
                win, &dgd, origin, stride, &src, origin, stride, h_start, h_end, v_start, v_end,
                &mut m_r, &mut h_r,
            );
            assert_eq!(
                m_r, m_c,
                "M tier!=C iter {iter} win {win} w{w} h{h} reg[{h_start},{h_end})x[{v_start},{v_end})"
            );
            assert_eq!(
                h_r, h_c,
                "H tier!=C iter {iter} win {win} w{w} h{h} reg[{h_start},{h_end})x[{v_start},{v_end})"
            );
            if let (Some(mf), Some(hf)) = (m_first.as_ref(), h_first.as_ref()) {
                assert_eq!(&m_r, mf, "M tier!=tier0 iter {iter}");
                assert_eq!(&h_r, hf, "H tier!=tier0 iter {iter}");
            } else {
                m_first = Some(m_r);
                h_first = Some(h_r);
            }
        });
    }
}

/// Full unit filter parity WITH the stripe-boundary machinery: random frame
/// sizes (including the 64/128 identity cells and odd sizes), random
/// boundary buffer contents (standing in for the saved deblock/CDEF lines),
/// luma and subsampled chroma geometry, both need_boundaries arms. `data`
/// must also be byte-identical AFTER the call (setup/restore must undo its
/// temporary edits exactly).
#[test]
fn filter_unit_matches_c() {
    let mut rng = Rng(0x5EED_0004);
    for iter in 0..200 {
        let ss = (iter % 2) as i32; // 0 = luma geometry, 1 = chroma
        let plane_w = (8 + rng.range(140) as i32) >> ss << ss.max(0);
        let plane_h = (8 + rng.range(140) as i32) >> ss << ss.max(0);
        let pw = (plane_w >> ss).max(1);
        let ph = (plane_h >> ss).max(1);

        let b = 4usize;
        let stride = pw as usize + 2 * b;
        let rows = ph as usize + 2 * b;
        let origin = b * stride + b;

        let mut data = vec![0u8; stride * rows];
        let n = data.len();
        fill_content(&mut rng, &mut data, 1, n, 1, iter % 3);
        // The application extends by RESTORATION_BORDER before filtering.
        rst::extend_frame(&mut data, origin, pw as usize, ph as usize, stride, 3, 3);

        // Boundary buffers in the C layout.
        let mut bnd = rst::alloc_stripe_boundaries(plane_w, plane_h, ss);
        for v in bnd.above.iter_mut() {
            *v = rng.byte();
        }
        for v in bnd.below.iter_mut() {
            *v = rng.byte();
        }

        let tile_rect = (0i32, 0i32, pw, ph);
        let need_boundaries = iter % 4 != 3; // mostly the decoder arm
        let rtype = if iter % 8 == 7 {
            rst::RESTORE_NONE
        } else {
            rst::RESTORE_WIENER
        };
        let win5 = iter % 3 == 0;
        let vf = random_filter(&mut rng, win5);
        let hf = random_filter(&mut rng, win5);

        // Single unit covering the plane (the ported presets' unit size 256
        // covers every tracked frame), like foreach_rest_unit computes it.
        let limits = {
            let mut l = (0i32, pw, 0i32, ph);
            // v_start offset (already 0), v_end stays plane height for the
            // last unit.
            l.2 = 0;
            l
        };

        let mut data_c = data.clone();
        let mut data_r = data.clone();
        let mut dst_c = vec![0u8; stride * rows];
        let mut dst_r = vec![0u8; stride * rows];

        cref::loop_restoration_filter_unit(
            need_boundaries,
            limits,
            rtype,
            &vf,
            &hf,
            &bnd.above,
            &bnd.below,
            bnd.stride,
            tile_rect,
            0,
            ss,
            ss,
            &mut data_c,
            origin,
            stride,
            &mut dst_c,
            origin,
            stride,
        );
        let rlimits = rst::TileLimits {
            h_start: limits.0,
            h_end: limits.1,
            v_start: limits.2,
            v_end: limits.3,
        };
        let rrect = rst::PixelRect {
            left: tile_rect.0,
            top: tile_rect.1,
            right: tile_rect.2,
            bottom: tile_rect.3,
        };
        let wi = rst::WienerInfo {
            vfilter: vf,
            hfilter: hf,
        };
        rst::loop_restoration_filter_unit(
            need_boundaries,
            &rlimits,
            rtype,
            &wi,
            &bnd,
            &rrect,
            0,
            ss,
            ss,
            &mut data_r,
            origin,
            stride,
            &mut dst_r,
            origin,
            stride,
        );

        for y in 0..ph as usize {
            for x in 0..pw as usize {
                assert_eq!(
                    dst_c[origin + y * stride + x],
                    dst_r[origin + y * stride + x],
                    "dst diverges iter {iter} px ({x},{y}) pw{pw} ph{ph} ss{ss} nb={need_boundaries} rtype={rtype}"
                );
            }
        }
        assert_eq!(data_c, data_r, "post-call data diverges iter {iter} (setup/restore mismatch)");
    }
}

// The subexp-with-reference tap coding chain is differentially tested in
// svtav1-entropy/tests/c_parity_lr_syntax.rs (it lives in the entropy
// crate; svtav1-dsp does not depend on it).
