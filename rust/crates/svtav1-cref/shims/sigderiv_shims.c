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
