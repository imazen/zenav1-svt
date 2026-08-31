/*
 * C shims for the MODE-DECISION wholesale port (lane wp-modedecision:
 * Codec/mode_decision.c + Codec/product_coding_loop.c).
 *
 * Kept in its OWN translation unit so this lane never shares an editable
 * file with the concurrent inter-MVP / inter-ME lanes.
 *
 * Every function here drives a REAL exported SVT-AV1 symbol (evidence
 * tier 1, docs/WORKING-ON-THIS.md section 4). Linkage was re-checked with
 * `nm -g Bin/Release/libSvtAv1Enc.a` rather than inferred from the header
 * or the `svt_aom_` prefix (mode_decision.c has BOTH traps: a prefixed
 * `static` and an unprefixed export).
 *
 *   svt_get_ref_frame_type            mode_decision.c:265
 *   svt_aom_get_max_drl_index         mode_decision.c:269
 *   svt_is_interintra_allowed         mode_decision.c:96
 *   svt_aom_is_valid_unipred_ref      mode_decision.c:762
 *   svt_aom_is_me_data_present        mode_decision.c:179
 *   svt_aom_obmc_motion_mode_allowed  mode_decision.c:214
 *   svt_aom_get_wedge_params_bits     inter_prediction.c:2053
 *   svt_aom_get_me_block_offset       mode_decision.c:117
 *
 * State discipline: every shim keeps its scratch on the STACK or in a
 * per-call calloc. cargo/nextest runs a test binary's tests on several
 * threads and a file-scope buffer would race.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "md_process.h"
#include "mode_decision.h"
#include "inter_prediction.h"
#include "adaptive_mv_pred.h"
#include "pcs.h"
#include "me_sb_results.h"

/* ------------------------------------------------------------------ *
 * Pure table / arithmetic helpers.
 * ------------------------------------------------------------------ */

int32_t ref_md_get_ref_frame_type(int32_t list, int32_t ref_idx) {
    return (int32_t)svt_get_ref_frame_type((uint8_t)list, (uint8_t)ref_idx);
}

int32_t ref_md_get_max_drl_index(int32_t refmv_cnt, int32_t mode) {
    return (int32_t)svt_aom_get_max_drl_index((uint8_t)refmv_cnt, (PredictionMode)mode);
}

int32_t ref_md_is_interintra_allowed(int32_t enable_inter_intra, int32_t bsize, int32_t mode,
                                     int32_t rf0, int32_t rf1) {
    MvReferenceFrame rf[2];
    rf[0] = (MvReferenceFrame)rf0;
    rf[1] = (MvReferenceFrame)rf1;
    return svt_is_interintra_allowed(
        (uint8_t)enable_inter_intra, (BlockSize)bsize, (PredictionMode)mode, rf);
}

int32_t ref_md_get_wedge_params_bits(int32_t bsize) {
    return svt_aom_get_wedge_params_bits((BlockSize)bsize);
}

int32_t ref_md_get_me_block_offset(int32_t org_x, int32_t org_y, int32_t bsize,
                                   int32_t enable_me_8x8, int32_t enable_me_16x16) {
    return (int32_t)svt_aom_get_me_block_offset((uint32_t)org_x,
                                                (uint32_t)org_y,
                                                (BlockSize)bsize,
                                                (uint8_t)enable_me_8x8,
                                                (uint8_t)enable_me_16x16);
}

/* svt_aom_is_valid_unipred_ref is EXPORTED (nm -g) but has no prototype
 * in any header — declare it here rather than let C99 implicit-declare it. */
bool svt_aom_is_valid_unipred_ref(ModeDecisionContext* ctx, uint8_t inter_cand_group,
                                  uint8_t list_idx, uint8_t ref_idx);

/* ------------------------------------------------------------------ *
 * Reference pruning gate (mode_decision.c:762).
 *
 * `do_ref_flat` is TOT_INTER_GROUP * MAX_NUM_OF_REF_PIC_LIST *
 * REF_LIST_MAX_DEPTH bytes in C's index order.
 * ------------------------------------------------------------------ */

int32_t ref_md_is_valid_unipred_ref(int32_t pruning_enabled, const uint8_t* do_ref_flat,
                                    const uint8_t* closest_refs, int32_t inter_cand_group,
                                    int32_t list_idx, int32_t ref_idx) {
    ModeDecisionContext* ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    ctx->ref_pruning_ctrls.enabled = (uint8_t)pruning_enabled;
    for (int g = 0; g < TOT_INTER_GROUP; g++) {
        ctx->ref_pruning_ctrls.closest_refs[g] = closest_refs[g];
        for (int l = 0; l < MAX_NUM_OF_REF_PIC_LIST; l++) {
            for (int r = 0; r < REF_LIST_MAX_DEPTH; r++) {
                ctx->ref_filtering_res[g][l][r].do_ref =
                    do_ref_flat[(g * MAX_NUM_OF_REF_PIC_LIST + l) * REF_LIST_MAX_DEPTH + r];
            }
        }
    }
    const int32_t out = svt_aom_is_valid_unipred_ref(
        ctx, (uint8_t)inter_cand_group, (uint8_t)list_idx, (uint8_t)ref_idx) ? 1 : 0;
    free(ctx);
    return out;
}

