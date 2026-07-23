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
REF_TBL(nmvc)
REF_TBL(ndvc)

/* ---- MV entropy encode (AUDIT 2026-07-14) ----
 *
 * Faithful transcription of svt_av1_encode_mv + encode_mv_component
 * (entropy_coding.c:1502-1576) driving the REAL primitives: the public
 * svt_av1_get_mv_class (md_rate_estimation.c) for the bit-exact class/offset
 * split, the header-inline aom_write_symbol (bitstream_unit.h) for the real
 * range coder + CDF adaptation, and the real default_nmv_context CDFs
 * (svt_aom_init_mode_probs). The CDFs adapt across the whole sequence exactly
 * as a frame's nmvc would, so the sequence oracle exercises adaptation too.
 * The MV-encode path is UNCHANGED 4.1->4.2 (not in mainline_v4.2.bit-affecting.diff).
 */
/* Direct oracle for the MV class/offset split (md_rate_estimation.c). */
int32_t ref_get_mv_class(int32_t z, int32_t* offset) { return (int32_t)svt_av1_get_mv_class(z, offset); }

static void ref_encode_mv_component(AomWriter* w, int32_t comp, NmvComponent* mvcomp,
                                    MvSubpelPrecision precision) {
    int32_t       offset;
    const int32_t sign     = comp < 0;
    const int32_t mag      = sign ? -comp : comp;
    const int32_t mv_class = svt_av1_get_mv_class(mag - 1, &offset);
    const int32_t d        = offset >> 3;
    const int32_t fr       = (offset >> 1) & 3;
    const int32_t hp       = offset & 1;

    aom_write_symbol(w, sign, mvcomp->sign_cdf, 2);
    aom_write_symbol(w, mv_class, mvcomp->classes_cdf, MV_CLASSES);
    if (mv_class == MV_CLASS_0) {
        aom_write_symbol(w, d, mvcomp->class0_cdf, CLASS0_SIZE);
    } else {
        const int32_t n = mv_class + CLASS0_BITS - 1;
        for (int32_t i = 0; i < n; ++i) {
            aom_write_symbol(w, (d >> i) & 1, mvcomp->bits_cdf[i], 2);
        }
    }
    if (precision > MV_SUBPEL_NONE) {
        aom_write_symbol(w, fr, mv_class == MV_CLASS_0 ? mvcomp->class0_fp_cdf[d] : mvcomp->fp_cdf, MV_FP_SIZE);
    }
    if (precision > MV_SUBPEL_LOW_PRECISION) {
        aom_write_symbol(w, hp, mv_class == MV_CLASS_0 ? mvcomp->class0_hp_cdf : mvcomp->hp_cdf, 2);
    }
}

/* Encode a whole sequence of MV diffs (mv - ref) with one adapting nmvc, in
 * the C encode order (Y/vertical component first). Returns the finalized byte
 * count; up to `cap` bytes are copied to `out`. `precision` is the
 * MvSubpelPrecision int (-1 none, 0 low, 1 high). */
uint32_t ref_encode_mv_seq(const int32_t* mv_y, const int32_t* mv_x, const int32_t* ref_y,
                           const int32_t* ref_x, int32_t n, int32_t precision, uint8_t* out,
                           uint32_t cap) {
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    FRAME_CONTEXT fc;
    memset(&fc, 0, sizeof(fc));
    svt_aom_init_mode_probs(&fc);
    NmvContext mvctx = fc.nmvc;

    AomWriter w;
    memset(&w, 0, sizeof(w));
    w.allow_update_cdf = 1;
    svt_od_ec_enc_init(&w.ec);
    w.ec.buf = (unsigned char*)malloc(REF_EC_BUF_CAP);
    svt_od_ec_enc_reset(&w.ec);

    for (int32_t k = 0; k < n; ++k) {
        int32_t           diff[2] = {mv_y[k] - ref_y[k], mv_x[k] - ref_x[k]};
        const MvJointType j       = (diff[0] == 0) ? (diff[1] == 0 ? MV_JOINT_ZERO : MV_JOINT_HNZVZ)
                                                   : (diff[1] == 0 ? MV_JOINT_HZVNZ : MV_JOINT_HNZVNZ);
        aom_write_symbol(&w, j, mvctx.joints_cdf, MV_JOINTS);
        if (mv_joint_vertical(j)) {
            ref_encode_mv_component(&w, diff[0], &mvctx.comps[0], (MvSubpelPrecision)precision);
        }
        if (mv_joint_horizontal(j)) {
            ref_encode_mv_component(&w, diff[1], &mvctx.comps[1], (MvSubpelPrecision)precision);
        }
    }

    uint32_t       nbytes = 0;
    const uint8_t* p      = svt_od_ec_enc_done(&w.ec, &nbytes);
    if (p) {
        uint32_t copyn = nbytes < cap ? nbytes : cap;
        memcpy(out, p, copyn);
    }
    free(w.ec.buf);
    return nbytes;
}

/* ---- Temporal-filter noise estimator (AUDIT 2026-07-14) ----
 * svt_estimate_noise_fp16_c (temporal_filtering.c:3555) is public/linkable and
 * unchanged 4.1->4.2 (the nearby bit-affecting hunk only adds VMAF RTCD decls).
 */
int32_t svt_estimate_noise_fp16_c(const uint8_t* src, uint16_t width, uint16_t height,
                                  uint16_t y_stride);
int32_t ref_estimate_noise_fp16(const uint8_t* src, uint16_t width, uint16_t height,
                                uint16_t y_stride) {
    return svt_estimate_noise_fp16_c(src, width, height, y_stride);
}

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

/* Bit-depth-parameterized variants (bd in {8,10,12}, the EbBitDepth values).
   The 8-bit-only shims above stay as-is for their existing callers. */
int16_t ref_dc_quant_qtx_bd(int32_t qindex, int32_t bd) {
    return svt_aom_dc_quant_qtx(qindex, 0, (EbBitDepth)bd);
}
int16_t ref_ac_quant_qtx_bd(int32_t qindex, int32_t bd) {
    return svt_aom_ac_quant_qtx(qindex, 0, (EbBitDepth)bd);
}

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

/* ---- High-bit-depth distortion / variance / SAD kernels ---- */
uint64_t svt_full_distortion_kernel16_bits_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                             uint8_t* pred, int32_t pred_offset, uint32_t pred_stride,
                                             uint32_t area_width, uint32_t area_height);
uint32_t svt_aom_variance_highbd_c(const uint16_t* a, int a_stride, const uint16_t* b, int b_stride, int w, int h,
                                   uint32_t* sse);
uint32_t svt_aom_sad_16b_kernel_c(uint16_t* src, uint32_t src_stride, uint16_t* ref, uint32_t ref_stride,
                                  uint32_t height, uint32_t width);

/* The C distortion kernel takes uint8_t* it internally reinterprets as
   uint16_t* AND applies the offsets after that cast, so a u16 buffer + a
   u16-element offset marshals directly (matching the port's plain-u16 form). */
uint64_t ref_full_distortion_kernel16(uint16_t* input, uint32_t in_off, uint32_t in_stride, uint16_t* pred,
                                      int32_t pred_off, uint32_t pred_stride, uint32_t w, uint32_t h) {
    return svt_full_distortion_kernel16_bits_c(
        (uint8_t*)input, in_off, in_stride, (uint8_t*)pred, pred_off, pred_stride, w, h);
}

uint32_t ref_variance_highbd(const uint16_t* a, int a_stride, const uint16_t* b, int b_stride, int w, int h,
                             uint32_t* sse_out) {
    return svt_aom_variance_highbd_c(a, a_stride, b, b_stride, w, h, sse_out);
}

/* C parameter order is (src, src_stride, ref, ref_stride, HEIGHT, WIDTH). */
uint32_t ref_sad_16b_kernel(uint16_t* src, uint32_t src_stride, uint16_t* ref, uint32_t ref_stride, uint32_t height,
                            uint32_t width) {
    return svt_aom_sad_16b_kernel_c(src, src_stride, ref, ref_stride, height, width);
}

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

/* High bit depth loop filters (deblocking_common.c:473+). Same kind dispatch
   as ref_lpf but on a uint16_t plane with an extra bd (10 or 12). The port's
   hbd::lpf_*_hbd entry points are the differential target. */
