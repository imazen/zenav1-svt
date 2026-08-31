/*
 * C shims for the per-preset signal derivations of
 * `Source/Lib/Codec/enc_mode_config.c` (port module
 * `svtav1-encoder/src/port_enc_mode_config`).
 *
 * Most of the exported getters here are pure functions of scalars and need no
 * shim at all -- they are bound directly in `src/sig_deriv.rs`. This file
 * exists for the ones whose C signature takes a `PictureControlSet*` /
 * `PictureParentControlSet*` / `SequenceControlSet*`: the shim builds a
 * SYNTHETIC control set holding only the fields the callee reads (each one
 * named in a comment, verified against the function body), calls the REAL
 * exported symbol, and returns the scalar result.
 *
 * RULE, inherited from ref_shims.c's file header and its 2026-08-31 audit:
 * A SHIM MUST NOT KEEP PER-CALL STATE IN A `static`. cargo runs a test
 * binary's tests on several threads, so a `static` synthetic control set is a
 * data race that fails as an occasional wrong NUMBER. Every synthetic struct
 * below is `calloc`/`free` per call.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "enc_mode_config.h"

/* ---- svt_aom_get_nsq_search_level_default (enc_mode_config.c:8254) ----
 * Reads: pcs->ppcs->temporal_layer_index, pcs->ppcs->r0_gen, pcs->ppcs->r0,
 *        pcs->slice_type, pcs->temporal_layer_index, pcs->scs->seq_qp_mod. */
uint8_t ref_get_nsq_search_level_default(int8_t enc_mode, int coeff_lvl, uint32_t qp,
                                         uint8_t ppcs_temporal_layer_index, uint8_t r0_gen, double r0,
                                         uint8_t is_islice, uint8_t temporal_layer_index, uint8_t seq_qp_mod) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    scs->seq_qp_mod              = seq_qp_mod;
    ppcs->temporal_layer_index   = ppcs_temporal_layer_index;
    ppcs->r0_gen                 = (bool)r0_gen;
    ppcs->r0                     = r0;
    pcs->ppcs                    = ppcs;
    pcs->scs                     = scs;
    pcs->slice_type              = is_islice ? I_SLICE : B_SLICE;
    pcs->temporal_layer_index    = temporal_layer_index;
    uint8_t out = svt_aom_get_nsq_search_level_default(pcs, (EncMode)enc_mode, (InputCoeffLvl)coeff_lvl, qp);
    free(pcs);
    free(ppcs);
    free(scs);
    return out;
}

/* ---- svt_aom_get_nsq_search_level_rtc (enc_mode_config.c:8326) ----
 * Reads: pcs->scs->seq_qp_mod. */
uint8_t ref_get_nsq_search_level_rtc(int coeff_lvl, uint32_t qp, uint8_t seq_qp_mod) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureControlSet*  pcs = (PictureControlSet*)calloc(1, sizeof(*pcs));
    scs->seq_qp_mod = seq_qp_mod;
    pcs->scs        = scs;
    uint8_t out     = svt_aom_get_nsq_search_level_rtc(pcs, (InputCoeffLvl)coeff_lvl, qp);
    free(pcs);
    free(scs);
    return out;
}

/* ---- svt_aom_get_nsq_search_level_allintra (enc_mode_config.c:8363) ----
 * Reads: pcs->coeff_lvl, pcs->scs->seq_qp_mod. */
uint8_t ref_get_nsq_search_level_allintra(int8_t enc_mode, uint32_t qp, int coeff_lvl, uint8_t seq_qp_mod) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureControlSet*  pcs = (PictureControlSet*)calloc(1, sizeof(*pcs));
    scs->seq_qp_mod = seq_qp_mod;
    pcs->scs        = scs;
    pcs->coeff_lvl  = (InputCoeffLvl)coeff_lvl;
    uint8_t out     = svt_aom_get_nsq_search_level_allintra(pcs, (EncMode)enc_mode, qp);
    free(pcs);
    free(scs);
    return out;
}

/* ---- svt_aom_derive_gm_level (enc_mode_config.c:194) ----
 * Reads: pcs->enc_mode, pcs->slice_type. */
uint8_t ref_derive_gm_level(int8_t enc_mode, uint8_t is_islice, uint8_t super_res_off) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    ppcs->enc_mode   = (EncMode)enc_mode;
    ppcs->slice_type = is_islice ? I_SLICE : B_SLICE;
    uint8_t out      = svt_aom_derive_gm_level(ppcs, (bool)super_res_off);
    free(ppcs);
    return out;
}

/* ---- svt_aom_sig_deriv_pre_analysis_pcs (enc_mode_config.c:2750) ----
 * Reads: pcs->scs->max_input_luma_width/height, scs->static_config.rtc,
 *        pcs->enc_mode. Writes the ten flags returned through `out`. */
void ref_sig_deriv_pre_analysis_pcs(int8_t enc_mode, uint16_t max_w, uint16_t max_h, uint8_t rtc, uint8_t out[10]) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    scs->max_input_luma_width  = max_w;
    scs->max_input_luma_height = max_h;
    scs->static_config.rtc     = (bool)rtc;
    ppcs->scs                  = scs;
    ppcs->enc_mode             = (EncMode)enc_mode;
    svt_aom_sig_deriv_pre_analysis_pcs(ppcs);
    out[0] = ppcs->enable_me_16x16;
    out[1] = ppcs->enable_me_8x8;
    out[2] = ppcs->enable_hme_flag;
    out[3] = ppcs->enable_hme_level0_flag;
    out[4] = ppcs->enable_hme_level1_flag;
    out[5] = ppcs->enable_hme_level2_flag;
    out[6] = ppcs->tf_enable_hme_flag;
    out[7] = ppcs->tf_enable_hme_level0_flag;
    out[8] = ppcs->tf_enable_hme_level1_flag;
    out[9] = ppcs->tf_enable_hme_level2_flag;
    free(ppcs);
    free(scs);
}

/* ---- svt_aom_set_mfmv_config (enc_mode_config.c:10134) ----
 * Reads: scs->static_config.enable_mfmv, scs->static_config.rtc.
 * Writes: scs->mfmv_enabled. */
uint8_t ref_set_mfmv_config(int8_t enc_mode, uint8_t rtc, int32_t config_enable_mfmv) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    scs->static_config.rtc          = (bool)rtc;
    scs->static_config.enable_mfmv  = config_enable_mfmv;
    svt_aom_set_mfmv_config(scs, enc_mode);
    uint8_t out = scs->mfmv_enabled;
    free(scs);
    return out;
}

/* ---- svt_aom_is_ref_same_size (enc_mode_config.c:2857) ----
 * Reads: pcs->ppcs->is_not_scaled, pcs->slice_type,
 *        pcs->ref_pic_ptr_array[list][idx] (and, when non-NULL, the
 *        EbReferenceObject's reference_picture width/height),
 *        pcs->ppcs->frame_width/height.
 * `ref_present == 0` leaves the wrapper pointer NULL, which is the branch the
 * port models as "reference absent". */
uint8_t ref_is_ref_same_size(uint8_t is_not_scaled, uint8_t is_b_slice, uint8_t ref_present, uint16_t ref_w,
                             uint16_t ref_h, uint16_t frame_w, uint16_t frame_h) {
    SequenceControlSet*      scs    = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs   = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs    = (PictureControlSet*)calloc(1, sizeof(*pcs));
    EbObjectWrapper*         wrap   = (EbObjectWrapper*)calloc(1, sizeof(*wrap));
    EbReferenceObject*       refobj = (EbReferenceObject*)calloc(1, sizeof(*refobj));
    EbPictureBufferDesc*     refpic = (EbPictureBufferDesc*)calloc(1, sizeof(*refpic));

    ppcs->is_not_scaled = is_not_scaled;
    ppcs->frame_width   = frame_w;
    ppcs->frame_height  = frame_h;
    pcs->ppcs           = ppcs;
    pcs->scs            = scs;
    /* SliceType has only B_SLICE and I_SLICE (definitions.h:1890); the C
       predicate is `slice_type != B_SLICE`. */
    pcs->slice_type     = is_b_slice ? B_SLICE : I_SLICE;
    if (ref_present) {
        refpic->width               = ref_w;
        refpic->height              = ref_h;
        refobj->reference_picture   = refpic;
        wrap->object_ptr            = refobj;
        pcs->ref_pic_ptr_array[0][0] = wrap;
    }
    uint8_t out = (uint8_t)svt_aom_is_ref_same_size(pcs, 0, 0);
    free(refpic);
    free(refobj);
    free(wrap);
    free(pcs);
    free(ppcs);
    free(scs);
    return out;
}

