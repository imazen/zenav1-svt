/*
 * C shims for the wx-intra-dblk lane: Codec/pic_operators.c,
 * Codec/deblocking_common.c and the residual entry points of
 * Codec/intra_prediction.c.
 *
 * Its own translation unit so this lane never shares a shim file with
 * another lane.
 *
 * RULE (see the header of ref_shims.c): NO PER-CALL STATE IN A `static`.
 * The only file-scope objects here are the two idempotent one-shot RTCD /
 * predictor-table init flags, which are initialization latches, not state.
 *
 * WHY THE RTCD LATCH EXISTS (docs/WORKING-ON-THIS.md §5, trap 2). Three of
 * the entry points below reach DISPATCHED kernels, not direct calls:
 *
 *   svt_aom_picture_full_distortion32_bits_single
 *       -> svt_full_distortion_kernel32_bits / _cbf_zero32_bits
 *   svt_spatial_full_distortion_kernel_facade
 *       -> svt_spatial_full_distortion_kernel  (or the 16-bit kernel)
 *   svt_aom_intra_prediction_open_loop_mb / svt_aom_dr_predictor
 *       -> svt_aom_eb_pred[][] / svt_aom_dc_pred[][][]
 *
 * The first pair IS #define'd to the _neon kernels by
 * common_dsp_rtcd_neon_devirt.h, so on aarch64 no pointer exists to be
 * null and a missing init is invisible. `svt_spatial_full_distortion_kernel`
 * is NOT in that devirt list: it stays an RTCD_EXTERN pointer in .bss on
 * BOTH ISAs and is null until svt_aom_setup_common_rtcd_internal runs. The
 * intra tables are plain arrays filled by two separate init functions that
 * enc_handle.c:1275-1277 calls once at library init. Calling all four here
 * makes the shim ISA-independent instead of accidentally-working on arm.
 */
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "av1_structs.h"
#include "segmentation_params.h"
#include "deblocking_common.h"
#include "common_dsp_rtcd.h"
#include "aom_dsp_rtcd.h"
#include "pic_operators.h"
#include "pic_buffer_desc.h"
#include "intra_prediction.h"

void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);
void       svt_aom_init_intra_dc_predictors_c_internal(void);
void       svt_aom_init_intra_predictors_internal(void);

static int g_picops_ready = 0;
static void picops_ensure_init(void) {
    if (!g_picops_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_init_intra_dc_predictors_c_internal();
        svt_aom_init_intra_predictors_internal();
        g_picops_ready = 1;
    }
}

/* Reports whether the dispatched-kernel slots this TU depends on are bound.
 * A tier-1 test asserts this is nonzero BEFORE trusting any result from the
 * dispatched entry points — the positive control WORKING-ON-THIS §5 demands
 * for a probe whose silence would look like success. */
int ref_picops_rtcd_ready(void) {
    picops_ensure_init();
#if defined(svt_full_distortion_kernel32_bits)
    /* Devirtualized to a direct call by the NEON header: no pointer to check,
     * but the spatial slot below is still a pointer on this ISA. */
    return svt_spatial_full_distortion_kernel != NULL && svt_aom_eb_pred[V_PRED][TX_8X8] != NULL;
#else
    return svt_full_distortion_kernel32_bits != NULL && svt_full_distortion_kernel_cbf_zero32_bits != NULL &&
        svt_spatial_full_distortion_kernel != NULL && svt_aom_eb_pred[V_PRED][TX_8X8] != NULL;
#endif
}

/* ---------------- pic_operators.c: residual + distortion ---------------- */

void ref_residual_kernel8bit(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                             int16_t* residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height) {
    svt_residual_kernel8bit_c(
        input, input_stride, pred, pred_stride, residual, residual_stride, area_width, area_height);
}

void ref_residual_kernel16bit(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                              int16_t* residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height) {
    svt_residual_kernel16bit_c(
        input, input_stride, pred, pred_stride, residual, residual_stride, area_width, area_height);
}

