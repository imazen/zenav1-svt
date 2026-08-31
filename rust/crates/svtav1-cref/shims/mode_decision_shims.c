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
/* For EbCpuFlags + the RTCD setup entry points (see md_ensure_rtcd below). */
#include "aom_dsp_rtcd.h"

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

/* ------------------------------------------------------------------ *
 * PME SAD kernel + the MD motion-search cost model.
 *
 *   svt_pme_sad_loop_kernel_c   product_coding_loop.c:1775 (EXPORTED)
 *   svt_aom_fp_mv_err_cost      mcomp.c:775                (EXPORTED)
 *   svt_aom_get_sad_per_bit     mode_decision.c:2048       (EXPORTED)
 *   svt_av1_init_me_luts        mode_decision.c:2063       (EXPORTED)
 *
 * The `_c` suffix is deliberate: it is the reference scalar kernel, which
 * is what the port transcribes. Driving the RTCD pointer instead would
 * compare against whichever SIMD variant this host dispatches to.
 * ------------------------------------------------------------------ */
#include "mcomp.h"

void svt_pme_sad_loop_kernel_c(const svt_mv_cost_param* mv_cost_params, uint8_t* src, uint32_t src_stride,
                               uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width,
                               uint32_t* best_cost, int16_t* best_mvx, int16_t* best_mvy,
                               int16_t search_position_start_x, int16_t search_position_start_y,
                               int16_t search_area_width, int16_t search_area_height, int16_t search_step,
                               int16_t mvx, int16_t mvy);
int  svt_aom_fp_mv_err_cost(const Mv* mv, const svt_mv_cost_param* mv_cost_params);
int  svt_aom_get_sad_per_bit(int qidx, EbBitDepth is_hbd);
void svt_av1_init_me_luts(void);

/* `mvj` is the 4-entry joint cost table; `mvc0`/`mvc1` are the two
 * per-component tables, each indexed by (value + MV_MAX) exactly as C
 * does through `mvcost[i] + ...`. Passing NULL for mvj selects C's
 * `if (mvcost)` NULL arm. */
int32_t ref_md_fp_mv_err_cost(int32_t mv_x, int32_t mv_y, int32_t ref_x, int32_t ref_y, int32_t mv_cost_type,
                              int32_t error_per_bit, const int* mvj, const int* mvc0, const int* mvc1,
                              int32_t use_tables) {
    Mv               ref_mv;
    svt_mv_cost_param p;
    memset(&p, 0, sizeof(p));
    ref_mv.x        = (int16_t)ref_x;
    ref_mv.y        = (int16_t)ref_y;
    p.ref_mv        = &ref_mv;
    p.mv_cost_type  = (MV_COST_TYPE)mv_cost_type;
    p.error_per_bit = error_per_bit;
    if (use_tables) {
        p.mvjcost   = mvj;
        p.mvcost[0] = mvc0;
        p.mvcost[1] = mvc1;
    } else {
        p.mvjcost   = NULL;
        p.mvcost[0] = NULL;
        p.mvcost[1] = NULL;
    }
    Mv mv;
    mv.x = (int16_t)mv_x;
    mv.y = (int16_t)mv_y;
    return svt_aom_fp_mv_err_cost(&mv, &p);
}

/* NOTE: the second parameter is DECLARED `EbBitDepth` but USED as a
 * boolean (`is_hbd ? lut_10 : lut_8`, mode_decision.c:2049), and
 * EB_EIGHT_BIT is 8 — truthy. Every C call site passes a 0/1 flag
 * (mode_decision.c:2109 passes a literal 0; product_coding_loop.c:1908
 * passes `hbd_md`, a uint8_t), so 0/1 is what the shim forwards.
 * Passing the enum would silently select the TEN-bit table for 8-bit. */
int32_t ref_md_get_sad_per_bit(int32_t qidx, int32_t is_hbd) {
    svt_av1_init_me_luts();
    return svt_aom_get_sad_per_bit(qidx, (EbBitDepth)(is_hbd ? 1 : 0));
}

