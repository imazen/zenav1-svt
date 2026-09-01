/*
 * C shims for the VBR/CBR rate-control state machine (`Codec/rc_vbr_cbr.c`,
 * lane `wx-rc`).
 *
 * EVIDENCE TIER 1 (docs/WORKING-ON-THIS.md §4). Every entry point here calls
 * the REAL compiled C function. Three of them
 * (`av1_rc_regulate_q`, `av1_rc_update_rate_correction_factors`,
 * `calc_active_worst_quality_no_stats_cbr`) plus the two recode helpers
 * (`get_regulated_q_overshoot` / `_undershoot`) are `static` in C and are
 * reached through the build script's `--globalize-symbol` promotion of a
 * PRIVATE COPY of `cbuild-static/.../rc_vbr_cbr.c.o` — the same mechanism
 * `pd_process.c`'s two statics already use. Nothing on this side is
 * transcribed.
 *
 * RULE (inherited from ref_shims.c, re-stated because it was paid for):
 * NO PER-CALL STATE IN A `static`. cargo runs a test binary's tests on several
 * threads, so a `static` scratch struct is a data race that fails as an
 * occasional wrong NUMBER rather than a crash. Everything below is calloc'd
 * and freed per call.
 *
 * TWO CONTRACTS THE ENCODER HANDS C THAT THIS SHIM MUST ALSO HAND IT
 * (trap #4 in docs/WORKING-ON-THIS.md §5):
 *   1. `rc->rc_mutex` and `enc_ctx->frame_updated_mutex` are DEREFERENCED, not
 *      merely tested: `get_rate_correction_factor` and
 *      `calc_active_worst_quality_no_stats_cbr` call `svt_block_on_mutex`,
 *      which is `pthread_mutex_lock((pthread_mutex_t*)handle)`. A calloc'd
 *      NULL handle segfaults. Both are created with `svt_create_mutex()` per
 *      call and destroyed after.
 *   2. `ppcs->av1_cm` is a POINTER, and `get_rate_correction_factor` reads
 *      `av1_cm->frm_size.frame_width/height` off it. A NULL there is a
 *      null-deref, so an `Av1Common` is allocated too.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "pcs.h"
#include "sequence_control_set.h"
#include "encode_context.h"
#include "rc_process.h"
#include "svt_threads.h"
#include "pass2_strategy.h"

#if defined(SVTAV1_CREF_RC_VBR_STATICS)

/* All five are `static` in rc_vbr_cbr.c, so no header declares them. The
 * signatures are transcribed from the DEFINITIONS at the line numbers given;
 * the promotion makes the symbols linkable but does NOT check the signature,
 * so a wrong one here would corrupt the stack rather than fail to link. */
int  av1_rc_regulate_q(PictureParentControlSet* ppcs, int active_best_quality, int active_worst_quality, int width,
                       int height); /* rc_vbr_cbr.c:307 */
void av1_rc_update_rate_correction_factors(PictureParentControlSet* ppcs, int width,
                                           int height); /* rc_vbr_cbr.c:1354 */
/* `calc_active_worst_quality_no_stats_cbr` is deliberately NOT bound: it has
 * a `t` symbol, but LLVM specialized its ABI (two args, `x0` is not a PPCS).
 * See `link_globalized_rc_vbr_statics` in build.rs for the disassembly. It is
 * driven indirectly through the exported `svt_av1_resize_reset_rc`. */
uint8_t clamp_qindex(SequenceControlSet* scs, int qindex); /* rc_vbr_cbr.c:21 */
int  get_regulated_q_overshoot(PictureParentControlSet* ppcs, int q_low, int q_high, int top_index,
                               int bottom_index); /* rc_vbr_cbr.c:1719 */
int  get_regulated_q_undershoot(PictureParentControlSet* ppcs, int q_high, int top_index,
                                int bottom_index); /* rc_vbr_cbr.c:1737 */

#endif /* SVTAV1_CREF_RC_VBR_STATICS */

/* EXPORTED (`nm -g` finds it) but declared in no SVT header — it is called
 * only from rc_vbr_cbr.c itself and rc_rtc_cbr.c, both of which see the
 * definition. Transcribed from the definition at rc_vbr_cbr.c:324. */
void svt_av1_resize_reset_rc(PictureParentControlSet* ppcs, int32_t resize_width, int32_t resize_height,
                             int32_t prev_width, int32_t prev_height);

/* The flat state the Rust side owns. Field-for-field with
 * `svtav1_encoder::port_rc_vbr_cbr_state`'s `RateControl` / `RateControlCfg` /
 * `FrameRc` / `SeqRc` / `CyclicRefresh`, flattened into one `#[repr(C)]`
 * struct so a call site cannot transpose two of thirty arguments. Mutated
 * fields are copied back before returning. */
typedef struct RefRcVbrState {
    /* --- RATE_CONTROL --- */
    int32_t  avg_frame_bandwidth;
    int32_t  prev_avg_frame_bandwidth;
    int32_t  max_frame_bandwidth;
    int64_t  optimal_buffer_level;
    int64_t  maximum_buffer_size;
    int64_t  buffer_level;
    int64_t  bits_off_target;
    double   rate_correction_factors[MAX_TEMPORAL_LAYERS + 1];
    int32_t  avg_frame_qindex[2];
    int32_t  worst_quality;
    int32_t  best_quality;
    int32_t  q_1_frame;
    int32_t  q_2_frame;
    int32_t  rc_1_frame;
    int32_t  rc_2_frame;
    int32_t  frames_since_key;
    int32_t  percent_refresh_adjustment;
    double   rate_ratio_qdelta_adjustment;
    uint32_t cur_avg_base_me_dist;
    uint32_t prev_avg_base_me_dist;
    int32_t  frame_updated;
    /* --- RateControlCfg --- */
    int32_t rc_mode; /* enum aom_rc_mode */
    int32_t under_shoot_pct;
    int32_t over_shoot_pct;
    /* --- PictureParentControlSet --- */
    int32_t frame_type;
    int32_t update_type;
    int32_t is_overlay;
    int32_t sc_class1;
    int32_t temporal_layer_index;
    int32_t hierarchical_levels;
    int32_t frame_width;
    int32_t frame_height;
    int32_t this_frame_target;
    int32_t projected_frame_size;
    int32_t base_q_idx;
    int32_t b64_total_count;
    /* --- CyclicRefresh --- */
    int32_t apply_cyclic_refresh;
    int32_t qindex_delta[3];
    int32_t actual_num_seg1_sbs;
    int32_t actual_num_seg2_sbs;
    /* --- SequenceControlSet --- */
    int32_t encoder_bit_depth;
} RefRcVbrState;

