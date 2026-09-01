//! Tier-1 differentials for `svtav1_encoder::port_tune_vmaf` — the
//! `--tune vmaf` luma preprocessing chain.
//!
//! The six leaf kernels (temporal_filtering.c:3636-3746) are driven through
//! their REAL exported `_c` symbols (`docs/WORKING-ON-THIS.md` §4 tier 1). The
//! nine helpers that assemble them (pic_analysis_process.c:1642-1899) are
//! `static` in C and reached only through a PictureAnalysisContext plus a PCS,
//! so their constants are pinned here at TIER 4 with the C line cited, and the
//! composition around them is exercised by feeding the tier-1 kernels' own
//! outputs through it.
//!
//! Every kernel test is mutation-checked: see the `port_tune_vmaf` commit
//! message for the list of constants each one separates.

use svtav1_cref::temporal_filtering as cref;
use svtav1_encoder::port_tune_vmaf as port;

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
        (self.next() >> 33) as u8
    }
}

/// Planes chosen so each kernel sees flat, low-contrast, structured and noisy
/// input — a kernel that agrees only on noise has not been tested at its
/// thresholds.
///
/// Four of these were added after a mutation run showed the first suite could
/// not separate constants the port depends on, each for a named reason:
/// `madedge` (a block whose truncated and rounded means give different MADs),
/// `diagbars` (oriented at 45 degrees, so the structure tensor's `sum_xy` is
/// large — every other plane leaves it near zero and the coherence formula's
/// `4 * xy * xy` term invisible), and the `sparse*` family (a tunable detail
/// fraction, so `vmaf_preprocess_frame`'s 85%-flat busy-frame test is crossed
/// from both sides instead of always landing on one).
fn planes(_w: usize, h: usize, stride: usize) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let n = stride * (h + 2);
    let mut out: Vec<(String, Vec<u8>)> = vec![
        ("flat0".into(), vec![0u8; n]),
        ("flat128".into(), vec![128u8; n]),
        ("flat255".into(), vec![255u8; n]),
        ("noise".into(), (0..n).map(|_| rng.byte()).collect()),
    ];
    // A near-flat plane: MAD lands in the 2..5 and 5..12 spatial tiers.
    out.push((
        "lowamp".into(),
        (0..n).map(|_| 128u8.wrapping_add(rng.byte() % 7)).collect(),
    ));
    out.push((
        "midamp".into(),
        (0..n)
            .map(|_| 128u8.wrapping_add(rng.byte() % 25))
            .collect(),
    ));
    // Vertical edges: gradient energy is entirely on one axis, so coherence
    // is near 1 and `sum_xy` is zero.
    let mut vert = vec![0u8; n];
    for r in 0..h + 2 {
        for c in 0..stride {
            vert[r * stride + c] = if (c / 3) % 2 == 0 { 30 } else { 220 };
        }
    }
    out.push(("vertbars".into(), vert));
    // 45-degree edges: `grad_x` and `grad_y` are equal and correlated, so
    // `sum_xy` carries the whole tensor. Without this the coherence formula's
    // `4 * xy * xy` term can be changed to `3 * xy * xy` undetected.
    let mut diag = vec![0u8; n];
    for r in 0..h + 2 {
        for c in 0..stride {
            diag[r * stride + c] = if ((r + c) / 3) % 2 == 0 { 20 } else { 235 };
        }
    }
    out.push(("diagbars".into(), diag));
    // The other diagonal, so `sum_xy` is driven negative as well as positive.
    let mut anti = vec![0u8; n];
    for r in 0..h + 2 {
        for c in 0..stride {
            anti[r * stride + c] = if ((r + stride - c) / 3).is_multiple_of(2) {
                20
            } else {
                235
            };
        }
    }
    out.push(("antidiag".into(), anti));
    // A diagonal ramp: oriented but smooth.
    let mut ramp = vec![0u8; n];
    for r in 0..h + 2 {
        for c in 0..stride {
            ramp[r * stride + c] = ((r * 3 + c * 5) % 256) as u8;
        }
    }
    out.push(("ramp".into(), ramp));
    // Isolated impulses on a flat field: high energy, low coherence.
    let mut imp = vec![110u8; n];
    for i in 0..n / 37 {
        imp[i * 37] = 250;
    }
    out.push(("impulse".into(), imp));
    // 8x8 tiles of 56 zeros and 8 fives. `sum` is 40, so C's truncated
    // `sum >> 6` is 0 and a rounded `(sum + 32) >> 6` would be 1, and the MADs
    // those two means produce are 40 and 88 — a whole unit of `avg_mad` apart
    // on a one-block frame. This is what makes the truncation testable.
    let mut madedge = vec![0u8; n];
    for r in 0..h + 2 {
        for c in 0..stride {
            // The last row of every 8x8 tile is 5, the other seven are 0.
            madedge[r * stride + c] = if (r % 8) == 7 { 5 } else { 0 };
        }
    }
    out.push(("madedge".into(), madedge));
    // A tunable detail fraction, for the busy-frame test. A flat field with
    // `k` impulses per 1000 pixels: the blur smears each impulse over its
    // neighbourhood, so the fraction of pixels within 12 of the blur moves
    // smoothly with `k` and crosses 85%.
    // The step is swept FINELY, not in a handful of jumps: a coarse sweep left
    // a gap between 80% and 86% flat, and the busy-frame test sits at 85%, so
    // no cell straddled it and the 85 could be changed to an 84 undetected.
    for step in (2usize..40).chain([42, 45, 48, 52, 56, 60, 70, 85, 100, 130, 170, 220]) {
        let mut sp = vec![100u8; n];
        let mut i = 0;
        while i < n {
            sp[i] = 240;
            i += step;
        }
        out.push((format!("sparse{step}"), sp));
    }
    out.push(("sparse_none".into(), vec![100u8; n]));
    out
}

