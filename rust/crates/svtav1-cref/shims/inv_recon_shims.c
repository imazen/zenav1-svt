/*
 * C shims for the inverse-transform RECONSTRUCTION entries of
 * Codec/inv_transforms.c:
 *   svt_aom_inv_transform_recon8bit (:3138)  - u8 pixels, bd 8
 *   svt_aom_inv_transform_recon     (:3237)  - u16 pixels, any bd
 *   svt_av1_inv_txfm_add_c          (:3266)  - the pinned SCALAR route the
 *                                              first one reaches only when
 *                                              RTCD dispatches to `_c`
 *
 * Its own translation unit so the wx-txfm lane never shares a shim file with
 * another lane.
 *
 * RULE (see the header of ref_shims.c): NO PER-CALL STATE IN A `static`.
 * The only file-scope object below is the one-shot RTCD guard, which is
 * dispatch setup, not per-call state.
 *
 * RTCD IS MANDATORY HERE, and this was checked rather than assumed.
 * `nm -gU Bin/Release/libSvtAv1Enc.a` reports `_svt_av1_inv_txfm_add` and
 * `_svt_av1_inv_txfm2d_add_*` as `C` (common, i.e. .bss function pointers) in
 * THIS aarch64 build: the NEON devirtualisation header
 * (common_dsp_rtcd_neon_devirt.h:35,40) that would `#define` them to direct
 * calls is NOT active, so every one of them is NULL until
 * `svt_aom_setup_common_rtcd_internal` runs. Both entry points below reach
 * those pointers one level down, so both call `ensure_rtcd()` first.
 *
 * ALIGNMENT AND STRIDE ARE PART OF THE CONTRACT, and this cost a SIGSEGV.
 * A first version handed C the Rust `Vec` pointers directly, at stride `w`.
 * It passed on macOS aarch64 and CRASHED on x86-64 Linux inside
 * `svt_dav1d_inv_txfm2d_add_8x8_avx2` (gdb: frame #0), reached from
 * `svt_aom_inv_transform_recon` -> `highbd_inv_txfm_add`. Only the
 * HIGH-BIT-DEPTH entry crashed, and that is the tell: the 8-bit entry goes
 * through `svt_av1_inv_txfm_add`, which stages the caller's pixels into its
 * own `DECLARE_ALIGNED(32, uint16_t, tmp[MAX_TX_SQUARE])` before touching a
 * kernel, so it never hands C's SIMD a caller buffer at all. The hbd entry
 * passes the caller's pointers straight down.
 *
 * So every buffer below is staged into `DECLARE_ALIGNED(64, ...)` scratch at
 * stride `MAX_TX_SIZE`, which is the shape the ENCODER hands these entries:
 * its recon planes are 64-aligned picture buffers with a picture stride, not
 * tightly packed `w * h` blocks (full_loop.c:1915 passes
 * `rec_buffer + rec_offset, rec_stride`). Staging also makes the strides
 * differ from `w`, so a stride bug in the port can no longer hide behind
 * `pred_stride == recon_stride == w`.
 *
 * WHICH ORACLE THIS IS, per ISA. After setup, `svt_av1_inv_txfm_add` is
 * `svt_dav1d_inv_txfm_add_neon` on aarch64 (common_dsp_rtcd.c:1099) and
 * `svt_av1_inv_txfm_add_{ssse3,avx2}` on x86-64 (:540/:542) - a different
 * implementation family from `svt_av1_inv_txfm_add_c`, not merely a widened
 * one. `ref_inv_txfm_add_c` below therefore exists as the PINNED scalar
 * route, so a divergence can be attributed to the port or to C's own SIMD
 * choice instead of being ambiguous.
 */
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "inv_transforms.h"

EbErrorType svt_aom_inv_transform_recon8bit(int32_t* coeff_buffer, uint8_t* recon_buffer_r, uint32_t recon_stride_r,
                                            uint8_t* recon_buffer_w, uint32_t recon_stride_w, TxSize txsize,
                                            TxType transform_type, PlaneType component_type, uint32_t eob,
                                            uint8_t lossless);
EbErrorType svt_aom_inv_transform_recon(int32_t* coeff_buffer, uint8_t* recon_buffer_r, uint32_t recon_stride_r,
                                        uint8_t* recon_buffer_w, uint32_t recon_stride_w, TxSize txsize,
                                        uint32_t bit_depth, TxType transform_type, PlaneType component_type,
                                        uint32_t eob, uint8_t lossless);