/* Scratch the four allocations live in, so build/teardown is written once. */
typedef struct RcHarness {
    PictureParentControlSet* ppcs;
    SequenceControlSet*      scs;
    EncodeContext*           enc_ctx;
    Av1Common*               cm;
} RcHarness;

static void rc_harness_build(RcHarness* h, const RefRcVbrState* st) {
    h->ppcs    = (PictureParentControlSet*)calloc(1, sizeof(*h->ppcs));
    h->scs     = (SequenceControlSet*)calloc(1, sizeof(*h->scs));
    h->enc_ctx = (EncodeContext*)calloc(1, sizeof(*h->enc_ctx));
    h->cm      = (Av1Common*)calloc(1, sizeof(*h->cm));

    h->scs->enc_ctx  = h->enc_ctx;
    h->ppcs->scs     = h->scs;
    h->ppcs->av1_cm  = h->cm;

    RATE_CONTROL*   rc     = &h->enc_ctx->rc;
    RateControlCfg* rc_cfg = &h->enc_ctx->rc_cfg;

    rc->rc_mutex                 = svt_create_mutex();
    h->enc_ctx->frame_updated_mutex = svt_create_mutex();

    rc->avg_frame_bandwidth      = st->avg_frame_bandwidth;
    rc->prev_avg_frame_bandwidth = st->prev_avg_frame_bandwidth;
    rc->max_frame_bandwidth      = st->max_frame_bandwidth;
    rc->optimal_buffer_level     = st->optimal_buffer_level;
    rc->maximum_buffer_size      = st->maximum_buffer_size;
    rc->buffer_level             = st->buffer_level;
    rc->bits_off_target          = st->bits_off_target;
    memcpy(rc->rate_correction_factors, st->rate_correction_factors, sizeof(rc->rate_correction_factors));
    rc->avg_frame_qindex[0]           = st->avg_frame_qindex[0];
    rc->avg_frame_qindex[1]           = st->avg_frame_qindex[1];
    rc->worst_quality                 = st->worst_quality;
    rc->best_quality                  = st->best_quality;
    rc->q_1_frame                     = st->q_1_frame;
    rc->q_2_frame                     = st->q_2_frame;
    rc->rc_1_frame                    = st->rc_1_frame;
    rc->rc_2_frame                    = st->rc_2_frame;
    rc->frames_since_key              = st->frames_since_key;
    rc->percent_refresh_adjustment    = st->percent_refresh_adjustment;
    rc->rate_ratio_qdelta_adjustment  = st->rate_ratio_qdelta_adjustment;
    rc->cur_avg_base_me_dist          = st->cur_avg_base_me_dist;
    rc->prev_avg_base_me_dist         = st->prev_avg_base_me_dist;
    h->enc_ctx->frame_updated         = st->frame_updated;

    rc_cfg->mode            = (enum aom_rc_mode)st->rc_mode;
    rc_cfg->under_shoot_pct = st->under_shoot_pct;
    rc_cfg->over_shoot_pct  = st->over_shoot_pct;

    h->ppcs->frm_hdr.frame_type                            = (FrameType)st->frame_type;
    h->ppcs->frm_hdr.quantization_params.base_q_idx         = st->base_q_idx;
    h->ppcs->update_type                                    = (SvtAv1FrameUpdateType)st->update_type;
    h->ppcs->is_overlay                                     = (uint8_t)st->is_overlay;
    h->ppcs->sc_class1                                      = (uint8_t)st->sc_class1;
    h->ppcs->temporal_layer_index                           = (uint8_t)st->temporal_layer_index;
    h->ppcs->hierarchical_levels                            = (uint8_t)st->hierarchical_levels;
    h->ppcs->this_frame_target                              = st->this_frame_target;
    h->ppcs->projected_frame_size                           = st->projected_frame_size;
    h->ppcs->b64_total_count                                = (uint16_t)st->b64_total_count;
    h->cm->frm_size.frame_width                             = st->frame_width;
    h->cm->frm_size.frame_height                            = st->frame_height;
    h->cm->frm_size.superres_upscaled_width                 = st->frame_width;

    h->ppcs->cyclic_refresh.apply_cyclic_refresh = st->apply_cyclic_refresh;
    h->ppcs->cyclic_refresh.qindex_delta[0]      = st->qindex_delta[0];
    h->ppcs->cyclic_refresh.qindex_delta[1]      = st->qindex_delta[1];
    h->ppcs->cyclic_refresh.qindex_delta[2]      = st->qindex_delta[2];
    h->ppcs->cyclic_refresh.actual_num_seg1_sbs  = st->actual_num_seg1_sbs;
    h->ppcs->cyclic_refresh.actual_num_seg2_sbs  = st->actual_num_seg2_sbs;

    h->scs->static_config.encoder_bit_depth = (EbBitDepth)st->encoder_bit_depth;
    h->scs->encoder_bit_depth               = (uint32_t)st->encoder_bit_depth;
}

