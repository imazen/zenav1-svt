/*
 * Differential oracles for `svtav1_encoder::md_subpel` — the wholesale port of
 * the sub-pixel motion search in Source/Lib/Codec/mcomp.c, and for
 * `svtav1_dsp::subpel_variance` — the `svf` kernel that search minimises.
 *
 * WHY A SHELL SHIM. Of mcomp.c's 17 functions only three are linkable
 * (`nm -g` on Bin/Release/libSvtAv1Enc.a prints `T` for
 * svt_av1_find_best_sub_pixel_tree, svt_av1_find_best_sub_pixel_tree_pruned
 * and svt_aom_fp_mv_err_cost; the other fourteen are `static` and print
 * nothing). The fourteen statics are reachable ONLY through those two entry
 * points, so building SUBPEL_MOTION_SEARCH_PARAMS + MacroBlockD +
 * ModeDecisionContext from plain scalars here and calling the entry points
 * gives a tier-1 differential (WORKING-ON-THIS.md §4) over the whole tree at
 * once — strictly stronger than fourteen hand-derived vector tests. This is
 * the same shell-construction pattern ref_shims.c uses (see its note at the
 * MacroBlockD/TileInfo/PCS shells) and inter_me_shims.c's hme_level_2 wrapper.
 *
 * `_c` SPELLINGS ARE DELIBERATE. `svt_aom_mefn_ptr[bsize].vf` / `.svf` are
 * RTCD pointers to whatever SIMD tier this host dispatches to. The Rust port
 * transcribes the `_c` kernels, so this shim installs an AomVarianceFnPtr
 * whose `vf`/`svf` are the `_c` symbols by name. `use_rtcd` swaps in
 * `svt_aom_mefn_ptr[bsize]` instead, SO THAT a test can ask whether the host's
 * SIMD tier agrees with `_c` rather than silently comparing against it.
 *
 * NO SHIM STATE. Every buffer below lives on the stack or in a per-call
 * malloc; cargo runs a test binary's tests on several threads and a `static`
 * scratch buffer here would race.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "mv.h"
#include "av1me.h"
#include "mcomp.h"
#include "md_process.h"
#include "aom_dsp_rtcd.h"

void       init_fn_ptr(void);
void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);

/* One-shot RTCD init. The flag is written with the same value by every racing
 * thread and guards only redundant work, never correctness. */
static int md_subpel_rtcd_done = 0;
static void md_subpel_ensure_rtcd(void) {
    if (!md_subpel_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        /* Populates svt_aom_mefn_ptr from the RTCD pointers; without it the
         * `use_rtcd` control reads a table of NULLs. */
        init_fn_ptr();
        md_subpel_rtcd_done = 1;
    }
}

/* ---- variance.c: the vf / svf families, `_c` spellings ---- */

#define DECL_VAR(W, H)                                                                                    \
    uint32_t svt_aom_variance##W##x##H##_c(const uint8_t*, int, const uint8_t*, int, uint32_t*);           \
    uint32_t svt_aom_sub_pixel_variance##W##x##H##_c(                                                     \
        const uint8_t*, int, int, int, const uint8_t*, int, uint32_t*);
DECL_VAR(128, 128) DECL_VAR(128, 64) DECL_VAR(64, 128) DECL_VAR(64, 64) DECL_VAR(64, 32)
DECL_VAR(32, 64) DECL_VAR(32, 32) DECL_VAR(32, 16) DECL_VAR(16, 32) DECL_VAR(16, 16)
DECL_VAR(16, 8) DECL_VAR(8, 16) DECL_VAR(8, 8) DECL_VAR(8, 4) DECL_VAR(4, 8)
DECL_VAR(4, 4) DECL_VAR(4, 16) DECL_VAR(16, 4) DECL_VAR(8, 32) DECL_VAR(32, 8)
DECL_VAR(16, 64) DECL_VAR(64, 16)
#undef DECL_VAR

