/*
 * C shims for differential parity testing.
 *
 * Exposes `static INLINE` functions from SVT-AV1 headers (which are not
 * linkable symbols) plus size/alignment info for opaque structs, so the Rust
 * side can drive the exact reference implementation.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

#include "bitstream_unit.h"
#include "cabac_context_model.h"

/* ---- OdEcEnc (range encoder) ---- */

size_t ref_od_ec_enc_sizeof(void) { return sizeof(OdEcEnc); }
size_t ref_od_ec_enc_alignof(void) { return _Alignof(OdEcEnc); }

/* v4.2.0-rc: the coder borrows its buffer (no internal malloc); the shim
   owns a fixed generous buffer so the Rust API stays capacity-based. */
#define REF_EC_BUF_CAP (1u << 20)

void ref_od_ec_enc_init(void* enc, uint32_t size) {
    OdEcEnc* e = (OdEcEnc*)enc;
    (void)size;
    svt_od_ec_enc_init(e);
    e->buf = (unsigned char*)malloc(REF_EC_BUF_CAP);
    svt_od_ec_enc_reset(e);
}
void ref_od_ec_enc_reset(void* enc) { svt_od_ec_enc_reset((OdEcEnc*)enc); }
void ref_od_ec_enc_clear(void* enc) {
    /* v4.2 removed the clear() declaration (buffer is borrowed); the shim
       owns the buffer, so free it and null the pointers directly. */
    OdEcEnc* e = (OdEcEnc*)enc;
    free(e->buf);
    e->buf = NULL;
    e->ptr = NULL;
}

void ref_od_ec_encode_cdf_q15(void* enc, int32_t s, const uint16_t* icdf, int32_t nsyms) {
    svt_od_ec_encode_cdf_q15((OdEcEnc*)enc, s, icdf, nsyms);
}

void ref_od_ec_encode_bool_q15(void* enc, int32_t val, uint32_t f) {
    svt_od_ec_encode_bool_q15((OdEcEnc*)enc, val, (unsigned)f);
}

const uint8_t* ref_od_ec_enc_done(void* enc, uint32_t* nbytes) {
    return svt_od_ec_enc_done((OdEcEnc*)enc, nbytes);
}

int32_t ref_od_ec_enc_error(const void* enc) { return ((const OdEcEnc*)enc)->error; }

uint32_t ref_od_ec_enc_tell(const void* enc) { return (uint32_t)svt_od_ec_enc_tell((const OdEcEnc*)enc); }

/* ---- CDF adaptation (static INLINE in cabac_context_model.h) ---- */

void ref_update_cdf(uint16_t* cdf, int8_t val, int32_t nsymbs) {
    update_cdf((AomCdfProb*)cdf, val, nsymbs);
}

/* ---- aom_write_symbol: encode + in-place CDF update (the real write path) ---- */

void ref_write_symbol(void* enc, int32_t symb, uint16_t* cdf, int32_t nsymbs) {
    svt_od_ec_encode_cdf_q15((OdEcEnc*)enc, symb, cdf, nsymbs);
    update_cdf((AomCdfProb*)cdf, (int8_t)symb, nsymbs);
}

/* ---- Default CDF table extraction (FRAME_CONTEXT) ---- */

#include <string.h>

static FRAME_CONTEXT g_fc;

/* RTCD dispatch pointers (svt_memcpy etc.) are populated by the library's
   init path; without this, table init calls through NULL. */
void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
uint64_t   svt_aom_get_cpu_flags_to_use(void);
static int g_rtcd_ready = 0;

void ref_fc_init(int32_t base_qindex) {
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    memset(&g_fc, 0, sizeof(g_fc));
    svt_av1_default_coef_probs(&g_fc, base_qindex);
    svt_aom_init_mode_probs(&g_fc);
}

#define REF_TBL(name)                                                            \
    size_t ref_fc_sizeof_##name(void) { return sizeof(g_fc.name); }              \
    void ref_fc_copy_##name(uint16_t* dst) { memcpy(dst, &g_fc.name, sizeof(g_fc.name)); }

