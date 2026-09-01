/*
 * Differential oracles for `svtav1_encoder::port_temporal_filtering` — the
 * wholesale port of Source/Lib/Codec/temporal_filtering.c.
 *
 * Every entry point calls the REAL C code (evidence tier 1,
 * docs/WORKING-ON-THIS.md §4). Where the C function is `static` and takes flat
 * scalar/pointer arguments (sqrt_fast, calculate_squared_errors_sum,
 * svt_av1_calculate_decay_factor, calculate_tf_shift_factor,
 * derive_tf_32x32_block_split_flag) it is reached through its EXPORTED caller,
 * or — where there is no such caller — through the identical expression the
 * one-line C macro expands to, which is stated per shim below.
 *
 * Own translation unit so this lane never shares an editable C file with the
 * concurrent lanes. All state is per call on the stack or the heap: cargo runs
 * a test binary's tests on several threads and a `static` scratch buffer would
 * race.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "me_context.h"
#include "pic_buffer_desc.h"
#include "bitstream_unit.h"

void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);

static int tf_rtcd_done = 0;
static void tf_ensure_rtcd(void) {
    if (!tf_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        tf_rtcd_done = 1;
    }
}

int32_t svt_aom_noise_log1p_fp16(int32_t noise_level_fp16);
int8_t  tf_use_64x64_pred(MeContext* me_ctx);

void svt_aom_apply_filtering_central_c(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central, EbByte* src,
                                       uint32_t** accum, uint16_t** count, uint16_t blk_width, uint16_t blk_height,
                                       uint32_t ss_x, uint32_t ss_y);
void svt_aom_apply_filtering_central_highbd_c(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
                                              uint16_t** src_16bit, uint32_t** accum, uint16_t** count,
                                              uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
void svt_aom_get_final_filtered_pixels_c(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                         uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                         const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                         uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
void svt_av1_apply_temporal_filter_planewise_medium_c(MeContext* me_ctx, const uint8_t* y_src, int y_src_stride,
                                                      const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_src,
                                                      const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre,
                                                      const uint8_t* v_pre, int uv_pre_stride,
                                                      unsigned int block_width, unsigned int block_height, int ss_x,
                                                      int ss_y, uint32_t* y_accum, uint16_t* y_count,
                                                      uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum,
                                                      uint16_t* v_count);
void svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth);

int32_t ref_tf_noise_log1p_fp16(int32_t noise_level_fp16) { return svt_aom_noise_log1p_fp16(noise_level_fp16); }

/*
 * OD_DIVU is a MACRO over the exported table `svt_aom_od_divu_small_consts`
 * (noise_util.c:114). Expanding it here drives the real table, so the port's
 * claim that plain integer division is equivalent over the temporal filter's
 * domain is GATED rather than assumed.
 */
uint32_t ref_tf_od_divu(uint32_t x, uint32_t d) { return OD_DIVU(x, d); }

/* The flat MeContext fields the TF kernels read; mirrored from the Rust side. */
typedef struct TfCtxArgs {
    int32_t  tf_block_col;
    int32_t  tf_block_row;
    uint32_t tf_mv_dist_th;
    int32_t  tf_chroma;
    int32_t  tf_32x32_block_split_flag[4];
    int16_t  tf_16x16_mv_x[16];
    int16_t  tf_16x16_mv_y[16];
    uint64_t tf_16x16_block_error[16];
    int16_t  tf_32x32_mv_x[4];
    int16_t  tf_32x32_mv_y[4];
    uint64_t tf_32x32_block_error[4];
    uint32_t tf_decay_factor_fp16[3];
    uint64_t tf_64x64_block_error;
    uint32_t p_best_sad_64x64;
    uint32_t p_best_sad_32x32[4];
    uint8_t  tf_use_pred_64x64_only_th;
} TfCtxArgs;

