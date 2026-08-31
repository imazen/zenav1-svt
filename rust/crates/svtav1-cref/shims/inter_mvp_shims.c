/*
 * C shims for the INTER MVP (motion-vector-predictor) differential
 * (inter campaign chunk C2, rust/docs/INTER-ENCODE-PLAN.md §2).
 *
 * Kept in its OWN translation unit rather than appended to ref_shims.c so
 * the C2 lane and the concurrent C3 (`inter_mv_code`) lane never share a
 * file in this checkout — a shared file cannot be committed by one lane
 * without sweeping in the other's in-flight edits.
 *
 * Every function here drives a REAL exported SVT-AV1 symbol (evidence
 * tier 1, docs/WORKING-ON-THIS.md §4):
 *   setup_ref_mv_list                     adaptive_mv_pred.c:651
 *   svt_aom_gm_get_motion_vector_enc      adaptive_mv_pred.c:983
 *   svt_aom_compute_inter_mode_ctx_light  adaptive_mv_pred.c:1138
 *   svt_aom_get_av1_mv_pred_drl           adaptive_mv_pred.c:1407
 *   svt_av1_find_best_ref_mvs_from_stack  adaptive_mv_pred.c:2030
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "adaptive_mv_pred.h"
#include "EbSvtAv1.h"
#include "md_process.h"
#include "common_utils.h"
#include "inter_prediction.h"
#include "av1_common.h"
#include "pcs.h"
#include "reference_object.h"
#include "sys_resource_manager.h"

/* RTCD dispatch pointers: setup_ref_mv_list itself calls no kernels, but
 * the library's function-pointer tables (svt_memcpy et al.) are shared and
 * must be populated before any library entry point is driven. */
void     svt_aom_setup_common_rtcd_internal(uint64_t flags);
void     svt_aom_setup_rtcd_internal(uint64_t flags);
uint64_t svt_aom_get_cpu_flags_to_use(void);
static int g_inter_mvp_rtcd_ready = 0;
static void inter_mvp_ensure_init(void) {
    if (!g_inter_mvp_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_inter_mvp_rtcd_ready = 1;
    }
}

void setup_ref_mv_list(PictureControlSet* pcs, const Av1Common* cm, const MacroBlockD* xd,
                       MvReferenceFrame ref_frame, uint8_t* refmv_count,
                       CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], Mv* gm_mv_candidates,
                       const WarpedMotionParams* gm_params, int32_t mi_row, int32_t mi_col,
                       ModeDecisionContext* ctx, uint8_t symteric_refs, Mv* mv_ref0,
                       int16_t* mode_context);
void svt_av1_find_best_ref_mvs_from_stack(int allow_hp,
                                          CandidateMv ref_mv_stack[][MAX_REF_MV_STACK_SIZE],
                                          MacroBlockD* xd, MvReferenceFrame ref_frame,
                                          Mv* nearest_mv, Mv* near_mv, int is_integer);

/* ---- INTER MVP stack (inter campaign chunk C2, INTER-ENCODE-PLAN.md §2) ----
 *
 * Differential oracle for svtav1-encoder/src/inter_mvp.rs. Drives the same
 * EXPORTED setup_ref_mv_list (adaptive_mv_pred.c:651) as the IntraBC shim
 * above, but for a GENERAL MvReferenceFrame (single or compound) on a grid
 * whose neighbours carry real inter refs and two MVs, with the temporal-MVP
 * (MFMV) block LIVE: `frm_hdr.use_ref_frame_mvs` is a parameter, `pcs->tpl_mvs`
 * is caller-supplied, and the reference order hints are wired through
 * synthetic EbReferenceObjects hung off `pcs->ref_pic_ptr_array`.
 *
 * Grid cells arrive packed 8 i32s each:
 *   bsize, mode, use_intrabc, ref0, ref1, mv0_as_int, mv1_as_int, partition.
 * tpl cells arrive packed 2 i32s each: mfmv0_as_int, ref_frame_offset.
 */

