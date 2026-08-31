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

/* ---- svt_aom_get_mini_gop_stats (utility.c:168-170) ---- */

#include "utility.h"

void ref_get_mini_gop_stats(uint32_t index, uint8_t* hier, uint8_t* start, uint8_t* end, uint8_t* len) {
    const MiniGopStats* s = svt_aom_get_mini_gop_stats(index);
    *hier                 = s->hierarchical_levels;
    *start                = s->start_index;
    *end                  = s->end_index;
    *len                  = s->length;
}

/* ---- is_pic_cutting_short_ra_mg (pd_process.c:928-941) ----
 *
 * PictureDecisionContext is large; calloc/free per call (never a static, see
 * the header note). Only mini_gop_length[] and mini_gop_idr_count[] are read. */

uint8_t is_pic_cutting_short_ra_mg(PictureDecisionContext* pd_ctx, PictureParentControlSet* pcs, uint32_t mg_idx);

int32_t ref_is_pic_cutting_short_ra_mg(uint32_t mg_len, uint32_t mg_idr_count, uint32_t entry_count,
                                       uint8_t pic_pred_type, int32_t idr_flag, int32_t cra_flag) {
    PictureDecisionContext*  ctx  = (PictureDecisionContext*)calloc(1, sizeof(*ctx));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PredictionStructure*     ps   = (PredictionStructure*)calloc(1, sizeof(*ps));

    ctx->mini_gop_length[0]     = mg_len;
    ctx->mini_gop_idr_count[0]  = mg_idr_count;
    ps->pred_struct_entry_count = entry_count;
    ps->pred_type               = (PredStructure)pic_pred_type;
    ppcs->pred_struct_ptr       = ps;
    ppcs->idr_flag              = idr_flag != 0;
    ppcs->cra_flag              = cra_flag != 0;

    int32_t r = (int32_t)is_pic_cutting_short_ra_mg(ctx, ppcs, 0);
    free(ps);
    free(ppcs);
    free(ctx);
    return r;
}

/* ---- svt_aom_is_delayed_intra (pd_process.c:3620-3635) ---- */

bool svt_aom_is_delayed_intra(PictureParentControlSet* pcs);

int32_t ref_is_delayed_intra(int32_t idr_flag, int32_t cra_flag, uint8_t pred_structure, int32_t intra_period_length,
                             int32_t end_of_sequence_flag, uint32_t pre_assignment_buffer_count,
                             uint32_t pred_struct_entry_count) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PredictionStructure*     ps   = (PredictionStructure*)calloc(1, sizeof(*ps));

    ppcs->idr_flag                          = idr_flag != 0;
    ppcs->cra_flag                          = cra_flag != 0;
    ppcs->pred_structure                    = (PredStructure)pred_structure;
    scs->static_config.intra_period_length  = intra_period_length;
    ppcs->scs                               = scs;
    ppcs->end_of_sequence_flag              = end_of_sequence_flag != 0;
    ppcs->pre_assignment_buffer_count       = pre_assignment_buffer_count;
    ps->pred_struct_entry_count             = pred_struct_entry_count;
    ppcs->pred_struct_ptr                   = ps;

    int32_t r = svt_aom_is_delayed_intra(ppcs) ? 1 : 0;
    free(ps);
    free(scs);
    free(ppcs);
    return r;
}

/* ---- search_this_pic (pd_process.c:3606-3619) ---- */

int32_t search_this_pic(PictureParentControlSet** buf, uint32_t buf_size, uint64_t input_pic);

int32_t ref_search_this_pic(const uint64_t* pocs, uint32_t buf_size, uint64_t input_pic) {
    PictureParentControlSet** buf = (PictureParentControlSet**)calloc(buf_size ? buf_size : 1, sizeof(*buf));
    for (uint32_t i = 0; i < buf_size; ++i) {
        buf[i]                 = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));
        buf[i]->picture_number = pocs[i];
    }
    int32_t r = search_this_pic(buf, buf_size, input_pic);
    for (uint32_t i = 0; i < buf_size; ++i) {
        free(buf[i]);
    }
    free(buf);
    return r;
}