/* Copy back every field the callees may have written. */
static void rc_harness_store(const RcHarness* h, RefRcVbrState* st) {
    const RATE_CONTROL* rc = &h->enc_ctx->rc;
    memcpy(st->rate_correction_factors, rc->rate_correction_factors, sizeof(st->rate_correction_factors));
    st->avg_frame_qindex[0]          = rc->avg_frame_qindex[0];
    st->avg_frame_qindex[1]          = rc->avg_frame_qindex[1];
    st->q_1_frame                    = rc->q_1_frame;
    st->q_2_frame                    = rc->q_2_frame;
    st->rc_1_frame                   = rc->rc_1_frame;
    st->rc_2_frame                   = rc->rc_2_frame;
    st->percent_refresh_adjustment   = rc->percent_refresh_adjustment;
    st->rate_ratio_qdelta_adjustment = rc->rate_ratio_qdelta_adjustment;
    st->buffer_level                 = rc->buffer_level;
    st->bits_off_target              = rc->bits_off_target;
    st->frame_updated                = h->enc_ctx->frame_updated;
}

static void rc_harness_free(RcHarness* h) {
    svt_destroy_mutex(h->enc_ctx->rc.rc_mutex);
    svt_destroy_mutex(h->enc_ctx->frame_updated_mutex);
    free(h->cm);
    free(h->enc_ctx);
    free(h->scs);
    free(h->ppcs);
}

#if defined(SVTAV1_CREF_RC_VBR_STATICS)

int32_t ref_rc_regulate_q(RefRcVbrState* st, int32_t active_best_quality, int32_t active_worst_quality, int32_t width,
                          int32_t height) {
    RcHarness h;
    rc_harness_build(&h, st);
    int r = av1_rc_regulate_q(h.ppcs, active_best_quality, active_worst_quality, width, height);
    rc_harness_store(&h, st);
    rc_harness_free(&h);
    return r;
}

/* `clamp_qindex` (rc_vbr_cbr.c:21) reads only
 * `scs->static_config.{min,max}_qp_allowed` and indexes the exported
 * `quantizer_to_qindex` table, so a stack SequenceControlSet is the whole
 * contract. Verified by reading the body: nothing else on `scs` is touched. */
int32_t ref_rc_clamp_qindex(int32_t min_qp_allowed, int32_t max_qp_allowed, int32_t qindex) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    scs->static_config.min_qp_allowed = (uint32_t)min_qp_allowed;
    scs->static_config.max_qp_allowed = (uint32_t)max_qp_allowed;
    int32_t r = (int32_t)clamp_qindex(scs, qindex);
    free(scs);
    return r;
}

void ref_rc_update_rate_correction_factors(RefRcVbrState* st, int32_t width, int32_t height) {
    RcHarness h;
    rc_harness_build(&h, st);
    av1_rc_update_rate_correction_factors(h.ppcs, width, height);
    rc_harness_store(&h, st);
    rc_harness_free(&h);
}

int32_t ref_get_regulated_q_overshoot(RefRcVbrState* st, int32_t q_low, int32_t q_high, int32_t top_index,
                                      int32_t bottom_index) {
    RcHarness h;
    rc_harness_build(&h, st);
    int r = get_regulated_q_overshoot(h.ppcs, q_low, q_high, top_index, bottom_index);
    rc_harness_store(&h, st);
    rc_harness_free(&h);
    return r;
}

int32_t ref_get_regulated_q_undershoot(RefRcVbrState* st, int32_t q_high, int32_t top_index, int32_t bottom_index) {
    RcHarness h;
    rc_harness_build(&h, st);
    int r = get_regulated_q_undershoot(h.ppcs, q_high, top_index, bottom_index);
    rc_harness_store(&h, st);
    rc_harness_free(&h);
    return r;
}

#endif /* SVTAV1_CREF_RC_VBR_STATICS */

/* `svt_av1_resize_reset_rc` (rc_vbr_cbr.c:324) IS exported, so it needs no
 * promotion. It calls two of the promoted statics internally, but through the
 * ARCHIVE's own copies, so it works with or without the promotion. */
void ref_rc_resize_reset_rc(RefRcVbrState* st, int32_t resize_width, int32_t resize_height, int32_t prev_width,
                            int32_t prev_height) {
    RcHarness h;
    rc_harness_build(&h, st);
    /* `svt_av1_resize_reset_rc` reads `rc->last_q[INTER_FRAME]`, which the
     * flat state does not carry, so it is set from `avg_frame_qindex[1]` on
     * the Rust side and mirrored here. */
    h.enc_ctx->rc.last_q[INTER_FRAME] = st->avg_frame_qindex[1];
    svt_av1_resize_reset_rc(h.ppcs, resize_width, resize_height, prev_width, prev_height);
    rc_harness_store(&h, st);
    st->this_frame_target = h.ppcs->this_frame_target;
    rc_harness_free(&h);
}

/* =========================================================================
 * `svt_av1_rc_calc_qindex_rate_control` (rc_vbr_cbr.c:1281) — EXPORTED.
 *
 * This is the tier-1 route to the whole qindex DECISION chain, none of which
 * has a symbol of its own: rc_pick_q_and_bounds{,_no_stats_cbr},
 * calc_active_best_quality_no_stats_cbr, get_active_best_quality,
 * adjust_active_best_and_worst_quality_org, av1_frame_type_qdelta_org, get_q,
 * find_min_ref_base_q_idx and cyclic_refresh_init are ALL inlined into it.
 *
 * It needs a populated DPB, so this harness builds the full
 * PCS -> EbObjectWrapper -> EbReferenceObject chain per reference slot, plus
 * the ME-distortion arrays the VBR reference-floor arm sums and the
 * `pa_me_data->me_results[i]->me_mv_array[0]` chain that
 * `svt_aom_cyclic_refresh_setup` dereferences when cyclic refresh is on.
 * Every one of those is a CONTRACT THE ENCODER HANDS C (docs/WORKING-ON-THIS.md
 * §5 trap #4): a null there is a segfault, not a wrong number.
 * ====================================================================== */

/* `svt_aom_cyclic_refresh_setup` is declared in rc_process.h (already
   included above); rc_aq.c has no header of its own. `EbReferenceObject` and
   `EbObjectWrapper` are the DPB chain `get_ref_obj` walks. */
