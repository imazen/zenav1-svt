//! Hierarchical motion estimation — the pyramid half of
//! `Source/Lib/Codec/motion_estimation.c`.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`hme_level_0`] | `hme_level_0` (:787) | no (`static`) |
//! | [`hme_level_1`] | `hme_level_1` (:878) | no (`static`) |
//! | [`hme_level_2`] | `hme_level_2` (:971) | **yes** |
//! | [`check_00_center`] | `check_00_center` (:1060) | **yes** |
//! | [`get_scaled_picture_distance`] | `svt_aom_get_scaled_picture_distance` (:1152) | **yes** |
//! | [`get_me_reference_dist`] | `get_me_reference` (:1128) | no (`static`) |
//! | [`prehme_core`] | `prehme_core` (:1458) | no (`static`) |
//! | [`get_zz_sad`] | `get_zz_sad` (:1541) | no (`static`) |
//! | [`check_prehme_early_exit`] | `check_prehme_early_exit` (:1567) | no (`static`) |
//! | [`prehme_b64`] | `prehme_b64` (:1595) | no (`static`) |
//! | [`get_hme_l0_search_area`] | `get_hme_l0_search_area` (:1674) | no (`static`) |
//! | [`get_worst_quadrant`] | `get_worst_quadrant` (:1737) | no (`static`) |
//! | [`hme_level0_b64`] | `hme_level0_b64` (:1769) | no (`static`) |
//! | [`hme_level1_b64`] | `hme_level1_b64` (:1895) | no (`static`) |
//! | [`hme_level2_b64`] | `hme_level2_b64` (:1972) | no (`static`) |
//! | [`set_final_search_centre_sb`] | `set_final_search_centre_sb` (:2026) | **yes** |
//! | [`init_zz_sad`] | `init_zz_sad` (:2193) | no (`static`) |
//! | [`hme_b64`] | `hme_b64` (:2249) | no (`static`) |
//! | [`hme_prune_ref_and_adjust_sr`] | `hme_prune_ref_and_adjust_sr` (:2290) | no (`static`) |
//!
//! **A C quirk that is reproduced on purpose.** Every "correct the left/top
//! edge" block writes the origin FIRST and then recomputes the width from the
//! already-corrected origin, so the width adjustment always evaluates to zero:
//!
//! ```text
//! sa_origin_x = -pad_width - org_x;                       // org_x+sa_origin_x == -pad_width
//! sa_width    = sa_width - (-pad_width - (org_x + sa_origin_x));   // ... - 0
//! ```
//!
//! It is transcribed statement-for-statement rather than "fixed"; a C bug is
//! still the oracle (`docs/WORKING-ON-THIS.md` §7).

use super::context::*;
use super::sad::{nxm_sad_kernel, sad_loop_kernel};

/// C `svt_aom_get_scaled_picture_distance` (motion_estimation.c:1152).
pub fn get_scaled_picture_distance(dist: u16) -> u16 {
    // C evaluates `dist * 5` after the integer promotion to `int`, then
    // narrows the sum back to `uint16_t` on return.
    let round_up = u32::from(!dist.is_multiple_of(8));
    (((u32::from(dist) * 5) / 8) + round_up) as u16
}

/// C `get_me_reference`'s `*dist` output (motion_estimation.c:1128). The
/// picture selection itself is a field access on [`MeRefs`]; only the distance
/// needs code. C also emits an `SVT_WARN` on a resolution mismatch — a log
/// line with no effect on the search, deliberately not ported.
pub fn get_me_reference_dist(picture_number: u64, ref_picture_number: u64) -> u16 {
    (picture_number as i64 - ref_picture_number as i64).unsigned_abs() as u16
}

/// C `hme_level_0` (motion_estimation.c:787). Returns
/// `(best_sad, hme_l0_sc_x, hme_l0_sc_y)`.
#[allow(clippy::too_many_arguments)]
pub fn hme_level_0(
    me_ctx: &MeContext,
    src: &MeSrcBufs,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    sa_width_in: i16,
    sa_height_in: i16,
    sixteenth_ref: &Plane,
    sr_w: u32,
    sr_h: u32,
) -> (u64, i16, i16) {
    let mut sa_width = ((i32::from(sa_width_in) + 7) & !0x07) as i16;
    let mut sa_height = sa_height_in;
    let pad_width = i32::from(sixteenth_ref.border) - 1;
    let pad_height = i32::from(sixteenth_ref.border) - 1;

    let x_search_region_distance = (i32::from(sa_width) * sr_w as i32) as i16;
    let y_search_region_distance = (i32::from(sa_height) * sr_h as i32) as i16;
    let mut sa_origin_x = (-((i32::from(sa_width) * i32::from(me_ctx.num_hme_sa_w)) >> 1)
        + i32::from(x_search_region_distance)) as i16;
    let mut sa_origin_y = (-((i32::from(sa_height) * i32::from(me_ctx.num_hme_sa_h)) >> 1)
        + i32::from(y_search_region_distance)) as i16;

    let ox = i32::from(org_x);
    let oy = i32::from(org_y);
    if ox + i32::from(sa_origin_x) < -pad_width {
        sa_origin_x = (-pad_width - ox) as i16;
        sa_width = (i32::from(sa_width) - (-pad_width - (ox + i32::from(sa_origin_x)))) as i16;
    }
    if ox + i32::from(sa_origin_x) > i32::from(sixteenth_ref.width) - 1 {
        sa_origin_x = (i32::from(sa_origin_x)
            - ((ox + i32::from(sa_origin_x)) - (i32::from(sixteenth_ref.width) - 1)))
            as i16;
    }
    if ox + i32::from(sa_origin_x) + i32::from(sa_width) > i32::from(sixteenth_ref.width) {
        sa_width = i32::max(
            1,
            i32::from(sa_width)
                - ((ox + i32::from(sa_origin_x) + i32::from(sa_width))
                    - i32::from(sixteenth_ref.width)),
        ) as i16;
    }
    sa_width = if sa_width < 8 {
        sa_width
    } else {
        sa_width & !0x07
    };

    if oy + i32::from(sa_origin_y) < -pad_height {
        sa_origin_y = (-pad_height - oy) as i16;
        sa_height = (i32::from(sa_height) - (-pad_height - (oy + i32::from(sa_origin_y)))) as i16;
    }
    if oy + i32::from(sa_origin_y) > i32::from(sixteenth_ref.height) - 1 {
        sa_origin_y = (i32::from(sa_origin_y)
            - ((oy + i32::from(sa_origin_y)) - (i32::from(sixteenth_ref.height) - 1)))
            as i16;
    }
    if oy + i32::from(sa_origin_y) + i32::from(sa_height) > i32::from(sixteenth_ref.height) {
        sa_height = i32::max(
            1,
            i32::from(sa_height)
                - ((oy + i32::from(sa_origin_y) + i32::from(sa_height))
                    - i32::from(sixteenth_ref.height)),
        ) as i16;
    }

    let x_tl = ox + i32::from(sa_origin_x);
    let y_tl = oy + i32::from(sa_origin_y);
    let search_region_index = i64::from(x_tl) + i64::from(y_tl) * sixteenth_ref.stride as i64;

    let full = me_ctx.hme_search_method == FULL_SAD_SEARCH;
    let r = sad_loop_kernel(
        src.sixteenth,
        if full {
            src.sixteenth_stride
        } else {
            src.sixteenth_stride * 2
        },
        sixteenth_ref.data,
        sixteenth_ref.abs(search_region_index),
        if full {
            sixteenth_ref.stride
        } else {
            sixteenth_ref.stride * 2
        },
        if full {
            block_height as usize
        } else {
            (block_height >> 1) as usize
        },
        block_width as usize,
        sixteenth_ref.stride,
        0,
        sa_width,
        sa_height,
    );

    let best_sad = if full { r.best_sad } else { r.best_sad * 2 };
    let sc_x = (r.x_search_center.wrapping_add(sa_origin_x)).wrapping_mul(4);
    let sc_y = (r.y_search_center.wrapping_add(sa_origin_y)).wrapping_mul(4);
    (best_sad, sc_x, sc_y)
}

