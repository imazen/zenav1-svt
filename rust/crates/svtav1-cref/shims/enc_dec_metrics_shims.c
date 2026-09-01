/*
 * C shims for `Codec/enc_dec_process.c`'s SSIM walkers (lane wx-rc).
 *
 * EVIDENCE TIER 1. `aom_ssim2` is `static` in C but survives the Release
 * build as a local symbol WITH ITS SOURCE ABI (prologue disassembled — see
 * `link_globalized_enc_dec_statics` in build.rs), so build.rs promotes it and
 * this wrapper calls the REAL compiled code. Nothing here is transcribed.
 *
 * One promotion pins the whole 8-bit chain: `ssim_8x8`,
 * `svt_aom_ssim_parms_8x8_c` and `svt_aom_similarity`, none of which has a
 * symbol of its own. The 10-bit twin is NOT bound — see below.
 *
 * RULE (see ref_shims.c): no per-call state in a `static`. These two are pure
 * functions over caller buffers, so there is none to have.
 */
#include <stdint.h>

#if defined(SVTAV1_CREF_ENC_DEC_STATICS)

/* Both are `static` in enc_dec_process.c, so no header declares them.
   Signatures transcribed from the definitions at :695 and :719. */
double aom_ssim2(const uint8_t* img1, int stride_img1, const uint8_t* img2, int stride_img2, int width, int height);
/* `aom_highbd_ssim2` is NOT bound: LLVM constant-folded its `bd` and `shift`
   arguments away (its only call site passes a literal shift = 0), so the
   compiled ABI is not the source one. See `link_globalized_enc_dec_statics`
   in build.rs for the evidence and the symptom. */

double ref_aom_ssim2(const uint8_t* img1, int32_t stride_img1, const uint8_t* img2, int32_t stride_img2, int32_t width,
                     int32_t height) {
    return aom_ssim2(img1, stride_img1, img2, stride_img2, width, height);
}

#endif /* SVTAV1_CREF_ENC_DEC_STATICS */
