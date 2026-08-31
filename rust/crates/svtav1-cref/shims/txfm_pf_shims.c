/*
 * C shims for the reduced-coefficient-shape (_N2 / _N4) forward transforms
 * and the transform-config surface of Codec/transforms.c.
 *
 * Its own translation unit so the wp-transforms lane never shares a shim
 * file with another lane.
 *
 * RULE (see the header of ref_shims.c): NO PER-CALL STATE IN A `static`.
 * Every scratch buffer below is a local; the only file-scope objects are
 * `static const` dispatch tables, which are not state.
 */
#include <stdint.h>
#include <string.h>

#include "transforms.h"
#include "inv_transforms.h"

/* Exported by libSvtAv1Enc but not declared in the headers this TU pulls in. */
void svt_aom_transform_config(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg);
void svt_av1_gen_fwd_stage_range(int8_t* stage_range_col, int8_t* stage_range_row, const Txfm2dFlipCfg* cfg,
                                 int32_t bd);
void svt_av1_highbd_fwd_txfm(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param);
void svt_av1_highbd_fwd_txfm_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param);
void svt_av1_highbd_fwd_txfm_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param);
void svt_av1_get_inv_txfm_cfg(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg);

/* ---- svt_aom_transform_config / svt_av1_gen_fwd_stage_range ---- */

/* Flattened so the Rust side never has to mirror Txfm2dFlipCfg's ABI:
 *   [0] ud_flip      [1] lr_flip
 *   [2..5) shift[0..3)
 *   [5] cos_bit_col  [6] cos_bit_row
 *   [7] txfm_type_col [8] txfm_type_row
 *   [9] stage_num_col [10] stage_num_row
 *   [11..23) stage_range_col   [23..35) stage_range_row
 */
void ref_transform_config(int32_t tx_type, int32_t tx_size, int32_t* out) {
    Txfm2dFlipCfg cfg;
    memset(&cfg, 0, sizeof(cfg));
    svt_aom_transform_config((TxType)tx_type, (TxSize)tx_size, &cfg);
    out[0] = cfg.ud_flip;
    out[1] = cfg.lr_flip;
    for (int i = 0; i < 3; ++i) out[2 + i] = cfg.shift[i];
    out[5]  = cfg.cos_bit_col;
    out[6]  = cfg.cos_bit_row;
    out[7]  = (int32_t)cfg.txfm_type_col;
    out[8]  = (int32_t)cfg.txfm_type_row;
    out[9]  = cfg.stage_num_col;
    out[10] = cfg.stage_num_row;
    for (int i = 0; i < MAX_TXFM_STAGE_NUM; ++i) {
        out[11 + i] = cfg.stage_range_col[i];
        out[11 + MAX_TXFM_STAGE_NUM + i] = cfg.stage_range_row[i];
    }
}

void ref_gen_fwd_stage_range(int32_t tx_type, int32_t tx_size, int32_t bd, int8_t* col, int8_t* row) {
    Txfm2dFlipCfg cfg;
    memset(&cfg, 0, sizeof(cfg));
    svt_aom_transform_config((TxType)tx_type, (TxSize)tx_size, &cfg);
    int8_t c[MAX_TXFM_STAGE_NUM];
    int8_t r[MAX_TXFM_STAGE_NUM];
    memset(c, 0, sizeof(c));
    memset(r, 0, sizeof(r));
    svt_av1_gen_fwd_stage_range(c, r, &cfg, bd);
    memcpy(col, c, sizeof(c));
    memcpy(row, r, sizeof(r));
}

/* ---- 2-D entry points, N2 and N4 ---- */

typedef void (*RefFwd2d)(int16_t*, int32_t*, uint32_t, TxType, uint8_t);

