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

/* ---- svt_aom_get_tpl_group_level / svt_aom_set_tpl_group
       (initial_rc_process.c:190-306) ---- */

uint8_t svt_aom_get_tpl_group_level(uint8_t tpl, int8_t enc_mode);
uint8_t svt_aom_set_tpl_group(PictureParentControlSet* pcs, uint8_t tpl_group_level, uint32_t source_width,
                              uint32_t source_height);

uint8_t ref_get_tpl_group_level(uint8_t tpl, int8_t enc_mode) {
    return svt_aom_get_tpl_group_level(tpl, enc_mode);
}

/* pcs_present == 0 drives C's `pcs == NULL` probe path, which returns only the
 * synthesizer block size and writes nothing back. */
uint8_t ref_set_tpl_group(int32_t pcs_present, uint8_t slice_type, uint8_t hierarchical_levels,
                          uint8_t input_resolution, uint8_t tpl_lad_mg, uint8_t rate_control_mode,
                          uint8_t tpl_group_level, uint32_t source_width, uint32_t source_height,
                          uint8_t* out_enable, int8_t* out_reduced, uint8_t* out_synth, double* out_r0_adjust) {
    if (!pcs_present) {
        uint8_t blk = svt_aom_set_tpl_group(NULL, tpl_group_level, source_width, source_height);
        *out_enable = 0;
        *out_reduced = 0;
        *out_synth = blk;
        *out_r0_adjust = 0.0;
        return blk;
    }
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));

    ppcs->slice_type                        = (SliceType)slice_type;
    ppcs->hierarchical_levels               = hierarchical_levels;
    scs->input_resolution                   = (ResolutionRange)input_resolution;
    scs->tpl_lad_mg                         = tpl_lad_mg;
    scs->static_config.rate_control_mode    = rate_control_mode;
    ppcs->scs                               = scs;

    uint8_t blk    = svt_aom_set_tpl_group(ppcs, tpl_group_level, source_width, source_height);
    *out_enable    = ppcs->tpl_ctrls.enable;
    *out_reduced   = ppcs->tpl_ctrls.reduced_tpl_group;
    *out_synth     = ppcs->tpl_ctrls.synth_blk_size;
    *out_r0_adjust = ppcs->tpl_ctrls.r0_adjust_factor;
    free(scs);
    free(ppcs);
    return blk;
}

/* ---- search_ref_in_ref_queue (pic_manager_process.c:178-188) ----
 *
 * enc_ctx->ref_pic_list is a ReferenceQueueEntry**; the shim builds the array
 * and its entries per call (never a static -- see the header note). Returns
 * the matched INDEX, or -1. */

#include "encode_context.h"
#include "pic_manager_queue.h"

ReferenceQueueEntry* search_ref_in_ref_queue(EncodeContext* enc_ctx, uint64_t ref_poc);

int32_t ref_search_ref_in_ref_queue(const uint64_t* pocs, const int32_t* valid, uint32_t n, uint64_t ref_poc) {
    EncodeContext*        enc     = (EncodeContext*)calloc(1, sizeof(*enc));
    ReferenceQueueEntry** list    = (ReferenceQueueEntry**)calloc(n ? n : 1, sizeof(*list));
    ReferenceQueueEntry*  entries = (ReferenceQueueEntry*)calloc(n ? n : 1, sizeof(*entries));
    for (uint32_t i = 0; i < n; ++i) {
        entries[i].picture_number = pocs[i];
        entries[i].is_valid       = valid[i] != 0;
        list[i]                   = &entries[i];
    }
    enc->ref_pic_list        = list;
    enc->ref_pic_list_length = n;

    ReferenceQueueEntry* hit = search_ref_in_ref_queue(enc, ref_poc);
    int32_t              idx = -1;
    for (uint32_t i = 0; i < n; ++i) {
        if (hit == &entries[i]) {
            idx = (int32_t)i;
            break;
        }
    }
    free(entries);
    free(list);
    free(enc);
    return idx;
}

/* ---- get_similar_ref_brightness (pd_process.c:4251-4267) ----
 *
 * Reads pcs->ref_pa_pic_ptr_array[0][0] and [1][0], each an EbObjectWrapper
 * whose object_ptr is an EbPaReferenceObject carrying avg_luma. */

#include "reference_object.h"

bool get_similar_ref_brightness(PictureParentControlSet* pcs);

int32_t ref_get_similar_ref_brightness(uint8_t slice_type, uint8_t hierarchical_levels, uint8_t ref_list1_count_try,
                                       uint64_t ref0_avg_luma, uint64_t ref1_avg_luma, uint64_t cur_avg_luma) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    EbObjectWrapper*         w0   = (EbObjectWrapper*)calloc(1, sizeof(*w0));
    EbObjectWrapper*         w1   = (EbObjectWrapper*)calloc(1, sizeof(*w1));
    EbPaReferenceObject*     o0   = (EbPaReferenceObject*)calloc(1, sizeof(*o0));
    EbPaReferenceObject*     o1   = (EbPaReferenceObject*)calloc(1, sizeof(*o1));

    o0->avg_luma                    = ref0_avg_luma;
    o1->avg_luma                    = ref1_avg_luma;
    w0->object_ptr                  = o0;
    w1->object_ptr                  = o1;
    ppcs->ref_pa_pic_ptr_array[0][0] = w0;
    ppcs->ref_pa_pic_ptr_array[1][0] = w1;
    ppcs->slice_type                 = (SliceType)slice_type;
    ppcs->hierarchical_levels        = hierarchical_levels;
    ppcs->ref_list1_count_try        = ref_list1_count_try;
    ppcs->avg_luma                   = cur_avg_luma;

    int32_t r = get_similar_ref_brightness(ppcs) ? 1 : 0;
    free(o1);
    free(o0);
    free(w1);
    free(w0);
    free(ppcs);
    return r;
}