void ref_full_distortion_kernel32_bits(int32_t* coeff, int32_t* recon_coeff, uint32_t stride, uint32_t area_width,
                                       uint32_t area_height, uint64_t* out2) {
    svt_full_distortion_kernel32_bits_c(coeff, recon_coeff, stride, area_width, area_height, out2);
}

void ref_full_distortion_kernel_cbf_zero32_bits(int32_t* coeff, uint32_t coeff_stride, uint32_t area_width,
                                                uint32_t area_height, uint64_t* out2) {
    svt_full_distortion_kernel_cbf_zero32_bits_c(coeff, coeff_stride, out2, area_width, area_height);
}

/* Dispatched: goes through the RTCD kernels, not the _c spellings. */
void ref_picture_full_distortion32_bits_single(int32_t* coeff, int32_t* recon_coeff, uint32_t stride, uint32_t bwidth,
                                               uint32_t bheight, uint32_t cnt_nz_coeff, uint64_t* out2) {
    picops_ensure_init();
    svt_aom_picture_full_distortion32_bits_single(coeff, recon_coeff, stride, bwidth, bheight, out2, cnt_nz_coeff);
}

uint64_t ref_spatial_full_distortion_kernel_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                              uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                              uint32_t area_width, uint32_t area_height) {
    return svt_spatial_full_distortion_kernel_c(
        input, input_offset, input_stride, recon, recon_offset, recon_stride, area_width, area_height);
}

/* The tx-bias facade. BlockModeInfo is built here from flat scalars so the
 * Rust side never mirrors its ABI; every field the facade reads is set. */
uint64_t ref_spatial_full_distortion_kernel_facade(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                   uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                   uint32_t area_width, uint32_t area_height, int32_t mode,
                                                   int32_t uv_mode, uint8_t is_interintra_used, int32_t compound_type,
                                                   int32_t is_chroma, uint8_t temporal_layer_index, double ac_bias,
                                                   uint8_t tx_bias) {
    picops_ensure_init();
    BlockModeInfo mi;
    memset(&mi, 0, sizeof(mi));
    mi.mode                 = (PredictionMode)mode;
    mi.uv_mode              = (UvPredictionMode)uv_mode;
    mi.is_interintra_used   = is_interintra_used;
    mi.interinter_comp.type = (CompoundType)compound_type;
    return svt_spatial_full_distortion_kernel_facade(input,
                                                     input_offset,
                                                     input_stride,
                                                     recon,
                                                     recon_offset,
                                                     recon_stride,
                                                     area_width,
                                                     area_height,
                                                     false,
                                                     &mi,
                                                     is_chroma != 0,
                                                     temporal_layer_index,
                                                     ac_bias,
                                                     tx_bias);
}

/* The 32-bit twin of the facade above: same bias layer, applied to both
 * DIST_CALC_RESIDUAL and DIST_CALC_PREDICTION. Dispatched (it calls
 * svt_aom_picture_full_distortion32_bits_single underneath). */
void ref_picture_full_distortion32_bits_single_facade(int32_t* coeff, int32_t* recon_coeff, uint32_t stride,
                                                      uint32_t bwidth, uint32_t bheight, uint32_t area_width,
                                                      uint32_t area_height, uint32_t cnt_nz_coeff, int32_t mode,
                                                      int32_t uv_mode, uint8_t is_interintra_used,
                                                      int32_t compound_type, int32_t is_chroma,
                                                      uint8_t temporal_layer_index, double ac_bias, uint8_t tx_bias,
                                                      uint64_t* out2) {
    picops_ensure_init();
    BlockModeInfo mi;
    memset(&mi, 0, sizeof(mi));
    mi.mode                 = (PredictionMode)mode;
    mi.uv_mode              = (UvPredictionMode)uv_mode;
    mi.is_interintra_used   = is_interintra_used;
    mi.interinter_comp.type = (CompoundType)compound_type;
    svt_aom_picture_full_distortion32_bits_single_facade(coeff,
                                                        recon_coeff,
                                                        stride,
                                                        bwidth,
                                                        bheight,
                                                        area_width,
                                                        area_height,
                                                        out2,
                                                        cnt_nz_coeff,
                                                        &mi,
                                                        is_chroma != 0,
                                                        temporal_layer_index,
                                                        ac_bias,
                                                        tx_bias);
}