#define DECL2D(NAME) void NAME(int16_t*, int32_t*, uint32_t, TxType, uint8_t);
DECL2D(svt_aom_transform_two_d_4x4_N2_c)
DECL2D(svt_aom_transform_two_d_8x8_N2_c)
DECL2D(svt_aom_transform_two_d_16x16_N2_c)
DECL2D(svt_aom_transform_two_d_32x32_N2_c)
DECL2D(svt_aom_transform_two_d_64x64_N2_c)
DECL2D(svt_av1_fwd_txfm2d_4x8_N2_c)
DECL2D(svt_av1_fwd_txfm2d_8x4_N2_c)
DECL2D(svt_av1_fwd_txfm2d_8x16_N2_c)
DECL2D(svt_av1_fwd_txfm2d_16x8_N2_c)
DECL2D(svt_av1_fwd_txfm2d_16x32_N2_c)
DECL2D(svt_av1_fwd_txfm2d_32x16_N2_c)
DECL2D(svt_av1_fwd_txfm2d_32x64_N2_c)
DECL2D(svt_av1_fwd_txfm2d_64x32_N2_c)
DECL2D(svt_av1_fwd_txfm2d_4x16_N2_c)
DECL2D(svt_av1_fwd_txfm2d_16x4_N2_c)
DECL2D(svt_av1_fwd_txfm2d_8x32_N2_c)
DECL2D(svt_av1_fwd_txfm2d_32x8_N2_c)
DECL2D(svt_av1_fwd_txfm2d_16x64_N2_c)
DECL2D(svt_av1_fwd_txfm2d_64x16_N2_c)
DECL2D(svt_aom_transform_two_d_4x4_N4_c)
DECL2D(svt_aom_transform_two_d_8x8_N4_c)
DECL2D(svt_aom_transform_two_d_16x16_N4_c)
DECL2D(svt_aom_transform_two_d_32x32_N4_c)
DECL2D(svt_aom_transform_two_d_64x64_N4_c)
DECL2D(svt_av1_fwd_txfm2d_4x8_N4_c)
DECL2D(svt_av1_fwd_txfm2d_8x4_N4_c)
DECL2D(svt_av1_fwd_txfm2d_8x16_N4_c)
DECL2D(svt_av1_fwd_txfm2d_16x8_N4_c)
DECL2D(svt_av1_fwd_txfm2d_16x32_N4_c)
DECL2D(svt_av1_fwd_txfm2d_32x16_N4_c)
DECL2D(svt_av1_fwd_txfm2d_32x64_N4_c)
DECL2D(svt_av1_fwd_txfm2d_64x32_N4_c)
DECL2D(svt_av1_fwd_txfm2d_4x16_N4_c)
DECL2D(svt_av1_fwd_txfm2d_16x4_N4_c)
DECL2D(svt_av1_fwd_txfm2d_8x32_N4_c)
DECL2D(svt_av1_fwd_txfm2d_32x8_N4_c)
DECL2D(svt_av1_fwd_txfm2d_16x64_N4_c)
DECL2D(svt_av1_fwd_txfm2d_64x16_N4_c)

/* Indexed by TxSize (TX_4X4..TX_64X16); `static const` table, not state. */
static const RefFwd2d ref_fwd2d_n2[TX_SIZES_ALL] = {
    svt_aom_transform_two_d_4x4_N2_c,   svt_aom_transform_two_d_8x8_N2_c,
    svt_aom_transform_two_d_16x16_N2_c, svt_aom_transform_two_d_32x32_N2_c,
    svt_aom_transform_two_d_64x64_N2_c, svt_av1_fwd_txfm2d_4x8_N2_c,
    svt_av1_fwd_txfm2d_8x4_N2_c,        svt_av1_fwd_txfm2d_8x16_N2_c,
    svt_av1_fwd_txfm2d_16x8_N2_c,       svt_av1_fwd_txfm2d_16x32_N2_c,
    svt_av1_fwd_txfm2d_32x16_N2_c,      svt_av1_fwd_txfm2d_32x64_N2_c,
    svt_av1_fwd_txfm2d_64x32_N2_c,      svt_av1_fwd_txfm2d_4x16_N2_c,
    svt_av1_fwd_txfm2d_16x4_N2_c,       svt_av1_fwd_txfm2d_8x32_N2_c,
    svt_av1_fwd_txfm2d_32x8_N2_c,       svt_av1_fwd_txfm2d_16x64_N2_c,
    svt_av1_fwd_txfm2d_64x16_N2_c,
};
static const RefFwd2d ref_fwd2d_n4[TX_SIZES_ALL] = {
    svt_aom_transform_two_d_4x4_N4_c,   svt_aom_transform_two_d_8x8_N4_c,
    svt_aom_transform_two_d_16x16_N4_c, svt_aom_transform_two_d_32x32_N4_c,
    svt_aom_transform_two_d_64x64_N4_c, svt_av1_fwd_txfm2d_4x8_N4_c,
    svt_av1_fwd_txfm2d_8x4_N4_c,        svt_av1_fwd_txfm2d_8x16_N4_c,
    svt_av1_fwd_txfm2d_16x8_N4_c,       svt_av1_fwd_txfm2d_16x32_N4_c,
    svt_av1_fwd_txfm2d_32x16_N4_c,      svt_av1_fwd_txfm2d_32x64_N4_c,
    svt_av1_fwd_txfm2d_64x32_N4_c,      svt_av1_fwd_txfm2d_4x16_N4_c,
    svt_av1_fwd_txfm2d_16x4_N4_c,       svt_av1_fwd_txfm2d_8x32_N4_c,
    svt_av1_fwd_txfm2d_32x8_N4_c,       svt_av1_fwd_txfm2d_16x64_N4_c,
    svt_av1_fwd_txfm2d_64x16_N4_c,
};

