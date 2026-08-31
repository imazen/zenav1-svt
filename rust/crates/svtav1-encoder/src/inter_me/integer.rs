//! Full-pel (integer) open-loop search — the second half of
//! `Source/Lib/Codec/motion_estimation.c`.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`get_eight_search_point_results_block`] | `open_loop_me_get_eight_search_point_results_block` (:408) | no |
//! | [`get_search_point_results_block`] | `open_loop_me_get_search_point_results_block` (:456) | no |
//! | [`fullpel_search_sblock`] | `open_loop_me_fullpel_search_sblock` (:755) | no |
//! | [`apply_me_sa_boost`] | `apply_me_sa_boost` (:1163) | no |
//! | [`integer_search_b64`] | `integer_search_b64` (:1185) | no |
//! | [`me_prune_ref`] | `me_prune_ref` (:1415) | no |

use super::context::*;
use super::hme::{check_00_center, get_me_reference_dist, get_scaled_picture_distance};
use super::sad::{
    ext_all_sad_calculation_8x8_16x16, ext_eight_sad_calculation_32x32_64x64,
    ext_sad_calculation_8x8_16x16, ext_sad_calculation_32x32_64x64,
};
use super::tables::TAB8X8;

/// The 16x16 visit order C hard-codes in
/// `open_loop_me_get_search_point_results_block` (and in
/// `svt_ext_all_sad_calculation_8x8_16x16_c`): raster position `4*y + x` maps
/// to 16x16 slot `Z16[4*y+x]`, 8x8 base `4 * Z16[4*y+x]`.
const Z16: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

/// C `open_loop_me_get_eight_search_point_results_block`
/// (motion_estimation.c:408).
#[allow(clippy::too_many_arguments)]
pub fn get_eight_search_point_results_block(
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    ref_plane: &Plane,
    list_index: usize,
    ref_pic_index: usize,
    search_region_index: i64,
    x_search_index: i32,
    y_search_index: i32,
) {
    let sub_sad = me_ctx.me_search_method == SUB_SAD_SEARCH;
    let ref_luma_stride = me_ctx.interpolated_full_stride[list_index][ref_pic_index];
    let half_tap = (ME_FILTER_TAP >> 1) as i64;
    let base = me_ctx.integer_buffer_off[list_index][ref_pic_index]
        + half_tap * ref_luma_stride as i64
        + half_tap
        + search_region_index;
    let rf = &ref_plane.data[base as usize..];

    let curr_mv = ((y_search_index as u32) << 16) | u32::from(x_search_index as u16);

    // C aims p_best_* at p_sb_best_{sad,mv}[list][ref] + PU_{8X8,16X16}_0.
    let mut best_sad = me_ctx.p_sb_best_sad[list_index][ref_pic_index];
    let mut best_mv = me_ctx.p_sb_best_mv[list_index][ref_pic_index];

    ext_all_sad_calculation_8x8_16x16(
        src.b64,
        src.b64_stride,
        rf,
        ref_luma_stride,
        curr_mv,
        &mut best_sad,
        &mut best_mv,
        PU_8X8_0,
        PU_16X16_0,
        &mut me_ctx.p_eight_sad16x16,
        sub_sad,
    );
    let eight16 = me_ctx.p_eight_sad16x16;
    ext_eight_sad_calculation_32x32_64x64(
        &eight16,
        &mut best_sad,
        &mut best_mv,
        PU_32X32_0,
        PU_64X64,
        curr_mv,
        &mut me_ctx.p_eight_sad32x32,
    );

    me_ctx.p_sb_best_sad[list_index][ref_pic_index] = best_sad;
    me_ctx.p_sb_best_mv[list_index][ref_pic_index] = best_mv;
}