void svt_av1_inv_txfm_add_c(const TranLow* dqcoeff, uint8_t* dst_r, int32_t stride_r, uint8_t* dst_w, int32_t stride_w,
                            const TxfmParam* txfm_param);

void        svt_aom_setup_common_rtcd_internal(uint64_t flags);
void        svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags  svt_aom_get_cpu_flags_to_use(void);

/* Coefficients a tx_size's buffer actually holds. The 64-dimension inverse
 * wrappers read min(w,32) columns over min(h,32) rows (inv_transforms.c:2614-
 * :2733) because svt_handle_transform* already repacked the forward output to
 * that shape, so tx_size_2d[] would over-read the caller's buffer there. */
static size_t coeff_count(TxSize t) {
    size_t w = tx_size_wide[t], h = tx_size_high[t];
    if (w > 32) w = 32;
    if (h > 32) h = 32;
    return w * h;
}

/* Scratch stride for every staged buffer: C's own `svt_av1_inv_txfm_add_c`
 * uses MAX_TX_SIZE for the same purpose (inv_transforms.c:3269). */
#define SHIM_STRIDE MAX_TX_SIZE

static int inv_recon_rtcd_done = 0;
static void ensure_rtcd(void) {
    if (!inv_recon_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        inv_recon_rtcd_done = 1;
    }
}

/* 8-bit entry. `pred` and `recon` are SEPARATE buffers unless
 * `alias_in_place` is set, in which case both C pointers are `recon` - the
 * shape TPL uses (src_ops_process.c:1142) and the only one in which C keeps
 * the caller's `eob`. */
