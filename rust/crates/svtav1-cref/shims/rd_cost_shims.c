/*
 * C shims for the RD-COST wholesale port (lane wx-md: Codec/rd_cost.c).
 *
 * Its OWN translation unit so this lane never shares an editable file with
 * the concurrent mode-decision / inter-MVP / entropy lanes.
 *
 * Every entry point here drives a REAL exported SVT-AV1 symbol — evidence
 * tier 1, docs/WORKING-ON-THIS.md section 4. Linkage was re-checked with
 * `nm -g Bin/Release/libSvtAv1Enc.a` rather than inferred from the header or
 * the `svt_aom_` prefix; rd_cost.c has BOTH traps (an unprefixed export,
 * `get_eob_cost`, and prefixed `static`s such as `get_compound_mode_rate`).
 *
 *   svt_aom_get_switchable_rate            rd_cost.c:849
 *   svt_aom_inter_fast_cost                rd_cost.c:1005
 *     -> reaches the two `static`s this lane also ports:
 *        av1_inter_fast_cost_light (:870) via approx_inter_rate,
 *        get_compound_mode_rate    (:783) unconditionally
 *   svt_aom_intra_fast_cost                rd_cost.c:526
 *   svt_aom_get_intra_uv_fast_rate         rd_cost.c:476
 *   svt_aom_full_cost                      rd_cost.c:1349
 *   svt_aom_full_cost_pd0                  rd_cost.c:1330
 *
 * State discipline: every shim keeps its scratch on the STACK or in a
 * per-call calloc. cargo/nextest runs a test binary's tests on several
 * threads and a file-scope buffer would race.
 *
 * The neighbour encoding (10 int32s per neighbour, `valid` separate from
 * up/left_available) is deliberately the SAME shape entropy_inter_shims.c
 * uses, because the contexts these cost functions call are the same ones.
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
#include "common_utils.h"
#include "entropy_coding.h"
#include "filter.h"
#include "inter_prediction.h"
#include "md_process.h"
#include "mode_decision.h"
#include "md_rate_estimation.h"
#include "rd_cost.h"
#include "av1_common.h"
#include "pcs.h"
#include "utility.h"
#include "sys_resource_manager.h"

/* ---- the exported declarations this TU drives ---- */
int32_t     svt_aom_get_switchable_rate(BlockModeInfo* block_mi, const FrameHeader* const frm_hdr,
                                        ModeDecisionContext* ctx, const bool enable_dual_filter);
uint64_t    svt_aom_inter_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                    ModeDecisionCandidateBuffer* cand_bf, uint64_t lambda,
                                    uint64_t luma_distortion);
uint64_t    svt_aom_intra_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                    ModeDecisionCandidateBuffer* cand_bf, uint64_t lambda,
                                    uint64_t luma_distortion);
uint64_t    svt_aom_get_intra_uv_fast_rate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                           ModeDecisionCandidateBuffer* cand_bf, bool use_accurate_cfl);
EbErrorType svt_aom_full_cost_pd0(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                  uint64_t* y_distortion, uint64_t lambda, uint64_t* y_coeff_bits);
void        svt_aom_full_cost(PictureControlSet* pcs, ModeDecisionContext* ctx,
                              ModeDecisionCandidateBuffer* cand_bf, uint64_t lambda,
                              uint64_t y_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cb_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cr_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* y_coeff_bits,
                              uint64_t* cb_coeff_bits, uint64_t* cr_coeff_bits);

/* ------------------------------------------------------------------ *
 * Scene assembly.
 * ------------------------------------------------------------------ */

#define RD_NB_FIELDS 10

static void rd_fill_mbmi(MbModeInfo* dst, const int32_t* f) {
    memset(dst, 0, sizeof(*dst));
    dst->block_mi.mode           = (PredictionMode)f[1];
    dst->block_mi.ref_frame[0]   = (MvReferenceFrame)f[2];
    dst->block_mi.ref_frame[1]   = (MvReferenceFrame)f[3];
    dst->block_mi.interp_filters = (uint32_t)f[4];
    dst->block_mi.use_intrabc    = (uint8_t)(f[5] & 1);
    dst->block_mi.skip_mode      = (uint8_t)(f[6] & 1);
    dst->block_mi.comp_group_idx = (uint8_t)(f[7] & 1);
    dst->block_mi.compound_idx   = (uint8_t)(f[8] & 1);
    dst->bsize                   = (BlockSize)f[9];
}