/// C `hme_level_1` (motion_estimation.c:878).
#[allow(clippy::too_many_arguments)]
pub fn hme_level_1(
    me_ctx: &MeContext,
    src: &MeSrcBufs,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    quarter_ref: &Plane,
    sa_width_in: i16,
    sa_height_in: i16,
    hme_l0_sc_x: i16,
    hme_l0_sc_y: i16,
) -> (u64, i16, i16) {
    let mut sa_width = ((i32::from(sa_width_in) + 7) & !0x07) as i16;
    let mut sa_height = sa_height_in;
    let pad_width = i32::from(quarter_ref.border) - 1;
    let pad_height = i32::from(quarter_ref.border) - 1;

    let mut sa_origin_x = (-(i32::from(sa_width) >> 1) + i32::from(hme_l0_sc_x)) as i16;
    let mut sa_origin_y = (-(i32::from(sa_height) >> 1) + i32::from(hme_l0_sc_y)) as i16;

    let ox = i32::from(org_x);
    let oy = i32::from(org_y);
    if ox + i32::from(sa_origin_x) < -pad_width {
        sa_origin_x = (-pad_width - ox) as i16;
        sa_width = (i32::from(sa_width) - (-pad_width - (ox + i32::from(sa_origin_x)))) as i16;
    }
    if ox + i32::from(sa_origin_x) > i32::from(quarter_ref.width) - 1 {
        sa_origin_x = (i32::from(sa_origin_x)
            - ((ox + i32::from(sa_origin_x)) - (i32::from(quarter_ref.width) - 1)))
            as i16;
    }
    if ox + i32::from(sa_origin_x) + i32::from(sa_width) > i32::from(quarter_ref.width) {
        sa_width = i32::max(
            1,
            i32::from(sa_width)
                - ((ox + i32::from(sa_origin_x) + i32::from(sa_width))
                    - i32::from(quarter_ref.width)),
        ) as i16;
    }
    sa_width = if sa_width < 8 {
        sa_width
    } else {
        sa_width & !0x07
    };

    if oy + i32::from(sa_origin_y) < -pad_height {
        sa_origin_y = (-pad_height - oy) as i16;
        sa_height = (i32::from(sa_height) - (-pad_height - (oy + i32::from(sa_origin_y)))) as i16;
    }
    if oy + i32::from(sa_origin_y) > i32::from(quarter_ref.height) - 1 {
        sa_origin_y = (i32::from(sa_origin_y)
            - ((oy + i32::from(sa_origin_y)) - (i32::from(quarter_ref.height) - 1)))
            as i16;
    }
    if oy + i32::from(sa_origin_y) + i32::from(sa_height) > i32::from(quarter_ref.height) {
        sa_height = i32::max(
            1,
            i32::from(sa_height)
                - ((oy + i32::from(sa_origin_y) + i32::from(sa_height))
                    - i32::from(quarter_ref.height)),
        ) as i16;
    }

    let x_tl = ox + i32::from(sa_origin_x);
    let y_tl = oy + i32::from(sa_origin_y);
    let search_region_index = i64::from(x_tl) + i64::from(y_tl) * quarter_ref.stride as i64;

    let full = me_ctx.hme_search_method == FULL_SAD_SEARCH;
    let r = sad_loop_kernel(
        src.quarter,
        if full {
            src.quarter_stride
        } else {
            src.quarter_stride * 2
        },
        quarter_ref.data,
        quarter_ref.abs(search_region_index),
        if full {
            quarter_ref.stride
        } else {
            quarter_ref.stride * 2
        },
        if full {
            block_height as usize
        } else {
            (block_height >> 1) as usize
        },
        block_width as usize,
        quarter_ref.stride,
        0,
        sa_width,
        sa_height,
    );

    let best_sad = if full { r.best_sad } else { r.best_sad * 2 };
    let sc_x = (r.x_search_center.wrapping_add(sa_origin_x)).wrapping_mul(2);
    let sc_y = (r.y_search_center.wrapping_add(sa_origin_y)).wrapping_mul(2);
    (best_sad, sc_x, sc_y)
}