/// C `open_loop_me_get_search_point_results_block`
/// (motion_estimation.c:456). C unrolls the sixteen 16x16 calls in the
/// [`Z16`] order; the loop below issues the identical sequence of kernel
/// invocations with the identical offsets.
#[allow(clippy::too_many_arguments)]
pub fn get_search_point_results_block(
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    ref_plane: &Plane,
    list_index: usize,
    ref_pic_index: usize,
    search_region_index: i64,
    x_search_index: i32,
    y_search_index: i32,
) {
    let sub_sad = me_ctx.me_search_method == SUB_SAD_SEARCH;
    let ref_luma_stride = me_ctx.interpolated_full_stride[list_index][ref_pic_index];
    let half_tap = (ME_FILTER_TAP >> 1) as i64;
    let ref_base = me_ctx.integer_buffer_off[list_index][ref_pic_index]
        + half_tap
        + half_tap * ref_luma_stride as i64;

    let curr_mv = ((y_search_index as u32) << 16) | u32::from(x_search_index as u16);
    let src_stride = src.b64_stride;
    let src_next_16x16_offset = src_stride << 4;
    let ref_next_16x16_offset = ref_luma_stride << 4;

    let mut best_sad = me_ctx.p_sb_best_sad[list_index][ref_pic_index];
    let mut best_mv = me_ctx.p_sb_best_mv[list_index][ref_pic_index];
    let mut sad16 = me_ctx.p_sad16x16;
    let mut sad8 = me_ctx.p_sad8x8;

    for y in 0..4usize {
        for x in 0..4usize {
            let idx16 = Z16[4 * y + x];
            let block_index = y * src_next_16x16_offset + 16 * x;
            let search_position_index =
                search_region_index + (y * ref_next_16x16_offset) as i64 + (16 * x) as i64;
            ext_sad_calculation_8x8_16x16(
                &src.b64[block_index..],
                src_stride,
                &ref_plane.data[(ref_base + search_position_index) as usize..],
                ref_luma_stride,
                &mut best_sad,
                &mut best_mv,
                PU_8X8_0 + 4 * idx16,
                PU_16X16_0 + idx16,
                curr_mv,
                &mut sad16,
                idx16,
                &mut sad8,
                4 * idx16,
                sub_sad,
            );
        }
    }

    let mut sad32 = me_ctx.p_sad32x32;
    ext_sad_calculation_32x32_64x64(
        &sad16,
        &mut best_sad,
        &mut best_mv,
        PU_32X32_0,
        PU_64X64,
        curr_mv,
        &mut sad32,
    );

    me_ctx.p_sb_best_sad[list_index][ref_pic_index] = best_sad;
    me_ctx.p_sb_best_mv[list_index][ref_pic_index] = best_mv;
    me_ctx.p_sad16x16 = sad16;
    me_ctx.p_sad8x8 = sad8;
    me_ctx.p_sad32x32 = sad32;
}

/// C `open_loop_me_fullpel_search_sblock` (motion_estimation.c:755).
#[allow(clippy::too_many_arguments)]
pub fn fullpel_search_sblock(
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    ref_plane: &Plane,
    list_index: usize,
    ref_pic_index: usize,
    x_search_area_origin: i16,
    y_search_area_origin: i16,
    search_area_width: u32,
    search_area_height: u32,
) {
    let rest8 = search_area_width & 7;
    let mult8 = search_area_width - rest8;
    let stride = me_ctx.interpolated_full_stride[list_index][ref_pic_index] as i64;

    for y in 0..search_area_height {
        let mut x = 0u32;
        while x < mult8 {
            get_eight_search_point_results_block(
                me_ctx,
                src,
                ref_plane,
                list_index,
                ref_pic_index,
                i64::from(x) + i64::from(y) * stride,
                x as i32 + i32::from(x_search_area_origin),
                y as i32 + i32::from(y_search_area_origin),
            );
            x += 8;
        }
        for x in mult8..search_area_width {
            get_search_point_results_block(
                me_ctx,
                src,
                ref_plane,
                list_index,
                ref_pic_index,
                i64::from(x) + i64::from(y) * stride,
                x as i32 + i32::from(x_search_area_origin),
                y as i32 + i32::from(y_search_area_origin),
            );
        }
    }
}

/// C `search_area_multipliers` (motion_estimation.c:1157).
const SEARCH_AREA_MULTIPLIERS: [[f64; 5]; 3] = [
    [1.0, 1.0, 3.0, 4.0, 5.0], // boost = 1
    [1.0, 1.0, 2.5, 3.5, 4.5], // boost = 2
    [1.0, 1.0, 2.0, 2.5, 3.5], // boost = 3
];

/// C `apply_me_sa_boost` (motion_estimation.c:1163). Index 1 of the table is
/// unreachable — C's ladder produces 0, 2, 3 or 4 — and is kept for fidelity.
pub fn apply_me_sa_boost(width: &mut i16, height: &mut i16, hme_sad: u64, sc_class_me_boost: u8) {
    let index = if hme_sad > 4 * 64 * 64 {
        4
    } else if hme_sad > 3 * 64 * 64 {
        3
    } else if hme_sad > 2 * 64 * 64 {
        2
    } else {
        0
    };
    let mult = SEARCH_AREA_MULTIPLIERS[sc_class_me_boost as usize - 1][index];
    *width = (f64::from(*width) * mult) as i16;
    *height = (f64::from(*height) * mult) as i16;
}