void ref_inv_transform_recon8bit(const int32_t* coeff, const uint8_t* pred, uint32_t pred_stride, uint8_t* recon,
                                 uint32_t recon_stride, int32_t txsize, int32_t tx_type, uint32_t eob,
                                 uint8_t lossless, int32_t alias_in_place) {
    ensure_rtcd();
    const TxSize  ts = (TxSize)txsize;
    const int32_t w = tx_size_wide[ts], h = tx_size_high[ts];
    DECLARE_ALIGNED(64, int32_t, coeff_copy[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint8_t, sp[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint8_t, sd[MAX_TX_SQUARE]);
    memset(sp, 0, sizeof(sp));
    memset(sd, 0, sizeof(sd));
    memcpy(coeff_copy, coeff, sizeof(int32_t) * coeff_count(ts));
    for (int32_t r = 0; r < h; ++r)
        memcpy(sp + (size_t)r * SHIM_STRIDE, (alias_in_place ? recon : pred) + (size_t)r * (alias_in_place ? recon_stride : pred_stride),
               (size_t)w);
    if (alias_in_place) {
        svt_aom_inv_transform_recon8bit(
            coeff_copy, sp, SHIM_STRIDE, sp, SHIM_STRIDE, ts, (TxType)tx_type, PLANE_TYPE_Y, eob, lossless);
        for (int32_t r = 0; r < h; ++r)
            memcpy(recon + (size_t)r * recon_stride, sp + (size_t)r * SHIM_STRIDE, (size_t)w);
    } else {
        svt_aom_inv_transform_recon8bit(
            coeff_copy, sp, SHIM_STRIDE, sd, SHIM_STRIDE, ts, (TxType)tx_type, PLANE_TYPE_Y, eob, lossless);
        for (int32_t r = 0; r < h; ++r)
            memcpy(recon + (size_t)r * recon_stride, sd + (size_t)r * SHIM_STRIDE, (size_t)w);
    }
}

/* The pinned SCALAR route: `svt_av1_inv_txfm_add_c` with the TxfmParam
 * `svt_aom_inv_transform_recon8bit` would have built, so the only difference
 * from the entry above is that RTCD cannot pick a SIMD `svt_av1_inv_txfm_add`
 * variant. (Its per-size arms still go through the
 * `svt_av1_inv_txfm2d_add_*` pointers, which is why RTCD setup still runs.) */
void ref_inv_txfm_add_c(const int32_t* coeff, const uint8_t* pred, uint32_t pred_stride, uint8_t* recon,
                        uint32_t recon_stride, int32_t txsize, int32_t tx_type, uint32_t eob, uint8_t lossless) {
    ensure_rtcd();
    const TxSize  ts = (TxSize)txsize;
    const int32_t w = tx_size_wide[ts], h = tx_size_high[ts];
    TxfmParam     p;
    memset(&p, 0, sizeof(p));
    p.tx_type  = (TxType)tx_type;
    p.tx_size  = ts;
    p.eob      = (int32_t)eob;
    p.lossless = lossless;
    p.bd       = 8;
    p.is_hbd   = 1;
    DECLARE_ALIGNED(64, int32_t, coeff_copy[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint8_t, sp[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint8_t, sd[MAX_TX_SQUARE]);
    memset(sp, 0, sizeof(sp));
    memset(sd, 0, sizeof(sd));
    memcpy(coeff_copy, coeff, sizeof(int32_t) * coeff_count(ts));
    for (int32_t r = 0; r < h; ++r)
        memcpy(sp + (size_t)r * SHIM_STRIDE, pred + (size_t)r * pred_stride, (size_t)w);
    svt_av1_inv_txfm_add_c(coeff_copy, sp, SHIM_STRIDE, sd, SHIM_STRIDE, &p);
    for (int32_t r = 0; r < h; ++r)
        memcpy(recon + (size_t)r * recon_stride, sd + (size_t)r * SHIM_STRIDE, (size_t)w);
}

/* High-bit-depth entry. C takes `uint8_t*` and CONVERT_TO_BYTEPTR()s a
 * uint16_t* into it (definitions.h:1019 - the libaom pointer<<1 trick), so
 * the shim hands it the same encoding its own callers do
 * (full_loop.c:1915). */
void ref_inv_transform_recon(const int32_t* coeff, const uint16_t* pred, uint32_t pred_stride, uint16_t* recon,
                             uint32_t recon_stride, int32_t txsize, uint32_t bit_depth, int32_t tx_type, uint32_t eob,
                             uint8_t lossless, int32_t alias_in_place) {
    ensure_rtcd();
    const TxSize  ts = (TxSize)txsize;
    const int32_t w = tx_size_wide[ts], h = tx_size_high[ts];
    DECLARE_ALIGNED(64, int32_t, coeff_copy[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint16_t, sp[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint16_t, sd[MAX_TX_SQUARE]);
    memset(sp, 0, sizeof(sp));
    memset(sd, 0, sizeof(sd));
    memcpy(coeff_copy, coeff, sizeof(int32_t) * coeff_count(ts));
    for (int32_t r = 0; r < h; ++r)
        memcpy(sp + (size_t)r * SHIM_STRIDE,
               (alias_in_place ? recon : pred) + (size_t)r * (alias_in_place ? recon_stride : pred_stride),
               sizeof(uint16_t) * (size_t)w);
    uint16_t* dst = alias_in_place ? sp : sd;
    svt_aom_inv_transform_recon(coeff_copy,
                                CONVERT_TO_BYTEPTR(sp),
                                SHIM_STRIDE,
                                CONVERT_TO_BYTEPTR(dst),
                                SHIM_STRIDE,
                                ts,
                                bit_depth,
                                (TxType)tx_type,
                                PLANE_TYPE_Y,
                                eob,
                                lossless);
    for (int32_t r = 0; r < h; ++r)
        memcpy(recon + (size_t)r * recon_stride, dst + (size_t)r * SHIM_STRIDE, sizeof(uint16_t) * (size_t)w);
}

/* ---- The pinned SCALAR hbd route ----
 *
 * `svt_aom_inv_transform_recon` reaches its per-size kernels through the
 * `svt_av1_inv_txfm2d_add_*` RTCD pointers, so on x86-64 it runs
 * `svt_dav1d_inv_txfm2d_add_*_avx2` and on aarch64 `_neon`. Calling the `_c`
 * symbols directly is the only way to ask "what does C's own SCALAR
 * definition say", which is what separates a port defect from a per-ISA
 * difference in C's SIMD. Added 2026-08-31 after bd12 diverged BETWEEN ISAs.
 *
 * The `_c` family has three signature shapes (common_dsp_rtcd.h): squares
 * take `(tx_type, bd)`, the small rects add `tx_size`, the rest add `eob`
 * too. `tx_size` and `eob` are UNUSED in the bodies. */
#define DECL_INV_SQ(N) \
    void svt_av1_inv_txfm2d_add_##N##x##N##_c(const int32_t*, uint16_t*, int32_t, uint16_t*, int32_t, TxType, int32_t);
DECL_INV_SQ(4) DECL_INV_SQ(8) DECL_INV_SQ(16) DECL_INV_SQ(32) DECL_INV_SQ(64)
#define DECL_INV_R(W, H) \
    void svt_av1_inv_txfm2d_add_##W##x##H##_c(                                                        \
        const int32_t*, uint16_t*, int32_t, uint16_t*, int32_t, TxType, TxSize, int32_t);
DECL_INV_R(4, 8) DECL_INV_R(8, 4) DECL_INV_R(4, 16) DECL_INV_R(16, 4)
#define DECL_INV_RE(W, H) \
    void svt_av1_inv_txfm2d_add_##W##x##H##_c(                                                        \
        const int32_t*, uint16_t*, int32_t, uint16_t*, int32_t, TxType, TxSize, int32_t, int32_t);
DECL_INV_RE(8, 16) DECL_INV_RE(16, 8) DECL_INV_RE(16, 32) DECL_INV_RE(32, 16)
DECL_INV_RE(32, 64) DECL_INV_RE(64, 32) DECL_INV_RE(8, 32) DECL_INV_RE(32, 8)
DECL_INV_RE(16, 64) DECL_INV_RE(64, 16)

/* Inverse transform + add through the `_c` kernel for `txsize`, at `bd`.
 * Buffers are staged exactly as the dispatched entries above stage theirs. */
void ref_inv_txfm2d_add_c_bd(const int32_t* coeff, const uint16_t* pred, uint32_t pred_stride, uint16_t* recon,
                             uint32_t recon_stride, int32_t txsize, int32_t tx_type, int32_t bd) {
    ensure_rtcd();
    const TxSize  ts = (TxSize)txsize;
    const TxType  tt = (TxType)tx_type;
    const int32_t w = tx_size_wide[ts], h = tx_size_high[ts];
    DECLARE_ALIGNED(64, int32_t, cc[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint16_t, sp[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(64, uint16_t, sd[MAX_TX_SQUARE]);
    memset(sp, 0, sizeof(sp));
    memset(sd, 0, sizeof(sd));
    memcpy(cc, coeff, sizeof(int32_t) * coeff_count(ts));
    for (int32_t r = 0; r < h; ++r)
        memcpy(sp + (size_t)r * SHIM_STRIDE, pred + (size_t)r * pred_stride, sizeof(uint16_t) * (size_t)w);
#define CALL_SQ(N) \
    case TX_##N##X##N: svt_av1_inv_txfm2d_add_##N##x##N##_c(cc, sp, SHIM_STRIDE, sd, SHIM_STRIDE, tt, bd); break;
#define CALL_R(W, H) \
    case TX_##W##X##H: \
        svt_av1_inv_txfm2d_add_##W##x##H##_c(cc, sp, SHIM_STRIDE, sd, SHIM_STRIDE, tt, ts, bd); \
        break;
#define CALL_RE(W, H) \
    case TX_##W##X##H: \
        svt_av1_inv_txfm2d_add_##W##x##H##_c(cc, sp, SHIM_STRIDE, sd, SHIM_STRIDE, tt, ts, 0, bd); \
        break;
    switch (ts) {
        CALL_SQ(4) CALL_SQ(8) CALL_SQ(16) CALL_SQ(32) CALL_SQ(64)
        CALL_R(4, 8) CALL_R(8, 4) CALL_R(4, 16) CALL_R(16, 4)
        CALL_RE(8, 16) CALL_RE(16, 8) CALL_RE(16, 32) CALL_RE(32, 16)
        CALL_RE(32, 64) CALL_RE(64, 32) CALL_RE(8, 32) CALL_RE(32, 8)
        CALL_RE(16, 64) CALL_RE(64, 16)
    default: break;
    }
    for (int32_t r = 0; r < h; ++r)
        memcpy(recon + (size_t)r * recon_stride, sd + (size_t)r * SHIM_STRIDE, sizeof(uint16_t) * (size_t)w);
}
