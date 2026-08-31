/*
 * C shims for the rate-control differential oracle (lane wp-ratecontrol,
 * `Codec/rc_process.c` + `Codec/pass2_strategy.c`).
 *
 * Most of this group's C surface is directly linkable (see
 * `src/rate_control.rs` — `svt_av1_rc_bits_per_mb`,
 * `svt_av1_get_cqp_kf_boost_from_r0`, `svt_av1_get_gfu_boost_from_r0_lap`,
 * `svt_av1_calculate_boost_bits` and every const table are plain exported
 * symbols bound with no wrapper at all). Only the functions that take an
 * SVT struct need a shim, and they are here.
 *
 * RULE, inherited from ref_shims.c and re-stated because it was paid for:
 * A SHIM MUST NOT KEEP PER-CALL STATE IN A `static`. cargo runs a test
 * binary's tests on several threads, so a `static` scratch struct is a data
 * race that fails as an occasional wrong NUMBER, not as a crash. Every
 * struct below is a STACK local, zero-initialised per call.
 */
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "rc_process.h"

/* `svt_av1_compute_qdelta_by_rate` (rc_process.c:290) takes a RATE_CONTROL*
   but reads exactly two fields off it — `rc->best_quality` and
   `rc->worst_quality` (rc_process.c:314, as the binary search's bounds).
   Verified by reading the function body: nothing else on `rc` is touched, and
   the callee `find_qindex_by_rate` takes those two as plain ints. So a
   stack-allocated, zeroed RATE_CONTROL with those two set drives the REAL
   exported symbol on the real code path. */
int32_t ref_rc_compute_qdelta_by_rate(int32_t best_quality, int32_t worst_quality, int32_t frame_type, int32_t qindex,
                                      double rate_target_ratio, int32_t bit_depth, int32_t is_screen_content_type) {
    RATE_CONTROL rc;
    memset(&rc, 0, sizeof(rc));
    rc.best_quality  = best_quality;
    rc.worst_quality = worst_quality;
    return svt_av1_compute_qdelta_by_rate(
        &rc, (FrameType)frame_type, qindex, rate_target_ratio, bit_depth, is_screen_content_type);
}

/* `svt_av1_convert_qindex_to_q` (rc_process.c:185) is exported and takes only
   scalars, but its second argument is the `EbBitDepth` enum whose numeric
   values are 8/10/12 rather than 0/1/2; the shim pins the cast in C so the
   Rust binding cannot get the enum ABI wrong. */
double ref_rc_convert_qindex_to_q(int32_t qindex, int32_t bit_depth) {
    return svt_av1_convert_qindex_to_q(qindex, (EbBitDepth)bit_depth);
}

/* ------------------------------------------------------------------------
 * `svt_aom_set_rc_param` (pass2_strategy.c:906) and `svt_av1_rc_init`
 * (rc_process.c:495), both EXPORTED, both taking a `SequenceControlSet*`.
 *
 * Both are driven for real here: the shim `calloc`s a SequenceControlSet and
 * an EncodeContext PER CALL (never `static` — see the header rule), wires
 * `scs->enc_ctx`, copies the caller's inputs into the fields the C functions
 * actually read, calls the real symbol, and copies the outputs back. Nothing
 * about either function is transcribed on this side.
 * ---------------------------------------------------------------------- */
#include "encode_context.h"
#include "sequence_control_set.h"
#include "pass2_strategy.h"

/* Inputs `svt_aom_set_rc_param` reads, flattened. Mirrors the Rust
   `SetRcParamInput`; field order is load-bearing across the FFI boundary. */
typedef struct {
    int32_t  first_pass_downsample;
    uint32_t max_input_luma_width;
    uint32_t max_input_luma_height;
    int32_t  encoder_bit_depth;
    int32_t  vbr_min_section_pct;
    int32_t  vbr_max_section_pct;
    int32_t  rate_control_mode;
    int32_t  min_qp_allowed;
    int32_t  max_qp_allowed;
    int32_t  gop_constraint_rc;
    int32_t  over_shoot_pct;
    int32_t  under_shoot_pct;
    int64_t  maximum_buffer_size_ms;
    int64_t  starting_buffer_level_ms;
    int64_t  optimal_buffer_level_ms;
    uint32_t max_intra_bitrate_pct;
    uint32_t max_inter_bitrate_pct;
    int32_t  sframe_dist;
    int32_t  sframe_mode;
} RefSetRcParamIn;