#[test]
fn compute_avg_mad_matches_c() {
    let mut cells = 0usize;
    let mut nonzero = 0usize;
    for (w, h) in [
        (64usize, 48usize),
        (24, 16),
        (7, 5),
        (8, 8),
        (65, 33),
        // Heights that are NOT multiples of 8, with a width that admits whole
        // blocks: without one of these, shrinking the block loop's bound from
        // `by + 8 <= height` to `by + 4` changes nothing at any size in the
        // suite, because 8 divides every other height here and the 7x5 cell
        // has no whole block at all.
        (24, 12),
        (16, 5),
        (40, 21),
    ] {
        let stride = w + 6;
        for (name, plane) in planes(w, h, stride) {
            let c = cref::vmaf_compute_avg_mad(&plane, w, h, stride);
            let r = port::compute_avg_mad(&plane, w, h, stride);
            assert_eq!(r, c, "avg_mad {name} {w}x{h}");
            if c != 0 {
                nonzero += 1;
            }
            cells += 1;
        }
    }
    assert_eq!(cells, 8 * planes(1, 1, 1).len());
    assert!(nonzero > 0, "every avg_mad probe returned 0");
    // 7x5 has no whole 8x8 block, which is C's `block_count == 0` arm.
    let flat = vec![7u8; 13 * 7];
    assert_eq!(port::compute_avg_mad(&flat, 7, 5, 13), 0);
    assert_eq!(cref::vmaf_compute_avg_mad(&flat, 7, 5, 13), 0);
}

#[test]
fn compute_gradient_coherence_matches_c() {
    let mut cells = 0usize;
    let mut distinct: Vec<u32> = Vec::new();
    for (w, h) in [
        (64usize, 48usize),
        (33, 19),
        (5, 5),
        (3, 3),
        (2, 2),
        (80, 80),
        (17, 35),
    ] {
        let stride = w + 6;
        for (name, plane) in planes(w, h, stride) {
            let c = cref::vmaf_compute_gradient_coherence(&plane, w, h, stride);
            let r = port::compute_gradient_coherence(&plane, w, h, stride);
            assert_eq!(
                r.to_bits(),
                c.to_bits(),
                "gradient coherence {name} {w}x{h}: port {r} vs C {c}"
            );
            if !distinct.contains(&c.to_bits()) {
                distinct.push(c.to_bits());
            }
            cells += 1;
        }
    }
    assert_eq!(cells, 7 * planes(1, 1, 1).len());
    // Anti-vacuity: 1.0 is the `weight_sum <= 0` fallback, so a suite that only
    // ever returns 1.0 has proved the fallback and nothing else.
    assert!(
        distinct.len() >= 5,
        "coherence took only {} distinct values",
        distinct.len()
    );
}

