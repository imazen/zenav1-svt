use super::*;

/// C `svt_av1_compute_stats_highbd_c` (restoration_pick.c:692).
///
/// Two deltas vs the 8-bit [`compute_stats`], both load-bearing:
/// * the windowed differences are `int32_t` (not `int16_t`) and the products
///   accumulate as `int64_t` — a 10-bit residual overflows the 16-bit form;
/// * every M and H entry is divided by `bit_depth_divider` at the end
///   (4 at EB_TEN_BIT, 16 at EB_TWELVE_BIT) — an integer division applied
///   AFTER accumulation, so it is NOT the same as scaling the inputs.
///   Note C divides the diagonal `H[k][k]` and the upper triangle, then
///   MIRRORS the divided upper triangle down; it never divides the lower
///   triangle separately.
#[allow(clippy::too_many_arguments)]
pub fn compute_stats_hbd(
    wiener_win: usize,
    dgd: &[u16],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
    bit_depth: u8,
) {
    let win2 = wiener_win * wiener_win;
    let halfwin = (wiener_win >> 1) as i32;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let avg = find_average_hbd(dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end) as i32;
    let divider: i64 = match bit_depth {
        12 => 16,
        10 => 4,
        _ => 1,
    };

    m[..win2].fill(0);
    h[..win2 * win2].fill(0);
    // Same byte-inert reshaping as the 8-bit [`compute_stats`]: exact-length
    // M/H slices + a check-free chunk/zip walk of the upper-triangular
    // accumulation. Identical i64 products in the same per-element order, so
    // M/H (and thus the divided/mirrored result below) are bit-for-bit equal.
    let m = &mut m[..win2];
    let h = &mut h[..win2 * win2];
    let mut y = [0i32; WIENER_WIN * WIENER_WIN];
    for i in v_start..v_end {
        for j in h_start..h_end {
            let sidx = src_origin as isize + i as isize * src_stride as isize + j as isize;
            let x = src[sidx as usize] as i32 - avg;
            let mut idx = 0usize;
            for k in -halfwin..=halfwin {
                for l in -halfwin..=halfwin {
                    let didx = dgd_origin as isize
                        + (i + l) as isize * dgd_stride as isize
                        + (j + k) as isize;
                    y[idx] = dgd[didx as usize] as i32 - avg;
                    idx += 1;
                }
            }
            debug_assert_eq!(idx, win2);
            let ys = &y[..win2];
            let xi = x as i64;
            for (k, hrow) in h.chunks_exact_mut(win2).enumerate() {
                let yk = ys[k] as i64;
                m[k] += yk * xi;
                for (hv, &yl) in hrow[k..].iter_mut().zip(&ys[k..]) {
                    *hv += yk * yl as i64;
                }
            }
        }
    }
    for k in 0..win2 {
        m[k] /= divider;
        h[k * win2 + k] /= divider;
        for l in (k + 1)..win2 {
            h[k * win2 + l] /= divider;
            h[l * win2 + k] = h[k * win2 + l];
        }
    }
}

/// C `svt_av1_highbd_wiener_convolve_add_src_c` (convolve.c:200) — u16 twin
/// of [`wiener_convolve_add_src`] with a live `bd`. `bd` enters in exactly
/// three places: the horizontal rounding offset `1 << (bd + FILTER_BITS - 1)`,
/// the intermediate clamp `WIENER_CLAMP_LIMIT(round0, bd)`, and the vertical
/// rounding offset `1 << (bd + round1 - 1)` + the final
/// `clip_pixel_highbd(_, bd)`.
///
/// `get_conv_params_wiener(bd)` leaves round_0/round_1 at 3/11 for bd <= 10
/// (`intbufrange = bd + 7 - 3 + 2` only exceeds 16 at bd12), so the shifts
/// are the bd8 ones — asserted below.
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src_hbd(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
    bd: i32,
) {
    debug_assert!(
        bd + FILTER_BITS - WIENER_ROUND0_BITS + 2 <= 16,
        "get_conv_params_wiener would re-balance round_0/round_1 above bd10"
    );
    let ih = h + 6;
    let mut temp = alloc::vec![0u16; (ih + 1) * w.max(1)];
    let tstride = w;

    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;
    for y in 0..ih {
        let row_base = (src_origin + y * src_stride) as isize - 3 * src_stride as isize;
        for x in 0..w {
            let px = |k: usize| -> i32 {
                let idx = row_base + x as isize + k as isize - 3;
                src[idx as usize] as i32
            };
            let mut sum: i32 = (px(3) << FILTER_BITS) + (1 << (bd + FILTER_BITS - 1));
            for (k, &f) in hfilter.iter().enumerate() {
                sum += px(k) * f as i32;
            }
            temp[y * tstride + x] =
                round_power_of_two(sum, WIENER_ROUND0_BITS).clamp(0, clamp_limit) as u16;
        }
    }

    let pixel_max = (1i32 << bd) - 1;
    for x in 0..w {
        for y in 0..h {
            let base = y * tstride + x;
            let center = temp[base + 3 * tstride] as i32;
            let mut sum: i32 = (center << FILTER_BITS) - (1 << (bd + WIENER_ROUND1_BITS - 1));
            for (k, &f) in vfilter.iter().enumerate() {
                sum += temp[base + k * tstride] as i32 * f as i32;
            }
            dst[dst_origin + y * dst_stride + x] =
                round_power_of_two(sum, WIENER_ROUND1_BITS).clamp(0, pixel_max) as u16;
        }
    }
}