static AomVarianceFn md_subpel_pick_vf(int w, int h) {
#define PICK(W, H) \
    if (w == (W) && h == (H)) return svt_aom_variance##W##x##H##_c;
    PICK(128, 128) PICK(128, 64) PICK(64, 128) PICK(64, 64) PICK(64, 32)
    PICK(32, 64) PICK(32, 32) PICK(32, 16) PICK(16, 32) PICK(16, 16)
    PICK(16, 8) PICK(8, 16) PICK(8, 8) PICK(8, 4) PICK(4, 8)
    PICK(4, 4) PICK(4, 16) PICK(16, 4) PICK(8, 32) PICK(32, 8)
    PICK(16, 64) PICK(64, 16)
#undef PICK
    return NULL;
}

static AomSubpixVarianceFn md_subpel_pick_svf(int w, int h) {
#define PICK(W, H) \
    if (w == (W) && h == (H)) return svt_aom_sub_pixel_variance##W##x##H##_c;
    PICK(128, 128) PICK(128, 64) PICK(64, 128) PICK(64, 64) PICK(64, 32)
    PICK(32, 64) PICK(32, 32) PICK(32, 16) PICK(16, 32) PICK(16, 16)
    PICK(16, 8) PICK(8, 16) PICK(8, 8) PICK(8, 4) PICK(4, 8)
    PICK(4, 4) PICK(4, 16) PICK(16, 4) PICK(8, 32) PICK(32, 8)
    PICK(16, 64) PICK(64, 16)
#undef PICK
    return NULL;
}

/* Returns UINT32_MAX when (w, h) is not one of the 22 instantiated sizes. */
uint32_t ref_sub_pixel_variance(int w, int h, const uint8_t* a, int a_stride, int xoffset, int yoffset,
                                const uint8_t* b, int b_stride, uint32_t* sse) {
    AomSubpixVarianceFn f = md_subpel_pick_svf(w, h);
    if (!f) {
        return UINT32_MAX;
    }
    return f(a, a_stride, xoffset, yoffset, b, b_stride, sse);
}

/* The `vf` twin, so a test can prove the svf oracle's final step is the same
 * variance the port's `variance_diff_sse` computes. */
uint32_t ref_subpel_variance_vf(int w, int h, const uint8_t* a, int a_stride, const uint8_t* b, int b_stride,
                                uint32_t* sse) {
    AomVarianceFn f = md_subpel_pick_vf(w, h);
    if (!f) {
        return UINT32_MAX;
    }
    return f(a, a_stride, b, b_stride, sse);
}

/* ---- mcomp.c:775 svt_aom_fp_mv_err_cost (EXPORTED) ----
 *
 * Wraps svt_mv_err_cost_ -> svt_mv_err_cost, i.e. the whole 5-way MV_COST_TYPE
 * dispatch. `mvjcost` is 4 ints; `mvcost_row` / `mvcost_col` are MV_VALS-long
 * and this shim centres them at MV_MAX exactly as
 * svt_av1_build_nmv_cost_table's consumers do. Pass mvcost_row == NULL to
 * exercise the `if (mvcost)` NULL arm of MV_COST_ENTROPY. */
int ref_fp_mv_err_cost(int mv_x, int mv_y, int ref_mv_x, int ref_mv_y, int mv_cost_type, const int* mvjcost,
                       const int* mvcost_row, const int* mvcost_col, int error_per_bit) {
    Mv mv, ref_mv;
    mv.x                = (int16_t)mv_x;
    mv.y                = (int16_t)mv_y;
    ref_mv.x            = (int16_t)ref_mv_x;
    ref_mv.y            = (int16_t)ref_mv_y;
    svt_mv_cost_param p = {0};
    p.ref_mv            = &ref_mv;
    p.mv_cost_type      = (MV_COST_TYPE)mv_cost_type;
    p.mvjcost           = mvjcost;
    if (mvcost_row) {
        p.mvcost[0] = mvcost_row + MV_MAX;
        p.mvcost[1] = mvcost_col + MV_MAX;
    } else {
        p.mvcost[0] = NULL;
        p.mvcost[1] = NULL;
    }
    p.error_per_bit = error_per_bit;
    return svt_aom_fp_mv_err_cost(&mv, &p);
}

