/*
 * C shims for the inter-prediction functions the wholesale-MC lane left
 * unported (Source/Lib/C_DEFAULT/inter_prediction_c.c and the 10-bit arm of
 * Source/Lib/Codec/inter_prediction.c).
 *
 * Its own translation unit so the wx-interpred lane never shares a shim file
 * with the lanes that own inter_pred_shims.c / inter_me_shims.c.
 *
 * RULE, inherited from ref_shims.c: A SHIM MUST NOT KEEP PER-CALL STATE IN A
 * `static`. Every scratch below is a local or caller-owned.
 */
#include <stddef.h>
#include <stdint.h>
#include <pthread.h>
#include <string.h>

#include "convolve.h"
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "inter_prediction.h"

/* Several kernels reached from here are RTCD FUNCTION POINTERS living in .bss
 * and NULL until `svt_aom_setup_common_rtcd_internal` runs
 * (`svt_pack2d_16_bit_src_mul4`, common_dsp_rtcd.h:148;
 * `svt_av1_build_compound_diffwtd_mask_d16`, :100;
 * `svt_aom_convolveHbd[][][]`, reached by `svt_inter_predictor_light_pd1`).
 * On aarch64 `common_dsp_rtcd_neon_devirt.h` `#define`s each of those names to
 * a concrete NEON symbol so no pointer exists to be null; on x86-64 the call
 * lands at rip=0x0. So the init is MANDATORY even though a purely aarch64 run
 * would pass without it — the same shape as the wedge-table init in
 * inter_pred_shims.c. `pthread_once` because cargo runs a test binary's tests
 * on several threads. */
uint64_t              svt_aom_get_cpu_flags_to_use(void);
void                  svt_aom_setup_common_rtcd_internal(uint64_t flags);
void                  svt_aom_setup_rtcd_internal(uint64_t flags);
void                  svt_aom_asm_set_convolve_asm_table(void);
void                  svt_aom_asm_set_convolve_hbd_asm_table(void);
static pthread_once_t g_gap_rtcd_once = PTHREAD_ONCE_INIT;
static void           init_gap_rtcd(void) {
    svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    /* ONE `pthread_once` covers the RTCD pointers AND the two convolve
     * function-pointer TABLES, deliberately. Splitting them into two
     * `pthread_once`es would let a second thread fill a table from RTCD
     * pointers the first thread had not finished assigning -- the exact
     * NULL-over-working-entry race inter_pred_shims.c documents at its own
     * init. The table fills are idempotent, so running them for every caller
     * in this TU (not only the light-PD1 one) costs nothing. */
    svt_aom_asm_set_convolve_asm_table();
    svt_aom_asm_set_convolve_hbd_asm_table();
}
static void ensure_gap_rtcd(void) { pthread_once(&g_gap_rtcd_once, init_gap_rtcd); }

/* ---- svt_av1_build_compound_diffwtd_mask_d16 -------------------------- */

/* The SCALAR `_c` entry (an exported `T` symbol), which is the semantics every
 * SIMD variant must reproduce and therefore what the port is gated against. */
void ref_build_compound_diffwtd_mask_d16_c(uint8_t* mask, int mask_type, const uint16_t* src0, int src0_stride,
                                           const uint16_t* src1, int src1_stride, int h, int w, int bd) {
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, /*is_compound=*/1, bd);
    svt_av1_build_compound_diffwtd_mask_d16_c(
        mask, (DIFFWTD_MASK_TYPE)mask_type, src0, src0_stride, src1, src1_stride, h, w, &cp, bd);
}

/* The RTCD-DISPATCHED entry, so a test can also pin this host's dispatched
 * tier against the `_c` kernel on identical inputs (same purpose as
 * ref_lowbd_blend_a64_d16_mask_rtcd in inter_pred_shims.c). */
void ref_build_compound_diffwtd_mask_d16_rtcd(uint8_t* mask, int mask_type, const uint16_t* src0, int src0_stride,
                                              const uint16_t* src1, int src1_stride, int h, int w, int bd) {
    ensure_gap_rtcd();
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, /*is_compound=*/1, bd);
    svt_av1_build_compound_diffwtd_mask_d16(
        mask, (DIFFWTD_MASK_TYPE)mask_type, src0, src0_stride, src1, src1_stride, h, w, &cp, bd);
}