/* ===========================================================================
 * level -> controls tables (enc_mode_config.c).
 *
 * Each shim allocates a ZEROED ModeDecisionContext / PictureParentControlSet
 * (calloc, per call -- never a `static`, see the file header), calls the real
 * exported setter, and copies the resulting control struct out field by field.
 * The zeroing matters: several C arms write only a subset of the struct, so
 * "what the caller sees" for the untouched fields is the zeroed value, and the
 * port models exactly that.
 * ======================================================================== */

/* ---- svt_aom_set_wm_controls (enc_mode_config.c:4397) ----
 * out: enabled, use_wm_for_mvp, refinement_iterations, refine_diag,
 *      refine_level, lower_band_th, upper_band_th, shut_approx_if_not_mds0. */
void ref_set_wm_controls(uint8_t wm_level, uint32_t out[8]) {
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    svt_aom_set_wm_controls(ctx, wm_level);
    out[0] = ctx->wm_ctrls.enabled;
    out[1] = ctx->wm_ctrls.use_wm_for_mvp;
    out[2] = ctx->wm_ctrls.refinement_iterations;
    out[3] = ctx->wm_ctrls.refine_diag;
    out[4] = ctx->wm_ctrls.refine_level;
    out[5] = ctx->wm_ctrls.lower_band_th;
    out[6] = ctx->wm_ctrls.upper_band_th;
    out[7] = ctx->wm_ctrls.shut_approx_if_not_mds0;
    free(ctx);
}

/* svt_aom_set_bipred3x3_controls is EXPORTED (nm -g finds it in
   libSvtAv1Enc.a) but has no prototype in enc_mode_config.h -- its only C
   caller is inside the .c itself. Declare it here so the shim links against
   the real symbol rather than an implicit declaration. */
void svt_aom_set_bipred3x3_controls(ModeDecisionContext* ctx, uint8_t bipred3x3_injection);

/* ---- svt_aom_set_bipred3x3_controls (enc_mode_config.c:5869) ----
 * out: enabled, search_diag, use_best_list, use_l0_l1_dev. */
void ref_set_bipred3x3_controls(uint8_t level, uint32_t out[4]) {
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    svt_aom_set_bipred3x3_controls(ctx, level);
    out[0] = ctx->bipred3x3_ctrls.enabled;
    out[1] = ctx->bipred3x3_ctrls.search_diag;
    out[2] = ctx->bipred3x3_ctrls.use_best_list;
    out[3] = ctx->bipred3x3_ctrls.use_l0_l1_dev;
    free(ctx);
}

/* ---- svt_aom_set_dist_based_ref_pruning_controls (enc_mode_config.c:5906) ----
 * out: enabled, use_tpl_info_offset, check_closest_multiplier,
 *      max_dev_to_best[11], closest_refs[11]  (25 slots). */
void ref_set_dist_based_ref_pruning_controls(uint8_t level, uint32_t out[25]) {
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    svt_aom_set_dist_based_ref_pruning_controls(ctx, level);
    out[0] = ctx->ref_pruning_ctrls.enabled;
    out[1] = ctx->ref_pruning_ctrls.use_tpl_info_offset;
    out[2] = ctx->ref_pruning_ctrls.check_closest_multiplier;
    for (int i = 0; i < TOT_INTER_GROUP; ++i) {
        out[3 + i]  = ctx->ref_pruning_ctrls.max_dev_to_best[i];
        out[14 + i] = ctx->ref_pruning_ctrls.closest_refs[i];
    }
    free(ctx);
}

/* ---- svt_aom_md_pme_search_controls (enc_mode_config.c:3310) ----
 * out (int32 because four fields are `int` and take MIN/MAX_SIGNED_VALUE):
 *   enabled, dist_type, full_pel_search_width, full_pel_search_height,
 *   early_check_mv_th_multiplier, pre_fp_pme_to_me_cost_th,
 *   pre_fp_pme_to_me_mv_th, post_fp_pme_to_me_cost_th,
 *   post_fp_pme_to_me_mv_th, enable_psad, sa_q_weight. */
void ref_md_pme_search_controls(uint8_t level, int32_t out[11]) {
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    svt_aom_md_pme_search_controls(ctx, level);
    out[0]  = ctx->md_pme_ctrls.enabled;
    out[1]  = (int32_t)ctx->md_pme_ctrls.dist_type;
    out[2]  = ctx->md_pme_ctrls.full_pel_search_width;
    out[3]  = ctx->md_pme_ctrls.full_pel_search_height;
    out[4]  = ctx->md_pme_ctrls.early_check_mv_th_multiplier;
    out[5]  = ctx->md_pme_ctrls.pre_fp_pme_to_me_cost_th;
    out[6]  = ctx->md_pme_ctrls.pre_fp_pme_to_me_mv_th;
    out[7]  = ctx->md_pme_ctrls.post_fp_pme_to_me_cost_th;
    out[8]  = ctx->md_pme_ctrls.post_fp_pme_to_me_mv_th;
    out[9]  = ctx->md_pme_ctrls.enable_psad;
    out[10] = ctx->md_pme_ctrls.sa_q_weight;
    free(ctx);
}

/* ---- svt_aom_set_gm_controls (enc_mode_config.c:2491) ----
 * Reads pcs->input_resolution for the MV-based correspondence method.
 * out: enabled, identiy_exit, search_start_model, search_end_model,
 *      skip_identity, bypass_based_on_me, params_refinement_steps,
 *      downsample_level, corners, chess_rfn, match_sz, inj_psq_glb,
 *      pp_enabled, ref_idx0_only, rfn_early_exit, correspondence_method. */
void ref_set_gm_controls(uint8_t gm_level, int input_resolution, uint32_t out[16]) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    ppcs->input_resolution = (ResolutionRange)input_resolution;
    svt_aom_set_gm_controls(ppcs, gm_level);
    out[0]  = ppcs->gm_ctrls.enabled;
    out[1]  = ppcs->gm_ctrls.identiy_exit;
    out[2]  = ppcs->gm_ctrls.search_start_model;
    out[3]  = ppcs->gm_ctrls.search_end_model;
    out[4]  = ppcs->gm_ctrls.skip_identity;
    out[5]  = ppcs->gm_ctrls.bypass_based_on_me;
    out[6]  = ppcs->gm_ctrls.params_refinement_steps;
    out[7]  = ppcs->gm_ctrls.downsample_level;
    out[8]  = ppcs->gm_ctrls.corners;
    out[9]  = ppcs->gm_ctrls.chess_rfn;
    out[10] = ppcs->gm_ctrls.match_sz;
    out[11] = (uint32_t)ppcs->gm_ctrls.inj_psq_glb;
    out[12] = (uint32_t)ppcs->gm_ctrls.pp_enabled;
    out[13] = (uint32_t)ppcs->gm_ctrls.ref_idx0_only;
    out[14] = ppcs->gm_ctrls.rfn_early_exit;
    out[15] = (uint32_t)ppcs->gm_ctrls.correspondence_method;
    free(ppcs);
}

/* ===========================================================================
 * ME signal derivation (enc_mode_config.c:700 and :848).
 *
 * Both entry points are EXPORTED, and between them they call every `static`
 * ME helper in the file (set_me_search_params, set_hme_search_params,
 * svt_aom_set_prehme_ctrls, svt_aom_set_me_hme_ref_prune_ctrls,
 * svt_aom_set_me_sr_adjustment_ctrls, svt_aom_set_mv_based_sa_ctrls,
 * svt_aom_set_me_8x8_var_ctrls, tf_set_me_hme_params_oq). Driving the entry
 * point and reading the whole MeContext back therefore gets TIER 1 evidence
 * for the statics for free -- which is the point of doing it this way rather
 * than hand-deriving eight tables of vectors.
 * ======================================================================== */

#include "me_context.h"

/* Field order of ref_sig_deriv_me`s `out` (ME_O_COUNT slots). Kept in one place
   because the Rust side indexes the same layout. */
