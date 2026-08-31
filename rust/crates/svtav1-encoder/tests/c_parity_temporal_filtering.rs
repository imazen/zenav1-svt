//! Tier-1 differentials for `svtav1_encoder::port_temporal_filtering`
//! (`docs/WORKING-ON-THIS.md` §4 tier 1: the real exported C symbol, driven
//! through `svtav1-cref`).

use svtav1_cref::temporal_filtering as cref;
use svtav1_encoder::port_temporal_filtering as port;

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

#[test]
fn noise_log1p_fp16_matches_c() {
    let mut cells = 0usize;
    let mut arm_low = 0usize;
    let mut arm_table = 0usize;
    let mut arm_linear = 0usize;
    // Sweep every table index boundary plus the two arm boundaries, and a
    // dense walk of the interpolation residue.
    let mut probes: Vec<i32> = Vec::new();
    for id in 0..=224i32 {
        for rest in [0i32, 1, 1023, 2047] {
            probes.push((id << 11) + rest - 65536);
        }
    }
    probes.extend([
        i32::MIN,
        -70000,
        -65537,
        -65536,
        -65535,
        -1,
        0,
        1,
        458751 - 65536,
        458752 - 65536,
        458753 - 65536,
        1 << 20,
        (1 << 22) - 1,
    ]);
    for &n in &probes {
        let c = cref::noise_log1p_fp16(n);
        let r = port::noise_log1p_fp16(n);
        assert_eq!(r, c, "noise_log1p_fp16 mismatch at {n}");
        let base = 65536i32.wrapping_add(n);
        if base <= 0 {
            arm_low += 1;
        } else if base < 458752 {
            arm_table += 1;
        } else {
            arm_linear += 1;
        }
        cells += 1;
    }
    assert!(cells > 900, "only {cells} probes");
    // Anti-vacuity: every one of the three arms must be exercised.
    assert!(
        arm_low > 0 && arm_table > 0 && arm_linear > 0,
        "arms {arm_low}/{arm_table}/{arm_linear}"
    );
}

/// The port implements `OD_DIVU` as plain integer division. C routes
/// denominators below 1024 through a reciprocal-multiply table
/// (`svt_aom_od_divu_small_consts`). This gates the equivalence claim against
/// the REAL macro instead of assuming it.
#[test]
fn od_divu_matches_c() {
    let mut cells = 0usize;
    let mut small = 0usize;
    let mut large = 0usize;
    // Denominators: every value 1..=64 (the dense low range TF actually hits),
    // both sides of the 1024 boundary, and a spread above it.
    let mut denoms: Vec<u32> = (1..=64).collect();
    denoms.extend([
        100,
        255,
        500,
        999,
        1000,
        1022,
        1023,
        1024,
        1025,
        2000,
        4000,
        65535,
        1 << 20,
    ]);
    // Numerators spanning the u32 range, including the exact multiples and
    // off-by-ones where a reciprocal approximation would break first.
    for &d in &denoms {
        let mut nums: Vec<u32> = vec![0, 1, d.saturating_sub(1), d, d + 1, u32::MAX];
        for k in [1u32, 7, 63, 1000, 65535, 1 << 20, (1 << 28) - 1] {
            nums.push(k.saturating_mul(d).wrapping_sub(1));
            nums.push(k.wrapping_mul(d));
            nums.push(k.wrapping_mul(d).wrapping_add(1));
        }
        for &x in &nums {
            let c = cref::od_divu(x, d);
            let r = port::od_divu(x, d);
            assert_eq!(r, c, "OD_DIVU mismatch: {x} / {d} (C {c}, port {r})");
            if d < 1024 {
                small += 1;
            } else {
                large += 1;
            }
            cells += 1;
        }
    }
    assert!(cells > 1500, "only {cells} cells");
    assert!(
        small > 0 && large > 0,
        "both OD_DIVU arms must run: {small}/{large}"
    );
}