#include "reference_object.h"
#include "sys_resource_manager.h"

#define REF_RC_MAX_REFS 4
#define REF_RC_MAX_B64 64

typedef struct RefRcQpickRef {
    int32_t  present;
    int32_t  tmp_layer_idx;
    int32_t  slice_type; /* ref_obj->slice_type */
    int32_t  pcs_slice_type; /* pcs->ref_slice_type[l][i] */
    uint64_t ref_poc;
    int32_t  base_q_idx; /* pcs->ref_base_q_idx[l][i] */
    double   pcs_r0; /* pcs->ref_pic_r0[l][i] */
    double   obj_r0; /* ref_obj->r0 */
} RefRcQpickRef;

typedef struct RefRcQpickState {
    RefRcVbrState base; /* every field rc_harness_build already maps */

    /* extra RATE_CONTROL */
    int32_t active_worst_quality;
    int32_t active_best_quality[MAX_ARF_LAYERS + 1];
    int32_t last_boosted_qindex;
    int32_t kf_boost;
    int32_t gfu_boost;
    int32_t arf_q; /* in AND out */
    int32_t frames_to_key;
    int32_t this_key_frame_forced;
    int32_t avg_frame_low_motion;

    /* extra PPCS / PCS */
    uint64_t picture_number;
    uint64_t frame_offset;
    int32_t  slice_type;
    int32_t  layer_depth;
    int32_t  is_ref;
    int32_t  transition_present;
    double   r0;

    /* SequenceControlSet */
    int32_t intra_period_length;
    int32_t gop_constraint_rc;
    int32_t is_short_clip;
    int32_t super_block_size;
    int32_t sb_total_count;
    int32_t passes;
    int32_t qp_scale_compress_strength;
    int32_t input_resolution;
    int32_t min_qp_allowed;
    int32_t max_qp_allowed;
    /* `scs->static_config.hierarchical_levels`, which `cyclic_refresh_init`
       reads — a DIFFERENT field from `ppcs->hierarchical_levels` that
       `adjust_q_cbr` reads. Leaving it 0 silently shifted the refresh
       threshold by 16x in a first draft. */
    int32_t seq_hierarchical_levels;

    /* TWO_PASS */
    int32_t extend_minq;
    int32_t extend_maxq;
    int32_t extend_minq_fast;
    int32_t kf_zeromotion_pct;

    /* DPB */
    int32_t       l0_count_try;
    int32_t       l1_count_try;
    RefRcQpickRef l0[REF_RC_MAX_REFS];
    RefRcQpickRef l1[REF_RC_MAX_REFS];

    /* ME distortion (b64_total_count entries used) */
    int32_t  b64_total_count;
    uint32_t me_cur_64x64[REF_RC_MAX_B64];
    uint32_t me_cur_8x8[REF_RC_MAX_B64];
    uint32_t me_ref_l0_64x64[REF_RC_MAX_B64];
    int32_t  me_mv_x[REF_RC_MAX_B64];
    int32_t  me_mv_y[REF_RC_MAX_B64];
    uint64_t norm_me_dist;

    /* cyclic refresh, in and out */
    uint32_t cr_sb_end_ctx; /* enc_ctx->cr_sb_end */
    int32_t  cr_apply;
    int32_t  cr_percent_refresh;
    uint32_t cr_sb_start;
    uint32_t cr_sb_end;
    int32_t  cr_max_qdelta_perc;
    int32_t  cr_rate_boost_fac;
    double   cr_rate_ratio_qdelta;
    double   cr_rate_ratio_qdelta_seg2;
    int32_t  cr_qindex_delta[3];
    int32_t  cr_actual_num_seg1_sbs;
    int32_t  cr_actual_num_seg2_sbs;

    /* outputs */
    int32_t out_base_q_idx;
    int32_t out_top_index;
    int32_t out_bottom_index;
} RefRcQpickState;

typedef struct QpickHarness {
    RcHarness            rc;
    PictureControlSet*   pcs;
    EbObjectWrapper*     wrappers[2][REF_RC_MAX_REFS];
    EbReferenceObject*   objs[2][REF_RC_MAX_REFS];
    MotionEstimationData* me_data;
    MeSbResults**        me_results;
    MeSbResults*         me_sb;
    Mv*                  me_mvs;
    uint32_t*            me_64x64;
    uint32_t*            me_8x8;
    uint32_t*            ref_me_64x64;
} QpickHarness;