REF_TBL(txb_skip_cdf)
REF_TBL(eob_extra_cdf)
REF_TBL(dc_sign_cdf)
REF_TBL(eob_flag_cdf16)
REF_TBL(eob_flag_cdf32)
REF_TBL(eob_flag_cdf64)
REF_TBL(eob_flag_cdf128)
REF_TBL(eob_flag_cdf256)
REF_TBL(eob_flag_cdf512)
REF_TBL(eob_flag_cdf1024)
REF_TBL(coeff_base_eob_cdf)
REF_TBL(coeff_base_cdf)
REF_TBL(coeff_br_cdf)
REF_TBL(partition_cdf)
REF_TBL(skip_cdfs)
REF_TBL(kf_y_cdf)
REF_TBL(angle_delta_cdf)
REF_TBL(intra_ext_tx_cdf)
REF_TBL(tx_size_cdf)
REF_TBL(uv_mode_cdf)
REF_TBL(filter_intra_cdfs)
REF_TBL(filter_intra_mode_cdf)
REF_TBL(delta_q_cdf)
REF_TBL(intrabc_cdf)
REF_TBL(y_mode_cdf)

/* ---- Scan orders + coefficient-context helpers ---- */

#include "coefficients.h"
#include "common_utils.h"
#include "encode_txb_ref_c.h"
#include "entropy_coding.h"

int32_t ref_scan_len(int32_t tx_size) {
    const TxSize adj = av1_get_adjusted_tx_size((TxSize)tx_size);
    return get_txb_wide(adj) * get_txb_high(adj);
}

void ref_scan_copy(int32_t tx_size, int32_t scan_class, int16_t* scan_out, int32_t len) {
    const ScanOrder* so = &eb_av1_scan_orders[tx_size][scan_class];
    memcpy(scan_out, so->scan, (size_t)len * sizeof(int16_t));
}

int32_t ref_tx_type_to_scan_index(int32_t tx_type) { return tx_type_to_scan_index[tx_type]; }

int32_t ref_get_br_ctx(const uint8_t* levels, int32_t c, int32_t bwl, int32_t tx_class) {
    return get_br_ctx(levels, c, bwl, (TxClass)tx_class);
}

int32_t ref_get_eob_pos_token(int32_t eob, int32_t* extra) {
    int e = 0;
    int t = get_eob_pos_token(eob, &e);
    *extra = e;
    return t;
}

int32_t ref_nz_map_ctx_offset(int32_t tx_size, int32_t coeff_idx) {
    return eb_av1_nz_map_ctx_offset[tx_size][coeff_idx];
}

/* Declared in the generated RTCD header only; prototype it directly. */
void svt_av1_txb_init_levels_c(const int32_t* const coeff, const int32_t width, const int32_t height,
                               uint8_t* const levels);

void ref_txb_init_levels(const int32_t* coeff, int32_t width, int32_t height, uint8_t* levels) {
    svt_av1_txb_init_levels_c(coeff, width, height, levels);
}

void ref_get_nz_map_contexts(const uint8_t* levels, const int16_t* scan, uint16_t eob, int32_t tx_size,
                             int32_t tx_class, int8_t* coeff_contexts) {
    svt_av1_get_nz_map_contexts_c(levels, scan, eob, (TxSize)tx_size, (TxClass)tx_class, coeff_contexts);
}

int32_t ref_get_txsize_entropy_ctx(int32_t tx_size) { return (int32_t)get_txsize_entropy_ctx((TxSize)tx_size); }
int32_t ref_get_txb_bwl(int32_t tx_size) { return get_txb_bwl((TxSize)tx_size); }
int32_t ref_get_txb_wide(int32_t tx_size) { return get_txb_wide((TxSize)tx_size); }
int32_t ref_get_txb_high(int32_t tx_size) { return get_txb_high((TxSize)tx_size); }

/* ---- AV1 quantizer step tables ---- */