/// C `integer_search_b64` (motion_estimation.c:1185).
pub fn integer_search_b64(
    pic: &MePicParams,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
    b64_origin_x: u32,
    b64_origin_y: u32,
) {
    let picture_width = i32::from(pic.aligned_width);
    let picture_height = i32::from(pic.aligned_height);
    let b64_width = me_ctx.b64_width;
    let b64_height = me_ctx.b64_height;
    let pad_width = BLOCK_SIZE_64 - 1;
    let pad_height = BLOCK_SIZE_64 - 1;
    let org_x = b64_origin_x as i16;
    let org_y = b64_origin_y as i16;

    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            let r = refs.get(list_index, ref_pic_index);
            let ref_pic = r.picture;
            let mut dist = get_me_reference_dist(pic.picture_number, r.picture_number);

            if me_ctx.search_results[list_index][ref_pic_index].do_ref == 0 {
                continue;
            }
            let mut x_search_center = me_ctx.search_results[list_index][ref_pic_index].hme_sc_x;
            let mut y_search_center = me_ctx.search_results[list_index][ref_pic_index].hme_sc_y;
            let mut search_area_width = me_ctx.me_sa.sa_min.width as i16;
            let mut search_area_height = me_ctx.me_sa.sa_min.height as i16;

            if me_ctx.me_type != MeType::Mctf {
                dist = get_scaled_picture_distance(dist);
            }
            search_area_width = i32::min(
                i32::from(search_area_width) * i32::from(dist),
                i32::from(me_ctx.me_sa.sa_max.width),
            ) as i16;
            search_area_height = i32::min(
                i32::from(search_area_height) * i32::from(dist),
                i32::from(me_ctx.me_sa.sa_max.height),
            ) as i16;

            if me_ctx.mv_based_sa_adj.enabled
                && (!me_ctx.mv_based_sa_adj.nearest_ref_only || ref_pic_index == 0)
            {
                if i32::from(x_search_center).abs() > i32::from(me_ctx.mv_based_sa_adj.mv_size_th) {
                    search_area_width = (i32::from(search_area_width)
                        * i32::from(me_ctx.mv_based_sa_adj.sa_multiplier))
                        as i16;
                }
                if i32::from(y_search_center).abs() > i32::from(me_ctx.mv_based_sa_adj.mv_size_th) {
                    search_area_height = (i32::from(search_area_height)
                        * i32::from(me_ctx.mv_based_sa_adj.sa_multiplier))
                        as i16;
                }
            }

            if me_ctx.sc_class_me_boost != 0
                && (pic.ahd_error == u32::MAX
                    || u64::from(pic.ahd_error)
                        < (((20 * u64::from(pic.enhanced_width) * u64::from(pic.enhanced_height))
                            / 128)
                            * u64::from(INPUT_SIZE_COUNT - u32::from(pic.input_resolution))))
            {
                let hme_sad = me_ctx.search_results[list_index][ref_pic_index].hme_sad;
                apply_me_sa_boost(
                    &mut search_area_width,
                    &mut search_area_height,
                    hme_sad,
                    me_ctx.sc_class_me_boost,
                );
            }

            // C divides an int16_t by a uint32_t here, so the arithmetic is
            // performed in uint32_t and only then truncated back to int16_t.
            let div = me_ctx.reduce_me_sr_divisor[list_index][ref_pic_index];
            search_area_width =
                ((u32::max(1, (search_area_width as u32) / div) + 7) & !0x07) as i16;
            search_area_height = u32::max(3, (search_area_height as u32) / div) as i16;
            let search_area_height_before_sr_reduction = search_area_height;
            let mut best_hme_sad = u64::MAX;

            if me_ctx.me_early_exit_th != 0 {
                if me_ctx.zz_sad[list_index][ref_pic_index] < (me_ctx.me_early_exit_th / 6) {
                    search_area_width = 1;
                    search_area_height = 1;
                }
            } else {
                let mut hme_is_accurate = true;
                if (x_search_center != 0 || y_search_center != 0) && me_ctx.is_ref {
                    best_hme_sad = u64::from(check_00_center(
                        &ref_pic,
                        me_ctx,
                        src,
                        b64_origin_x,
                        b64_origin_y,
                        b64_width,
                        b64_height,
                        &mut x_search_center,
                        &mut y_search_center,
                        me_ctx.zz_sad[list_index][ref_pic_index],
                    ));
                    if x_search_center == 0 && y_search_center == 0 {
                        hme_is_accurate = false;
                    }
                }
                if me_ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment == 2 {
                    if (hme_is_accurate && best_hme_sad < (24 * 24))
                        || (me_ctx.is_ref
                            && me_ctx.search_results[list_index][ref_pic_index].hme_sad < (24 * 24))
                    {
                        search_area_height /= 2;
                    }
                    if (list_index != 0 || ref_pic_index != 0)
                        && me_ctx.p_sb_best_sad[0][0][0] < 5000
                        && search_area_height == search_area_height_before_sr_reduction
                    {
                        search_area_height >>= 1;
                        search_area_width >>= 1;
                    }
                }
            }

            // svt_initialize_buffer_32bits(..., 21, 1, MAX_SAD_VALUE) fills
            // 21*4 + 1 == SQUARE_PU_COUNT entries (me_sad_calculation.c:14).
            me_ctx.p_sb_best_sad[list_index][ref_pic_index] = [MAX_SAD_VALUE; SQUARE_PU_COUNT];

            let mut x_search_area_origin;
            let mut y_search_area_origin;

            if me_ctx.me_8x8_var_ctrls.enabled != 0
                && (i32::from(search_area_width) * i32::from(search_area_height)) > 24
            {
                x_search_area_origin = x_search_center;
                y_search_area_origin = y_search_center;
                let x_tl = i32::from(b64_origin_x as i16) - (ME_FILTER_TAP >> 1)
                    + i32::from(x_search_area_origin);
                let y_tl = i32::from(b64_origin_y as i16) - (ME_FILTER_TAP >> 1)
                    + i32::from(y_search_area_origin);
                let search_region_index = i64::from(x_tl) + i64::from(y_tl) * ref_pic.stride as i64;
                me_ctx.integer_buffer_off[list_index][ref_pic_index] =
                    ref_pic.abs(search_region_index);
                me_ctx.interpolated_full_stride[list_index][ref_pic_index] = ref_pic.stride;

                fullpel_search_sblock(
                    me_ctx,
                    src,
                    &ref_pic,
                    list_index,
                    ref_pic_index,
                    x_search_center,
                    y_search_center,
                    1,
                    1,
                );

                let mean_dist_8x8 = me_ctx.p_sb_best_sad[list_index][ref_pic_index][PU_64X64] / 64;
                let mut sum_ofsq_dist_8x8 = 0u32;
                for i in 0..64usize {
                    let diff = me_ctx.p_sb_best_sad[list_index][ref_pic_index][PU_8X8_0 + i] as i32
                        - mean_dist_8x8 as i32;
                    sum_ofsq_dist_8x8 = sum_ofsq_dist_8x8.wrapping_add((diff * diff) as u32);
                }
                let me_8x8_cost_var = sum_ofsq_dist_8x8 / 64;

                if me_8x8_cost_var > me_ctx.me_8x8_var_ctrls.me_sr_mult2_th {
                    search_area_width =
                        ((i32::max(1, i32::from(search_area_width) * 3 / 2) + 7) & !0x7) as i16;
                    search_area_height = i32::max(1, i32::from(search_area_height) * 3 / 2) as i16;
                }
                if me_8x8_cost_var < me_ctx.me_8x8_var_ctrls.me_sr_div4_th {
                    search_area_width =
                        ((i32::max(1, i32::from(search_area_width) >> 2) + 7) & !0x7) as i16;
                    search_area_height = i32::max(1, i32::from(search_area_height) >> 2) as i16;
                    search_area_height = i32::max(3, i32::from(search_area_height)) as i16;
                } else if me_8x8_cost_var < me_ctx.me_8x8_var_ctrls.me_sr_div2_th {
                    search_area_width = ((i32::min(
                        i32::from(search_area_width),
                        i32::from(search_area_width) >> 1,
                    ) + 7)
                        & !0x7) as i16;
                    search_area_height = i32::min(
                        i32::from(search_area_height),
                        i32::from(search_area_height) >> 1,
                    ) as i16;
                    search_area_height = i32::max(3, i32::from(search_area_height)) as i16;
                }
            }

            x_search_area_origin =
                (i32::from(x_search_center) - (i32::from(search_area_width) >> 1)) as i16;
            y_search_area_origin =
                (i32::from(y_search_center) - (i32::from(search_area_height) >> 1)) as i16;

            let ox = i32::from(org_x);
            let oy = i32::from(org_y);
            // C's ternary form: the width term re-reads the corrected origin
            // and is therefore always a no-op. Transcribed as written.
            let corrected_x = if ox + i32::from(x_search_area_origin) < -pad_width {
                (-pad_width - ox) as i16
            } else {
                x_search_area_origin
            };
            search_area_width = if ox + i32::from(corrected_x) < -pad_width {
                (i32::from(search_area_width) - (-pad_width - (ox + i32::from(corrected_x)))) as i16
            } else {
                search_area_width
            };
            x_search_area_origin = corrected_x;
            x_search_area_origin = if ox + i32::from(x_search_area_origin) > picture_width - 1 {
                (i32::from(x_search_area_origin)
                    - ((ox + i32::from(x_search_area_origin)) - (picture_width - 1)))
                    as i16
            } else {
                x_search_area_origin
            };
            search_area_width = if ox
                + i32::from(x_search_area_origin)
                + i32::from(search_area_width)
                > picture_width
            {
                i32::max(
                    1,
                    i32::from(search_area_width)
                        - ((ox + i32::from(x_search_area_origin) + i32::from(search_area_width))
                            - picture_width),
                ) as i16
            } else {
                search_area_width
            };
            search_area_width = if search_area_width < 8 {
                search_area_width
            } else {
                search_area_width & !0x07
            };

            let corrected_y = if oy + i32::from(y_search_area_origin) < -pad_height {
                (-pad_height - oy) as i16
            } else {
                y_search_area_origin
            };
            search_area_height = if oy + i32::from(corrected_y) < -pad_height {
                (i32::from(search_area_height) - (-pad_height - (oy + i32::from(corrected_y))))
                    as i16
            } else {
                search_area_height
            };
            y_search_area_origin = corrected_y;
            y_search_area_origin = if oy + i32::from(y_search_area_origin) > picture_height - 1 {
                (i32::from(y_search_area_origin)
                    - ((oy + i32::from(y_search_area_origin)) - (picture_height - 1)))
                    as i16
            } else {
                y_search_area_origin
            };
            search_area_height = if oy
                + i32::from(y_search_area_origin)
                + i32::from(search_area_height)
                > picture_height
            {
                i32::max(
                    1,
                    i32::from(search_area_height)
                        - ((oy + i32::from(y_search_area_origin) + i32::from(search_area_height))
                            - picture_height),
                ) as i16
            } else {
                search_area_height
            };

            let x_tl = i32::from(b64_origin_x as i16) - (ME_FILTER_TAP >> 1)
                + i32::from(x_search_area_origin);
            let y_tl = i32::from(b64_origin_y as i16) - (ME_FILTER_TAP >> 1)
                + i32::from(y_search_area_origin);
            let search_region_index = i64::from(x_tl) + i64::from(y_tl) * ref_pic.stride as i64;
            me_ctx.integer_buffer_off[list_index][ref_pic_index] = ref_pic.abs(search_region_index);
            me_ctx.interpolated_full_stride[list_index][ref_pic_index] = ref_pic.stride;

            fullpel_search_sblock(
                me_ctx,
                src,
                &ref_pic,
                list_index,
                ref_pic_index,
                x_search_area_origin,
                y_search_area_origin,
                search_area_width as u32,
                search_area_height as u32,
            );
        }
    }
}