/* shape: 1 = N2_SHAPE, 2 = N4_SHAPE. */
void ref_fwd_txfm2d_pf(int32_t tx_size, int32_t shape, int16_t* input, int32_t* output, uint32_t input_stride,
                       int32_t tx_type, uint8_t bd) {
    const RefFwd2d f = (shape == 2) ? ref_fwd2d_n4[tx_size] : ref_fwd2d_n2[tx_size];
    f(input, output, input_stride, (TxType)tx_type, bd);
}

/* ---- DEFAULT-shape 2-D entries (the `_c` implementations, no RTCD) ---- */

#define DECL2DC(NAME) void NAME(int16_t*, int32_t*, uint32_t, TxType, uint8_t);
DECL2DC(svt_av1_transform_two_d_4x4_c)
DECL2DC(svt_av1_transform_two_d_8x8_c)
DECL2DC(svt_av1_transform_two_d_16x16_c)
DECL2DC(svt_av1_transform_two_d_32x32_c)
DECL2DC(svt_av1_transform_two_d_64x64_c)
DECL2DC(svt_av1_fwd_txfm2d_4x8_c)
DECL2DC(svt_av1_fwd_txfm2d_8x4_c)
DECL2DC(svt_av1_fwd_txfm2d_8x16_c)
DECL2DC(svt_av1_fwd_txfm2d_16x8_c)
DECL2DC(svt_av1_fwd_txfm2d_16x32_c)
DECL2DC(svt_av1_fwd_txfm2d_32x16_c)
DECL2DC(svt_av1_fwd_txfm2d_32x64_c)
DECL2DC(svt_av1_fwd_txfm2d_64x32_c)
DECL2DC(svt_av1_fwd_txfm2d_4x16_c)
DECL2DC(svt_av1_fwd_txfm2d_16x4_c)
DECL2DC(svt_av1_fwd_txfm2d_8x32_c)
DECL2DC(svt_av1_fwd_txfm2d_32x8_c)
DECL2DC(svt_av1_fwd_txfm2d_16x64_c)
DECL2DC(svt_av1_fwd_txfm2d_64x16_c)

static const RefFwd2d ref_fwd2d_default[TX_SIZES_ALL] = {
    svt_av1_transform_two_d_4x4_c,   svt_av1_transform_two_d_8x8_c,
    svt_av1_transform_two_d_16x16_c, svt_av1_transform_two_d_32x32_c,
    svt_av1_transform_two_d_64x64_c, svt_av1_fwd_txfm2d_4x8_c,
    svt_av1_fwd_txfm2d_8x4_c,        svt_av1_fwd_txfm2d_8x16_c,
    svt_av1_fwd_txfm2d_16x8_c,       svt_av1_fwd_txfm2d_16x32_c,
    svt_av1_fwd_txfm2d_32x16_c,      svt_av1_fwd_txfm2d_32x64_c,
    svt_av1_fwd_txfm2d_64x32_c,      svt_av1_fwd_txfm2d_4x16_c,
    svt_av1_fwd_txfm2d_16x4_c,       svt_av1_fwd_txfm2d_8x32_c,
    svt_av1_fwd_txfm2d_32x8_c,       svt_av1_fwd_txfm2d_16x64_c,
    svt_av1_fwd_txfm2d_64x16_c,
};

void ref_fwd_txfm2d_default(int32_t tx_size, int16_t* input, int32_t* output, uint32_t input_stride, int32_t tx_type,
                            uint8_t bd) {
    ref_fwd2d_default[tx_size](input, output, input_stride, (TxType)tx_type, bd);
}

/* ---- svt_av1_wht_fwd_txfm and the highbd dispatch tables ---- */

/* svt_av1_highbd_fwd_txfm* and svt_av1_wht_fwd_txfm route through the RTCD
 * function-pointer table (aom_dsp_rtcd.h:120 et al.), which is NULL until
 * setup runs. `g_txfm_pf_rtcd_ready` is an idempotent one-shot init flag, not
 * per-call state: a racing double-init re-runs the same setup with the same
 * CPU flags and lands the same pointers. */