int16_t svt_aom_dc_quant_qtx(int qindex, int delta, EbBitDepth bit_depth);
int16_t svt_aom_ac_quant_qtx(int qindex, int delta, EbBitDepth bit_depth);

int16_t ref_dc_quant_qtx(int32_t qindex) { return svt_aom_dc_quant_qtx(qindex, 0, EB_EIGHT_BIT); }
int16_t ref_ac_quant_qtx(int32_t qindex) { return svt_aom_ac_quant_qtx(qindex, 0, EB_EIGHT_BIT); }

/* ---- 2D transform wrappers (square sizes, C reference versions) ---- */

#define DECL_TXFM2D(N) \
    void svt_av1_transform_two_d_##N##x##N##_c(int16_t* input, int32_t* output, uint32_t input_stride, \
                                               TxType tx_type, uint8_t bd); \
    void svt_av1_inv_txfm2d_add_##N##x##N##_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, \
                                              uint16_t* output_w, int32_t stride_w, TxType tx_type, int32_t bd);
DECL_TXFM2D(4)
DECL_TXFM2D(8)
DECL_TXFM2D(16)
DECL_TXFM2D(32)
DECL_TXFM2D(64)

void ref_fwd_txfm2d(int32_t n, int16_t* input, int32_t* output, uint32_t stride, int32_t tx_type) {
    switch (n) {
    case 4: svt_av1_transform_two_d_4x4_c(input, output, stride, (TxType)tx_type, 8); break;
    case 8: svt_av1_transform_two_d_8x8_c(input, output, stride, (TxType)tx_type, 8); break;
    case 16: svt_av1_transform_two_d_16x16_c(input, output, stride, (TxType)tx_type, 8); break;
    case 32: svt_av1_transform_two_d_32x32_c(input, output, stride, (TxType)tx_type, 8); break;
    default: svt_av1_transform_two_d_64x64_c(input, output, stride, (TxType)tx_type, 8); break;
    }
}

void ref_inv_txfm2d_add(int32_t n, const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                        int32_t stride_w, int32_t tx_type) {
    switch (n) {
    case 4: svt_av1_inv_txfm2d_add_4x4_c(input, output_r, stride_r, output_w, stride_w, (TxType)tx_type, 8); break;
    case 8: svt_av1_inv_txfm2d_add_8x8_c(input, output_r, stride_r, output_w, stride_w, (TxType)tx_type, 8); break;
    case 16: svt_av1_inv_txfm2d_add_16x16_c(input, output_r, stride_r, output_w, stride_w, (TxType)tx_type, 8); break;
    case 32: svt_av1_inv_txfm2d_add_32x32_c(input, output_r, stride_r, output_w, stride_w, (TxType)tx_type, 8); break;
    default: svt_av1_inv_txfm2d_add_64x64_c(input, output_r, stride_r, output_w, stride_w, (TxType)tx_type, 8); break;
    }
}

/* ---- 2D transform wrappers (rectangular sizes, C reference versions) ---- */

#define DECL_FWD_RECT(W, H) \
    void svt_av1_fwd_txfm2d_##W##x##H##_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType tx_type, \
                                          uint8_t bd);
DECL_FWD_RECT(4, 8)
DECL_FWD_RECT(8, 4)
DECL_FWD_RECT(8, 16)
DECL_FWD_RECT(16, 8)
DECL_FWD_RECT(16, 32)
DECL_FWD_RECT(32, 16)
DECL_FWD_RECT(32, 64)
DECL_FWD_RECT(64, 32)
DECL_FWD_RECT(4, 16)
DECL_FWD_RECT(16, 4)
DECL_FWD_RECT(8, 32)
DECL_FWD_RECT(32, 8)
DECL_FWD_RECT(16, 64)
DECL_FWD_RECT(64, 16)

/* Small rects take (..., tx_type, tx_size, bd); the rest take
 * (..., tx_type, tx_size, eob, bd). tx_size/eob are UNUSED in the C bodies. */
#define DECL_INV_RECT(W, H) \
    void svt_av1_inv_txfm2d_add_##W##x##H##_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, \
                                              uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, \
                                              int32_t bd);
