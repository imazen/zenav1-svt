/*
 * Differential oracles for `svtav1_encoder::inter_me` — the wholesale port of
 * Source/Lib/Codec/motion_estimation.c.
 *
 * Ten C entry points are EXPORTED and are called DIRECTLY here (no
 * transcription): the five ext-SAD accumulators, the two compute_sad_c loop
 * kernels, svt_aom_get_scaled_picture_distance, hme_level_2 and
 * check_00_center. The last two take a MeContext* / EbPictureBufferDesc*, so
 * the shim allocates real ones and fills only the fields those functions read
 * — the same facade-over-the-real-function pattern the IntraBC shims use.
 *
 * The `_c` spellings are deliberate where the port is a transcription of the
 * `_c` kernel: comparing against the RTCD pointer would compare against
 * whatever SIMD tier this host dispatches to, which is a different oracle.
 * `ref_me_sad_loop_kernel_rtcd` exists SO THAT a test can ask whether the
 * host's SIMD tier agrees with `_c` — see the tie-break note in the Rust test.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "me_context.h"
#include "pic_buffer_desc.h"
#include "aom_dsp_rtcd.h"

void svt_aom_setup_common_rtcd_internal(uint64_t flags);
void svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);

static int me_rtcd_done = 0;
static void me_ensure_rtcd(void) {
    if (!me_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        me_rtcd_done = 1;
    }
}

/* ---- motion_estimation.c leaf kernels (all EXPORTED `_c` symbols) ---- */

uint32_t svt_aom_compute8x4_sad_kernel_c(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride);
void     svt_ext_sad_calculation_8x8_16x16_c(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                             uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
                                             uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16, uint32_t mv,
                                             uint32_t* p_sad16x16, uint32_t* p_sad8x8, bool sub_sad);
void     svt_ext_sad_calculation_32x32_64x64_c(uint32_t* p_sad16x16, uint32_t* p_best_sad_32x32,
                                               uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                               uint32_t* p_best_mv64x64, uint32_t mv, uint32_t* p_sad32x32);
void     svt_ext_all_sad_calculation_8x8_16x16_c(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                 uint32_t mv, uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
                                                 uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
                                                 uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8],
                                                 bool sub_sad);
void     svt_ext_eight_sad_calculation_32x32_64x64_c(const uint32_t p_sad16x16[16][8], uint32_t* p_best_sad_32x32,
                                                     uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                                     uint32_t* p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);
uint32_t svt_nxm_sad_kernel_helper_c(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                     uint32_t height, uint32_t width);
void     svt_sad_loop_kernel_c(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                               uint32_t block_height, uint32_t block_width, uint64_t* best_sad,
                               int16_t* x_search_center, int16_t* y_search_center, uint32_t src_stride_raw,
                               uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);
uint16_t svt_aom_get_scaled_picture_distance(uint16_t dist);

void hme_level_2(MeContext* me_ctx, int16_t org_x, int16_t org_y, uint32_t block_width, uint32_t block_height,
                 EbPictureBufferDesc* ref_pic_ptr, int16_t sa_width, int16_t sa_height, int16_t hme_l1_sc_x,
                 int16_t hme_l1_sc_y, uint64_t* best_sad, int16_t* hme_l2_sc_x, int16_t* hme_l2_sc_y);
uint32_t check_00_center(EbPictureBufferDesc* ref_pic_ptr, MeContext* me_ctx, uint32_t sb_origin_x,
                         uint32_t sb_origin_y, uint32_t sb_width, uint32_t sb_height, int16_t* x_search_center,
                         int16_t* y_search_center, uint32_t zz_sad);

uint32_t ref_me_compute8x4_sad(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride) {
    return svt_aom_compute8x4_sad_kernel_c((uint8_t*)src, src_stride, (uint8_t*)ref, ref_stride);
}

/* best_sad / best_mv are the whole 85-entry p_sb_best_* rows; off8 / off16
 * are the offsets the caller aims the two C pointers at. */
void ref_me_ext_sad_8x8_16x16(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                              uint32_t* best_sad, uint32_t* best_mv, uint32_t off8, uint32_t off16, uint32_t mv,
                              uint32_t* p_sad16x16, uint32_t i16, uint32_t* p_sad8x8, uint32_t i8, int sub_sad) {
    svt_ext_sad_calculation_8x8_16x16_c((uint8_t*)src,
                                        src_stride,
                                        (uint8_t*)ref,
                                        ref_stride,
                                        best_sad + off8,
                                        best_sad + off16,
                                        best_mv + off8,
                                        best_mv + off16,
                                        mv,
                                        p_sad16x16 + i16,
                                        p_sad8x8 + i8,
                                        sub_sad ? true : false);
}

void ref_me_ext_sad_32x32_64x64(const uint32_t* p_sad16x16, uint32_t* best_sad, uint32_t* best_mv, uint32_t off32,
                                uint32_t off64, uint32_t mv, uint32_t* p_sad32x32) {
    svt_ext_sad_calculation_32x32_64x64_c(
        (uint32_t*)p_sad16x16, best_sad + off32, best_sad + off64, best_mv + off32, best_mv + off64, mv, p_sad32x32);
}

void ref_me_ext_all_sad_8x8_16x16(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                  uint32_t mv, uint32_t* best_sad, uint32_t* best_mv, uint32_t off8, uint32_t off16,
                                  uint32_t* p_eight_sad16x16, int sub_sad) {
    /* C also takes a p_eight_sad8x8[64][8] it never writes (the kernel does
     * `(void)p_eight_sad8x8`); a scratch block keeps the ABI honest. */
    static uint32_t scratch8x8[64][8];
    svt_ext_all_sad_calculation_8x8_16x16_c((uint8_t*)src,
                                            src_stride,
                                            (uint8_t*)ref,
                                            ref_stride,
                                            mv,
                                            best_sad + off8,
                                            best_sad + off16,
                                            best_mv + off8,
                                            best_mv + off16,
                                            (uint32_t(*)[8])p_eight_sad16x16,
                                            scratch8x8,
                                            sub_sad ? true : false);
}

