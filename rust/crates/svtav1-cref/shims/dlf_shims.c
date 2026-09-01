/*
 * C shim for the DEBLOCK signal derivation of
 * `Source/Lib/Codec/enc_mode_config.c` (port: `port_enc_mode_config::leaf`'s
 * `get_dlf_level_*` ladders + `port_enc_mode_config::ctrls::set_dlf_controls`).
 *
 * WHY ITS OWN TU: per-lane shim-file ownership (see `build.rs`) -- the
 * sigderiv lane owns `sigderiv_shims.c` and deliberately pins
 * `enable_dlf_flag = 0` there so its md-config differential does not vary the
 * deblocking path. This lane needs the opposite, so it gets its own file
 * rather than editing that one.
 *
 * EVIDENCE TIER 1. `get_dlf_level_default` / `get_dlf_level_allintra` /
 * `dlf_level_modulation` / `svt_aom_set_dlf_controls` are all file-`static`
 * (the `svt_aom_` prefix on the last one notwithstanding), so none can be
 * called directly -- but the EXPORTED
 * `svt_aom_sig_deriv_mode_decision_config_{default,allintra}` reach every one
 * of them and leave the result in `ppcs->dlf_ctrls`, which this shim reads
 * back. So the differential drives the real C ladder AND the real C controls
 * table; it is not a transcription agreeing with a transcription.
 *
 * The `dlf_level` itself is never stored, only the controls -- but the eight
 * control fields are distinct for every one of the eight levels, so the
 * composite is fully observable.
 *
 * RULE, inherited from ref_shims.c: A SHIM MUST NOT KEEP PER-CALL STATE IN A
 * `static`. Every synthetic struct below is calloc/free per call.
 *
 * The control-set population is copied verbatim from
 * `sigderiv_shims.c`'s `ref_sig_deriv_md_config_default` (whose deref-safety
 * analysis is recorded in that file's block comment above it) with two fields
 * promoted to inputs: `enable_dlf_flag` and `frm_hdr.allow_intrabc`, the two
 * halves of C's `if (enable_dlf_flag && allow_intrabc == 0)` guard.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "enc_mode_config.h"

enum {
    DLF_I_ENC_MODE = 0, DLF_I_IS_REF, DLF_I_TEMPORAL_LAYER, DLF_I_INPUT_RES,
    DLF_I_IS_ISLICE, DLF_I_SC_CLASS5, DLF_I_FAST_DECODE, DLF_I_HIER_LEVELS,
    DLF_I_TRANSITION, DLF_I_IS_HIGHEST_LAYER, DLF_I_SQ_QP, DLF_I_MFMV_ENABLED,
    DLF_I_ERROR_RESILIENT, DLF_I_BASE_Q, DLF_I_REF_HP_PERC, DLF_I_SCS_INPUT_RES,
    DLF_I_FRAME_IS_INTRA, DLF_I_SUPERRES, DLF_I_RESIZE_ENABLED, DLF_I_SEQ_QP_MOD,
    DLF_I_RESIZE_MODE, DLF_I_REF_INTRA_PERC, DLF_I_RC_STAT_GEN, DLF_I_REF_SKIP_PERC,
    DLF_I_COEFF_LVL, DLF_I_REF_L0_TRY, DLF_I_REF_L1_TRY, DLF_I_ENABLE_II,
    DLF_I_BIT_DEPTH, DLF_I_SEGMENTATION, DLF_I_SB_SIZE, DLF_I_HBD_MD,
    DLF_I_R0_GEN, DLF_I_R0_MILLI, DLF_I_PCS_TEMPORAL_LAYER, DLF_I_TUNE,
    DLF_I_PICTURE_QP, DLF_I_EXT_CRF_OFFSET,
    DLF_I_ENABLE_DLF_FLAG, DLF_I_ALLOW_INTRABC,
    DLF_I_COUNT
};
enum {
    DLF_O_ENABLED = 0, DLF_O_SB_BASED, DLF_O_AVG, DLF_O_USE_REF_AVG_Y,
    DLF_O_USE_REF_AVG_UV, DLF_O_EARLY_EXIT, DLF_O_ZERO_FILT_STRENGTH,
    DLF_O_PREV_DIST_TH,
    DLF_O_COUNT
};

static void dlf_read_out(PictureParentControlSet* ppcs, int64_t* out) {
    out[DLF_O_ENABLED]              = ppcs->dlf_ctrls.enabled;
    out[DLF_O_SB_BASED]             = ppcs->dlf_ctrls.sb_based_dlf;
    out[DLF_O_AVG]                  = ppcs->dlf_ctrls.dlf_avg;
    out[DLF_O_USE_REF_AVG_Y]        = ppcs->dlf_ctrls.use_ref_avg_y;
    out[DLF_O_USE_REF_AVG_UV]       = ppcs->dlf_ctrls.use_ref_avg_uv;
    out[DLF_O_EARLY_EXIT]           = ppcs->dlf_ctrls.early_exit_convergence;
    out[DLF_O_ZERO_FILT_STRENGTH]   = ppcs->dlf_ctrls.zero_filter_strength_lvl;
    out[DLF_O_PREV_DIST_TH]         = ppcs->dlf_ctrls.prev_dlf_dist_th;
}

/* The VIDEO arm: svt_aom_sig_deriv_mode_decision_config_default
 * (enc_mode_config.c:8900) -> get_dlf_level_default (:1466). */