int32_t ref_rc_calc_qindex_rate_control(RefRcQpickState* st) {
    QpickHarness h;
    memset(&h, 0, sizeof(h));
    rc_harness_build(&h.rc, &st->base);

    PictureParentControlSet* ppcs    = h.rc.ppcs;
    SequenceControlSet*      scs     = h.rc.scs;
    EncodeContext*           enc_ctx = h.rc.enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;

    rc->active_worst_quality = st->active_worst_quality;
    for (int i = 0; i <= MAX_ARF_LAYERS; ++i) {
        rc->active_best_quality[i] = st->active_best_quality[i];
    }
    rc->last_boosted_qindex   = st->last_boosted_qindex;
    rc->kf_boost              = st->kf_boost;
    rc->gfu_boost             = st->gfu_boost;
    rc->arf_q                 = st->arf_q;
    rc->frames_to_key         = st->frames_to_key;
    rc->this_key_frame_forced = st->this_key_frame_forced;
    rc->avg_frame_low_motion  = st->avg_frame_low_motion;

    ppcs->picture_number     = st->picture_number;
    ppcs->frame_offset       = st->frame_offset;
    ppcs->slice_type         = (SliceType)st->slice_type;
    ppcs->layer_depth        = st->layer_depth;
    ppcs->is_ref             = (bool)st->is_ref;
    ppcs->transition_present = (int8_t)st->transition_present;
    ppcs->r0                 = st->r0;
    ppcs->ref_list0_count_try = (uint8_t)st->l0_count_try;
    ppcs->ref_list1_count_try = (uint8_t)st->l1_count_try;
    ppcs->b64_total_count     = (uint16_t)st->b64_total_count;
    ppcs->norm_me_dist        = st->norm_me_dist;

    scs->static_config.intra_period_length = st->intra_period_length;
    scs->static_config.gop_constraint_rc   = (bool)st->gop_constraint_rc;
    scs->static_config.min_qp_allowed      = (uint32_t)st->min_qp_allowed;
    scs->static_config.max_qp_allowed      = (uint32_t)st->max_qp_allowed;
    scs->is_short_clip                     = (uint8_t)st->is_short_clip;
    scs->super_block_size                  = (uint32_t)st->super_block_size;
    scs->sb_total_count                    = (uint16_t)st->sb_total_count;
    scs->passes                            = st->passes;
    scs->static_config.hierarchical_levels = (uint32_t)st->seq_hierarchical_levels;
    scs->input_resolution                  = (ResolutionRange)st->input_resolution;
#if !SVT_HDR_MODE
    scs->static_config.qp_scale_compress_strength_unused = (uint8_t)st->qp_scale_compress_strength;
#endif
    scs->twopass.extend_minq       = st->extend_minq;
    scs->twopass.extend_maxq       = st->extend_maxq;
    scs->twopass.extend_minq_fast  = st->extend_minq_fast;
    scs->twopass.kf_zeromotion_pct = st->kf_zeromotion_pct;

    enc_ctx->cr_sb_end = st->cr_sb_end_ctx;

    /* The DPB: PCS -> wrapper -> reference object, per slot. */
    h.pcs = (PictureControlSet*)calloc(1, sizeof(*h.pcs));
    h.pcs->ppcs                 = ppcs;
    h.pcs->picture_number       = st->picture_number;
    h.pcs->slice_type           = (SliceType)st->slice_type;
    h.pcs->temporal_layer_index = (uint8_t)st->base.temporal_layer_index;
    h.pcs->b64_total_count      = (uint16_t)st->b64_total_count;
    ppcs->child_pcs             = h.pcs;

    for (int l = 0; l < 2; ++l) {
        const RefRcQpickRef* src = (l == 0) ? st->l0 : st->l1;
        for (int i = 0; i < REF_RC_MAX_REFS; ++i) {
            if (!src[i].present) {
                continue;
            }
            h.objs[l][i]     = (EbReferenceObject*)calloc(1, sizeof(EbReferenceObject));
            h.wrappers[l][i] = (EbObjectWrapper*)calloc(1, sizeof(EbObjectWrapper));
            h.objs[l][i]->tmp_layer_idx = (uint8_t)src[i].tmp_layer_idx;
            h.objs[l][i]->slice_type    = (SliceType)src[i].slice_type;
            h.objs[l][i]->ref_poc       = src[i].ref_poc;
            h.objs[l][i]->r0            = src[i].obj_r0;
            h.wrappers[l][i]->object_ptr = h.objs[l][i];
            h.pcs->ref_pic_ptr_array[l][i] = h.wrappers[l][i];
            h.pcs->ref_base_q_idx[l][i]    = (uint8_t)src[i].base_q_idx;
            h.pcs->ref_slice_type[l][i]    = (SliceType)src[i].pcs_slice_type;
            h.pcs->ref_pic_r0[l][i]        = src[i].pcs_r0;
        }
    }

    /* ME distortion arrays. `sb_me_64x64_dist` hangs off the L0[0] reference
       object; `me_64x64_distortion` / `me_8x8_distortion` off the PPCS. */
    h.me_64x64     = (uint32_t*)calloc(REF_RC_MAX_B64, sizeof(uint32_t));
    h.me_8x8       = (uint32_t*)calloc(REF_RC_MAX_B64, sizeof(uint32_t));
    h.ref_me_64x64 = (uint32_t*)calloc(REF_RC_MAX_B64, sizeof(uint32_t));
    for (int i = 0; i < REF_RC_MAX_B64; ++i) {
        h.me_64x64[i]     = st->me_cur_64x64[i];
        h.me_8x8[i]       = st->me_cur_8x8[i];
        h.ref_me_64x64[i] = st->me_ref_l0_64x64[i];
    }
    ppcs->me_64x64_distortion = h.me_64x64;
    ppcs->me_8x8_distortion   = h.me_8x8;
    if (h.objs[0][0]) {
        h.objs[0][0]->sb_me_64x64_dist = h.ref_me_64x64;
    }

    /* `svt_aom_cyclic_refresh_setup` walks
       `ppcs->pa_me_data->me_results[i]->me_mv_array[0]`. */
    h.me_data    = (MotionEstimationData*)calloc(1, sizeof(MotionEstimationData));
    h.me_results = (MeSbResults**)calloc(REF_RC_MAX_B64, sizeof(MeSbResults*));
    h.me_sb      = (MeSbResults*)calloc(REF_RC_MAX_B64, sizeof(MeSbResults));
    /* `me_mv_array` is a `Mv*`, NOT an inline array — a calloc'd MeSbResults
       leaves it NULL and `is_cr_motion_static` dereferences it. One Mv per
       b64 is all that function reads (index 0). */
    h.me_mvs = (Mv*)calloc(REF_RC_MAX_B64, sizeof(Mv));
    for (int i = 0; i < REF_RC_MAX_B64; ++i) {
        h.me_mvs[i].x               = (int16_t)st->me_mv_x[i];
        h.me_mvs[i].y               = (int16_t)st->me_mv_y[i];
        h.me_sb[i].me_mv_array      = &h.me_mvs[i];
        h.me_results[i]             = &h.me_sb[i];
    }
    h.me_data->me_results = h.me_results;
    ppcs->pa_me_data      = h.me_data;

    svt_av1_rc_calc_qindex_rate_control(h.pcs, scs);

    /* Outputs. */
    rc_harness_store(&h.rc, &st->base);
    st->out_base_q_idx   = ppcs->frm_hdr.quantization_params.base_q_idx;
    st->out_top_index    = ppcs->top_index;
    st->out_bottom_index = ppcs->bottom_index;
    st->arf_q            = rc->arf_q;
    st->active_worst_quality = rc->active_worst_quality;
    st->cr_sb_end_ctx    = enc_ctx->cr_sb_end;

    const CyclicRefresh* cr        = &ppcs->cyclic_refresh;
    st->cr_apply                   = cr->apply_cyclic_refresh;
    st->cr_percent_refresh         = cr->percent_refresh;
    st->cr_sb_start                = cr->sb_start;
    st->cr_sb_end                  = cr->sb_end;
    st->cr_max_qdelta_perc         = cr->max_qdelta_perc;
    st->cr_rate_boost_fac          = cr->rate_boost_fac;
    st->cr_rate_ratio_qdelta       = cr->rate_ratio_qdelta;
    st->cr_rate_ratio_qdelta_seg2  = cr->rate_ratio_qdelta_seg2;
    st->cr_qindex_delta[0]         = cr->qindex_delta[0];
    st->cr_qindex_delta[1]         = cr->qindex_delta[1];
    st->cr_qindex_delta[2]         = cr->qindex_delta[2];
    st->cr_actual_num_seg1_sbs     = cr->actual_num_seg1_sbs;
    st->cr_actual_num_seg2_sbs     = cr->actual_num_seg2_sbs;

    free(h.me_mvs);
    free(h.me_sb);
    free(h.me_results);
    free(h.me_data);
    free(h.ref_me_64x64);
    free(h.me_8x8);
    free(h.me_64x64);
    for (int l = 0; l < 2; ++l) {
        for (int i = 0; i < REF_RC_MAX_REFS; ++i) {
            free(h.wrappers[l][i]);
            free(h.objs[l][i]);
        }
    }
    free(h.pcs);
    rc_harness_free(&h.rc);
    return st->out_base_q_idx;
}