/* The whole MD world one cost call needs, all owned by the caller's frame. */
typedef struct RdScene {
    PictureControlSet*       pcs;
    PictureParentControlSet* ppcs;
    SequenceControlSet*      scs;
    ModeDecisionContext*     ctx;
    ModeDecisionCandidateBuffer* cand_bf;
    ModeDecisionCandidate*   cand;
    MdRateEstimationContext* rates;
    BlkStruct*               blk;
    MacroBlockD*             xd;
    BlockGeom*               geom;
    Av1Common*               cm;
    MbModeInfo               above;
    MbModeInfo               left;
    MbModeInfo               cur;
    MbModeInfo*              grid[3];
    uint64_t                 full_cost;
    uint64_t                 full_cost_ssim;
} RdScene;

/* mi grid laid out [above, left, cur] with mi = &grid[2], mi_stride = 2, so
 * mi[-1] == left and mi[-2] == above — the layout
 * svt_aom_get_pred_context_switchable_interp indexes. */
static void rd_scene_init(RdScene* s, const int32_t* above, const int32_t* left, int up_avail, int left_avail) {
    memset(s, 0, sizeof(*s));
    s->pcs     = (PictureControlSet*)calloc(1, sizeof(*s->pcs));
    s->ppcs    = (PictureParentControlSet*)calloc(1, sizeof(*s->ppcs));
    s->scs     = (SequenceControlSet*)calloc(1, sizeof(*s->scs));
    s->ctx     = (ModeDecisionContext*)calloc(1, sizeof(*s->ctx));
    s->cand_bf = (ModeDecisionCandidateBuffer*)calloc(1, sizeof(*s->cand_bf));
    s->cand    = (ModeDecisionCandidate*)calloc(1, sizeof(*s->cand));
    s->rates   = (MdRateEstimationContext*)calloc(1, sizeof(*s->rates));
    s->blk     = (BlkStruct*)calloc(1, sizeof(*s->blk));
    s->xd      = (MacroBlockD*)calloc(1, sizeof(*s->xd));
    s->geom    = (BlockGeom*)calloc(1, sizeof(*s->geom));
    s->cm      = (Av1Common*)calloc(1, sizeof(*s->cm));

    rd_fill_mbmi(&s->above, above);
    rd_fill_mbmi(&s->left, left);
    memset(&s->cur, 0, sizeof(s->cur));
    s->grid[0] = &s->above;
    s->grid[1] = &s->left;
    s->grid[2] = &s->cur;

    s->xd->mi             = &s->grid[2];
    s->xd->mi_stride      = 2;
    s->xd->up_available   = (int8_t)(up_avail != 0);
    s->xd->left_available = (int8_t)(left_avail != 0);
    s->xd->above_mbmi     = above[0] ? &s->above : NULL;
    s->xd->left_mbmi      = left[0] ? &s->left : NULL;

    s->pcs->ppcs        = s->ppcs;
    s->pcs->scs         = s->scs;
    s->ppcs->scs        = s->scs;
    s->ppcs->av1_cm     = s->cm;
    s->blk->av1xd       = s->xd;
    s->ctx->blk_ptr     = s->blk;
    s->ctx->blk_geom    = s->geom;
    s->ctx->md_rate_est_ctx = s->rates;
    s->cand_bf->cand    = s->cand;
    s->cand_bf->full_cost      = &s->full_cost;
    s->cand_bf->full_cost_ssim = &s->full_cost_ssim;
}

static void rd_scene_free(RdScene* s) {
    free(s->cm);
    free(s->geom);
    free(s->xd);
    free(s->blk);
    free(s->rates);
    free(s->cand);
    free(s->cand_bf);
    free(s->ctx);
    free(s->scs);
    free(s->ppcs);
    free(s->pcs);
}

/* Set the block geometry from a BlockSize, the way `svt_aom_get_blk_geom_mds`
 * would; only the four fields these cost functions read. */