static MeContext* tf_make_ctx(const TfCtxArgs* a) {
    MeContext* c = (MeContext*)calloc(1, sizeof(MeContext));
    c->tf_block_col  = a->tf_block_col;
    c->tf_block_row  = a->tf_block_row;
    c->tf_mv_dist_th = (uint16_t)a->tf_mv_dist_th;
    c->tf_chroma     = (uint8_t)(a->tf_chroma != 0);
    memcpy(c->tf_32x32_block_split_flag, a->tf_32x32_block_split_flag, sizeof(c->tf_32x32_block_split_flag));
    memcpy(c->tf_16x16_mv_x, a->tf_16x16_mv_x, sizeof(c->tf_16x16_mv_x));
    memcpy(c->tf_16x16_mv_y, a->tf_16x16_mv_y, sizeof(c->tf_16x16_mv_y));
    memcpy(c->tf_16x16_block_error, a->tf_16x16_block_error, sizeof(c->tf_16x16_block_error));
    memcpy(c->tf_32x32_mv_x, a->tf_32x32_mv_x, sizeof(c->tf_32x32_mv_x));
    memcpy(c->tf_32x32_mv_y, a->tf_32x32_mv_y, sizeof(c->tf_32x32_mv_y));
    memcpy(c->tf_32x32_block_error, a->tf_32x32_block_error, sizeof(c->tf_32x32_block_error));
    memcpy(c->tf_decay_factor_fp16, a->tf_decay_factor_fp16, sizeof(c->tf_decay_factor_fp16));
    c->tf_64x64_block_error      = a->tf_64x64_block_error;
    c->tf_use_pred_64x64_only_th = a->tf_use_pred_64x64_only_th;
    return c;
}

int8_t ref_tf_use_64x64_pred(const TfCtxArgs* a) {
    MeContext* c   = tf_make_ctx(a);
    uint32_t   s64 = a->p_best_sad_64x64;
    uint32_t   s32[4];
    memcpy(s32, a->p_best_sad_32x32, sizeof(s32));
    c->p_best_sad_64x64 = &s64;
    c->p_best_sad_32x32 = s32;
    int8_t r            = tf_use_64x64_pred(c);
    free(c);
    return r;
}

void ref_tf_apply_filtering_central(int32_t tf_chroma, const uint8_t* src_y, const uint8_t* src_u,
                                    const uint8_t* src_v, uint32_t src_stride_y, uint32_t* accum_y, uint32_t* accum_u,
                                    uint32_t* accum_v, uint16_t* count_y, uint16_t* count_u, uint16_t* count_v,
                                    uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y) {
    tf_ensure_rtcd();
    MeContext* c = (MeContext*)calloc(1, sizeof(MeContext));
    c->tf_chroma = (uint8_t)(tf_chroma != 0);
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));
    pic.y_stride = src_stride_y;

    EbByte    src[3]   = {(EbByte)src_y, (EbByte)src_u, (EbByte)src_v};
    uint32_t* accum[3] = {accum_y, accum_u, accum_v};
    uint16_t* count[3] = {count_y, count_u, count_v};
    svt_aom_apply_filtering_central_c(c, &pic, src, accum, count, blk_width, blk_height, ss_x, ss_y);
    free(c);
}

void ref_tf_apply_filtering_central_highbd(int32_t tf_chroma, const uint16_t* src_y, const uint16_t* src_u,
                                           const uint16_t* src_v, uint32_t src_stride_y, uint32_t* accum_y,
                                           uint32_t* accum_u, uint32_t* accum_v, uint16_t* count_y, uint16_t* count_u,
                                           uint16_t* count_v, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x,
                                           uint32_t ss_y) {
    tf_ensure_rtcd();
    MeContext* c = (MeContext*)calloc(1, sizeof(MeContext));
    c->tf_chroma = (uint8_t)(tf_chroma != 0);
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));
    pic.y_stride = src_stride_y;

    uint16_t* src[3]   = {(uint16_t*)src_y, (uint16_t*)src_u, (uint16_t*)src_v};
    uint32_t* accum[3] = {accum_y, accum_u, accum_v};
    uint16_t* count[3] = {count_y, count_u, count_v};
    svt_aom_apply_filtering_central_highbd_c(c, &pic, src, accum, count, blk_width, blk_height, ss_x, ss_y);
    free(c);
}