void ref_me_ext_eight_sad_32x32_64x64(const uint32_t* p_sad16x16, uint32_t* best_sad, uint32_t* best_mv,
                                      uint32_t off32, uint32_t off64, uint32_t mv, uint32_t* p_sad32x32) {
    svt_ext_eight_sad_calculation_32x32_64x64_c((const uint32_t(*)[8])p_sad16x16,
                                                best_sad + off32,
                                                best_sad + off64,
                                                best_mv + off32,
                                                best_mv + off64,
                                                mv,
                                                (uint32_t(*)[8])p_sad32x32);
}

uint32_t ref_me_nxm_sad(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                        uint32_t height, uint32_t width) {
    return svt_nxm_sad_kernel_helper_c(src, src_stride, ref, ref_stride, height, width);
}

void ref_me_sad_loop_kernel(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                            uint32_t block_height, uint32_t block_width, uint64_t* best_sad, int16_t* x_sc,
                            int16_t* y_sc, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t sa_w,
                            int16_t sa_h) {
    svt_sad_loop_kernel_c((uint8_t*)src,
                          src_stride,
                          (uint8_t*)ref,
                          ref_stride,
                          block_height,
                          block_width,
                          best_sad,
                          x_sc,
                          y_sc,
                          src_stride_raw,
                          skip_search_line,
                          sa_w,
                          sa_h);
}

/* The RTCD-dispatched variant — the kernel the real encoder actually runs on
 * this host. A test can assert it agrees with `_c` (including the tie-break
 * that decides WHICH minimum is reported). */
void ref_me_sad_loop_kernel_rtcd(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                 uint32_t block_height, uint32_t block_width, uint64_t* best_sad, int16_t* x_sc,
                                 int16_t* y_sc, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t sa_w,
                                 int16_t sa_h) {
    me_ensure_rtcd();
    svt_sad_loop_kernel((uint8_t*)src,
                        src_stride,
                        (uint8_t*)ref,
                        ref_stride,
                        block_height,
                        block_width,
                        best_sad,
                        x_sc,
                        y_sc,
                        src_stride_raw,
                        skip_search_line,
                        sa_w,
                        sa_h);
}

uint16_t ref_me_get_scaled_picture_distance(uint16_t dist) { return svt_aom_get_scaled_picture_distance(dist); }

/* ---- MeContext-taking entry points ----
 * `ref_alloc` is the whole padded allocation; `ref_org` is the index of pixel
 * (0,0) inside it, i.e. C's `y_buffer - buffer_y`. */

static EbPictureBufferDesc* me_make_pic(const uint8_t* ref_alloc, uint32_t ref_org, uint16_t stride, uint16_t w,
                                        uint16_t h, uint16_t border) {
    EbPictureBufferDesc* p = (EbPictureBufferDesc*)calloc(1, sizeof(*p));
    p->y_buffer            = (uint8_t*)ref_alloc + ref_org;
    p->y_stride            = stride;
    p->width               = w;
    p->height              = h;
    p->border              = border;
    return p;
}

void ref_me_hme_level_2(const uint8_t* b64_src, uint32_t b64_src_stride, uint8_t hme_search_method,
                        const uint8_t* ref_alloc, uint32_t ref_org, uint16_t ref_stride, uint16_t ref_w,
                        uint16_t ref_h, int16_t org_x, int16_t org_y, uint32_t block_width, uint32_t block_height,
                        int16_t sa_width, int16_t sa_height, int16_t l1x, int16_t l1y, uint64_t* best_sad,
                        int16_t* sc_x, int16_t* sc_y) {
    me_ensure_rtcd();
    MeContext* ctx           = (MeContext*)calloc(1, sizeof(*ctx));
    ctx->hme_search_method   = hme_search_method;
    ctx->b64_src_ptr         = (uint8_t*)b64_src;
    ctx->b64_src_stride      = b64_src_stride;
    EbPictureBufferDesc* pic = me_make_pic(ref_alloc, ref_org, ref_stride, ref_w, ref_h, 0);
    hme_level_2(ctx, org_x, org_y, block_width, block_height, pic, sa_width, sa_height, l1x, l1y, best_sad, sc_x, sc_y);
    free(pic);
    free(ctx);
}

uint32_t ref_me_check_00_center(const uint8_t* b64_src, uint32_t b64_src_stride, uint32_t me_early_exit_th,
                                const uint8_t* ref_alloc, uint32_t ref_org, uint16_t ref_stride, uint16_t ref_w,
                                uint16_t ref_h, uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t sb_width,
                                uint32_t sb_height, int16_t* x_sc, int16_t* y_sc, uint32_t zz_sad) {
    me_ensure_rtcd();
    MeContext* ctx           = (MeContext*)calloc(1, sizeof(*ctx));
    ctx->b64_src_ptr         = (uint8_t*)b64_src;
    ctx->b64_src_stride      = b64_src_stride;
    ctx->me_early_exit_th    = me_early_exit_th;
    EbPictureBufferDesc* pic = me_make_pic(ref_alloc, ref_org, ref_stride, ref_w, ref_h, 0);
    uint32_t r = check_00_center(pic, ctx, sb_origin_x, sb_origin_y, sb_width, sb_height, x_sc, y_sc, zz_sad);
    free(pic);
    free(ctx);
    return r;
}