static void rd_set_geom(RdScene* s, int32_t bsize) {
    s->geom->bsize      = (BlockSize)bsize;
    s->geom->bwidth     = block_size_wide[bsize];
    s->geom->bheight    = block_size_high[bsize];
    s->geom->bwidth_uv  = (uint8_t)(block_size_wide[bsize] >> 1);
    s->geom->bheight_uv = (uint8_t)(block_size_high[bsize] >> 1);
}

/* ------------------------------------------------------------------ *
 * 1. svt_aom_get_switchable_rate (rd_cost.c:849)
 * ------------------------------------------------------------------ */

/* `tbl` is SWITCHABLE_FILTER_CONTEXTS * SWITCHABLE_FILTERS int32s, row-major
 * — exactly `switchable_interp_fac_bitss`. */
int32_t ref_rd_switchable_rate(int32_t interp_filter, int32_t rf0, int32_t rf1, int32_t interp_filters,
                               const int32_t* above, const int32_t* left, int32_t up_avail, int32_t left_avail,
                               int32_t enable_dual_filter, const int32_t* tbl) {
    RdScene s;
    rd_scene_init(&s, above, left, up_avail, left_avail);
    s.ppcs->frm_hdr.interpolation_filter = (InterpFilter)interp_filter;
    memcpy(s.rates->switchable_interp_fac_bitss, tbl,
           sizeof(s.rates->switchable_interp_fac_bitss));

    BlockModeInfo bmi;
    memset(&bmi, 0, sizeof(bmi));
    bmi.ref_frame[0]   = (MvReferenceFrame)rf0;
    bmi.ref_frame[1]   = (MvReferenceFrame)rf1;
    bmi.interp_filters = (uint32_t)interp_filters;

    const int32_t r = svt_aom_get_switchable_rate(&bmi, &s.ppcs->frm_hdr, s.ctx,
                                                  enable_dual_filter != 0);
    rd_scene_free(&s);
    return r;
}

/* ------------------------------------------------------------------ *
 * 2. svt_aom_inter_fast_cost (rd_cost.c:1005)
 * ------------------------------------------------------------------ */

/* Flat description of one inter candidate + its block/frame context.
 * `i` carries the integers, `t` the whole MdRateEstimationContext as raw
 * bytes so the Rust side can hand over exactly the tables it priced with
 * (a per-field copy list here would be a second place to keep in sync).
 *
 * i[] layout — kept flat rather than a struct so the Rust declaration is a
 * plain `*const i32` and the two sides cannot disagree about padding:
 *   0 bsize            1 mode              2 rf0              3 rf1
 *   4 mv0.x            5 mv0.y             6 mv1.x            7 mv1.y
 *   8 pred0.x          9 pred0.y          10 pred1.x         11 pred1.y
 *  12 drl_index       13 interp_filters   14 motion_mode     15 num_proj_ref
 *  16 is_interintra   17 interintra_mode  18 use_wedge_ii    19 ii_wedge_idx
 *  20 comp_group_idx  21 compound_idx     22 interinter_type 23 interinter_wedge
 *  24 skip_mode_allowed                   25 skip_mode_ctx   26 is_inter_ctx
 *  27 inter_mode_ctx  28 ref_mv_count     29 approx_inter_rate
 *  30 ifs_at_mds0     31 interp_filter_hdr
 *  32 skip_mode_flag  33 is_motion_mode_switchable            34 force_integer_mv
 *  35 allow_warped_motion                 36 enable_dual_filter
 *  37 enable_masked_compound              38 enable_jnt_comp
 *  39 enable_interintra_compound          40 enable_order_hint
 *  41 order_hint_bits 42 cur_order_hint   43 allow_screen_content_tools
 *  44 up_avail        45 left_avail       46 overlappable_neighbors
 */
#define RD_IFC_FIELDS 47

int32_t ref_rd_inter_fast_cost_fields(void) { return RD_IFC_FIELDS; }
int32_t ref_rd_nb_fields(void) { return RD_NB_FIELDS; }

