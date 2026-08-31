/*
 * C shims for the picture-decision reference-structure functions
 * (`Codec/pd_process.c`).
 *
 * These drive the REAL exported C symbols (evidence tier 1). Each shim builds
 * the minimum synthetic control-set the callee reads, calls the exported
 * function, and copies the results out.
 *
 * RULE (see ref_shims.c): NO PER-CALL STATE IN A `static`. cargo runs a test
 * binary's tests on several threads; a `static` scratch here is a data race
 * that shows up as an occasional wrong NUMBER, not a crash. Everything below
 * is either on the stack or calloc/free per call.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "pcs.h"
#include "pd_process.h"
#include "sequence_control_set.h"
#include "pred_structure.h"

/* Neither of these two exported symbols is declared in any SVT header (both
 * are defined in pd_process.c and called only from that translation unit or
 * via an extern-less forward use). Declared here so the shim links the REAL
 * symbol rather than an implicit-int guess; signatures transcribed from the
 * definitions at pd_process.c:4507 and pd_process.c:102. */
void update_count_try(SequenceControlSet* scs, PictureParentControlSet* pcs);
void svt_av1_setup_skip_mode_allowed(PictureParentControlSet* pcs);

/* ---- svt_aom_is_pic_used_as_ref (pd_process.c:1770-1803) ---- */

int32_t ref_is_pic_used_as_ref(uint32_t hierarchical_levels, uint32_t temporal_layer, uint32_t picture_index,
                               uint32_t referencing_scheme, int32_t is_overlay) {
    return svt_aom_is_pic_used_as_ref(
               hierarchical_levels, temporal_layer, picture_index, referencing_scheme, is_overlay != 0)
        ? 1
        : 0;
}

/* ---- svt_aom_get_gm_needed_resolutions (pd_process.c:990-994) ---- */

void ref_get_gm_needed_resolutions(uint8_t ds_lvl, int32_t* full, int32_t* quart, int32_t* sixteen) {
    bool f = false, q = false, s = false;
    svt_aom_get_gm_needed_resolutions(ds_lvl, &f, &q, &s);
    *full    = f ? 1 : 0;
    *quart   = q ? 1 : 0;
    *sixteen = s ? 1 : 0;
}

/* ---- svt_aom_is_incomp_mg_frame (pd_process.c:4986-4989) ---- */

int32_t ref_is_incomp_mg_frame(uint8_t pic_pred_type, uint8_t seq_pred_structure) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PredictionStructure*     ps   = (PredictionStructure*)calloc(1, sizeof(*ps));

    ps->pred_type                       = (PredStructure)pic_pred_type;
    ppcs->pred_struct_ptr               = ps;
    scs->static_config.pred_structure   = (PredStructure)seq_pred_structure;
    ppcs->scs                           = scs;

    int32_t r = svt_aom_is_incomp_mg_frame(ppcs) ? 1 : 0;
    free(ps);
    free(scs);
    free(ppcs);
    return r;
}

/* ---- update_count_try (pd_process.c:4507-4517) ----
 *
 * `frame_is_boosted` reads `frm_hdr.frame_type` (via frame_is_intra_only) and
 * `update_type`, so both are set from the caller rather than inferred. */

void ref_update_count_try(uint8_t frame_type, uint8_t update_type, uint8_t list0_count, uint8_t list1_count,
                          uint8_t base_l0, uint8_t base_l1, uint8_t nonbase_l0, uint8_t nonbase_l1,
                          uint8_t* out_l0_try, uint8_t* out_l1_try) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));

    ppcs->frm_hdr.frame_type = (FrameType)frame_type;
    ppcs->update_type        = (SvtAv1FrameUpdateType)update_type;
    ppcs->ref_list0_count    = list0_count;
    ppcs->ref_list1_count    = list1_count;

    scs->mrp_ctrls.base_ref_list0_count     = base_l0;
    scs->mrp_ctrls.base_ref_list1_count     = base_l1;
    scs->mrp_ctrls.non_base_ref_list0_count = nonbase_l0;
    scs->mrp_ctrls.non_base_ref_list1_count = nonbase_l1;
    ppcs->scs                               = scs;

    update_count_try(scs, ppcs);
    *out_l0_try = ppcs->ref_list0_count_try;
    *out_l1_try = ppcs->ref_list1_count_try;
    free(scs);
    free(ppcs);
}

/* ---- svt_av1_setup_skip_mode_allowed (pd_process.c:102-166) ---- */

void ref_setup_skip_mode_allowed(int32_t enable_order_hint, uint8_t order_hint_bits, uint8_t slice_type,
                                 uint8_t reference_mode, const uint32_t* ref_order_hint, uint32_t cur_order_hint,
                                 int32_t* out_allowed, int32_t* out_idx0, int32_t* out_idx1) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));

    scs->seq_header.order_hint_info.enable_order_hint = enable_order_hint != 0;
    scs->seq_header.order_hint_info.order_hint_bits   = order_hint_bits;
    ppcs->scs                                         = scs;
    ppcs->slice_type                                  = (SliceType)slice_type;
    ppcs->frm_hdr.reference_mode                      = (ReferenceMode)reference_mode;
    for (int i = 0; i < INTER_REFS_PER_FRAME; ++i) {
        ppcs->ref_order_hint[i] = ref_order_hint[i];
    }
    ppcs->cur_order_hint = cur_order_hint;

    svt_av1_setup_skip_mode_allowed(ppcs);

    *out_allowed = ppcs->frm_hdr.skip_mode_params.skip_mode_allowed;
    *out_idx0    = ppcs->frm_hdr.skip_mode_params.ref_frame_idx_0;
    *out_idx1    = ppcs->frm_hdr.skip_mode_params.ref_frame_idx_1;
    free(scs);
    free(ppcs);
}