/* =========================================================================
 * The post-encode / recode / resize group. Four EXPORTED entry points, so
 * tier 1 with no promotion:
 *   svt_av1_rc_postencode_update            (rc_vbr_cbr.c:1562)
 *   svt_av1_rc_postencode_update_gop_const  (rc_vbr_cbr.c:1494)
 *   recode_loop_update_q                    (rc_vbr_cbr.c:1793)
 *   svt_aom_dynamic_resize_decision         (rc_vbr_cbr.c:497)
 * Between them they drag in update_buffer_level,
 * av1_rc_update_rate_correction_factors, av1_rc_compute_frame_size_bounds,
 * recode_loop_test, av1_find_qindex, av1_get_compression_ratio,
 * get_regulated_q_overshoot/_undershoot, dynamic_resize_one_pass_cbr,
 * svt_av1_resize_reset_rc and set_gf_interval_update_onepass_rt.
 * ====================================================================== */

typedef struct RefRcUpdateState {
    RefRcVbrState base;

    /* extra RATE_CONTROL, in and out */
    int32_t last_boosted_qindex;
    int32_t last_q[2];
    int32_t rolling_target_bits;
    int32_t rolling_actual_bits;
    int64_t total_actual_bits;
    int64_t total_target_bits;
    int32_t avg_frame_low_motion;
    int32_t constrained_gf_group;
    int32_t frames_since_cdf_update;
    int32_t frames_to_key;
    int32_t this_key_frame_forced;
    int32_t active_worst_quality;
    int32_t kf_boost;
    int32_t gfu_boost;
    int32_t baseline_gf_interval;
    int32_t resize_state;
    int32_t resize_avg_qp;
    int32_t resize_buffer_underflow;
    int32_t resize_count;

    /* extra PPCS */
    uint64_t picture_number;
    uint64_t frame_offset;
    uint64_t total_num_bits;
    uint64_t pcs_total_rate;
    uint64_t avg_cnt_zeromv;
    int32_t  showable_frame;
    int32_t  loop_count;
    int32_t  max_frame_size;
    int32_t  src_frame_width;
    int32_t  src_frame_height;
    int32_t  scene_change_flag;

    /* SequenceControlSet / EncodeContext */
    int32_t min_qp_allowed;
    int32_t max_qp_allowed;
    int32_t recode_loop;
    int32_t recode_tolerance;
    int32_t min_cr;
    uint32_t max_bit_rate;
    int32_t rtc;
    int32_t max_input_luma_width;
    int32_t max_input_luma_height;
    int32_t intra_period_length;
    int32_t seq_profile;
    double  new_framerate;

    /* RateControlIntervalParamContext, in and out */
    int32_t param_rolling_target_bits;
    int32_t param_rolling_actual_bits;
    int64_t param_total_actual_bits;
    int64_t param_total_target_bits;

    /* recode-loop state, in and out */
    int32_t recode_q;
    int32_t recode_q_low;
    int32_t recode_q_high;
    int32_t recode_undershoot_seen;
    int32_t recode_overshoot_seen;
    int32_t recode_low_cr_seen;
    int32_t recode_loop_again; /* out */
    int32_t top_index;
    int32_t bottom_index;

    /* resize_pending_params, in and out */
    int32_t pending_resize_state;
    int32_t pending_resize_denom;

    /* outputs */
    int32_t out_projected_frame_size;
    int32_t out_this_frame_target;
    int32_t out_base_frame_target;

    /* --- appended for svt_av1_twopass_postencode_update{,_gop_const} --- */
    int64_t vbr_bits_off_target;
    int64_t vbr_bits_off_target_fast;
    int32_t rate_error_estimate;
    int32_t active_best_quality[MAX_ARF_LAYERS + 1];
    int32_t extend_minq;
    int32_t extend_maxq;
    int32_t extend_minq_fast;
    int32_t base_frame_target;
    int32_t layer_depth;
    int32_t is_short_clip;
    int64_t param_vbr_bits_off_target;
    int64_t param_vbr_bits_off_target_fast;
    int32_t param_rate_error_estimate;
    int32_t param_extend_minq;
    int32_t param_extend_maxq;
    int32_t param_extend_minq_fast;
} RefRcUpdateState;