/* Scatter a flat int32 array into the named MdRateEstimationContext fields.
 * The ORDER below is the contract the Rust side mirrors; a blob memcpy of the
 * whole struct would instead make both sides depend on C's field OFFSETS,
 * which is exactly the ABI coupling `-DNDEBUG` incidents are made of. */
static int32_t rd_scatter_inter(MdRateEstimationContext* r, const int32_t* p) {
    const int32_t* c = p;
#define TAKE(field)                                     \
    do {                                                \
        memcpy((field), c, sizeof(field));              \
        c += (int32_t)(sizeof(field) / sizeof(int32_t));\
    } while (0)
    TAKE(r->skip_mode_fac_bits);
    TAKE(r->intra_inter_fac_bits);
    TAKE(r->new_mv_mode_fac_bits);
    TAKE(r->zero_mv_mode_fac_bits);
    TAKE(r->ref_mv_mode_fac_bits);
    TAKE(r->drl_mode_fac_bits);
    TAKE(r->inter_compound_mode_fac_bits);
    TAKE(r->switchable_interp_fac_bitss);
    TAKE(r->motion_mode_fac_bits);
    TAKE(r->motion_mode_fac_bits1);
    TAKE(r->inter_intra_fac_bits);
    TAKE(r->inter_intra_mode_fac_bits);
    TAKE(r->wedge_inter_intra_fac_bits);
    TAKE(r->wedge_idx_fac_bits);
    TAKE(r->comp_group_idx_fac_bits);
    TAKE(r->comp_idx_fac_bits);
    TAKE(r->compound_type_fac_bits);
#undef TAKE
    return (int32_t)(c - p);
}

static int32_t rd_scatter_intra(MdRateEstimationContext* r, const int32_t* p) {
    const int32_t* c = p;
#define TAKE(field)                                     \
    do {                                                \
        memcpy((field), c, sizeof(field));              \
        c += (int32_t)(sizeof(field) / sizeof(int32_t));\
    } while (0)
    TAKE(r->y_mode_fac_bits);
    TAKE(r->mb_mode_fac_bits);
    TAKE(r->intra_uv_mode_fac_bits);
    TAKE(r->angle_delta_fac_bits);
    TAKE(r->cfl_alpha_fac_bits);
    TAKE(r->filter_intra_fac_bits);
    TAKE(r->filter_intra_mode_fac_bits);
    TAKE(r->palette_ymode_fac_bits);
    TAKE(r->palette_uv_mode_fac_bits);
    TAKE(r->intra_inter_fac_bits);
    TAKE(r->skip_mode_fac_bits);
    TAKE(r->intrabc_fac_bits);
#undef TAKE
    return (int32_t)(c - p);
}

/* Element counts, so the Rust side can assert its own layout matches. */
int32_t ref_rd_inter_table_len(void) {
    MdRateEstimationContext* r = (MdRateEstimationContext*)calloc(1, sizeof(*r));
    static int32_t zero[4096];
    const int32_t  n = rd_scatter_inter(r, zero);
    free(r);
    return n;
}
int32_t ref_rd_intra_table_len(void) {
    MdRateEstimationContext* r = (MdRateEstimationContext*)calloc(1, sizeof(*r));
    static int32_t zero[4096];
    const int32_t  n = rd_scatter_intra(r, zero);
    free(r);
    return n;
}
int32_t ref_rd_mv_vals(void) { return MV_VALS; }