/// C `hme_level_2` (motion_estimation.c:971) — EXPORTED, so this is the
/// pyramid level with a tier-1 differential oracle.
///
/// The padding used here is `BLOCK_SIZE_64 - 1`, NOT the picture's own border:
/// full-resolution HME assumes a 64-sample border regardless of the descriptor.
#[allow(clippy::too_many_arguments)]
pub fn hme_level_2(
    me_ctx: &MeContext,
    src: &MeSrcBufs,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    ref_pic: &Plane,
    sa_width_in: i16,
    sa_height_in: i16,
    hme_l1_sc_x: i16,
    hme_l1_sc_y: i16,
) -> (u64, i16, i16) {
    let mut sa_width = ((i32::from(sa_width_in) + 7) & !0x07) as i16;
    let mut sa_height = sa_height_in;
    let pad_width = BLOCK_SIZE_64 - 1;
    let pad_height = BLOCK_SIZE_64 - 1;

    let mut sa_origin_x = (-(i32::from(sa_width) >> 1) + i32::from(hme_l1_sc_x)) as i16;
    let mut sa_origin_y = (-(i32::from(sa_height) >> 1) + i32::from(hme_l1_sc_y)) as i16;

    let ox = i32::from(org_x);
    let oy = i32::from(org_y);
    if ox + i32::from(sa_origin_x) < -pad_width {
        sa_origin_x = (-pad_width - ox) as i16;
        sa_width = (i32::from(sa_width) - (-pad_width - (ox + i32::from(sa_origin_x)))) as i16;
    }
    if ox + i32::from(sa_origin_x) > i32::from(ref_pic.width) - 1 {
        sa_origin_x = (i32::from(sa_origin_x)
            - ((ox + i32::from(sa_origin_x)) - (i32::from(ref_pic.width) - 1)))
            as i16;
    }
    if ox + i32::from(sa_origin_x) + i32::from(sa_width) > i32::from(ref_pic.width) {
        sa_width = i32::max(
            1,
            i32::from(sa_width)
                - ((ox + i32::from(sa_origin_x) + i32::from(sa_width)) - i32::from(ref_pic.width)),
        ) as i16;
    }
    sa_width = if sa_width < 8 {
        sa_width
    } else {
        sa_width & !0x07
    };

    if oy + i32::from(sa_origin_y) < -pad_height {
        sa_origin_y = (-pad_height - oy) as i16;
        sa_height = (i32::from(sa_height) - (-pad_height - (oy + i32::from(sa_origin_y)))) as i16;
    }
    if oy + i32::from(sa_origin_y) > i32::from(ref_pic.height) - 1 {
        sa_origin_y = (i32::from(sa_origin_y)
            - ((oy + i32::from(sa_origin_y)) - (i32::from(ref_pic.height) - 1)))
            as i16;
    }
    if oy + i32::from(sa_origin_y) + i32::from(sa_height) > i32::from(ref_pic.height) {
        sa_height = i32::max(
            1,
            i32::from(sa_height)
                - ((oy + i32::from(sa_origin_y) + i32::from(sa_height))
                    - i32::from(ref_pic.height)),
        ) as i16;
    }

    let x_tl = ox + i32::from(sa_origin_x);
    let y_tl = oy + i32::from(sa_origin_y);
    let search_region_index = i64::from(x_tl) + i64::from(y_tl) * ref_pic.stride as i64;

    let full = me_ctx.hme_search_method == FULL_SAD_SEARCH;
    let r = sad_loop_kernel(
        src.b64,
        if full {
            src.b64_stride
        } else {
            src.b64_stride * 2
        },
        ref_pic.data,
        ref_pic.abs(search_region_index),
        if full {
            ref_pic.stride
        } else {
            ref_pic.stride * 2
        },
        if full {
            block_height as usize
        } else {
            (block_height >> 1) as usize
        },
        block_width as usize,
        ref_pic.stride,
        0,
        sa_width,
        sa_height,
    );

    let best_sad = if full { r.best_sad } else { r.best_sad * 2 };
    // Level 2 is already full resolution: no x4 / x2 rescale.
    let sc_x = r.x_search_center.wrapping_add(sa_origin_x);
    let sc_y = r.y_search_center.wrapping_add(sa_origin_y);
    (best_sad, sc_x, sc_y)
}

/// C `check_00_center` (motion_estimation.c:1060) — EXPORTED.
///
/// Clamps the HME search centre into the picture, then compares its SAD
/// against the zero-MV SAD and zeroes the centre when zero-MV wins (`<=`, via
/// `MIN` plus an equality test — a tie zeroes the centre).
#[allow(clippy::too_many_arguments)]
pub fn check_00_center(
    ref_pic: &Plane,
    me_ctx: &MeContext,
    src: &MeSrcBufs,
    sb_origin_x: u32,
    sb_origin_y: u32,
    sb_width: u32,
    sb_height: u32,
    x_search_center: &mut i16,
    y_search_center: &mut i16,
    zz_sad: u32,
) -> u32 {
    let org_x = sb_origin_x as i16;
    let org_y = sb_origin_y as i16;
    let subsample_sad = 1u32;
    let pad_width = BLOCK_SIZE_64 - 1;
    let pad_height = BLOCK_SIZE_64 - 1;

    let mut search_region_index = i64::from(org_x) + i64::from(org_y) * ref_pic.stride as i64;
    let mut zero_mv_sad: u64 = if me_ctx.me_early_exit_th != 0 {
        u64::from(zz_sad)
    } else {
        u64::from(nxm_sad_kernel(
            src.b64,
            src.b64_stride << subsample_sad,
            ref_pic.at(search_region_index),
            ref_pic.stride << subsample_sad,
            (sb_height >> subsample_sad) as usize,
            sb_width as usize,
        ))
    };
    zero_mv_sad <<= subsample_sad;

    let ox = i32::from(org_x);
    let oy = i32::from(org_y);
    if ox + i32::from(*x_search_center) < -pad_width {
        *x_search_center = (-pad_width - ox) as i16;
    }
    if ox + i32::from(*x_search_center) > i32::from(ref_pic.width) - 1 {
        *x_search_center = (i32::from(*x_search_center)
            - ((ox + i32::from(*x_search_center)) - (i32::from(ref_pic.width) - 1)))
            as i16;
    }
    if oy + i32::from(*y_search_center) < -pad_height {
        *y_search_center = (-pad_height - oy) as i16;
    }
    if oy + i32::from(*y_search_center) > i32::from(ref_pic.height) - 1 {
        *y_search_center = (i32::from(*y_search_center)
            - ((oy + i32::from(*y_search_center)) - (i32::from(ref_pic.height) - 1)))
            as i16;
    }

    let zero_mv_cost = zero_mv_sad << COST_PRECISION;
    search_region_index = i64::from(org_x.wrapping_add(*x_search_center))
        + i64::from(org_y.wrapping_add(*y_search_center)) * ref_pic.stride as i64;

    let mut hme_mv_sad = u64::from(nxm_sad_kernel(
        src.b64,
        src.b64_stride << subsample_sad,
        ref_pic.at(search_region_index),
        ref_pic.stride << subsample_sad,
        (sb_height >> subsample_sad) as usize,
        sb_width as usize,
    ));
    hme_mv_sad <<= subsample_sad;
    let hme_mv_cost = hme_mv_sad << COST_PRECISION;
    let search_center_cost = u64::min(zero_mv_cost, hme_mv_cost);

    if search_center_cost == zero_mv_cost {
        *x_search_center = 0;
        *y_search_center = 0;
    }
    hme_mv_sad as u32
}

/// C `get_zz_sad` (motion_estimation.c:1541).
pub fn get_zz_sad(
    ref_pic: &Plane,
    src: &MeSrcBufs,
    sb_origin_x: u32,
    sb_origin_y: u32,
    sb_width: u32,
    sb_height: u32,
) -> u32 {
    let org_x = sb_origin_x as i16;
    let org_y = sb_origin_y as i16;
    let subsample_sad = 1u32;
    let search_region_index = i64::from(org_x) + i64::from(org_y) * ref_pic.stride as i64;
    let zero_mv_sad = nxm_sad_kernel(
        src.b64,
        src.b64_stride << subsample_sad,
        ref_pic.at(search_region_index),
        ref_pic.stride << subsample_sad,
        (sb_height >> subsample_sad) as usize,
        sb_width as usize,
    );
    zero_mv_sad << subsample_sad
}