/* ------------- pic_operators.c: padding / plane copy / widen ------------- */

void svt_aom_generate_padding16_bit(uint16_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                                    uint32_t original_src_height, uint32_t padding_width, uint32_t padding_height);
void svt_aom_pad_input_picture_16bit(uint16_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                                     uint32_t original_src_height, uint32_t pad_right, uint32_t pad_bottom);
void svt_convert_8bit_to_16bit_c(uint8_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride, uint32_t width,
                                 uint32_t height);

void ref_generate_padding16_bit(uint16_t* buf, uint32_t origin, uint32_t src_stride, uint32_t original_src_width,
                                uint32_t original_src_height, uint32_t padding_width, uint32_t padding_height) {
    picops_ensure_init();
    svt_aom_generate_padding16_bit(
        buf + origin, src_stride, original_src_width, original_src_height, padding_width, padding_height);
}

void ref_pad_input_picture_16bit(uint16_t* src, uint32_t src_stride, uint32_t original_src_width,
                                 uint32_t original_src_height, uint32_t pad_right, uint32_t pad_bottom) {
    picops_ensure_init();
    svt_aom_pad_input_picture_16bit(
        src, src_stride, original_src_width, original_src_height, pad_right, pad_bottom);
}

void ref_convert_8bit_to_16bit(uint8_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride, uint32_t width,
                               uint32_t height) {
    picops_ensure_init();
    svt_convert_8bit_to_16bit_c(src, src_stride, dst, dst_stride, width, height);
}

/* svt_aom_yv12_copy_{y,u,v}_c over a Yv12BufferConfig the shim builds. The
 * three C functions differ only in which field pair they read, so the shim
 * exposes one entry point per bit depth with a plane selector and the Rust
 * side compares all three against one generic port function.
 *
 * WORKING-ON-THIS §5 trap 4 again, in its nastiest form: on the highbd arm
 * C reads the plane through CONVERT_TO_SHORTPTR, which is aom's POINTER
 * TAGGING — `((uint16_t*)(((uintptr_t)(x)) << 1))` (definitions.h:1019). A
 * Yv12BufferConfig therefore stores a 16-bit plane as `ptr >> 1`, NOT as
 * the pointer itself. Handing it a plain cast `(uint8_t*)u16_ptr` would
 * make C read from `ptr << 1` — a wild address, and one that would look
 * like a plausible garbage mismatch rather than a crash on a machine where
 * it happens to be mapped. The shim applies CONVERT_TO_BYTEPTR, which is
 * what the encoder's own buffer setup does. */
static void picops_fill_yv12(Yv12BufferConfig* c, int32_t plane, uint8_t* buf, int32_t stride, int32_t width,
                             int32_t height) {
    if (plane == 0) {
        c->y_buffer = buf;
        c->y_stride = stride;
        c->y_width  = width;
        c->y_height = height;
    } else {
        c->uv_stride = stride;
        c->uv_width  = width;
        c->uv_height = height;
        if (plane == 1) {
            c->u_buffer = buf;
        } else {
            c->v_buffer = buf;
        }
    }
}

static void picops_yv12_dispatch(int32_t plane, Yv12BufferConfig* s, Yv12BufferConfig* d) {
    if (plane == 0) {
        svt_aom_yv12_copy_y_c(s, d);
    } else if (plane == 1) {
        svt_aom_yv12_copy_u_c(s, d);
    } else {
        svt_aom_yv12_copy_v_c(s, d);
    }
}