enum {
    ME_O_SA_MIN_W = 0, ME_O_SA_MIN_H, ME_O_SA_MAX_W, ME_O_SA_MAX_H,
    ME_O_NUM_HME_W, ME_O_NUM_HME_H,
    ME_O_HME_L0_MIN_W, ME_O_HME_L0_MIN_H, ME_O_HME_L0_MAX_W, ME_O_HME_L0_MAX_H,
    ME_O_HME_L1_W, ME_O_HME_L1_H, ME_O_HME_L2_W, ME_O_HME_L2_H,
    ME_O_EN_HME, ME_O_EN_HME_L0, ME_O_EN_HME_L1, ME_O_EN_HME_L2,
    ME_O_HME_METHOD, ME_O_ME_METHOD,
    ME_O_RED_HME_MIN, ME_O_RED_HME_MAX,
    ME_O_PREHME_EN,
    ME_O_PREHME_V_MIN_W, ME_O_PREHME_V_MIN_H, ME_O_PREHME_V_MAX_W, ME_O_PREHME_V_MAX_H,
    ME_O_PREHME_H_MIN_W, ME_O_PREHME_H_MIN_H, ME_O_PREHME_H_MAX_W, ME_O_PREHME_H_MAX_H,
    ME_O_PREHME_SKIP_LINE, ME_O_PREHME_L1_EXIT,
    ME_O_PRUNE_EN, ME_O_PRUNE_HME_DEV, ME_O_PRUNE_ME_DEV,
    ME_O_PRUNE_ZZ_TH, ME_O_PRUNE_ZZ_PCT, ME_O_PRUNE_PHME_TH, ME_O_PRUNE_PHME_PCT,
    ME_O_SR_EN, ME_O_SR_MV_LEN_TH, ME_O_SR_STAT_TH, ME_O_SR_STAT_DIV,
    ME_O_SR_RED_TH, ME_O_SR_LOW_DIV, ME_O_SR_DIST_RESIZE,
    ME_O_MVSA_EN, ME_O_MVSA_NEAREST, ME_O_MVSA_MV_TH, ME_O_MVSA_MULT,
    ME_O_VAR_EN, ME_O_VAR_DIV4, ME_O_VAR_DIV2, ME_O_VAR_MULT2,
    ME_O_PRUNE_CAND_TH, ME_O_SC_BOOST, ME_O_BEST_UNIPRED,
    ME_O_EARLY_EXIT, ME_O_STATIC_B64, ME_O_SAFE_ZZ, ME_O_PREV_STAGE,
    ME_O_COUNT
};

static void me_ctx_dump_common(const MeContext* m, uint32_t* out) {
    out[ME_O_SA_MIN_W]  = m->me_sa.sa_min.width;
    out[ME_O_SA_MIN_H]  = m->me_sa.sa_min.height;
    out[ME_O_SA_MAX_W]  = m->me_sa.sa_max.width;
    out[ME_O_SA_MAX_H]  = m->me_sa.sa_max.height;
    out[ME_O_NUM_HME_W] = m->num_hme_sa_w;
    out[ME_O_NUM_HME_H] = m->num_hme_sa_h;
    out[ME_O_HME_L1_W]  = m->hme_l1_sa.width;
    out[ME_O_HME_L1_H]  = m->hme_l1_sa.height;
    out[ME_O_HME_L2_W]  = m->hme_l2_sa.width;
    out[ME_O_HME_L2_H]  = m->hme_l2_sa.height;
    out[ME_O_EN_HME]    = m->enable_hme_flag;
    out[ME_O_EN_HME_L0] = m->enable_hme_level0_flag;
    out[ME_O_EN_HME_L1] = m->enable_hme_level1_flag;
    out[ME_O_EN_HME_L2] = m->enable_hme_level2_flag;
    out[ME_O_HME_METHOD] = m->hme_search_method;
    out[ME_O_ME_METHOD]  = m->me_search_method;
    out[ME_O_RED_HME_MIN] = m->reduce_hme_l0_sr_th_min;
    out[ME_O_RED_HME_MAX] = m->reduce_hme_l0_sr_th_max;
    out[ME_O_PREHME_EN]        = m->prehme_ctrl.enable;
    out[ME_O_PREHME_V_MIN_W]   = m->prehme_ctrl.prehme_sa_cfg[0].sa_min.width;
    out[ME_O_PREHME_V_MIN_H]   = m->prehme_ctrl.prehme_sa_cfg[0].sa_min.height;
    out[ME_O_PREHME_V_MAX_W]   = m->prehme_ctrl.prehme_sa_cfg[0].sa_max.width;
    out[ME_O_PREHME_V_MAX_H]   = m->prehme_ctrl.prehme_sa_cfg[0].sa_max.height;
    out[ME_O_PREHME_H_MIN_W]   = m->prehme_ctrl.prehme_sa_cfg[1].sa_min.width;
    out[ME_O_PREHME_H_MIN_H]   = m->prehme_ctrl.prehme_sa_cfg[1].sa_min.height;
    out[ME_O_PREHME_H_MAX_W]   = m->prehme_ctrl.prehme_sa_cfg[1].sa_max.width;
    out[ME_O_PREHME_H_MAX_H]   = m->prehme_ctrl.prehme_sa_cfg[1].sa_max.height;
    out[ME_O_PREHME_SKIP_LINE] = m->prehme_ctrl.skip_search_line;
    out[ME_O_PREHME_L1_EXIT]   = m->prehme_ctrl.l1_early_exit;
    out[ME_O_PRUNE_EN]       = m->me_hme_prune_ctrls.enable_me_hme_ref_pruning;
    out[ME_O_PRUNE_HME_DEV]  = m->me_hme_prune_ctrls.prune_ref_if_hme_sad_dev_bigger_than_th;
    out[ME_O_PRUNE_ME_DEV]   = m->me_hme_prune_ctrls.prune_ref_if_me_sad_dev_bigger_than_th;
    out[ME_O_PRUNE_ZZ_TH]    = m->me_hme_prune_ctrls.zz_sad_th;
    out[ME_O_PRUNE_ZZ_PCT]   = m->me_hme_prune_ctrls.zz_sad_pct;
    out[ME_O_PRUNE_PHME_TH]  = m->me_hme_prune_ctrls.phme_sad_th;
    out[ME_O_PRUNE_PHME_PCT] = m->me_hme_prune_ctrls.phme_sad_pct;
    out[ME_O_SR_EN]          = m->me_sr_adjustment_ctrls.enable_me_sr_adjustment;
    out[ME_O_SR_MV_LEN_TH]   = m->me_sr_adjustment_ctrls.reduce_me_sr_based_on_mv_length_th;
    out[ME_O_SR_STAT_TH]     = m->me_sr_adjustment_ctrls.stationary_hme_sad_abs_th;
    out[ME_O_SR_STAT_DIV]    = m->me_sr_adjustment_ctrls.stationary_me_sr_divisor;
    out[ME_O_SR_RED_TH]      = m->me_sr_adjustment_ctrls.reduce_me_sr_based_on_hme_sad_abs_th;
    out[ME_O_SR_LOW_DIV]     = m->me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad;
    out[ME_O_SR_DIST_RESIZE] = m->me_sr_adjustment_ctrls.distance_based_hme_resizing;
    out[ME_O_MVSA_EN]      = m->mv_based_sa_adj.enabled;
    out[ME_O_MVSA_NEAREST] = m->mv_based_sa_adj.nearest_ref_only;
    out[ME_O_MVSA_MV_TH]   = m->mv_based_sa_adj.mv_size_th;
    out[ME_O_MVSA_MULT]    = m->mv_based_sa_adj.sa_multiplier;
    out[ME_O_VAR_EN]     = m->me_8x8_var_ctrls.enabled;
    out[ME_O_VAR_DIV4]   = m->me_8x8_var_ctrls.me_sr_div4_th;
    out[ME_O_VAR_DIV2]   = m->me_8x8_var_ctrls.me_sr_div2_th;
    out[ME_O_VAR_MULT2]  = m->me_8x8_var_ctrls.me_sr_mult2_th;
    out[ME_O_SC_BOOST]   = m->sc_class_me_boost;
    out[ME_O_EARLY_EXIT] = m->me_early_exit_th;
    out[ME_O_SAFE_ZZ]    = m->me_safe_limit_zz_th;
    out[ME_O_PREV_STAGE] = m->prev_me_stage_based_exit_th;
}