void ref_dlf_ctrls_default(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    EbObjectWrapper*   w0 = (EbObjectWrapper*)calloc(1, sizeof(*w0));
    EbObjectWrapper*   w1 = (EbObjectWrapper*)calloc(1, sizeof(*w1));
    EbReferenceObject* r0 = (EbReferenceObject*)calloc(1, sizeof(*r0));
    EbReferenceObject* r1 = (EbReferenceObject*)calloc(1, sizeof(*r1));

    scs->static_config.fast_decode = (uint8_t)in[DLF_I_FAST_DECODE];
    scs->static_config.qp          = (uint32_t)in[DLF_I_SQ_QP];
    scs->static_config.resize_mode = (uint8_t)in[DLF_I_RESIZE_MODE];
    scs->static_config.encoder_bit_depth = (uint32_t)in[DLF_I_BIT_DEPTH];
    scs->static_config.enable_dlf_flag   = (uint8_t)in[DLF_I_ENABLE_DLF_FLAG];
    scs->static_config.tune              = (uint8_t)in[DLF_I_TUNE];
    scs->static_config.extended_crf_qindex_offset = (uint8_t)in[DLF_I_EXT_CRF_OFFSET];
    scs->seq_qp_mod            = (uint8_t)in[DLF_I_SEQ_QP_MOD];
    scs->mfmv_enabled          = (uint8_t)in[DLF_I_MFMV_ENABLED];
    scs->rc_stat_gen_pass_mode = (uint8_t)in[DLF_I_RC_STAT_GEN];
    scs->input_resolution      = (ResolutionRange)in[DLF_I_SCS_INPUT_RES];
    scs->super_block_size      = (uint32_t)in[DLF_I_SB_SIZE];
    scs->seq_header.enable_interintra_compound = (uint8_t)in[DLF_I_ENABLE_II];
    scs->tpl                   = 0;

    ppcs->scs                  = scs;
    ppcs->is_ref               = (bool)in[DLF_I_IS_REF];
    ppcs->temporal_layer_index = (uint8_t)in[DLF_I_TEMPORAL_LAYER];
    ppcs->input_resolution     = (ResolutionRange)in[DLF_I_INPUT_RES];
    ppcs->sc_class5            = (uint8_t)in[DLF_I_SC_CLASS5];
    ppcs->hierarchical_levels  = (uint8_t)in[DLF_I_HIER_LEVELS];
    ppcs->transition_present   = (int8_t)in[DLF_I_TRANSITION];
    ppcs->is_highest_layer     = (bool)in[DLF_I_IS_HIGHEST_LAYER];
    ppcs->frame_superres_enabled = (bool)in[DLF_I_SUPERRES];
    ppcs->frame_resize_enabled   = (bool)in[DLF_I_RESIZE_ENABLED];
    ppcs->ref_list0_count_try  = (uint8_t)in[DLF_I_REF_L0_TRY];
    ppcs->ref_list1_count_try  = (uint8_t)in[DLF_I_REF_L1_TRY];
    ppcs->hbd_md               = (int8_t)in[DLF_I_HBD_MD];
    ppcs->r0_gen               = (bool)in[DLF_I_R0_GEN];
    ppcs->r0                   = (double)in[DLF_I_R0_MILLI] / 1000.0;
    ppcs->picture_qp           = (uint8_t)in[DLF_I_PICTURE_QP];
    ppcs->frm_hdr.error_resilient_mode = (uint8_t)in[DLF_I_ERROR_RESILIENT];
    ppcs->frm_hdr.quantization_params.base_q_idx = (int32_t)in[DLF_I_BASE_Q];
    ppcs->frm_hdr.allow_intrabc = (uint8_t)in[DLF_I_ALLOW_INTRABC];
    ppcs->frm_hdr.frame_type = in[DLF_I_FRAME_IS_INTRA] ? KEY_FRAME : INTER_FRAME;
    ppcs->frm_hdr.segmentation_params.segmentation_enabled = (uint8_t)in[DLF_I_SEGMENTATION];
    ppcs->picture_sb_width  = 0;
    ppcs->picture_sb_height = 0;

    pcs->ppcs       = ppcs;
    pcs->scs        = scs;
    pcs->enc_mode   = (EncMode)in[DLF_I_ENC_MODE];
    pcs->slice_type = in[DLF_I_IS_ISLICE] ? I_SLICE : B_SLICE;
    pcs->ref_hp_percentage    = (int16_t)in[DLF_I_REF_HP_PERC];
    pcs->ref_intra_percentage = (uint8_t)in[DLF_I_REF_INTRA_PERC];
    pcs->ref_skip_percentage  = (uint8_t)in[DLF_I_REF_SKIP_PERC];
    pcs->coeff_lvl            = (InputCoeffLvl)in[DLF_I_COEFF_LVL];
    pcs->temporal_layer_index = (uint8_t)in[DLF_I_PCS_TEMPORAL_LAYER];
    r0->is_mfmv_used = 0;
    r1->is_mfmv_used = 0;
    w0->object_ptr = r0;
    w1->object_ptr = r1;
    pcs->ref_pic_ptr_array[REF_LIST_0][0] = w0;
    pcs->ref_pic_ptr_array[REF_LIST_1][0] = w1;

    svt_aom_sig_deriv_mode_decision_config_default(scs, pcs);
    dlf_read_out(ppcs, out);

    /* rtime_alloc_ec_ctx_array uses EB_MALLOC_ARRAY, which is plain malloc in
       a release build, so free() is the matching deallocation.
       picture_sb_width / height are 0, so this is a zero-element array. */
    free(pcs->ec_ctx_array);
    free(r1); free(r0); free(w1); free(w0);
    free(pcs); free(ppcs); free(scs);
}