/// C `prehme_core` (motion_estimation.c:1458). Writes `sad` / `best_mv` /
/// `valid` back into `me_ctx.prehme_data[list][ref][sr]`.
///
/// MEASURED difference from `hme_level_*`, found by the parity test rather
/// than by reading: pre-HME does **not** round the search width up to a
/// multiple of 8 on the way in, and does **not** apply the `& ~7` round-DOWN
/// after the right-edge crop. So an odd `sa.width`, or a block near the right
/// edge, searches a different number of columns than the HME levels would.
/// Faithful; do not "harmonise" it.
#[allow(clippy::too_many_arguments)]
pub fn prehme_core(
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    org_x: i16,
    org_y: i16,
    sb_width: u32,
    sb_height: u32,
    sixteenth_ref: &Plane,
    list_i: usize,
    ref_i: usize,
    sr_i: usize,
) {
    let pad_width = i32::from(sixteenth_ref.border) - 1;
    let pad_height = i32::from(sixteenth_ref.border) - 1;

    let mut search_area_width = me_ctx.prehme_data[list_i][ref_i][sr_i].sa.width as i16;
    let mut search_area_height = me_ctx.prehme_data[list_i][ref_i][sr_i].sa.height as i16;

    let mut x_origin = -(search_area_width >> 1);
    let mut y_origin = -(search_area_height >> 1);

    let ox = i32::from(org_x);
    let oy = i32::from(org_y);
    // NOTE: C uses the ternary form here, so — unlike hme_level_*, which uses
    // sequential assignment — the width term re-reads the ALREADY-updated
    // origin and therefore also evaluates to zero. Same outcome, same code.
    let new_x = if ox + i32::from(x_origin) < -pad_width {
        (-pad_width - ox) as i16
    } else {
        x_origin
    };
    search_area_width = if ox + i32::from(new_x) < -pad_width {
        (i32::from(search_area_width) - (-pad_width - (ox + i32::from(new_x)))) as i16
    } else {
        search_area_width
    };
    x_origin = new_x;
    x_origin = if ox + i32::from(x_origin) > i32::from(sixteenth_ref.width) - 1 {
        (i32::from(x_origin) - ((ox + i32::from(x_origin)) - (i32::from(sixteenth_ref.width) - 1)))
            as i16
    } else {
        x_origin
    };
    search_area_width = if ox + i32::from(x_origin) + i32::from(search_area_width)
        > i32::from(sixteenth_ref.width)
    {
        i32::max(
            1,
            i32::from(search_area_width)
                - ((ox + i32::from(x_origin) + i32::from(search_area_width))
                    - i32::from(sixteenth_ref.width)),
        ) as i16
    } else {
        search_area_width
    };

    let new_y = if oy + i32::from(y_origin) < -pad_height {
        (-pad_height - oy) as i16
    } else {
        y_origin
    };
    search_area_height = if oy + i32::from(new_y) < -pad_height {
        (i32::from(search_area_height) - (-pad_height - (oy + i32::from(new_y)))) as i16
    } else {
        search_area_height
    };
    y_origin = new_y;
    y_origin = if oy + i32::from(y_origin) > i32::from(sixteenth_ref.height) - 1 {
        (i32::from(y_origin) - ((oy + i32::from(y_origin)) - (i32::from(sixteenth_ref.height) - 1)))
            as i16
    } else {
        y_origin
    };
    search_area_height = if oy + i32::from(y_origin) + i32::from(search_area_height)
        > i32::from(sixteenth_ref.height)
    {
        i32::max(
            1,
            i32::from(search_area_height)
                - ((oy + i32::from(y_origin) + i32::from(search_area_height))
                    - i32::from(sixteenth_ref.height)),
        ) as i16
    } else {
        search_area_height
    };

    let x_tl = ox + i32::from(x_origin);
    let y_tl = oy + i32::from(y_origin);
    let search_region_index = i64::from(x_tl) + i64::from(y_tl) * sixteenth_ref.stride as i64;

    let full = me_ctx.hme_search_method == FULL_SAD_SEARCH;
    let r = sad_loop_kernel(
        src.sixteenth,
        if full {
            src.sixteenth_stride
        } else {
            src.sixteenth_stride * 2
        },
        sixteenth_ref.data,
        sixteenth_ref.abs(search_region_index),
        if full {
            sixteenth_ref.stride
        } else {
            sixteenth_ref.stride * 2
        },
        if full {
            sb_height as usize
        } else {
            (sb_height >> 1) as usize
        },
        sb_width as usize,
        sixteenth_ref.stride,
        me_ctx.prehme_ctrl.skip_search_line,
        search_area_width,
        search_area_height,
    );

    let d = &mut me_ctx.prehme_data[list_i][ref_i][sr_i];
    d.sad = if full { r.best_sad } else { r.best_sad * 2 };
    d.best_mv.x = (r.x_search_center.wrapping_add(x_origin)).wrapping_mul(4);
    d.best_mv.y = (r.y_search_center.wrapping_add(y_origin)).wrapping_mul(4);
    d.valid = 1;
}

/// C `check_prehme_early_exit` (motion_estimation.c:1567).
pub fn check_prehme_early_exit(
    me_ctx: &mut MeContext,
    list_i: usize,
    ref_i: usize,
    sr_i: usize,
) -> bool {
    if me_ctx.me_early_exit_th != 0 && me_ctx.zz_sad[list_i][ref_i] < me_ctx.me_early_exit_th {
        let d = &mut me_ctx.prehme_data[list_i][ref_i][sr_i];
        d.best_mv = svtav1_types::motion::Mv::ZERO;
        d.sad = 0;
        d.valid = 1;
        return true;
    }
    if me_ctx.prehme_ctrl.l1_early_exit != 0
        && list_i == 1
        && me_ctx.prehme_data[0][ref_i][sr_i].valid != 0
        && (me_ctx.prehme_data[0][ref_i][sr_i].sad < (32 * 32)
            || (me_ctx.prehme_data[0][ref_i][sr_i].best_mv.x.abs() < 16
                && me_ctx.prehme_data[0][ref_i][sr_i].best_mv.y.abs() < 16))
    {
        let src_d = me_ctx.prehme_data[0][ref_i][sr_i];
        let d = &mut me_ctx.prehme_data[1][ref_i][sr_i];
        d.best_mv.x = -src_d.best_mv.x;
        d.best_mv.y = -src_d.best_mv.y;
        d.sad = src_d.sad;
        d.valid = 1;
        return true;
    }
    false
}