void ref_md_pme_sad_loop_kernel(int32_t ref_x, int32_t ref_y, int32_t mv_cost_type, int32_t error_per_bit,
                                const int* mvj, const int* mvc0, const int* mvc1, int32_t use_tables,
                                const uint8_t* src, int32_t src_stride, const uint8_t* refbuf, int32_t ref_stride,
                                int32_t block_height, int32_t block_width, uint32_t* best_cost, int16_t* best_mvx,
                                int16_t* best_mvy, int32_t search_position_start_x, int32_t search_position_start_y,
                                int32_t search_area_width, int32_t search_area_height, int32_t search_step,
                                int32_t mvx, int32_t mvy) {
    Mv                ref_mv;
    svt_mv_cost_param p;
    memset(&p, 0, sizeof(p));
    ref_mv.x        = (int16_t)ref_x;
    ref_mv.y        = (int16_t)ref_y;
    p.ref_mv        = &ref_mv;
    p.full_ref_mv   = get_fullmv_from_mv(&ref_mv);
    p.mv_cost_type  = (MV_COST_TYPE)mv_cost_type;
    p.error_per_bit = error_per_bit;
    if (use_tables) {
        p.mvjcost   = mvj;
        p.mvcost[0] = mvc0;
        p.mvcost[1] = mvc1;
    }
    svt_pme_sad_loop_kernel_c(&p,
                              (uint8_t*)src,
                              (uint32_t)src_stride,
                              (uint8_t*)refbuf,
                              (uint32_t)ref_stride,
                              (uint32_t)block_height,
                              (uint32_t)block_width,
                              best_cost,
                              best_mvx,
                              best_mvy,
                              (int16_t)search_position_start_x,
                              (int16_t)search_position_start_y,
                              (int16_t)search_area_width,
                              (int16_t)search_area_height,
                              (int16_t)search_step,
                              (int16_t)mvx,
                              (int16_t)mvy);
}

/* ------------------------------------------------------------------ *
 * Per-stage candidate counts.
 *
 *   svt_aom_set_nics      product_coding_loop.c:1358 (EXPORTED)
 *   set_md_stage_counts   product_coding_loop.c:1394 (EXPORTED — and it
 *                         carries NO svt_aom_ prefix; nm -g, not the
 *                         name, is what says so)
 * ------------------------------------------------------------------ */

void svt_aom_set_nics(SequenceControlSet* scs, NicScalingCtrls* scaling_ctrls,
                      uint32_t mds1_count[CAND_CLASS_TOTAL], uint32_t mds2_count[CAND_CLASS_TOTAL],
                      uint32_t mds3_count[CAND_CLASS_TOTAL], uint8_t pic_type, uint32_t qp);
void set_md_stage_counts(PictureControlSet* pcs, ModeDecisionContext* ctx);

void ref_md_set_nics(int32_t s1, int32_t s2, int32_t s3, int32_t pic_type, int32_t qp,
                     int32_t nic_max_qp_based_th_scaling, uint32_t* mds1, uint32_t* mds2,
                     uint32_t* mds3) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    NicScalingCtrls     sc;
    memset(&sc, 0, sizeof(sc));
    sc.stage1_scaling_num = (uint8_t)s1;
    sc.stage2_scaling_num = (uint8_t)s2;
    sc.stage3_scaling_num = (uint8_t)s3;
    scs->qp_based_th_scaling_ctrls.nic_max_qp_based_th_scaling = (bool)nic_max_qp_based_th_scaling;
    svt_aom_set_nics(scs, &sc, mds1, mds2, mds3, (uint8_t)pic_type, (uint32_t)qp);
    free(scs);
}