static void update_harness_build(RcHarness* h, RateControlIntervalParamContext** param_out,
                                 PictureControlSet** child_out, RefRcUpdateState* st) {
    rc_harness_build(h, &st->base);
    PictureParentControlSet* ppcs    = h->ppcs;
    SequenceControlSet*      scs     = h->scs;
    EncodeContext*           enc_ctx = h->enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;

    rc->last_boosted_qindex     = st->last_boosted_qindex;
    rc->last_q[0]               = st->last_q[0];
    rc->last_q[1]               = st->last_q[1];
    rc->rolling_target_bits     = st->rolling_target_bits;
    rc->rolling_actual_bits     = st->rolling_actual_bits;
    rc->total_actual_bits       = st->total_actual_bits;
    rc->total_target_bits       = st->total_target_bits;
    rc->avg_frame_low_motion    = st->avg_frame_low_motion;
    rc->constrained_gf_group    = st->constrained_gf_group;
    rc->frames_since_cdf_update = st->frames_since_cdf_update;
    rc->frames_to_key           = st->frames_to_key;
    rc->this_key_frame_forced   = st->this_key_frame_forced;
    rc->active_worst_quality    = st->active_worst_quality;
    rc->kf_boost                = st->kf_boost;
    rc->gfu_boost               = st->gfu_boost;
    rc->baseline_gf_interval    = st->baseline_gf_interval;
    rc->resize_state            = (RESIZE_STATE)st->resize_state;
    rc->resize_avg_qp           = st->resize_avg_qp;
    rc->resize_buffer_underflow = st->resize_buffer_underflow;
    rc->resize_count            = st->resize_count;
    rc->vbr_bits_off_target      = st->vbr_bits_off_target;
    rc->vbr_bits_off_target_fast = st->vbr_bits_off_target_fast;
    rc->rate_error_estimate      = st->rate_error_estimate;
    for (int i = 0; i <= MAX_ARF_LAYERS; ++i) {
        rc->active_best_quality[i] = st->active_best_quality[i];
    }
    scs->twopass.extend_minq      = st->extend_minq;
    scs->twopass.extend_maxq      = st->extend_maxq;
    scs->twopass.extend_minq_fast = st->extend_minq_fast;
    scs->is_short_clip            = (uint8_t)st->is_short_clip;

    ppcs->base_frame_target     = st->base_frame_target;
    ppcs->layer_depth           = st->layer_depth;
    ppcs->picture_number        = st->picture_number;
    ppcs->frame_offset          = st->frame_offset;
    ppcs->total_num_bits        = st->total_num_bits;
    ppcs->pcs_total_rate        = st->pcs_total_rate;
    ppcs->frm_hdr.showable_frame = (uint8_t)st->showable_frame;
    ppcs->loop_count            = st->loop_count;
    ppcs->max_frame_size        = st->max_frame_size;
    ppcs->frame_width           = (uint16_t)st->src_frame_width;
    ppcs->frame_height          = (uint16_t)st->src_frame_height;
    ppcs->scene_change_flag     = (bool)st->scene_change_flag;
    ppcs->top_index             = st->top_index;
    ppcs->bottom_index          = st->bottom_index;

    scs->static_config.min_qp_allowed = (uint32_t)st->min_qp_allowed;
    scs->static_config.max_qp_allowed = (uint32_t)st->max_qp_allowed;
    scs->static_config.max_bit_rate   = st->max_bit_rate;
    scs->static_config.rtc            = (bool)st->rtc;
    scs->static_config.intra_period_length = st->intra_period_length;
    scs->max_input_luma_width         = (uint16_t)st->max_input_luma_width;
    scs->max_input_luma_height        = (uint16_t)st->max_input_luma_height;
    scs->new_framerate                = st->new_framerate;
    scs->seq_header.seq_profile       = (EbAv1SeqProfile)st->seq_profile;
    scs->resize_pending_params.resize_state = (RESIZE_STATE)st->pending_resize_state;
    scs->resize_pending_params.resize_denom = (uint8_t)st->pending_resize_denom;

    enc_ctx->recode_loop      = (RecodeLoopType)st->recode_loop;
    enc_ctx->recode_tolerance = st->recode_tolerance;
    enc_ctx->rc_cfg.min_cr    = (unsigned int)st->min_cr;

    RateControlIntervalParamContext* params =
        (RateControlIntervalParamContext*)calloc(1, sizeof(*params));
    params->rolling_target_bits = st->param_rolling_target_bits;
    params->rolling_actual_bits = st->param_rolling_actual_bits;
    params->total_actual_bits   = st->param_total_actual_bits;
    params->total_target_bits   = st->param_total_target_bits;
    params->vbr_bits_off_target      = st->param_vbr_bits_off_target;
    params->vbr_bits_off_target_fast = st->param_vbr_bits_off_target_fast;
    params->rate_error_estimate      = st->param_rate_error_estimate;
    params->extend_minq              = st->param_extend_minq;
    params->extend_maxq              = st->param_extend_maxq;
    params->extend_minq_fast         = st->param_extend_minq_fast;
    ppcs->rate_control_param_ptr = params;
    *param_out                   = params;

    PictureControlSet* child = (PictureControlSet*)calloc(1, sizeof(*child));
    child->ppcs             = ppcs;
    child->avg_cnt_zeromv   = st->avg_cnt_zeromv;
    ppcs->child_pcs         = child;
    *child_out              = child;
}

