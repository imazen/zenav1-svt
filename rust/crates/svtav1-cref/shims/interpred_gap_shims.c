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
static pthread_once_t g_gap_rtcd_once = PTHREAD_ONCE_INIT;
static void           init_gap_rtcd(void) {
    svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
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