void ref_yv12_copy_plane8(int32_t plane, uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                          int32_t width, int32_t height) {
    picops_ensure_init();
    Yv12BufferConfig s, d;
    memset(&s, 0, sizeof(s));
    memset(&d, 0, sizeof(d));
    picops_fill_yv12(&s, plane, src, src_stride, width, height);
    picops_fill_yv12(&d, plane, dst, dst_stride, width, height);
    picops_yv12_dispatch(plane, &s, &d);
}

void ref_yv12_copy_plane16(int32_t plane, uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                           int32_t width, int32_t height) {
    picops_ensure_init();
    Yv12BufferConfig s, d;
    memset(&s, 0, sizeof(s));
    memset(&d, 0, sizeof(d));
    s.flags = YV12_FLAG_HIGHBITDEPTH;
    d.flags = YV12_FLAG_HIGHBITDEPTH;
    picops_fill_yv12(&s, plane, CONVERT_TO_BYTEPTR(src), src_stride, width, height);
    picops_fill_yv12(&d, plane, CONVERT_TO_BYTEPTR(dst), dst_stride, width, height);
    picops_yv12_dispatch(plane, &s, &d);
}

/* -------------------- deblocking_common.c: LF levels -------------------- */

/* svt_aom_update_sharpness fills lfi->lfthr[lvl].{lim,mblim} for every
 * level; the port states the same arithmetic per level, so the shim reports
 * the pair for one level. */
void ref_update_sharpness(int32_t sharpness_lvl, int32_t lvl, uint8_t* out_lim, uint8_t* out_mblim) {
    LoopFilterInfoN lfi;
    memset(&lfi, 0, sizeof(lfi));
    svt_aom_update_sharpness(&lfi, sharpness_lvl);
    *out_lim   = lfi.lfthr[lvl].lim[0];
    *out_mblim = lfi.lfthr[lvl].mblim[0];
}

/* Fill a FrameHeader's loop-filter / segmentation / delta-lf state from flat
 * scalars. `seg_data` is MAX_SEGMENTS * SEG_LVL_MAX ints in row-major order,
 * `seg_enabled` the same shape as bytes. */
static void ref_fill_frame_header(FrameHeader* fh, const int32_t* filt_lvl4, int32_t sharpness,
                                  uint8_t mode_ref_delta_enabled, const int8_t* ref_deltas, const int8_t* mode_deltas,
                                  uint8_t segmentation_enabled, const uint8_t* seg_enabled, const int32_t* seg_data,
                                  uint8_t delta_lf_present, uint8_t delta_lf_multi) {
    memset(fh, 0, sizeof(*fh));
    fh->loop_filter_params.filter_level[0]        = filt_lvl4[0];
    fh->loop_filter_params.filter_level[1]        = filt_lvl4[1];
    fh->loop_filter_params.filter_level_u         = filt_lvl4[2];
    fh->loop_filter_params.filter_level_v         = filt_lvl4[3];
    fh->loop_filter_params.sharpness_level        = sharpness;
    fh->loop_filter_params.mode_ref_delta_enabled = mode_ref_delta_enabled;
    for (int i = 0; i < REF_FRAMES; ++i) fh->loop_filter_params.ref_deltas[i] = ref_deltas[i];
    for (int i = 0; i < MAX_MODE_LF_DELTAS; ++i) fh->loop_filter_params.mode_deltas[i] = mode_deltas[i];
    fh->segmentation_params.segmentation_enabled = segmentation_enabled;
    for (int s = 0; s < MAX_SEGMENTS; ++s) {
        for (int f = 0; f < SEG_LVL_MAX; ++f) {
            fh->segmentation_params.feature_enabled[s][f] = seg_enabled[s * SEG_LVL_MAX + f];
            fh->segmentation_params.feature_data[s][f]    = (int16_t)seg_data[s * SEG_LVL_MAX + f];
        }
    }
    fh->delta_lf_params.delta_lf_present = delta_lf_present;
    fh->delta_lf_params.delta_lf_multi   = delta_lf_multi;
}