int32_t ref_setup_ref_mv_list_inter(
    const int32_t* cells, int32_t grid_rows, int32_t grid_cols, int32_t mi_row, int32_t mi_col,
    int32_t bsize_cur, int32_t mi_rows, int32_t mi_cols, int32_t tile_row_start,
    int32_t tile_row_end, int32_t tile_col_start, int32_t tile_col_end, int32_t sb_size_is_128,
    int32_t ref_frame, const int32_t* gm_wmtype /*[8]*/, const int32_t* gm_wmmat /*[8][6]*/,
    const int32_t* sign_bias /*[8]*/, int32_t allow_high_precision_mv, int32_t force_integer_mv,
    int32_t use_ref_frame_mvs, int32_t enable_order_hint, int32_t order_hint_bits,
    int32_t cur_order_hint, const int32_t* ref_order_hint /*[8]*/,
    const int32_t* tpl_cells /*[tpl_n][2]*/, int32_t tpl_n, int32_t mi_stride_full,
    int32_t sb64_sq_no4xn_geom, int32_t symmetric_refs,
    int32_t* stack_out /* 8 x {this_as_int, comp_as_int, weight} */, int32_t* mode_ctx_out,
    uint32_t* nearest_out, uint32_t* near_out, uint32_t* mv_ref0_out /*[64]*/) {
    inter_mvp_ensure_init();
    const int32_t n_cells = grid_rows * grid_cols;
    MbModeInfo*   mbmi    = (MbModeInfo*)calloc((size_t)n_cells, sizeof(MbModeInfo));
    MbModeInfo**  grid    = (MbModeInfo**)calloc((size_t)n_cells, sizeof(MbModeInfo*));
    for (int32_t i = 0; i < n_cells; i++) {
        const int32_t* c              = cells + (size_t)i * 8;
        mbmi[i].bsize                 = (BlockSize)c[0];
        mbmi[i].block_mi.mode         = (PredictionMode)c[1];
        mbmi[i].block_mi.use_intrabc  = (uint8_t)c[2];
        mbmi[i].block_mi.ref_frame[0] = (MvReferenceFrame)c[3];
        mbmi[i].block_mi.ref_frame[1] = (MvReferenceFrame)c[4];
        mbmi[i].block_mi.mv[0].as_int = (uint32_t)c[5];
        mbmi[i].block_mi.mv[1].as_int = (uint32_t)c[6];
        mbmi[i].partition             = (PartitionType)c[7];
        grid[i]                       = &mbmi[i];
    }

    Av1Common* cm = (Av1Common*)calloc(1, sizeof(Av1Common));
    cm->mi_rows   = mi_rows;
    cm->mi_cols   = mi_cols;
    cm->mi_stride = mi_stride_full;
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; i++) { cm->ref_frame_sign_bias[i] = (uint32_t)sign_bias[i]; }

    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    pcs->ppcs                     = ppcs;
    pcs->scs                      = scs;
    ppcs->scs                     = scs;
    ppcs->av1_cm                  = cm;
    scs->seq_header.sb_size       = sb_size_is_128 ? BLOCK_128X128 : BLOCK_64X64;
    scs->seq_header.order_hint_info.enable_order_hint = (uint8_t)enable_order_hint;
    scs->seq_header.order_hint_info.order_hint_bits   = (uint8_t)order_hint_bits;
    ppcs->frm_hdr.use_ref_frame_mvs        = (uint8_t)use_ref_frame_mvs;
    ppcs->frm_hdr.allow_high_precision_mv  = (uint8_t)allow_high_precision_mv;
    ppcs->frm_hdr.force_integer_mv         = (uint8_t)force_integer_mv;
    ppcs->cur_order_hint                   = (uint32_t)cur_order_hint;
    ctx->sb64_sq_no4xn_geom                = (uint8_t)sb64_sq_no4xn_geom;

    /* Global motion params (setup_ref_mv_list takes them as an argument). */
    WarpedMotionParams gm_params[TOTAL_REFS_PER_FRAME];
    memset(gm_params, 0, sizeof(gm_params));
    for (int i = 0; i < TOTAL_REFS_PER_FRAME; i++) {
        gm_params[i].wmtype = (TransformationType)gm_wmtype[i];
        for (int j = 0; j < 6; j++) { gm_params[i].wmmat[j] = gm_wmmat[i * 6 + j]; }
    }

    /* Synthetic reference objects: setup_ref_mv_list's MFMV block reads
     * `pcs->ref_pic_ptr_array[get_list_idx(rf)][get_ref_frame_idx(rf)]
     *  ->object_ptr->order_hint`. One per inter ref type (1..7). */
    EbReferenceObject* refobj  = (EbReferenceObject*)calloc(8, sizeof(EbReferenceObject));
    EbObjectWrapper*   wrapper = (EbObjectWrapper*)calloc(8, sizeof(EbObjectWrapper));
    for (int rt = LAST_FRAME; rt <= ALTREF_FRAME; rt++) {
        refobj[rt].order_hint            = (uint32_t)ref_order_hint[rt];
        wrapper[rt].object_ptr           = &refobj[rt];
        pcs->ref_pic_ptr_array[get_list_idx((uint8_t)rt)][get_ref_frame_idx((uint8_t)rt)] =
            &wrapper[rt];
    }

    /* Temporal MV field. */
    TPL_MV_REF* tpl = (TPL_MV_REF*)calloc((size_t)(tpl_n > 0 ? tpl_n : 1), sizeof(TPL_MV_REF));
    for (int32_t i = 0; i < tpl_n; i++) {
        tpl[i].mfmv0.as_int     = (uint32_t)tpl_cells[i * 2];
        tpl[i].ref_frame_offset = (uint8_t)tpl_cells[i * 2 + 1];
    }
    pcs->tpl_mvs = tpl;

    /* svt_aom_init_xd (adaptive_mv_pred.c:1046-1091). */
    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    const int32_t bw = mi_size_wide[bsize_cur];
    const int32_t bh = mi_size_high[bsize_cur];
    xd.n4_w = xd.n8_w = (uint8_t)bw;
    xd.n4_h = xd.n8_h = (uint8_t)bh;
    xd.mi_row            = mi_row;
    xd.mi_col            = mi_col;
    xd.mb_to_top_edge    = -((mi_row * MI_SIZE) * 8);
    xd.mb_to_bottom_edge = ((mi_rows - bh - mi_row) * MI_SIZE) * 8;
    xd.mb_to_left_edge   = -((mi_col * MI_SIZE) * 8);
    xd.mb_to_right_edge  = ((mi_cols - bw - mi_col) * MI_SIZE) * 8;
    xd.tile.mi_row_start = tile_row_start;
    xd.tile.mi_row_end   = tile_row_end;
    xd.tile.mi_col_start = tile_col_start;
    xd.tile.mi_col_end   = tile_col_end;
    xd.up_available      = (int8_t)(mi_row > tile_row_start);
    xd.left_available    = (int8_t)(mi_col > tile_col_start);
    xd.is_sec_rect       = 0;
    if (xd.n8_w < xd.n8_h) {
        if (!((mi_col + xd.n8_w) & (xd.n8_h - 1))) { xd.is_sec_rect = 1; }
    }
    if (xd.n8_w > xd.n8_h) {
        if (mi_row & (xd.n8_w - 1)) { xd.is_sec_rect = 1; }
    }
    xd.mi_stride = grid_cols;
    xd.mi        = grid + (size_t)mi_row * grid_cols + mi_col;

    /* generate_av1_mvp_table's per-ref preamble (:1366-1394). */
    /* NOT static: cargo runs the tests in one binary on several threads and
       two of them drive this shim concurrently, so a shared buffer is a data
       race that shows up as an intermittent count/weight mismatch. 2.8 KB on
       the stack. (Measured 2026-08-31: with `static` here,
       c_parity_intrabc_mvp failed at partition=0 with count 1 vs 2 under
       --test-threads=3 and passed under --test-threads=1.) */
    CandidateMv stack2d[MODE_CTX_REF_FRAMES][MAX_REF_MV_STACK_SIZE];
    memset(stack2d, 0, sizeof(stack2d));
    memset(stack2d[ref_frame], 0, sizeof(CandidateMv) * MAX_REF_MV_STACK_SIZE);
    xd.ref_mv_count[ref_frame] = 0;

    MvReferenceFrame rf[2];
    av1_set_ref_frame(rf, (int8_t)ref_frame);
    Mv gm_mv[2];
    if (ref_frame == INTRA_FRAME) {
        gm_mv[0].as_int = gm_mv[1].as_int = 0;
    } else if (ref_frame < REF_FRAMES) {
        gm_mv[0] = svt_aom_gm_get_motion_vector_enc(&gm_params[ref_frame],
                                                    allow_high_precision_mv,
                                                    (BlockSize)bsize_cur,
                                                    mi_col,
                                                    mi_row,
                                                    force_integer_mv);
        gm_mv[1].as_int = 0;
    } else {
        gm_mv[0] = svt_aom_gm_get_motion_vector_enc(&gm_params[rf[0]],
                                                    allow_high_precision_mv,
                                                    (BlockSize)bsize_cur,
                                                    mi_col,
                                                    mi_row,
                                                    force_integer_mv);
        gm_mv[1] = svt_aom_gm_get_motion_vector_enc(&gm_params[rf[1]],
                                                    allow_high_precision_mv,
                                                    (BlockSize)bsize_cur,
                                                    mi_col,
                                                    mi_row,
                                                    force_integer_mv);
    }

    /* mv_ref0 is IN/OUT: C's generate_av1_mvp_table keeps one array across
     * its whole ref-frame loop, and the symteric_refs shortcut reads back
     * what an earlier ref's pass stored. */
    Mv mv_ref0[64];
    for (int i = 0; i < 64; i++) { mv_ref0[i].as_int = mv_ref0_out[i]; }
    int16_t mode_ctx = 0;
    uint8_t count    = 0;
    setup_ref_mv_list(pcs,
                      cm,
                      &xd,
                      (MvReferenceFrame)ref_frame,
                      &count,
                      stack2d[ref_frame],
                      gm_mv,
                      gm_params,
                      mi_row,
                      mi_col,
                      ctx,
                      (uint8_t)symmetric_refs,
                      mv_ref0,
                      &mode_ctx);
    xd.ref_mv_count[ref_frame] = count;

    for (int i = 0; i < MAX_REF_MV_STACK_SIZE; i++) {
        stack_out[i * 3]     = (int32_t)stack2d[ref_frame][i].this_mv.as_int;
        stack_out[i * 3 + 1] = (int32_t)stack2d[ref_frame][i].comp_mv.as_int;
        stack_out[i * 3 + 2] = stack2d[ref_frame][i].weight;
    }
    *mode_ctx_out = mode_ctx;
    for (int i = 0; i < 64; i++) { mv_ref0_out[i] = mv_ref0[i].as_int; }

    Mv nearest, near_mv;
    svt_av1_find_best_ref_mvs_from_stack(
        allow_high_precision_mv, stack2d, &xd, (MvReferenceFrame)ref_frame, &nearest, &near_mv,
        force_integer_mv);
    *nearest_out = nearest.as_int;
    *near_out    = near_mv.as_int;

    free(tpl);
    free(wrapper);
    free(refobj);
    free(ctx);
    free(scs);
    free(pcs);
    free(ppcs);
    free(cm);
    free(grid);
    free(mbmi);
    return count;
}