/* Returns the cost; writes fast_luma_rate / fast_chroma_rate to out[0..1]. */
uint64_t ref_rd_inter_fast_cost(const int32_t* i, const int32_t* above, const int32_t* left,
                                const int32_t* tables, const int32_t* nmv_vec_cost,
                                const int32_t* nmv_costs2, const int32_t* ref_order_hint7,
                                const int32_t* gm_wmtype8, const int32_t* stack_weights,
                                uint64_t ref_frames_num_bits, uint64_t lambda, uint64_t luma_distortion,
                                uint32_t* out) {
    RdScene s;
    rd_scene_init(&s, above, left, i[44], i[45]);
    rd_scatter_inter(s.rates, tables);
    memcpy(s.rates->nmv_vec_cost, nmv_vec_cost, sizeof(s.rates->nmv_vec_cost));
    memcpy(s.rates->nmv_costs, nmv_costs2, sizeof(s.rates->nmv_costs));
    /* `nmvcoststack` is what svt_av1_mv_bit_cost actually indexes, and it is
     * a POINTER pair into the context's own arrays (md_rate_estimation.c:479).
     * It must be re-pointed here or the MV rate reads a null. */
    s.rates->nmvcoststack[0] = &s.rates->nmv_costs[0][MV_MAX];
    s.rates->nmvcoststack[1] = &s.rates->nmv_costs[1][MV_MAX];

    rd_set_geom(&s, i[0]);
    s.ppcs->frm_hdr.interpolation_filter        = (InterpFilter)i[31];
    s.ppcs->frm_hdr.skip_mode_params.skip_mode_flag = (uint8_t)i[32];
    s.ppcs->frm_hdr.is_motion_mode_switchable   = (uint8_t)i[33];
    s.ppcs->frm_hdr.force_integer_mv            = (uint8_t)i[34];
    s.ppcs->frm_hdr.allow_warped_motion         = (uint8_t)i[35];
    s.ppcs->frm_hdr.allow_screen_content_tools  = (uint8_t)i[43];
    s.scs->seq_header.enable_dual_filter        = (uint8_t)i[36];
    s.scs->seq_header.enable_masked_compound    = (uint8_t)i[37];
    s.scs->seq_header.order_hint_info.enable_jnt_comp    = (uint8_t)i[38];
    s.scs->seq_header.enable_interintra_compound         = (uint8_t)i[39];
    s.scs->seq_header.order_hint_info.enable_order_hint  = (uint8_t)i[40];
    s.scs->seq_header.order_hint_info.order_hint_bits    = (uint8_t)i[41];
    s.ppcs->cur_order_hint                      = (uint32_t)i[42];
    for (int k = 0; k < 7; ++k) {
        s.ppcs->ref_order_hint[k] = (uint32_t)ref_order_hint7[k];
    }
    for (int k = 0; k < 8; ++k) {
        s.ppcs->global_motion[k].wmtype = (TransformationType)gm_wmtype8[k];
    }
    s.pcs->slice_type = B_SLICE;

    s.ctx->skip_mode_ctx      = (uint8_t)i[25];
    s.ctx->is_inter_ctx       = (uint8_t)i[26];
    s.ctx->approx_inter_rate  = (uint8_t)i[29];
    s.ctx->ifs_ctrls.level    = i[30] ? IFS_MDS0 : IFS_OFF;
    s.blk->overlappable_neighbors = (uint8_t)i[46];

    ModeDecisionCandidate* c = s.cand;
    c->block_mi.mode          = (PredictionMode)i[1];
    c->block_mi.ref_frame[0]  = (MvReferenceFrame)i[2];
    c->block_mi.ref_frame[1]  = (MvReferenceFrame)i[3];
    c->block_mi.mv[0].x       = (int16_t)i[4];
    c->block_mi.mv[0].y       = (int16_t)i[5];
    c->block_mi.mv[1].x       = (int16_t)i[6];
    c->block_mi.mv[1].y       = (int16_t)i[7];
    c->pred_mv[0].x           = (int16_t)i[8];
    c->pred_mv[0].y           = (int16_t)i[9];
    c->pred_mv[1].x           = (int16_t)i[10];
    c->pred_mv[1].y           = (int16_t)i[11];
    c->drl_index              = (uint8_t)i[12];
    c->block_mi.interp_filters= (uint32_t)i[13];
    c->block_mi.motion_mode   = (MotionMode)i[14];
    c->block_mi.num_proj_ref  = (uint16_t)i[15];
    c->block_mi.is_interintra_used   = (uint8_t)i[16];
    c->block_mi.interintra_mode      = (InterIntraMode)i[17];
    c->block_mi.use_wedge_interintra = (uint8_t)i[18];
    c->block_mi.interintra_wedge_index = (int8_t)i[19];
    c->block_mi.comp_group_idx = (uint8_t)i[20];
    c->block_mi.compound_idx   = (uint8_t)i[21];
    c->block_mi.interinter_comp.type = (CompoundType)i[22];
    c->block_mi.interinter_comp.wedge_index = (int8_t)i[23];
    c->skip_mode_allowed      = (bool)i[24];

    const int8_t rft = av1_ref_frame_type(c->block_mi.ref_frame);
    s.ctx->inter_mode_ctx[rft] = (int16_t)i[27];
    s.ctx->estimate_ref_frames_num_bits[rft] = ref_frames_num_bits;
    s.xd->ref_mv_count[rft]    = (uint8_t)i[28];
    for (int k = 0; k < MAX_REF_MV_STACK_SIZE; ++k) {
        s.ctx->ref_mv_stack[rft][k].weight = stack_weights[k];
    }

    const uint64_t cost = svt_aom_inter_fast_cost(s.pcs, s.ctx, s.cand_bf, lambda, luma_distortion);
    out[0] = s.cand_bf->fast_luma_rate;
    out[1] = s.cand_bf->fast_chroma_rate;
    rd_scene_free(&s);
    return cost;
}