#[test]
fn use_64x64_pred_matches_c() {
    let mut cells = 0usize;
    let mut saw_one = 0usize;
    let mut saw_zero = 0usize;
    for &s64 in &[0u32, 1, 100, 1000, 4000, 100_000, u32::MAX] {
        for &s32 in &[
            [0u32; 4],
            [1, 1, 1, 1],
            [250, 250, 250, 250],
            [1000, 0, 0, 0],
            [u32::MAX / 4; 4],
        ] {
            for &th in &[0u8, 1, 5, 25, 100, 255] {
                let mut args = cref::TfCtxArgs {
                    p_best_sad_64x64: s64,
                    p_best_sad_32x32: s32,
                    tf_use_pred_64x64_only_th: th,
                    ..Default::default()
                };
                args.tf_chroma = 1;
                let c = cref::use_64x64_pred(&args);
                let r = port::tf_use_64x64_pred(s64, &s32, i64::from(th));
                assert_eq!(
                    r, c,
                    "tf_use_64x64_pred mismatch s64 {s64} s32 {s32:?} th {th}"
                );
                if c == 1 {
                    saw_one += 1;
                } else {
                    saw_zero += 1;
                }
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 7 * 5 * 6);
    assert!(
        saw_one > 0 && saw_zero > 0,
        "both outcomes must occur: {saw_one}/{saw_zero}"
    );
}

#[test]
fn apply_filtering_central_matches_c() {
    let mut cells = 0usize;
    for &(bw, bh) in &[(64usize, 64usize), (32, 32), (16, 8)] {
        for &tf_chroma in &[true, false] {
            let (ss_x, ss_y) = (1u32, 1u32);
            let stride_y = bw + 11;
            // The chroma stride is `stride_y >> ss_x`, which is what C uses.
            let src_y = fill(bw as u64 * 31 + bh as u64, stride_y * (bh + 2));
            let src_u = fill(77 + bw as u64, stride_y * (bh + 2));
            let src_v = fill(177 + bh as u64, stride_y * (bh + 2));

            let n = bw * bh + 64;
            let mut c_acc = [vec![0xEEu32; n], vec![0xEEu32; n], vec![0xEEu32; n]];
            let mut c_cnt = [vec![0x77u16; n], vec![0x77u16; n], vec![0x77u16; n]];
            cref::apply_filtering_central(
                tf_chroma,
                [&src_y, &src_u, &src_v],
                stride_y as u32,
                &mut c_acc,
                &mut c_cnt,
                bw as u16,
                bh as u16,
                ss_x,
                ss_y,
            );

            let mut r_acc = [vec![0xEEu32; n], vec![0xEEu32; n], vec![0xEEu32; n]];
            let mut r_cnt = [vec![0x77u16; n], vec![0x77u16; n], vec![0x77u16; n]];
            port::apply_filtering_central(
                tf_chroma, &src_y, &src_u, &src_v, stride_y, &mut r_acc, &mut r_cnt, bw, bh, ss_x,
                ss_y,
            );

            assert_eq!(
                r_acc, c_acc,
                "central accum mismatch {bw}x{bh} chroma {tf_chroma}"
            );
            assert_eq!(
                r_cnt, c_cnt,
                "central count mismatch {bw}x{bh} chroma {tf_chroma}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 6);
}

#[test]
fn apply_filtering_central_highbd_matches_c() {
    let mut cells = 0usize;
    for &(bw, bh) in &[(64usize, 64usize), (32, 16)] {
        for &tf_chroma in &[true, false] {
            let (ss_x, ss_y) = (1u32, 1u32);
            let stride_y = bw + 7;
            let src_y = fill16(bw as u64 * 13 + 1, stride_y * (bh + 2), 0x3FF);
            let src_u = fill16(bw as u64 * 13 + 2, stride_y * (bh + 2), 0x3FF);
            let src_v = fill16(bw as u64 * 13 + 3, stride_y * (bh + 2), 0x3FF);

            let n = bw * bh + 64;
            let mut c_acc = [vec![0xEEu32; n], vec![0xEEu32; n], vec![0xEEu32; n]];
            let mut c_cnt = [vec![0x77u16; n], vec![0x77u16; n], vec![0x77u16; n]];
            cref::apply_filtering_central_highbd(
                tf_chroma,
                [&src_y, &src_u, &src_v],
                stride_y as u32,
                &mut c_acc,
                &mut c_cnt,
                bw as u16,
                bh as u16,
                ss_x,
                ss_y,
            );

            let mut r_acc = [vec![0xEEu32; n], vec![0xEEu32; n], vec![0xEEu32; n]];
            let mut r_cnt = [vec![0x77u16; n], vec![0x77u16; n], vec![0x77u16; n]];
            port::apply_filtering_central_highbd(
                tf_chroma, &src_y, &src_u, &src_v, stride_y, &mut r_acc, &mut r_cnt, bw, bh, ss_x,
                ss_y,
            );

            assert_eq!(r_acc, c_acc, "central hbd accum mismatch {bw}x{bh}");
            assert_eq!(r_cnt, c_cnt, "central hbd count mismatch {bw}x{bh}");
            cells += 1;
        }
    }
    assert_eq!(cells, 4);
}

/// Build a plausible accum/count pair: `count` in the range the filter
/// produces (never 0 — C divides by it) and `accum` consistent with it.
fn accum_count(seed: u64, n: usize) -> (Vec<u32>, Vec<u16>) {
    let noise = fill(seed, n * 2);
    let mut accum = vec![0u32; n];
    let mut count = vec![0u16; n];
    for i in 0..n {
        // 1000..=8000, the shape TF_PLANEWISE_FILTER_WEIGHT_SCALE plus a few
        // reference weights gives.
        let c = 1000u16 + (u16::from(noise[i]) * 27);
        count[i] = c;
        accum[i] = u32::from(c) * u32::from(noise[n + i]);
    }
    (accum, count)
}

#[test]
fn get_final_filtered_pixels_matches_c() {
    let mut cells = 0usize;
    for &tf_chroma in &[true, false] {
        for &(bwc, bhc) in &[(32usize, 32usize), (16, 16)] {
            let stride_y = 64usize + 13;
            let stride_c = 32usize + 5;
            let y_off = 3 * stride_y + 4;
            let c_off = 2 * stride_c + 1;
            let y_len = y_off + 64 * stride_y + 64;
            let c_len = c_off + bhc * stride_c + bwc;

            let (ay, cy) = accum_count(11, 64 * 64);
            let (au, cu) = accum_count(22, bwc * bhc);
            let (av, cv) = accum_count(33, bwc * bhc);
            let accum = [ay, au, av];
            let count = [cy, cu, cv];

            let base_y = fill(5, y_len);
            let base_c = fill(6, c_len);

            let mut c_src = [base_y.clone(), base_c.clone(), base_c.clone()];
            cref::get_final_filtered_pixels(
                tf_chroma,
                &mut c_src,
                &accum,
                &count,
                [stride_y as u32, stride_c as u32, stride_c as u32],
                y_off as i32,
                c_off as i32,
                bwc as u16,
                bhc as u16,
            );

            let mut r_src = [base_y.clone(), base_c.clone(), base_c.clone()];
            port::get_final_filtered_pixels(
                tf_chroma,
                &mut r_src,
                &accum,
                &count,
                &[stride_y, stride_c, stride_c],
                y_off,
                c_off,
                bwc,
                bhc,
            );

            assert_eq!(
                r_src, c_src,
                "final pixels mismatch chroma {tf_chroma} {bwc}x{bhc}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 4);
}

#[test]
fn get_final_filtered_pixels_highbd_matches_c() {
    let mut cells = 0usize;
    for &tf_chroma in &[true, false] {
        let (bwc, bhc) = (32usize, 32usize);
        let stride_y = 64usize + 9;
        let stride_c = 32usize + 3;
        let y_off = 2 * stride_y + 2;
        let c_off = stride_c + 1;
        let y_len = y_off + 64 * stride_y + 64;
        let c_len = c_off + bhc * stride_c + bwc;

        let (ay, cy) = accum_count(41, 64 * 64);
        let (au, cu) = accum_count(42, bwc * bhc);
        let (av, cv) = accum_count(43, bwc * bhc);
        let accum = [ay, au, av];
        let count = [cy, cu, cv];

        let base_y = fill16(51, y_len, 0x3FF);
        let base_c = fill16(52, c_len, 0x3FF);

        let mut c_h = [base_y.clone(), base_c.clone(), base_c.clone()];
        cref::get_final_filtered_pixels_highbd(
            tf_chroma,
            &mut c_h,
            &accum,
            &count,
            [stride_y as u32, stride_c as u32, stride_c as u32],
            y_off as i32,
            c_off as i32,
            bwc as u16,
            bhc as u16,
        );

        let mut r_h = [base_y.clone(), base_c.clone(), base_c.clone()];
        port::get_final_filtered_pixels_highbd(
            tf_chroma,
            &mut r_h,
            &accum,
            &count,
            &[stride_y, stride_c, stride_c],
            y_off,
            c_off,
            bwc,
            bhc,
        );

        assert_eq!(r_h, c_h, "final hbd pixels mismatch chroma {tf_chroma}");
        cells += 1;
    }
    assert_eq!(cells, 2);
}

/// Contexts covering both the split and the no-split arm of the medium
/// kernel, with MVs at zero, small and large distance.
fn kernel_contexts() -> Vec<(cref::TfCtxArgs, port::TfKernelCtx)> {
    let mut out = Vec::new();
    for &split in &[0i32, 1] {
        for &(mv, err) in &[(0i16, 0u64), (3, 5_000), (40, 900_000), (-17, 40_000_000)] {
            for &idx in &[0usize, 3] {
                for &dec in &[1u32 << 16, 1 << 20, 12345, 1] {
                    let mut c = cref::TfCtxArgs {
                        tf_block_col: (idx % 2) as i32,
                        tf_block_row: (idx / 2) as i32,
                        tf_mv_dist_th: 10,
                        tf_chroma: 1,
                        tf_decay_factor_fp16: [dec, dec / 2 + 1, dec / 3 + 1],
                        ..Default::default()
                    };
                    c.tf_32x32_block_split_flag[idx] = split;
                    for i in 0..16 {
                        c.tf_16x16_mv_x[i] = mv + i as i16;
                        c.tf_16x16_mv_y[i] = -mv + (i as i16) / 2;
                        c.tf_16x16_block_error[i] = err + (i as u64) * 97;
                    }
                    for i in 0..4 {
                        c.tf_32x32_mv_x[i] = mv - i as i16;
                        c.tf_32x32_mv_y[i] = mv / 2 + i as i16;
                        c.tf_32x32_block_error[i] = err * 4 + (i as u64) * 313;
                    }
                    let p = port::TfKernelCtx {
                        tf_block_col: c.tf_block_col,
                        tf_block_row: c.tf_block_row,
                        tf_mv_dist_th: c.tf_mv_dist_th,
                        tf_chroma: c.tf_chroma != 0,
                        tf_32x32_block_split_flag: [
                            c.tf_32x32_block_split_flag[0] as u8,
                            c.tf_32x32_block_split_flag[1] as u8,
                            c.tf_32x32_block_split_flag[2] as u8,
                            c.tf_32x32_block_split_flag[3] as u8,
                        ],
                        tf_16x16_mv_x: c.tf_16x16_mv_x,
                        tf_16x16_mv_y: c.tf_16x16_mv_y,
                        tf_16x16_block_error: c.tf_16x16_block_error,
                        tf_32x32_mv_x: c.tf_32x32_mv_x,
                        tf_32x32_mv_y: c.tf_32x32_mv_y,
                        tf_32x32_block_error: c.tf_32x32_block_error,
                        tf_decay_factor_fp16: c.tf_decay_factor_fp16,
                    };
                    out.push((c, p));
                }
            }
        }
    }
    out
}

#[test]
fn apply_planewise_medium_matches_c() {
    let mut cells = 0usize;
    let (bw, bh) = (64usize, 64usize);
    let (ss_x, ss_y) = (1i32, 1i32);
    let y_stride = bw + 8;
    let uv_stride = (bw >> 1) + 4;

    let y_src = fill(101, y_stride * (bh + 2));
    let y_pre = fill(102, y_stride * (bh + 2));
    let u_src = fill(103, uv_stride * (bh / 2 + 2));
    let v_src = fill(104, uv_stride * (bh / 2 + 2));
    let u_pre = fill(105, uv_stride * (bh / 2 + 2));
    let v_pre = fill(106, uv_stride * (bh / 2 + 2));

    for (mut cargs, pctx) in kernel_contexts() {
        for &tf_chroma in &[true, false] {
            cargs.tf_chroma = i32::from(tf_chroma);
            let mut pctx = pctx.clone();
            pctx.tf_chroma = tf_chroma;

            let ny = y_stride * (bh + 2);
            let nc = uv_stride * (bh / 2 + 2);
            let base_acc = [vec![7u32; ny], vec![9u32; nc], vec![11u32; nc]];
            let base_cnt = [vec![3u16; ny], vec![5u16; nc], vec![13u16; nc]];

            let mut c_acc = base_acc.clone();
            let mut c_cnt = base_cnt.clone();
            cref::apply_planewise_medium(
                &cargs,
                &y_src,
                y_stride as i32,
                &y_pre,
                y_stride as i32,
                &u_src,
                &v_src,
                uv_stride as i32,
                &u_pre,
                &v_pre,
                uv_stride as i32,
                bw as u32,
                bh as u32,
                ss_x,
                ss_y,
                &mut c_acc,
                &mut c_cnt,
            );

            let mut r_acc = base_acc.clone();
            let mut r_cnt = base_cnt.clone();
            {
                let (a0, rest) = r_acc.split_at_mut(1);
                let (a1, a2) = rest.split_at_mut(1);
                let (c0, crest) = r_cnt.split_at_mut(1);
                let (c1, c2) = crest.split_at_mut(1);
                port::apply_temporal_filter_planewise_medium(
                    &pctx,
                    &y_src,
                    y_stride,
                    &y_pre,
                    y_stride,
                    &u_src,
                    &v_src,
                    uv_stride,
                    &u_pre,
                    &v_pre,
                    uv_stride,
                    bw,
                    bh,
                    ss_x as u32,
                    ss_y as u32,
                    &mut a0[0],
                    &mut c0[0],
                    &mut a1[0],
                    &mut c1[0],
                    &mut a2[0],
                    &mut c2[0],
                );
            }

            assert_eq!(
                r_acc, c_acc,
                "medium accum mismatch ctx {cargs:?} chroma {tf_chroma}"
            );
            assert_eq!(
                r_cnt, c_cnt,
                "medium count mismatch ctx {cargs:?} chroma {tf_chroma}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 2 * 4 * 2 * 4 * 2);
}

#[test]
fn apply_planewise_medium_hbd_matches_c() {
    let mut cells = 0usize;
    let (bw, bh) = (64usize, 64usize);
    let (ss_x, ss_y) = (1i32, 1i32);
    let y_stride = bw + 6;
    let uv_stride = (bw >> 1) + 2;

    let y_src = fill16(201, y_stride * (bh + 2), 0x3FF);
    let y_pre = fill16(202, y_stride * (bh + 2), 0x3FF);
    let u_src = fill16(203, uv_stride * (bh / 2 + 2), 0x3FF);
    let v_src = fill16(204, uv_stride * (bh / 2 + 2), 0x3FF);
    let u_pre = fill16(205, uv_stride * (bh / 2 + 2), 0x3FF);
    let v_pre = fill16(206, uv_stride * (bh / 2 + 2), 0x3FF);

    for (mut cargs, pctx) in kernel_contexts() {
        cargs.tf_chroma = 1;
        let mut pctx = pctx.clone();
        pctx.tf_chroma = true;

        let ny = y_stride * (bh + 2);
        let nc = uv_stride * (bh / 2 + 2);
        let base_acc = [vec![7u32; ny], vec![9u32; nc], vec![11u32; nc]];
        let base_cnt = [vec![3u16; ny], vec![5u16; nc], vec![13u16; nc]];

        let mut c_acc = base_acc.clone();
        let mut c_cnt = base_cnt.clone();
        cref::apply_planewise_medium_hbd(
            &cargs,
            &y_src,
            y_stride as i32,
            &y_pre,
            y_stride as i32,
            &u_src,
            &v_src,
            uv_stride as i32,
            &u_pre,
            &v_pre,
            uv_stride as i32,
            bw as u32,
            bh as u32,
            ss_x,
            ss_y,
            &mut c_acc,
            &mut c_cnt,
            10,
        );

        let mut r_acc = base_acc.clone();
        let mut r_cnt = base_cnt.clone();
        {
            let (a0, rest) = r_acc.split_at_mut(1);
            let (a1, a2) = rest.split_at_mut(1);
            let (c0, crest) = r_cnt.split_at_mut(1);
            let (c1, c2) = crest.split_at_mut(1);
            port::apply_temporal_filter_planewise_medium_hbd(
                &pctx,
                &y_src,
                y_stride,
                &y_pre,
                y_stride,
                &u_src,
                &v_src,
                uv_stride,
                &u_pre,
                &v_pre,
                uv_stride,
                bw,
                bh,
                ss_x as u32,
                ss_y as u32,
                &mut a0[0],
                &mut c0[0],
                &mut a1[0],
                &mut c1[0],
                &mut a2[0],
                &mut c2[0],
                10,
            );
        }

        assert_eq!(r_acc, c_acc, "medium hbd accum mismatch ctx {cargs:?}");
        assert_eq!(r_cnt, c_cnt, "medium hbd count mismatch ctx {cargs:?}");
        cells += 1;
    }
    assert_eq!(cells, 2 * 4 * 2 * 4);
}