/* ---- svt_aom_sig_deriv_me (enc_mode_config.c:700) ---- */
void ref_sig_deriv_me(int8_t enc_mode, uint8_t sc_class5, int input_resolution, uint8_t rtc,
                      uint8_t is_base, uint8_t hierarchical_levels, uint8_t en_hme, uint8_t en_hme_l0,
                      uint8_t en_hme_l1, uint8_t en_hme_l2, uint8_t use_best_unipred,
                      uint8_t me_qp_scaling, uint8_t hme_qp_scaling, uint32_t qp,
                      uint8_t safe_limit_nref, uint32_t safe_limit_zz_th, uint32_t* out) {
    SequenceControlSet*      scs    = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs   = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    MeContext*               me_ctx = (MeContext*)calloc(1, sizeof(*me_ctx));

    scs->input_resolution      = (ResolutionRange)input_resolution;
    scs->static_config.rtc     = (bool)rtc;
    scs->static_config.qp      = qp;
    scs->qp_based_th_scaling_ctrls.me_qp_based_th_scaling  = (bool)me_qp_scaling;
    scs->qp_based_th_scaling_ctrls.hme_qp_based_th_scaling = (bool)hme_qp_scaling;
    scs->mrp_ctrls.safe_limit_nref   = safe_limit_nref;
    scs->mrp_ctrls.safe_limit_zz_th  = safe_limit_zz_th;

    ppcs->scs                 = scs;
    ppcs->enc_mode            = (EncMode)enc_mode;
    ppcs->sc_class5           = sc_class5;
    ppcs->hierarchical_levels = hierarchical_levels;
    /* frame_is_boosted(ppcs) == frame_is_intra_only(ppcs) ||
       update_type is ARF/GF (enc_mode_config.h:103). Drive it through the
       KEY_FRAME arm for is_base, and an inter LF update otherwise. */
    ppcs->frm_hdr.frame_type = is_base ? KEY_FRAME : INTER_FRAME;
    ppcs->update_type        = is_base ? SVT_AV1_KF_UPDATE : SVT_AV1_LF_UPDATE;
    ppcs->enable_hme_flag        = en_hme;
    ppcs->enable_hme_level0_flag = en_hme_l0;
    ppcs->enable_hme_level1_flag = en_hme_l1;
    ppcs->enable_hme_level2_flag = en_hme_l2;
    ppcs->use_best_me_unipred_cand_only = use_best_unipred;

    svt_aom_sig_deriv_me(scs, ppcs, me_ctx);

    me_ctx_dump_common(me_ctx, out);
    out[ME_O_HME_L0_MIN_W] = me_ctx->hme_l0_sa.sa_min.width;
    out[ME_O_HME_L0_MIN_H] = me_ctx->hme_l0_sa.sa_min.height;
    out[ME_O_HME_L0_MAX_W] = me_ctx->hme_l0_sa.sa_max.width;
    out[ME_O_HME_L0_MAX_H] = me_ctx->hme_l0_sa.sa_max.height;
    out[ME_O_PRUNE_CAND_TH] = me_ctx->prune_me_candidates_th;
    out[ME_O_BEST_UNIPRED]  = me_ctx->use_best_unipred_cand_only;
    out[ME_O_STATIC_B64]    = me_ctx->me_static_b64_th;
    free(me_ctx);
    free(ppcs);
    free(scs);
}

/* ---- svt_aom_sig_deriv_me_tf (enc_mode_config.c:848) ----
 * Reads pcs->tf_ctrls.{hme_me_level,qp_opt}, pcs->scs->input_resolution,
 * pcs->scs->qp_based_th_scaling_ctrls.tf_me_qp_based_th_scaling,
 * pcs->scs->static_config.qp and pcs->tf_enable_hme_level*_flag.
 * The TF arm writes hme_l0_sa_default_tf, NOT hme_l0_sa -- so the two L0
 * slots below read the *_default_tf field. */
void ref_sig_deriv_me_tf(uint8_t hme_me_level, int input_resolution, uint8_t qp_opt,
                         uint8_t tf_me_qp_scaling, uint32_t qp, uint8_t tf_en_hme,
                         uint8_t tf_en_hme_l0, uint8_t tf_en_hme_l1, uint8_t tf_en_hme_l2,
                         uint32_t* out) {
    SequenceControlSet*      scs    = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs   = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    MeContext*               me_ctx = (MeContext*)calloc(1, sizeof(*me_ctx));

    scs->input_resolution = (ResolutionRange)input_resolution;
    scs->static_config.qp = qp;
    scs->qp_based_th_scaling_ctrls.tf_me_qp_based_th_scaling = (bool)tf_me_qp_scaling;

    ppcs->scs                       = scs;
    ppcs->tf_ctrls.hme_me_level     = hme_me_level;
    ppcs->tf_ctrls.qp_opt           = qp_opt;
    ppcs->tf_enable_hme_flag        = tf_en_hme;
    ppcs->tf_enable_hme_level0_flag = tf_en_hme_l0;
    ppcs->tf_enable_hme_level1_flag = tf_en_hme_l1;
    ppcs->tf_enable_hme_level2_flag = tf_en_hme_l2;

    svt_aom_sig_deriv_me_tf(ppcs, me_ctx);

    me_ctx_dump_common(me_ctx, out);
    out[ME_O_HME_L0_MIN_W] = me_ctx->hme_l0_sa_default_tf.sa_min.width;
    out[ME_O_HME_L0_MIN_H] = me_ctx->hme_l0_sa_default_tf.sa_min.height;
    out[ME_O_HME_L0_MAX_W] = me_ctx->hme_l0_sa_default_tf.sa_max.width;
    out[ME_O_HME_L0_MAX_H] = me_ctx->hme_l0_sa_default_tf.sa_max.height;
    out[ME_O_PRUNE_CAND_TH] = 0;
    out[ME_O_BEST_UNIPRED]  = 0;
    out[ME_O_STATIC_B64]    = 0;
    free(me_ctx);
    free(ppcs);
    free(scs);
}

/* Guard: the Rust side hardcodes ME_OUT_SLOTS; fail the BUILD if this layout
   changes, because the C writes into a Rust-owned array of exactly that size
   and a silent growth is a buffer overrun, not a test failure. */
typedef char me_out_slot_count_check[(ME_O_COUNT == 62) ? 1 : -1];

/* ===========================================================================
 * svt_aom_sig_deriv_enc_dec_default (enc_mode_config.c:7815) -- the PD1
 * per-SB signal set for VIDEO.
 *
 * EXPORTED, and it reaches a dozen file-`static` tables by passing one
 * picture-level level into each. Driving it on a synthetic PCS and reading the
 * ModeDecisionContext back therefore gates those statics at TIER 1.
 *
 * Deref safety, checked against the C bodies rather than assumed:
 *  - set_nsq_search_ctrls indexes ppcs->me_8x8_distortion[sb_index] and
 *    ppcs->me_8x8_cost_variance[sb_index] when the picture is not an I-slice
 *    and nsq_search_level != 0, so both arrays are allocated here.
 *  - get_sb_tpl_intra_stats (reached from set_intra_ctrls) is gated on
 *    ppcs->tpl_ctrls.enable, which stays 0, so pa_me_data is never touched.
 *  - super_block_size is forced to 64 so the SB128 branch (get_sb128_me_data,
 *    which walks further ppcs arrays) is not taken.
 * ======================================================================== */

/* Input slot order; the Rust side builds the same array. */
enum {
    ED_I_ENC_MODE = 0, ED_I_IS_ISLICE, ED_I_NSQ_SEARCH, ED_I_NIC, ED_I_CAND_RED,
    ED_I_TXT, ED_I_TX_SHORTCUT, ED_I_IFS, ED_I_CHROMA, ED_I_CFL,
    ED_I_WM, ED_I_BIPRED3X3, ED_I_INTER_COMP, ED_I_REF_PRUNE, ED_I_SPATIAL_SSE,
    ED_I_RDOQ, ED_I_COEFF_SHAVE, ED_I_OBMC, ED_I_INTER_INTRA, ED_I_TXS,
    ED_I_FILTER_INTRA, ED_I_MD_SQ_MV, ED_I_MD_NSQ_MV, ED_I_MD_PME,
    ED_I_ME_SUBPEL, ED_I_PME_SUBPEL, ED_I_RATE_EST, ED_I_INTRA,
    ED_I_DIST_ANG_INTRA, ED_I_MDS0, ED_I_UPDATE_TYPE, ED_I_ME_8X8_DIST,
    ED_I_ME_8X8_VAR, ED_I_UNIPRED3X3, ED_I_NN_COMB, ED_I_APPROX_INTER_RATE,
    ED_I_ALLOW_INTRABC, ED_I_PALETTE_LEVEL, ED_I_GM_ENABLED, ED_I_PICTURE_QP,
    ED_I_REF_SKIP_PERC,
    /* set_cand_reduction_ctrls reads these six beyond its level. */
    ED_I_RTC, ED_I_HIER_LEVELS, ED_I_LPD1_PD1_LEVEL,
    ED_I_REF_L0_TRY, ED_I_REF_L1_TRY, ED_I_PPCS_BEST_UNIPRED,
    ED_I_COUNT
};