/// C `prehme_b64` (motion_estimation.c:1595).
pub fn prehme_b64(
    pic: &MePicParams,
    org_x: u32,
    org_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
) {
    let block_width = me_ctx.b64_width;
    let block_height = me_ctx.b64_height;
    let mut best_sad = u64::from(u32::MAX);

    for list_i in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_i] as usize;
        for ref_i in 0..num_refs {
            let r = refs.get(list_i, ref_i);
            let dist = get_me_reference_dist(pic.picture_number, r.picture_number);
            if me_ctx.temporal_layer_index > 0 || list_i == 0 {
                let hme_sr_factor = u32::from(get_scaled_picture_distance(dist));
                for sr_i in 0..SEARCH_REGION_COUNT {
                    if check_prehme_early_exit(me_ctx, list_i, ref_i, sr_i) {
                        continue;
                    }
                    if me_ctx.search_results[list_i][ref_i].do_ref == 0 {
                        let d = &mut me_ctx.prehme_data[list_i][ref_i][sr_i];
                        d.best_mv = svtav1_types::motion::Mv::ZERO;
                        d.sad = u64::from(u32::MAX);
                        continue;
                    }
                    let cfg = me_ctx.prehme_ctrl.prehme_sa_cfg[sr_i];
                    me_ctx.prehme_data[list_i][ref_i][sr_i].sa.width = u32::min(
                        u32::from(cfg.sa_min.width) * hme_sr_factor,
                        u32::from(cfg.sa_max.width),
                    ) as u16;
                    me_ctx.prehme_data[list_i][ref_i][sr_i].sa.height = u32::min(
                        u32::from(cfg.sa_min.height) * hme_sr_factor,
                        u32::from(cfg.sa_max.height),
                    )
                        as u16;
                    prehme_core(
                        me_ctx,
                        src,
                        (org_x as i16) >> 2,
                        (org_y as i16) >> 2,
                        block_width >> 2,
                        block_height >> 2,
                        &r.sixteenth,
                        list_i,
                        ref_i,
                        sr_i,
                    );
                    me_ctx.performed_phme[list_i][ref_i][sr_i] = 1;
                }
                let min_sad = u64::min(
                    me_ctx.prehme_data[list_i][ref_i][0].sad,
                    me_ctx.prehme_data[list_i][ref_i][1].sad,
                );
                best_sad = u64::min(best_sad, min_sad);
            } else {
                for sr_i in 0..SEARCH_REGION_COUNT {
                    let s = me_ctx.prehme_data[0][ref_i][sr_i];
                    let d = &mut me_ctx.prehme_data[1][ref_i][sr_i];
                    d.best_mv.x = -s.best_mv.x;
                    d.best_mv.y = -s.best_mv.y;
                    d.sad = s.sad;
                }
            }
        }
    }

    if !pic.frame_is_boosted && best_sad < u64::from(me_ctx.me_hme_prune_ctrls.phme_sad_th) {
        for list_i in 0..me_ctx.num_of_list_to_search as usize {
            for ref_i in 0..me_ctx.num_of_ref_pic_to_search[list_i] as usize {
                if me_ctx.search_results[list_i][ref_i].do_ref == 0 || ref_i == 0 {
                    continue;
                }
                let prhme_th = u64::from(me_ctx.me_hme_prune_ctrls.phme_sad_pct);
                let prehme_sad = u64::min(
                    me_ctx.prehme_data[list_i][ref_i][0].sad,
                    me_ctx.prehme_data[list_i][ref_i][1].sad,
                );
                // C computes this in uint32_t; the subtraction wraps when the
                // ref's SAD is below `best_sad` (it cannot be, since best_sad
                // is the min over the same set) — kept in u32 regardless.
                let lhs = (prehme_sad as u32)
                    .wrapping_sub(best_sad as u32)
                    .wrapping_mul(100);
                if u64::from(lhs) > prhme_th * best_sad {
                    me_ctx.search_results[list_i][ref_i].do_ref = 0;
                }
            }
        }
    }
}

/// C `get_hme_l0_search_area` (motion_estimation.c:1674). Mutates
/// `me_ctx.hme_l0_sa` — the caller (`hme_level0_b64`) saves and restores it.
pub fn get_hme_l0_search_area(
    me_ctx: &mut MeContext,
    list_index: usize,
    ref_pic_index: usize,
    dist: u16,
) -> (i16, i16) {
    if me_ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment != 0
        && me_ctx.me_sr_adjustment_ctrls.distance_based_hme_resizing != 0
    {
        let mut is_hor = true;
        let mut is_ver = true;
        let mut is_still = false;
        if me_ctx.reduce_hme_l0_sr_th_min != 0
            && me_ctx.reduce_hme_l0_sr_th_max != 0
            && (list_index != 0 || ref_pic_index != 0)
        {
            let l0_mvx = i32::from(me_ctx.x_hme_level0_search_center[0][0][0][0]);
            let l0_mvy = i32::from(me_ctx.y_hme_level0_search_center[0][0][0][0]);
            let th_min = i32::from(me_ctx.reduce_hme_l0_sr_th_min);
            let th_max = i32::from(me_ctx.reduce_hme_l0_sr_th_max);
            is_ver = l0_mvx.abs() < th_min && l0_mvy.abs() > th_max;
            is_hor = l0_mvx.abs() > th_max && l0_mvy.abs() < th_min;
            is_still = l0_mvx.abs() < th_min * 3 && l0_mvy.abs() < th_min * 3;
        }

        let mut x_offset = 1u32;
        let mut y_offset = 1u32;
        if !is_ver {
            y_offset = 2;
        }
        if !is_hor {
            x_offset = 2;
        }
        if me_ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment == 2 && is_still {
            x_offset = 4;
            y_offset = 4;
        }
        let rp = ref_pic_index as u32;
        me_ctx.hme_l0_sa.sa_min.width =
            (u32::from(me_ctx.hme_l0_sa.sa_min.width) / (x_offset + rp)) as u16;
        me_ctx.hme_l0_sa.sa_min.height =
            (u32::from(me_ctx.hme_l0_sa.sa_min.height) / (y_offset + rp)) as u16;
        me_ctx.hme_l0_sa.sa_max.width =
            (u32::from(me_ctx.hme_l0_sa.sa_max.width) / (x_offset + rp)) as u16;
        me_ctx.hme_l0_sa.sa_max.height =
            (u32::from(me_ctx.hme_l0_sa.sa_max.height) / (y_offset + rp)) as u16;
    }

    let hme_sr_factor = i32::from(get_scaled_picture_distance(dist));
    let nw = i32::from(me_ctx.num_hme_sa_w);
    let nh = i32::from(me_ctx.num_hme_sa_h);

    let sa_w0 = i32::from(me_ctx.hme_l0_sa.sa_min.width) / nw;
    let sa_width = i32::min(
        ((sa_w0 * hme_sr_factor) + 15) & !0x0F,
        ((i32::from(me_ctx.hme_l0_sa.sa_max.width) / nw) + 15) & !0x0F,
    ) as i16;
    let sa_h0 = i32::from(me_ctx.hme_l0_sa.sa_min.height) / nh;
    let sa_height = i32::min(
        sa_h0 * hme_sr_factor,
        i32::from(me_ctx.hme_l0_sa.sa_max.height) / nh,
    ) as i16;

    (sa_width, sa_height)
}

/// C `get_worst_quadrant` (motion_estimation.c:1737).
///
/// C asserts `num_hme_sa_{w,h} == 2` and returns without writing when that
/// fails; the port returns `None` so the caller can reproduce C's "leave the
/// caller's uninitialised `best_w`/`best_h`" only where C actually does.
pub fn get_worst_quadrant(
    me_ctx: &MeContext,
    list_index: usize,
    ref_pic_index: usize,
) -> Option<(usize, usize)> {
    if me_ctx.num_hme_sa_w != 2 || me_ctx.num_hme_sa_h != 2 {
        return None;
    }
    let sad = &me_ctx.hme_level0_sad[list_index][ref_pic_index];
    let mut max_sad = 0u64;
    let mut best = (0usize, 0usize);
    if sad[0][0] > max_sad {
        max_sad = sad[0][0];
        best = (0, 0);
    }
    if sad[1][0] > max_sad {
        max_sad = sad[1][0];
        best = (1, 0);
    }
    if sad[0][1] > max_sad {
        max_sad = sad[0][1];
        best = (0, 1);
    }
    // C's last comparison does not update max_sad (it is dead there too).
    if sad[1][1] > max_sad {
        best = (1, 1);
    }
    Some(best)
}