/* ------------------------------------------------------------------ *
 * ME-data presence (mode_decision.c:179).
 *
 * `cands` arrives packed 5 i32s per MeCandidate:
 *   direction, ref_idx_l0, ref_idx_l1, ref0_list, ref1_list.
 * ------------------------------------------------------------------ */

int32_t ref_md_is_me_data_present(int32_t me_block_offset, int32_t me_cand_offset,
                                  const uint8_t* total_me_candidate_index, int32_t n_blocks,
                                  const int32_t* cands, int32_t n_cands, int32_t list_idx,
                                  int32_t ref_idx) {
    MeSbResults res;
    memset(&res, 0, sizeof(res));
    uint8_t*     totals = (uint8_t*)calloc((size_t)(n_blocks > 0 ? n_blocks : 1), 1);
    MeCandidate* arr    = (MeCandidate*)calloc((size_t)(n_cands > 0 ? n_cands : 1), sizeof(MeCandidate));
    for (int32_t i = 0; i < n_blocks; i++) { totals[i] = total_me_candidate_index[i]; }
    for (int32_t i = 0; i < n_cands; i++) {
        const int32_t* c  = cands + (size_t)i * 5;
        arr[i].direction  = (uint8_t)c[0] & 0x3;
        arr[i].ref_idx_l0 = (uint8_t)c[1] & 0x3;
        arr[i].ref_idx_l1 = (uint8_t)c[2] & 0x3;
        arr[i].ref0_list  = (uint8_t)c[3] & 0x1;
        arr[i].ref1_list  = (uint8_t)c[4] & 0x1;
    }
    res.total_me_candidate_index = totals;
    res.me_candidate_array       = arr;
    const int32_t out            = (int32_t)svt_aom_is_me_data_present(
        (uint32_t)me_block_offset, (uint32_t)me_cand_offset, &res, (uint8_t)list_idx, (uint8_t)ref_idx);
    free(totals);
    free(arr);
    return out;
}

/* ------------------------------------------------------------------ *
 * Motion-mode gate (mode_decision.c:214).
 *
 * CONFIG_ENABLE_OBMC is 1 in this build (EbConfigMacros.h:82; RTC_BUILD
 * is 0), so the `#if CONFIG_ENABLE_OBMC` half of mode_decision.c is live.
 * ------------------------------------------------------------------ */

int32_t ref_md_obmc_motion_mode_allowed(int32_t trans_face_off, int32_t obmc_enabled,
                                        int32_t obmc_max_blk_size, int32_t situation,
                                        int32_t is_motion_mode_switchable, int32_t force_integer_mv,
                                        const int32_t* gm_wmtype /*[TOTAL_REFS_PER_FRAME]*/,
                                        int32_t overlappable_neighbors, int32_t bsize, int32_t rf0,
                                        int32_t rf1, int32_t mode) {
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(*blk));
    pcs->ppcs                     = ppcs;
    ppcs->frm_hdr.is_motion_mode_switchable = (uint8_t)is_motion_mode_switchable;
    ppcs->frm_hdr.force_integer_mv          = (uint8_t)force_integer_mv;
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; i++) {
        ppcs->global_motion[i].wmtype = (TransformationType)gm_wmtype[i];
    }
    ctx->obmc_ctrls.trans_face_off = (uint8_t)trans_face_off;
    ctx->obmc_ctrls.enabled        = (uint8_t)obmc_enabled;
    ctx->obmc_ctrls.max_blk_size   = (uint8_t)obmc_max_blk_size;
    blk->overlappable_neighbors    = (uint32_t)overlappable_neighbors;
    ctx->blk_ptr                   = blk;

    const int32_t out = (int32_t)svt_aom_obmc_motion_mode_allowed(pcs,
                                                                  ctx,
                                                                  (BlockSize)bsize,
                                                                  (uint8_t)situation,
                                                                  (MvReferenceFrame)rf0,
                                                                  (MvReferenceFrame)rf1,
                                                                  (PredictionMode)mode);
    free(blk);
    free(ctx);
    free(ppcs);
    free(pcs);
    return out;
}