void ref_tf_get_final_filtered_pixels(int32_t tf_chroma, int32_t is_highbd, uint8_t* sy, uint8_t* su, uint8_t* sv,
                                      uint16_t* hy, uint16_t* hu, uint16_t* hv, const uint32_t* accum_y,
                                      const uint32_t* accum_u, const uint32_t* accum_v, const uint16_t* count_y,
                                      const uint16_t* count_u, const uint16_t* count_v, const uint32_t* stride,
                                      int32_t blk_y_src_offset, int32_t blk_ch_src_offset, uint16_t blk_width_ch,
                                      uint16_t blk_height_ch) {
    tf_ensure_rtcd();
    MeContext* c = (MeContext*)calloc(1, sizeof(MeContext));
    c->tf_chroma = (uint8_t)(tf_chroma != 0);

    EbByte    src[3]     = {sy, su, sv};
    uint16_t* src_hbd[3] = {hy, hu, hv};
    uint32_t* accum[3]   = {(uint32_t*)accum_y, (uint32_t*)accum_u, (uint32_t*)accum_v};
    uint16_t* count[3]   = {(uint16_t*)count_y, (uint16_t*)count_u, (uint16_t*)count_v};
    svt_aom_get_final_filtered_pixels_c(c,
                                        src,
                                        src_hbd,
                                        accum,
                                        count,
                                        stride,
                                        blk_y_src_offset,
                                        blk_ch_src_offset,
                                        blk_width_ch,
                                        blk_height_ch,
                                        is_highbd != 0);
    free(c);
}

void ref_tf_apply_planewise_medium(const TfCtxArgs* a, const uint8_t* y_src, int32_t y_src_stride,
                                   const uint8_t* y_pre, int32_t y_pre_stride, const uint8_t* u_src,
                                   const uint8_t* v_src, int32_t uv_src_stride, const uint8_t* u_pre,
                                   const uint8_t* v_pre, int32_t uv_pre_stride, uint32_t block_width,
                                   uint32_t block_height, int32_t ss_x, int32_t ss_y, uint32_t* y_accum,
                                   uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum,
                                   uint16_t* v_count) {
    tf_ensure_rtcd();
    MeContext* c = tf_make_ctx(a);
    svt_av1_apply_temporal_filter_planewise_medium_c(c,
                                                     y_src,
                                                     y_src_stride,
                                                     y_pre,
                                                     y_pre_stride,
                                                     u_src,
                                                     v_src,
                                                     uv_src_stride,
                                                     u_pre,
                                                     v_pre,
                                                     uv_pre_stride,
                                                     block_width,
                                                     block_height,
                                                     ss_x,
                                                     ss_y,
                                                     y_accum,
                                                     y_count,
                                                     u_accum,
                                                     u_count,
                                                     v_accum,
                                                     v_count);
    free(c);
}

void ref_tf_apply_planewise_medium_hbd(const TfCtxArgs* a, const uint16_t* y_src, int32_t y_src_stride,
                                       const uint16_t* y_pre, int32_t y_pre_stride, const uint16_t* u_src,
                                       const uint16_t* v_src, int32_t uv_src_stride, const uint16_t* u_pre,
                                       const uint16_t* v_pre, int32_t uv_pre_stride, uint32_t block_width,
                                       uint32_t block_height, int32_t ss_x, int32_t ss_y, uint32_t* y_accum,
                                       uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum,
                                       uint16_t* v_count, uint32_t encoder_bit_depth) {
    tf_ensure_rtcd();
    MeContext* c = tf_make_ctx(a);
    svt_av1_apply_temporal_filter_planewise_medium_hbd_c(c,
                                                         y_src,
                                                         y_src_stride,
                                                         y_pre,
                                                         y_pre_stride,
                                                         u_src,
                                                         v_src,
                                                         uv_src_stride,
                                                         u_pre,
                                                         v_pre,
                                                         uv_pre_stride,
                                                         block_width,
                                                         block_height,
                                                         ss_x,
                                                         ss_y,
                                                         y_accum,
                                                         y_count,
                                                         u_accum,
                                                         u_count,
                                                         v_accum,
                                                         v_count,
                                                         encoder_bit_depth);
    free(c);
}