uint8_t ref_get_filter_level_delta_lf(const int32_t* filt_lvl4, int32_t sharpness, uint8_t mode_ref_delta_enabled,
                                      const int8_t* ref_deltas, const int8_t* mode_deltas, uint8_t segmentation_enabled,
                                      const uint8_t* seg_enabled, const int32_t* seg_data, uint8_t delta_lf_multi,
                                      int32_t dir_idx, int32_t plane, int32_t* sb_delta_lf, uint8_t seg_id,
                                      int32_t pred_mode, int32_t ref_frame_0) {
    FrameHeader fh;
    ref_fill_frame_header(&fh,
                          filt_lvl4,
                          sharpness,
                          mode_ref_delta_enabled,
                          ref_deltas,
                          mode_deltas,
                          segmentation_enabled,
                          seg_enabled,
                          seg_data,
                          /*delta_lf_present=*/1,
                          delta_lf_multi);
    return svt_aom_get_filter_level_delta_lf(
        &fh, dir_idx, plane, sb_delta_lf, seg_id, (PredictionMode)pred_mode, (MvReferenceFrame)ref_frame_0);
}

/* svt_av1_loop_filter_frame_init over planes [plane_start, plane_end), with
 * the resulting lvl table flattened into
 * out[plane][seg][dir][ref][mode] laid out row-major
 * (MAX_PLANES * MAX_SEGMENTS * 2 * REF_FRAMES * MAX_MODE_LF_DELTAS bytes).
 * The buffer is PRESET to 0xFF by the caller so untouched cells are
 * distinguishable from a written zero. */
void ref_loop_filter_frame_init(const int32_t* filt_lvl4, int32_t sharpness, uint8_t mode_ref_delta_enabled,
                                const int8_t* ref_deltas, const int8_t* mode_deltas, uint8_t segmentation_enabled,
                                const uint8_t* seg_enabled, const int32_t* seg_data, int32_t plane_start,
                                int32_t plane_end, uint8_t* out_lvl) {
    FrameHeader fh;
    ref_fill_frame_header(&fh,
                          filt_lvl4,
                          sharpness,
                          mode_ref_delta_enabled,
                          ref_deltas,
                          mode_deltas,
                          segmentation_enabled,
                          seg_enabled,
                          seg_data,
                          /*delta_lf_present=*/0,
                          /*delta_lf_multi=*/0);
    LoopFilterInfoN lfi;
    memset(&lfi, 0, sizeof(lfi));
    memcpy(&lfi.lvl[0][0][0][0][0], out_lvl, sizeof(lfi.lvl));
    svt_av1_loop_filter_frame_init(&fh, &lfi, plane_start, plane_end);
    memcpy(out_lvl, &lfi.lvl[0][0][0][0][0], sizeof(lfi.lvl));
}

/* --------------------- intra_prediction.c residuals --------------------- */

int ref_intra_is_smooth(int32_t mode, int32_t uv_mode, int32_t plane) {
    BlockModeInfo mi;
    memset(&mi, 0, sizeof(mi));
    mi.mode         = (PredictionMode)mode;
    mi.uv_mode      = (UvPredictionMode)uv_mode;
    mi.ref_frame[0] = INTRA_FRAME;
    return svt_aom_is_smooth(&mi, plane);
}

int ref_intra_is_smooth_inter(int32_t mode, int32_t uv_mode, int32_t plane, int32_t ref_frame_0) {
    BlockModeInfo mi;
    memset(&mi, 0, sizeof(mi));
    mi.mode         = (PredictionMode)mode;
    mi.uv_mode      = (UvPredictionMode)uv_mode;
    mi.ref_frame[0] = (MvReferenceFrame)ref_frame_0;
    return svt_aom_is_smooth(&mi, plane);
}