void ref_md_set_md_stage_counts(int32_t s1, int32_t s2, int32_t s3, int32_t md_staging_mode,
                                int32_t is_i_slice, int32_t is_highest_layer, int32_t qp,
                                int32_t nic_max_qp_based_th_scaling, uint32_t* mds1, uint32_t* mds2,
                                uint32_t* mds3, int32_t* bypass1, int32_t* bypass2) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    pcs->ppcs                     = ppcs;
    pcs->scs                      = scs;
    ppcs->scs                     = scs;
    pcs->slice_type               = is_i_slice ? I_SLICE : B_SLICE;
    ppcs->is_highest_layer        = (bool)is_highest_layer;
    scs->static_config.qp         = (uint32_t)qp;
    scs->qp_based_th_scaling_ctrls.nic_max_qp_based_th_scaling = (bool)nic_max_qp_based_th_scaling;
    ctx->nic_ctrls.scaling_ctrls.stage1_scaling_num            = (uint8_t)s1;
    ctx->nic_ctrls.scaling_ctrls.stage2_scaling_num            = (uint8_t)s2;
    ctx->nic_ctrls.scaling_ctrls.stage3_scaling_num            = (uint8_t)s3;
    ctx->nic_ctrls.md_staging_mode                             = (uint8_t)md_staging_mode;

    set_md_stage_counts(pcs, ctx);

    for (int i = 0; i < CAND_CLASS_TOTAL; i++) {
        mds1[i] = ctx->md_stage_1_count[i];
        mds2[i] = ctx->md_stage_2_count[i];
        mds3[i] = ctx->md_stage_3_count[i];
    }
    *bypass1 = ctx->bypass_md_stage_1 ? 1 : 0;
    *bypass2 = ctx->bypass_md_stage_2 ? 1 : 0;
    free(ctx);
    free(pcs);
    free(ppcs);
    free(scs);
}

/* ------------------------------------------------------------------ *
 * DRL selection.
 *
 *   svt_aom_choose_best_av1_mv_pred   mode_decision.c:527 (EXPORTED)
 *
 * The exported symbol reaches the `static INLINE` av1_drl_ctx
 * (rd_cost.h:85) and svt_av1_mv_bit_cost / _light (rd_cost.c:59-78)
 * along the way, so all four are covered by one tier-1 driver.
 *
 * `stack` arrives packed 3 i32s per CandidateMv:
 *   this_mv_as_int, comp_mv_as_int, weight.
 * ------------------------------------------------------------------ */

#include "md_rate_estimation.h"

void svt_aom_choose_best_av1_mv_pred(ModeDecisionContext* ctx, MvReferenceFrame ref_frame,
                                     PredictionMode mode, Mv mv0, Mv mv1, uint8_t* bestDrlIndex,
                                     Mv best_pred_mv[2]);

