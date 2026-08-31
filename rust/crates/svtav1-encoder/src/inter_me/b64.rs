//! The per-64x64 ME entry point — `svt_aom_motion_estimation_b64` and the two
//! helpers immediately above it in `Source/Lib/Codec/motion_estimation.c`.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`init_me_hme_data`] | `init_me_hme_data` (:2788) | no (`static INLINE`) |
//! | [`me_static_b64_bypass`] | `me_static_b64_bypass` (:2832) | no (`static`) |
//! | [`motion_estimation_b64`] | `svt_aom_motion_estimation_b64` (:2889) | **yes** |

use super::candidates::{
    compute_distortion, construct_me_candidate_array, construct_me_candidate_array_mrp_off,
    construct_me_candidate_array_single_ref, perform_gm_detection,
};
use super::context::*;
use super::hme::{get_zz_sad, hme_b64};
use super::integer::{integer_search_b64, me_prune_ref};

/// C `init_me_hme_data` (motion_estimation.c:2788).
///
/// The `p_sb_best_mv` wipe is the "R2R FIX" guard: without it a b64 that takes
/// the static bypass inherits a previous b64's MV in the reference slots it
/// never fills, which is an out-of-bounds fetch in inter prediction.
pub fn init_me_hme_data(me_ctx: &mut MeContext) {
    if me_ctx.enable_hme_flag {
        me_ctx.x_hme_level0_search_center = Default::default();
        me_ctx.y_hme_level0_search_center = Default::default();
        me_ctx.x_hme_level1_search_center = Default::default();
        me_ctx.y_hme_level1_search_center = Default::default();
        me_ctx.x_hme_level2_search_center = Default::default();
        me_ctx.y_hme_level2_search_center = Default::default();
    }

    me_ctx.p_sb_best_mv = [[[0; SQUARE_PU_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST];

    for li in 0..MAX_NUM_OF_REF_PIC_LIST {
        for ri in 0..REF_LIST_MAX_DEPTH {
            if me_ctx.me_type != MeType::Mctf {
                me_ctx.search_results[li][ri].list_i = li as u8;
            }
            me_ctx.search_results[li][ri].ref_i = ri as u8;
            me_ctx.search_results[li][ri].do_ref = 1;
            me_ctx.search_results[li][ri].hme_sad = u64::from(u32::MAX);
            me_ctx.reduce_me_sr_divisor[li][ri] = 1;
            me_ctx.zz_sad[li][ri] = u32::MAX;
            me_ctx.prehme_data[li][ri][0].valid = 0;
            me_ctx.prehme_data[li][ri][1].valid = 0;
        }
    }
    me_ctx.performed_phme = [[[0; 2]; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST];
}

/// C `me_static_b64_bypass` (motion_estimation.c:2832): if the list0/ref0
/// zero-motion SAD is below `me_static_b64_th`, publish a zero-MV result for
/// list0/ref0, switch every farther reference off, and tell the caller to skip
/// HME + integer ME.
pub fn me_static_b64_bypass(
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
    b64_origin_x: u32,
    b64_origin_y: u32,
) -> bool {
    if me_ctx.me_static_b64_th == 0 {
        return false;
    }

    let l0r0_raw = get_zz_sad(
        &refs.get(0, 0).picture,
        src,
        b64_origin_x,
        b64_origin_y,
        me_ctx.b64_width,
        me_ctx.b64_height,
    );
    if u64::from(l0r0_raw) * 64 * 64
        >= u64::from(me_ctx.me_static_b64_th) * u64::from(me_ctx.b64_width) * u64::from(me_ctx.b64_height)
    {
        return false;
    }

    let zz_sad = (u64::from(l0r0_raw) * 64 * 64 / u64::from(me_ctx.b64_width * me_ctx.b64_height)) as u32;
    me_ctx.zz_sad[0][0] = zz_sad;
    me_ctx.search_results[0][0].do_ref = 1;
    me_ctx.p_sb_best_sad[0][0][PU_64X64] = zz_sad;
    let sad32 = zz_sad >> 2;
    for i in PU_32X32_0..PU_16X16_0 {
        me_ctx.p_sb_best_sad[0][0][i] = sad32;
    }
    let sad16 = zz_sad >> 4;
    for i in PU_16X16_0..PU_8X8_0 {
        me_ctx.p_sb_best_sad[0][0][i] = sad16;
    }
    let sad8 = zz_sad >> 6;
    for i in PU_8X8_0..SQUARE_PU_COUNT {
        me_ctx.p_sb_best_sad[0][0][i] = sad8;
    }
    for list_index in 0..me_ctx.num_of_list_to_search as usize {
        for ref_idx in 0..me_ctx.num_of_ref_pic_to_search[list_index] as usize {
            if list_index != 0 || ref_idx != 0 {
                me_ctx.search_results[list_index][ref_idx].do_ref = 0;
            }
        }
    }
    true
}

/// C `svt_aom_motion_estimation_b64` (motion_estimation.c:2889) — EXPORTED.
///
/// `pic.aligned_width` / `pic.aligned_height` are the picture-level values
/// `integer_search_b64` uses; the b64 extent below is derived from
/// `input_width`/`input_height` rounded up to 8, exactly as C does with
/// `ALIGN_POWER_OF_TWO(input_ptr->width, 3)`.
pub fn motion_estimation_b64(
    pic: &MePicParams,
    b64_origin_x: u32,
    b64_origin_y: u32,
    me_ctx: &mut MeContext,
    src: &MeSrcBufs,
    refs: &MeRefs,
    out: &mut MeB64Output,
) {
    let num_of_list_to_search = u32::from(me_ctx.num_of_list_to_search);

    let aligned_width = u32::from(pic.input_width).div_ceil(8) * 8;
    let aligned_height = u32::from(pic.input_height).div_ceil(8) * 8;
    me_ctx.b64_width = if aligned_width - b64_origin_x < BLOCK_SIZE_64 as u32 {
        aligned_width - b64_origin_x
    } else {
        BLOCK_SIZE_64 as u32
    };
    me_ctx.b64_height = if aligned_height - b64_origin_y < BLOCK_SIZE_64 as u32 {
        aligned_height - b64_origin_y
    } else {
        BLOCK_SIZE_64 as u32
    };

    let prune_ref = me_ctx.enable_hme_flag && me_ctx.me_type != MeType::Mctf;

    init_me_hme_data(me_ctx);
    if !me_static_b64_bypass(me_ctx, src, refs, b64_origin_x, b64_origin_y) {
        hme_b64(pic, b64_origin_x, b64_origin_y, me_ctx, src, refs);

        if me_ctx.me_type == MeType::Mctf
            && me_ctx.search_results[0][0].hme_sad < u64::from(me_ctx.tf_me_exit_th)
        {
            me_ctx.tf_use_pred_64x64_only_th = u8::MAX;
            return;
        }
        if prune_ref {
            super::hme::hme_prune_ref_and_adjust_sr(me_ctx);
        }
        integer_search_b64(pic, me_ctx, src, refs, b64_origin_x, b64_origin_y);
        if prune_ref && me_ctx.me_hme_prune_ctrls.enable_me_hme_ref_pruning {
            me_prune_ref(me_ctx);
        }
    }

    if me_ctx.me_type != MeType::Mctf {
        if me_ctx.num_of_ref_pic_to_search[0] == 1 && me_ctx.num_of_ref_pic_to_search[1] == 0 {
            construct_me_candidate_array_single_ref(pic, me_ctx, num_of_list_to_search, out);
        } else if me_ctx.num_of_ref_pic_to_search[0] == 1 && me_ctx.num_of_ref_pic_to_search[1] == 1 {
            construct_me_candidate_array_mrp_off(pic, me_ctx, num_of_list_to_search, out);
        } else {
            construct_me_candidate_array(pic, me_ctx, num_of_list_to_search, out);
        }
        compute_distortion(pic, me_ctx, out);
        out.rc_me_allow_gm = 0;
        if pic.gm_enabled {
            perform_gm_detection(pic, me_ctx, out);
        }
    }
}