int32_t ref_dlf_ctrls_in_slots(void) { return DLF_I_COUNT; }
int32_t ref_dlf_ctrls_out_slots(void) { return DLF_O_COUNT; }

/* The STILL arm: svt_aom_sig_deriv_mode_decision_config_allintra
 * (enc_mode_config.c:9895) -> get_dlf_level_allintra (:1540). */
void ref_dlf_ctrls_allintra(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    EbObjectWrapper*   w0 = (EbObjectWrapper*)calloc(1, sizeof(*w0));
    EbObjectWrapper*   w1 = (EbObjectWrapper*)calloc(1, sizeof(*w1));
    EbReferenceObject* r0 = (EbReferenceObject*)calloc(1, sizeof(*r0));
    EbReferenceObject* r1 = (EbReferenceObject*)calloc(1, sizeof(*r1));

    scs->static_config.fast_decode = (uint8_t)in[DLF_I_FAST_DECODE];
    scs->static_config.qp          = (uint32_t)in[DLF_I_SQ_QP];
    scs->static_config.resize_mode = (uint8_t)in[DLF_I_RESIZE_MODE];
    scs->static_config.encoder_bit_depth = (uint32_t)in[DLF_I_BIT_DEPTH];
    scs->static_config.enable_dlf_flag   = (uint8_t)in[DLF_I_ENABLE_DLF_FLAG];
    scs->static_config.tune              = (uint8_t)in[DLF_I_TUNE];
    scs->static_config.extended_crf_qindex_offset = (uint8_t)in[DLF_I_EXT_CRF_OFFSET];
    scs->seq_qp_mod            = (uint8_t)in[DLF_I_SEQ_QP_MOD];
    scs->mfmv_enabled          = (uint8_t)in[DLF_I_MFMV_ENABLED];
    scs->rc_stat_gen_pass_mode = (uint8_t)in[DLF_I_RC_STAT_GEN];
    scs->input_resolution      = (ResolutionRange)in[DLF_I_SCS_INPUT_RES];
    scs->super_block_size      = (uint32_t)in[DLF_I_SB_SIZE];
    scs->seq_header.enable_interintra_compound = (uint8_t)in[DLF_I_ENABLE_II];
    scs->tpl                   = 0;

    ppcs->scs                  = scs;
    ppcs->is_ref               = (bool)in[DLF_I_IS_REF];
    ppcs->temporal_layer_index = (uint8_t)in[DLF_I_TEMPORAL_LAYER];
    ppcs->input_resolution     = (ResolutionRange)in[DLF_I_INPUT_RES];
    ppcs->sc_class5            = (uint8_t)in[DLF_I_SC_CLASS5];
    ppcs->hierarchical_levels  = (uint8_t)in[DLF_I_HIER_LEVELS];
    ppcs->transition_present   = (int8_t)in[DLF_I_TRANSITION];
    ppcs->is_highest_layer     = (bool)in[DLF_I_IS_HIGHEST_LAYER];
    ppcs->frame_superres_enabled = (bool)in[DLF_I_SUPERRES];
    ppcs->frame_resize_enabled   = (bool)in[DLF_I_RESIZE_ENABLED];
    ppcs->ref_list0_count_try  = (uint8_t)in[DLF_I_REF_L0_TRY];
    ppcs->ref_list1_count_try  = (uint8_t)in[DLF_I_REF_L1_TRY];
    ppcs->hbd_md               = (int8_t)in[DLF_I_HBD_MD];
    ppcs->r0_gen               = (bool)in[DLF_I_R0_GEN];
    ppcs->r0                   = (double)in[DLF_I_R0_MILLI] / 1000.0;
    ppcs->picture_qp           = (uint8_t)in[DLF_I_PICTURE_QP];
    ppcs->frm_hdr.error_resilient_mode = (uint8_t)in[DLF_I_ERROR_RESILIENT];
    ppcs->frm_hdr.quantization_params.base_q_idx = (int32_t)in[DLF_I_BASE_Q];
    ppcs->frm_hdr.allow_intrabc = (uint8_t)in[DLF_I_ALLOW_INTRABC];
    ppcs->frm_hdr.frame_type = in[DLF_I_FRAME_IS_INTRA] ? KEY_FRAME : INTER_FRAME;
    ppcs->frm_hdr.segmentation_params.segmentation_enabled = (uint8_t)in[DLF_I_SEGMENTATION];
    ppcs->picture_sb_width  = 0;
    ppcs->picture_sb_height = 0;

    pcs->ppcs       = ppcs;
    pcs->scs        = scs;
    pcs->enc_mode   = (EncMode)in[DLF_I_ENC_MODE];
    pcs->slice_type = in[DLF_I_IS_ISLICE] ? I_SLICE : B_SLICE;
    pcs->ref_hp_percentage    = (int16_t)in[DLF_I_REF_HP_PERC];
    pcs->ref_intra_percentage = (uint8_t)in[DLF_I_REF_INTRA_PERC];
    pcs->ref_skip_percentage  = (uint8_t)in[DLF_I_REF_SKIP_PERC];
    pcs->coeff_lvl            = (InputCoeffLvl)in[DLF_I_COEFF_LVL];
    pcs->temporal_layer_index = (uint8_t)in[DLF_I_PCS_TEMPORAL_LAYER];
    r0->is_mfmv_used = 0;
    r1->is_mfmv_used = 0;
    w0->object_ptr = r0;
    w1->object_ptr = r1;
    pcs->ref_pic_ptr_array[REF_LIST_0][0] = w0;
    pcs->ref_pic_ptr_array[REF_LIST_1][0] = w1;

    svt_aom_sig_deriv_mode_decision_config_allintra(scs, pcs);
    dlf_read_out(ppcs, out);

    /* rtime_alloc_ec_ctx_array uses EB_MALLOC_ARRAY, which is plain malloc in
       a release build, so free() is the matching deallocation.
       picture_sb_width / height are 0, so this is a zero-element array. */
    free(pcs->ec_ctx_array);
    free(r1); free(r0); free(w1); free(w0);
    free(pcs); free(ppcs); free(scs);
}
