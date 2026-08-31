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