void svt_aom_highbd_lpf_horizontal_4_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_4_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_6_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_6_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_8_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_8_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_14_c(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                        const uint8_t* thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_14_c(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                      const uint8_t* thresh, int32_t bd);

void ref_lpf_hbd(int32_t kind, uint16_t* buf, int32_t off, int32_t pitch, uint8_t blimit, uint8_t limit,
                 uint8_t thresh, int32_t bd) {
    uint16_t* s = buf + off;
    switch (kind) {
    case 0: svt_aom_highbd_lpf_horizontal_4_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 1: svt_aom_highbd_lpf_vertical_4_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 2: svt_aom_highbd_lpf_horizontal_6_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 3: svt_aom_highbd_lpf_vertical_6_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 4: svt_aom_highbd_lpf_horizontal_8_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 5: svt_aom_highbd_lpf_vertical_8_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 6: svt_aom_highbd_lpf_horizontal_14_c(s, pitch, &blimit, &limit, &thresh, bd); break;
    case 7: svt_aom_highbd_lpf_vertical_14_c(s, pitch, &blimit, &limit, &thresh, bd); break;
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

/* ---- CDEF kernels (cdef.c) ---- */

uint8_t svt_aom_cdef_find_dir_c(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift);
uint8_t svt_aom_cdef_find_dir_8bit_c(const uint8_t* img, int32_t stride, int32_t* var, int32_t coeff_shift);
void    svt_cdef_filter_block_c(uint8_t* dst8, uint16_t* dst16, int32_t dstride, const uint16_t* in,
                                int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping,
                                int32_t sec_damping, int32_t bsize, int32_t coeff_shift,
                                uint8_t subsampling_factor);
void    svt_cdef_filter_block_8bit_c(uint8_t* dst, int32_t dstride, const uint8_t* in, int32_t pri_strength,
                                     int32_t sec_strength, int32_t dir, int32_t damping, int32_t bsize,
                                     int32_t coeff_shift, uint8_t subsampling_factor);

uint8_t ref_cdef_find_dir(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    return svt_aom_cdef_find_dir_c(img, stride, var, coeff_shift);
}

uint8_t ref_cdef_find_dir_8bit(const uint8_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    return svt_aom_cdef_find_dir_8bit_c(img, stride, var, coeff_shift);
}

/* dst8 arm only (8-bit pipeline); `in` points at the block origin inside the
   CDEF_BSTRIDE-padded buffer, exactly like svt_cdef_filter_fb passes it. */
void ref_cdef_filter_block_8(uint8_t* dst, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                             int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping,
                             int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor) {
    svt_cdef_filter_block_c(dst, NULL, dstride, in, pri_strength, sec_strength, dir, pri_damping, sec_damping,
                            bsize, coeff_shift, subsampling_factor);
}

void ref_cdef_filter_block_8bit(uint8_t* dst, int32_t dstride, const uint8_t* in, int32_t pri_strength,
                                int32_t sec_strength, int32_t dir, int32_t damping, int32_t bsize,
                                int32_t coeff_shift, uint8_t subsampling_factor) {
    svt_cdef_filter_block_8bit_c(dst, dstride, in, pri_strength, sec_strength, dir, damping, bsize, coeff_shift,
                                 subsampling_factor);
}

/* dst16 arm — the one the 10-bit (is_16bit) pipeline takes: cdef_seg_search
   passes `svt_cdef_filter_fb(NULL, tmp_dst, ...)` at is_16bit, which reaches
   svt_cdef_filter_block_c with dst8 == NULL. Same `in` layout as the dst8 arm
   above. */
void ref_cdef_filter_block_16(uint16_t* dst, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                              int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping,
                              int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor) {
    svt_cdef_filter_block_c(NULL, dst, dstride, in, pri_strength, sec_strength, dir, pri_damping, sec_damping,
                            bsize, coeff_shift, subsampling_factor);
}

/* ---- CDEF search distortion (enc_cdef.c) ----
   NOTE the C parameter naming is inverted relative to the call site: in
   cdef_seg_search `dst` receives the SOURCE picture (with the fb offset +
   the picture stride) and `src` receives the PACKED filtered blocks
   (tmp_dst). These shims keep C's parameter names so the signature matches
   the header, and the Rust side documents the roles. */
uint64_t svt_aom_compute_cdef_dist_16bit_c(const uint16_t* dst, int32_t dstride, const uint16_t* src,
                                           const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                           int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_8bit_c(const uint8_t* dst8, int32_t dstride, const uint8_t* src8,
                                          const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                          int32_t coeff_shift, uint8_t subsampling_factor);

/* dlist is passed as flat (by, bx) byte pairs so Rust need not know the
   CdefList layout; skip_cdef/… fields are unused by the dist kernels. */
static void ref_fill_dlist(CdefList* dl, const uint8_t* byx, int32_t n) {
    for (int32_t i = 0; i < n; i++) {
        memset(&dl[i], 0, sizeof(dl[i]));
        dl[i].by = byx[2 * i];
        dl[i].bx = byx[2 * i + 1];
    }
}

uint64_t ref_compute_cdef_dist_16bit(const uint16_t* plane, int32_t dstride, const uint16_t* packed,
                                     const uint8_t* byx, int32_t cdef_count, int32_t bsize, int32_t coeff_shift,
                                     uint8_t subsampling_factor) {
    CdefList dl[64 * 64];
    ref_fill_dlist(dl, byx, cdef_count);
    return svt_aom_compute_cdef_dist_16bit_c(
        plane, dstride, packed, dl, cdef_count, (BlockSize)bsize, coeff_shift, subsampling_factor);
}

uint64_t ref_compute_cdef_dist_8bit(const uint8_t* plane, int32_t dstride, const uint8_t* packed,
                                    const uint8_t* byx, int32_t cdef_count, int32_t bsize, int32_t coeff_shift,
                                    uint8_t subsampling_factor) {
    CdefList dl[64 * 64];
    ref_fill_dlist(dl, byx, cdef_count);
    return svt_aom_compute_cdef_dist_8bit_c(
        plane, dstride, packed, dl, cdef_count, (BlockSize)bsize, coeff_shift, subsampling_factor);
}

/* ---- CDEF strength-from-QP picker (svt_pick_cdef_from_qp is static; this
   replicates its intra branch verbatim from enc_cdef.c:849 against the
   REAL svt_aom_ac_quant_qtx, pinning the C float-expression semantics the
   Rust port must reproduce bit-exactly) ---- */

#include <math.h>

int16_t svt_aom_ac_quant_qtx(int32_t qindex, int32_t delta, EbBitDepth bit_depth);

void ref_pick_cdef_from_qp_intra(int32_t base_q_idx, int32_t bit_depth,
                                 int32_t* pred_y_strength, int32_t* pred_uv_strength) {
    /* svt_pick_cdef_from_qp (enc_cdef.c:829-830): bd-aware AC step
       normalized back to the 8-bit scale. bit_depth is the EbBitDepth enum
       value (8/10/12), passed straight to svt_aom_ac_quant_qtx. */
    int32_t q = svt_aom_ac_quant_qtx(base_q_idx, 0, bit_depth);
    q >>= (bit_depth - 8);

    /* enc_cdef.c:880-888, Intra branch, verbatim. */
    int32_t y_f1  = (int32_t)roundf(q * q * 0.0000033731974f + q * 0.008070594f + 0.0187634f);
    int32_t y_f2  = (int32_t)roundf(q * q * 0.0000029167343f + q * 0.0027798624f + 0.0079405f);
    int32_t uv_f1 = (int32_t)roundf(q * q * -0.0000130790995f + q * 0.012892405f - 0.00748388f);
    int32_t uv_f2 = (int32_t)roundf(q * q * 0.0000032651783f + q * 0.00035520183f + 0.00228092f);

    y_f1  = y_f1 < 0 ? 0 : (y_f1 > 15 ? 15 : y_f1);
    y_f2  = y_f2 < 0 ? 0 : (y_f2 > 3 ? 3 : y_f2);
    uv_f1 = uv_f1 < 0 ? 0 : (uv_f1 > 15 ? 15 : uv_f1);
    uv_f2 = uv_f2 < 0 ? 0 : (uv_f2 > 3 ? 3 : uv_f2);

    *pred_y_strength  = y_f1 * 4 + y_f2;
    *pred_uv_strength = uv_f1 * 4 + uv_f2;
}

/* ---- RD multiplier base (rc_process.c:365) ----
   The lambda every post-MD RD search is built from: `svt_aom_lambda_assign`
   calls `svt_aom_compute_rd_mult` -> `svt_aom_compute_rd_mult_based_on_qindex`
   then applies `rd_frame_type_factor[bit_depth != 8][update_type] >> 7`. This
   shim exposes the (real) qindex/bit-depth base so the Rust bd10 lambdas are
   differentially pinned rather than hand-transcribed. SVT_AV1_KF_UPDATE = 0. */
int svt_aom_compute_rd_mult_based_on_qindex(EbBitDepth bit_depth, SvtAv1FrameUpdateType update_type, int qindex);

int32_t ref_compute_rd_mult_based_on_qindex(int32_t bit_depth, int32_t update_type, int32_t qindex) {
    return svt_aom_compute_rd_mult_based_on_qindex(
        (EbBitDepth)bit_depth, (SvtAv1FrameUpdateType)update_type, qindex);
}

/* ---- Loop restoration (Wiener): kernel, stats, filter_unit, subexp coding ---- */

#include "restoration.h"
#include "convolve.h"

static void ref_rtcd_once(void) {
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
}

void svt_av1_wiener_convolve_add_src_c(const uint8_t* const src, const ptrdiff_t src_stride, uint8_t* const dst,
                                       const ptrdiff_t dst_stride, const int16_t* const filter_x,
                                       const int16_t* const filter_y, const int32_t w, const int32_t h,
                                       const ConvolveParams* const conv_params);
void svt_av1_compute_stats_c(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start,
                             int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride,
                             int64_t* M, int64_t* H);

/* src/dst point at the block origin (borders live around them, caller's
   responsibility). Filters are copied into 16-aligned storage so the C
   entry's InterpKernel base/offset arithmetic behaves exactly as with the
   real WienerInfo (DECLARE_ALIGNED(16, ...)). */
void ref_wiener_convolve_add_src(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                                 const int16_t* filter_x, const int16_t* filter_y, int32_t w, int32_t h) {
    ref_rtcd_once();
    _Alignas(16) int16_t fx[8];
    _Alignas(16) int16_t fy[8];
    memcpy(fx, filter_x, sizeof(fx));
    memcpy(fy, filter_y, sizeof(fy));
    const ConvolveParams conv_params = get_conv_params_wiener(8);
    svt_av1_wiener_convolve_add_src_c(src, src_stride, dst, dst_stride, fx, fy, w, h, &conv_params);
}

void ref_compute_stats(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start, int32_t h_end,
                       int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t* m,
                       int64_t* h) {
    svt_av1_compute_stats_c(wiener_win, dgd, src, h_start, h_end, v_start, v_end, dgd_stride, src_stride, m, h);
}

/* Full per-unit stripe machinery. data/dst point at plane (0,0) of padded
   planes (>=4px physical margins); boundary buffers follow the C layout
   (col i == plane col i-4, row 2*stripe + j). */
void ref_loop_restoration_filter_unit(uint8_t need_boundaries, int32_t h_start, int32_t h_end, int32_t v_start,
                                      int32_t v_end, int32_t rtype, const int16_t* vfilter, const int16_t* hfilter,
                                      const uint8_t* bdry_above, const uint8_t* bdry_below, int32_t bdry_stride,
                                      int32_t tile_left, int32_t tile_top, int32_t tile_right, int32_t tile_bottom,
                                      int32_t tile_stripe0, int32_t ss_x, int32_t ss_y, uint8_t* data,
                                      int32_t stride, uint8_t* dst, int32_t dst_stride) {
    ref_rtcd_once();
    RestorationTileLimits limits = {h_start, h_end, v_start, v_end};
    Av1PixelRect          rect   = {tile_left, tile_top, tile_right, tile_bottom};
    RestorationUnitInfo   rui;
    memset(&rui, 0, sizeof(rui));
    rui.restoration_type = (RestorationType)rtype;
    memcpy(rui.wiener_info.vfilter, vfilter, 8 * sizeof(int16_t));
    memcpy(rui.wiener_info.hfilter, hfilter, 8 * sizeof(int16_t));
    RestorationStripeBoundaries rsb;
    rsb.stripe_boundary_above  = (uint8_t*)bdry_above;
    rsb.stripe_boundary_below  = (uint8_t*)bdry_below;
    rsb.stripe_boundary_stride = bdry_stride;
    rsb.stripe_boundary_size   = 0;
    static RestorationLineBuffers rlbs; /* large; single-threaded tests */
    svt_av1_loop_restoration_filter_unit(need_boundaries, &limits, &rui, &rsb, &rlbs, &rect, tile_stripe0, ss_x,
                                         ss_y, /*highbd*/ 0, /*bit_depth*/ 8, data, stride, dst, dst_stride,
                                         /*tmpbuf*/ NULL, /*optimized_lr*/ 0);
}

void ref_extend_frame(uint8_t* data, int32_t width, int32_t height, int32_t stride, int32_t border_horz,
                      int32_t border_vert) {
    ref_rtcd_once();
    svt_extend_frame(data, width, height, stride, border_horz, border_vert, /*highbd*/ 0);
}

/* ---- Loop restoration, HIGHBD arm (the is_16bit / 10-bit pipeline) ----
   Every highbd entry below takes a real uint16_t* from Rust and hands C the
   CONVERT_TO_BYTEPTR form (definitions.h:1020, the libaom pointer>>1 trick)
   that the C signatures expect. */

void svt_av1_highbd_wiener_convolve_add_src_c(const uint8_t* const src, const ptrdiff_t src_stride,
                                              uint8_t* const dst, const ptrdiff_t dst_stride,
                                              const int16_t* const filter_x, const int16_t* const filter_y,
                                              const int32_t w, const int32_t h,
                                              const ConvolveParams* const conv_params, const int32_t bd);
void svt_av1_compute_stats_highbd_c(int32_t wiener_win, const uint8_t* dgd8, const uint8_t* src8, int32_t h_start,
                                    int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                                    int32_t src_stride, int64_t* M, int64_t* H, EbBitDepth bit_depth);
int64_t svt_aom_highbd_get_sse(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, int width,
                               int height);

void ref_highbd_wiener_convolve_add_src(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                        const int16_t* filter_x, const int16_t* filter_y, int32_t w, int32_t h,
                                        int32_t bd) {
    ref_rtcd_once();
    _Alignas(16) int16_t fx[8];
    _Alignas(16) int16_t fy[8];
    memcpy(fx, filter_x, sizeof(fx));
    memcpy(fy, filter_y, sizeof(fy));
    const ConvolveParams conv_params = get_conv_params_wiener(bd);
    svt_av1_highbd_wiener_convolve_add_src_c(CONVERT_TO_BYTEPTR(src),
                                             src_stride,
                                             CONVERT_TO_BYTEPTR(dst),
                                             dst_stride,
                                             fx,
                                             fy,
                                             w,
                                             h,
                                             &conv_params,
                                             bd);
}

void ref_compute_stats_highbd(int32_t wiener_win, const uint16_t* dgd, const uint16_t* src, int32_t h_start,
                              int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                              int32_t src_stride, int64_t* m, int64_t* h, int32_t bit_depth) {
    svt_av1_compute_stats_highbd_c(wiener_win,
                                   CONVERT_TO_BYTEPTR(dgd),
                                   CONVERT_TO_BYTEPTR(src),
                                   h_start,
                                   h_end,
                                   v_start,
                                   v_end,
                                   dgd_stride,
                                   src_stride,
                                   m,
                                   h,
                                   (EbBitDepth)bit_depth);
}

/* `svt_aom_highbd_get_sse` calls the rtcd pointer `svt_aom_highbd_mse16x16`,
   which lives in the ENCODER dsp table (aom_dsp_rtcd.c:269), not the common
   one `ref_rtcd_once` initializes — without this it is NULL and the call
   segfaults. Defined further down; forward-declared here. */
static void ref_dsp_rtcd_once(void);

int64_t ref_highbd_get_sse(const uint16_t* a, int32_t a_stride, const uint16_t* b, int32_t b_stride, int32_t width,
                           int32_t height) {
    ref_dsp_rtcd_once();
    return svt_aom_highbd_get_sse(CONVERT_TO_BYTEPTR(a), a_stride, CONVERT_TO_BYTEPTR(b), b_stride, width, height);
}

void ref_extend_frame_highbd(uint16_t* data, int32_t width, int32_t height, int32_t stride, int32_t border_horz,
                             int32_t border_vert) {
    ref_rtcd_once();
    svt_extend_frame(CONVERT_TO_BYTEPTR(data), width, height, stride, border_horz, border_vert, /*highbd*/ 1);
}

/* Search-path unit filter: need_boundaries = 0 (use_boundaries_in_rest_search
   = 0, enc_handle.c:4483), so the stripe-boundary save/restore never runs and
   the boundary buffers are unused. */
void ref_loop_restoration_filter_unit_highbd(int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end,
                                             int32_t rtype, const int16_t* vfilter, const int16_t* hfilter,
                                             int32_t tile_left, int32_t tile_top, int32_t tile_right,
                                             int32_t tile_bottom, int32_t tile_stripe0, int32_t ss_x, int32_t ss_y,
                                             int32_t bit_depth, uint16_t* data, int32_t stride, uint16_t* dst,
                                             int32_t dst_stride) {
    ref_rtcd_once();
    RestorationTileLimits limits = {h_start, h_end, v_start, v_end};
    Av1PixelRect          rect   = {tile_left, tile_top, tile_right, tile_bottom};
    RestorationUnitInfo   rui;
    memset(&rui, 0, sizeof(rui));
    rui.restoration_type = (RestorationType)rtype;
    memcpy(rui.wiener_info.vfilter, vfilter, 8 * sizeof(int16_t));
    memcpy(rui.wiener_info.hfilter, hfilter, 8 * sizeof(int16_t));
    RestorationStripeBoundaries rsb;
    memset(&rsb, 0, sizeof(rsb));
    static RestorationLineBuffers rlbs; /* large; single-threaded tests */
    svt_av1_loop_restoration_filter_unit(/*need_boundaries*/ 0,
                                         &limits,
                                         &rui,
                                         &rsb,
                                         &rlbs,
                                         &rect,
                                         tile_stripe0,
                                         ss_x,
                                         ss_y,
                                         /*highbd*/ 1,
                                         bit_depth,
                                         CONVERT_TO_BYTEPTR(data),
                                         stride,
                                         CONVERT_TO_BYTEPTR(dst),
                                         dst_stride,
                                         /*tmpbuf*/ NULL,
                                         /*optimized_lr*/ 0);
}

/* Subexp-with-reference bit chain (tap coding). Returns the coded byte
   stream via the od_ec coder, so Rust can byte-compare its port. */
void    svt_aom_write_primitive_refsubexpfin(AomWriter* w, uint16_t n, uint16_t k, uint16_t ref, uint16_t v);
int32_t svt_aom_count_primitive_refsubexpfin(uint16_t n, uint16_t k, uint16_t ref, uint16_t v);

uint32_t ref_write_refsubexpfin_bytes(uint16_t n, uint16_t k, uint16_t ref, uint16_t v, uint8_t* out,
                                      uint32_t cap) {
    AomWriter w;
    memset(&w, 0, sizeof(w));
    w.allow_update_cdf = 1;
    svt_od_ec_enc_init(&w.ec);
    w.ec.buf = (unsigned char*)malloc(REF_EC_BUF_CAP);
    svt_od_ec_enc_reset(&w.ec);
    svt_aom_write_primitive_refsubexpfin(&w, n, k, ref, v);
    uint32_t       nbytes;
    const uint8_t* p = svt_od_ec_enc_done(&w.ec, &nbytes);
    if (nbytes > cap) {
        nbytes = cap;
    }
    memcpy(out, p, nbytes);
    free(w.ec.buf);
    return nbytes;
}

int32_t ref_count_refsubexpfin(uint16_t n, uint16_t k, uint16_t ref, uint16_t v) {
    return svt_aom_count_primitive_refsubexpfin(n, k, ref, v);
}

/* ---- Intra edge filter (svt_memcpy is an rtcd pointer: init first) ---- */
void svt_av1_filter_intra_edge_c(uint8_t* p, int32_t sz, int32_t strength);
void ref_filter_intra_edge(uint8_t* p, int32_t sz, int32_t strength) {
    ref_rtcd_once();
    svt_av1_filter_intra_edge_c(p, sz, strength);
}

/* ===========================================================================
 * AUDIT 2026-07-14 (wave2/entropy-c-parity): inter / motion DSP oracles.
 * The `_c` scalar reference is ground truth for the pre-v4.2-bump Rust ports
 * in svtav1-dsp {sad, variance, inter_pred, obmc, warp, scale, superres}.
 * v4.2_functions.md shows NONE of these C files changed 4.1->4.2, so any
 * divergence a c_parity_* test finds is a pre-existing port bug, not drift.
 * =========================================================================== */

#include "aom_dsp_rtcd.h"

/* ---- SAD: svt_aom_sad{W}x{H}_c (compute_sad_c.c) ---- */
uint32_t ref_sad(int32_t w, int32_t h, const uint8_t* src, int32_t ss, const uint8_t* ref, int32_t rs) {
    switch (w * 1000 + h) {
    case 4004:     return svt_aom_sad4x4_c(src, ss, ref, rs);
    case 4008:     return svt_aom_sad4x8_c(src, ss, ref, rs);
    case 4016:     return svt_aom_sad4x16_c(src, ss, ref, rs);
    case 8004:     return svt_aom_sad8x4_c(src, ss, ref, rs);
    case 8008:     return svt_aom_sad8x8_c(src, ss, ref, rs);
    case 8016:     return svt_aom_sad8x16_c(src, ss, ref, rs);
    case 8032:     return svt_aom_sad8x32_c(src, ss, ref, rs);
    case 16004:    return svt_aom_sad16x4_c(src, ss, ref, rs);
    case 16008:    return svt_aom_sad16x8_c(src, ss, ref, rs);
    case 16016:    return svt_aom_sad16x16_c(src, ss, ref, rs);
    case 16032:    return svt_aom_sad16x32_c(src, ss, ref, rs);
    case 16064:    return svt_aom_sad16x64_c(src, ss, ref, rs);
    case 32008:    return svt_aom_sad32x8_c(src, ss, ref, rs);
    case 32016:    return svt_aom_sad32x16_c(src, ss, ref, rs);
    case 32032:    return svt_aom_sad32x32_c(src, ss, ref, rs);
    case 32064:    return svt_aom_sad32x64_c(src, ss, ref, rs);
    case 64016:    return svt_aom_sad64x16_c(src, ss, ref, rs);
    case 64032:    return svt_aom_sad64x32_c(src, ss, ref, rs);
    case 64064:    return svt_aom_sad64x64_c(src, ss, ref, rs);
    case 64128:    return svt_aom_sad64x128_c(src, ss, ref, rs);
    case 128064:   return svt_aom_sad128x64_c(src, ss, ref, rs);
    case 128128:   return svt_aom_sad128x128_c(src, ss, ref, rs);
    default:       return 0xFFFFFFFFu; /* unsupported size sentinel */
    }
}

/* ---- Variance: svt_aom_variance{W}x{H}_c (variance.c).
   Two-block: outputs *sse = sum((a-b)^2), returns *sse - sum(a-b)^2 / (W*H). ---- */
uint32_t ref_variance(int32_t w, int32_t h, const uint8_t* a, int32_t as, const uint8_t* b, int32_t bs,
                      uint32_t* sse) {
    *sse = 0xFFFFFFFFu;
    switch (w * 1000 + h) {
    case 4004:     return svt_aom_variance4x4_c(a, as, b, bs, sse);
    case 4008:     return svt_aom_variance4x8_c(a, as, b, bs, sse);
    case 4016:     return svt_aom_variance4x16_c(a, as, b, bs, sse);
    case 8004:     return svt_aom_variance8x4_c(a, as, b, bs, sse);
    case 8008:     return svt_aom_variance8x8_c(a, as, b, bs, sse);
    case 8016:     return svt_aom_variance8x16_c(a, as, b, bs, sse);
    case 8032:     return svt_aom_variance8x32_c(a, as, b, bs, sse);
    case 16004:    return svt_aom_variance16x4_c(a, as, b, bs, sse);
    case 16008:    return svt_aom_variance16x8_c(a, as, b, bs, sse);
    case 16016:    return svt_aom_variance16x16_c(a, as, b, bs, sse);
    case 16032:    return svt_aom_variance16x32_c(a, as, b, bs, sse);
    case 16064:    return svt_aom_variance16x64_c(a, as, b, bs, sse);
    case 32008:    return svt_aom_variance32x8_c(a, as, b, bs, sse);
    case 32016:    return svt_aom_variance32x16_c(a, as, b, bs, sse);
    case 32032:    return svt_aom_variance32x32_c(a, as, b, bs, sse);
    case 32064:    return svt_aom_variance32x64_c(a, as, b, bs, sse);
    case 64016:    return svt_aom_variance64x16_c(a, as, b, bs, sse);
    case 64032:    return svt_aom_variance64x32_c(a, as, b, bs, sse);
    case 64064:    return svt_aom_variance64x64_c(a, as, b, bs, sse);
    case 64128:    return svt_aom_variance64x128_c(a, as, b, bs, sse);
    case 128064:   return svt_aom_variance128x64_c(a, as, b, bs, sse);
    case 128128:   return svt_aom_variance128x128_c(a, as, b, bs, sse);
    default:       return 0xFFFFFFFFu;
    }
}

/* ---- convolve8 single-pass horiz / vert (convolve.c).
   svt_aom_convolve8_horiz_c recovers the 16-phase InterpKernel base from the
   filter pointer via (ptr & ~0xFF) [get_filter_base] and its offset. We stage
   the caller's 8 taps into every row of a 256-byte-aligned table so, whichever
   offset it computes, the applied taps are the caller's, x_step_q4=16 => 1px
   per output. The kernel does src-=3 internally, sum of 8 taps, then
   clip_pixel(ROUND_POWER_OF_TWO(sum,7)) -- what our Rust convolve_{horiz,vert}
   does on a caller-offset slice. ---- */
void svt_aom_convolve8_horiz_c(const uint8_t* src, ptrdiff_t src_stride, uint8_t* dst, ptrdiff_t dst_stride,
                               const int16_t* filter_x, int x_step_q4, const int16_t* filter_y, int y_step_q4,
                               int w, int h);
void svt_aom_convolve8_vert_c(const uint8_t* src, ptrdiff_t src_stride, uint8_t* dst, ptrdiff_t dst_stride,
                              const int16_t* filter_x, int x_step_q4, const int16_t* filter_y, int y_step_q4, int w,
                              int h);

void ref_convolve8_horiz(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                         const int16_t* taps, int32_t w, int32_t h) {
    _Alignas(256) int16_t table[16][8];
    for (int p = 0; p < 16; ++p) memcpy(table[p], taps, 8 * sizeof(int16_t));
    svt_aom_convolve8_horiz_c(src, src_stride, dst, dst_stride, table[0], 16, NULL, -1, w, h);
}

void ref_convolve8_vert(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                        const int16_t* taps, int32_t w, int32_t h) {
    _Alignas(256) int16_t table[16][8];
    for (int p = 0; p < 16; ++p) memcpy(table[p], taps, 8 * sizeof(int16_t));
    svt_aom_convolve8_vert_c(src, src_stride, dst, dst_stride, NULL, -1, table[0], 16, w, h);
}

/* ---- OBMC blend (enc_inter_prediction.c build_obmc_inter_pred_{above,left}):
   the reconstruction-side blend combines the current prediction (src0) with a
   neighbor prediction (src1) using svt_av1_get_obmc_mask(overlap) via
   svt_aom_blend_a64_{v,h}mask_c. AOM_BLEND_A64(m, v0, v1)=((m*v0+(64-m)*v1)+32)>>6.
   'above' blends the top `overlap` rows (mask by row, vmask); 'left' blends the
   left `overlap` cols (mask by col, hmask). ---- */
const uint8_t* svt_av1_get_obmc_mask(int length);
void svt_aom_blend_a64_vmask_c(uint8_t* dst, uint32_t dst_stride, const uint8_t* src0, uint32_t src0_stride,
                               const uint8_t* src1, uint32_t src1_stride, const uint8_t* mask, int w, int h);
void svt_aom_blend_a64_hmask_c(uint8_t* dst, uint32_t dst_stride, const uint8_t* src0, uint32_t src0_stride,
                               const uint8_t* src1, uint32_t src1_stride, const uint8_t* mask, int w, int h);

void ref_obmc_mask(int32_t length, uint8_t* out) {
    const uint8_t* m = svt_av1_get_obmc_mask(length);
    memcpy(out, m, (size_t)length);
}

void ref_obmc_blend_above(uint8_t* dst, int32_t dst_stride, const uint8_t* above, int32_t above_stride, int32_t w,
                          int32_t overlap) {
    const uint8_t* mask = svt_av1_get_obmc_mask(overlap);
    svt_aom_blend_a64_vmask_c(dst, dst_stride, dst, dst_stride, above, above_stride, mask, w, overlap);
}

void ref_obmc_blend_left(uint8_t* dst, int32_t dst_stride, const uint8_t* left, int32_t left_stride, int32_t overlap,
                         int32_t h) {
    const uint8_t* mask = svt_av1_get_obmc_mask(overlap);
    svt_aom_blend_a64_hmask_c(dst, dst_stride, dst, dst_stride, left, left_stride, mask, overlap, h);
}

/* ---- Warp affine: svt_av1_warp_affine_c (warped_motion.c). The real warp
   uses svt_aom_warped_filter (WARPEDPIXEL_PREC_SHIFTS=64, ~193 phases),
   alpha/beta/gamma/delta shear, conv_params ROUND0/ROUND1 and 8x8 tiling. The
   Rust warp.rs is a homegrown 16-phase SUB_PEL approximation (stub); this shim
   is the oracle a real port must match. Non-compound 8-bit conv params. ---- */
void svt_av1_warp_affine_c(const int32_t* mat, const uint8_t* ref, int width, int height, int stride, uint8_t* pred,
                           int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x,
                           int subsampling_y, ConvolveParams* conv_params, int16_t alpha, int16_t beta, int16_t gamma,
                           int16_t delta);

void ref_warp_affine(const int32_t* mat, const uint8_t* ref, int32_t width, int32_t height, int32_t stride,
                     uint8_t* pred, int32_t p_col, int32_t p_row, int32_t p_width, int32_t p_height, int32_t p_stride,
                     int16_t alpha, int16_t beta, int16_t gamma, int16_t delta) {
    ConvolveParams cp = get_conv_params(0, 8); /* non-compound, 8-bit (v4.2.0-final 2-arg signature) */
    svt_av1_warp_affine_c(mat, ref, width, height, stride, pred, p_col, p_row, p_width, p_height, p_stride, 0, 0, &cp,
                          alpha, beta, gamma, delta);
}

/* ---- Scaled 2D convolve: svt_av1_convolve_2d_scale_c (inter_prediction.c).
   Real scaled inter prediction: SCALE_SUBPEL_BITS=10 phase domain, EIGHTTAP
   InterpFilterParams, ROUND0/ROUND1 + 16-bit intermediate. scale.rs is a naive
   16-phase separable stub. Non-compound 8-bit; EIGHTTAP_REGULAR both axes. ---- */
#include "inter_prediction.h"
void svt_av1_convolve_2d_scale_c(const uint8_t* src, int src_stride, uint8_t* dst8, int dst8_stride, int w, int h,
                                 const InterpFilterParams* fx, const InterpFilterParams* fy, int subpel_x_qn,
                                 int x_step_qn, int subpel_y_qn, int y_step_qn, ConvolveParams* conv_params);

void ref_convolve_2d_scale(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                           int32_t h, int32_t subpel_x_qn, int32_t x_step_qn, int32_t subpel_y_qn, int32_t y_step_qn) {
    const InterpFilterParams* fx = &av1_interp_filter_params_list[EIGHTTAP_REGULAR];
    const InterpFilterParams* fy = &av1_interp_filter_params_list[EIGHTTAP_REGULAR];
    ConvolveParams            cp = get_conv_params(0, 8); /* v4.2.0-final 2-arg signature */
    svt_av1_convolve_2d_scale_c(src, src_stride, dst, dst_stride, w, h, fx, fy, subpel_x_qn, x_step_qn, subpel_y_qn,
                                y_step_qn, &cp);
}

/* ---- Superres normative upscale: upscale_normative_rect ->
   av1_convolve_horiz_rs_c (super_res.c), svt_av1_resize_filter_normative
   (RS_SUBPEL_BITS=6 -> 64 phases, RS_SCALE_SUBPEL_BITS=14 phase accumulator).
   superres.rs uses a 16-phase table and Q14/4-bit-phase math (stub). ---- */
#include "super_res.h"
void upscale_normative_rect(const uint8_t* input, int height, int width, int in_stride, uint8_t* output, int height2,
                            int width2, int out_stride, int x_step_qn, int x0_qn, int pad_left, int pad_right);

/* Copy one 8-tap phase of the C normative filter (phase in 0..63). */
void ref_superres_filter_normative(int32_t phase, int16_t* out8) {
    memcpy(out8, &svt_av1_resize_filter_normative[phase][0], 8 * sizeof(int16_t));
}

/* av1_get_upscale_convolve_step / get_upscale_convolve_x0 are static; replicate
   verbatim so the oracle scales exactly like svt_av1_upscale_normative_rows. */
static int32_t ref_us_step(int32_t in_len, int32_t out_len) {
    return ((in_len << RS_SCALE_SUBPEL_BITS) + out_len / 2) / out_len;
}
static int32_t ref_us_x0(int32_t in_len, int32_t out_len, int32_t x_step_qn) {
    const int     err = out_len * x_step_qn - (in_len << RS_SCALE_SUBPEL_BITS);
    const int32_t x0  = (-((out_len - in_len) << (RS_SCALE_SUBPEL_BITS - 1)) + out_len / 2) / out_len +
        RS_SCALE_EXTRA_OFF - err / 2;
    return (int32_t)((uint32_t)x0 & RS_SCALE_SUBPEL_MASK);
}

/* One-row normative horizontal upscale (in_width -> out_width). `input` must
   carry >= 5 border bytes each side (rect saves/replicates/restores them). */
void ref_superres_upscale_row(const uint8_t* input, int32_t in_width, uint8_t* output, int32_t out_width) {
    ref_rtcd_once(); /* svt_memcpy in the border save path */
    const int32_t step = ref_us_step(in_width, out_width);
    const int32_t x0   = ref_us_x0(in_width, out_width, step);
    upscale_normative_rect((uint8_t*)input, 1, in_width, in_width, output, 1, out_width, out_width, step, x0, 1, 1);
}

/* ---- Quantizers (full_loop.c): the MD + encode-pass quantize kernels ----
 *
 * `svt_av1_quantize_fp_facade` (full_loop.c:462) and the `perform_rdoq == 0`
 * arm of `svt_aom_quantize_inv_quantize` (:1785) are what actually turn
 * transform coefficients into (qcoeff, dqcoeff, eob) for every MD candidate.
 * They dispatch through RTCD, so a real encode runs the AVX2 kernel — these
 * shims expose BOTH that pointer (`dispatch = 1`) and the scalar `_c`
 * reference (`dispatch = 0`) so the port can be pinned against the kernel the
 * encoder really calls, and the two C paths can be cross-checked. QM is off on
 * the allintra path, so qm/iqm are NULL (the facade's non-qm branch).
 */

#include "aom_dsp_rtcd.h"

void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
static int g_dsp_rtcd_ready = 0;
static void ref_dsp_rtcd_once(void) {
    if (!g_dsp_rtcd_ready) {
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_dsp_rtcd_ready = 1;
    }
}

uint16_t ref_quantize_fp(const int32_t* coeff, intptr_t n_coeffs, const int16_t* zbin, const int16_t* round_fp,
                         const int16_t* quant_fp, const int16_t* quant_shift, int32_t* qcoeff, int32_t* dqcoeff,
                         const int16_t* dequant, const int16_t* scan, const int16_t* iscan, int32_t log_scale,
                         int32_t dispatch) {
    uint16_t eob = 0;
    if (dispatch) {
        ref_dsp_rtcd_once();
        switch (log_scale) {
        case 0:
            svt_av1_quantize_fp(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        case 1:
            svt_av1_quantize_fp_32x32(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        default:
            svt_av1_quantize_fp_64x64(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        }
    } else {
        switch (log_scale) {
        case 0:
            svt_av1_quantize_fp_c(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        case 1:
            svt_av1_quantize_fp_32x32_c(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        default:
            svt_av1_quantize_fp_64x64_c(
                coeff, n_coeffs, zbin, round_fp, quant_fp, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan);
            break;
        }
    }
    return eob;
}

/* `svt_aom_quantize_b_avx2` reads the coefficients with `_mm256_load_si256`
   (highbd_quantize_intrin_avx2.c:227/:242) — the ALIGNED load. The real
   encoder satisfies that because every coefficient buffer is
   EB_MALLOC_ALIGNED/DECLARE_ALIGNED; a plain Rust `Vec<i32>` is only 4-byte
   aligned and faults. Stage through 32-byte-aligned buffers so the shim
   reproduces the caller contract the library assumes rather than a weaker one.
   Max adjusted coefficient count is av1_get_max_eob(TX_64X64) = 1024. */
#define REF_QUANT_MAX_COEFFS 1024

uint16_t ref_quantize_b(const int32_t* coeff, intptr_t n_coeffs, const int16_t* zbin, const int16_t* round,
                        const int16_t* quant, const int16_t* quant_shift, int32_t* qcoeff, int32_t* dqcoeff,
                        const int16_t* dequant, const int16_t* scan, const int16_t* iscan, int32_t log_scale,
                        int32_t dispatch) {
    uint16_t eob = 0;
    if (n_coeffs > REF_QUANT_MAX_COEFFS) {
        abort();
    }
    _Alignas(32) int32_t a_coeff[REF_QUANT_MAX_COEFFS];
    _Alignas(32) int32_t a_qcoeff[REF_QUANT_MAX_COEFFS];
    _Alignas(32) int32_t a_dqcoeff[REF_QUANT_MAX_COEFFS];
    _Alignas(32) int16_t a_iscan[REF_QUANT_MAX_COEFFS];
    _Alignas(32) int16_t a_scan[REF_QUANT_MAX_COEFFS];
    memcpy(a_coeff, coeff, (size_t)n_coeffs * sizeof(int32_t));
    memcpy(a_iscan, iscan, (size_t)n_coeffs * sizeof(int16_t));
    memcpy(a_scan, scan, (size_t)n_coeffs * sizeof(int16_t));
    memset(a_qcoeff, 0, (size_t)n_coeffs * sizeof(int32_t));
    memset(a_dqcoeff, 0, (size_t)n_coeffs * sizeof(int32_t));

    if (dispatch) {
        ref_dsp_rtcd_once();
        svt_aom_quantize_b(a_coeff,
                           n_coeffs,
                           zbin,
                           round,
                           quant,
                           quant_shift,
                           a_qcoeff,
                           a_dqcoeff,
                           dequant,
                           &eob,
                           a_scan,
                           a_iscan,
                           NULL,
                           NULL,
                           log_scale);
    } else {
        svt_aom_quantize_b_c(a_coeff,
                             n_coeffs,
                             zbin,
                             round,
                             quant,
                             quant_shift,
                             a_qcoeff,
                             a_dqcoeff,
                             dequant,
                             &eob,
                             a_scan,
                             a_iscan,
                             NULL,
                             NULL,
                             log_scale);
    }
    memcpy(qcoeff, a_qcoeff, (size_t)n_coeffs * sizeof(int32_t));
    memcpy(dqcoeff, a_dqcoeff, (size_t)n_coeffs * sizeof(int32_t));
    return eob;
}

/* ---- tune-SSIM MD distortion kernel (mode_decision.c:4430) ----
   Exported symbol; internally routes svt_ssim_8x8/4x4 + svt_psy_distortion
   through BOTH RTCD tables, so init them first (ac-bias shim pattern). */
uint64_t svt_spatial_full_distortion_ssim_kernel(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                 uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                 uint32_t area_width, uint32_t area_height, bool hbd, double ac_bias);

uint64_t ref_spatial_full_distortion_ssim(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                          uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                          uint32_t area_width, uint32_t area_height, double ac_bias) {
    /* ssim tiles + psy hadamard live in COMMON dsp rtcd; satd in aom_dsp
       rtcd — init both (same pattern as ref_psy_distortion). */
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    ref_dsp_rtcd_once();
    return svt_spatial_full_distortion_ssim_kernel(
        input, input_offset, input_stride, recon, recon_offset, recon_stride, area_width, area_height, false, ac_bias);
}

/* ---- Photon-noise film-grain table generation (noise_generation.c) ----
   Drives the exported svt_av1_generate_noise_table with a real (zeroed)
   EbSvtAv1EncConfiguration, then flattens the returned AomFilmGrain into an
   int32 buffer for the Rust differential. The shim includes the real API
   header, so struct layout/ABI is the library's own. */
#include "EbSvtAv1Enc.h"

EbErrorType svt_av1_generate_noise_table(EbSvtAv1EncConfiguration* config);

/* out layout (all int32): apply_grain, num_y_points, y[14][2],
   chroma_scaling_from_luma, num_cb, cb[10][2], num_cr, cr[10][2],
   scaling_shift, ar_coeff_lag, ar_y[24], ar_cb[25], ar_cr[25],
   ar_coeff_shift, grain_scale_shift, cb_mult, cb_luma_mult, cb_offset,
   cr_mult, cr_luma_mult, cr_offset, overlap_flag, clip_to_restricted_range
   = 1+1+28+1+1+20+1+20+1+1+24+25+25+1+1+3+3+1+1 = 159 entries. */
int32_t ref_generate_noise_table(uint32_t width, uint32_t height, uint32_t noise_strength,
                                 int32_t noise_strength_chroma, int32_t noise_chroma_from_luma,
                                 int32_t noise_size, int32_t color_range_provided, int32_t color_range,
                                 int32_t avif, int32_t* out) {
    EbSvtAv1EncConfiguration cfg;
    memset(&cfg, 0, sizeof(cfg));
    cfg.source_width           = width;
    cfg.source_height          = height;
    cfg.noise_strength         = noise_strength;
    cfg.noise_strength_chroma  = noise_strength_chroma;
    cfg.noise_chroma_from_luma = (uint8_t)noise_chroma_from_luma;
    cfg.noise_size             = (int8_t)noise_size;
    cfg.color_range_provided   = color_range_provided;
    cfg.color_range            = (EbColorRange)color_range;
    cfg.avif                   = (bool)avif;
    if (svt_av1_generate_noise_table(&cfg) != EB_ErrorNone || !cfg.fgs_table) {
        return -1;
    }
    AomFilmGrain* fg = cfg.fgs_table;
    int32_t*      o  = out;
    *o++ = fg->apply_grain;
    *o++ = fg->num_y_points;
    for (int i = 0; i < 14; i++) { *o++ = fg->scaling_points_y[i][0]; *o++ = fg->scaling_points_y[i][1]; }
    *o++ = fg->chroma_scaling_from_luma;
    *o++ = fg->num_cb_points;
    for (int i = 0; i < 10; i++) { *o++ = fg->scaling_points_cb[i][0]; *o++ = fg->scaling_points_cb[i][1]; }
    *o++ = fg->num_cr_points;
    for (int i = 0; i < 10; i++) { *o++ = fg->scaling_points_cr[i][0]; *o++ = fg->scaling_points_cr[i][1]; }
    *o++ = fg->scaling_shift;
    *o++ = fg->ar_coeff_lag;
    for (int i = 0; i < 24; i++) { *o++ = fg->ar_coeffs_y[i]; }
    for (int i = 0; i < 25; i++) { *o++ = fg->ar_coeffs_cb[i]; }
    for (int i = 0; i < 25; i++) { *o++ = fg->ar_coeffs_cr[i]; }
    *o++ = fg->ar_coeff_shift;
    *o++ = fg->grain_scale_shift;
    *o++ = fg->cb_mult;
    *o++ = fg->cb_luma_mult;
    *o++ = fg->cb_offset;
    *o++ = fg->cr_mult;
    *o++ = fg->cr_luma_mult;
    *o++ = fg->cr_offset;
    *o++ = fg->overlap_flag;
    *o++ = fg->clip_to_restricted_range;
    free(fg);
    return (int32_t)(o - out);
}

/* ---- QM quantize kernels (full_loop.c QM branches) ----
   Direct pass-through to the exported scalar kernels with non-NULL qm/iqm
   pointers, validating BOTH the Rust QM kernels and the transcribed
   wt/iwt_matrix_ref tables the test feeds from the Rust side. */

void svt_av1_quantize_fp_qm_c(const int32_t* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                              const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                              int32_t* qcoeff_ptr, int32_t* dqcoeff_ptr, const int16_t* dequant_ptr,
                              uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, const uint8_t* qm_ptr,
                              const uint8_t* iqm_ptr, int16_t log_scale);

uint16_t ref_quantize_b_qm(const int32_t* coeff, intptr_t n_coeffs, const int16_t* zbin, const int16_t* round,
                           const int16_t* quant, const int16_t* quant_shift, int32_t* qcoeff, int32_t* dqcoeff,
                           const int16_t* dequant, const int16_t* scan, const int16_t* iscan, const uint8_t* qm,
                           const uint8_t* iqm, int32_t log_scale) {
    uint16_t eob = 0;
    svt_aom_quantize_b_c(
        coeff, n_coeffs, zbin, round, quant, quant_shift, qcoeff, dqcoeff, dequant, &eob, scan, iscan, qm, iqm, log_scale);
    return eob;
}

uint16_t ref_quantize_fp_qm(const int32_t* coeff, intptr_t n_coeffs, const int16_t* zbin, const int16_t* round_fp,
                            const int16_t* quant_fp, const int16_t* quant_shift, int32_t* qcoeff, int32_t* dqcoeff,
                            const int16_t* dequant, const int16_t* scan, const int16_t* iscan, const uint8_t* qm,
                            const uint8_t* iqm, int32_t log_scale) {
    uint16_t eob = 0;
    svt_av1_quantize_fp_qm_c(coeff,
                             n_coeffs,
                             zbin,
                             round_fp,
                             quant_fp,
                             quant_shift,
                             qcoeff,
                             dqcoeff,
                             dequant,
                             &eob,
                             scan,
                             iscan,
                             qm,
                             iqm,
                             (int16_t)log_scale);
    return eob;
}

/* ---- AC-bias psychovisual kernels (ac_bias.c) ----
   svt_psy_distortion calls svt_aom_hadamard_{4x4,8x8}/svt_aom_satd through
   the aom_dsp RTCD dispatch table, which is NULL until setup — wrap with
   the init-once guard. */
uint64_t svt_psy_distortion(const uint8_t* input, uint32_t input_stride, const uint8_t* recon, uint32_t recon_stride,
                            uint32_t width, uint32_t height);

uint64_t ref_psy_distortion(const uint8_t* input, uint32_t input_stride, const uint8_t* recon, uint32_t recon_stride,
                            uint32_t width, uint32_t height) {
    /* hadamard_{4x4,8x8} live in COMMON dsp rtcd; satd in aom_dsp rtcd —
       init both tables. */
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    ref_dsp_rtcd_once();
    return svt_psy_distortion(input, input_stride, recon, recon_stride, width, height);
}

/* ---- fork mds0 distortion facade (pic_operators.c, SVT_HDR_MODE feature) ----
   Drive the real facade with a synthetic BlockModeInfo built from scalars so
   the Rust bias-layer port can be pinned against the C mode-enum logic. */
#include "block_structures.h"
#include "pic_operators.h"

uint64_t ref_spatial_facade(const uint8_t* input, uint32_t input_stride, const uint8_t* recon, uint32_t recon_stride,
                            uint32_t width, uint32_t height, uint8_t mode, uint8_t uv_mode, uint8_t is_chroma,
                            uint8_t is_interintra, uint8_t comp_type, uint8_t temporal_layer_index, double ac_bias,
                            uint8_t tx_bias) {
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    BlockModeInfo bmi;
    memset(&bmi, 0, sizeof(bmi));
    bmi.mode                 = (PredictionMode)mode;
    bmi.uv_mode              = (UvPredictionMode)uv_mode;
    bmi.is_interintra_used   = is_interintra;
    bmi.interinter_comp.type = (CompoundType)comp_type;
    return svt_spatial_full_distortion_kernel_facade((uint8_t*)input, 0, input_stride, (uint8_t*)recon, 0,
                                                     recon_stride, width, height, false, &bmi, is_chroma != 0,
                                                     temporal_layer_index, ac_bias, tx_bias);
}

/* ---- fork noise normalization (full_loop.c, SVT_HDR_MODE feature) ----
   The real function reads only: p->dequant_qtx (per-position DC/AC),
   qparam->iqmatrix (NULL = no QM), pcs->scs->static_config
   .noise_norm_strength, tx_size/tx_type/eob and the coefficient buffers.
   Build the minimal struct chain and forward. */
#include "pcs.h"
#include "transforms.h"
void svt_av1_perform_noise_normalization(MacroblockPlane* p, QuantParam* qparam, TranLow* coeff_ptr,
                                         TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, TxSize tx_size, TxType tx_type,
                                         uint16_t* eob, PictureControlSet* pcs);

void ref_noise_normalization(const int16_t dequant_dc, const int16_t dequant_ac, const int32_t* coeff,
                             int32_t* qcoeff, int32_t* dqcoeff, uint16_t* eob, int32_t tx_size, int32_t tx_type,
                             uint8_t strength) {
    static SequenceControlSet g_nn_scs;
    static PictureControlSet  g_nn_pcs;
    int16_t                   dequant[2] = {dequant_dc, dequant_ac};
    MacroblockPlane           p;
    QuantParam                qp;
    memset(&p, 0, sizeof(p));
    memset(&qp, 0, sizeof(qp));
    p.dequant_qtx                              = dequant;
    qp.iqmatrix                                = NULL;
    g_nn_pcs.scs                               = &g_nn_scs;
    g_nn_scs.static_config.noise_norm_strength = strength;
    svt_av1_perform_noise_normalization(
        &p, &qp, (TranLow*)coeff, qcoeff, dqcoeff, (TxSize)tx_size, (TxType)tx_type, eob, &g_nn_pcs);
}

/* ===========================================================================
 * High-bit-depth intra predictors (intra_prediction.c sized macro, :1602).
 *
 * The port's hbd::predict_*_hbd family (svtav1-dsp/src/hbd.rs) is the
 * differential target. C exposes one sized wrapper per (mode, W, H):
 *   svt_aom_highbd_<mode>_predictor_<W>x<H>_c(dst, stride, above, left, bd)
 * all declared in common_dsp_rtcd.h (already included transitively via
 * cabac_context_model.h at the top of this file). `mode` dispatch:
 *   0 dc      (DC of above+left; port has_above=1 has_left=1)
 *   1 dc_top  (DC of above only; port has_above=1 has_left=0)
 *   2 dc_left (DC of left only;  port has_above=0 has_left=1)
 *   3 dc_128  (128<<(bd-8);      port has_above=0 has_left=0)
 *   4 v  5 h  6 paeth  7 smooth  8 smooth_v  9 smooth_h
 *
 * highbd_v_predictor dereferences svt_memcpy (an RTCD function pointer, NULL
 * until svt_aom_setup_common_rtcd_internal runs) -> ref_rtcd_once() first. The
 * other kernels use the static-inline svt_aom_memset16 and need no init.
 * paeth reads above[-1] as its top-left corner: the shim takes `top_left`
 * explicitly (matching the port's separate arg) and stages a corner-prefixed
 * above row so C sees above[-1] == top_left. `above` holds W samples, `left`
 * holds H samples, `dst` is `stride`-strided with >= H rows.
 * =========================================================================== */

/* One sized-wrapper case per (W,H); pastes the mode token into the C name. */
#define HBD_IPRED_CASE(TYPE, W, H)                                                     \
    case (W) * 1000 + (H):                                                             \
        svt_aom_highbd_##TYPE##_predictor_##W##x##H##_c(dst, stride, above, left, bd); \
        break;

/* The 19 sizes the sized macro instantiates (5 square + 14 rectangular). */
#define HBD_IPRED_ALL_SIZES(TYPE)                                                          \
    HBD_IPRED_CASE(TYPE, 4, 4) HBD_IPRED_CASE(TYPE, 8, 8) HBD_IPRED_CASE(TYPE, 16, 16)      \
    HBD_IPRED_CASE(TYPE, 32, 32) HBD_IPRED_CASE(TYPE, 64, 64)                               \
    HBD_IPRED_CASE(TYPE, 4, 8) HBD_IPRED_CASE(TYPE, 4, 16) HBD_IPRED_CASE(TYPE, 8, 4)       \
    HBD_IPRED_CASE(TYPE, 8, 16) HBD_IPRED_CASE(TYPE, 8, 32) HBD_IPRED_CASE(TYPE, 16, 4)     \
    HBD_IPRED_CASE(TYPE, 16, 8) HBD_IPRED_CASE(TYPE, 16, 32) HBD_IPRED_CASE(TYPE, 16, 64)   \
    HBD_IPRED_CASE(TYPE, 32, 8) HBD_IPRED_CASE(TYPE, 32, 16) HBD_IPRED_CASE(TYPE, 32, 64)   \
    HBD_IPRED_CASE(TYPE, 64, 16) HBD_IPRED_CASE(TYPE, 64, 32)

#define DEF_HBD_IPRED(FNNAME, TYPE)                                             \
    static void FNNAME(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,  \
                       const uint16_t* left, int32_t bd, int32_t w, int32_t h) {\
        switch (w * 1000 + h) {                                                 \
            HBD_IPRED_ALL_SIZES(TYPE)                                           \
        default: break;                                                         \
        }                                                                       \
    }

DEF_HBD_IPRED(ref_hbd_ipred_dc, dc)
DEF_HBD_IPRED(ref_hbd_ipred_dc_top, dc_top)
DEF_HBD_IPRED(ref_hbd_ipred_dc_left, dc_left)
DEF_HBD_IPRED(ref_hbd_ipred_dc_128, dc_128)
DEF_HBD_IPRED(ref_hbd_ipred_v, v)
DEF_HBD_IPRED(ref_hbd_ipred_h, h)
DEF_HBD_IPRED(ref_hbd_ipred_paeth, paeth)
DEF_HBD_IPRED(ref_hbd_ipred_smooth, smooth)
DEF_HBD_IPRED(ref_hbd_ipred_smooth_v, smooth_v)
DEF_HBD_IPRED(ref_hbd_ipred_smooth_h, smooth_h)

#undef DEF_HBD_IPRED
#undef HBD_IPRED_ALL_SIZES
#undef HBD_IPRED_CASE

void ref_highbd_intra_pred(int32_t mode, uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                           const uint16_t* left, uint16_t top_left, int32_t w, int32_t h, int32_t bd) {
    ref_rtcd_once(); /* highbd_v_predictor dereferences svt_memcpy */
    switch (mode) {
    case 0: ref_hbd_ipred_dc(dst, stride, above, left, bd, w, h); break;
    case 1: ref_hbd_ipred_dc_top(dst, stride, above, left, bd, w, h); break;
    case 2: ref_hbd_ipred_dc_left(dst, stride, above, left, bd, w, h); break;
    case 3: ref_hbd_ipred_dc_128(dst, stride, above, left, bd, w, h); break;
    case 4: ref_hbd_ipred_v(dst, stride, above, left, bd, w, h); break;
    case 5: ref_hbd_ipred_h(dst, stride, above, left, bd, w, h); break;
    case 6: {
        /* paeth: C reads above[-1] as the top-left; stage a corner-prefixed
           row (w <= 64, so 65 entries cover corner + every above sample). */
        uint16_t corner_above[65];
        corner_above[0] = top_left;
        for (int32_t i = 0; i < w; i++) {
            corner_above[i + 1] = above[i];
        }
        ref_hbd_ipred_paeth(dst, stride, corner_above + 1, left, bd, w, h);
        break;
    }
    case 7: ref_hbd_ipred_smooth(dst, stride, above, left, bd, w, h); break;
    case 8: ref_hbd_ipred_smooth_v(dst, stride, above, left, bd, w, h); break;
    case 9: ref_hbd_ipred_smooth_h(dst, stride, above, left, bd, w, h); break;
    default: break;
    }
}

/* ---- IntraBC (IBC chunks 2-3, docs/ibc-port-map.md) ----
 *
 * Differential oracles for the pure-math IBC translations in
 * svtav1-encoder/src/intrabc.rs. Exported C entry points are called
 * directly; the shims below only assemble the tiny struct inputs
 * (MacroBlockD/TileInfo/PCS shells) from plain scalars.
 */
#include "coding_unit.h"       /* MacroBlockD */
#include "adaptive_mv_pred.h"  /* svt_aom_is_dv_valid */
#include "md_rate_estimation.h"/* MdRateEstimationContext, MV_VALS/MV_JOINTS */

/* svt_aom_is_dv_valid (adaptive_mv_pred.c:1908, EXPORTED) reads ONLY
 * xd->tile from the MacroBlockD (verified against the function body). */
int32_t ref_is_dv_valid(int16_t dv_x, int16_t dv_y, int32_t mi_row, int32_t mi_col,
                        int32_t bsize, int32_t mib_size_log2, int32_t tile_row_start,
                        int32_t tile_row_end, int32_t tile_col_start, int32_t tile_col_end) {
    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.tile.mi_row_start = tile_row_start;
    xd.tile.mi_row_end   = tile_row_end;
    xd.tile.mi_col_start = tile_col_start;
    xd.tile.mi_col_end   = tile_col_end;
    Mv dv;
    dv.x = dv_x;
    dv.y = dv_y;
    return svt_aom_is_dv_valid(dv, &xd, mi_row, mi_col, (BlockSize)bsize, mib_size_log2);
}

/* svt_aom_find_ref_dv (inter_prediction.c:2390, EXPORTED). Reads only
 * tile->mi_row_start; mi_col is (void)ed by C but passed through anyway. */
uint32_t ref_find_ref_dv(int32_t tile_row_start, int32_t mib_size, int32_t mi_row,
                         int32_t mi_col) {
    TileInfo tile;
    memset(&tile, 0, sizeof(tile));
    tile.mi_row_start = tile_row_start;
    Mv dv;
    dv.as_int = 0;
    svt_aom_find_ref_dv(&dv, &tile, mib_size, mi_row, mi_col);
    return dv.as_int;
}

/* svt_aom_get_qp_based_th_scaling_factors (enc_mode_config.c:25, EXPORTED). */
void svt_aom_get_qp_based_th_scaling_factors(bool enable_qp_based_th_scaling,
                                             uint32_t* ret_q_weight,
                                             uint32_t* ret_q_weight_denom, uint32_t qp);
void ref_qp_based_th_scaling_factors(int32_t enable, uint32_t qp, uint32_t* q_weight,
                                     uint32_t* q_weight_denom) {
    svt_aom_get_qp_based_th_scaling_factors(enable != 0, q_weight, q_weight_denom, qp);
}

/* Flat NmvContext staging: the struct is all-u16 (143 entries, no padding —
 * ref_nmv_context_flat_len lets the Rust side assert that), so a memcpy of
 * the c_parity_mv.rs field-order serialization IS the C layout. */
size_t ref_nmv_context_flat_len(void) { return sizeof(NmvContext) / sizeof(uint16_t); }

/* svt_aom_estimate_mv_rate (md_rate_estimation.c:458, EXPORTED): builds the
 * nmv tables (via the static svt_av1_build_nmv_cost_table +
 * build_nmv_component_cost_table chain) and, when allow_intrabc, the dv
 * tables from fc->ndvc at MV_SUBPEL_NONE. Replicates the approx_inter_rate
 * early-return ordering hazard by construction (it IS the real fn).
 * Outputs: nmv_joint[MV_JOINTS], nmv_costs[2*MV_VALS] (the SELECTED
 * hp/non-hp stack), dv_joint[MV_JOINTS], dv_costs[2*MV_VALS]. dv outputs are
 * pre-filled with a sentinel by the caller to observe "left unfilled". */
void ref_estimate_mv_rate(int32_t approx_inter_rate, int32_t allow_intrabc,
                          int32_t allow_high_precision_mv, const uint16_t* nmvc_flat,
                          const uint16_t* ndvc_flat, int32_t* nmv_joint, int32_t* nmv_costs,
                          int32_t* dv_joint, int32_t* dv_costs) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    MdRateEstimationContext* md   = (MdRateEstimationContext*)calloc(1, sizeof(*md));
    FRAME_CONTEXT*           fc   = (FRAME_CONTEXT*)calloc(1, sizeof(*fc));
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    svt_aom_init_mode_probs(fc);
    if (nmvc_flat)
        memcpy(&fc->nmvc, nmvc_flat, sizeof(NmvContext));
    if (ndvc_flat)
        memcpy(&fc->ndvc, ndvc_flat, sizeof(NmvContext));
    pcs->ppcs                             = ppcs;
    pcs->approx_inter_rate                = (uint8_t)approx_inter_rate;
    ppcs->frm_hdr.allow_intrabc           = allow_intrabc;
    ppcs->frm_hdr.allow_high_precision_mv = (uint8_t)allow_high_precision_mv;
    /* seed the dv outputs' backing fields with the caller's sentinel so the
     * "!allow_intrabc leaves them untouched" arm is observable */
    memcpy(md->dv_joint_cost, dv_joint, sizeof(int32_t) * MV_JOINTS);
    memcpy(md->dv_cost[0], dv_costs, sizeof(int32_t) * MV_VALS);
    memcpy(md->dv_cost[1], dv_costs + MV_VALS, sizeof(int32_t) * MV_VALS);
    svt_aom_estimate_mv_rate(pcs, md, fc);
    memcpy(nmv_joint, md->nmv_vec_cost, sizeof(int32_t) * MV_JOINTS);
    memcpy(nmv_costs, md->nmvcoststack[0] - MV_MAX, sizeof(int32_t) * MV_VALS);
    memcpy(nmv_costs + MV_VALS, md->nmvcoststack[1] - MV_MAX, sizeof(int32_t) * MV_VALS);
    memcpy(dv_joint, md->dv_joint_cost, sizeof(int32_t) * MV_JOINTS);
    memcpy(dv_costs, md->dv_cost[0], sizeof(int32_t) * MV_VALS);
    memcpy(dv_costs + MV_VALS, md->dv_cost[1], sizeof(int32_t) * MV_VALS);
    free(fc);
    free(md);
    free(pcs);
    free(ppcs);
}

/* svt_av1_mv_bit_cost / svt_aom_mv_err_cost (+ _light) — EXPORTED; direct
 * wrappers assembling the Mv values and the mvcost[2] mid-table pointers. */
int32_t svt_av1_mv_bit_cost(const Mv* mv, const Mv* ref, const int32_t* mvjcost,
                            const int32_t* const mvcost[2], int32_t weight);
int32_t svt_av1_mv_bit_cost_light(const Mv* mv, const Mv* ref);
int     svt_aom_mv_err_cost(const Mv* mv, const Mv* ref, const int* mvjcost, const int* mvcost[2],
                            int error_per_bit);
int     svt_aom_mv_err_cost_light(const Mv* mv, const Mv* ref);

int32_t ref_mv_bit_cost(int16_t mv_x, int16_t mv_y, int16_t ref_x, int16_t ref_y,
                        const int32_t* mvjcost, const int32_t* mvcost0_full,
                        const int32_t* mvcost1_full, int32_t weight) {
    Mv mv, rf;
    mv.x = mv_x;
    mv.y = mv_y;
    rf.x = ref_x;
    rf.y = ref_y;
    const int32_t* stack[2] = {mvcost0_full + MV_MAX, mvcost1_full + MV_MAX};
    return svt_av1_mv_bit_cost(&mv, &rf, mvjcost, stack, weight);
}

int32_t ref_mv_bit_cost_light(int16_t mv_x, int16_t mv_y, int16_t ref_x, int16_t ref_y) {
    Mv mv, rf;
    mv.x = mv_x;
    mv.y = mv_y;
    rf.x = ref_x;
    rf.y = ref_y;
    return svt_av1_mv_bit_cost_light(&mv, &rf);
}

int32_t ref_mv_err_cost(int16_t mv_x, int16_t mv_y, int16_t ref_x, int16_t ref_y,
                        const int32_t* mvjcost, const int32_t* mvcost0_full,
                        const int32_t* mvcost1_full, int32_t error_per_bit) {
    Mv mv, rf;
    mv.x = mv_x;
    mv.y = mv_y;
    rf.x = ref_x;
    rf.y = ref_y;
    const int* stack[2] = {(const int*)(mvcost0_full + MV_MAX), (const int*)(mvcost1_full + MV_MAX)};
    return svt_aom_mv_err_cost(&mv, &rf, (const int*)mvjcost, stack, error_per_bit);
}

int32_t ref_mv_err_cost_light(int16_t mv_x, int16_t mv_y, int16_t ref_x, int16_t ref_y) {
    Mv mv, rf;
    mv.x = mv_x;
    mv.y = mv_y;
    rf.x = ref_x;
    rf.y = ref_y;
    return svt_aom_mv_err_cost_light(&mv, &rf);
}

/* svt_aom_estimate_syntax_rate (md_rate_estimation.c:74, EXPORTED) — the
 * intrabc_fac_bits fill site (:253-255, gated allow_intrabc). Only the
 * intrabc slice is exported; fac bits are pre-seeded with the caller's
 * sentinel so the "!allow_intrabc leaves them untouched" arm is observable. */
void svt_aom_estimate_syntax_rate(MdRateEstimationContext* md_rate_est_ctx, bool is_i_slice,
                                  uint8_t pic_filter_intra_level, uint8_t allow_screen_content_tools,
                                  uint8_t enable_restoration, uint8_t allow_intrabc,
                                  FRAME_CONTEXT* fc);
void ref_estimate_syntax_rate_intrabc(const uint16_t* intrabc_cdf3, int32_t allow_intrabc,
                                      int32_t* fac_out2) {
    FRAME_CONTEXT*           fc = (FRAME_CONTEXT*)calloc(1, sizeof(*fc));
    MdRateEstimationContext* md = (MdRateEstimationContext*)calloc(1, sizeof(*md));
    if (!g_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_rtcd_ready = 1;
    }
    svt_aom_init_mode_probs(fc);
    if (intrabc_cdf3)
        memcpy(fc->intrabc_cdf, intrabc_cdf3, 3 * sizeof(uint16_t));
    md->intrabc_fac_bits[0] = fac_out2[0];
    md->intrabc_fac_bits[1] = fac_out2[1];
    svt_aom_estimate_syntax_rate(md, true, 1, 1, 1, (uint8_t)allow_intrabc, fc);
    fac_out2[0] = md->intrabc_fac_bits[0];
    fac_out2[1] = md->intrabc_fac_bits[1];
    free(md);
    free(fc);
}

/* ---- IntraBC hash table (IBC chunk 4, docs/ibc-port-map.md §D) ----
 *
 * Differential oracles for svtav1-encoder/src/intrabc_hash.rs. All entry
 * points are EXPORTED T-symbols (verified via nm): the CRC, the frame
 * pyramid builders, the table create/add, the count/iterator readback,
 * and the per-block query. Shims only assemble the small struct inputs
 * (Yv12BufferConfig / IntraBcContext shells) and run the one-time global
 * init (CRC table + aom_dsp RTCD pointers) the encoder normally performs
 * in init_global_tables / rtcd setup.
 */
#include "hash.h"        /* svt_av1_crc32c_table_init, AOM_BUFFER_SIZE_FOR_BLOCK_HASH */
#include "hash_motion.h" /* HashTable, BlockHash, generate/add/query fns */

void init_fn_ptr(void); /* av1me.c:26 (EXPORTED, no header decl) */

static int g_ibc_rtcd_ready = 0;
static void ibc_ensure_init(void) {
    if (!g_ibc_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_av1_crc32c_table_init();
        init_fn_ptr(); /* svt_aom_mefn_ptr[bsize].sdf/vf/sdx4df (av1me.c:26) */
        g_ibc_rtcd_ready = 1;
    }
}

uint32_t ref_crc32c(const uint8_t* buf, size_t len) {
    ibc_ensure_init();
    return svt_av1_get_crc32c_value_c(buf, len);
}

/* svt_av1_generate_block_2x2_hash_value (hash_motion.c:153, EXPORTED),
 * bd8 arm (flags = 0). */
void ref_generate_block_2x2_hash(const uint8_t* pic, int32_t stride, int32_t w, int32_t h,
                                 uint32_t* dst) {
    ibc_ensure_init();
    Yv12BufferConfig buf;
    memset(&buf, 0, sizeof(buf));
    buf.y_buffer      = (uint8_t*)pic;
    buf.y_stride      = stride;
    buf.y_crop_width  = w;
    buf.y_crop_height = h;
    buf.flags         = 0;
    svt_av1_generate_block_2x2_hash_value(&buf, dst);
}

/* svt_av1_generate_block_hash_value (hash_motion.c:192, EXPORTED). */
void ref_generate_block_hash(int32_t w, int32_t h, int32_t block_size, const uint32_t* src,
                             uint32_t* dst) {
    ibc_ensure_init();
    Yv12BufferConfig buf;
    memset(&buf, 0, sizeof(buf));
    buf.y_crop_width  = w;
    buf.y_crop_height = h;
    svt_av1_generate_block_hash_value(&buf, block_size, (uint32_t*)src, dst);
}

/* Hash table lifecycle: create + per-size add + bucket readback + destroy.
 * The Rust side drives these through an opaque handle. */
void* ref_hash_table_create(void) {
    ibc_ensure_init();
    HashTable* t = (HashTable*)calloc(1, sizeof(HashTable));
    svt_aom_rtime_alloc_svt_av1_hash_table_create(t);
    return t;
}

void ref_hash_table_add(void* table, const uint32_t* pic_hash, int32_t pic_width,
                        int32_t pic_height, int32_t block_size, uint16_t max_cand_per_bucket) {
    svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data(
        (HashTable*)table, (uint32_t*)pic_hash, pic_width, pic_height, block_size,
        max_cand_per_bucket);
}

int32_t ref_hash_table_count(void* table, uint32_t hash_value1) {
    return svt_av1_hash_table_count((const HashTable*)table, hash_value1);
}

/* Read back one bucket IN ITERATION ORDER (the order the DV search
 * consumes candidates in — the cost-tie tie-break). Returns the entry
 * count; fills up to cap entries. */
int32_t ref_hash_table_read_bucket(void* table, uint32_t hash_value1, int16_t* xs, int16_t* ys,
                                   uint32_t* hv2s, int32_t cap) {
    HashTable* t     = (HashTable*)table;
    int32_t    count = svt_av1_hash_table_count(t, hash_value1);
    if (count <= 0) {
        return 0;
    }
    Iterator it = svt_av1_hash_get_first_iterator(t, hash_value1);
    for (int32_t i = 0; i < count && i < cap; i++, svt_aom_iterator_increment(&it)) {
        BlockHash* e = (BlockHash*)svt_aom_iterator_get(&it);
        xs[i]        = e->x;
        ys[i]        = e->y;
        hv2s[i]      = e->hash_value2;
    }
    return count;
}

void ref_hash_table_destroy(void* table) {
    svt_av1_hash_table_destroy((HashTable*)table);
    free(table);
}

/* svt_av1_get_block_hash_value (hash_motion.c:309, EXPORTED), bd8 arm
 * (use_highbitdepth = 0, matching the one call site av1me.c:1071). */
void ref_get_block_hash_value(const uint8_t* src, int32_t stride, int32_t block_size,
                              uint32_t* hash_value1, uint32_t* hash_value2) {
    ibc_ensure_init();
    IntraBcContext x;
    memset(&x, 0, sizeof(x));
    for (int i = 0; i < 2; i++) {
        x.hash_value_buffer[i] = (uint32_t*)malloc(sizeof(uint32_t) * AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
    }
    svt_av1_get_block_hash_value((uint8_t*)src, stride, block_size, hash_value1, hash_value2, 0, &x);
    for (int i = 0; i < 2; i++) {
        free(x.hash_value_buffer[i]);
    }
}

/* ---- IntraBC DV search (IBC chunk 5, docs/ibc-port-map.md §D) ----
 *
 * Differential oracles for the intrabc.rs search core. The workhorses are
 * EXPORTED (svt_av1_diamond_search_sad_c, svt_av1_full_pixel_search,
 * svt_av1_intrabc_hash_search, svt_av1_set_mv_search_range,
 * svt_av1_init3smotion_compensation, svt_aom_is_dv_valid) and are called
 * directly; the shims assemble IntraBcContext / PictureControlSet /
 * SequenceControlSet shells exactly as mode_decision.c:2976-3125 does.
 * The one static piece — the per-direction driver intra_bc_search itself —
 * is transcribed VERBATIM below (ref_intra_bc_search_driver) over the real
 * exported sub-functions (the sanctioned facade-over-real-fn pattern).
 */
#include "av1me.h" /* SearchSiteConfig, AomVarianceFnPtr, search fns */
#include "sequence_control_set.h"

extern AomVarianceFnPtr svt_aom_mefn_ptr[BLOCK_SIZES_ALL];

/* av1me.c:291 (EXPORTED, no header decl — the direct-call `_c` name). */
int svt_av1_diamond_search_sad_c(IntraBcContext* x, const SearchSiteConfig* cfg, Mv* ref_mv,
                                 Mv* best_mv, int search_param, int sad_per_bit, int* num00,
                                 const AomVarianceFnPtr* fn_ptr, const Mv* center_mv);

/* Kernel oracles: the EXACT sdf/vf the search binds for this bsize (RTCD-
 * resolved function pointers captured by init_fn_ptr — includes any SIMD
 * dispatch the real encoder would use). */
uint32_t ref_mefn_sdf(int32_t bsize, const uint8_t* src, int32_t src_stride, const uint8_t* ref,
                      int32_t ref_stride) {
    ibc_ensure_init();
    return svt_aom_mefn_ptr[bsize].sdf(src, src_stride, ref, ref_stride);
}

uint32_t ref_mefn_vf(int32_t bsize, const uint8_t* src, int32_t src_stride, const uint8_t* ref,
                     int32_t ref_stride) {
    ibc_ensure_init();
    unsigned int sse;
    return svt_aom_mefn_ptr[bsize].vf(src, src_stride, ref, ref_stride, &sse);
}

/* Shared IntraBcContext assembly (mode_decision.c:2978-3044 minus the
 * PCS plumbing): src == ref == the SOURCE plane at the block origin. */
static void ibc_setup_x(IntraBcContext* x, const int32_t** stack, const uint8_t* pic,
                        int32_t stride, int32_t x_pos, int32_t y_pos, int32_t col_min,
                        int32_t col_max, int32_t row_min, int32_t row_max,
                        const int32_t* dv_joint, const int32_t* dv_cost0,
                        const int32_t* dv_cost1, int32_t errorperbit,
                        int32_t approx_inter_rate) {
    memset(x, 0, sizeof(*x));
    x->plane[0].src.buf      = (uint8_t*)pic + y_pos * stride + x_pos;
    x->plane[0].src.stride   = stride;
    x->xdplane[0].pre[0]     = x->plane[0].src;
    x->mv_limits.col_min     = col_min;
    x->mv_limits.col_max     = col_max;
    x->mv_limits.row_min     = row_min;
    x->mv_limits.row_max     = row_max;
    x->errorperbit           = errorperbit;
    x->approx_inter_rate     = (uint8_t)approx_inter_rate;
    stack[0]                 = dv_cost0 + MV_MAX;
    stack[1]                 = dv_cost1 + MV_MAX;
    x->nmv_vec_cost          = (int*)dv_joint;
    x->mv_cost_stack         = stack;
}

/* svt_av1_diamond_search_sad_c (av1me.c:291, EXPORTED). The seed is the
 * folded IBC form: mvp_full = center >> 3 (the only shape the IBC call
 * chain ever uses — full_pixel_diamond av1me.c:497/521). Returns bestsad;
 * outputs the winner and num00. */
int32_t ref_diamond_search(const uint8_t* pic, int32_t stride, int32_t x_pos, int32_t y_pos,
                           int32_t bsize, int32_t center_x_ep, int32_t center_y_ep,
                           int32_t search_param, int32_t sad_per_bit, int32_t col_min,
                           int32_t col_max, int32_t row_min, int32_t row_max,
                           const int32_t* dv_joint, const int32_t* dv_cost0,
                           const int32_t* dv_cost1, int32_t errorperbit,
                           int32_t approx_inter_rate, int32_t* out_x, int32_t* out_y,
                           int32_t* out_num00) {
    ibc_ensure_init();
    IntraBcContext x;
    const int32_t* stack[2];
    ibc_setup_x(&x, stack, pic, stride, x_pos, y_pos, col_min, col_max, row_min, row_max,
                dv_joint, dv_cost0, dv_cost1, errorperbit, approx_inter_rate);
    SearchSiteConfig cfg;
    svt_av1_init3smotion_compensation(&cfg, stride);
    Mv center;
    center.x = (int16_t)center_x_ep;
    center.y = (int16_t)center_y_ep;
    Mv mvp_full;
    mvp_full.x = center.x >> 3;
    mvp_full.y = center.y >> 3;
    Mv  best;
    int num00   = 0;
    int bestsad = svt_av1_diamond_search_sad_c(&x, &cfg, &mvp_full, &best, search_param,
                                               sad_per_bit, &num00, &svt_aom_mefn_ptr[bsize],
                                               &center);
    *out_x     = best.x;
    *out_y     = best.y;
    *out_num00 = num00;
    return bestsad;
}

/* svt_av1_full_pixel_search (av1me.c:1115, EXPORTED): diamond + optional
 * mesh. mesh_patterns8 = 4 (range, interval) pairs. */
void ref_full_pixel_search(const uint8_t* pic, int32_t stride, int32_t x_pos, int32_t y_pos,
                           int32_t bsize, int32_t ref_mv_x_ep, int32_t ref_mv_y_ep,
                           int32_t sad_per_bit, int32_t col_min, int32_t col_max,
                           int32_t row_min, int32_t row_max, uint64_t exhaustive_mesh_thresh,
                           int32_t mesh_search_mv_diff_threshold, const int32_t* mesh_patterns8,
                           const int32_t* dv_joint, const int32_t* dv_cost0,
                           const int32_t* dv_cost1, int32_t errorperbit,
                           int32_t approx_inter_rate, int32_t* out_x, int32_t* out_y) {
    ibc_ensure_init();
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    pcs->ppcs                     = ppcs;
    ppcs->intrabc_ctrls.exhaustive_mesh_thresh          = exhaustive_mesh_thresh;
    ppcs->intrabc_ctrls.mesh_search_mv_diff_threshold   = mesh_search_mv_diff_threshold;
    for (int i = 0; i < MAX_MESH_STEP; i++) {
        ppcs->intrabc_ctrls.mesh_patterns[i].range    = mesh_patterns8[2 * i];
        ppcs->intrabc_ctrls.mesh_patterns[i].interval = mesh_patterns8[2 * i + 1];
    }
    svt_av1_init3smotion_compensation(&pcs->ss_cfg, stride);

    IntraBcContext x;
    const int32_t* stack[2];
    ibc_setup_x(&x, stack, pic, stride, x_pos, y_pos, col_min, col_max, row_min, row_max,
                dv_joint, dv_cost0, dv_cost1, errorperbit, approx_inter_rate);
    Mv ref_mv;
    ref_mv.x = (int16_t)ref_mv_x_ep;
    ref_mv.y = (int16_t)ref_mv_y_ep;
    Mv mvp_full;
    mvp_full.x = ref_mv.x >> 3;
    mvp_full.y = ref_mv.y >> 3;
    x.best_mv.as_int = 0;
    (void)svt_av1_full_pixel_search(pcs, &x, (BlockSize)bsize, &mvp_full, 0, sad_per_bit, NULL,
                                    &ref_mv);
    *out_x = x.best_mv.x;
    *out_y = x.best_mv.y;
    free(pcs);
    free(ppcs);
}

/* svt_av1_intrabc_hash_search (av1me.c:1056, EXPORTED). `hash_table` is a
 * chunk-4 CHashTable handle; the PCS embeds a struct copy (shared lookup
 * array — freed only through the handle). Returns best_hash_cost (INT_MAX
 * = no candidate), outputs the FULL-PEL winning mv. */
int32_t ref_intrabc_hash_search(const uint8_t* pic, int32_t stride, int32_t x_pos, int32_t y_pos,
                                int32_t bsize, int32_t ref_mv_x_ep, int32_t ref_mv_y_ep,
                                void* hash_table, int32_t max_block_size_hash,
                                int32_t sb_size_log2, int32_t tile_row_start,
                                int32_t tile_row_end, int32_t tile_col_start,
                                int32_t tile_col_end, int32_t col_min, int32_t col_max,
                                int32_t row_min, int32_t row_max, const int32_t* dv_joint,
                                const int32_t* dv_cost0, const int32_t* dv_cost1,
                                int32_t errorperbit, int32_t approx_inter_rate, int32_t* out_x,
                                int32_t* out_y) {
    ibc_ensure_init();
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    pcs->ppcs                     = ppcs;
    ppcs->scs                     = scs;
    scs->seq_header.sb_size_log2  = sb_size_log2;
    ppcs->intrabc_ctrls.max_block_size_hash = (uint8_t)max_block_size_hash;
    pcs->hash_table               = *(HashTable*)hash_table;

    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.tile.mi_row_start = tile_row_start;
    xd.tile.mi_row_end   = tile_row_end;
    xd.tile.mi_col_start = tile_col_start;
    xd.tile.mi_col_end   = tile_col_end;

    IntraBcContext x;
    const int32_t* stack[2];
    ibc_setup_x(&x, stack, pic, stride, x_pos, y_pos, col_min, col_max, row_min, row_max,
                dv_joint, dv_cost0, dv_cost1, errorperbit, approx_inter_rate);
    x.xd = &xd;
    for (int i = 0; i < 2; i++) {
        x.hash_value_buffer[i] = (uint32_t*)malloc(sizeof(uint32_t) * AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
    }
    Mv ref_mv;
    ref_mv.x = (int16_t)ref_mv_x_ep;
    ref_mv.y = (int16_t)ref_mv_y_ep;

    int best_hash_cost = INT_MAX;
    Mv  best_hash_mv;
    best_hash_mv.as_int = 0;
    svt_av1_intrabc_hash_search(pcs, &x, (BlockSize)bsize, x_pos, y_pos, &ref_mv, 1,
                                &svt_aom_mefn_ptr[bsize], &best_hash_cost, &best_hash_mv);
    *out_x = best_hash_mv.x;
    *out_y = best_hash_mv.y;
    for (int i = 0; i < 2; i++) {
        free(x.hash_value_buffer[i]);
    }
    free(scs);
    free(pcs);
    free(ppcs);
    return best_hash_cost;
}

/* mv_check_bounds (mode_decision.c:2965-2968, static INLINE) — verbatim. */
static INLINE int shim_mv_check_bounds(const MvLimits* mv_limits, const Mv* mv) {
    return (mv->y >> 3) < mv_limits->row_min || (mv->y >> 3) > mv_limits->row_max ||
        (mv->x >> 3) < mv_limits->col_min || (mv->x >> 3) > mv_limits->col_max;
}

/* intra_bc_search (mode_decision.c:2976-3125, static) — VERBATIM
 * transcription of the per-direction driver over the REAL exported
 * sub-functions. Deviations from C, all plumbing-only: dv_ref arrives
 * already composed (the from-stack + find_ref_dv composition is chunk 6's
 * surface, differentially locked separately); sadperbit16/errorperbit and
 * the cost tables arrive as parameters instead of deriving from
 * lambda/qindex; the source-plane Buf2D is set directly instead of via
 * svt_aom_link_eb_to_aom_buffer_desc_8bit + svt_av1_setup_pred_block
 * (same buf/stride result). Returns the number of DV candidates (0-2);
 * out_dv holds eighth-pel x,y pairs. */
int32_t ref_intra_bc_search_driver(
    const uint8_t* pic, int32_t stride, int32_t bsize, int32_t bw, int32_t bh, int32_t mi_row,
    int32_t mi_col, int32_t mi_rows, int32_t mi_cols, int32_t sb_mi_size, int32_t sb_size_log2,
    int32_t tile_row_start, int32_t tile_row_end, int32_t tile_col_start, int32_t tile_col_end,
    int32_t dv_ref_x, int32_t dv_ref_y, int32_t search_dir, int32_t max_block_size_hash,
    uint64_t exhaustive_mesh_thresh, int32_t mesh_search_mv_diff_threshold,
    const int32_t* mesh_patterns8, void* hash_table_or_null, int32_t sadperbit16,
    int32_t errorperbit, const int32_t* dv_joint, const int32_t* dv_cost0,
    const int32_t* dv_cost1, int32_t approx_inter_rate, int16_t* out_dv) {
    ibc_ensure_init();
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    pcs->ppcs                     = ppcs;
    ppcs->scs                     = scs;
    scs->seq_header.sb_size_log2  = sb_size_log2;
    scs->seq_header.sb_mi_size    = sb_mi_size;
    ppcs->intrabc_ctrls.max_block_size_hash           = (uint8_t)max_block_size_hash;
    ppcs->intrabc_ctrls.exhaustive_mesh_thresh        = exhaustive_mesh_thresh;
    ppcs->intrabc_ctrls.mesh_search_mv_diff_threshold = mesh_search_mv_diff_threshold;
    ppcs->intrabc_ctrls.search_dir                    = (uint8_t)search_dir;
    for (int i = 0; i < MAX_MESH_STEP; i++) {
        ppcs->intrabc_ctrls.mesh_patterns[i].range    = mesh_patterns8[2 * i];
        ppcs->intrabc_ctrls.mesh_patterns[i].interval = mesh_patterns8[2 * i + 1];
    }
    if (hash_table_or_null) {
        pcs->hash_table = *(HashTable*)hash_table_or_null;
    }
    svt_av1_init3smotion_compensation(&pcs->ss_cfg, stride);

    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.tile.mi_row_start = tile_row_start;
    xd.tile.mi_row_end   = tile_row_end;
    xd.tile.mi_col_start = tile_col_start;
    xd.tile.mi_col_end   = tile_col_end;

    const int32_t x_pos = mi_col * 4; /* MI_SIZE */
    const int32_t y_pos = mi_row * 4;

    /* --- transcription of mode_decision.c:2976-3125 begins --- */
    IntraBcContext x_st;
    IntraBcContext* x = &x_st;
    const int32_t* stack[2];
    /* whole-frame limits (:3005-3008); per-direction arms overwrite all 4 */
    const int mi_width  = bw / 4;
    const int mi_height = bh / 4;
    ibc_setup_x(x, stack, pic, stride, x_pos, y_pos,
                -(((mi_col + mi_width) * 4) + 4 /*AOM_INTERP_EXTEND*/),
                (mi_cols - mi_col) * 4 + 4,
                -(((mi_row + mi_height) * 4) + 4),
                (mi_rows - mi_row) * 4 + 4,
                dv_joint, dv_cost0, dv_cost1, errorperbit, approx_inter_rate);
    /* ibc_setup_x fills (col_min, col_max, row_min, row_max) in that order;
     * re-set explicitly to keep the row/col mapping obvious: */
    x->mv_limits.row_min = -(((mi_row + mi_height) * 4) + 4);
    x->mv_limits.col_min = -(((mi_col + mi_width) * 4) + 4);
    x->mv_limits.row_max = (mi_rows - mi_row) * 4 + 4;
    x->mv_limits.col_max = (mi_cols - mi_col) * 4 + 4;
    x->xd          = &xd;
    x->sadperbit16 = sadperbit16;
    for (int i = 0; i < 2; i++) {
        x->hash_value_buffer[i] = (uint32_t*)malloc(sizeof(uint32_t) * AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
    }
    Mv dv_ref;
    dv_ref.x = (int16_t)dv_ref_x;
    dv_ref.y = (int16_t)dv_ref_y;

    const int w      = bw;
    const int h      = bh;
    const int sb_row = mi_row >> sb_size_log2;
    const int sb_col = mi_col >> sb_size_log2;

    int32_t num_dv_cand = 0;

    enum IntrabcMotionDirection max_dir = ppcs->intrabc_ctrls.search_dir ? IBC_MOTION_LEFT
                                                                         : IBC_MOTION_DIRECTIONS;

    for (enum IntrabcMotionDirection dir = IBC_MOTION_ABOVE; dir < max_dir; ++dir) {
        const MvLimits tmp_mv_limits = x->mv_limits;

        switch (dir) {
        case IBC_MOTION_ABOVE:
            x->mv_limits.col_min = (xd.tile.mi_col_start - mi_col) * 4;
            x->mv_limits.col_max = (xd.tile.mi_col_end - mi_col) * 4 - w;
            x->mv_limits.row_min = (xd.tile.mi_row_start - mi_row) * 4;
            x->mv_limits.row_max = (sb_row * sb_mi_size - mi_row) * 4 - h;
            break;
        case IBC_MOTION_LEFT: {
            x->mv_limits.col_min = (xd.tile.mi_col_start - mi_col) * 4;
            x->mv_limits.col_max = (sb_col * sb_mi_size - mi_col) * 4 - w;
            x->mv_limits.row_min = (xd.tile.mi_row_start - mi_row) * 4;
            int bottom_coded_mi_edge = AOMMIN((sb_row + 1) * sb_mi_size, xd.tile.mi_row_end);
            x->mv_limits.row_max     = (bottom_coded_mi_edge - mi_row) * 4 - h;
            break;
        }
        default:
            break;
        }

        svt_av1_set_mv_search_range(&x->mv_limits, &dv_ref);

        if (x->mv_limits.col_max < x->mv_limits.col_min ||
            x->mv_limits.row_max < x->mv_limits.row_min) {
            x->mv_limits = tmp_mv_limits;
            continue;
        }
        Mv mvp_full = dv_ref;
        mvp_full.x >>= 3;
        mvp_full.y >>= 3;
        x->best_mv.as_int = 0;

        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[bsize];

        int best_hash_cost = INT_MAX;
        Mv  best_hash_mv;
        best_hash_mv.as_int = 0;

        if (hash_table_or_null) {
            svt_av1_intrabc_hash_search(pcs, x, (BlockSize)bsize, x_pos, y_pos, &dv_ref, 1,
                                        fn_ptr, &best_hash_cost, &best_hash_mv);
        }

        if (best_hash_cost < INT_MAX) {
            Mv dv;
            dv.x = best_hash_mv.x * 8;
            dv.y = best_hash_mv.y * 8;
            out_dv[num_dv_cand * 2]     = dv.x;
            out_dv[num_dv_cand * 2 + 1] = dv.y;
            num_dv_cand++;
            x->best_mv = best_hash_mv;
        } else {
            svt_av1_full_pixel_search(pcs, x, (BlockSize)bsize, &mvp_full, 0, x->sadperbit16,
                                      NULL, &dv_ref);
            Mv dv;
            dv.x = x->best_mv.x * 8;
            dv.y = x->best_mv.y * 8;
            if (!shim_mv_check_bounds(&x->mv_limits, &dv) &&
                svt_aom_is_dv_valid(dv, &xd, mi_row, mi_col, (BlockSize)bsize, sb_size_log2)) {
                out_dv[num_dv_cand * 2]     = dv.x;
                out_dv[num_dv_cand * 2 + 1] = dv.y;
                num_dv_cand++;
            }
        }

        x->mv_limits = tmp_mv_limits;
    }
    /* --- transcription ends --- */

    for (int i = 0; i < 2; i++) {
        free(x->hash_value_buffer[i]);
    }
    free(scs);
    free(pcs);
    free(ppcs);
    return num_dv_cand;
}