/// C `me_prune_ref` (motion_estimation.c:1415).
pub fn me_prune_ref(me_ctx: &mut MeContext) {
    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            me_ctx.search_results[list_index][ref_pic_index].hme_sad = 0;
            if me_ctx.search_results[list_index][ref_pic_index].do_ref == 0 {
                me_ctx.search_results[list_index][ref_pic_index].hme_sad =
                    u64::from(MAX_SAD_VALUE) * 64;
                continue;
            }
            let mut acc = 0u64;
            for pu_index in 0..64usize {
                let idx = TAB8X8[pu_index] as usize;
                acc += u64::from(me_ctx.p_sb_best_sad[list_index][ref_pic_index][PU_8X8_0 + idx]);
            }
            me_ctx.search_results[list_index][ref_pic_index].hme_sad = acc;
        }
    }

    let prune_ref_th = me_ctx
        .me_hme_prune_ctrls
        .prune_ref_if_me_sad_dev_bigger_than_th;
    if me_ctx.me_hme_prune_ctrls.enable_me_hme_ref_pruning && prune_ref_th != u16::MAX {
        let mut best = u64::MAX;
        for i in 0..MAX_NUM_OF_REF_PIC_LIST {
            for j in 0..REF_LIST_MAX_DEPTH {
                if me_ctx.search_results[i][j].hme_sad < best {
                    best = me_ctx.search_results[i][j].hme_sad;
                }
            }
        }
        for li in 0..MAX_NUM_OF_REF_PIC_LIST {
            for ri in 1..REF_LIST_MAX_DEPTH {
                if me_ctx.search_results[li][ri]
                    .hme_sad
                    .wrapping_sub(best)
                    .wrapping_mul(100)
                    > u64::from(prune_ref_th).wrapping_mul(best)
                {
                    me_ctx.search_results[li][ri].do_ref = 0;
                }
            }
        }
    }
}