#[test]
fn hpass_row_matches_c() {
    let mut cells = 0usize;
    for w in [1usize, 2, 3, 4, 8, 17, 64, 65] {
        let stride = w + 6;
        for (name, plane) in planes(w, 3, stride) {
            let mut c_row = vec![0i16; w + 4];
            let mut r_row = vec![0i16; w + 4];
            cref::vmaf_hpass_row(&plane, w, &mut c_row);
            port::hpass_row(&plane, w, &mut r_row);
            assert_eq!(r_row, c_row, "hpass {name} w={w}");
            // The two cascaded [1,2,1] stages give gain 16, so nothing can
            // exceed 16 * 255 and the int16 store cannot wrap. Checked, not
            // assumed — the port's doc comment claims it.
            assert!(
                c_row.iter().all(|v| (0..=4080).contains(v)),
                "hpass output left [0, 4080] for {name} w={w}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 8 * planes(1, 1, 1).len());
}

#[test]
fn vpass_row_matches_c() {
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    let mut cells = 0usize;
    for w in [1usize, 4, 17, 64] {
        for trial in 0..6 {
            let rows: Vec<Vec<i16>> = (0..5)
                .map(|k| {
                    (0..w + 4)
                        .map(|_| match trial {
                            0 => 0,
                            1 => 4080,
                            2 => (k as i16) * 500,
                            _ => (rng.next() % 4081) as i16,
                        })
                        .collect()
                })
                .collect();
            let view = [
                rows[0].as_slice(),
                rows[1].as_slice(),
                rows[2].as_slice(),
                rows[3].as_slice(),
                rows[4].as_slice(),
            ];
            let mut c_out = vec![0u8; w];
            let mut r_out = vec![0u8; w];
            cref::vmaf_vpass_row(view, &mut c_out, w, 2);
            port::vpass_row(view, &mut r_out, w, 2);
            assert_eq!(r_out, c_out, "vpass w={w} trial={trial}");
            cells += 1;
        }
    }
    assert_eq!(cells, 4 * 6);
}

#[test]
fn apply_unsharp_row_matches_c() {
    let mut rng = Rng(0x0F0F_0F0F_1111_2222);
    let mut cells = 0usize;
    let mut clipped_low = 0usize;
    let mut clipped_high = 0usize;
    for w in [1usize, 7, 64] {
        for &amount in &[0i32, 1, 4915, 9830, 32768, 65535] {
            for &max_delta in &[0i32, 1, 4, 8, 12, 200] {
                let src: Vec<u8> = (0..w).map(|_| rng.byte()).collect();
                let blur: Vec<u8> = (0..w).map(|_| rng.byte()).collect();
                let mut c_out = vec![0u8; w];
                let mut r_out = vec![0u8; w];
                cref::vmaf_apply_unsharp_row(&src, &blur, &mut c_out, w, amount, max_delta);
                port::apply_unsharp_row(&src, &blur, &mut r_out, w, amount, max_delta);
                assert_eq!(
                    r_out, c_out,
                    "unsharp w={w} amount={amount} clip={max_delta}"
                );
                for j in 0..w {
                    if c_out[j] == 0 && src[j] != 0 {
                        clipped_low += 1;
                    }
                    if c_out[j] == 255 && src[j] != 255 {
                        clipped_high += 1;
                    }
                }
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 3 * 6 * 6);
    // Both output clamps must have fired, or the `clamp(0, 255)` is untested.
    assert!(clipped_low > 0, "the 0 clamp never fired");
    assert!(clipped_high > 0, "the 255 clamp never fired");
}

#[test]
fn count_detail_le_matches_c() {
    let mut rng = Rng(0xABCD_1234_5678_9999);
    let mut cells = 0usize;
    let mut saw_partial = false;
    for (w, h) in [(64usize, 48usize), (7, 5), (1, 1), (33, 17)] {
        let stride = w + 6;
        let src: Vec<u8> = (0..stride * h).map(|_| rng.byte()).collect();
        let blur: Vec<u8> = (0..w * h).map(|_| rng.byte()).collect();
        for &thresh in &[-1i32, 0, 1, 12, 40, 255, 256] {
            let c = cref::vmaf_count_detail_le(&src, &blur, w, h, stride, thresh);
            let r = port::count_detail_le(&src, &blur, w, h, stride, thresh);
            assert_eq!(r, c, "count_detail_le {w}x{h} thresh={thresh}");
            if c != 0 && c != (w * h) as u32 {
                saw_partial = true;
            }
            cells += 1;
        }
    }
    assert_eq!(cells, 4 * 7);
    assert!(
        saw_partial,
        "the count was always 0 or all pixels, so the threshold is untested"
    );
}

// ---------------------------------------------------------------------------
// The assembled chain
// ---------------------------------------------------------------------------

/// The blur pass, checked against a C-driven reference built from the same two
/// tier-1 kernels. This does NOT drive `vmaf_box_blur_frame` itself — that one
/// is `static` and takes the analysis context's ring — so it is the ROW
/// SEQUENCING that is under test here, against C's own row kernels.
#[test]
fn box_blur_frame_matches_a_c_driven_reference() {
    for (w, h) in [(1usize, 1usize), (8, 3), (17, 9), (64, 48), (33, 5)] {
        let stride = w + 6;
        for (name, plane) in planes(w, h, stride) {
            // C-driven reference: exactly the loop at
            // pic_analysis_process.c:1773, with C's kernels in it.
            let mut rows: Vec<Vec<i16>> = (0..5).map(|_| vec![0i16; w + 4]).collect();
            let mut c_blur = vec![0u8; w * h];
            for k in 0..4usize {
                let row = (k as i32 - 2).clamp(0, h as i32 - 1) as usize;
                cref::vmaf_hpass_row(&plane[row * stride..], w, &mut rows[k]);
            }
            for m in 0..h {
                let row = (m + 2).min(h - 1);
                cref::vmaf_hpass_row(&plane[row * stride..], w, &mut rows[4]);
                let view = [
                    rows[0].as_slice(),
                    rows[1].as_slice(),
                    rows[2].as_slice(),
                    rows[3].as_slice(),
                    rows[4].as_slice(),
                ];
                cref::vmaf_vpass_row(view, &mut c_blur[m * w..], w, 2);
                rows.rotate_left(1);
            }

            let mut ring = port::VmafRing::new(w);
            let mut r_blur = vec![0u8; w * h];
            port::box_blur_frame(&plane, stride, &mut r_blur, w, h, &mut ring);
            assert_eq!(r_blur, c_blur, "box blur {name} {w}x{h}");
        }
    }
}

/// The whole `vmaf_preprocess_frame` chain, against a reference assembled from
/// the C kernels plus the port's own (tier-4, `static`-in-C) ladder.
///
/// What this catches is the SEQUENCING and the plumbing: which plane feeds
/// which kernel, the tightly-packed blur stride, the `>> 15`, the busy-frame
/// comparison, and the in-place rewrite. What it cannot catch is a wrong
/// constant in the ladder, which is why those are cited to their C lines and
/// pinned separately below.
#[test]
fn preprocess_frame_matches_a_c_driven_reference() {
    for (w, h) in [(64usize, 48usize), (33, 17), (8, 8), (40, 21)] {
        let stride = w + 6;
        for (name, plane) in planes(w, h, stride) {
            for &qp in &[0u32, 10, 34, 35, 42, 43, 51, 52, 57, 58, 63] {
                // C-driven reference for every kernel in the chain.
                let avg_mad = cref::vmaf_compute_avg_mad(&plane, w, h, stride);
                let gcoh = cref::vmaf_compute_gradient_coherence(&plane, w, h, stride);
                let mut sharp = (port::combined_amount(qp, avg_mad, gcoh) * 32768.0) as i32;
                sharp = (sharp as f32 * port::noise_gate(&plane, w, h, stride)) as i32;

                let mut rows: Vec<Vec<i16>> = (0..5).map(|_| vec![0i16; w + 4]).collect();
                let mut blur = vec![0u8; w * h];
                for k in 0..4usize {
                    let row = (k as i32 - 2).clamp(0, h as i32 - 1) as usize;
                    cref::vmaf_hpass_row(&plane[row * stride..], w, &mut rows[k]);
                }
                for m in 0..h {
                    let row = (m + 2).min(h - 1);
                    cref::vmaf_hpass_row(&plane[row * stride..], w, &mut rows[4]);
                    let view = [
                        rows[0].as_slice(),
                        rows[1].as_slice(),
                        rows[2].as_slice(),
                        rows[3].as_slice(),
                        rows[4].as_slice(),
                    ];
                    cref::vmaf_vpass_row(view, &mut blur[m * w..], w, 2);
                    rows.rotate_left(1);
                }

                let pixel_count = (w * h) as u32;
                let flat_target = pixel_count.wrapping_mul(85) / 100;
                let flat_count = cref::vmaf_count_detail_le(&plane, &blur, w, h, stride, 12);
                let busy = flat_count < flat_target;
                let clip = port::delta_clip(qp as i32, busy);

                let mut c_plane = plane.clone();
                for y in 0..h {
                    let (src_row, dst_row) = {
                        let s = c_plane[y * stride..y * stride + w].to_vec();
                        (s, y * stride)
                    };
                    let mut out = vec![0u8; w];
                    cref::vmaf_apply_unsharp_row(
                        &src_row,
                        &blur[y * w..],
                        &mut out,
                        w,
                        sharp,
                        clip,
                    );
                    c_plane[dst_row..dst_row + w].copy_from_slice(&out);
                }

                let mut r_plane = plane.clone();
                let got = port::preprocess_frame(&mut r_plane, stride, w, h, qp);
                assert_eq!(
                    got,
                    port::VmafPreprocess {
                        sharpening_amount: sharp,
                        max_delta: clip,
                    },
                    "preprocess scalars {name} {w}x{h} qp={qp}"
                );
                assert_eq!(r_plane, c_plane, "preprocess plane {name} {w}x{h} qp={qp}");
            }
        }
    }
}

/// TIER 4. The four `static` threshold ladders, pinned to the C lines they were
/// transcribed from, including both sides of every boundary.
#[test]
fn tier4_strength_ladders_pinned() {
    // vmaf_get_spatial_amount, pic_analysis_process.c:1642.
    assert_eq!(port::spatial_amount(0), 0.15);
    assert_eq!(port::spatial_amount(1), 0.15);
    assert_eq!(port::spatial_amount(2), 0.22);
    assert_eq!(port::spatial_amount(4), 0.22);
    assert_eq!(port::spatial_amount(5), 0.28);
    assert_eq!(port::spatial_amount(11), 0.28);
    assert_eq!(port::spatial_amount(12), 0.30);
    assert_eq!(port::spatial_amount(u32::MAX), 0.30);

    // vmaf_get_qp_amount, :1654 — 0.5 at qp 0, easing to the 0.3 floor at 35.
    assert_eq!(port::qp_amount(0), 0.5);
    assert_eq!(port::qp_amount(35), 0.3);
    assert_eq!(port::qp_amount(63), 0.3);
    assert_eq!(port::qp_amount(34), 0.5 - (34.0f32 / 35.0) * (0.5 - 0.3));
    assert!(port::qp_amount(1) < port::qp_amount(0));
    assert!(port::qp_amount(34) > port::qp_amount(35));

    // vmaf_get_coherence_factor, :1661.
    assert_eq!(port::coherence_factor(0.0), 0.80);
    assert_eq!(port::coherence_factor(0.399), 0.80);
    assert_eq!(port::coherence_factor(0.40), 0.9);
    assert_eq!(port::coherence_factor(0.599), 0.9);
    assert_eq!(port::coherence_factor(0.60), 1.0);
    assert_eq!(port::coherence_factor(1.0), 1.0);

    // vmaf_get_delta_clip, :1741 — both sides of all three boundaries, and the
    // busy-frame subtraction of 4.
    for &(qp, want) in &[
        (0i32, 8i32),
        (42, 8),
        (43, 9),
        (51, 9),
        (52, 10),
        (57, 10),
        (58, 12),
    ] {
        assert_eq!(port::delta_clip(qp, false), want, "delta_clip qp={qp}");
        assert_eq!(
            port::delta_clip(qp, true),
            want - 4,
            "delta_clip busy qp={qp}"
        );
    }
}