/* svt_aom_gm_get_motion_vector_enc (adaptive_mv_pred.c:983, EXPORTED). */
uint32_t ref_gm_get_motion_vector_enc(int32_t wmtype, const int32_t* wmmat /*[6]*/,
                                      int32_t allow_hp, int32_t bsize, int32_t mi_col,
                                      int32_t mi_row, int32_t is_integer) {
    WarpedMotionParams gm;
    memset(&gm, 0, sizeof(gm));
    gm.wmtype = (TransformationType)wmtype;
    for (int j = 0; j < 6; j++) { gm.wmmat[j] = wmmat[j]; }
    Mv r = svt_aom_gm_get_motion_vector_enc(&gm, allow_hp, (BlockSize)bsize, mi_col, mi_row,
                                            is_integer);
    return r.as_int;
}

/* svt_aom_compute_inter_mode_ctx_light (adaptive_mv_pred.c:1138, EXPORTED).
 * Same packed grid as ref_setup_ref_mv_list_inter. */
void svt_aom_compute_inter_mode_ctx_light(ModeDecisionContext* ctx, BlkStruct* blk_ptr,
                                          MvReferenceFrame ref_frame, PictureControlSet* pcs);

int32_t ref_compute_inter_mode_ctx_light(const int32_t* cells, int32_t grid_rows,
                                         int32_t grid_cols, int32_t mi_row, int32_t mi_col,
                                         int32_t bsize_cur, int32_t mi_rows, int32_t mi_cols,
                                         int32_t tile_row_start, int32_t tile_row_end,
                                         int32_t tile_col_start, int32_t tile_col_end,
                                         int32_t sb_size_is_128, int32_t ref_frame) {
    inter_mvp_ensure_init();
    const int32_t n_cells = grid_rows * grid_cols;
    MbModeInfo*   mbmi    = (MbModeInfo*)calloc((size_t)n_cells, sizeof(MbModeInfo));
    MbModeInfo**  grid    = (MbModeInfo**)calloc((size_t)n_cells, sizeof(MbModeInfo*));
    for (int32_t i = 0; i < n_cells; i++) {
        const int32_t* c              = cells + (size_t)i * 8;
        mbmi[i].bsize                 = (BlockSize)c[0];
        mbmi[i].block_mi.mode         = (PredictionMode)c[1];
        mbmi[i].block_mi.use_intrabc  = (uint8_t)c[2];
        mbmi[i].block_mi.ref_frame[0] = (MvReferenceFrame)c[3];
        mbmi[i].block_mi.ref_frame[1] = (MvReferenceFrame)c[4];
        mbmi[i].block_mi.mv[0].as_int = (uint32_t)c[5];
        mbmi[i].block_mi.mv[1].as_int = (uint32_t)c[6];
        mbmi[i].partition             = (PartitionType)c[7];
        grid[i]                       = &mbmi[i];
    }

    Av1Common* cm = (Av1Common*)calloc(1, sizeof(Av1Common));
    cm->mi_rows   = mi_rows;
    cm->mi_cols   = mi_cols;
    cm->mi_stride = grid_cols;

    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs  = (SequenceControlSet*)calloc(1, sizeof(*scs));
    ModeDecisionContext*     ctx  = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(*blk));
    MacroBlockD*             xd   = (MacroBlockD*)calloc(1, sizeof(*xd));
    pcs->ppcs               = ppcs;
    pcs->scs                = scs;
    ppcs->scs               = scs;
    ppcs->av1_cm            = cm;
    scs->seq_header.sb_size = sb_size_is_128 ? BLOCK_128X128 : BLOCK_64X64;
    blk->av1xd              = xd;
    ctx->blk_ptr            = blk;
    ctx->blk_org_x          = (uint16_t)(mi_col << MI_SIZE_LOG2);
    ctx->blk_org_y          = (uint16_t)(mi_row << MI_SIZE_LOG2);

    const int32_t bw = mi_size_wide[bsize_cur];
    const int32_t bh = mi_size_high[bsize_cur];
    xd->n4_w = xd->n8_w = (uint8_t)bw;
    xd->n4_h = xd->n8_h = (uint8_t)bh;
    xd->mi_row            = mi_row;
    xd->mi_col            = mi_col;
    xd->mb_to_top_edge    = -((mi_row * MI_SIZE) * 8);
    xd->mb_to_bottom_edge = ((mi_rows - bh - mi_row) * MI_SIZE) * 8;
    xd->mb_to_left_edge   = -((mi_col * MI_SIZE) * 8);
    xd->mb_to_right_edge  = ((mi_cols - bw - mi_col) * MI_SIZE) * 8;
    xd->tile.mi_row_start = tile_row_start;
    xd->tile.mi_row_end   = tile_row_end;
    xd->tile.mi_col_start = tile_col_start;
    xd->tile.mi_col_end   = tile_col_end;
    xd->up_available      = (int8_t)(mi_row > tile_row_start);
    xd->left_available    = (int8_t)(mi_col > tile_col_start);
    xd->is_sec_rect       = 0;
    if (xd->n8_w < xd->n8_h) {
        if (!((mi_col + xd->n8_w) & (xd->n8_h - 1))) { xd->is_sec_rect = 1; }
    }
    if (xd->n8_w > xd->n8_h) {
        if (mi_row & (xd->n8_w - 1)) { xd->is_sec_rect = 1; }
    }
    xd->mi_stride = grid_cols;
    xd->mi        = grid + (size_t)mi_row * grid_cols + mi_col;

    svt_aom_compute_inter_mode_ctx_light(ctx, blk, (MvReferenceFrame)ref_frame, pcs);
    const int32_t out = ctx->inter_mode_ctx[ref_frame];

    free(xd);
    free(blk);
    free(ctx);
    free(scs);
    free(pcs);
    free(ppcs);
    free(cm);
    free(grid);
    free(mbmi);
    return out;
}

