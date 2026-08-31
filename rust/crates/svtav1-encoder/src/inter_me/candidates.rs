//! ME candidate-array construction, global-motion detection and the per-block
//! distortion summary — the picture-facing tail of
//! `Source/Lib/Codec/motion_estimation.c`.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`construct_me_candidate_array_mrp_off`] | `construct_me_candidate_array_mrp_off` (:2335) | no |
//! | [`construct_me_candidate_array_single_ref`] | `construct_me_candidate_array_single_ref` (:2446) | no |
//! | [`construct_me_candidate_array`] | `construct_me_candidate_array` (:2499) | no |
//! | [`perform_gm_detection`] | `perform_gm_detection` (:2637) | no |
//! | [`compute_distortion`] | `compute_distortion` (:2739) | no |
//!
//! **Bitfield truncation is load-bearing.** C writes `ref0_list = 24` into a
//! one-bit field, which stores `0`. [`MeCandidate::set`] masks on write so the
//! port stores the same `0`, not `24`.

use super::context::*;
use super::sad::{mvxt, mvyt};
use super::tables::{ME_IDX_16X16_TO_PARENT_32X32, ME_IDX_85_8X8_TO_16X16, Z_TO_RASTER};
use svtav1_types::motion::Mv;

/// C's `number_of_pus` ladder, shared by the two MRP-off constructors.
fn number_of_pus(pic: &MePicParams) -> usize {
    if pic.enable_me_16x16 {
        if pic.enable_me_8x8 {
            pic.max_number_of_pus_per_sb as usize
        } else {
            MAX_SB64_PU_COUNT_NO_8X8
        }
    } else {
        MAX_SB64_PU_COUNT_WO_16X16
    }
}

/// C's per-`n_idx` `use_me_pu` predicate.
fn use_me_pu(pic: &MePicParams, n_idx: usize) -> bool {
    if pic.enable_me_16x16 {
        pic.enable_me_8x8 || n_idx < MAX_SB64_PU_COUNT_NO_8X8
    } else {
        n_idx < MAX_SB64_PU_COUNT_WO_16X16
    }
}

/// C `construct_me_candidate_array_mrp_off` (motion_estimation.c:2335) — the
/// one-reference-per-list path.
pub fn construct_me_candidate_array_mrp_off(
    pic: &MePicParams,
    me_ctx: &mut MeContext,
    num_of_list_to_search: u32,
    out: &mut MeB64Output,
) {
    debug_assert_eq!(me_ctx.num_of_ref_pic_to_search[0], 1);
    debug_assert_eq!(me_ctx.num_of_ref_pic_to_search[1], 1);
    let ref_pic_idx = 0usize;

    let mut blk_do_ref_org = [0u8; MAX_NUM_OF_REF_PIC_LIST];
    blk_do_ref_org[0] = me_ctx.search_results[0][0].do_ref;
    blk_do_ref_org[1] = if num_of_list_to_search == 1 { 0 } else { me_ctx.search_results[1][0].do_ref };

    let mut num_of_list_to_search = num_of_list_to_search;
    if num_of_list_to_search < 2 || me_ctx.search_results[1][0].do_ref == 0 {
        num_of_list_to_search = 1;
    }
    let me_prune_th = if blk_do_ref_org[0] != 0 && blk_do_ref_org[1] != 0 {
        me_ctx.prune_me_candidates_th as u32
    } else {
        0
    };

    let n_pus = number_of_pus(pic);
    out.total_me_candidate_index[..n_pus].fill(1);

    for n_idx in 0..pic.max_number_of_pus_per_sb as usize {
        let pu_index = Z_TO_RASTER[n_idx] as usize;
        let mut me_cand_offset = 0usize;
        let upu = use_me_pu(pic, n_idx);

        let mut blk_do_ref = blk_do_ref_org;
        let best_me_dist = if blk_do_ref_org[0] != 0 && blk_do_ref_org[1] != 0 {
            u32::min(
                me_ctx.p_sb_best_sad[0][ref_pic_idx][n_idx],
                me_ctx.p_sb_best_sad[1][ref_pic_idx][n_idx],
            )
        } else if blk_do_ref_org[0] != 0 {
            me_ctx.p_sb_best_sad[0][ref_pic_idx][n_idx]
        } else {
            me_ctx.p_sb_best_sad[1][ref_pic_idx][n_idx]
        };

        me_ctx.me_distortion[pu_index] = best_me_dist;
        let mut min_dist_list: i8 = -1;
        if me_ctx.use_best_unipred_cand_only != 0 && blk_do_ref[0] != 0 && blk_do_ref[1] != 0 {
            min_dist_list = i8::from(
                me_ctx.p_sb_best_sad[0][ref_pic_idx][n_idx] >= me_ctx.p_sb_best_sad[1][ref_pic_idx][n_idx],
            );
        }

        let mut list_index = 0usize;
        while (list_index as u32) < num_of_list_to_search && (upu || me_cand_offset == 0) {
            if blk_do_ref[list_index] == 0 {
                list_index += 1;
                continue;
            }
            if me_prune_th > 0 {
                let d = me_ctx.p_sb_best_sad[list_index][ref_pic_idx][n_idx]
                    .wrapping_sub(best_me_dist)
                    .wrapping_mul(100);
                if u64::from(d) > u64::from(best_me_dist) * u64::from(me_prune_th) {
                    blk_do_ref[list_index] = 0;
                    list_index += 1;
                    continue;
                }
            }
            if min_dist_list != -1 && min_dist_list != list_index as i8 {
                if upu {
                    let slot = pu_index * pic.max_refs + if list_index != 0 { pic.max_l0 } else { 0 } + ref_pic_idx;
                    out.me_mv_array[slot] =
                        Mv::from_int(me_ctx.p_sb_best_mv[list_index][ref_pic_idx][n_idx]);
                }
                list_index += 1;
                continue;
            }
            if upu {
                let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
                c.set(
                    list_index as u8,
                    ref_pic_idx as u8,
                    ref_pic_idx as u8,
                    if list_index == 0 { list_index as u8 } else { 24 },
                    if list_index == 1 { list_index as u8 } else { 24 },
                );
                let slot = pu_index * pic.max_refs + if list_index != 0 { pic.max_l0 } else { 0 } + ref_pic_idx;
                out.me_mv_array[slot] = Mv::from_int(me_ctx.p_sb_best_mv[list_index][ref_pic_idx][n_idx]);
            }
            me_cand_offset += 1;
            list_index += 1;
        }

        if blk_do_ref[0] != 0 && blk_do_ref[1] != 0 && upu {
            debug_assert_eq!(num_of_list_to_search, 2);
            let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
            c.set(BI_PRED, ref_pic_idx as u8, ref_pic_idx as u8, 0, 1);
            out.total_me_candidate_index[pu_index] = (me_cand_offset + 1) as u8;
        }
    }
}