/// C `hme_level0_b64` (motion_estimation.c:1769).
pub fn hme_level0_b64(
    pic: &MePicParams,
    org_x: u32,
    org_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
) {
    let block_width = me_ctx.b64_width;
    let block_height = me_ctx.b64_height;
    let base_hme_sa = me_ctx.hme_l0_sa;

    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            if me_ctx.me_early_exit_th != 0
                && me_ctx.zz_sad[list_index][ref_pic_index] < (me_ctx.me_early_exit_th >> 2)
            {
                for sr_y in 0..me_ctx.num_hme_sa_h as usize {
                    for sr_x in 0..me_ctx.num_hme_sa_w as usize {
                        me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][sr_x][sr_y] =
                            0;
                        me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][sr_x][sr_y] =
                            0;
                        me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_x][sr_y] = 0;
                    }
                }
                continue;
            }
            if me_ctx.prev_me_stage_based_exit_th != 0 {
                let sr_i = usize::from(
                    me_ctx.prehme_data[list_index][ref_pic_index][0].sad
                        > me_ctx.prehme_data[list_index][ref_pic_index][1].sad,
                );
                if me_ctx.performed_phme[list_index][ref_pic_index][sr_i] != 0
                    && me_ctx.prehme_data[list_index][ref_pic_index][sr_i].sad
                        < u64::from(me_ctx.prev_me_stage_based_exit_th >> 4)
                {
                    let d = me_ctx.prehme_data[list_index][ref_pic_index][sr_i];
                    for sr_y in 0..me_ctx.num_hme_sa_h as usize {
                        for sr_x in 0..me_ctx.num_hme_sa_w as usize {
                            me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = d.best_mv.x;
                            me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = d.best_mv.y;
                            me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_x][sr_y] = d.sad;
                        }
                    }
                    continue;
                }
            }

            if me_ctx.search_results[list_index][ref_pic_index].do_ref == 0 {
                for sr_y in 0..me_ctx.num_hme_sa_h as usize {
                    for sr_x in 0..me_ctx.num_hme_sa_w as usize {
                        me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][sr_x][sr_y] =
                            0;
                        me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][sr_x][sr_y] =
                            0;
                        me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_x][sr_y] =
                            u64::from(u32::MAX);
                    }
                }
                continue;
            }

            let r = refs.get(list_index, ref_pic_index);
            let dist = get_me_reference_dist(pic.picture_number, r.picture_number);

            if me_ctx.temporal_layer_index > 0 || list_index == 0 {
                let (sa_width, sa_height) =
                    get_hme_l0_search_area(me_ctx, list_index, ref_pic_index, dist);
                for sr_h in 0..me_ctx.num_hme_sa_h as usize {
                    for sr_w in 0..me_ctx.num_hme_sa_w as usize {
                        let (sad, scx, scy) = hme_level_0(
                            me_ctx,
                            src,
                            (org_x as i16) >> 2,
                            (org_y as i16) >> 2,
                            block_width >> 2,
                            block_height >> 2,
                            sa_width,
                            sa_height,
                            &r.sixteenth,
                            sr_w as u32,
                            sr_h as u32,
                        );
                        me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_w][sr_h] = sad;
                        me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scx;
                        me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scy;
                    }
                }

                if me_ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment != 0
                    && me_ctx.me_sr_adjustment_ctrls.distance_based_hme_resizing != 0
                {
                    me_ctx.hme_l0_sa.sa_min = base_hme_sa.sa_min;
                    me_ctx.hme_l0_sa.sa_max = base_hme_sa.sa_max;
                }

                if me_ctx.prehme_ctrl.enable != 0
                    && let Some((sr_w_max, sr_h_max)) =
                        get_worst_quadrant(me_ctx, list_index, ref_pic_index)
                {
                    {
                        let sr_i = usize::from(
                            me_ctx.prehme_data[list_index][ref_pic_index][0].sad
                                > me_ctx.prehme_data[list_index][ref_pic_index][1].sad,
                        );
                        let d = me_ctx.prehme_data[list_index][ref_pic_index][sr_i];
                        if d.sad
                            < me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_w_max][sr_h_max]
                        {
                            me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_w_max][sr_h_max] =
                                d.sad;
                            me_ctx.x_hme_level0_search_center[list_index][ref_pic_index]
                                [sr_w_max][sr_h_max] = d.best_mv.x;
                            me_ctx.y_hme_level0_search_center[list_index][ref_pic_index]
                                [sr_w_max][sr_h_max] = d.best_mv.y;
                        }
                    }
                }
            }
        }
    }
}

/// C `hme_level1_b64` (motion_estimation.c:1895).
pub fn hme_level1_b64(
    pic: &MePicParams,
    org_x: u32,
    org_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
) {
    let block_width = me_ctx.b64_width;
    let block_height = me_ctx.b64_height;

    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            let r = refs.get(list_index, ref_pic_index);
            let _dist = get_me_reference_dist(pic.picture_number, r.picture_number);

            if me_ctx.temporal_layer_index > 0 || list_index == 0 {
                if me_ctx.me_early_exit_th != 0
                    && me_ctx.zz_sad[list_index][ref_pic_index] < (me_ctx.me_early_exit_th >> 2)
                {
                    for sr_y in 0..me_ctx.num_hme_sa_h as usize {
                        for sr_x in 0..me_ctx.num_hme_sa_w as usize {
                            me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = 0;
                            me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = 0;
                            me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_x][sr_y] = 0;
                        }
                    }
                    continue;
                }
                if me_ctx.search_results[list_index][ref_pic_index].do_ref == 0 {
                    for sr_y in 0..me_ctx.num_hme_sa_h as usize {
                        for sr_x in 0..me_ctx.num_hme_sa_w as usize {
                            me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = 0;
                            me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][sr_x]
                                [sr_y] = 0;
                            me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_x][sr_y] =
                                u64::from(u32::MAX);
                        }
                    }
                    continue;
                }
                for sr_h in 0..me_ctx.num_hme_sa_h as usize {
                    for sr_w in 0..me_ctx.num_hme_sa_w as usize {
                        if me_ctx.prev_me_stage_based_exit_th != 0
                            && me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_w][sr_h]
                                < u64::from(me_ctx.prev_me_stage_based_exit_th >> 5)
                        {
                            me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h] = me_ctx.x_hme_level0_search_center[list_index]
                                [ref_pic_index][sr_w][sr_h];
                            me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h] = me_ctx.y_hme_level0_search_center[list_index]
                                [ref_pic_index][sr_w][sr_h];
                            me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_w][sr_h] =
                                me_ctx.hme_level0_sad[list_index][ref_pic_index][sr_w][sr_h];
                            continue;
                        }
                        let (sad, scx, scy) = hme_level_1(
                            me_ctx,
                            src,
                            (org_x as i16) >> 1,
                            (org_y as i16) >> 1,
                            block_width >> 1,
                            block_height >> 1,
                            &r.quarter,
                            me_ctx.hme_l1_sa.width as i16,
                            me_ctx.hme_l1_sa.height as i16,
                            me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h]
                                >> 1,
                            me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h]
                                >> 1,
                        );
                        me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_w][sr_h] = sad;
                        me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scx;
                        me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scy;
                    }
                }
            }
        }
    }
}

