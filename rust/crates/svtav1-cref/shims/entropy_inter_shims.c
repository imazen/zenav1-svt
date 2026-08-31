/*
 * C shims for the INTER BITSTREAM-SYNTAX differential
 * (wholesale port of Codec/entropy_coding.c's inter group).
 *
 * Its own translation unit, like inter_mvp_shims.c and inter_me_shims.c, so
 * this lane never shares an editable file with a concurrent lane.
 *
 * Every entry point drives a REAL exported SVT-AV1 symbol (evidence tier 1,
 * docs/WORKING-ON-THIS.md section 4). The exported symbols reached here:
 *   svt_aom_collect_neighbors_ref_counts_new    entropy_coding.c:1877
 *   svt_av1_get_intra_inter_context             entropy_coding.c:1127
 *   av1_get_skip_mode_context                   entropy_coding.c:1097
 *   svt_aom_get_reference_mode_context_new      entropy_coding.c:1833
 *   svt_aom_get_comp_reference_type_context_new entropy_coding.c:1695
 *   svt_av1_get_pred_context_uni_comp_ref_p{,1,2}   :1774/:1797/:1819
 *   svt_av1_get_pred_context_comp_ref_p{,1,2}       :1992/:1999/:2006
 *   svt_av1_get_pred_context_comp_bwdref_p{,1}      :2012/:2018
 *   svt_av1_get_pred_context_single_ref_p{1..6}     :2026..:2092
 *   svt_aom_get_reference_mode_cdf              entropy_coding.c:1636
 *   svt_aom_get_comp_reference_type_cdf         entropy_coding.c:1650
 *   svt_aom_get_pred_cdf_uni_comp_ref_p{,1,2}       :1655/:1660/:1665
 *   svt_aom_get_pred_cdf_comp_ref_p{,1,2}           :1670/:1675/:1680
 *   svt_aom_get_pred_cdf_comp_bwdref_p{,1}          :1685/:1690
 *   svt_aom_get_pred_cdf_single_ref_p{1..6}         :2041..:2061
 *   svt_aom_get_comp_index_context_enc          entropy_coding.c:52
 *   svt_aom_get_comp_group_idx_context_enc      entropy_coding.c:80
 *   svt_aom_get_pred_context_switchable_interp  entropy_coding.c:1527
 *   svt_aom_is_nontrans_global_motion           entropy_coding.c:1572
 *   svt_aom_is_interintra_allowed               entropy_coding.c:4927
 *   svt_aom_motion_mode_allowed                 entropy_coding.c:1159
 *   svt_aom_wb_write_signed_primitive_refsubexpfin  entropy_coding.c:2989
 *   svt_aom_init_mode_probs                     cabac_context_model.c:740
 *
 * Shim state is per-call and on the stack or freshly heap-allocated (never a
 * `static` buffer): cargo runs a test binary's tests on several threads.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "block_structures.h"
#include "coding_unit.h"
#include "cabac_context_model.h"
#include "entropy_coding.h"
#include "common_utils.h"
#include "filter.h"
#include "inter_prediction.h"
#include "mode_decision.h"
#include "av1_common.h"
#include "pcs.h"
#include "sys_resource_manager.h"

/* ---- the exported declarations this TU drives ---- */
void        svt_aom_collect_neighbors_ref_counts_new(MacroBlockD* const xd);
uint8_t     svt_av1_get_intra_inter_context(const MacroBlockD* xd);
uint8_t     av1_get_skip_mode_context(const MacroBlockD* xd);
int         svt_aom_get_reference_mode_context_new(const MacroBlockD* xd);
int         svt_aom_get_comp_reference_type_context_new(const MacroBlockD* xd);
int         svt_av1_get_pred_context_uni_comp_ref_p(const MacroBlockD* xd);
int         svt_av1_get_pred_context_uni_comp_ref_p1(const MacroBlockD* xd);
int         svt_av1_get_pred_context_uni_comp_ref_p2(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_comp_ref_p(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_comp_ref_p1(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_comp_ref_p2(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_comp_bwdref_p(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_comp_bwdref_p1(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p1(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p2(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p3(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p4(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p5(const MacroBlockD* xd);
int32_t     svt_av1_get_pred_context_single_ref_p6(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_reference_mode_cdf(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_comp_reference_type_cdf(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p3(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p4(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p5(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p6(const MacroBlockD* xd);
int         svt_aom_get_comp_index_context_enc(PictureParentControlSet* pcs, int cur_frame_index, int bck_frame_index,
                                               int fwd_frame_index, const MacroBlockD* xd);
int         svt_aom_get_comp_group_idx_context_enc(const MacroBlockD* xd);
int         svt_aom_get_pred_context_switchable_interp(MvReferenceFrame rf0, MvReferenceFrame rf1,
                                                       const MacroBlockD* xd, int dir);
int         svt_aom_is_nontrans_global_motion(const BlockModeInfo* block_mi, const BlockSize bsize,
                                              PictureParentControlSet* pcs);
int         svt_aom_is_interintra_allowed(const MbModeInfo* mbmi);
MotionMode  svt_aom_motion_mode_allowed(const PictureControlSet* pcs, uint16_t num_proj_ref,
                                        uint32_t overlappable_neighbors, const BlockSize bsize, MvReferenceFrame rf0,
                                        MvReferenceFrame rf1, PredictionMode mode);
void        svt_aom_wb_write_signed_primitive_refsubexpfin(AomWriteBitBuffer* wb, uint16_t n, uint16_t k, int16_t ref,
                                                           int16_t v);
void        svt_aom_init_mode_probs(FRAME_CONTEXT* fc);

/* `svt_aom_init_mode_probs` copies every default CDF table with `COPY_CDF`,
 * which is `svt_memcpy` (cabac_context_model.c:735) — an RTCD FUNCTION POINTER
 * in .bss (common_dsp_rtcd.h:1083), NULL until
 * `svt_aom_setup_common_rtcd_internal` runs. The same file uses the null-safe
 * `SVT_MEMCPY` at :1923, so the bare spelling at :735 is a call site that
 * assumes setup already happened. On aarch64 the hazard cannot fire: NEON
 * devirtualization rewrites `svt_memcpy` to the concrete `svt_memcpy_neon`
 * (common_dsp_rtcd_neon_devirt.h:266). On x86-64 the call lands at rip=0x0.
 * MEASURED 2026-08-31: without this, all 7 tests in
 * `c_parity_entropy_inter.rs` SIGSEGV on x86_64-linux and pass on
 * aarch64-darwin. `g_ec_rtcd_ready` is an idempotent one-shot, not per-call
 * state — a racing double-init lands the same pointers. */
typedef uint64_t EcCpuFlags;
EcCpuFlags svt_aom_get_cpu_flags_to_use(void);
void       svt_aom_setup_common_rtcd_internal(uint64_t flags);

static int  g_ec_rtcd_ready = 0;
static void ec_init_mode_probs(FRAME_CONTEXT* fc) {
    if (!g_ec_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_ec_rtcd_ready = 1;
    }
    svt_aom_init_mode_probs(fc);
}

/* ---- neighbour description ----
 * One neighbour is 10 int32s so the Rust side can build the exact
 * BlockModeInfo fields every context function reads:
 *   [0] valid (0 -> the xd->{above,left}_mbmi POINTER is NULL)
 *   [1] mode  [2] ref_frame[0]  [3] ref_frame[1]  [4] interp_filters
 *   [5] use_intrabc  [6] skip_mode  [7] comp_group_idx  [8] compound_idx
 *   [9] bsize
 * `valid` and up/left_available are DISTINCT knobs on purpose: C's
 * av1_get_skip_mode_context / comp_index / comp_group_idx test the POINTER,
 * while the ref-count and reference-mode contexts test up_available /
 * left_available. Conflating them is a real divergence.
 */
#define EC_NB_FIELDS 10

static void ec_fill_mbmi(MbModeInfo* dst, const int32_t* f) {
    memset(dst, 0, sizeof(*dst));
    dst->block_mi.mode            = (PredictionMode)f[1];
    dst->block_mi.ref_frame[0]    = (MvReferenceFrame)f[2];
    dst->block_mi.ref_frame[1]    = (MvReferenceFrame)f[3];
    dst->block_mi.interp_filters  = (uint32_t)f[4];
    dst->block_mi.use_intrabc     = (uint8_t)(f[5] & 1);
    dst->block_mi.skip_mode       = (uint8_t)(f[6] & 1);
    dst->block_mi.comp_group_idx  = (uint8_t)(f[7] & 1);
    dst->block_mi.compound_idx    = (uint8_t)(f[8] & 1);
    dst->bsize                    = (BlockSize)f[9];
}

/* xd plus its owned storage; every call builds its own (no static state). */
typedef struct EcXd {
    MacroBlockD   xd;
    MbModeInfo    above;
    MbModeInfo    left;
    MbModeInfo    cur;
    MbModeInfo*   grid[3]; /* [0] = left (mi[-1]), [1] = cur (mi[0]), [2] = above */
    FRAME_CONTEXT fc;
} EcXd;

/* `svt_aom_get_pred_context_switchable_interp` reads xd->mi[-1] (left) and
 * xd->mi[-xd->mi_stride] (above) rather than the above_mbmi/left_mbmi
 * pointers, so the grid is laid out [above, left, cur] with mi = &grid[2] and
 * mi_stride = 2: mi[-1] == left, mi[-2] == above. */
static void ec_build_xd(EcXd* s, const int32_t* above, const int32_t* left, int up_avail, int left_avail) {
    memset(s, 0, sizeof(*s));
    ec_fill_mbmi(&s->above, above);
    ec_fill_mbmi(&s->left, left);
    memset(&s->cur, 0, sizeof(s->cur));
    s->grid[0]           = &s->above;
    s->grid[1]           = &s->left;
    s->grid[2]           = &s->cur;
    s->xd.mi             = &s->grid[2];
    s->xd.mi_stride      = 2;
    s->xd.up_available   = (int8_t)(up_avail != 0);
    s->xd.left_available = (int8_t)(left_avail != 0);
    s->xd.above_mbmi     = above[0] ? &s->above : NULL;
    s->xd.left_mbmi      = left[0] ? &s->left : NULL;
    ec_init_mode_probs(&s->fc);
    s->xd.tile_ctx = &s->fc;
}

/* Flat row index of `row` inside a CDF_SIZE(2) table whose base is `base`. */
static int32_t ec_row2(const AomCdfProb* base, const AomCdfProb* row) {
    return (int32_t)((size_t)(row - base) / CDF_SIZE(2));
}

/* ---- 1. neighbour ref counts (svt_aom_collect_neighbors_ref_counts_new) ---- */
void ref_ec_collect_neighbors_ref_counts(const int32_t* above, const int32_t* left, int32_t up_avail,
                                         int32_t left_avail, uint8_t* out8) {
    EcXd s;
    ec_build_xd(&s, above, left, up_avail, left_avail);
    svt_aom_collect_neighbors_ref_counts_new(&s.xd);
    memcpy(out8, s.xd.neighbors_ref_counts, TOTAL_REFS_PER_FRAME);
}

/* ---- 2. every ref-frame prediction CONTEXT, in one call ----
 * out[0..5]   single_ref p1..p6
 * out[6..8]   comp_ref p, p1, p2
 * out[9..10]  comp_bwdref p, p1
 * out[11..13] uni_comp_ref p, p1, p2
 * out[14]     reference_mode (comp_inter)
 * out[15]     comp_reference_type
 * out[16]     intra_inter
 * out[17]     skip_mode
 * out[18]     comp_group_idx
 */
#define EC_N_CTX 19
void ref_ec_ref_contexts(const int32_t* above, const int32_t* left, int32_t up_avail, int32_t left_avail,
                         int32_t* out) {
    EcXd s;
    ec_build_xd(&s, above, left, up_avail, left_avail);
    svt_aom_collect_neighbors_ref_counts_new(&s.xd);
    out[0]  = svt_av1_get_pred_context_single_ref_p1(&s.xd);
    out[1]  = svt_av1_get_pred_context_single_ref_p2(&s.xd);
    out[2]  = svt_av1_get_pred_context_single_ref_p3(&s.xd);
    out[3]  = svt_av1_get_pred_context_single_ref_p4(&s.xd);
    out[4]  = svt_av1_get_pred_context_single_ref_p5(&s.xd);
    out[5]  = svt_av1_get_pred_context_single_ref_p6(&s.xd);
    out[6]  = svt_av1_get_pred_context_comp_ref_p(&s.xd);
    out[7]  = svt_av1_get_pred_context_comp_ref_p1(&s.xd);
    out[8]  = svt_av1_get_pred_context_comp_ref_p2(&s.xd);
    out[9]  = svt_av1_get_pred_context_comp_bwdref_p(&s.xd);
    out[10] = svt_av1_get_pred_context_comp_bwdref_p1(&s.xd);
    out[11] = svt_av1_get_pred_context_uni_comp_ref_p(&s.xd);
    out[12] = svt_av1_get_pred_context_uni_comp_ref_p1(&s.xd);
    out[13] = svt_av1_get_pred_context_uni_comp_ref_p2(&s.xd);
    out[14] = svt_aom_get_reference_mode_context_new(&s.xd);
    out[15] = svt_aom_get_comp_reference_type_context_new(&s.xd);
    out[16] = (int32_t)svt_av1_get_intra_inter_context(&s.xd);
    out[17] = (int32_t)av1_get_skip_mode_context(&s.xd);
    out[18] = svt_aom_get_comp_group_idx_context_enc(&s.xd);
}

/* ---- 3. the CDF SELECTORS, as flat row indices into their tables ----
 * Same order as ref_ec_ref_contexts for entries 0..15 (the selectors that
 * exist); the row index is what WRITE_REF_BIT actually dispatches on, so
 * this pins BOTH the context derivation and the [ctx][slot] indexing.
 */
void ref_ec_cdf_rows(const int32_t* above, const int32_t* left, int32_t up_avail, int32_t left_avail, int32_t* out) {
    EcXd s;
    ec_build_xd(&s, above, left, up_avail, left_avail);
    svt_aom_collect_neighbors_ref_counts_new(&s.xd);
    const AomCdfProb* sr = &s.fc.single_ref_cdf[0][0][0];
    const AomCdfProb* cr = &s.fc.comp_ref_cdf[0][0][0];
    const AomCdfProb* cb = &s.fc.comp_bwdref_cdf[0][0][0];
    const AomCdfProb* uc = &s.fc.uni_comp_ref_cdf[0][0][0];
    out[0]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p1(&s.xd));
    out[1]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p2(&s.xd));
    out[2]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p3(&s.xd));
    out[3]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p4(&s.xd));
    out[4]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p5(&s.xd));
    out[5]  = ec_row2(sr, svt_aom_get_pred_cdf_single_ref_p6(&s.xd));
    out[6]  = ec_row2(cr, svt_aom_get_pred_cdf_comp_ref_p(&s.xd));
    out[7]  = ec_row2(cr, svt_aom_get_pred_cdf_comp_ref_p1(&s.xd));
    out[8]  = ec_row2(cr, svt_aom_get_pred_cdf_comp_ref_p2(&s.xd));
    out[9]  = ec_row2(cb, svt_aom_get_pred_cdf_comp_bwdref_p(&s.xd));
    out[10] = ec_row2(cb, svt_aom_get_pred_cdf_comp_bwdref_p1(&s.xd));
    out[11] = ec_row2(uc, svt_aom_get_pred_cdf_uni_comp_ref_p(&s.xd));
    out[12] = ec_row2(uc, svt_aom_get_pred_cdf_uni_comp_ref_p1(&s.xd));
    out[13] = ec_row2(uc, svt_aom_get_pred_cdf_uni_comp_ref_p2(&s.xd));
    out[14] = ec_row2(&s.fc.comp_inter_cdf[0][0], svt_aom_get_reference_mode_cdf(&s.xd));
    out[15] = ec_row2(&s.fc.comp_ref_type_cdf[0][0], svt_aom_get_comp_reference_type_cdf(&s.xd));
}

/* ---- 4. compound index context (needs the seq header's order-hint info) ---- */
int32_t ref_ec_comp_index_context(int32_t enable_order_hint, int32_t order_hint_bits, int32_t cur_frame_index,
                                  int32_t bck_frame_index, int32_t fwd_frame_index, const int32_t* above,
                                  const int32_t* left) {
    EcXd s;
    ec_build_xd(&s, above, left, 1, 1);
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    ppcs->scs                                       = scs;
    scs->seq_header.order_hint_info.enable_order_hint = (uint8_t)enable_order_hint;
    scs->seq_header.order_hint_info.order_hint_bits  = (uint8_t)order_hint_bits;
    const int32_t r = svt_aom_get_comp_index_context_enc(ppcs, cur_frame_index, bck_frame_index, fwd_frame_index,
                                                         &s.xd);
    free(scs);
    free(ppcs);
    return r;
}

/* ---- 5. switchable interpolation-filter context ---- */
int32_t ref_ec_switchable_interp_context(int32_t rf0, int32_t rf1, int32_t dir, const int32_t* above,
                                         const int32_t* left, int32_t up_avail, int32_t left_avail) {
    EcXd s;
    ec_build_xd(&s, above, left, up_avail, left_avail);
    return svt_aom_get_pred_context_switchable_interp((MvReferenceFrame)rf0, (MvReferenceFrame)rf1, &s.xd, dir);
}

/* ---- 6. non-translational global motion gate ---- */
int32_t ref_ec_is_nontrans_global_motion(int32_t mode, int32_t bsize, int32_t rf0, int32_t rf1,
                                         const int32_t* gm_wmtype /* TOTAL_REFS_PER_FRAME */) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; ++i) {
        ppcs->global_motion[i].wmtype = (TransformationType)gm_wmtype[i];
    }
    BlockModeInfo bmi;
    memset(&bmi, 0, sizeof(bmi));
    bmi.mode         = (PredictionMode)mode;
    bmi.ref_frame[0] = (MvReferenceFrame)rf0;
    bmi.ref_frame[1] = (MvReferenceFrame)rf1;
    const int32_t r  = svt_aom_is_nontrans_global_motion(&bmi, (BlockSize)bsize, ppcs);
    free(ppcs);
    return r;
}

/* ---- 7. interintra gate ---- */
int32_t ref_ec_is_interintra_allowed(int32_t bsize, int32_t mode, int32_t rf0, int32_t rf1) {
    MbModeInfo mbmi;
    memset(&mbmi, 0, sizeof(mbmi));
    mbmi.bsize                = (BlockSize)bsize;
    mbmi.block_mi.mode        = (PredictionMode)mode;
    mbmi.block_mi.ref_frame[0] = (MvReferenceFrame)rf0;
    mbmi.block_mi.ref_frame[1] = (MvReferenceFrame)rf1;
    return svt_aom_is_interintra_allowed(&mbmi);
}

/* ---- 8. motion-mode gate ---- */
int32_t ref_ec_motion_mode_allowed(int32_t is_motion_mode_switchable, int32_t force_integer_mv,
                                   int32_t allow_warped_motion, const int32_t* gm_wmtype, int32_t num_proj_ref,
                                   int32_t overlappable_neighbors, int32_t bsize, int32_t rf0, int32_t rf1,
                                   int32_t mode) {
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    pcs->ppcs                             = ppcs;
    ppcs->frm_hdr.is_motion_mode_switchable = (uint8_t)is_motion_mode_switchable;
    ppcs->frm_hdr.force_integer_mv          = (uint8_t)force_integer_mv;
    ppcs->frm_hdr.allow_warped_motion       = (uint8_t)allow_warped_motion;
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; ++i) {
        ppcs->global_motion[i].wmtype = (TransformationType)gm_wmtype[i];
    }
    const MotionMode r = svt_aom_motion_mode_allowed(pcs, (uint16_t)num_proj_ref, (uint32_t)overlappable_neighbors,
                                                     (BlockSize)bsize, (MvReferenceFrame)rf0, (MvReferenceFrame)rf1,
                                                     (PredictionMode)mode);
    free(pcs);
    free(ppcs);
    return (int32_t)r;
}

/* ---- 9. the header bit-buffer signed recentred subexponential code ----
 * Drives svt_aom_wb_write_signed_primitive_refsubexpfin, which is the entry
 * point every global-motion coefficient goes through; the whole aom_wb_*
 * primitive stack (refsubexpfin -> subexpfin -> quniform) is underneath it.
 * Returns the bit count; `out` receives the bytes (MSB-first, as C writes).
 */
int32_t ref_ec_wb_signed_refsubexpfin(int32_t n, int32_t k, int32_t ref, int32_t v, uint8_t* out, int32_t out_cap) {
    uint8_t*          buf = (uint8_t*)calloc((size_t)out_cap, 1);
    AomWriteBitBuffer wb  = {buf, 0};
    svt_aom_wb_write_signed_primitive_refsubexpfin(&wb, (uint16_t)n, (uint16_t)k, (int16_t)ref, (int16_t)v);
    const int32_t bits = (int32_t)wb.bit_offset;
    memcpy(out, buf, (size_t)((bits + 7) / 8));
    free(buf);
    return bits;
}

/* ---- 10. default FRAME_CONTEXT tables this lane needs ----
 * The mainline defaults, taken from the REAL svt_aom_init_mode_probs, so the
 * port's tables are gated against C rather than against a transcription.
 */
#define EC_FC_TABLE(name, field)                                                 \
    size_t ref_ec_sizeof_##name(void) {                                          \
        FRAME_CONTEXT fc;                                                        \
        return sizeof(fc.field);                                                 \
    }                                                                            \
    void ref_ec_copy_##name(uint16_t* dst) {                                     \
        FRAME_CONTEXT fc;                                                        \
        ec_init_mode_probs(&fc);                                            \
        memcpy(dst, &fc.field, sizeof(fc.field));                                \
    }

EC_FC_TABLE(comp_ref_type_cdf, comp_ref_type_cdf)
EC_FC_TABLE(uni_comp_ref_cdf, uni_comp_ref_cdf)
EC_FC_TABLE(comp_bwdref_cdf, comp_bwdref_cdf)
EC_FC_TABLE(single_ref_cdf, single_ref_cdf)
EC_FC_TABLE(comp_ref_cdf, comp_ref_cdf)
EC_FC_TABLE(comp_inter_cdf, comp_inter_cdf)
EC_FC_TABLE(skip_mode_cdfs, skip_mode_cdfs)
EC_FC_TABLE(newmv_cdf, newmv_cdf)
EC_FC_TABLE(zeromv_cdf, zeromv_cdf)
EC_FC_TABLE(refmv_cdf, refmv_cdf)
EC_FC_TABLE(drl_cdf, drl_cdf)
EC_FC_TABLE(inter_compound_mode_cdf, inter_compound_mode_cdf)
EC_FC_TABLE(switchable_interp_cdf, switchable_interp_cdf)
EC_FC_TABLE(motion_mode_cdf, motion_mode_cdf)
EC_FC_TABLE(obmc_cdf, obmc_cdf)
EC_FC_TABLE(compound_index_cdf, compound_index_cdf)
EC_FC_TABLE(comp_group_idx_cdf, comp_group_idx_cdf)
EC_FC_TABLE(interintra_cdf, interintra_cdf)
EC_FC_TABLE(interintra_mode_cdf, interintra_mode_cdf)
EC_FC_TABLE(wedge_interintra_cdf, wedge_interintra_cdf)
EC_FC_TABLE(wedge_idx_cdf, wedge_idx_cdf)
EC_FC_TABLE(compound_type_cdf, compound_type_cdf)