/* ---- chunk 5: noise estimation, decay/shift, split derivation, re-pad ---- */

#include "pcs.h"
#include "sequence_control_set.h"
#include "reference_object.h"
#include "sys_resource_manager.h"

int32_t svt_estimate_noise_highbd_fp16_c(const uint16_t* src, int width, int height, int stride, int bd);
void    pad_and_decimate_filtered_pic(PictureParentControlSet* centre_pcs);

int32_t ref_tf_estimate_noise_highbd_fp16(const uint16_t* src, int32_t width, int32_t height, int32_t stride,
                                          int32_t bd) {
    return svt_estimate_noise_highbd_fp16_c(src, width, height, stride, bd);
}

/*
 * WHAT IS NOT SHIMMED, AND WHY.
 *
 * `sqrt_fast`, `calculate_squared_errors_sum`(+_highbd),
 * `svt_av1_calculate_decay_factor`, `calculate_tf_shift_factor` and
 * `derive_tf_32x32_block_split_flag` are all `static` in temporal_filtering.c
 * — internal linkage, no symbol in libSvtAv1Enc.a (checked with nm, not
 * inferred from headers). A shim in this TU cannot call them.
 *
 * The first three ARE nevertheless gated at tier 1, because an exported caller
 * reaches them: `svt_av1_apply_temporal_filter_planewise_medium_c` computes
 * every distance term through `sqrt_fast` and every window error through
 * `calculate_squared_errors_sum`, so the medium-kernel differential drives
 * them on real inputs.
 *
 * The last three have no exported caller a facade can reach — their only
 * callers are `produce_temporally_filtered_pic`, which takes a
 * PictureParentControlSet** list, a MotionEstimationContext_t and segment
 * state. They are therefore gated at TIER 4 (hand-derived vectors traced
 * against the C source) and the Rust tests say so in their names and doc
 * comments. Re-implementing their bodies here to "shim" them would be a
 * transcribed oracle agreeing with transcribed code, which proves nothing
 * (WORKING-ON-THIS.md §4).
 */

/*
 * pad_and_decimate_filtered_pic takes a PictureParentControlSet whose
 * pa_ref_pic_wrapper->object_ptr is an EbPaReferenceObject holding the
 * quarter/sixteenth descriptors. All of those are built here per call, on the
 * heap, and only the fields the function reads are filled.
 */