/* svt_aom_get_av1_mv_pred_drl (adaptive_mv_pred.c:1407, EXPORTED). */
void svt_aom_get_av1_mv_pred_drl(ModeDecisionContext* ctx, BlkStruct* blk_ptr,
                                 MvReferenceFrame ref_frame, uint8_t is_compound,
                                 PredictionMode mode, uint8_t drl_index, Mv nearestmv[2],
                                 Mv nearmv[2], Mv ref_mv[2]);

void ref_get_av1_mv_pred_drl(const int32_t* stack_in /*8 x {this,comp,weight}*/,
                             int32_t refmv_count, int32_t ref_frame, int32_t is_compound,
                             int32_t mode, int32_t drl_index, uint32_t* io /*[6] nearest0,
                             nearest1, near0, near1, refmv0, refmv1 in/out*/) {
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));
    BlkStruct*           blk = (BlkStruct*)calloc(1, sizeof(*blk));
    MacroBlockD*         xd  = (MacroBlockD*)calloc(1, sizeof(*xd));
    blk->av1xd               = xd;
    for (int i = 0; i < MAX_REF_MV_STACK_SIZE; i++) {
        ctx->ref_mv_stack[ref_frame][i].this_mv.as_int = (uint32_t)stack_in[i * 3];
        ctx->ref_mv_stack[ref_frame][i].comp_mv.as_int = (uint32_t)stack_in[i * 3 + 1];
        ctx->ref_mv_stack[ref_frame][i].weight         = stack_in[i * 3 + 2];
    }
    xd->ref_mv_count[ref_frame] = (uint8_t)refmv_count;

    Mv nearestmv[2], nearmv[2], ref_mv[2];
    nearestmv[0].as_int = io[0];
    nearestmv[1].as_int = io[1];
    nearmv[0].as_int    = io[2];
    nearmv[1].as_int    = io[3];
    ref_mv[0].as_int    = io[4];
    ref_mv[1].as_int    = io[5];

    svt_aom_get_av1_mv_pred_drl(ctx,
                                blk,
                                (MvReferenceFrame)ref_frame,
                                (uint8_t)is_compound,
                                (PredictionMode)mode,
                                (uint8_t)drl_index,
                                nearestmv,
                                nearmv,
                                ref_mv);

    io[0] = nearestmv[0].as_int;
    io[1] = nearestmv[1].as_int;
    io[2] = nearmv[0].as_int;
    io[3] = nearmv[1].as_int;
    io[4] = ref_mv[0].as_int;
    io[5] = ref_mv[1].as_int;

    free(xd);
    free(blk);
    free(ctx);
}