/* Outputs it writes. */
typedef struct {
    int32_t frame_width;
    int32_t frame_height;
    int32_t mb_rows;
    int32_t mb_cols;
    int32_t num_mbs;
    int32_t bit_depth;
    int32_t vbrmin_section;
    int32_t vbrmax_section;
    int32_t mode;
    int32_t best_allowed_q;
    int32_t worst_allowed_q;
    int32_t over_shoot_pct;
    int32_t under_shoot_pct;
    int64_t maximum_buffer_size_ms;
    int64_t starting_buffer_level_ms;
    int64_t optimal_buffer_level_ms;
    uint32_t max_intra_bitrate_pct;
    uint32_t max_inter_bitrate_pct;
    int32_t sframe_dist;
    int32_t sframe_mode;
} RefSetRcParamOut;

void ref_rc_set_rc_param(const RefSetRcParamIn* in, RefSetRcParamOut* out) {
    SequenceControlSet* scs     = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    EncodeContext*      enc_ctx = (EncodeContext*)calloc(1, sizeof(EncodeContext));
    scs->enc_ctx                = enc_ctx;

    scs->first_pass_downsample                = (bool)in->first_pass_downsample;
    scs->max_input_luma_width                 = in->max_input_luma_width;
    scs->max_input_luma_height                = in->max_input_luma_height;
    scs->static_config.encoder_bit_depth      = (EbBitDepth)in->encoder_bit_depth;
    scs->static_config.vbr_min_section_pct    = in->vbr_min_section_pct;
    scs->static_config.vbr_max_section_pct    = in->vbr_max_section_pct;
    scs->static_config.rate_control_mode      = (uint8_t)in->rate_control_mode;
    scs->static_config.min_qp_allowed         = in->min_qp_allowed;
    scs->static_config.max_qp_allowed         = in->max_qp_allowed;
    scs->static_config.gop_constraint_rc      = (bool)in->gop_constraint_rc;
    scs->static_config.over_shoot_pct         = in->over_shoot_pct;
    scs->static_config.under_shoot_pct        = in->under_shoot_pct;
    scs->static_config.maximum_buffer_size_ms = in->maximum_buffer_size_ms;
    scs->static_config.starting_buffer_level_ms = in->starting_buffer_level_ms;
    scs->static_config.optimal_buffer_level_ms  = in->optimal_buffer_level_ms;
    scs->static_config.max_intra_bitrate_pct     = in->max_intra_bitrate_pct;
    scs->static_config.max_inter_bitrate_pct     = in->max_inter_bitrate_pct;
    scs->static_config.sframe_dist               = in->sframe_dist;
    scs->static_config.sframe_mode               = (EbSFrameMode)in->sframe_mode;

    svt_aom_set_rc_param(scs);

    out->frame_width              = enc_ctx->frame_info.frame_width;
    out->frame_height             = enc_ctx->frame_info.frame_height;
    out->mb_rows                  = enc_ctx->frame_info.mb_rows;
    out->mb_cols                  = enc_ctx->frame_info.mb_cols;
    out->num_mbs                  = enc_ctx->frame_info.num_mbs;
    out->bit_depth                = (int32_t)enc_ctx->frame_info.bit_depth;
    out->vbrmin_section           = enc_ctx->two_pass_cfg.vbrmin_section;
    out->vbrmax_section           = enc_ctx->two_pass_cfg.vbrmax_section;
    out->mode                     = (int32_t)enc_ctx->rc_cfg.mode;
    out->best_allowed_q           = enc_ctx->rc_cfg.best_allowed_q;
    out->worst_allowed_q          = enc_ctx->rc_cfg.worst_allowed_q;
    out->over_shoot_pct           = enc_ctx->rc_cfg.over_shoot_pct;
    out->under_shoot_pct          = enc_ctx->rc_cfg.under_shoot_pct;
    out->maximum_buffer_size_ms   = enc_ctx->rc_cfg.maximum_buffer_size_ms;
    out->starting_buffer_level_ms = enc_ctx->rc_cfg.starting_buffer_level_ms;
    out->optimal_buffer_level_ms  = enc_ctx->rc_cfg.optimal_buffer_level_ms;
    out->max_intra_bitrate_pct    = enc_ctx->rc_cfg.max_intra_bitrate_pct;
    out->max_inter_bitrate_pct    = enc_ctx->rc_cfg.max_inter_bitrate_pct;
    out->sframe_dist              = enc_ctx->sf_cfg.sframe_dist;
    out->sframe_mode              = (int32_t)enc_ctx->sf_cfg.sframe_mode;

    free(enc_ctx);
    free(scs);
}