/* Both entry points below stage their pixel buffers in ALIGNED locals and
 * copy the result out. WORKING-ON-THIS §5 trap 4: the shim must hand C the
 * contract the ENCODER hands it, and these reach svt_aom_eb_pred[][] /
 * svt_aom_dc_pred[][][], whose x86 members are AVX2/AVX-512 kernels that
 * store with vector instructions. A 1-byte-aligned Rust Vec would be a
 * different contract from the encoder's own 64-aligned picture buffers, and
 * the difference is invisible on aarch64. `above_data`/`left_data` mirror
 * C's own layout (`above_row = above_data + 16`) so the [-1] corner sample
 * every zone-2 predictor reads is valid. */
#define PICOPS_EDGE_ORIGIN 16
#define PICOPS_EDGE_LEN 160
#define PICOPS_MAX_DIM 64

void ref_dr_predictor(uint8_t* dst, int32_t stride, int32_t tx_size, const uint8_t* above_data,
                      const uint8_t* left_data, int32_t upsample_above, int32_t upsample_left, int32_t angle,
                      int32_t bw, int32_t bh) {
    picops_ensure_init();
    DECLARE_ALIGNED(64, uint8_t, a[PICOPS_EDGE_LEN]);
    DECLARE_ALIGNED(64, uint8_t, l[PICOPS_EDGE_LEN]);
    DECLARE_ALIGNED(64, uint8_t, d[PICOPS_MAX_DIM * PICOPS_MAX_DIM]);
    memcpy(a, above_data, PICOPS_EDGE_LEN);
    memcpy(l, left_data, PICOPS_EDGE_LEN);
    memset(d, 0, sizeof(d));
    svt_aom_dr_predictor(d,
                         (ptrdiff_t)PICOPS_MAX_DIM,
                         (TxSize)tx_size,
                         a + PICOPS_EDGE_ORIGIN,
                         l + PICOPS_EDGE_ORIGIN,
                         upsample_above,
                         upsample_left,
                         angle);
    for (int32_t r = 0; r < bh; ++r) memcpy(dst + (size_t)r * stride, d + (size_t)r * PICOPS_MAX_DIM, (size_t)bw);
}

int32_t ref_intra_prediction_open_loop_mb(int32_t p_angle, uint8_t ois_intra_mode, uint32_t src_origin_x,
                                          uint32_t src_origin_y, int32_t tx_size, const uint8_t* above_data,
                                          const uint8_t* left_data, uint8_t* dst, int32_t stride, int32_t bw,
                                          int32_t bh) {
    picops_ensure_init();
    DECLARE_ALIGNED(64, uint8_t, a[PICOPS_EDGE_LEN]);
    DECLARE_ALIGNED(64, uint8_t, l[PICOPS_EDGE_LEN]);
    DECLARE_ALIGNED(64, uint8_t, d[PICOPS_MAX_DIM * PICOPS_MAX_DIM]);
    memcpy(a, above_data, PICOPS_EDGE_LEN);
    memcpy(l, left_data, PICOPS_EDGE_LEN);
    memset(d, 0, sizeof(d));
    EbErrorType e = svt_aom_intra_prediction_open_loop_mb(p_angle,
                                                          ois_intra_mode,
                                                          src_origin_x,
                                                          src_origin_y,
                                                          (TxSize)tx_size,
                                                          a + PICOPS_EDGE_ORIGIN,
                                                          l + PICOPS_EDGE_ORIGIN,
                                                          d,
                                                          PICOPS_MAX_DIM);
    for (int32_t r = 0; r < bh; ++r) memcpy(dst + (size_t)r * stride, d + (size_t)r * PICOPS_MAX_DIM, (size_t)bw);
    return (int32_t)e;
}