/* Output slot order. */
enum {
    ED_O_SUBRES_STEP = 0, ED_O_SUBRES_DEV,
    ED_O_PF_SHAPE,
    ED_O_SSSE_LEVEL,
    ED_O_TXSC_BYPASS, ED_O_TXSC_PF, ED_O_TXSC_CHROMA, ED_O_TXSC_MDS3,
    ED_O_SHAVE_EN, ED_O_SHAVE_LVL, ED_O_SHAVE_GAP, ED_O_SHAVE_RD,
    ED_O_DEE_SPLIT, ED_O_DEE_EXIT,
    ED_O_SSD_EN, ED_O_SSD_MAX, ED_O_SSD_QUAD, ED_O_SSD_PERC,
    ED_O_NSQME_EN, ED_O_NSQME_DIST, ED_O_NSQME_W, ED_O_NSQME_H, ED_O_NSQME_PSAD,
    ED_O_SQME_EN, ED_O_SQME_DIST, ED_O_SQME_PAME_TH,
    ED_O_SQME_L0_EN, ED_O_SQME_L0_STEP, ED_O_SQME_L0_W, ED_O_SQME_L0_H,
    ED_O_SQME_L0_MAXW, ED_O_SQME_L0_MAXH, ED_O_SQME_L0_MULT,
    ED_O_SQME_L1_EN, ED_O_SQME_L1_STEP, ED_O_SQME_L1_W, ED_O_SQME_L1_H,
    ED_O_SQME_L1_MAXW, ED_O_SQME_L1_MAXH, ED_O_SQME_L1_MULT,
    ED_O_SQME_L2_EN, ED_O_SQME_L2_STEP, ED_O_SQME_L2_W, ED_O_SQME_L2_H,
    ED_O_SQME_PSAD,
    ED_O_SPME_EN, ED_O_SPME_TYPE, ED_O_SPME_PREC, ED_O_SPME_METHOD,
    ED_O_SPME_ITERS, ED_O_SPME_VAR_TH, ED_O_SPME_ABS_MULT, ED_O_SPME_ROUND_DEV,
    ED_O_SPME_DIAG, ED_O_SPME_MINBLK, ED_O_SPME_MVP_TH, ED_O_SPME_HP_TH, ED_O_SPME_BIAS,
    ED_O_SPPME_EN, ED_O_SPPME_TYPE, ED_O_SPPME_PREC, ED_O_SPPME_METHOD,
    ED_O_SPPME_ITERS, ED_O_SPPME_VAR_TH, ED_O_SPPME_ABS_MULT, ED_O_SPPME_ROUND_DEV,
    ED_O_SPPME_DIAG, ED_O_SPPME_MINBLK, ED_O_SPPME_MVP_TH, ED_O_SPPME_HP_TH, ED_O_SPPME_BIAS,
    ED_O_OBMC_EN, ED_O_OBMC_REFINE_SZ, ED_O_OBMC_MAX_SZ, ED_O_OBMC_REFINE_LVL,
    ED_O_OBMC_FACEOFF, ED_O_OBMC_RANGE, ED_O_OBMC_DIAG,
    ED_O_II_EN, ED_O_II_RD, ED_O_II_WSQ, ED_O_II_WNSQ,
    ED_O_IC_TOT, ED_O_IC_ME, ED_O_IC_PME, ED_O_IC_NN, ED_O_IC_NEAR,
    ED_O_IC_NNN, ED_O_IC_3X3, ED_O_IC_GLOBAL, ED_O_IC_MULT, ED_O_IC_MAXMV,
    ED_O_IC_SKIPREF, ED_O_IC_RATE, ED_O_IC_NOSYM,
    ED_O_IFS_LEVEL,
    ED_O_GM_INJ, ED_O_NN_INJ, ED_O_NNC_INJ, ED_O_UNI3X3_INJ,
    ED_O_ALLOW_IBC, ED_O_PALETTE_LVL, ED_O_APPROX_RATE, ED_O_SHUT_FAST_RATE,
    ED_O_MDS0_HADAMARD, ED_O_PARENT_COST_BIAS, ED_O_TUNE_SSIM,
    ED_O_UV_MODE,
    /* cand_reduction_ctrls */
    ED_O_CR_RED_SCORE, ED_O_CR_RED_MAG, ED_O_CR_NEAR_EN, ED_O_CR_NEAR_CNT,
    ED_O_CR_NEARNEAR_CNT, ED_O_CR_LPD1_MVP, ED_O_CR_USE_NEIGH,
    ED_O_CR_ELIM_EN, ED_O_CR_ELIM_DC_TH, ED_O_CR_ELIM_SKIP_TH,
    ED_O_CR_REDUCE_UNI,
    ED_O_COUNT
};

