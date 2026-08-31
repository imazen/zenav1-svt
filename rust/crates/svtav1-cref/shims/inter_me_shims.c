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

/* ==========================================================================
 * av1me.c — the OBMC search, and the four C_DEFAULT kernels it drives.
 *
 * svt_av1_obmc_full_pixel_search and svt_av1_find_best_obmc_sub_pixel_tree_up
 * are EXPORTED, so both are called directly; the shim assembles the
 * IntraBcContext / ModeDecisionContext shells the way mode_decision.c:2100-2170
 * does. The kernels are compared against their `_c` spellings, not the RTCD
 * pointers: the port transcribes the `_c` bodies, and comparing against a SIMD
 * tier would be comparing against a different oracle.
 * ========================================================================== */

#include "md_process.h"

#define DECL_OBMC(W, H)                                                                            \
    unsigned int svt_aom_obmc_sad##W##x##H##_c(                                                    \
        const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask);             \
    unsigned int svt_aom_obmc_variance##W##x##H##_c(const uint8_t* pre,                            \
                                                    int            pre_stride,                     \
                                                    const int32_t* wsrc,                           \
                                                    const int32_t* mask,                           \
                                                    unsigned int*  sse);                           \
    unsigned int svt_aom_obmc_sub_pixel_variance##W##x##H##_c(const uint8_t* pre,                  \
                                                              int            pre_stride,           \
                                                              int            xoffset,              \
                                                              int            yoffset,              \
                                                              const int32_t* wsrc,                 \
                                                              const int32_t* mask,                 \
                                                              unsigned int*  sse);

DECL_OBMC(4, 4)
DECL_OBMC(4, 8)
DECL_OBMC(8, 4)
DECL_OBMC(8, 8)
DECL_OBMC(8, 16)
DECL_OBMC(16, 8)
DECL_OBMC(16, 16)
DECL_OBMC(16, 32)
DECL_OBMC(32, 16)
DECL_OBMC(32, 32)
#undef DECL_OBMC

void svt_aom_upsampled_pred_c(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col,
                              const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3,
                              int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search);
void svt_aom_convolve8_horiz_c(const uint8_t* src, ptrdiff_t src_stride, uint8_t* dst, ptrdiff_t dst_stride,
                               const int16_t* filter_x, int x_step_q4, const int16_t* filter_y, int y_step_q4, int w,
                               int h);
void svt_aom_convolve8_vert_c(const uint8_t* src, ptrdiff_t src_stride, uint8_t* dst, ptrdiff_t dst_stride,
                              const int16_t* filter_x, int x_step_q4, const int16_t* filter_y, int y_step_q4, int w,
                              int h);
int svt_av1_obmc_full_pixel_search(ModeDecisionContext* ctx, IntraBcContext* x, const Mv* mvp_full, int sadpb,
                                   const AomVarianceFnPtr* fn_ptr, const Mv* ref_mv, Mv* dst_mv, int is_second);
int svt_av1_find_best_obmc_sub_pixel_tree_up(ModeDecisionContext* ctx, IntraBcContext* x,
                                             const struct Av1Common* const cm, int mi_row, int mi_col, Mv* bestmv,
                                             const Mv* ref_mv, int allow_hp, int error_per_bit,
                                             const AomVarianceFnPtr* vfp, int forced_stop, int iters_per_step,
                                             int* mvjcost, const int* mvcost[2], int* distortion, unsigned int* sse1,
                                             int is_second, int use_accurate_subpel_search);
void init_fn_ptr(void);
extern AomVarianceFnPtr svt_aom_mefn_ptr[BLOCK_SIZES_ALL];

static int obmc_rtcd_done = 0;
static void obmc_ensure_init(void) {
    if (!obmc_rtcd_done) {
        me_ensure_rtcd();
        init_fn_ptr();
        obmc_rtcd_done = 1;
    }
}

#define OBMC_CASE(W, H)                                                                  \
    if (width == W && height == H) {                                                     \
        switch (which) {                                                                 \
        case 0: return svt_aom_obmc_sad##W##x##H##_c(pre, pre_stride, wsrc, mask);       \
        case 1: return svt_aom_obmc_variance##W##x##H##_c(pre, pre_stride, wsrc, mask, sse); \
        default:                                                                         \
            return svt_aom_obmc_sub_pixel_variance##W##x##H##_c(                         \
                pre, pre_stride, xoffset, yoffset, wsrc, mask, sse);                     \
        }                                                                                \
    }