void ref_md_choose_best_av1_mv_pred(int32_t shut_fast_rate, int32_t approx_inter_rate,
                                    const int32_t* stack /*[MAX_REF_MV_STACK_SIZE][3]*/,
                                    int32_t ref_mv_count, int32_t ref_frame, int32_t mode,
                                    uint32_t mv0_as_int, uint32_t mv1_as_int,
                                    const int32_t* nmv_vec_cost /*[MV_JOINTS]*/,
                                    const int32_t* nmv_costs0 /*[MV_VALS]*/,
                                    const int32_t* nmv_costs1 /*[MV_VALS]*/,
                                    const int32_t* drl_fac_bits /*[DRL_MODE_CONTEXTS][2]*/,
                                    int32_t* best_drl_index_io, uint32_t* best_pred_mv_io /*[2]*/) {
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(*blk));
    MacroBlockD*             xd   = (MacroBlockD*)calloc(1, sizeof(*xd));
    MdRateEstimationContext* rate = (MdRateEstimationContext*)calloc(1, sizeof(*rate));

    ctx->shut_fast_rate    = (uint8_t)shut_fast_rate;
    ctx->approx_inter_rate = (uint8_t)approx_inter_rate;
    ctx->blk_ptr           = blk;
    blk->av1xd             = xd;
    ctx->md_rate_est_ctx   = rate;
    xd->ref_mv_count[ref_frame] = (uint8_t)ref_mv_count;

    for (int i = 0; i < MAX_REF_MV_STACK_SIZE; i++) {
        ctx->ref_mv_stack[ref_frame][i].this_mv.as_int = (uint32_t)stack[i * 3 + 0];
        ctx->ref_mv_stack[ref_frame][i].comp_mv.as_int = (uint32_t)stack[i * 3 + 1];
        ctx->ref_mv_stack[ref_frame][i].weight         = stack[i * 3 + 2];
    }
    for (int i = 0; i < MV_JOINTS; i++) { rate->nmv_vec_cost[i] = nmv_vec_cost[i]; }
    for (int i = 0; i < MV_VALS; i++) {
        rate->nmv_costs[0][i] = nmv_costs0[i];
        rate->nmv_costs[1][i] = nmv_costs1[i];
    }
    rate->nmvcoststack[0] = &rate->nmv_costs[0][MV_MAX];
    rate->nmvcoststack[1] = &rate->nmv_costs[1][MV_MAX];
    for (int c = 0; c < DRL_MODE_CONTEXTS; c++) {
        rate->drl_mode_fac_bits[c][0] = drl_fac_bits[c * 2 + 0];
        rate->drl_mode_fac_bits[c][1] = drl_fac_bits[c * 2 + 1];
    }

    Mv mv0, mv1;
    mv0.as_int = mv0_as_int;
    mv1.as_int = mv1_as_int;
    /* The two outputs are IN/OUT: C leaves them untouched on the
     * shut_fast_rate early return, so the caller's prior value must be
     * observable. */
    uint8_t best_drl = (uint8_t)*best_drl_index_io;
    Mv      best_pred_mv[2];
    best_pred_mv[0].as_int = best_pred_mv_io[0];
    best_pred_mv[1].as_int = best_pred_mv_io[1];

    svt_aom_choose_best_av1_mv_pred(
        ctx, (MvReferenceFrame)ref_frame, (PredictionMode)mode, mv0, mv1, &best_drl, best_pred_mv);

    *best_drl_index_io = best_drl;
    best_pred_mv_io[0] = best_pred_mv[0].as_int;
    best_pred_mv_io[1] = best_pred_mv[1].as_int;

    free(rate);
    free(xd);
    free(blk);
    free(ctx);
}

/* ------------------------------------------------------------------ *
 * High-bit-depth tune-SSIM distortion.
 *
 *   svt_aom_similarity                       enc_dec_process.c:645 (EXPORTED)
 *   svt_ssim_4x4_hbd_c                       mode_decision.c:4220  (EXPORTED)
 *   svt_ssim_8x8_hbd_c                       mode_decision.c:4245  (EXPORTED)
 *   svt_spatial_full_distortion_ssim_kernel  mode_decision.c:4372  (EXPORTED)
 *
 * The `_c` suffix on the two tile kernels is deliberate: they are the
 * scalar references the port transcribes, not the RTCD pointers.
 * ------------------------------------------------------------------ */

double   svt_aom_similarity(uint32_t sum_s, uint32_t sum_r, uint32_t sum_sq_s, uint32_t sum_sq_r,
                            uint32_t sum_sxr, int count, uint32_t bd);
void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);

