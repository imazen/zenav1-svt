//! Tier-1 differentials for `svtav1_encoder::port_preanalysis`
//! (`docs/WORKING-ON-THIS.md` §4 tier 1: the real exported C symbol, driven
//! through `svtav1-cref`).
//!
//! Covers `svt_aom_downsample_2d_c`, `calculate_histogram`,
//! `svt_aom_generate_padding`, `pad_input_picture` and
//! `svt_aom_is_input_luma_dominant`.

use svtav1_cref::preanalysis as cref;
use svtav1_encoder::port_preanalysis as port;

/// Deterministic pseudo-random bytes — a plain LCG so both sides see the exact
/// same picture and a failure is reproducible from the seed alone.
fn fill(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u8
        })
        .collect()
}

#[test]
fn downsample_2d_matches_c() {
    let mut cells = 0usize;
    // decim_step 2 is the 1/4 plane; 4 is the direct-4x sixteenth route.
    for &decim_step in &[2u32, 4] {
        for &(w, h) in &[(16u32, 16u32), (64, 64), (32, 16), (96, 88), (12, 20)] {
            for &stride_pad in &[0u32, 7] {
                let stride = w + stride_pad;
                let input = fill(
                    u64::from(w) * 7717
                        + u64::from(h) * 131
                        + u64::from(decim_step) * 3
                        + u64::from(stride_pad),
                    (stride * h) as usize,
                );
                let out_w = (w / decim_step) as usize + 4;
                let out_h = (h / decim_step) as usize + 4;
                let out_stride = out_w as u32;

                let mut c_out = vec![0xABu8; out_w * out_h];
                cref::downsample_2d(&input, stride, w, h, &mut c_out, out_stride, decim_step);

                let mut r_out = vec![0xABu8; out_w * out_h];
                port::downsample_2d(
                    &input,
                    stride as usize,
                    w as usize,
                    h as usize,
                    &mut r_out,
                    out_stride as usize,
                    decim_step as usize,
                );

                assert_eq!(
                    r_out, c_out,
                    "downsample_2d mismatch: {w}x{h} stride {stride} step {decim_step}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 20, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn calculate_histogram_matches_c() {
    let mut cells = 0usize;
    // decim_step 1 is the scene-change-detection route; 4 is the default.
    for &decim_step in &[1u8, 4] {
        for &(w, h) in &[(16u32, 16u32), (40u32, 24u32), (7u32, 5u32)] {
            let stride = w + 3;
            let input = fill(
                u64::from(w) * 991 + u64::from(h) * 17 + u64::from(decim_step),
                (stride * h) as usize,
            );

            // C's caller seeds every bin to 1 and `sum` accumulates, so the
            // pre-state must be identical on both sides.
            let mut c_hist = [1u32; 256];
            let mut c_sum = 5u64;
            cref::calculate_histogram(&input, w, h, stride, decim_step, &mut c_hist, &mut c_sum);

            let mut r_hist = [1u32; 256];
            let mut r_sum = 5u64;
            port::calculate_histogram(
                &input,
                w as usize,
                h as usize,
                stride as usize,
                usize::from(decim_step),
                &mut r_hist,
                &mut r_sum,
            );

            assert_eq!(
                r_hist, c_hist,
                "histogram mismatch {w}x{h} step {decim_step}"
            );
            assert_eq!(
                r_sum, c_sum,
                "histogram sum mismatch {w}x{h} step {decim_step}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 6);
}

#[test]
fn generate_padding_matches_c() {
    let mut cells = 0usize;
    // border 68 is the real luma border; 34 is the chroma one (68 >> 1);
    // the small ones keep the test cheap while exercising odd geometry.
    for &border in &[2u32, 8, 34, 68] {
        for &(w, h) in &[(16u32, 16u32), (64, 64), (96, 88)] {
            let stride = w + 2 * border + 5; // deliberate slack past the border
            let alloc = (stride * (h + 2 * border)) as usize;
            let origin = (border * stride + border) as usize;

            // Fill the WHOLE allocation with noise, not zeros: the vertical
            // copy length is `src_stride`, so it drags along whatever trails
            // the right padding. Zeroed slack would hide a wrong length.
            let base = fill(
                u64::from(w) * 313 + u64::from(h) * 29 + u64::from(border),
                alloc,
            );

            let mut c_buf = base.clone();
            cref::generate_padding(&mut c_buf, origin as u32, stride, w, h, border, border);

            let mut r_buf = base.clone();
            port::generate_padding(
                &mut r_buf,
                origin,
                stride as usize,
                w as usize,
                h as usize,
                border as usize,
                border as usize,
            );

            assert_eq!(
                r_buf, c_buf,
                "generate_padding mismatch {w}x{h} border {border}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 12);
}

#[test]
fn pad_input_picture_matches_c() {
    let mut cells = 0usize;
    // 426x240 -> 432x240 is the case the C comment names.
    for &(w, h, pr, pb) in &[
        (426u32, 240u32, 6u32, 0u32),
        (16, 16, 0, 0),
        (13, 11, 3, 5),
        (64, 60, 0, 4),
    ] {
        let stride = w + pr + 9;
        let alloc = (stride * (h + pb + 2)) as usize;
        let base = fill(u64::from(w) * 77 + u64::from(h) * 13 + u64::from(pr), alloc);

        let mut c_buf = base.clone();
        cref::pad_input_picture(&mut c_buf, stride, w, h, pr, pb);

        let mut r_buf = base.clone();
        port::pad_input_picture(
            &mut r_buf,
            stride as usize,
            w as usize,
            h as usize,
            pr as usize,
            pb as usize,
        );

        assert_eq!(
            r_buf, c_buf,
            "pad_input_picture mismatch {w}x{h} pad {pr}/{pb}"
        );
        cells += 1;
    }
    assert_eq!(cells, 4);
}

#[test]
fn is_input_luma_dominant_matches_c() {
    let mut cells = 0usize;
    let mut saw_true = 0usize;
    let mut saw_false = 0usize;

    // Three regimes so the gate cannot be vacuous: exactly neutral chroma
    // (true), a small dither around 128 (straddles the core/tail/neutral
    // thresholds), and saturated chroma (false).
    for &(w, h) in &[(64u32, 64u32), (96, 88), (426, 240), (2, 2)] {
        let uv_w = (w >> 1) as usize;
        let uv_h = (h >> 1) as usize;
        let u_stride = uv_w + 4;
        let v_stride = uv_w + 7;
        for regime in 0..5u32 {
            let mut u = vec![0u8; u_stride * uv_h.max(1) + u_stride];
            let mut v = vec![0u8; v_stride * uv_h.max(1) + v_stride];
            for y in 0..uv_h {
                for x in 0..uv_w {
                    let (uu, vv) = match regime {
                        0 => (128u8, 128u8),
                        1 => (
                            (128 + ((x + y) % 3) as i32 - 1) as u8,
                            (128 + ((x * 2 + y) % 3) as i32 - 1) as u8,
                        ),
                        2 => (
                            (128 + ((x + y) % 17) as i32 - 8) as u8,
                            (128 + ((x + 3 * y) % 17) as i32 - 8) as u8,
                        ),
                        3 => (200, 60),
                        _ => (
                            (128 + ((x * 7 + y * 5) % 41) as i32 - 20) as u8,
                            (128 + ((x * 3 + y * 11) % 41) as i32 - 20) as u8,
                        ),
                    };
                    u[y * u_stride + x] = uu;
                    v[y * v_stride + x] = vv;
                }
            }

            // EB_YUV420 == 1
            let c = cref::is_input_luma_dominant(1, w, h, &u, u_stride as u32, &v, v_stride as u32);
            let r =
                port::is_input_luma_dominant(1, w as usize, h as usize, &u, u_stride, &v, v_stride);
            assert_eq!(r, c, "luma-dominant mismatch {w}x{h} regime {regime}");
            if c {
                saw_true += 1;
            } else {
                saw_false += 1;
            }
            cells += 1;
        }

        // EB_YUV400 == 0 is rejected outright on both sides.
        let u = vec![128u8; u_stride * uv_h.max(1)];
        let v = vec![128u8; v_stride * uv_h.max(1)];
        let c = cref::is_input_luma_dominant(0, w, h, &u, u_stride as u32, &v, v_stride as u32);
        let r = port::is_input_luma_dominant(0, w as usize, h as usize, &u, u_stride, &v, v_stride);
        assert_eq!(r, c);
        assert!(!c, "EB_YUV400 must be rejected");
        cells += 1;
    }

    assert_eq!(cells, 24);
    // Anti-vacuity: a predicate that always returned the same value would
    // pass a one-sided gate. Both outcomes must actually occur.
    assert!(saw_true > 0, "no cell was luma-dominant");
    assert!(saw_false > 0, "no cell was rejected");
}

#[test]
fn downsample_filtering_input_picture_matches_c() {
    use svtav1_cref::preanalysis::PlaneGeom;
    let mut cells = 0usize;
    // The live encoder configuration is all three enables set
    // (enc_mode_config.c:1987-1999). The other rows exercise the arms that
    // are reachable only through the tf_* flags or with level 1 off — the
    // direct-4x sixteenth route, which produces DIFFERENT pixels.
    let rows: [[bool; 6]; 5] = [
        [true, false, true, false, true, false],    // the live route
        [true, false, true, false, false, false],   // direct 4x sixteenth
        [false, true, false, true, false, true],    // tf_* mirror of the live route
        [true, false, false, false, true, false],   // quarter only
        [false, false, false, false, false, false], // fully gated off
    ];
    for &(w, h) in &[(64u32, 64u32), (128, 96)] {
        let border = 68u32;
        let stride = w + 2 * border;
        let origin = border * stride + border;
        let alloc = (stride * (h + 2 * border)) as usize;

        let qw = w / 2;
        let qh = h / 2;
        let qb = 34u32;
        let qstride = qw + 2 * qb;
        let qorigin = qb * qstride + qb;
        let qalloc = (qstride * (qh + 2 * qb)) as usize;

        let sw = w / 4;
        let sh = h / 4;
        let sb = 17u32;
        let sstride = sw + 2 * sb;
        let sorigin = sb * sstride + sb;
        let salloc = (sstride * (sh + 2 * sb)) as usize;

        let input = fill(u64::from(w) * 4241 + u64::from(h), alloc);
        let q_base = fill(999 + u64::from(w), qalloc);
        let s_base = fill(31337 + u64::from(h), salloc);

        let in_geom = PlaneGeom {
            origin,
            stride,
            width: w,
            height: h,
            border,
        };
        let q_geom = PlaneGeom {
            origin: qorigin,
            stride: qstride,
            width: qw,
            height: qh,
            border: qb,
        };
        let s_geom = PlaneGeom {
            origin: sorigin,
            stride: sstride,
            width: sw,
            height: sh,
            border: sb,
        };

        for row in rows {
            let mut cq = q_base.clone();
            let mut cs = s_base.clone();
            cref::downsample_filtering_input_picture(
                row, &input, in_geom, &mut cq, q_geom, &mut cs, s_geom,
            );

            let mut rq = q_base.clone();
            let mut rs = s_base.clone();
            {
                let mut input_owned = input.clone();
                let ip = port::Plane {
                    buf: &mut input_owned,
                    origin: origin as usize,
                    stride: stride as usize,
                    width: w as usize,
                    height: h as usize,
                    border: border as usize,
                };
                let mut qp = port::Plane {
                    buf: &mut rq,
                    origin: qorigin as usize,
                    stride: qstride as usize,
                    width: qw as usize,
                    height: qh as usize,
                    border: qb as usize,
                };
                let mut sp = port::Plane {
                    buf: &mut rs,
                    origin: sorigin as usize,
                    stride: sstride as usize,
                    width: sw as usize,
                    height: sh as usize,
                    border: sb as usize,
                };
                let flags = port::HmeEnables {
                    enable_hme: row[0],
                    tf_enable_hme: row[1],
                    enable_hme_level0: row[2],
                    tf_enable_hme_level0: row[3],
                    enable_hme_level1: row[4],
                    tf_enable_hme_level1: row[5],
                };
                port::downsample_filtering_input_picture(&flags, &ip, &mut qp, &mut sp);
            }

            assert_eq!(rq, cq, "quarter plane mismatch {w}x{h} enables {row:?}");
            assert_eq!(rs, cs, "sixteenth plane mismatch {w}x{h} enables {row:?}");
            cells += 1;
        }

        // Anti-vacuity for the route split: the two sixteenth routes must
        // actually produce DIFFERENT bytes, or this test would pass with the
        // wrong arm ported.
        let mut a_q = q_base.clone();
        let mut a_s = s_base.clone();
        cref::downsample_filtering_input_picture(
            rows[0], &input, in_geom, &mut a_q, q_geom, &mut a_s, s_geom,
        );
        let mut b_q = q_base.clone();
        let mut b_s = s_base.clone();
        cref::downsample_filtering_input_picture(
            rows[1], &input, in_geom, &mut b_q, q_geom, &mut b_s, s_geom,
        );
        assert_ne!(
            a_s, b_s,
            "2x-then-2x and direct-4x sixteenth routes must differ (they did not at {w}x{h})"
        );
    }
    assert_eq!(cells, 10);
}

#[test]
fn pad_input_pictures_matches_c() {
    use svtav1_cref::preanalysis::PlaneGeom;
    let mut cells = 0usize;
    // (luma width, luma height, pad_right, pad_bottom). 426x240 -> 432x240 is
    // the case the C comment names; the 8-aligned rows exercise pad == 0.
    for &(w, h, pr, pb) in &[(432u32, 240u32, 6u32, 0u32), (64, 64, 0, 0), (72, 40, 4, 8)] {
        for &border in &[8u32, 68] {
            for &min_blk_only in &[true, false] {
                let stride = w + 2 * border + 3;
                let origin = border * stride + border;
                let alloc = (stride * (h + 2 * border)) as usize;

                let cb = border / 2;
                let cstride = w / 2 + 2 * cb + 5;
                let corigin = cb * cstride + cb;
                let calloc = (cstride * (h / 2 + 2 * cb)) as usize;

                let y_base = fill(u64::from(w) * 61 + u64::from(border) + u64::from(pb), alloc);
                let u_base = fill(u64::from(h) * 97 + u64::from(border), calloc);
                let v_base = fill(u64::from(h) * 197 + u64::from(border), calloc);

                let y_geom = PlaneGeom {
                    origin,
                    stride,
                    width: w,
                    height: h,
                    border,
                };
                let c_geom = PlaneGeom {
                    origin: corigin,
                    stride: cstride,
                    width: w / 2,
                    height: h / 2,
                    border: cb,
                };

                let (mut cy, mut cu, mut cv) = (y_base.clone(), u_base.clone(), v_base.clone());
                // EB_EIGHT_BIT == 8, EB_YUV420 == 1
                cref::pad_input_pictures(
                    min_blk_only,
                    8,
                    1,
                    1,
                    1,
                    pr,
                    pb,
                    &mut cy,
                    y_geom,
                    &mut cu,
                    c_geom,
                    &mut cv,
                    c_geom,
                );

                let (mut ry, mut ru, mut rv) = (y_base.clone(), u_base.clone(), v_base.clone());
                {
                    let mut yp = port::Plane {
                        buf: &mut ry,
                        origin: origin as usize,
                        stride: stride as usize,
                        width: w as usize,
                        height: h as usize,
                        border: border as usize,
                    };
                    let mut up = port::Plane {
                        buf: &mut ru,
                        origin: corigin as usize,
                        stride: cstride as usize,
                        width: (w / 2) as usize,
                        height: (h / 2) as usize,
                        border: cb as usize,
                    };
                    let mut vp = port::Plane {
                        buf: &mut rv,
                        origin: corigin as usize,
                        stride: cstride as usize,
                        width: (w / 2) as usize,
                        height: (h / 2) as usize,
                        border: cb as usize,
                    };
                    if min_blk_only {
                        port::pad_picture_to_multiple_of_min_blk_size_dimensions(
                            1,
                            pr as usize,
                            pb as usize,
                            &mut yp,
                            Some(&mut up),
                            Some(&mut vp),
                        );
                    } else {
                        port::pad_input_pictures(
                            1,
                            1,
                            pr as usize,
                            pb as usize,
                            1,
                            &mut yp,
                            Some(&mut up),
                            Some(&mut vp),
                        );
                    }
                }

                assert_eq!(
                    ry, cy,
                    "Y mismatch {w}x{h} pad {pr}/{pb} border {border} minblk {min_blk_only}"
                );
                assert_eq!(
                    ru, cu,
                    "U mismatch {w}x{h} pad {pr}/{pb} border {border} minblk {min_blk_only}"
                );
                assert_eq!(
                    rv, cv,
                    "V mismatch {w}x{h} pad {pr}/{pb} border {border} minblk {min_blk_only}"
                );
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 12);
}

#[test]
fn gathering_picture_statistics_matches_c() {
    let mut cells = 0usize;
    let mut hist_ran = 0usize;
    // HIGHER_THAN_CLASS_1_REGION_SPLIT_PER_* gives 4x4 for >= 64px sources;
    // 1x1 is what a sub-64 axis gets (enc_handle.c:4392).
    for &(rw, rh) in &[(4u32, 4u32), (1, 1), (4, 1)] {
        // The 1/16 plane's own dimensions. 33x21 makes the last region on each
        // axis absorb a non-zero remainder, which is where region_*_offset
        // matters.
        for &(w, h) in &[(16u32, 16u32), (33, 21), (8, 8)] {
            for &scd in &[false, true] {
                for &calc_hist in &[true, false] {
                    let border = 4u32;
                    let stride = w + 2 * border;
                    let origin = border * stride + border;
                    let alloc = (stride * (h + 2 * border)) as usize;
                    let plane = fill(
                        u64::from(w) * 811 + u64::from(h) * 13 + u64::from(rw) + u64::from(scd),
                        alloc,
                    );

                    let c = cref::gathering_picture_statistics(
                        calc_hist, false, rw, rh, scd, &plane, origin, stride, w, h,
                    )
                    .expect("calculate_variance == 0 is always drivable");

                    let mut stats = port::PictureStatistics::default();
                    port::gathering_picture_statistics(
                        calc_hist,
                        false,
                        rw as usize,
                        rh as usize,
                        scd,
                        &plane[origin as usize..],
                        stride as usize,
                        w as usize,
                        h as usize,
                        None,
                        &mut stats,
                    );

                    assert_eq!(
                        stats.avg_luma, c.avg_luma,
                        "avg_luma mismatch {w}x{h} regions {rw}x{rh} scd {scd} calc_hist {calc_hist}"
                    );
                    assert_eq!(stats.pic_avg_variance, c.pic_avg_variance);

                    if calc_hist {
                        hist_ran += 1;
                        // Only the regions C actually visited are written; the
                        // rest of the PCS array is untouched on both sides, so
                        // compare exactly the visited sub-grid.
                        for wi in 0..rw as usize {
                            for hi in 0..rh as usize {
                                let base = (wi * 4 + hi) * 256;
                                assert_eq!(
                                    &stats.picture_histogram[wi][hi][..],
                                    &c.histogram[base..base + 256],
                                    "histogram mismatch region ({wi},{hi}) {w}x{h} regions {rw}x{rh} scd {scd}"
                                );
                                assert_eq!(
                                    stats.average_intensity_per_region[wi][hi],
                                    c.average_intensity_per_region[wi * 4 + hi],
                                    "avg intensity mismatch region ({wi},{hi}) {w}x{h}"
                                );
                            }
                        }
                    } else {
                        // The gate is off: avg_luma must be the INVALID_LUMA
                        // sentinel on both sides, and nothing else is written.
                        assert_eq!(c.avg_luma, 256);
                    }
                    cells += 1;
                }
            }
        }
    }
    assert_eq!(cells, 36);
    assert_eq!(hist_ran, 18, "the calc_hist arm must actually run");
}