/* ---- set_ref_list_counts / set_all_ref_frame_type / scene_transition_detector
       (pd_process.c:1804-1900, 1044-1099, 256-378) ----
 *
 * These three are `static` in pd_process.c. build.rs promotes them to global
 * symbols with `llvm-objcopy --globalize-symbol` on a PRIVATE COPY of the
 * CMake object file and links that object alongside the archive, so these
 * shims drive the REAL C code (evidence tier 1) rather than a transcription.
 *
 * Guarded by SVTAV1_CREF_PICSTRUCT_STATICS, which build.rs defines only when
 * that promotion actually happened. */
#ifdef SVTAV1_CREF_PICSTRUCT_STATICS

void set_ref_list_counts(PictureParentControlSet* pcs, PictureDecisionContext* ctx);
void set_all_ref_frame_type(PictureDecisionContext* ctx, PictureParentControlSet* ppcs,
                            MvReferenceFrame ref_frame_arr[], uint8_t* tot_ref_frames);

void ref_set_ref_list_counts(uint8_t slice_type, uint8_t frame_type, uint8_t update_type, uint8_t is_overlay,
                             uint8_t pic_pred_type, uint8_t seq_pred_structure, const uint64_t* ref_poc_array,
                             uint8_t base_l0, uint8_t base_l1, uint8_t nonbase_l0, uint8_t nonbase_l1,
                             uint64_t picture_number, uint64_t sframe_poc, uint8_t* out_l0, uint8_t* out_l1) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PredictionStructure*     ps   = (PredictionStructure*)calloc(1, sizeof(*ps));
    PictureDecisionContext*  ctx  = (PictureDecisionContext*)calloc(1, sizeof(*ctx));

    ppcs->slice_type              = (SliceType)slice_type;
    ppcs->frm_hdr.frame_type      = (FrameType)frame_type;
    ppcs->update_type             = (SvtAv1FrameUpdateType)update_type;
    ppcs->is_overlay              = is_overlay != 0;
    ppcs->picture_number          = picture_number;
    ps->pred_type                 = (PredStructure)pic_pred_type;
    ppcs->pred_struct_ptr         = ps;
    scs->static_config.pred_structure       = (PredStructure)seq_pred_structure;
    scs->mrp_ctrls.base_ref_list0_count     = base_l0;
    scs->mrp_ctrls.base_ref_list1_count     = base_l1;
    scs->mrp_ctrls.non_base_ref_list0_count = nonbase_l0;
    scs->mrp_ctrls.non_base_ref_list1_count = nonbase_l1;
    ppcs->scs                     = scs;
    ctx->sframe_poc               = sframe_poc;
    for (int i = 0; i < 7; ++i) {
        ppcs->av1_ref_signal.ref_poc_array[i] = ref_poc_array[i];
    }

    set_ref_list_counts(ppcs, ctx);
    *out_l0 = ppcs->ref_list0_count;
    *out_l1 = ppcs->ref_list1_count;
    free(ctx);
    free(ps);
    free(scs);
    free(ppcs);
}

/* ctx->sframe_poc is left 0 so prune_sframe_refs (called at the tail of
 * set_all_ref_frame_type) is a no-op, matching the port's envelope. */
uint8_t ref_set_all_ref_frame_type(uint8_t slice_type, uint8_t l0_try, uint8_t l1_try, int8_t* out_arr) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureDecisionContext*  ctx  = (PictureDecisionContext*)calloc(1, sizeof(*ctx));

    ppcs->slice_type          = (SliceType)slice_type;
    ppcs->ref_list0_count_try = l0_try;
    ppcs->ref_list1_count_try = l1_try;
    ppcs->scs                 = scs;

    uint8_t tot = 0;
    set_all_ref_frame_type(ctx, ppcs, ppcs->ref_frame_type_arr, &tot);
    for (uint8_t i = 0; i < tot; ++i) {
        out_arr[i] = (int8_t)ppcs->ref_frame_type_arr[i];
    }
    free(ctx);
    free(scs);
    free(ppcs);
    return tot;
}

/* scene_transition_detector is DELIBERATELY NOT shimmed here, and the reason
 * is a measured hazard that applies to this whole technique:
 *
 *   `llvm-objcopy --globalize-symbol` gives you a LINKABLE symbol. It does NOT
 *   give you the source-level ABI. LLVM is free to change the calling
 *   convention of an `internal` function, and for scene_transition_detector it
 *   did: the source signature is
 *     (PictureDecisionContext*, SequenceControlSet*, PictureParentControlSet** window)
 *   but the compiled symbol's third argument is the CURRENT PPCS itself
 *   (argument promotion of window[1]) --
 *     80e0: ldr  w10, [x1, #0x86c]   ; scs->picture_analysis_number_of_regions_per_width
 *     8110: ldr  x15, [x2, #0x28]    ; x2->enhanced_pic  (PPCS+40), not window[1]
 *     8114: ldrh w14, [x15, #0x68]   ; ->width
 *   Calling it with the source signature reads enhanced_pic out of the array
 *   and segfaults on a NULL+0x68, which is exactly what happened.
 *
 * The two functions that ARE shimmed above were checked the same way and their
 * ABIs are unchanged: set_ref_list_counts starts `ldrb w8, [x0, #0xe8]`
 * (PPCS.slice_type at offset 232) so x0 is the PPCS and x1 the context, and
 * set_all_ref_frame_type uses x0..x3 for its four source parameters. A
 * globalized static is only usable at tier 1 after that check. */

#endif /* SVTAV1_CREF_PICSTRUCT_STATICS */