/* One-shot RTCD init, same shape as md_subpel_shims.c's.
 *
 * WHY IT IS NEEDED HERE, measured 2026-08-31 on x86_64-linux: the EXPORTED
 * entry point `svt_spatial_full_distortion_ssim_kernel` is a real function
 * (`nm`: `T`), but its hbd arm dereferences `svt_ssim_4x4_hbd` and
 * `svt_ssim_8x8_hbd` — which are RTCD POINTERS (`nm`: `B`, i.e. .bss), NULL
 * until setup. gdb on the failing test: `#0 0x0000000000000000 in ?? ()`
 * called from `#1 svt_spatial_full_distortion_ssim_kernel`. A jump to zero.
 *
 * It passed on aarch64 because the NEON devirtualization header `#define`s
 * those two names to direct calls, so no pointer exists to be NULL — the
 * green there was structural, not evidence. Identical in shape to the
 * `svt_memcpy` NULL that SIGSEGV'd `upsampled_pred` (SUSPECTED-C-BUGS #16),
 * one level deeper: there the shim called the pointer, here the exported
 * function it calls does.
 *
 * The `_c`-suffixed spellings just below are deliberately NOT affected —
 * those are the scalar references the port transcribes, and they are real
 * functions on both ISAs. Only the un-suffixed entry point needs this.
 *
 * The flag is written with the same value by every racing thread and guards
 * only redundant work, never correctness. */
static int md_rtcd_done = 0;
static void md_ensure_rtcd(void) {
    if (!md_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        md_rtcd_done = 1;
    }
}

double   svt_ssim_4x4_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
double   svt_ssim_8x8_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
uint64_t svt_spatial_full_distortion_ssim_kernel(uint8_t* input, uint32_t input_offset,
                                                 uint32_t input_stride, uint8_t* recon,
                                                 int32_t recon_offset, uint32_t recon_stride,
                                                 uint32_t area_width, uint32_t area_height,
                                                 bool hbd, double ac_bias);

double ref_md_similarity(uint32_t sum_s, uint32_t sum_r, uint32_t sum_sq_s, uint32_t sum_sq_r,
                         uint32_t sum_sxr, int32_t count, uint32_t bd) {
    return svt_aom_similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, bd);
}

double ref_md_ssim_4x4_hbd(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    return svt_ssim_4x4_hbd_c(s, sp, r, rp);
}

double ref_md_ssim_8x8_hbd(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    return svt_ssim_8x8_hbd_c(s, sp, r, rp);
}

/* The kernel takes uint8_t* and casts internally on the hbd arm, so the
 * offsets it applies are in uint16_t ELEMENTS, not bytes. Forwarding a
 * uint16_t buffer through a uint8_t* is exactly what C's own call sites
 * do (they pass EbPictureBufferDesc::y_buffer). */
uint64_t ref_md_spatial_full_distortion_ssim(const uint16_t* input, uint32_t input_offset,
                                             uint32_t input_stride, const uint16_t* recon,
                                             int32_t recon_offset, uint32_t recon_stride,
                                             uint32_t area_width, uint32_t area_height,
                                             int32_t hbd, double ac_bias) {
    md_ensure_rtcd();
    return svt_spatial_full_distortion_ssim_kernel((uint8_t*)input,
                                                   input_offset,
                                                   input_stride,
                                                   (uint8_t*)recon,
                                                   recon_offset,
                                                   recon_stride,
                                                   area_width,
                                                   area_height,
                                                   hbd ? true : false,
                                                   ac_bias);
}

/* ------------------------------------------------------------------ *
 * Reference-frame signalling rate.
 *
 *   svt_aom_collect_neighbors_ref_counts_new  entropy_coding.c:1877 (EXPORTED)
 *   svt_aom_get_reference_mode_context_new    entropy_coding.c:1833 (EXPORTED)
 *   estimate_ref_frame_type_bits              rd_cost.c:643         (EXPORTED)
 *
 * The last one reaches every prediction-context function in the family,
 * because its only inputs beyond the rate tables are the two neighbours
 * and each reference type takes a different path through them.
 *
 * Neighbours arrive as 3 i32s: ref0, ref1, use_intrabc; a NULL pointer
 * means "edge not available".
 * ------------------------------------------------------------------ */

#include "rd_cost.h"

void     svt_aom_collect_neighbors_ref_counts_new(MacroBlockD* const xd);
int      svt_aom_get_reference_mode_context_new(const MacroBlockD* xd);
uint64_t estimate_ref_frame_type_bits(ModeDecisionContext* ctx, BlkStruct* blk_ptr,
                                      uint8_t ref_frame_type, bool is_compound);