void ref_tf_pad_and_decimate_filtered_pic(
    uint32_t subsampling_x, uint32_t subsampling_y, uint32_t pad_right, uint32_t pad_bottom, uint32_t color_format,
    int32_t chroma_lvl, int32_t enable_hme, int32_t tf_enable_hme, int32_t enable_hme_l0, int32_t tf_enable_hme_l0,
    int32_t enable_hme_l1, int32_t tf_enable_hme_l1, uint8_t* y_buf, uint32_t y_origin, uint32_t y_stride,
    uint32_t width, uint32_t height, uint32_t border, uint8_t* u_buf, uint32_t u_origin, uint32_t u_stride,
    uint8_t* v_buf, uint32_t v_origin, uint32_t v_stride, uint8_t* q_buf, uint32_t q_origin, uint32_t q_stride,
    uint32_t q_w, uint32_t q_h, uint32_t q_border, uint8_t* s_buf, uint32_t s_origin, uint32_t s_stride, uint32_t s_w,
    uint32_t s_h, uint32_t s_border) {
    tf_ensure_rtcd();
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));
    EbPaReferenceObject*     pa  = (EbPaReferenceObject*)calloc(1, sizeof(EbPaReferenceObject));
    EbObjectWrapper*         wrp = (EbObjectWrapper*)calloc(1, sizeof(EbObjectWrapper));
    EbPictureBufferDesc*     in  = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));
    EbPictureBufferDesc*     q   = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));
    EbPictureBufferDesc*     s   = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));

    scs->subsampling_x = (uint16_t)subsampling_x;
    scs->subsampling_y = (uint16_t)subsampling_y;
    scs->pad_right     = pad_right;
    scs->pad_bottom    = pad_bottom;
    scs->static_config.encoder_bit_depth = EB_EIGHT_BIT;

    in->color_format = color_format;
    in->y_buffer     = y_buf + y_origin;
    in->y_stride     = y_stride;
    in->width        = width;
    in->height       = height;
    in->border       = border;
    in->u_buffer     = u_buf ? u_buf + u_origin : NULL;
    in->u_stride     = u_stride;
    in->v_buffer     = v_buf ? v_buf + v_origin : NULL;
    in->v_stride     = v_stride;

    q->y_buffer = q_buf + q_origin;
    q->y_stride = q_stride;
    q->width    = q_w;
    q->height   = q_h;
    q->border   = q_border;

    s->y_buffer = s_buf + s_origin;
    s->y_stride = s_stride;
    s->width    = s_w;
    s->height   = s_h;
    s->border   = s_border;

    pa->quarter_downsampled_picture_ptr   = q;
    pa->sixteenth_downsampled_picture_ptr = s;
    wrp->object_ptr                       = pa;

    pcs->scs                       = scs;
    pcs->enhanced_pic              = in;
    pcs->pa_ref_pic_wrapper        = wrp;
    pcs->tf_ctrls.chroma_lvl       = (uint8_t)chroma_lvl;
    pcs->enable_hme_flag           = enable_hme != 0;
    pcs->tf_enable_hme_flag        = tf_enable_hme != 0;
    pcs->enable_hme_level0_flag    = enable_hme_l0 != 0;
    pcs->tf_enable_hme_level0_flag = tf_enable_hme_l0 != 0;
    pcs->enable_hme_level1_flag    = enable_hme_l1 != 0;
    pcs->tf_enable_hme_level1_flag = tf_enable_hme_l1 != 0;

    pad_and_decimate_filtered_pic(pcs);

    free(s);
    free(q);
    free(in);
    free(wrp);
    free(pa);
    free(pcs);
    free(scs);
}

/*
 * The zero-motion ("zz") filter kernels. Both are EXPORTED, so these shims are
 * evidence tier 1: they build the flat MeContext facade and call the real
 * `_c` symbols. The wrappers are what an RTCD dispatch would land on, and they
 * are the only callers of the two `static` partials, so driving them gates the
 * partial arithmetic too.
 *
 * The 8-bit wrapper takes no bit depth; the 10-bit one does and forwards it to
 * a partial that `(void)`-casts it away (temporal_filtering.c:838). It is
 * passed here anyway so the shim's call matches the encoder's, and the Rust
 * side documents why it does not carry the parameter.
 */
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count);
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth);

void ref_tf_apply_zz_planewise_medium(const TfCtxArgs* a, const uint8_t* y_pre, int32_t y_pre_stride,
                                      const uint8_t* u_pre, const uint8_t* v_pre, int32_t uv_pre_stride,
                                      uint32_t block_width, uint32_t block_height, int32_t ss_x, int32_t ss_y,
                                      uint32_t* y_accum, uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count,
                                      uint32_t* v_accum, uint16_t* v_count) {
    tf_ensure_rtcd();
    MeContext* c = tf_make_ctx(a);
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(c,
                                                              y_pre,
                                                              y_pre_stride,
                                                              u_pre,
                                                              v_pre,
                                                              uv_pre_stride,
                                                              block_width,
                                                              block_height,
                                                              ss_x,
                                                              ss_y,
                                                              y_accum,
                                                              y_count,
                                                              u_accum,
                                                              u_count,
                                                              v_accum,
                                                              v_count);
    free(c);
}