/// C `construct_me_candidate_array_single_ref` (motion_estimation.c:2446).
pub fn construct_me_candidate_array_single_ref(
    pic: &MePicParams,
    me_ctx: &mut MeContext,
    _num_of_list_to_search: u32,
    out: &mut MeB64Output,
) {
    debug_assert_eq!(me_ctx.num_of_ref_pic_to_search[0], 1);
    debug_assert_eq!(me_ctx.num_of_ref_pic_to_search[1], 0);
    let ref_pic_idx = 0usize;
    let blk_do_ref = me_ctx.search_results[0][0].do_ref;

    let n_pus = number_of_pus(pic);
    out.total_me_candidate_index[..n_pus].fill(1);

    for n_idx in 0..pic.max_number_of_pus_per_sb as usize {
        let pu_index = Z_TO_RASTER[n_idx] as usize;
        let upu = use_me_pu(pic, n_idx);
        me_ctx.me_distortion[pu_index] = me_ctx.p_sb_best_sad[0][ref_pic_idx][n_idx];
        if blk_do_ref == 0 {
            continue;
        }
        if upu {
            let c = &mut out.me_candidate_array[pu_index * pic.max_cand];
            c.set(0, ref_pic_idx as u8, ref_pic_idx as u8, 0, 0);
            out.me_mv_array[pu_index * pic.max_refs + ref_pic_idx] =
                Mv::from_int(me_ctx.p_sb_best_mv[0][ref_pic_idx][n_idx]);
        }
    }
}