static void md_fill_neighbor(MbModeInfo* mbmi, const int32_t* spec) {
    memset(mbmi, 0, sizeof(*mbmi));
    mbmi->block_mi.ref_frame[0] = (MvReferenceFrame)spec[0];
    mbmi->block_mi.ref_frame[1] = (MvReferenceFrame)spec[1];
    mbmi->block_mi.use_intrabc  = (uint8_t)spec[2];
}

/* All six rate tables arrive flattened in C's index order. */
uint64_t ref_md_estimate_ref_frame_type_bits(
    const int32_t* above /*[3] or NULL*/, const int32_t* left /*[3] or NULL*/,
    int32_t ref_frame_type, int32_t is_compound,
    const int32_t* comp_ref_type /*[5][2]*/, const int32_t* uni_comp_ref /*[3][3][2]*/,
    const int32_t* comp_ref /*[3][3][2]*/, const int32_t* comp_bwd_ref /*[3][2][2]*/,
    const int32_t* single_ref /*[3][6][2]*/, int32_t* mode_ctx_out,
    uint8_t* ref_counts_out /*[8]*/) {
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(*blk));
    MacroBlockD*             xd   = (MacroBlockD*)calloc(1, sizeof(*xd));
    MdRateEstimationContext* rate = (MdRateEstimationContext*)calloc(1, sizeof(*rate));
    MbModeInfo*              cur  = (MbModeInfo*)calloc(1, sizeof(*cur));
    MbModeInfo               above_mi, left_mi;

    ctx->blk_ptr         = blk;
    ctx->md_rate_est_ctx = rate;
    blk->av1xd           = xd;
    xd->mi               = (MbModeInfo**)calloc(1, sizeof(MbModeInfo*));
    xd->mi[0]            = cur;

    if (above) {
        md_fill_neighbor(&above_mi, above);
        xd->above_mbmi     = &above_mi;
        xd->up_available   = 1;
    } else {
        xd->above_mbmi   = NULL;
        xd->up_available = 0;
    }
    if (left) {
        md_fill_neighbor(&left_mi, left);
        xd->left_mbmi     = &left_mi;
        xd->left_available = 1;
    } else {
        xd->left_mbmi      = NULL;
        xd->left_available = 0;
    }

    for (int c = 0; c < COMP_REF_TYPE_CONTEXTS; c++) {
        for (int b = 0; b < 2; b++) {
            rate->comp_ref_type_fac_bits[c][b] = comp_ref_type[c * 2 + b];
        }
    }
    for (int c = 0; c < REF_CONTEXTS; c++) {
        for (int i = 0; i < 3; i++) {
            for (int b = 0; b < 2; b++) {
                rate->uni_comp_ref_fac_bits[c][i][b] = uni_comp_ref[(c * 3 + i) * 2 + b];
                rate->comp_ref_fac_bits[c][i][b]     = comp_ref[(c * 3 + i) * 2 + b];
            }
        }
        for (int i = 0; i < 2; i++) {
            for (int b = 0; b < 2; b++) {
                rate->comp_bwd_ref_fac_bits[c][i][b] = comp_bwd_ref[(c * 2 + i) * 2 + b];
            }
        }
        for (int i = 0; i < 6; i++) {
            for (int b = 0; b < 2; b++) {
                rate->single_ref_fac_bits[c][i][b] = single_ref[(c * 6 + i) * 2 + b];
            }
        }
    }

    svt_aom_collect_neighbors_ref_counts_new(xd);
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; i++) { ref_counts_out[i] = xd->neighbors_ref_counts[i]; }
    *mode_ctx_out = svt_aom_get_reference_mode_context_new(xd);

    const uint64_t out = estimate_ref_frame_type_bits(
        ctx, blk, (uint8_t)ref_frame_type, is_compound ? true : false);

    free(xd->mi);
    free(cur);
    free(rate);
    free(xd);
    free(blk);
    free(ctx);
    return out;
}