/* Inputs `svt_av1_rc_init` reads: the rc_cfg the call above produced, the
   two RATE_CONTROL fields it copies forward, and hierarchical_levels. */
typedef struct {
    int32_t mode;
    int32_t best_allowed_q;
    int32_t worst_allowed_q;
    int64_t starting_buffer_level;
    int32_t avg_frame_bandwidth;
    int32_t hierarchical_levels;
    int32_t frame_rate_numerator;
    int32_t frame_rate_denominator;
} RefRcInitIn;

/* Outputs it writes (the fields any downstream consumer reads). */
typedef struct {
    int32_t avg_frame_qindex_key;
    int32_t avg_frame_qindex_inter;
    int32_t last_q_key;
    int32_t last_q_inter;
    int64_t buffer_level;
    int64_t bits_off_target;
    int32_t rolling_target_bits;
    int32_t rolling_actual_bits;
    int64_t total_actual_bits;
    int64_t total_target_bits;
    int32_t frames_since_key;
    int32_t frames_since_cdf_update;
    int32_t this_key_frame_forced;
    double  rate_correction_factors[MAX_TEMPORAL_LAYERS + 1];
    int32_t baseline_gf_interval;
    int32_t worst_quality;
    int32_t best_quality;
    uint32_t cur_avg_base_me_dist;
    uint32_t prev_avg_base_me_dist;
    int32_t  avg_frame_low_motion;
} RefRcInitOut;

void ref_rc_init(const RefRcInitIn* in, RefRcInitOut* out) {
    SequenceControlSet* scs     = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    EncodeContext*      enc_ctx = (EncodeContext*)calloc(1, sizeof(EncodeContext));
    scs->enc_ctx                = enc_ctx;

    enc_ctx->rc_cfg.mode            = (enum aom_rc_mode)in->mode;
    enc_ctx->rc_cfg.best_allowed_q  = in->best_allowed_q;
    enc_ctx->rc_cfg.worst_allowed_q = in->worst_allowed_q;
    enc_ctx->rc.starting_buffer_level = in->starting_buffer_level;
    enc_ctx->rc.avg_frame_bandwidth   = in->avg_frame_bandwidth;
    scs->static_config.hierarchical_levels    = (uint32_t)in->hierarchical_levels;
    scs->static_config.frame_rate_numerator   = (uint32_t)in->frame_rate_numerator;
    scs->static_config.frame_rate_denominator = (uint32_t)in->frame_rate_denominator;

    svt_av1_rc_init(scs);

    RATE_CONTROL* rc              = &enc_ctx->rc;
    out->avg_frame_qindex_key     = rc->avg_frame_qindex[KEY_FRAME];
    out->avg_frame_qindex_inter   = rc->avg_frame_qindex[INTER_FRAME];
    out->last_q_key               = rc->last_q[KEY_FRAME];
    out->last_q_inter             = rc->last_q[INTER_FRAME];
    out->buffer_level             = rc->buffer_level;
    out->bits_off_target          = rc->bits_off_target;
    out->rolling_target_bits      = rc->rolling_target_bits;
    out->rolling_actual_bits      = rc->rolling_actual_bits;
    out->total_actual_bits        = rc->total_actual_bits;
    out->total_target_bits        = rc->total_target_bits;
    out->frames_since_key         = rc->frames_since_key;
    out->frames_since_cdf_update  = rc->frames_since_cdf_update;
    out->this_key_frame_forced    = rc->this_key_frame_forced;
    for (int i = 0; i < MAX_TEMPORAL_LAYERS + 1; ++i) {
        out->rate_correction_factors[i] = rc->rate_correction_factors[i];
    }
    out->baseline_gf_interval  = rc->baseline_gf_interval;
    out->worst_quality         = rc->worst_quality;
    out->best_quality          = rc->best_quality;
    out->cur_avg_base_me_dist  = rc->cur_avg_base_me_dist;
    out->prev_avg_base_me_dist = rc->prev_avg_base_me_dist;
    out->avg_frame_low_motion  = rc->avg_frame_low_motion;

    free(enc_ctx);
    free(scs);
}