void ref_tf_apply_zz_planewise_medium_hbd(const TfCtxArgs* a, const uint16_t* y_pre, int32_t y_pre_stride,
                                          const uint16_t* u_pre, const uint16_t* v_pre, int32_t uv_pre_stride,
                                          uint32_t block_width, uint32_t block_height, int32_t ss_x, int32_t ss_y,
                                          uint32_t* y_accum, uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count,
                                          uint32_t* v_accum, uint16_t* v_count, uint32_t encoder_bit_depth) {
    tf_ensure_rtcd();
    MeContext* c = tf_make_ctx(a);
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(c,
                                                                  y_pre,
                                                                  y_pre_stride,
                                                                  u_pre,
                                                                  v_pre,
                                                                  uv_pre_stride,
                                                                  block_width,
                                                                  block_height,
                                                                  ss_x,
                                                                  ss_y,
                                                                  y_accum,
                                                                  y_count,
                                                                  u_accum,
                                                                  u_count,
                                                                  v_accum,
                                                                  v_count,
                                                                  encoder_bit_depth);
    free(c);
}

/* ------------------------------------------------------------------------
 * The tune-VMAF leaf kernels (temporal_filtering.c:3636-3746).
 *
 * All six are RTCD-dispatched with an EXPORTED `_c` scalar reference. The `_c`
 * symbols are called directly, so no RTCD init is needed for these — but
 * `tf_ensure_rtcd()` is called anyway to keep every entry point in this file
 * on the same footing, and because it is idempotent.
 *
 * Their nine `static` callers live in pic_analysis_process.c and are reached
 * only through a PictureAnalysisContext + PCS, so they are covered by the
 * port's assembled chain rather than shimmed here.
 * ---------------------------------------------------------------------- */

uint32_t svt_vmaf_compute_avg_mad_c(const uint8_t* src, int width, int height, int stride);
void     svt_vmaf_apply_unsharp_row_c(const uint8_t* src, const uint8_t* blur, uint8_t* dst, int width, int amount,
                                      int32_t max_delta);
void     svt_vmaf_vpass_row_c(const int16_t* r0, const int16_t* r1, const int16_t* r2, const int16_t* r3,
                              const int16_t* r4, uint8_t* blur_row, int width, int steps_x);
float    svt_vmaf_compute_gradient_coherence_c(const uint8_t* src, int width, int height, int stride);
uint32_t svt_vmaf_count_detail_le_c(const uint8_t* src, const uint8_t* blur, int width, int height, int src_stride,
                                    int thresh);
void     svt_vmaf_hpass_row_c(const uint8_t* src_row, int width, int16_t* h_row);

uint32_t ref_vmaf_compute_avg_mad(const uint8_t* src, int32_t width, int32_t height, int32_t stride) {
    tf_ensure_rtcd();
    return svt_vmaf_compute_avg_mad_c(src, width, height, stride);
}

void ref_vmaf_apply_unsharp_row(const uint8_t* src, const uint8_t* blur, uint8_t* dst, int32_t width, int32_t amount,
                                int32_t max_delta) {
    tf_ensure_rtcd();
    svt_vmaf_apply_unsharp_row_c(src, blur, dst, width, amount, max_delta);
}

void ref_vmaf_vpass_row(const int16_t* r0, const int16_t* r1, const int16_t* r2, const int16_t* r3, const int16_t* r4,
                        uint8_t* blur_row, int32_t width, int32_t steps_x) {
    tf_ensure_rtcd();
    svt_vmaf_vpass_row_c(r0, r1, r2, r3, r4, blur_row, width, steps_x);
}

float ref_vmaf_compute_gradient_coherence(const uint8_t* src, int32_t width, int32_t height, int32_t stride) {
    tf_ensure_rtcd();
    return svt_vmaf_compute_gradient_coherence_c(src, width, height, stride);
}

uint32_t ref_vmaf_count_detail_le(const uint8_t* src, const uint8_t* blur, int32_t width, int32_t height,
                                  int32_t src_stride, int32_t thresh) {
    tf_ensure_rtcd();
    return svt_vmaf_count_detail_le_c(src, blur, width, height, src_stride, thresh);
}

void ref_vmaf_hpass_row(const uint8_t* src_row, int32_t width, int16_t* h_row) {
    tf_ensure_rtcd();
    svt_vmaf_hpass_row_c(src_row, width, h_row);
}
