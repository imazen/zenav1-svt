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

/* ---- CDEF strength-from-QP picker (svt_pick_cdef_from_qp is static; this
   replicates its intra branch verbatim from enc_cdef.c:849 against the
   REAL svt_aom_ac_quant_qtx, pinning the C float-expression semantics the
   Rust port must reproduce bit-exactly) ---- */

#include <math.h>

int16_t svt_aom_ac_quant_qtx(int32_t qindex, int32_t delta, EbBitDepth bit_depth);

void ref_pick_cdef_from_qp_intra_8bit(int32_t base_q_idx, int32_t* pred_y_strength,
                                      int32_t* pred_uv_strength) {
    int32_t q = svt_aom_ac_quant_qtx(base_q_idx, 0, EB_EIGHT_BIT);
    q >>= 0; /* (bit_depth - 8) */

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
    ConvolveParams cp = get_conv_params(0, 0, 0, 8); /* non-compound, 8-bit */
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
    ConvolveParams            cp = get_conv_params(0, 0, 0, 8);
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