static void update_harness_store(const RcHarness* h, const RateControlIntervalParamContext* params,
                                 RefRcUpdateState* st) {
    const RATE_CONTROL* rc = &h->enc_ctx->rc;
    rc_harness_store(h, &st->base);
    st->last_boosted_qindex     = rc->last_boosted_qindex;
    st->last_q[0]               = rc->last_q[0];
    st->last_q[1]               = rc->last_q[1];
    st->rolling_target_bits     = rc->rolling_target_bits;
    st->rolling_actual_bits     = rc->rolling_actual_bits;
    st->total_actual_bits       = rc->total_actual_bits;
    st->total_target_bits       = rc->total_target_bits;
    st->avg_frame_low_motion    = rc->avg_frame_low_motion;
    st->frames_since_cdf_update = rc->frames_since_cdf_update;
    st->frames_to_key           = rc->frames_to_key;
    st->this_key_frame_forced   = rc->this_key_frame_forced;
    st->active_worst_quality    = rc->active_worst_quality;
    st->kf_boost                = rc->kf_boost;
    st->gfu_boost               = rc->gfu_boost;
    st->baseline_gf_interval    = rc->baseline_gf_interval;
    st->constrained_gf_group    = rc->constrained_gf_group;
    st->resize_state            = (int32_t)rc->resize_state;
    st->resize_avg_qp           = rc->resize_avg_qp;
    st->resize_buffer_underflow = rc->resize_buffer_underflow;
    st->resize_count            = rc->resize_count;
    st->base.avg_frame_qindex[0] = rc->avg_frame_qindex[0];
    st->base.avg_frame_qindex[1] = rc->avg_frame_qindex[1];
    /* A key frame ZEROES this; without copying it back the differential
       compared C's INPUT against the port's OUTPUT and reported a false
       mismatch. */
    st->base.frames_since_key    = rc->frames_since_key;

    st->param_rolling_target_bits = params->rolling_target_bits;
    st->param_rolling_actual_bits = params->rolling_actual_bits;
    st->param_total_actual_bits   = params->total_actual_bits;
    st->param_total_target_bits   = params->total_target_bits;
    st->param_vbr_bits_off_target      = params->vbr_bits_off_target;
    st->param_vbr_bits_off_target_fast = params->vbr_bits_off_target_fast;
    st->param_rate_error_estimate      = params->rate_error_estimate;
    st->param_extend_minq              = params->extend_minq;
    st->param_extend_maxq              = params->extend_maxq;
    st->param_extend_minq_fast         = params->extend_minq_fast;
    st->vbr_bits_off_target      = rc->vbr_bits_off_target;
    st->vbr_bits_off_target_fast = rc->vbr_bits_off_target_fast;
    st->rate_error_estimate      = rc->rate_error_estimate;
    for (int i = 0; i <= MAX_ARF_LAYERS; ++i) {
        st->active_best_quality[i] = rc->active_best_quality[i];
    }
    st->extend_minq      = h->scs->twopass.extend_minq;
    st->extend_maxq      = h->scs->twopass.extend_maxq;
    st->extend_minq_fast = h->scs->twopass.extend_minq_fast;
    st->base_frame_target = h->ppcs->base_frame_target;

    st->out_projected_frame_size = h->ppcs->projected_frame_size;
    st->out_this_frame_target    = h->ppcs->this_frame_target;
    st->out_base_frame_target    = h->ppcs->base_frame_target;
    st->top_index                = h->ppcs->top_index;
    st->bottom_index             = h->ppcs->bottom_index;
    st->pending_resize_state     = (int32_t)h->scs->resize_pending_params.resize_state;
    st->pending_resize_denom     = h->scs->resize_pending_params.resize_denom;
}

static void update_harness_free(RcHarness* h, RateControlIntervalParamContext* params,
                                PictureControlSet* child) {
    free(child);
    free(params);
    rc_harness_free(h);
}

void ref_rc_postencode_update(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    svt_av1_rc_postencode_update(h.ppcs);
    update_harness_store(&h, params, st);
    update_harness_free(&h, params, child);
}

void ref_rc_postencode_update_gop_const(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    svt_av1_rc_postencode_update_gop_const(h.ppcs);
    update_harness_store(&h, params, st);
    update_harness_free(&h, params, child);
}

void ref_rc_recode_loop_update_q(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    bool loop            = false;
    int  q               = st->recode_q;
    int  q_low           = st->recode_q_low;
    int  q_high          = st->recode_q_high;
    int  undershoot_seen = st->recode_undershoot_seen;
    int  overshoot_seen  = st->recode_overshoot_seen;
    int  low_cr_seen     = st->recode_low_cr_seen;
    recode_loop_update_q(h.ppcs,
                         &loop,
                         &q,
                         &q_low,
                         &q_high,
                         st->top_index,
                         st->bottom_index,
                         &undershoot_seen,
                         &overshoot_seen,
                         &low_cr_seen,
                         st->loop_count);
    update_harness_store(&h, params, st);
    st->recode_q               = q;
    st->recode_q_low           = q_low;
    st->recode_q_high          = q_high;
    st->recode_undershoot_seen = undershoot_seen;
    st->recode_overshoot_seen  = overshoot_seen;
    st->recode_low_cr_seen     = low_cr_seen;
    st->recode_loop_again      = loop ? 1 : 0;
    update_harness_free(&h, params, child);
}

void ref_rc_dynamic_resize_decision(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    svt_aom_dynamic_resize_decision(h.ppcs);
    update_harness_store(&h, params, st);
    update_harness_free(&h, params, child);
}

/* `svt_av1_twopass_postencode_update{,_gop_const}` (pass2_strategy.c:1176 and
 * :1063) — both EXPORTED, and the pair that runs on every frame of a VBR
 * encode. They reuse the same harness; the gop_const one keeps its drift state
 * on `ppcs->rate_control_param_ptr` instead of on RATE_CONTROL/TWO_PASS. */
void ref_twopass_postencode_update(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    svt_av1_twopass_postencode_update(h.ppcs);
    update_harness_store(&h, params, st);
    update_harness_free(&h, params, child);
}

void ref_twopass_postencode_update_gop_const(RefRcUpdateState* st) {
    RcHarness h;
    RateControlIntervalParamContext* params;
    PictureControlSet*               child;
    update_harness_build(&h, &params, &child, st);
    svt_av1_twopass_postencode_update_gop_const(h.ppcs);
    update_harness_store(&h, params, st);
    update_harness_free(&h, params, child);
}