/* svt_aom_mode_context_analyzer (inter_prediction.c:2565, EXPORTED). */
int16_t svt_aom_mode_context_analyzer(int16_t mode_context, const MvReferenceFrame* const rf);

int32_t ref_mode_context_analyzer(int32_t mode_context, int32_t rf0, int32_t rf1) {
    MvReferenceFrame rf[2] = {(MvReferenceFrame)rf0, (MvReferenceFrame)rf1};
    return (int32_t)svt_aom_mode_context_analyzer((int16_t)mode_context, rf);
}

/* svt_av1_count_overlappable_neighbors (adaptive_mv_pred.c:1893, EXPORTED).
 * Same packed grid as ref_setup_ref_mv_list_inter. */
void svt_av1_count_overlappable_neighbors(const PictureControlSet* pcs, BlkStruct* blk_ptr,
                                          const BlockSize bsize, int32_t mi_row, int32_t mi_col);

int32_t ref_count_overlappable_neighbors(const int32_t* cells, int32_t grid_rows,
                                         int32_t grid_cols, int32_t mi_row, int32_t mi_col,
                                         int32_t bsize_cur, int32_t mi_rows, int32_t mi_cols,
                                         int32_t tile_row_start, int32_t tile_row_end,
                                         int32_t tile_col_start, int32_t tile_col_end) {
    inter_mvp_ensure_init();
    const int32_t n_cells = grid_rows * grid_cols;
    MbModeInfo*   mbmi    = (MbModeInfo*)calloc((size_t)n_cells, sizeof(MbModeInfo));
    MbModeInfo**  grid    = (MbModeInfo**)calloc((size_t)n_cells, sizeof(MbModeInfo*));
    for (int32_t i = 0; i < n_cells; i++) {
        const int32_t* c              = cells + (size_t)i * 8;
        mbmi[i].bsize                 = (BlockSize)c[0];
        mbmi[i].block_mi.mode         = (PredictionMode)c[1];
        mbmi[i].block_mi.use_intrabc  = (uint8_t)c[2];
        mbmi[i].block_mi.ref_frame[0] = (MvReferenceFrame)c[3];
        mbmi[i].block_mi.ref_frame[1] = (MvReferenceFrame)c[4];
        mbmi[i].block_mi.mv[0].as_int = (uint32_t)c[5];
        mbmi[i].block_mi.mv[1].as_int = (uint32_t)c[6];
        mbmi[i].partition             = (PartitionType)c[7];
        grid[i]                       = &mbmi[i];
    }

    Av1Common* cm = (Av1Common*)calloc(1, sizeof(Av1Common));
    cm->mi_rows   = mi_rows;
    cm->mi_cols   = mi_cols;
    cm->mi_stride = grid_cols;

    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    BlkStruct*               blk  = (BlkStruct*)calloc(1, sizeof(*blk));
    MacroBlockD*             xd   = (MacroBlockD*)calloc(1, sizeof(*xd));
    pcs->ppcs                     = ppcs;
    ppcs->av1_cm                  = cm;
    blk->av1xd                    = xd;

    const int32_t bw = mi_size_wide[bsize_cur];
    const int32_t bh = mi_size_high[bsize_cur];
    xd->n4_w = xd->n8_w = (uint8_t)bw;
    xd->n4_h = xd->n8_h = (uint8_t)bh;
    xd->mi_row            = mi_row;
    xd->mi_col            = mi_col;
    xd->tile.mi_row_start = tile_row_start;
    xd->tile.mi_row_end   = tile_row_end;
    xd->tile.mi_col_start = tile_col_start;
    xd->tile.mi_col_end   = tile_col_end;
    xd->up_available      = (int8_t)(mi_row > tile_row_start);
    xd->left_available    = (int8_t)(mi_col > tile_col_start);
    xd->mi_stride         = grid_cols;
    xd->mi                = grid + (size_t)mi_row * grid_cols + mi_col;

    svt_av1_count_overlappable_neighbors(pcs, blk, (BlockSize)bsize_cur, mi_row, mi_col);
    const int32_t out = (int32_t)blk->overlappable_neighbors;

    free(xd);
    free(blk);
    free(pcs);
    free(ppcs);
    free(cm);
    free(grid);
    free(mbmi);
    return out;
}
