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

// ---------------------------------------------------------------------------
// The remaining padding entry points
// ---------------------------------------------------------------------------

fn fill16(seed: u64, n: usize, mask: u16) -> Vec<u16> {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as u16) & mask
        })
        .collect()
}

/// `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions_16bit` — the
/// 10-bit path, which the 8-bit sibling's differential cannot reach.
#[test]
fn pad_min_blk_16bit_matches_c() {
    let mut cells = 0usize;
    let mut moved = 0usize;
    // 420 only (`EB_YUV420 == 1`) plus 422 and 444, which C's
    // `verify_settings` rejects but this function does not — the shift
    // arithmetic differs per format and porting only 420 would be untested at
    // the two the code still spells.
    for color_format in [1u32, 2, 3] {
        for (w, h) in [(64usize, 48usize), (66, 50), (70, 34), (32, 32)] {
            for (pad_right, pad_bottom) in [(0usize, 0usize), (2, 0), (0, 6), (6, 2), (7, 7)] {
                let (y_stride, c_stride) = (w + 12, w / 2 + 12);
                let y0 = fill16(0x51, y_stride * (h + 12), 0x3ff);
                let u0 = fill16(0x52, c_stride * (h + 12), 0x3ff);
                let v0 = fill16(0x53, c_stride * (h + 12), 0x3ff);

                let (mut cy, mut cu, mut cv) = (y0.clone(), u0.clone(), v0.clone());
                cref::pad_min_blk_16bit(
                    color_format,
                    pad_right,
                    pad_bottom,
                    w,
                    h,
                    (&mut cy, y_stride),
                    (&mut cu, c_stride),
                    (&mut cv, c_stride),
                );

                let (mut ry, mut ru, mut rv) = (y0.clone(), u0.clone(), v0.clone());
                port::pad_picture_to_multiple_of_min_blk_size_dimensions_16bit(
                    color_format,
                    pad_right,
                    pad_bottom,
                    w,
                    h,
                    (&mut ry, y_stride),
                    Some((&mut ru, c_stride)),
                    Some((&mut rv, c_stride)),
                );

                assert_eq!(
                    ry, cy,
                    "16bit pad Y cf={color_format} {w}x{h} {pad_right}/{pad_bottom}"
                );
                assert_eq!(
                    ru, cu,
                    "16bit pad U cf={color_format} {w}x{h} {pad_right}/{pad_bottom}"
                );
                assert_eq!(
                    rv, cv,
                    "16bit pad V cf={color_format} {w}x{h} {pad_right}/{pad_bottom}"
                );
                if cy != y0 {
                    moved += 1;
                }
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 3 * 4 * 5);
    assert!(moved > 0, "the 16-bit pad never changed a plane");
}

/// `svt_aom_pad_picture_to_multiple_of_sb_dimensions`.
#[test]
fn pad_to_sb_matches_c() {
    let mut cells = 0usize;
    let mut moved = 0usize;
    for (w, h) in [(16usize, 16usize), (48, 32), (65, 33), (7, 5)] {
        for border in [0usize, 1, 4, 16, 68] {
            let stride = w + 2 * border + 6;
            let n = stride * (h + 2 * border + 6);
            let origin = border * stride + border;
            let base = fill(0x77, n);

            let mut c = base.clone();
            cref::pad_to_sb(&mut c, origin, stride, w, h, border);

            let mut r = base.clone();
            let mut plane = port::Plane {
                buf: &mut r,
                origin,
                stride,
                width: w,
                height: h,
                border,
            };
            port::pad_picture_to_multiple_of_sb_dimensions(&mut plane);

            assert_eq!(r, c, "pad_to_sb {w}x{h} border {border}");
            if border > 0 && c != base {
                moved += 1;
            }
            cells += 1;
        }
    }
    assert_eq!(cells, 4 * 5);
    assert!(moved > 0, "pad_to_sb never wrote a border");
}

/// `svt_aom_down_sample_chroma`. Dead in the shipping encoder (see the port's
/// reachability note) but exported, so it is gated rather than trusted.
#[test]
fn down_sample_chroma_matches_c() {
    let mut cells = 0usize;
    let mut nonidentity = 0usize;
    // Input 444 (3) and 422 (2) — the only formats whose call site is
    // reachable in C's own source — down to 420 (1) and to themselves.
    for in_cf in [2u32, 3] {
        for out_cf in [1u32, 2, 3] {
            for (ow, oh) in [(64usize, 48usize), (32, 16), (8, 8)] {
                // A 444 or 422 input is point-sampled at `ii << 1` / `jj << 1`,
                // so the SOURCE must span twice the output extent in whichever
                // direction is not already subsampled. Sizing it to the output
                // extent reads past the end (found by an index-out-of-bounds
                // panic in the port, which is what the bounds are for).
                let (si_u, si_v, so_u, so_v) = (2 * ow + 9, 2 * ow + 11, ow + 7, ow + 5);
                let n_in = si_u.max(si_v) * (2 * oh + 4);
                let mut u_in = fill(0x81, n_in);
                let mut v_in = fill(0x82, n_in);
                let n_out = so_u.max(so_v) * (oh + 4);
                let base_u = fill(0x83, n_out);
                let base_v = fill(0x84, n_out);

                let (mut cu, mut cv) = (base_u.clone(), base_v.clone());
                cref::down_sample_chroma(
                    in_cf, out_cf, ow, oh, &mut u_in, si_u, &mut v_in, si_v, &mut cu, so_u,
                    &mut cv, so_v,
                );

                let (mut ru, mut rv) = (base_u.clone(), base_v.clone());
                port::down_sample_chroma(
                    in_cf, out_cf, ow, oh, &u_in, &v_in, si_u, si_v, &mut ru, &mut rv, so_u, so_v,
                );

                assert_eq!(ru, cu, "down_sample U {in_cf}->{out_cf} {ow}x{oh}");
                assert_eq!(rv, cv, "down_sample V {in_cf}->{out_cf} {ow}x{oh}");
                if cu != base_u {
                    nonidentity += 1;
                }
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 2 * 3 * 3);
    assert!(nonidentity > 0, "down_sample_chroma never wrote anything");
}

/// `pad_2b_compressed_input_picture` — `static` in C, reached at TIER 1
/// through its only caller.
///
/// This is the test that gates the port's COLLAPSE of C's eight
/// near-identical `pad_right == N` bodies into one rule plus the `== 4`
/// special case. All eight values are swept, on every row count, so a
/// mis-derived mask or shift shows up as a byte difference rather than as a
/// plausible-looking plane.
#[test]
fn pad_2b_compressed_matches_c() {
    let mut cells = 0usize;
    let mut arms_seen = [false; 8];
    // Every cell keeps `h - pad_bottom >= 1` and `w - pad_right >= 4`. C
    // computes `(original_src_height - 1) * src_stride` in `uint32_t`, so a
    // height of zero wraps and the bottom-pad memcpy SIGBUSes; the encoder
    // never produces that, and the port refuses it (checked at the end).
    for (w, h) in [(64usize, 8usize), (68, 5), (32, 16), (12, 6)] {
        for pad_right in 0usize..=7 {
            for pad_bottom in [0usize, 1, 3] {
                // The main planes are the EIGHT-BIT high bytes of SVT's
                // unpacked 10-bit layout; the compressed stride is `/ 4` of
                // theirs, which C derives itself.
                let y_stride = w + 16;
                let c_stride = w / 2 + 16;
                let inc_stride_y = y_stride / 4;
                let inc_stride_c = c_stride / 4;
                let y0 = fill(0x91, y_stride * (h + 8));
                let u0 = fill(0x92, c_stride * (h + 8));
                let v0 = fill(0x93, c_stride * (h + 8));
                let yi0 = fill(0x94, inc_stride_y * (h + 8));
                let ui0 = fill(0x95, inc_stride_c * (h + 8));
                let vi0 = fill(0x96, inc_stride_c * (h + 8));

                let (mut cy, mut cu, mut cv) = (y0.clone(), u0.clone(), v0.clone());
                let (mut cyi, mut cui, mut cvi) = (yi0.clone(), ui0.clone(), vi0.clone());
                cref::pad_2b_compressed(
                    1,
                    pad_right,
                    pad_bottom,
                    w,
                    h,
                    (&mut cy, y_stride),
                    (&mut cu, c_stride),
                    (&mut cv, c_stride),
                    &mut cyi,
                    &mut cui,
                    &mut cvi,
                );

                let mut ryi = yi0.clone();
                let ok = port::pad_2b_compressed_input_picture(
                    &mut ryi,
                    inc_stride_y,
                    w - pad_right,
                    h - pad_bottom,
                    pad_right,
                    pad_bottom,
                );
                assert!(ok, "pad_right {pad_right} was rejected");
                assert_eq!(
                    ryi, cyi,
                    "2b compressed luma pad {w}x{h} right={pad_right} bottom={pad_bottom}"
                );
                if cyi != yi0 {
                    arms_seen[pad_right] = true;
                }
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 4 * 8 * 3);
    // Every `pad_right` arm from 1 to 7 must have CHANGED the plane; if one
    // did not, the collapse of C's eight bodies was compared against nothing.
    for (n, seen) in arms_seen.iter().enumerate().skip(1) {
        assert!(*seen, "pad_right == {n} never modified the plane");
    }
    // Out of range is C's `assert_err`; the port refuses instead.
    let mut buf = vec![0u8; 64];
    assert!(!port::pad_2b_compressed_input_picture(
        &mut buf, 16, 16, 2, 8, 0
    ));
    // A zero height wraps `original_src_height - 1` in C's `uint32_t` and
    // SIGBUSes; the port refuses. NOT driven through the oracle, on purpose —
    // the differential would crash the test process, which is how this was
    // found in the first place.
    assert!(!port::pad_2b_compressed_input_picture(
        &mut buf, 16, 16, 0, 4, 1
    ));
}