/* ------------------------------------------------------------------ *
 * 3. svt_aom_intra_fast_cost / svt_aom_get_intra_uv_fast_rate
 * ------------------------------------------------------------------ */

/* i[] layout:
 *   0 bsize          1 mode            2 uv_mode        3 angle_delta_y
 *   4 angle_delta_uv 5 cfl_alpha_signs 6 cfl_alpha_idx  7 filter_intra_mode
 *   8 palette_size_y 9 palette_size_uv 10 use_intrabc
 *  11 is_key_slice  12 allow_intrabc  13 allow_screen_content_tools
 *  14 skip_mode_flag 15 filter_intra_level 16 has_uv
 *  17 intra_luma_top_ctx 18 intra_luma_left_ctx
 *  19 is_inter_ctx  20 skip_mode_ctx  21 blk_org_x  22 blk_org_y
 *  23 up_avail      24 left_avail     25 use_accurate_cfl
 *  26 mv.x          27 mv.y           28 pred_mv.x     29 pred_mv.y
 */
#define RD_INTRA_FIELDS 30

int32_t ref_rd_intra_fields(void) { return RD_INTRA_FIELDS; }

static void rd_intra_scene(RdScene* s, const int32_t* i, const int32_t* above, const int32_t* left,
                           const int32_t* tables, const int32_t* dv_joint, const int32_t* dv_cost2) {
    rd_scene_init(s, above, left, i[23], i[24]);
    rd_scatter_intra(s->rates, tables);
    memcpy(s->rates->dv_joint_cost, dv_joint, sizeof(s->rates->dv_joint_cost));
    memcpy(s->rates->dv_cost, dv_cost2, sizeof(s->rates->dv_cost));
    rd_set_geom(s, i[0]);
    s->pcs->slice_type = i[11] ? I_SLICE : B_SLICE;
    s->ppcs->slice_type = s->pcs->slice_type;
    /* svt_aom_allow_intrabc(frm_hdr, slice_type) is
     * `frm_hdr->allow_intrabc && slice_type == I_SLICE`. */
    s->ppcs->frm_hdr.allow_intrabc = (uint8_t)i[12];
    s->ppcs->frm_hdr.allow_screen_content_tools = (uint8_t)i[13];
    s->ppcs->frm_hdr.skip_mode_params.skip_mode_flag = (uint8_t)i[14];
    s->scs->seq_header.filter_intra_level = (uint8_t)i[15];
    s->ctx->has_uv = (bool)i[16];
    s->ctx->intra_luma_top_ctx  = (uint8_t)i[17];
    s->ctx->intra_luma_left_ctx = (uint8_t)i[18];
    s->ctx->is_inter_ctx        = (uint8_t)i[19];
    s->ctx->skip_mode_ctx       = (uint8_t)i[20];
    s->ctx->blk_org_x           = (uint16_t)i[21];
    s->ctx->blk_org_y           = (uint16_t)i[22];

    ModeDecisionCandidate* c = s->cand;
    c->block_mi.mode         = (PredictionMode)i[1];
    c->block_mi.uv_mode      = (UvPredictionMode)i[2];
    c->block_mi.angle_delta[PLANE_TYPE_Y]  = (int8_t)i[3];
    c->block_mi.angle_delta[PLANE_TYPE_UV] = (int8_t)i[4];
    c->block_mi.cfl_alpha_signs = (uint8_t)i[5];
    c->block_mi.cfl_alpha_idx   = (uint8_t)i[6];
    c->block_mi.filter_intra_mode = (FilterIntraMode)i[7];
    c->palette_size[0]       = (uint8_t)i[8];
    c->palette_size[1]       = (uint8_t)i[9];
    c->palette_info          = NULL;
    c->block_mi.use_intrabc  = (uint8_t)i[10];
    c->block_mi.mv[0].x      = (int16_t)i[26];
    c->block_mi.mv[0].y      = (int16_t)i[27];
    c->pred_mv[0].x          = (int16_t)i[28];
    c->pred_mv[0].y          = (int16_t)i[29];
}