#define DECL_INV_RECT_EOB(W, H) \
    void svt_av1_inv_txfm2d_add_##W##x##H##_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, \
                                              uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, \
                                              int32_t eob, int32_t bd);
DECL_INV_RECT(4, 8)
DECL_INV_RECT(8, 4)
DECL_INV_RECT(4, 16)
DECL_INV_RECT(16, 4)
DECL_INV_RECT_EOB(8, 16)
DECL_INV_RECT_EOB(16, 8)
DECL_INV_RECT_EOB(16, 32)
DECL_INV_RECT_EOB(32, 16)
DECL_INV_RECT_EOB(32, 64)
DECL_INV_RECT_EOB(64, 32)
DECL_INV_RECT_EOB(8, 32)
DECL_INV_RECT_EOB(32, 8)
DECL_INV_RECT_EOB(16, 64)
DECL_INV_RECT_EOB(64, 16)

void ref_fwd_txfm2d_rect(int32_t w, int32_t h, int16_t* input, int32_t* output, uint32_t stride, int32_t tx_type) {
    const TxType t = (TxType)tx_type;
    switch (w * 100 + h) {
    case 408: svt_av1_fwd_txfm2d_4x8_c(input, output, stride, t, 8); break;
    case 804: svt_av1_fwd_txfm2d_8x4_c(input, output, stride, t, 8); break;
    case 816: svt_av1_fwd_txfm2d_8x16_c(input, output, stride, t, 8); break;
    case 1608: svt_av1_fwd_txfm2d_16x8_c(input, output, stride, t, 8); break;
    case 1632: svt_av1_fwd_txfm2d_16x32_c(input, output, stride, t, 8); break;
    case 3216: svt_av1_fwd_txfm2d_32x16_c(input, output, stride, t, 8); break;
    case 3264: svt_av1_fwd_txfm2d_32x64_c(input, output, stride, t, 8); break;
    case 6432: svt_av1_fwd_txfm2d_64x32_c(input, output, stride, t, 8); break;
    case 416: svt_av1_fwd_txfm2d_4x16_c(input, output, stride, t, 8); break;
    case 1604: svt_av1_fwd_txfm2d_16x4_c(input, output, stride, t, 8); break;
    case 832: svt_av1_fwd_txfm2d_8x32_c(input, output, stride, t, 8); break;
    case 3208: svt_av1_fwd_txfm2d_32x8_c(input, output, stride, t, 8); break;
    case 1664: svt_av1_fwd_txfm2d_16x64_c(input, output, stride, t, 8); break;
    case 6416: svt_av1_fwd_txfm2d_64x16_c(input, output, stride, t, 8); break;
    default: break;
    }
}