/* Expose the `round` the C kernel derives, so the port's own derivation is
 * gated rather than re-transcribed:
 * `2 * FILTER_BITS - round_0 - round_1 + (bd - 8)`. */
int ref_d16_diff_round(int is_compound, int bd) {
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, is_compound, bd);
    return 2 * FILTER_BITS - cp.round_0 - cp.round_1 + (bd - 8);
}

/* ---- svt_aom_pack_block ----------------------------------------------- */

void svt_aom_pack_block(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer, uint32_t inn_stride,
                        uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height);
void svt_enc_msb_pack2_d(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                         uint16_t* out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width,
                         uint32_t height);

/* `svt_aom_pack_block` -> `svt_aom_pack2d_src`, which dispatches to the RTCD
 * pointer `svt_pack2d_16_bit_src_mul4` when width%4==0 && height%2==0. That
 * pointer is .bss-NULL until setup on x86-64 (common_dsp_rtcd.h:148); on
 * aarch64 it is `#define`d to `svt_enc_msb_pack2d_neon`
 * (common_dsp_rtcd_neon_devirt.h:44) and the hazard cannot fire. Init anyway. */
void ref_pack_block(const uint8_t* in8, uint32_t in8_stride, const uint8_t* inn, uint32_t inn_stride, uint16_t* out16,
                    uint32_t out_stride, uint32_t width, uint32_t height) {
    ensure_gap_rtcd();
    svt_aom_pack_block((uint8_t*)in8, in8_stride, (uint8_t*)inn, inn_stride, out16, out_stride, width, height);
}

/* The SCALAR arm on its own, so a test can pin the two C arms against each
 * other on an extent where the dispatch would have chosen the SIMD one. */
void ref_enc_msb_pack2_d(const uint8_t* in8, uint32_t in8_stride, const uint8_t* inn, uint32_t inn_stride,
                         uint16_t* out16, uint32_t out_stride, uint32_t width, uint32_t height) {
    svt_enc_msb_pack2_d((uint8_t*)in8, in8_stride, (uint8_t*)inn, out16, inn_stride, out_stride, width, height);
}

/* ---- svt_inter_predictor_light_pd1, the bd > 8 arm --------------------- */

/* The 8-bit arm is bound in inter_pred_shims.c
 * (`ref_inter_predictor_light_pd1_8bit`); this is the OTHER branch of the same
 * C function, which packs `src` (8 MSB) + `src_2b` (2 LSB) into a 10-bit
 * scratch with `svt_aom_pack_block` and convolves through
 * `svt_aom_convolveHbd[][][]`.
 *
 * CONTRACT THE ENCODER HANDS IT, reproduced here (WORKING-ON-THIS §5 trap 4):
 *  - both planes are read from `src - 8 - 8 * src_stride`, so the caller must
 *    pass an origin at least 8 rows and 8 columns inside its buffer;
 *  - BOTH planes are indexed at `src_stride` (:1312 passes `src_stride` for
 *    the n-bit stride too) — a separate n-bit stride is never used;
 *  - `svt_aom_convolveHbd` is a function-pointer TABLE filled by
 *    `svt_aom_asm_set_convolve_hbd_asm_table()` FROM the RTCD pointers, so
 *    both inits must have run, in that order, before the first call --
 *    `init_gap_rtcd` above does both under one `pthread_once`. */
void ref_inter_predictor_light_pd1_hbd(uint8_t* src, uint8_t* src_2b, int32_t src_stride, uint16_t* dst,
                                       int32_t dst_stride, int32_t w, int32_t h, uint32_t interp_filters, int32_t xs,
                                       int32_t ys, int32_t subpel_x, int32_t subpel_y, uint16_t* conv_buf,
                                       int conv_stride, int is_compound, int do_average, int use_jnt, int fwd, int bck,
                                       int bd) {
    ensure_gap_rtcd();
    SubpelParams   sp        = {.xs = xs, .ys = ys, .subpel_x = subpel_x, .subpel_y = subpel_y};
    ConvolveParams cp        = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    cp.use_jnt_comp_avg      = use_jnt;
    cp.use_dist_wtd_comp_avg = use_jnt;
    cp.fwd_offset            = fwd;
    cp.bck_offset            = bck;
    svt_inter_predictor_light_pd1(
        src, src_2b, src_stride, (uint8_t*)dst, dst_stride, w, h, interp_filters, &sp, &cp, bd);
}