void ref_sig_deriv_enc_dec_default(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs    = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs   = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs    = (PictureControlSet*)calloc(1, sizeof(*pcs));
    ModeDecisionContext*     ctx    = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    uint32_t*                me_d   = (uint32_t*)calloc(1, sizeof(uint32_t));
    uint32_t*                me_v   = (uint32_t*)calloc(1, sizeof(uint32_t));

    scs->super_block_size  = 64; /* keep the SB128 me-data branch out of reach */
    /* rtc is read ONLY by set_cand_reduction_ctrls's use_flat_ipp on this
       path, so it is an input rather than a constant. */
    scs->static_config.rtc = (bool)in[ED_I_RTC];

    ppcs->scs                  = scs;
    ppcs->picture_qp           = (uint32_t)in[ED_I_PICTURE_QP];
    ppcs->update_type          = (SvtAv1FrameUpdateType)in[ED_I_UPDATE_TYPE];
    ppcs->hierarchical_levels  = (uint8_t)in[ED_I_HIER_LEVELS];
    ppcs->ref_list0_count_try  = (uint8_t)in[ED_I_REF_L0_TRY];
    ppcs->ref_list1_count_try  = (uint8_t)in[ED_I_REF_L1_TRY];
    ppcs->use_best_me_unipred_cand_only = (uint8_t)in[ED_I_PPCS_BEST_UNIPRED];
    ppcs->gm_ctrls.enabled     = (uint8_t)in[ED_I_GM_ENABLED];
    ppcs->pic_obmc_level       = (uint8_t)in[ED_I_OBMC];
    ppcs->frm_hdr.allow_intrabc = (uint8_t)in[ED_I_ALLOW_INTRABC];
    ppcs->palette_level        = (uint8_t)in[ED_I_PALETTE_LEVEL];
    me_d[0]                    = (uint32_t)in[ED_I_ME_8X8_DIST];
    me_v[0]                    = (uint32_t)in[ED_I_ME_8X8_VAR];
    ppcs->me_8x8_distortion    = me_d;
    ppcs->me_8x8_cost_variance = me_v;

    pcs->ppcs       = ppcs;
    pcs->scs        = scs;
    pcs->enc_mode   = (EncMode)in[ED_I_ENC_MODE];
    pcs->slice_type = in[ED_I_IS_ISLICE] ? I_SLICE : B_SLICE;
    pcs->ref_skip_percentage        = (uint8_t)in[ED_I_REF_SKIP_PERC];
    pcs->nsq_search_level           = (uint8_t)in[ED_I_NSQ_SEARCH];
    pcs->nic_level                  = (uint8_t)in[ED_I_NIC];
    pcs->cand_reduction_level       = (uint8_t)in[ED_I_CAND_RED];
    pcs->txt_level                  = (uint8_t)in[ED_I_TXT];
    pcs->tx_shortcut_level          = (uint8_t)in[ED_I_TX_SHORTCUT];
    pcs->interpolation_search_level = (uint8_t)in[ED_I_IFS];
    pcs->chroma_level               = (uint8_t)in[ED_I_CHROMA];
    pcs->cfl_level                  = (uint8_t)in[ED_I_CFL];
    pcs->wm_level                   = (uint8_t)in[ED_I_WM];
    pcs->bipred3x3_injection        = (uint8_t)in[ED_I_BIPRED3X3];
    pcs->unipred3x3_injection       = (uint8_t)in[ED_I_UNIPRED3X3];
    pcs->new_nearest_near_comb_injection = (uint8_t)in[ED_I_NN_COMB];
    pcs->inter_compound_mode        = (uint8_t)in[ED_I_INTER_COMP];
    pcs->dist_based_ref_pruning     = (uint8_t)in[ED_I_REF_PRUNE];
    pcs->spatial_sse_full_loop_level = (uint8_t)in[ED_I_SPATIAL_SSE];
    pcs->rdoq_level                 = (uint8_t)in[ED_I_RDOQ];
    pcs->coeff_shaving_level        = (uint8_t)in[ED_I_COEFF_SHAVE];
    pcs->inter_intra_level          = (uint8_t)in[ED_I_INTER_INTRA];
    pcs->txs_level                  = (uint8_t)in[ED_I_TXS];
    pcs->pic_filter_intra_level     = (uint8_t)in[ED_I_FILTER_INTRA];
    pcs->md_sq_mv_search_level      = (uint8_t)in[ED_I_MD_SQ_MV];
    pcs->md_nsq_mv_search_level     = (uint8_t)in[ED_I_MD_NSQ_MV];
    pcs->md_pme_level               = (uint8_t)in[ED_I_MD_PME];
    pcs->me_subpel_level            = (uint8_t)in[ED_I_ME_SUBPEL];
    pcs->pme_subpel_level           = (uint8_t)in[ED_I_PME_SUBPEL];
    pcs->rate_est_level             = (uint8_t)in[ED_I_RATE_EST];
    pcs->approx_inter_rate          = (uint8_t)in[ED_I_APPROX_INTER_RATE];
    pcs->intra_level                = (uint8_t)in[ED_I_INTRA];
    pcs->dist_based_ang_intra_level = (uint8_t)in[ED_I_DIST_ANG_INTRA];
    pcs->mds0_level                 = (uint8_t)in[ED_I_MDS0];

    /* is_lpd1 inside set_cand_reduction_ctrls is `pd1_level > REGULAR_PD1`,
       and REGULAR_PD1 is -1 -- so a calloc'd context (pd1_level 0) is ALREADY
       "lpd1". Set it explicitly so both arms are reachable. */
    ctx->lpd1_ctrls.pd1_level = (Pd1Level)in[ED_I_LPD1_PD1_LEVEL];

    svt_aom_sig_deriv_enc_dec_default(pcs, ctx);

    out[ED_O_SUBRES_STEP] = ctx->subres_ctrls.step;
    out[ED_O_SUBRES_DEV]  = ctx->subres_ctrls.odd_to_even_deviation_th;
    out[ED_O_PF_SHAPE]    = ctx->pf_ctrls.pf_shape;
    out[ED_O_SSSE_LEVEL]  = ctx->spatial_sse_ctrls.level;
    out[ED_O_TXSC_BYPASS] = ctx->tx_shortcut_ctrls.bypass_tx_th;
    out[ED_O_TXSC_PF]     = ctx->tx_shortcut_ctrls.apply_pf_on_coeffs;
    out[ED_O_TXSC_CHROMA] = ctx->tx_shortcut_ctrls.chroma_detector_level;
    out[ED_O_TXSC_MDS3]   = ctx->tx_shortcut_ctrls.use_mds3_shortcuts_th;
    out[ED_O_SHAVE_EN]    = ctx->coeff_shaving_ctrls.enabled;
    out[ED_O_SHAVE_LVL]   = ctx->coeff_shaving_ctrls.level_threshold;
    out[ED_O_SHAVE_GAP]   = ctx->coeff_shaving_ctrls.zero_gap_threshold;
    out[ED_O_SHAVE_RD]    = ctx->coeff_shaving_ctrls.rd_zero_strength;
    out[ED_O_DEE_SPLIT]   = ctx->depth_early_exit_ctrls.split_cost_th;
    out[ED_O_DEE_EXIT]    = ctx->depth_early_exit_ctrls.early_exit_th;
    out[ED_O_SSD_EN]      = ctx->skip_sub_depth_ctrls.enabled;
    out[ED_O_SSD_MAX]     = ctx->skip_sub_depth_ctrls.max_size;
    out[ED_O_SSD_QUAD]    = ctx->skip_sub_depth_ctrls.quad_deviation_th;
    out[ED_O_SSD_PERC]    = ctx->skip_sub_depth_ctrls.coeff_perc;
    out[ED_O_NSQME_EN]    = ctx->md_nsq_me_ctrls.enabled;
    out[ED_O_NSQME_DIST]  = ctx->md_nsq_me_ctrls.dist_type;
    out[ED_O_NSQME_W]     = ctx->md_nsq_me_ctrls.full_pel_search_width;
    out[ED_O_NSQME_H]     = ctx->md_nsq_me_ctrls.full_pel_search_height;
    out[ED_O_NSQME_PSAD]  = ctx->md_nsq_me_ctrls.enable_psad;
    out[ED_O_SQME_EN]       = ctx->md_sq_me_ctrls.enabled;
    out[ED_O_SQME_DIST]     = ctx->md_sq_me_ctrls.dist_type;
    out[ED_O_SQME_PAME_TH]  = ctx->md_sq_me_ctrls.pame_distortion_th;
    out[ED_O_SQME_L0_EN]    = ctx->md_sq_me_ctrls.sprs_lev0_enabled;
    out[ED_O_SQME_L0_STEP]  = ctx->md_sq_me_ctrls.sprs_lev0_step;
    out[ED_O_SQME_L0_W]     = ctx->md_sq_me_ctrls.sprs_lev0_w;
    out[ED_O_SQME_L0_H]     = ctx->md_sq_me_ctrls.sprs_lev0_h;
    out[ED_O_SQME_L0_MAXW]  = ctx->md_sq_me_ctrls.max_sprs_lev0_w;
    out[ED_O_SQME_L0_MAXH]  = ctx->md_sq_me_ctrls.max_sprs_lev0_h;
    out[ED_O_SQME_L0_MULT]  = ctx->md_sq_me_ctrls.sprs_lev0_multiplier;
    out[ED_O_SQME_L1_EN]    = ctx->md_sq_me_ctrls.sprs_lev1_enabled;
    out[ED_O_SQME_L1_STEP]  = ctx->md_sq_me_ctrls.sprs_lev1_step;
    out[ED_O_SQME_L1_W]     = ctx->md_sq_me_ctrls.sprs_lev1_w;
    out[ED_O_SQME_L1_H]     = ctx->md_sq_me_ctrls.sprs_lev1_h;
    out[ED_O_SQME_L1_MAXW]  = ctx->md_sq_me_ctrls.max_sprs_lev1_w;
    out[ED_O_SQME_L1_MAXH]  = ctx->md_sq_me_ctrls.max_sprs_lev1_h;
    out[ED_O_SQME_L1_MULT]  = ctx->md_sq_me_ctrls.sprs_lev1_multiplier;
    out[ED_O_SQME_L2_EN]    = ctx->md_sq_me_ctrls.sprs_lev2_enabled;
    out[ED_O_SQME_L2_STEP]  = ctx->md_sq_me_ctrls.sprs_lev2_step;
    out[ED_O_SQME_L2_W]     = ctx->md_sq_me_ctrls.sprs_lev2_w;
    out[ED_O_SQME_L2_H]     = ctx->md_sq_me_ctrls.sprs_lev2_h;
    out[ED_O_SQME_PSAD]     = ctx->md_sq_me_ctrls.enable_psad;
#define DUMP_SUBPEL(base, c)                                     \
    out[base + 0]  = (c).enabled;                                \
    out[base + 1]  = (c).subpel_search_type;                     \
    out[base + 2]  = (c).max_precision;                          \
    out[base + 3]  = (c).subpel_search_method;                   \
    out[base + 4]  = (c).subpel_iters_per_step;                  \
    out[base + 5]  = (c).pred_variance_th;                       \
    out[base + 6]  = (c).abs_th_mult;                            \
    out[base + 7]  = (c).round_dev_th;                           \
    out[base + 8]  = (c).skip_diag_refinement;                   \
    out[base + 9]  = (c).min_blk_sz;                             \
    out[base + 10] = (c).mvp_th;                                 \
    out[base + 11] = (c).hp_mv_th;                               \
    out[base + 12] = (c).bias_fp
    DUMP_SUBPEL(ED_O_SPME_EN, ctx->md_subpel_me_ctrls);
    DUMP_SUBPEL(ED_O_SPPME_EN, ctx->md_subpel_pme_ctrls);
#undef DUMP_SUBPEL
    out[ED_O_OBMC_EN]         = ctx->obmc_ctrls.enabled;
    out[ED_O_OBMC_REFINE_SZ]  = ctx->obmc_ctrls.max_blk_size_to_refine;
    out[ED_O_OBMC_MAX_SZ]     = ctx->obmc_ctrls.max_blk_size;
    out[ED_O_OBMC_REFINE_LVL] = ctx->obmc_ctrls.refine_level;
    out[ED_O_OBMC_FACEOFF]    = ctx->obmc_ctrls.trans_face_off;
    out[ED_O_OBMC_RANGE]      = ctx->obmc_ctrls.fpel_search_range;
    out[ED_O_OBMC_DIAG]       = ctx->obmc_ctrls.fpel_search_diag;
    out[ED_O_II_EN]   = ctx->inter_intra_comp_ctrls.enabled;
    out[ED_O_II_RD]   = ctx->inter_intra_comp_ctrls.use_rd_model;
    out[ED_O_II_WSQ]  = ctx->inter_intra_comp_ctrls.wedge_mode_sq;
    out[ED_O_II_WNSQ] = ctx->inter_intra_comp_ctrls.wedge_mode_nsq;
    out[ED_O_IC_TOT]     = ctx->inter_comp_ctrls.tot_comp_types;
    out[ED_O_IC_ME]      = ctx->inter_comp_ctrls.do_me;
    out[ED_O_IC_PME]     = ctx->inter_comp_ctrls.do_pme;
    out[ED_O_IC_NN]      = ctx->inter_comp_ctrls.do_nearest_nearest;
    out[ED_O_IC_NEAR]    = ctx->inter_comp_ctrls.do_near_near;
    out[ED_O_IC_NNN]     = ctx->inter_comp_ctrls.do_nearest_near_new;
    out[ED_O_IC_3X3]     = ctx->inter_comp_ctrls.do_3x3_bi;
    out[ED_O_IC_GLOBAL]  = ctx->inter_comp_ctrls.do_global;
    out[ED_O_IC_MULT]    = ctx->inter_comp_ctrls.pred0_to_pred1_mult;
    out[ED_O_IC_MAXMV]   = ctx->inter_comp_ctrls.max_mv_length;
    out[ED_O_IC_SKIPREF] = ctx->inter_comp_ctrls.skip_on_ref_info;
    out[ED_O_IC_RATE]    = ctx->inter_comp_ctrls.use_rate;
    out[ED_O_IC_NOSYM]   = ctx->inter_comp_ctrls.no_sym_dist;
    out[ED_O_IFS_LEVEL]  = ctx->ifs_ctrls.level;
    out[ED_O_GM_INJ]     = ctx->global_mv_injection;
    out[ED_O_NN_INJ]     = ctx->new_nearest_injection;
    out[ED_O_NNC_INJ]    = ctx->new_nearest_near_comb_injection;
    out[ED_O_UNI3X3_INJ] = ctx->unipred3x3_injection;
    out[ED_O_ALLOW_IBC]  = ctx->md_allow_intrabc;
    out[ED_O_PALETTE_LVL] = ctx->md_palette_level;
    out[ED_O_APPROX_RATE] = ctx->approx_inter_rate;
    out[ED_O_SHUT_FAST_RATE] = ctx->shut_fast_rate;
    out[ED_O_MDS0_HADAMARD]  = ctx->mds0_use_hadamard_sb;
    out[ED_O_PARENT_COST_BIAS] = ctx->parent_cost_bias;
    out[ED_O_TUNE_SSIM]        = ctx->tune_ssim_level;
    /* uv_mode comes from svt_aom_set_chroma_controls, a table this lane has
       NOT ported; it is exported so the Rust side can feed C's value back in
       for the blk_skip_decision check rather than guessing it. */
    out[ED_O_UV_MODE] = ctx->uv_ctrls.uv_mode;
    out[ED_O_CR_RED_SCORE]    = ctx->cand_reduction_ctrls.redundant_cand_ctrls.score_th;
    out[ED_O_CR_RED_MAG]      = ctx->cand_reduction_ctrls.redundant_cand_ctrls.mag_th;
    out[ED_O_CR_NEAR_EN]      = ctx->cand_reduction_ctrls.near_count_ctrls.enabled;
    out[ED_O_CR_NEAR_CNT]     = ctx->cand_reduction_ctrls.near_count_ctrls.near_count;
    out[ED_O_CR_NEARNEAR_CNT] = ctx->cand_reduction_ctrls.near_count_ctrls.near_near_count;
    out[ED_O_CR_LPD1_MVP]     = ctx->cand_reduction_ctrls.lpd1_mvp_best_me_list;
    out[ED_O_CR_USE_NEIGH]    = ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled;
    out[ED_O_CR_ELIM_EN]      = ctx->cand_reduction_ctrls.cand_elimination_ctrls.enabled;
    out[ED_O_CR_ELIM_DC_TH]   = ctx->cand_reduction_ctrls.cand_elimination_ctrls.dc_only_th;
    out[ED_O_CR_ELIM_SKIP_TH] = ctx->cand_reduction_ctrls.cand_elimination_ctrls.skip_dc_th;
    out[ED_O_CR_REDUCE_UNI]   = ctx->cand_reduction_ctrls.reduce_unipred_candidates;

    free(me_v);
    free(me_d);
    free(ctx);
    free(pcs);
    free(ppcs);
    free(scs);
}