uint64_t ref_rd_intra_uv_fast_rate(const int32_t* i, const int32_t* above, const int32_t* left,
                                   const int32_t* tables, const int32_t* dv_joint,
                                   const int32_t* dv_cost2) {
    RdScene s;
    rd_intra_scene(&s, i, above, left, tables, dv_joint, dv_cost2);
    const uint64_t r = svt_aom_get_intra_uv_fast_rate(s.pcs, s.ctx, s.cand_bf, (bool)i[25]);
    rd_scene_free(&s);
    return r;
}

uint64_t ref_rd_intra_fast_cost(const int32_t* i, const int32_t* above, const int32_t* left,
                                const int32_t* tables, const int32_t* dv_joint, const int32_t* dv_cost2,
                                uint64_t lambda, uint64_t luma_distortion, uint32_t* out) {
    RdScene s;
    rd_intra_scene(&s, i, above, left, tables, dv_joint, dv_cost2);
    const uint64_t cost = svt_aom_intra_fast_cost(s.pcs, s.ctx, s.cand_bf, lambda, luma_distortion);
    out[0] = s.cand_bf->fast_luma_rate;
    out[1] = s.cand_bf->fast_chroma_rate;
    rd_scene_free(&s);
    return cost;
}

/* ------------------------------------------------------------------ *
 * 4. svt_aom_full_cost / svt_aom_full_cost_pd0
 * ------------------------------------------------------------------ */

/* i[] layout:
 *   0 skip_coeff_ctx  1 skip_mode_ctx  2 update_full_cost_ssim
 *   3 shut_fast_rate  4 tx_mode_select 5 lossless_segment
 *   6 blk_skip_decision 7 block_has_coeff 8 is_inter_mode 9 skip_mode_allowed
 *  10 bsize
 * d[] is 12 u64s: y/cb/cr x (ssd_nonskip, ssd_skip, ssim_nonskip, ssim_skip).
 * out[] is 6 u64s: cost, total_rate, full_dist, full_cost_ssim,
 *                  forced_coeff_skip, skip_mode.
 *
 * `non_skip_tx_size_bits` / `skip_tx_size_bits` cannot be injected directly —
 * C recomputes them from `svt_aom_get_tx_size_bits`. This shim therefore
 * drives the case the port's caller supplies them for by pinning
 * `tx_mode == ONLY_4X4` (so C's `TX_MODE_SELECT` gate fails and BOTH terms
 * are 0), and the Rust side is asked for the same configuration. The
 * tx-size arm itself is exercised by `crate::vartx`'s own differential.
 */
#define RD_FULL_FIELDS 11

int32_t ref_rd_full_fields(void) { return RD_FULL_FIELDS; }