/* which: 0 = obmc_sad, 1 = obmc_variance, 2 = obmc_sub_pixel_variance.
 * Returns UINT32_MAX with *sse untouched for an unsupported size, so the Rust
 * side fails loudly instead of silently comparing nothing. */
unsigned int ref_obmc_kernel(int which, int width, int height, const uint8_t* pre, int pre_stride, int xoffset,
                             int yoffset, const int32_t* wsrc, const int32_t* mask, unsigned int* sse) {
    OBMC_CASE(4, 4)
    OBMC_CASE(4, 8)
    OBMC_CASE(8, 4)
    OBMC_CASE(8, 8)
    OBMC_CASE(8, 16)
    OBMC_CASE(16, 8)
    OBMC_CASE(16, 16)
    OBMC_CASE(16, 32)
    OBMC_CASE(32, 16)
    OBMC_CASE(32, 32)
    return 0xFFFFFFFFu;
}
#undef OBMC_CASE

/* `obmc_ensure_init` is MANDATORY here, not defensive: on its
 * `!subpel_x_q3 && !subpel_y_q3` arm `svt_aom_upsampled_pred_c` (variance.c:92)
 * calls bare `svt_memcpy`, which is an RTCD FUNCTION POINTER living in .bss
 * (common_dsp_rtcd.h:1083) and is NULL until `svt_aom_setup_common_rtcd_internal`
 * runs. The header offers a null-safe `SVT_MEMCPY` for exactly this, and that
 * call site does not use it. On aarch64 the hazard is invisible: NEON
 * devirtualization rewrites `svt_memcpy` to the concrete `svt_memcpy_neon`
 * (common_dsp_rtcd_neon_devirt.h:266), so the pointer never exists. On x86-64
 * an uninitialized call lands at rip=0x0. MEASURED 2026-08-31: without this
 * line `upsampled_pred_matches_c` SIGSEGVs on x86_64-linux at its very first
 * cell (4x4, USE_2_TAPS, offset (0,0)) and passes on aarch64-darwin. */
void ref_upsampled_pred(uint8_t* comp_pred, int width, int height, int subpel_x_q3, int subpel_y_q3,
                        const uint8_t* ref_alloc, int ref_base, int ref_stride, int subpel_search) {
    obmc_ensure_init();
    Mv mv = {{0, 0}};
    svt_aom_upsampled_pred_c(NULL, NULL, 0, 0, &mv, comp_pred, width, height, subpel_x_q3, subpel_y_q3,
                             ref_alloc + ref_base, ref_stride, subpel_search);
}

/* `svt_aom_convolve8_{horiz,vert}_c` do NOT take a phase index: they recover
 * the 16-phase InterpKernel table and the phase from the ADDRESS of the filter
 * pointer -- `get_filter_base` masks it with `~0xFF` and `get_filter_offset`
 * subtracts (convolve.c:54-61, "NOTE: This assumes that the filter table is
 * 256-byte aligned"). Every real call site satisfies that with
 * `DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8[16])`
 * (inter_prediction.c:238). Forwarding the caller's raw pointer instead makes
 * the applied taps a function of where the linker happened to put the caller's
 * table: with `x_step_q4 == 16` the source stepping is unaffected but the taps
 * become those at `kernel_addr - (kernel_addr % 16)`, i.e. the CALLER'S TAPS
 * only when the caller's array is 16-byte aligned. MEASURED 2026-08-31: the
 * Rust `SUB_PEL_FILTERS_8` static landed at `%16 == 0` in the aarch64-darwin
 * test binary (oracle correct, by luck) and at `%16 == 8` in the x86_64-linux
 * one (oracle silently applied the wrong 8 taps).
 *
 * So stage the caller's taps into a 256-byte-aligned table, replicated into
 * every phase row: whichever offset the kernel derives, the taps it applies
 * are the caller's. Same treatment as `ref_convolve8_horiz` in ref_shims.c. */