int32_t ref_enc_dec_default_in_slots(void) { return ED_I_COUNT; }
int32_t ref_enc_dec_default_out_slots(void) { return ED_O_COUNT; }

/* ===========================================================================
 * svt_aom_sig_deriv_enc_dec_pd0 (enc_mode_config.c:7207) -- the per-SB PD0
 * signal set, shared by ALL THREE arms.
 *
 * Deref safety, from the C body:
 *  - the SB128 branch (get_sb128_me_data) is avoided by leaving
 *    scs->seq_header.sb_size at BLOCK_64X64.
 *  - the three ppcs me arrays and b64_geom are indexed at sb_index 0, so one
 *    element of each is allocated.
 *  - set_intra_ctrls reaches get_sb_tpl_intra_stats, gated on
 *    ppcs->tpl_ctrls.enable, which stays 0.
 * ======================================================================== */

enum {
    PD0_I_LEVEL = 0, PD0_I_IS_ISLICE, PD0_I_ALLINTRA, PD0_I_RTC, PD0_I_UPDATE_TYPE,
    PD0_I_ENC_MODE, PD0_I_TRANSITION, PD0_I_PRED_DEPTH_ONLY, PD0_I_CTX_HBD,
    PD0_I_PCS_HBD, PD0_I_LAMBDA8, PD0_I_LAMBDA10, PD0_I_ME64_DIST,
    PD0_I_ME8_VAR, PD0_I_ME8_DIST, PD0_I_BASE_Q, PD0_I_BIAS_WEIGHT,
    PD0_I_RATE_EST, PD0_I_DISALLOW_4X4, PD0_I_DISALLOW_8X8,
    PD0_I_DR_ENABLED, PD0_I_DR_B16, PD0_I_DR_B32, PD0_I_DR_B64,
    PD0_I_B64_COMPLETE, PD0_I_SB_SIZE, PD0_I_COUNT
};

enum {
    PD0_O_NSQ_OFF = 0, PD0_O_SHUT_FAST_RATE,
    PD0_O_DEE_SPLIT, PD0_O_DEE_EXIT,
    PD0_O_PARENT_BIAS, PD0_O_USE_SRC,
    PD0_O_PF_SHAPE, PD0_O_SUBRES_STEP, PD0_O_SUBRES_DEV,
    PD0_O_APPROX_RATE, PD0_O_UV_MODE,
    /* rate_est_ctrls */
    PD0_O_RE_SKIPCTX, PD0_O_RE_SKIPCOEFF, PD0_O_RE_COEFF_LVL,
    PD0_O_RE_QP_OFFSET, PD0_O_RE_FAST_EST,
    /* intra_ctrls -- from the UNPORTED set_intra_ctrls; the Rust side
       cross-checks its own derived intra_level by driving C's set_intra_ctrls
       through the enc_dec_default entry point at that level. */
    PD0_O_IC_EN, PD0_O_IC_MODE_END, PD0_O_IC_ANG, PD0_O_IC_PRUNE_BEST,
    PD0_O_IC_PRUNE_EDGE, PD0_O_IC_D1, PD0_O_IC_D2, PD0_O_IC_D3,
    PD0_O_COUNT
};

