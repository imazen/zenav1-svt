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