typedef uint64_t EbCpuFlags;
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);

static int g_txfm_pf_rtcd_ready = 0;
static void txfm_pf_ensure_rtcd(void) {
    if (!g_txfm_pf_rtcd_ready) {
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_txfm_pf_rtcd_ready = 1;
    }
}

void ref_wht_fwd_txfm(int16_t* src_diff, int32_t bw, int32_t* coeff, int32_t tx_size, int32_t pf_shape,
                      int32_t bit_depth, int32_t is_hbd) {
    txfm_pf_ensure_rtcd();
    svt_av1_wht_fwd_txfm(src_diff, bw, coeff, (TxSize)tx_size, (TxCoeffShape)pf_shape, bit_depth, is_hbd);
}

/* variant: 0 = default, 1 = _n2, 2 = _n4. */
void ref_highbd_fwd_txfm(int32_t variant, int16_t* src_diff, int32_t* coeff, int32_t diff_stride, int32_t tx_type,
                         int32_t tx_size, int32_t bd) {
    txfm_pf_ensure_rtcd();
    TxfmParam p;
    memset(&p, 0, sizeof(p));
    p.tx_type     = (TxType)tx_type;
    p.tx_size     = (TxSize)tx_size;
    p.lossless    = 0;
    p.tx_set_type = EXT_TX_SET_ALL16;
    p.bd          = bd;
    p.is_hbd      = (bd > 8);
    if (variant == 2) {
        svt_av1_highbd_fwd_txfm_n4(src_diff, coeff, diff_stride, &p);
    } else if (variant == 1) {
        svt_av1_highbd_fwd_txfm_n2(src_diff, coeff, diff_stride, &p);
    } else {
        svt_av1_highbd_fwd_txfm(src_diff, coeff, diff_stride, &p);
    }
}

/* ---- svt_handle_transform* (full and N2_N4 variants) ---- */

uint64_t svt_handle_transform16x64_c(int32_t*);
uint64_t svt_handle_transform32x64_c(int32_t*);
uint64_t svt_handle_transform64x16_c(int32_t*);
uint64_t svt_handle_transform64x32_c(int32_t*);
uint64_t svt_handle_transform64x64_c(int32_t*);
uint64_t svt_handle_transform16x64_N2_N4_c(int32_t*);
uint64_t svt_handle_transform32x64_N2_N4_c(int32_t*);
uint64_t svt_handle_transform64x16_N2_N4_c(int32_t*);
uint64_t svt_handle_transform64x32_N2_N4_c(int32_t*);
uint64_t svt_handle_transform64x64_N2_N4_c(int32_t*);

/* which: 0=16x64 1=32x64 2=64x16 3=64x32 4=64x64; pf != 0 selects _N2_N4. */
uint64_t ref_handle_transform(int32_t which, int32_t pf, int32_t* output) {
    if (pf) {
        switch (which) {
        case 0: return svt_handle_transform16x64_N2_N4_c(output);
        case 1: return svt_handle_transform32x64_N2_N4_c(output);
        case 2: return svt_handle_transform64x16_N2_N4_c(output);
        case 3: return svt_handle_transform64x32_N2_N4_c(output);
        default: return svt_handle_transform64x64_N2_N4_c(output);
        }
    }
    switch (which) {
    case 0: return svt_handle_transform16x64_c(output);
    case 1: return svt_handle_transform32x64_c(output);
    case 2: return svt_handle_transform64x16_c(output);
    case 3: return svt_handle_transform64x32_c(output);
    default: return svt_handle_transform64x64_c(output);
    }
}

/* ---- svt_aom_fwd_txfm_type_to_func / svt_aom_inv_txfm_type_to_func ----
 *
 * These return a function POINTER, which is not portable to compare across
 * the FFI boundary. Instead the shim CALLS the returned pointer, so the Rust
 * side gates the dispatch table by the behaviour it selects. A NULL return
 * (C's assert-and-return-NULL arm) is reported as 0 without being called.
 */
int32_t ref_call_fwd_txfm_type_to_func(int32_t txfm_type, const int32_t* input, int32_t* output, int8_t cos_bit) {
    const int8_t stage_range[MAX_TXFM_STAGE_NUM] = {0};
    TxfmFunc     f                               = svt_aom_fwd_txfm_type_to_func((TxfmType)txfm_type);
    if (!f) {
        return 0;
    }
    f(input, output, cos_bit, stage_range);
    return 1;
}