/// C `hme_level2_b64` (motion_estimation.c:1972).
pub fn hme_level2_b64(
    pic: &MePicParams,
    org_x: u32,
    org_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
) {
    let block_width = me_ctx.b64_width;
    let block_height = me_ctx.b64_height;

    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            let r = refs.get(list_index, ref_pic_index);
            let _dist = get_me_reference_dist(pic.picture_number, r.picture_number);

            if me_ctx.temporal_layer_index > 0 || list_index == 0 {
                for sr_h in 0..me_ctx.num_hme_sa_h as usize {
                    for sr_w in 0..me_ctx.num_hme_sa_w as usize {
                        if me_ctx.prev_me_stage_based_exit_th != 0
                            && me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_w][sr_h]
                                < u64::from(me_ctx.prev_me_stage_based_exit_th >> 2)
                        {
                            me_ctx.x_hme_level2_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h] = me_ctx.x_hme_level1_search_center[list_index]
                                [ref_pic_index][sr_w][sr_h];
                            me_ctx.y_hme_level2_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h] = me_ctx.y_hme_level1_search_center[list_index]
                                [ref_pic_index][sr_w][sr_h];
                            me_ctx.hme_level2_sad[list_index][ref_pic_index][sr_w][sr_h] =
                                me_ctx.hme_level1_sad[list_index][ref_pic_index][sr_w][sr_h];
                            continue;
                        }
                        let (sad, scx, scy) = hme_level_2(
                            me_ctx,
                            src,
                            org_x as i16,
                            org_y as i16,
                            block_width,
                            block_height,
                            &r.picture,
                            me_ctx.hme_l2_sa.width as i16,
                            me_ctx.hme_l2_sa.height as i16,
                            me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h],
                            me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][sr_w]
                                [sr_h],
                        );
                        me_ctx.hme_level2_sad[list_index][ref_pic_index][sr_w][sr_h] = sad;
                        me_ctx.x_hme_level2_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scx;
                        me_ctx.y_hme_level2_search_center[list_index][ref_pic_index][sr_w][sr_h] =
                            scy;
                    }
                }
            }
        }
    }
}

/// C `set_final_search_centre_sb` (motion_estimation.c:2026) — EXPORTED.
///
/// Two C behaviours that look like bugs and are reproduced verbatim:
/// * `x_search_center` / `y_search_center` / `hmeMvSad` are declared ONCE
///   outside the list loop, so a reference whose `temporal_layer_index == 0 &&
///   list_index != 0` branch runs inherits the previous reference's `hmeMvSad`
///   (only x/y are reset to 0 there);
/// * the level-0 / level-1 quadrant scans start `search_region_number_in_width`
///   at 1 and reset it to 0 only after the first row, so quadrant (0, 0) of
///   later rows IS visited while (0, 0) of the first row is the seed.
pub fn set_final_search_centre_sb(me_ctx: &mut MeContext) {
    let mut hme_sc_x: i16 = 0;
    let mut hme_sc_y: i16 = 0;
    let mut x_search_center: i16 = 0;
    let mut y_search_center: i16 = 0;
    let mut hme_mv_sad: u64 = 0;

    let e0 = me_ctx.enable_hme_level0_flag;
    let e1 = me_ctx.enable_hme_level1_flag;
    let e2 = me_ctx.enable_hme_level2_flag;

    let mut best_cost = u64::MAX;
    me_ctx.best_list_idx = 0;
    me_ctx.best_ref_idx = 0;

    let nw = me_ctx.num_hme_sa_w as usize;
    let nh = me_ctx.num_hme_sa_h as usize;

    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
        for ref_pic_index in 0..num_refs {
            if me_ctx.temporal_layer_index > 0 || list_index == 0 {
                if me_ctx.enable_hme_flag {
                    if e0 && !e1 && !e2 {
                        hme_sc_x =
                            me_ctx.x_hme_level0_search_center[list_index][ref_pic_index][0][0];
                        hme_sc_y =
                            me_ctx.y_hme_level0_search_center[list_index][ref_pic_index][0][0];
                        hme_mv_sad = me_ctx.hme_level0_sad[list_index][ref_pic_index][0][0];
                        let mut w = 1usize;
                        let mut h = 0usize;
                        while h < nh {
                            while w < nw {
                                if me_ctx.hme_level0_sad[list_index][ref_pic_index][w][h]
                                    < hme_mv_sad
                                {
                                    hme_sc_x = me_ctx.x_hme_level0_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_sc_y = me_ctx.y_hme_level0_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_mv_sad =
                                        me_ctx.hme_level0_sad[list_index][ref_pic_index][w][h];
                                }
                                w += 1;
                            }
                            w = 0;
                            h += 1;
                        }
                    }
                    if e1 && !e2 {
                        hme_sc_x =
                            me_ctx.x_hme_level1_search_center[list_index][ref_pic_index][0][0];
                        hme_sc_y =
                            me_ctx.y_hme_level1_search_center[list_index][ref_pic_index][0][0];
                        hme_mv_sad = me_ctx.hme_level1_sad[list_index][ref_pic_index][0][0];
                        let mut w = 1usize;
                        let mut h = 0usize;
                        while h < nh {
                            while w < nw {
                                if me_ctx.hme_level1_sad[list_index][ref_pic_index][w][h]
                                    < hme_mv_sad
                                {
                                    hme_sc_x = me_ctx.x_hme_level1_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_sc_y = me_ctx.y_hme_level1_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_mv_sad =
                                        me_ctx.hme_level1_sad[list_index][ref_pic_index][w][h];
                                }
                                w += 1;
                            }
                            w = 0;
                            h += 1;
                        }
                    }
                    if e2 {
                        hme_sc_x =
                            me_ctx.x_hme_level2_search_center[list_index][ref_pic_index][0][0];
                        hme_sc_y =
                            me_ctx.y_hme_level2_search_center[list_index][ref_pic_index][0][0];
                        hme_mv_sad = me_ctx.hme_level2_sad[list_index][ref_pic_index][0][0];
                        let mut w = 1usize;
                        let mut h = 0usize;
                        while h < nh {
                            while w < nw {
                                if me_ctx.hme_level2_sad[list_index][ref_pic_index][w][h]
                                    < hme_mv_sad
                                {
                                    hme_sc_x = me_ctx.x_hme_level2_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_sc_y = me_ctx.y_hme_level2_search_center[list_index]
                                        [ref_pic_index][w][h];
                                    hme_mv_sad =
                                        me_ctx.hme_level2_sad[list_index][ref_pic_index][w][h];
                                }
                                w += 1;
                            }
                            w = 0;
                            h += 1;
                        }
                    }
                    x_search_center = hme_sc_x;
                    y_search_center = hme_sc_y;
                }
            } else {
                x_search_center = 0;
                y_search_center = 0;
            }

            me_ctx.search_results[list_index][ref_pic_index].hme_sc_x = x_search_center;
            me_ctx.search_results[list_index][ref_pic_index].hme_sc_y = y_search_center;
            me_ctx.search_results[list_index][ref_pic_index].hme_sad = hme_mv_sad;
            if hme_mv_sad < best_cost {
                best_cost = hme_mv_sad;
                me_ctx.best_list_idx = list_index as u8;
                me_ctx.best_ref_idx = ref_pic_index as u8;
            }
        }
    }
}