void ref_sig_deriv_enc_dec_pd0(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    uint32_t* me64 = (uint32_t*)calloc(1, sizeof(uint32_t));
    uint32_t* me8v = (uint32_t*)calloc(1, sizeof(uint32_t));
    uint32_t* me8d = (uint32_t*)calloc(1, sizeof(uint32_t));
    B64Geom*  b64  = (B64Geom*)calloc(1, sizeof(B64Geom));

    scs->allintra            = (bool)in[PD0_I_ALLINTRA];
    scs->static_config.rtc   = (bool)in[PD0_I_RTC];
    scs->super_block_size    = (uint32_t)in[PD0_I_SB_SIZE];
    scs->seq_header.sb_size  = BLOCK_64X64; /* keep get_sb128_me_data unreachable */

    me64[0] = (uint32_t)in[PD0_I_ME64_DIST];
    me8v[0] = (uint32_t)in[PD0_I_ME8_VAR];
    me8d[0] = (uint32_t)in[PD0_I_ME8_DIST];
    b64->is_complete_b64 = (uint8_t)in[PD0_I_B64_COMPLETE];

    ppcs->scs                   = scs;
    ppcs->update_type           = (SvtAv1FrameUpdateType)in[PD0_I_UPDATE_TYPE];
    ppcs->transition_present    = (int8_t)in[PD0_I_TRANSITION];
    ppcs->me_64x64_distortion   = me64;
    ppcs->me_8x8_cost_variance  = me8v;
    ppcs->me_8x8_distortion     = me8d;
    ppcs->b64_geom              = b64;
    ppcs->frm_hdr.quantization_params.base_q_idx = (int32_t)in[PD0_I_BASE_Q];

    pcs->ppcs                 = ppcs;
    pcs->scs                  = scs;
    pcs->enc_mode             = (EncMode)in[PD0_I_ENC_MODE];
    pcs->slice_type           = in[PD0_I_IS_ISLICE] ? I_SLICE : B_SLICE;
    pcs->hbd_md               = (uint8_t)in[PD0_I_PCS_HBD];
    pcs->pd0_cost_bias_weight = (uint32_t)in[PD0_I_BIAS_WEIGHT];
    pcs->rate_est_level       = (uint8_t)in[PD0_I_RATE_EST];

    ctx->pd0_ctrls.pd0_level   = (Pd0Level)in[PD0_I_LEVEL];
    ctx->pic_pred_depth_only   = (bool)in[PD0_I_PRED_DEPTH_ONLY];
    ctx->hbd_md                = (uint8_t)in[PD0_I_CTX_HBD];
    ctx->fast_lambda_md[EB_8_BIT_MD]  = (uint32_t)in[PD0_I_LAMBDA8];
    ctx->fast_lambda_md[EB_10_BIT_MD] = (uint32_t)in[PD0_I_LAMBDA10];
    ctx->disallow_4x4          = (bool)in[PD0_I_DISALLOW_4X4];
    ctx->disallow_8x8          = (bool)in[PD0_I_DISALLOW_8X8];
    ctx->depth_removal_ctrls.enabled              = (uint8_t)in[PD0_I_DR_ENABLED];
    ctx->depth_removal_ctrls.disallow_below_16x16 = (uint8_t)in[PD0_I_DR_B16];
    ctx->depth_removal_ctrls.disallow_below_32x32 = (uint8_t)in[PD0_I_DR_B32];
    ctx->depth_removal_ctrls.disallow_below_64x64 = (uint8_t)in[PD0_I_DR_B64];

    svt_aom_sig_deriv_enc_dec_pd0(scs, pcs, ctx);

    out[PD0_O_NSQ_OFF]         = ctx->md_disallow_nsq_search;
    out[PD0_O_SHUT_FAST_RATE]  = ctx->shut_fast_rate;
    out[PD0_O_DEE_SPLIT]       = ctx->depth_early_exit_ctrls.split_cost_th;
    out[PD0_O_DEE_EXIT]        = ctx->depth_early_exit_ctrls.early_exit_th;
    out[PD0_O_PARENT_BIAS]     = ctx->parent_cost_bias;
    out[PD0_O_USE_SRC]         = ctx->pd0_use_src_samples;
    out[PD0_O_PF_SHAPE]        = ctx->pf_ctrls.pf_shape;
    out[PD0_O_SUBRES_STEP]     = ctx->subres_ctrls.step;
    out[PD0_O_SUBRES_DEV]      = ctx->subres_ctrls.odd_to_even_deviation_th;
    out[PD0_O_APPROX_RATE]     = ctx->approx_inter_rate;
    out[PD0_O_UV_MODE]         = ctx->uv_ctrls.uv_mode;
    out[PD0_O_RE_SKIPCTX]      = ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx;
    out[PD0_O_RE_SKIPCOEFF]    = ctx->rate_est_ctrls.update_skip_coeff_ctx;
    out[PD0_O_RE_COEFF_LVL]    = ctx->rate_est_ctrls.coeff_rate_est_lvl;
    out[PD0_O_RE_QP_OFFSET]    = ctx->rate_est_ctrls.lpd0_qp_offset;
    out[PD0_O_RE_FAST_EST]     = ctx->rate_est_ctrls.pd0_fast_coeff_est_level;
    out[PD0_O_IC_EN]           = ctx->intra_ctrls.enable_intra;
    out[PD0_O_IC_MODE_END]     = ctx->intra_ctrls.intra_mode_end;
    out[PD0_O_IC_ANG]          = ctx->intra_ctrls.angular_pred_level;
    out[PD0_O_IC_PRUNE_BEST]   = ctx->intra_ctrls.prune_using_best_mode;
    out[PD0_O_IC_PRUNE_EDGE]   = ctx->intra_ctrls.prune_using_edge_info;
    out[PD0_O_IC_D1]           = ctx->intra_ctrls.skip_angular_delta1_th;
    out[PD0_O_IC_D2]           = ctx->intra_ctrls.skip_angular_delta2_th;
    out[PD0_O_IC_D3]           = ctx->intra_ctrls.skip_angular_delta3_th;

    free(b64); free(me8d); free(me8v); free(me64);
    free(ctx); free(pcs); free(ppcs); free(scs);
}

/* Drive C's set_intra_ctrls alone, at a caller-chosen level, so the Rust side
   can validate its own derived intra_level without transcribing that table.
   set_intra_ctrls is `static`; svt_aom_sig_deriv_enc_dec_pd0 is the exported
   entry point that reaches it with a level the caller controls, via
   pd0_level == PD0_LVL_0 (intra_level = MAX_INTRA_LEVEL-1), an I-slice
   (intra_level = 1) and so on -- which is not enough. Use the pd0 path with
   an explicit override instead: the shim sets ctx->intra_ctrls from the C
   table by calling the exported enc_dec_default entry point, whose
   pcs->intra_level IS a direct input. */
void ref_set_intra_ctrls_via_enc_dec_default(int32_t intra_level, int32_t dist_ang_level,
                                             int32_t is_islice, int64_t out[8]) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    uint32_t* me_d = (uint32_t*)calloc(1, sizeof(uint32_t));
    uint32_t* me_v = (uint32_t*)calloc(1, sizeof(uint32_t));

    scs->super_block_size      = 64;
    ppcs->scs                  = scs;
    ppcs->hierarchical_levels  = 4;
    ppcs->me_8x8_distortion    = me_d;
    ppcs->me_8x8_cost_variance = me_v;
    pcs->ppcs       = ppcs;
    pcs->scs        = scs;
    pcs->enc_mode   = ENC_M5;
    pcs->slice_type = is_islice ? I_SLICE : B_SLICE;
    pcs->intra_level                = (uint8_t)intra_level;
    pcs->dist_based_ang_intra_level = (uint8_t)dist_ang_level;
    /* every other level stays 0, which is in-domain for each table */
    svt_aom_sig_deriv_enc_dec_default(pcs, ctx);
    out[0] = ctx->intra_ctrls.enable_intra;
    out[1] = ctx->intra_ctrls.intra_mode_end;
    out[2] = ctx->intra_ctrls.angular_pred_level;
    out[3] = ctx->intra_ctrls.prune_using_best_mode;
    out[4] = ctx->intra_ctrls.prune_using_edge_info;
    out[5] = ctx->intra_ctrls.skip_angular_delta1_th;
    out[6] = ctx->intra_ctrls.skip_angular_delta2_th;
    out[7] = ctx->intra_ctrls.skip_angular_delta3_th;
    free(me_v); free(me_d); free(ctx); free(pcs); free(ppcs); free(scs);
}

int32_t ref_pd0_in_slots(void) { return PD0_I_COUNT; }
int32_t ref_pd0_out_slots(void) { return PD0_O_COUNT; }