/* ---- mcomp.c:599 / :683, the two EXPORTED sub-pel tree entry points ----
 *
 * `ref_alloc` is the whole reference allocation and `ref_base` the index of
 * the block's (0, 0) inside it, so the negative offsets svt_get_buf_from_mv
 * produces for negative MVs stay inside the caller's buffer. Same shape as
 * inter_me/obmc_search.rs's `pre` / `pre_base` pair.
 *
 * `ctx_*` reproduce the ModeDecisionContext fields the unpruned entry point
 * reads (pd_pass, md_subpel_me_ctrls.{mvp_th,hp_mv_th}, best_fp_mvp_dist,
 * best_fp_mvp_idx, mvp_array) plus the fp_me_dist it writes. `use_ctx == 0`
 * passes ictx = NULL, which is the arm every caller outside PD_PASS_1 takes.
 */
typedef struct RefSubpelArgs {
    int      pruned;
    int      use_rtcd;
    int      use_ctx;
    /* buffers */
    const uint8_t* src;
    int            src_stride;
    const uint8_t* ref_alloc;
    int            ref_base;
    int            ref_stride;
    /* geometry */
    int w, h, bsize;
    /* SUBPEL_MOTION_SEARCH_PARAMS scalars */
    int allow_hp;
    int forced_stop;
    int iters_per_step;
    int pred_variance_th;
    int abs_th_mult;
    int round_dev_th;
    int skip_diag_refinement;
    int search_stage;
    int list_idx;
    int ref_idx;
    int subpel_search_type;
    int bias_fp;
    int col_min, col_max, row_min, row_max;
    /* svt_mv_cost_param */
    int          ref_mv_x, ref_mv_y;
    int          mv_cost_type;
    const int*   mvjcost;
    const int*   mvcost_row;
    const int*   mvcost_col;
    int          error_per_bit;
    int          early_exit_th;
    /* ModeDecisionContext fields (only read when use_ctx) */
    int      pd_pass;
    int      mvp_th;
    int      hp_mv_th;
    uint32_t best_fp_mvp_dist;
    int      best_fp_mvp_x;
    int      best_fp_mvp_y;
    /* start */
    int start_mv_x, start_mv_y;
    /* outputs */
    int*      best_mv_x;
    int*      best_mv_y;
    int*      distortion;
    uint32_t* sse1;
    uint32_t* fp_me_dist_out;
} RefSubpelArgs;