void ref_rd_full_cost(const int32_t* i, const uint64_t* d, const int32_t* skip_fac_bits,
                      const int32_t* skip_mode_fac_bits, uint64_t y_coeff_bits, uint64_t cb_coeff_bits,
                      uint64_t cr_coeff_bits, uint64_t lambda, uint64_t* out) {
    static const int32_t none_nb[RD_NB_FIELDS] = {0};
    RdScene s;
    rd_scene_init(&s, none_nb, none_nb, 0, 0);
    memcpy(s.rates->skip_fac_bits, skip_fac_bits, sizeof(s.rates->skip_fac_bits));
    memcpy(s.rates->skip_mode_fac_bits, skip_mode_fac_bits, sizeof(s.rates->skip_mode_fac_bits));
    rd_set_geom(&s, i[10]);

    s.ctx->skip_coeff_ctx    = (uint8_t)i[0];
    s.ctx->skip_mode_ctx     = (uint8_t)i[1];
    s.ctx->tune_ssim_level   = i[2] ? SSIM_LVL_1 : SSIM_LVL_0;
    s.ctx->shut_fast_rate    = (uint8_t)i[3];
    s.ppcs->frm_hdr.tx_mode  = i[4] ? TX_MODE_SELECT : ONLY_4X4;
    s.ctx->blk_skip_decision = (bool)i[6];
    s.ctx->pd_pass           = PD_PASS_1;
    s.ctx->md_stage          = MD_STAGE_3;
    s.blk->segment_id        = 0;
    /* `svt_av1_is_lossless_segment` (mode_decision.c:71) returns
     * `pcs->lossless[segment_id]` with segmentation on and `pcs->lossless[0]`
     * without — segmentation is off here, so this one flag is the whole
     * input. */
    s.pcs->lossless[0] = (bool)i[5];

    s.cand_bf->block_has_coeff = (uint8_t)i[7];
    s.cand_bf->fast_luma_rate  = 0;
    s.cand_bf->fast_chroma_rate = 0;
    s.cand->block_mi.mode = i[8] ? NEWMV : DC_PRED;
    s.cand->skip_mode_allowed = (bool)i[9];

    uint64_t yd[DIST_TOTAL][DIST_CALC_TOTAL];
    uint64_t cbd[DIST_TOTAL][DIST_CALC_TOTAL];
    uint64_t crd[DIST_TOTAL][DIST_CALC_TOTAL];
    yd[DIST_SSD][0]  = d[0];  yd[DIST_SSD][1]  = d[1];
    yd[DIST_SSIM][0] = d[2];  yd[DIST_SSIM][1] = d[3];
    cbd[DIST_SSD][0] = d[4];  cbd[DIST_SSD][1] = d[5];
    cbd[DIST_SSIM][0]= d[6];  cbd[DIST_SSIM][1]= d[7];
    crd[DIST_SSD][0] = d[8];  crd[DIST_SSD][1] = d[9];
    crd[DIST_SSIM][0]= d[10]; crd[DIST_SSIM][1]= d[11];

    uint64_t ycb = y_coeff_bits, cbcb = cb_coeff_bits, crcb = cr_coeff_bits;
    svt_aom_full_cost(s.pcs, s.ctx, s.cand_bf, lambda, yd, cbd, crd, &ycb, &cbcb, &crcb);

    out[0] = s.full_cost;
    out[1] = s.cand_bf->total_rate;
    out[2] = s.cand_bf->full_dist;
    out[3] = i[2] ? s.full_cost_ssim : 0;
    /* Report the STATE C left behind, not an inference about which arm ran:
     * `blk_skip_decision` and the skip-mode arm both clear block_has_coeff,
     * so a derived "forced skip" flag cannot distinguish them here. The Rust
     * side derives the same post-state from its own two flags. */
    out[4] = (uint64_t)s.cand_bf->block_has_coeff;
    out[5] = (uint64_t)s.cand->block_mi.skip_mode;
    rd_scene_free(&s);
}

uint64_t ref_rd_full_cost_pd0(uint64_t y_coeff_bits, uint64_t y_distortion, int32_t skip_fac_bits_00,
                              int32_t partition_fac_bits_0_none, uint64_t lambda) {
    static const int32_t none_nb[RD_NB_FIELDS] = {0};
    RdScene s;
    rd_scene_init(&s, none_nb, none_nb, 0, 0);
    s.rates->skip_fac_bits[0][0] = skip_fac_bits_00;
    s.rates->partition_fac_bits[0][PARTITION_NONE] = partition_fac_bits_0_none;
    uint64_t yd[2] = {y_distortion, 0};
    uint64_t ycb   = y_coeff_bits;
    svt_aom_full_cost_pd0(s.ctx, s.cand_bf, yd, lambda, &ycb);
    const uint64_t r = s.full_cost;
    rd_scene_free(&s);
    return r;
}