/// C `wiener_filter_stripe_highbd` (restoration.c): u16 twin of
/// [`wiener_filter_stripe`].
#[allow(clippy::too_many_arguments)]
pub(super) fn wiener_filter_stripe_hbd(
    wiener: &WienerInfo,
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min((stripe_width - j + 15) & !15);
        wiener_convolve_add_src_hbd(
            src,
            src_origin + j as usize,
            src_stride,
            dst,
            dst_origin + j as usize,
            dst_stride,
            &wiener.hfilter,
            &wiener.vfilter,
            w as usize,
            stripe_height as usize,
            bd,
        );
        j += procunit_width;
    }
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040) at
/// `highbd = 1` and `need_boundaries = 0` — the SEARCH path.
///
/// `use_boundaries_in_rest_search = 0` (enc_handle.c:4483) means
/// `try_restoration_unit_seg` never runs the stripe-boundary save/restore, so
/// this omits that machinery entirely rather than carrying an untested copy
/// of it; the bd8 [`loop_restoration_filter_unit`] keeps both arms because it
/// also serves the decoder-exact APPLY. The stripe SPLIT itself is kept
/// verbatim — it is what makes the filter see 64-row stripes offset by
/// `RESTORATION_UNIT_OFFSET`, which changes the output even with no boundary
/// substitution.
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_search_hbd(
    limits: &TileLimits,
    rui: &RestUnitParams,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &[u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let unit_h = limits.v_end - limits.v_start;
    let unit_w = limits.h_end - limits.h_start;
    let data_tl = data_origin + limits.v_start as usize * stride + limits.h_start as usize;
    let dst_tl = dst_origin + limits.v_start as usize * dst_stride + limits.h_start as usize;

    if rui.rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            dst[d..d + unit_w as usize].copy_from_slice(&data[s..s + unit_w as usize]);
        }
        return;
    }
    debug_assert!(rui.rtype == RESTORE_WIENER || rui.rtype == RESTORE_SGRPROJ);

    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let mut i = 0i32;
    while i < unit_h {
        let v_start = limits.v_start + i;
        let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
        let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;
        let tile_stripe = (v_start - tile_rect.top + runit_offset) / full_stripe_height;
        let _frame_stripe = tile_stripe0 + tile_stripe;
        let nominal_stripe_height =
            full_stripe_height - if tile_stripe == 0 { runit_offset } else { 0 };
        let h = nominal_stripe_height.min(limits.v_end - v_start);

        if rui.rtype == RESTORE_SGRPROJ {
            crate::port_sgr::sgrproj_filter_stripe_highbd(
                rui.sgr_ep,
                &rui.sgr_xqd,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        } else {
            wiener_filter_stripe_hbd(
                &rui.wiener,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        }
        i += h;
    }
}

/// C `svt_aom_highbd_get_sse` (svt_psnr.c:93), the kernel behind
/// `sse_restoration_unit` at `highbd = 1`.
///
/// Reproduces C's decomposition verbatim — 16x16 blocks plus a right strip
/// of `width % 16` and a bottom strip of `height % 16` — INCLUDING the
/// `(uint32_t)` truncation C applies to each partial sum before accumulating
/// into the i64 total. At 10 bits a tall right strip can genuinely exceed
/// 2^32 (15 cols * 384 rows * 1023^2 > 2^32), so the truncation is
/// observable, not cosmetic.
#[allow(clippy::too_many_arguments)]
pub fn sse_region_hbd(
    a: &[u16],
    a_origin: usize,
    a_stride: usize,
    b: &[u16],
    b_origin: usize,
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    // C `highbd_variance` (svt_psnr.c:78) over a sub-rect, returning i64;
    // the caller truncates to u32.
    let var = |ao: usize, bo: usize, w: usize, h: usize| -> i64 {
        let mut sse = 0i64;
        for i in 0..h {
            for j in 0..w {
                let d = a[ao + i * a_stride + j] as i64 - b[bo + i * b_stride + j] as i64;
                sse += d * d;
            }
        }
        sse
    };
    let dw = width % 16;
    let dh = height % 16;
    let mut total = 0i64;
    if dw > 0 {
        total += var(a_origin + width - dw, b_origin + width - dw, dw, height) as u32 as i64;
    }
    if dh > 0 {
        total += var(
            a_origin + (height - dh) * a_stride,
            b_origin + (height - dh) * b_stride,
            width - dw,
            dh,
        ) as u32 as i64;
    }
    for y in 0..height / 16 {
        for x in 0..width / 16 {
            let ao = a_origin + y * 16 * a_stride + x * 16;
            let bo = b_origin + y * 16 * b_stride + x * 16;
            // `svt_aom_highbd_mse16x16` — always < 2^32 for bd <= 12.
            total += var(ao, bo, 16, 16) as u32 as i64;
        }
    }
    total
}