/// C `init_zz_sad` (motion_estimation.c:2193).
pub fn init_zz_sad(
    pic: &MePicParams,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
    org_x: u32,
    org_y: u32,
) {
    let block_width = me_ctx.b64_width;
    let block_height = me_ctx.b64_height;
    let mut best_zz_sad = u32::MAX;

    for list_i in 0..me_ctx.num_of_list_to_search as usize {
        for ref_i in 0..me_ctx.num_of_ref_pic_to_search[list_i] as usize {
            if me_ctx.temporal_layer_index > 0 || list_i == 0 {
                let r = refs.get(list_i, ref_i);
                let zz = get_zz_sad(&r.picture, src, org_x, org_y, block_width, block_height);
                let zz = ((u64::from(zz) * 64 * 64) / u64::from(block_width * block_height)) as u32;
                me_ctx.zz_sad[list_i][ref_i] = zz;
                best_zz_sad = u32::min(best_zz_sad, zz);
            }
        }
    }

    let zz_th = me_ctx.me_hme_prune_ctrls.zz_sad_th;
    if !pic.frame_is_boosted && best_zz_sad < zz_th {
        for list_i in 0..me_ctx.num_of_list_to_search as usize {
            for ref_i in 0..me_ctx.num_of_ref_pic_to_search[list_i] as usize {
                if ref_i == 0 {
                    continue;
                }
                let pct = me_ctx.me_hme_prune_ctrls.zz_sad_pct;
                let lhs = me_ctx.zz_sad[list_i][ref_i]
                    .wrapping_sub(best_zz_sad)
                    .wrapping_mul(100);
                if u64::from(lhs) > u64::from(pct) * u64::from(best_zz_sad) {
                    me_ctx.search_results[list_i][ref_i].do_ref = 0;
                }
            }
        }
    }

    let safe_limit_zz_th = me_ctx.me_safe_limit_zz_th;
    if safe_limit_zz_th != 0 {
        let me_safe_limit_refs = pic.hierarchical_levels > 0
            && me_ctx.num_of_list_to_search == 2
            && pic.frame_is_leaf
            && pic.similar_brightness_refs
            && me_ctx.zz_sad[0][0] < safe_limit_zz_th
            && me_ctx.zz_sad[1][0] < safe_limit_zz_th;
        for list_i in 0..me_ctx.num_of_list_to_search as usize {
            for ref_i in 0..me_ctx.num_of_ref_pic_to_search[list_i] as usize {
                if me_safe_limit_refs && ref_i > 0 {
                    me_ctx.search_results[list_i][ref_i].do_ref = 0;
                }
            }
        }
    }
}

/// C `hme_b64` (motion_estimation.c:2249).
pub fn hme_b64(
    pic: &MePicParams,
    org_x: u32,
    org_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
) {
    if me_ctx.me_early_exit_th != 0 || me_ctx.me_safe_limit_zz_th != 0 {
        init_zz_sad(pic, me_ctx, src, refs, org_x, org_y);
    }
    if me_ctx.prehme_ctrl.enable != 0 {
        prehme_b64(pic, org_x, org_y, me_ctx, src, refs);
    }
    if me_ctx.enable_hme_flag {
        if me_ctx.enable_hme_level0_flag {
            hme_level0_b64(pic, org_x, org_y, me_ctx, src, refs);
        }
        if me_ctx.enable_hme_level1_flag {
            hme_level1_b64(pic, org_x, org_y, me_ctx, src, refs);
        }
        if me_ctx.enable_hme_level2_flag {
            hme_level2_b64(pic, org_x, org_y, me_ctx, src, refs);
        }
    }
    set_final_search_centre_sb(me_ctx);

    if me_ctx.me_type == MeType::Mctf {
        if me_ctx.search_results[0][0].hme_sc_x.abs() > me_ctx.search_results[0][0].hme_sc_y.abs() {
            me_ctx.tf_tot_horz_blks += 1;
        } else {
            me_ctx.tf_tot_vert_blks += 1;
        }
    }
}

/// C `hme_prune_ref_and_adjust_sr` (motion_estimation.c:2290).
///
/// Both loops walk the FULL `[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH]`
/// rectangle, not `num_of_ref_pic_to_search` — including slots ME never
/// searched, whose `hme_sad` is `MAX_U32` from `init_me_hme_data`. Transcribed
/// as written.
pub fn hme_prune_ref_and_adjust_sr(me_ctx: &mut MeContext) {
    let prune_ref_th = me_ctx
        .me_hme_prune_ctrls
        .prune_ref_if_hme_sad_dev_bigger_than_th;
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
    if me_ctx.me_sr_adjustment_ctrls.enable_me_sr_adjustment != 0 {
        let mv_length_th = i32::from(
            me_ctx
                .me_sr_adjustment_ctrls
                .reduce_me_sr_based_on_mv_length_th,
        );
        let stationary_hme_sad_abs_th =
            u64::from(me_ctx.me_sr_adjustment_ctrls.stationary_hme_sad_abs_th);
        let reduce_th = u64::from(
            me_ctx
                .me_sr_adjustment_ctrls
                .reduce_me_sr_based_on_hme_sad_abs_th,
        );
        for li in 0..MAX_NUM_OF_REF_PIC_LIST {
            for ri in 0..REF_LIST_MAX_DEPTH {
                let sr = me_ctx.search_results[li][ri];
                if i32::from(sr.hme_sc_x).abs() <= mv_length_th
                    && i32::from(sr.hme_sc_y).abs() <= mv_length_th
                    && sr.hme_sad < stationary_hme_sad_abs_th
                {
                    me_ctx.reduce_me_sr_divisor[li][ri] =
                        u32::from(me_ctx.me_sr_adjustment_ctrls.stationary_me_sr_divisor);
                } else if sr.hme_sad < reduce_th {
                    me_ctx.reduce_me_sr_divisor[li][ri] =
                        u32::from(me_ctx.me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad);
                }
            }
        }
    }
}