void ref_me_convolve8_horiz(const uint8_t* src_alloc, int src_base, int src_stride, uint8_t* dst, int dst_stride,
                         const int16_t* kernel, int w, int h) {
    _Alignas(256) int16_t table[16][8];
    for (int p = 0; p < 16; ++p) memcpy(table[p], kernel, 8 * sizeof(int16_t));
    svt_aom_convolve8_horiz_c(src_alloc + src_base, src_stride, dst, dst_stride, table[0], 16, NULL, -1, w, h);
}

void ref_me_convolve8_vert(const uint8_t* src_alloc, int src_base, int src_stride, uint8_t* dst, int dst_stride,
                        const int16_t* kernel, int w, int h) {
    _Alignas(256) int16_t table[16][8];
    for (int p = 0; p < 16; ++p) memcpy(table[p], kernel, 8 * sizeof(int16_t));
    svt_aom_convolve8_vert_c(src_alloc + src_base, src_stride, dst, dst_stride, NULL, -1, table[0], 16, w, h);
}

/* Shared IntraBcContext + ModeDecisionContext assembly for the two OBMC
 * search entry points (mode_decision.c:2100-2170). `pre_base` is the index of
 * the reference block's (0,0) inside `pre_alloc`. */
static void obmc_setup(IntraBcContext* x, ModeDecisionContext* mdc, BlockGeom* geom, MacroBlockD* xd,
                       EbPictureBufferDesc* cur_buf, const int32_t** stack, const uint8_t* pre_alloc, int pre_base,
                       int pre_stride, int32_t* wsrc, int32_t* mask, int bsize, int col_min, int col_max,
                       int row_min, int row_max, const int32_t* mv_joint, const int32_t* mv_cost0,
                       const int32_t* mv_cost1, int errorperbit, int approx_inter_rate, int fpel_range,
                       int fpel_diag) {
    memset(x, 0, sizeof(*x));
    memset(mdc, 0, sizeof(*mdc));
    memset(geom, 0, sizeof(*geom));
    memset(xd, 0, sizeof(*xd));
    memset(cur_buf, 0, sizeof(*cur_buf));

    x->xdplane[0].pre[0].buf    = (uint8_t*)pre_alloc + pre_base;
    x->xdplane[0].pre[0].stride = pre_stride;
    x->mv_limits.col_min        = col_min;
    x->mv_limits.col_max        = col_max;
    x->mv_limits.row_min        = row_min;
    x->mv_limits.row_max        = row_max;
    x->errorperbit              = errorperbit;
    x->approx_inter_rate        = (uint8_t)approx_inter_rate;
    stack[0]                    = mv_cost0 + MV_MAX;
    stack[1]                    = mv_cost1 + MV_MAX;
    x->nmv_vec_cost             = (int*)mv_joint;
    x->mv_cost_stack            = stack;
    /* CONFIG_AV1_HIGHBITDEPTH is NOT defined in this tree, so
     * upsampled_obmc_pref_error takes the plain 8-bit arm and never reads xd. */
    x->xd                       = xd;

    geom->bsize                    = (BlockSize)bsize;
    mdc->blk_geom                  = geom;
    mdc->wsrc_buf                  = wsrc;
    mdc->mask_buf                  = mask;
    mdc->approx_inter_rate         = (uint8_t)approx_inter_rate;
    mdc->obmc_ctrls.fpel_search_range = (uint8_t)fpel_range;
    mdc->obmc_ctrls.fpel_search_diag  = (uint8_t)fpel_diag;
}

int ref_obmc_full_pixel_search(const uint8_t* pre_alloc, int pre_base, int pre_stride, int32_t* wsrc, int32_t* mask,
                               int bsize, int mvp_x, int mvp_y, int sadpb, int ref_mv_x, int ref_mv_y, int col_min,
                               int col_max, int row_min, int row_max, const int32_t* mv_joint,
                               const int32_t* mv_cost0, const int32_t* mv_cost1, int errorperbit,
                               int approx_inter_rate, int fpel_range, int fpel_diag, int* out_x, int* out_y) {
    obmc_ensure_init();
    IntraBcContext       x;
    BlockGeom            geom;
    MacroBlockD          xd;
    EbPictureBufferDesc  cur_buf;
    const int32_t*       stack[2];
    ModeDecisionContext* mdc = (ModeDecisionContext*)malloc(sizeof(*mdc));
    obmc_setup(&x, mdc, &geom, &xd, &cur_buf, stack, pre_alloc, pre_base, pre_stride, wsrc, mask, bsize, col_min,
               col_max, row_min, row_max, mv_joint, mv_cost0, mv_cost1, errorperbit, approx_inter_rate, fpel_range,
               fpel_diag);
    Mv mvp = {{(int16_t)mvp_x, (int16_t)mvp_y}};
    Mv rmv = {{(int16_t)ref_mv_x, (int16_t)ref_mv_y}};
    Mv dst = {{0, 0}};
    int r  = svt_av1_obmc_full_pixel_search(mdc, &x, &mvp, sadpb, &svt_aom_mefn_ptr[bsize], &rmv, &dst, 0);
    *out_x = dst.x;
    *out_y = dst.y;
    free(mdc);
    return r;
}