/// C `construct_me_candidate_array` (motion_estimation.c:2499) — the general
/// MRP path.
///
/// Note C's `pu_index = (n_idx > 4) ? z_to_raster[n_idx] : n_idx`, which
/// differs from the two specialised constructors (they always take
/// `z_to_raster[n_idx]`). `z_to_raster[0..=4] == {0,1,2,3,4}` so the two spell
/// the same map, but the asymmetry is transcribed rather than normalised.
pub fn construct_me_candidate_array(
    pic: &MePicParams,
    me_ctx: &mut MeContext,
    num_of_list_to_search: u32,
    out: &mut MeB64Output,
) {
    for n_idx in 0..pic.max_number_of_pus_per_sb as usize {
        let pu_index = if n_idx > 4 { Z_TO_RASTER[n_idx] as usize } else { n_idx };
        let mut me_cand_offset = 0usize;
        let upu = use_me_pu(pic, n_idx);

        let mut blk_do_ref = [[0u8; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST];
        let me_prune_th = me_ctx.prune_me_candidates_th as u32;
        let mut best_me_dist = u32::MAX;

        for list_index in 0..num_of_list_to_search as usize {
            let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
            for ref_pic in 0..num_refs {
                blk_do_ref[list_index][ref_pic] = me_ctx.search_results[list_index][ref_pic].do_ref;
                if blk_do_ref[list_index][ref_pic] == 0 {
                    continue;
                }
                if me_ctx.p_sb_best_sad[list_index][ref_pic][n_idx] < best_me_dist {
                    best_me_dist = me_ctx.p_sb_best_sad[list_index][ref_pic][n_idx];
                }
            }
        }

        me_ctx.me_distortion[pu_index] = best_me_dist;

        let mut list_index = 0usize;
        while (list_index as u32) < num_of_list_to_search && (upu || me_cand_offset == 0) {
            let num_refs = me_ctx.num_of_ref_pic_to_search[list_index] as usize;
            let mut ref_pic_index = 0usize;
            while ref_pic_index < num_refs && (upu || me_cand_offset == 0) {
                if blk_do_ref[list_index][ref_pic_index] == 0 {
                    ref_pic_index += 1;
                    continue;
                }
                if me_prune_th > 0 {
                    let d = me_ctx.p_sb_best_sad[list_index][ref_pic_index][n_idx]
                        .wrapping_sub(best_me_dist)
                        .wrapping_mul(100);
                    if u64::from(d) > u64::from(best_me_dist) * u64::from(me_prune_th) {
                        blk_do_ref[list_index][ref_pic_index] = 0;
                        ref_pic_index += 1;
                        continue;
                    }
                }
                if upu {
                    let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
                    c.set(
                        list_index as u8,
                        ref_pic_index as u8,
                        ref_pic_index as u8,
                        if list_index == 0 { list_index as u8 } else { 24 },
                        if list_index == 1 { list_index as u8 } else { 24 },
                    );
                    let slot =
                        pu_index * pic.max_refs + if list_index != 0 { pic.max_l0 } else { 0 } + ref_pic_index;
                    out.me_mv_array[slot] = Mv::from_int(me_ctx.p_sb_best_mv[list_index][ref_pic_index][n_idx]);
                }
                me_cand_offset += 1;
                ref_pic_index += 1;
            }
            list_index += 1;
        }

        if num_of_list_to_search == 2 && upu {
            // 1st set: (L0[i], L1[j]) for every allowed pair.
            for f in 0..me_ctx.num_of_ref_pic_to_search[0] as usize {
                for s in 0..me_ctx.num_of_ref_pic_to_search[1] as usize {
                    if pic.only_l_bwd && (f > 0 || s > 0) {
                        continue;
                    }
                    if blk_do_ref[0][f] != 0 && blk_do_ref[1][s] != 0 {
                        let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
                        c.set(BI_PRED, f as u8, s as u8, 0, 1);
                        me_cand_offset += 1;
                    }
                }
            }
            if !pic.only_l_bwd {
                // 2nd set: (L0[0], L0[i]) for i >= 1.
                for f in 1..me_ctx.num_of_ref_pic_to_search[0] as usize {
                    if blk_do_ref[0][0] != 0 && blk_do_ref[0][f] != 0 {
                        let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
                        c.set(BI_PRED, 0, f as u8, 0, 0);
                        me_cand_offset += 1;
                    }
                }
                // 3rd set: (L1[0], L1[2]).
                if me_ctx.num_of_ref_pic_to_search[1] == 3 && blk_do_ref[1][0] != 0 && blk_do_ref[1][2] != 0 {
                    let c = &mut out.me_candidate_array[pu_index * pic.max_cand + me_cand_offset];
                    c.set(BI_PRED, 0, 2, 1, 1);
                    me_cand_offset += 1;
                }
            }
        }

        if upu {
            out.total_me_candidate_index[pu_index] = me_cand_offset as u8;
        }
    }
}

/// C `perform_gm_detection` (motion_estimation.c:2637). Sets
/// `out.rc_me_allow_gm` when either MV component of a majority of blocks
/// exceeds the resolution-dependent activity threshold.
pub fn perform_gm_detection(pic: &MePicParams, me_ctx: &MeContext, out: &mut MeB64Output) {
    let mut per_sig_cnt =
        [[[[0u64; NUM_MV_HIST]; NUM_MV_COMPONENTS]; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST];
    let mut tot_cnt = 0u64;

    let (count, first, active_th) = if pic.input_resolution <= INPUT_SIZE_480P_RANGE {
        (64usize, 21usize, 4i32)
    } else {
        (16usize, 5usize, 32i32)
    };

    for i in 0..count {
        let mut n_idx = (first + i) as u8;
        if pic.input_resolution <= INPUT_SIZE_480P_RANGE {
            if !pic.enable_me_8x8 {
                if n_idx as usize >= MAX_SB64_PU_COUNT_NO_8X8 {
                    n_idx = ME_IDX_85_8X8_TO_16X16[n_idx as usize - MAX_SB64_PU_COUNT_NO_8X8];
                }
                if !pic.enable_me_16x16 && n_idx as usize >= MAX_SB64_PU_COUNT_WO_16X16 {
                    n_idx = ME_IDX_16X16_TO_PARENT_32X32[n_idx as usize - MAX_SB64_PU_COUNT_WO_16X16];
                }
            }
        } else if !pic.enable_me_16x16 && n_idx as usize >= MAX_SB64_PU_COUNT_WO_16X16 {
            n_idx = ME_IDX_16X16_TO_PARENT_32X32[n_idx as usize - MAX_SB64_PU_COUNT_WO_16X16];
        }

        let cand = out.me_candidate_array[n_idx as usize * pic.max_cand];
        let list_index = usize::from(if cand.direction == 0 || cand.direction == 2 {
            cand.ref0_list
        } else {
            cand.ref1_list
        });
        let ref_pic_index = usize::from(if cand.direction == 0 || cand.direction == 2 {
            cand.ref_idx_l0
        } else {
            cand.ref_idx_l1
        });

        let packed = me_ctx.p_sb_best_mv[list_index][ref_pic_index][n_idx as usize];
        let mx = i32::from(mvxt(packed)) * 4;
        if mx < -active_th {
            per_sig_cnt[list_index][ref_pic_index][0][0] += 1;
        } else if mx > active_th {
            per_sig_cnt[list_index][ref_pic_index][0][1] += 1;
        }
        let my = i32::from(mvyt(packed)) * 4;
        if my < -active_th {
            per_sig_cnt[list_index][ref_pic_index][1][0] += 1;
        } else if my > active_th {
            per_sig_cnt[list_index][ref_pic_index][1][1] += 1;
        }
        tot_cnt += 1;
    }

    for l in 0..MAX_NUM_OF_REF_PIC_LIST {
        for r in 0..REF_LIST_MAX_DEPTH {
            for c in 0..NUM_MV_COMPONENTS {
                for s in 0..NUM_MV_HIST {
                    if per_sig_cnt[l][r][c][s] > tot_cnt / 2 {
                        out.rc_me_allow_gm = 1;
                        break;
                    }
                }
            }
        }
    }
}

/// C `compute_distortion` (motion_estimation.c:2739).
pub fn compute_distortion(pic: &MePicParams, me_ctx: &MeContext, out: &mut MeB64Output) {
    let b64_size = 64u64 * 64;
    let dist_64x64 = u64::from(me_ctx.me_distortion[0]);
    let mut dist_32x32 = 0u64;
    for i in 0..4 {
        dist_32x32 += u64::from(me_ctx.me_distortion[1 + i]);
    }
    let mut dist_16x16 = 0u64;
    for i in 0..16 {
        dist_16x16 += u64::from(me_ctx.me_distortion[5 + i]);
    }
    let mut dist_8x8 = 0u64;
    for i in 0..64 {
        dist_8x8 += u64::from(me_ctx.me_distortion[21 + i]);
    }

    // C accumulates dist_* in uint32_t; the sums are re-narrowed here so an
    // overflowing SB wraps exactly as C's does.
    let dist_32x32 = dist_32x32 as u32;
    let dist_16x16 = dist_16x16 as u32;
    let dist_8x8 = dist_8x8 as u32;
    let dist_64x64 = dist_64x64 as u32;

    let mean_dist_8x8 = u64::from(dist_8x8) / 64;
    let mut sum_ofsq_dist_8x8 = 0u64;
    for i in 0..64 {
        let diff = i64::from(me_ctx.me_distortion[21 + i]) - mean_dist_8x8 as i64;
        sum_ofsq_dist_8x8 = sum_ofsq_dist_8x8.wrapping_add((diff * diff) as u64);
    }

    out.me_8x8_cost_variance = (sum_ofsq_dist_8x8 / 64) as u32;
    out.rc_me_distortion = if pic.input_resolution <= INPUT_SIZE_480P_RANGE { dist_8x8 } else { dist_16x16 };
    let pix_num = u64::from(pic.b64_geom_width * pic.b64_geom_height);
    out.me_64x64_distortion = ((u64::from(dist_64x64) * b64_size) / pix_num) as u32;
    out.me_32x32_distortion = ((u64::from(dist_32x32) * b64_size) / pix_num) as u32;
    out.me_16x16_distortion = ((u64::from(dist_16x16) * b64_size) / pix_num) as u32;
    out.me_8x8_distortion = ((u64::from(dist_8x8) * b64_size) / pix_num) as u32;
}