void ref_inv_txfm2d_add_rect(int32_t w, int32_t h, const int32_t* input, uint16_t* output_r, int32_t stride_r,
                             uint16_t* output_w, int32_t stride_w, int32_t tx_type) {
    const TxType t = (TxType)tx_type;
    switch (w * 100 + h) {
    case 408: svt_av1_inv_txfm2d_add_4x8_c(input, output_r, stride_r, output_w, stride_w, t, TX_4X8, 8); break;
    case 804: svt_av1_inv_txfm2d_add_8x4_c(input, output_r, stride_r, output_w, stride_w, t, TX_8X4, 8); break;
    case 416: svt_av1_inv_txfm2d_add_4x16_c(input, output_r, stride_r, output_w, stride_w, t, TX_4X16, 8); break;
    case 1604: svt_av1_inv_txfm2d_add_16x4_c(input, output_r, stride_r, output_w, stride_w, t, TX_16X4, 8); break;
    case 816: svt_av1_inv_txfm2d_add_8x16_c(input, output_r, stride_r, output_w, stride_w, t, TX_8X16, 0, 8); break;
    case 1608: svt_av1_inv_txfm2d_add_16x8_c(input, output_r, stride_r, output_w, stride_w, t, TX_16X8, 0, 8); break;
    case 1632: svt_av1_inv_txfm2d_add_16x32_c(input, output_r, stride_r, output_w, stride_w, t, TX_16X32, 0, 8); break;
    case 3216: svt_av1_inv_txfm2d_add_32x16_c(input, output_r, stride_r, output_w, stride_w, t, TX_32X16, 0, 8); break;
    case 3264: svt_av1_inv_txfm2d_add_32x64_c(input, output_r, stride_r, output_w, stride_w, t, TX_32X64, 0, 8); break;
    case 6432: svt_av1_inv_txfm2d_add_64x32_c(input, output_r, stride_r, output_w, stride_w, t, TX_64X32, 0, 8); break;
    case 832: svt_av1_inv_txfm2d_add_8x32_c(input, output_r, stride_r, output_w, stride_w, t, TX_8X32, 0, 8); break;
    case 3208: svt_av1_inv_txfm2d_add_32x8_c(input, output_r, stride_r, output_w, stride_w, t, TX_32X8, 0, 8); break;
    case 1664: svt_av1_inv_txfm2d_add_16x64_c(input, output_r, stride_r, output_w, stride_w, t, TX_16X64, 0, 8); break;
    case 6416: svt_av1_inv_txfm2d_add_64x16_c(input, output_r, stride_r, output_w, stride_w, t, TX_64X16, 0, 8); break;
    default: break;
    }
}

/* ---- Deblocking loop filter kernels + thresholds ---- */

#include "deblocking_common.h"

void svt_aom_lpf_horizontal_4_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh);
void svt_aom_lpf_vertical_4_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh);
void svt_aom_lpf_horizontal_6_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh);
void svt_aom_lpf_vertical_6_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh);
void svt_aom_lpf_horizontal_8_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh);
void svt_aom_lpf_vertical_8_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh);
void svt_aom_lpf_horizontal_14_c(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                 const uint8_t* thresh);
void svt_aom_lpf_vertical_14_c(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                               const uint8_t* thresh);

/* kind: 0..7 = h4, v4, h6, v6, h8, v8, h14, v14. `off` indexes q0 of the
   first filtered line; the SIMD-width blimit/limit/thresh arrays only have
   their first byte read by the _c kernels, so scalars suffice. */
void ref_lpf(int32_t kind, uint8_t* buf, int32_t off, int32_t pitch, uint8_t blimit, uint8_t limit,
             uint8_t thresh) {
    uint8_t* s = buf + off;
    switch (kind) {
    case 0: svt_aom_lpf_horizontal_4_c(s, pitch, &blimit, &limit, &thresh); break;
    case 1: svt_aom_lpf_vertical_4_c(s, pitch, &blimit, &limit, &thresh); break;
    case 2: svt_aom_lpf_horizontal_6_c(s, pitch, &blimit, &limit, &thresh); break;
    case 3: svt_aom_lpf_vertical_6_c(s, pitch, &blimit, &limit, &thresh); break;
    case 4: svt_aom_lpf_horizontal_8_c(s, pitch, &blimit, &limit, &thresh); break;
    case 5: svt_aom_lpf_vertical_8_c(s, pitch, &blimit, &limit, &thresh); break;
    case 6: svt_aom_lpf_horizontal_14_c(s, pitch, &blimit, &limit, &thresh); break;
    case 7: svt_aom_lpf_vertical_14_c(s, pitch, &blimit, &limit, &thresh); break;
    default: break;
    }
}

/* Reference limits per level for a sharpness setting: lim_out/mblim_out are
   64-entry arrays indexed by filter level (svt_aom_update_sharpness). */
void ref_lf_limits(int32_t sharpness, uint8_t* lim_out, uint8_t* mblim_out) {
    static LoopFilterInfoN lfi; /* large; keep off the stack */
    svt_aom_update_sharpness(&lfi, sharpness);
    for (int l = 0; l <= MAX_LOOP_FILTER; l++) {
        lim_out[l]   = lfi.lfthr[l].lim[0];
        mblim_out[l] = lfi.lfthr[l].mblim[0];
    }
}