unsigned int ref_obmc_sub_pixel_tree_up(const uint8_t* pre_alloc, int pre_base, int pre_stride, int32_t* wsrc,
                                        int32_t* mask, int bsize, int best_x, int best_y, int ref_mv_x,
                                        int ref_mv_y, int allow_hp, int errorperbit, int forced_stop,
                                        int iters_per_step, int col_min, int col_max, int row_min, int row_max,
                                        const int32_t* mv_joint, const int32_t* mv_cost0, const int32_t* mv_cost1,
                                        int approx_inter_rate, int use_accurate_subpel_search, int* out_x,
                                        int* out_y, int* out_distortion, unsigned int* out_sse) {
    obmc_ensure_init();
    IntraBcContext       x;
    BlockGeom            geom;
    MacroBlockD          xd;
    EbPictureBufferDesc  cur_buf;
    const int32_t*       stack[2];
    ModeDecisionContext* mdc = (ModeDecisionContext*)malloc(sizeof(*mdc));
    obmc_setup(&x, mdc, &geom, &xd, &cur_buf, stack, pre_alloc, pre_base, pre_stride, wsrc, mask, bsize, col_min,
               col_max, row_min, row_max, mv_joint, mv_cost0, mv_cost1, errorperbit, approx_inter_rate, 8, 1);
    Mv           best = {{(int16_t)best_x, (int16_t)best_y}};
    Mv           rmv  = {{(int16_t)ref_mv_x, (int16_t)ref_mv_y}};
    int          dis  = 0;
    unsigned int sse  = 0;
    unsigned int r    = (unsigned)svt_av1_find_best_obmc_sub_pixel_tree_up(mdc,
                                                                       &x,
                                                                       NULL,
                                                                       0,
                                                                       0,
                                                                       &best,
                                                                       &rmv,
                                                                       allow_hp,
                                                                       errorperbit,
                                                                       &svt_aom_mefn_ptr[bsize],
                                                                       forced_stop,
                                                                       iters_per_step,
                                                                       x.nmv_vec_cost,
                                                                       x.mv_cost_stack,
                                                                       &dis,
                                                                       &sse,
                                                                       0,
                                                                       use_accurate_subpel_search);
    *out_x          = best.x;
    *out_y          = best.y;
    *out_distortion = dis;
    *out_sse        = sse;
    free(mdc);
    return r;
}

/* The RTCD-dispatched OBMC kernels — what the real encoder runs on THIS host.
 * A test can assert they agree with the `_c` spellings the port transcribes;
 * when they do not, the port is still faithful to the C SOURCE and the
 * divergence belongs in docs/SUSPECTED-C-BUGS.md. `which` matches
 * ref_obmc_kernel: 0 = osdf, 1 = ovf, 2 = osvf. */
unsigned int ref_obmc_kernel_rtcd(int which, int bsize, const uint8_t* pre, int pre_stride, int xoffset, int yoffset,
                                  const int32_t* wsrc, const int32_t* mask, unsigned int* sse) {
    obmc_ensure_init();
    switch (which) {
    case 0: return svt_aom_mefn_ptr[bsize].osdf(pre, pre_stride, wsrc, mask);
    case 1: return svt_aom_mefn_ptr[bsize].ovf(pre, pre_stride, wsrc, mask, sse);
    default: return svt_aom_mefn_ptr[bsize].osvf(pre, pre_stride, xoffset, yoffset, wsrc, mask, sse);
    }
}