unsigned int ref_md_subpel_tree(const RefSubpelArgs* a) {
    md_subpel_ensure_rtcd();

    /* xd: only mi_row / mi_col are read (by svt_aom_upsampled_pred, which
     * ignores them in the `_c` arm). Zeroed on the stack. */
    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));

    AomVarianceFnPtr vfp;
    if (a->use_rtcd) {
        vfp = svt_aom_mefn_ptr[a->bsize];
        if (!vfp.vf || !vfp.svf) {
            return UINT32_MAX;
        }
    } else {
        memset(&vfp, 0, sizeof(vfp));
        vfp.vf  = md_subpel_pick_vf(a->w, a->h);
        vfp.svf = md_subpel_pick_svf(a->w, a->h);
        if (!vfp.vf || !vfp.svf) {
            return UINT32_MAX;
        }
    }

    svt_buf_2d src_buf = {(uint8_t*)a->src, a->w, a->h, a->src_stride};
    svt_buf_2d ref_buf = {(uint8_t*)a->ref_alloc + a->ref_base, a->w, a->h, a->ref_stride};

    SUBPEL_MOTION_SEARCH_PARAMS ms;
    memset(&ms, 0, sizeof(ms));
    ms.allow_hp             = a->allow_hp;
    ms.forced_stop          = (SUBPEL_FORCE_STOP)a->forced_stop;
    ms.iters_per_step       = a->iters_per_step;
    ms.pred_variance_th     = a->pred_variance_th;
    ms.abs_th_mult          = (uint8_t)a->abs_th_mult;
    ms.round_dev_th         = a->round_dev_th;
    ms.skip_diag_refinement = (uint8_t)a->skip_diag_refinement;
    ms.search_stage         = (SUBPEL_STAGE)a->search_stage;
    ms.list_idx             = (uint8_t)a->list_idx;
    ms.ref_idx              = (uint8_t)a->ref_idx;
    ms.mv_limits.col_min    = a->col_min;
    ms.mv_limits.col_max    = a->col_max;
    ms.mv_limits.row_min    = a->row_min;
    ms.mv_limits.row_max    = a->row_max;

    Mv ref_mv;
    ref_mv.x                     = (int16_t)a->ref_mv_x;
    ref_mv.y                     = (int16_t)a->ref_mv_y;
    ms.mv_cost_params.ref_mv     = &ref_mv;
    ms.mv_cost_params.mv_cost_type = (MV_COST_TYPE)a->mv_cost_type;
    ms.mv_cost_params.mvjcost      = a->mvjcost;
    if (a->mvcost_row) {
        ms.mv_cost_params.mvcost[0] = a->mvcost_row + MV_MAX;
        ms.mv_cost_params.mvcost[1] = a->mvcost_col + MV_MAX;
    }
    ms.mv_cost_params.error_per_bit = a->error_per_bit;
    ms.mv_cost_params.early_exit_th = a->early_exit_th;

    ms.var_params.vfp                = &vfp;
    ms.var_params.subpel_search_type = (SUBPEL_SEARCH_TYPE)a->subpel_search_type;
    ms.var_params.ms_buffers.src     = &src_buf;
    ms.var_params.ms_buffers.ref     = &ref_buf;
    ms.var_params.w                  = a->w;
    ms.var_params.h                  = a->h;
    ms.var_params.bias_fp            = a->bias_fp;

    ModeDecisionContext* ctx = NULL;
    if (a->use_ctx) {
        ctx = (ModeDecisionContext*)calloc(1, sizeof(ModeDecisionContext));
        if (!ctx) {
            return UINT32_MAX;
        }
        ctx->pd_pass                                       = (PdPass)a->pd_pass;
        ctx->md_subpel_me_ctrls.mvp_th                     = a->mvp_th;
        ctx->md_subpel_me_ctrls.hp_mv_th                   = a->hp_mv_th;
        ctx->best_fp_mvp_dist[a->list_idx][a->ref_idx]     = a->best_fp_mvp_dist;
        ctx->best_fp_mvp_idx[a->list_idx][a->ref_idx]      = 0;
        ctx->mvp_array[a->list_idx][a->ref_idx][0].x       = (int16_t)a->best_fp_mvp_x;
        ctx->mvp_array[a->list_idx][a->ref_idx][0].y       = (int16_t)a->best_fp_mvp_y;
    }

    Mv start_mv;
    start_mv.x = (int16_t)a->start_mv_x;
    start_mv.y = (int16_t)a->start_mv_y;
    Mv           best_mv    = {{0, 0}};
    int          distortion = 0;
    unsigned int sse1       = 0;
    unsigned int r;
    if (a->pruned) {
        r = (unsigned int)svt_av1_find_best_sub_pixel_tree_pruned(
            ctx, &xd, NULL, &ms, start_mv, &best_mv, &distortion, &sse1, (BlockSize)a->bsize);
    } else {
        r = (unsigned int)svt_av1_find_best_sub_pixel_tree(
            ctx, &xd, NULL, &ms, start_mv, &best_mv, &distortion, &sse1, (BlockSize)a->bsize);
    }
    *a->best_mv_x  = best_mv.x;
    *a->best_mv_y  = best_mv.y;
    *a->distortion = distortion;
    *a->sse1       = sse1;
    if (a->fp_me_dist_out) {
        *a->fp_me_dist_out = ctx ? ctx->fp_me_dist[a->list_idx][a->ref_idx] : 0;
    }
    free(ctx);
    return r;
}