int32_t ref_call_inv_txfm_type_to_func(int32_t txfm_type, const int32_t* input, int32_t* output, int8_t cos_bit,
                                       int8_t stage_range_fill) {
    /* The port's inverse kernels take ONE range for the whole kernel, so the
     * only comparable configuration is a uniformly filled stage_range. */
    int8_t stage_range[MAX_TXFM_STAGE_NUM];
    for (int i = 0; i < MAX_TXFM_STAGE_NUM; ++i) stage_range[i] = stage_range_fill;
    TxfmFunc f = svt_aom_inv_txfm_type_to_func((TxfmType)txfm_type);
    if (!f) {
        return 0;
    }
    f(input, output, cos_bit, stage_range);
    return 1;
}

/* ---- svt_av1_get_inv_txfm_cfg ---- */

/* Flattened like ref_transform_config. */
void ref_get_inv_txfm_cfg(int32_t tx_type, int32_t tx_size, int32_t* out) {
    Txfm2dFlipCfg cfg;
    memset(&cfg, 0, sizeof(cfg));
    svt_av1_get_inv_txfm_cfg((TxType)tx_type, (TxSize)tx_size, &cfg);
    out[0] = cfg.ud_flip;
    out[1] = cfg.lr_flip;
    for (int i = 0; i < 3; ++i) out[2 + i] = cfg.shift[i];
    out[5]  = cfg.cos_bit_col;
    out[6]  = cfg.cos_bit_row;
    out[7]  = (int32_t)cfg.txfm_type_col;
    out[8]  = (int32_t)cfg.txfm_type_row;
    out[9]  = cfg.stage_num_col;
    out[10] = cfg.stage_num_row;
    for (int i = 0; i < MAX_TXFM_STAGE_NUM; ++i) {
        out[11 + i] = cfg.stage_range_col[i];
        out[11 + MAX_TXFM_STAGE_NUM + i] = cfg.stage_range_row[i];
    }
}

/* ---- svt_aom_estimate_transform ----
 *
 * The exported entry takes a PictureControlSet* and a ModeDecisionContext*,
 * but reads NOTHING from either except through
 * `svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id)`
 * (mode_decision.c:71 -> ppcs->frm_hdr.segmentation_params.segmentation_enabled
 * and pcs->lossless[]). The shim therefore builds exactly that much state.
 *
 * calloc/free per call, never `static` — cargo runs a test binary's tests on
 * several threads and the `lossless` flag is written per call.
 */
#include "pcs.h"
#include "md_process.h"

EbErrorType svt_aom_estimate_transform(PictureControlSet* pcs, ModeDecisionContext* ctx, int16_t* residual_buffer,
                                       uint32_t residual_stride, int32_t* coeff_buffer, uint32_t coeff_stride,
                                       TxSize transform_size, uint64_t* three_quad_energy, uint32_t bit_depth,
                                       TxType transform_type, PlaneType component_type,
                                       TxCoeffShape trans_coeff_shape);

int32_t ref_estimate_transform(int16_t* residual_buffer, uint32_t residual_stride, int32_t* coeff_buffer,
                               uint32_t coeff_stride, int32_t transform_size, uint64_t* three_quad_energy,
                               uint32_t bit_depth, int32_t transform_type, int32_t component_type,
                               int32_t trans_coeff_shape, int32_t lossless) {
    txfm_pf_ensure_rtcd();
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(PictureControlSet));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(ModeDecisionContext));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(BlkStruct));
    if (!pcs || !ppcs || !ctx || !blk) {
        free(pcs);
        free(ppcs);
        free(ctx);
        free(blk);
        return -1;
    }
    pcs->ppcs                                              = ppcs;
    ppcs->frm_hdr.segmentation_params.segmentation_enabled = 0;
    pcs->lossless[0]                                       = (uint8_t)lossless;
    blk->segment_id                                        = 0;
    ctx->blk_ptr                                           = blk;

    EbErrorType rc = svt_aom_estimate_transform(pcs,
                                                ctx,
                                                residual_buffer,
                                                residual_stride,
                                                coeff_buffer,
                                                coeff_stride,
                                                (TxSize)transform_size,
                                                three_quad_energy,
                                                bit_depth,
                                                (TxType)transform_type,
                                                (PlaneType)component_type,
                                                (TxCoeffShape)trans_coeff_shape);
    free(blk);
    free(ctx);
    free(ppcs);
    free(pcs);
    return (int32_t)rc;
}
