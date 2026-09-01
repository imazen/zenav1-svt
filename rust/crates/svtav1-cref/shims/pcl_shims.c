/*
 * C shims for the product_coding_loop.c candidate-STAGING lane (wx-pcl).
 *
 * Kept in its OWN translation unit so this lane never shares an editable
 * file with the concurrent MD / inter lanes.
 *
 * Every function here drives a REAL exported SVT-AV1 symbol (evidence
 * tier 1, docs/WORKING-ON-THIS.md section 4). Linkage was checked with
 * `nm -g Bin/Release/libSvtAv1Enc.a`, not inferred from a prefix — the
 * file is full of both traps (prefixed `static`s, unprefixed exports):
 *
 *   sort_full_cost_based_candidates  product_coding_loop.c:1438  (T, no prefix)
 *   chroma_complexity_check_pred     product_coding_loop.c:6013  (T, no prefix)
 *
 * Neither has a prototype in any header, so both are declared here rather
 * than left to C99 implicit declaration.
 *
 * State discipline: every shim keeps its scratch on the STACK or in a
 * per-call calloc, because nextest runs a binary's tests on several
 * threads and a file-scope buffer would race.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "md_process.h"
#include "mode_decision.h"

void sort_full_cost_based_candidates(ModeDecisionContext* ctx, uint32_t num_of_cand_to_sort,
                                     uint32_t* cand_buff_indices);

/*
 * Drive the real exported exchange sort.
 *
 * `costs[i]` is buffer `i`'s full cost. The shim builds the minimum state
 * the function touches — `ctx->cand_bf_ptr_array[i]->full_cost` — and
 * nothing else, because that is genuinely all it reads (:1438-1452).
 *
 * Returns 0 on success, -1 if an allocation failed.
 */
int32_t ref_pcl_sort_full_cost(const uint64_t* costs, uint32_t num_buffers,
                               const uint32_t* in_indices, uint32_t num_to_sort,
                               uint32_t* out_indices) {
    if (num_buffers == 0 || num_to_sort == 0) {
        return 0;
    }
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(ModeDecisionContext));
    ModeDecisionCandidateBuffer* bufs =
        (ModeDecisionCandidateBuffer*)calloc(num_buffers, sizeof(ModeDecisionCandidateBuffer));
    ModeDecisionCandidateBuffer** arr =
        (ModeDecisionCandidateBuffer**)calloc(num_buffers, sizeof(ModeDecisionCandidateBuffer*));
    uint64_t* cost_store = (uint64_t*)calloc(num_buffers, sizeof(uint64_t));
    if (!ctx || !bufs || !arr || !cost_store) {
        free(ctx);
        free(bufs);
        free(arr);
        free(cost_store);
        return -1;
    }
    for (uint32_t i = 0; i < num_buffers; i++) {
        cost_store[i]   = costs[i];
        bufs[i].full_cost = &cost_store[i];
        arr[i]            = &bufs[i];
    }
    ctx->cand_bf_ptr_array = arr;

    memcpy(out_indices, in_indices, num_to_sort * sizeof(uint32_t));
    sort_full_cost_based_candidates(ctx, num_to_sort, out_indices);

    free(cost_store);
    free(arr);
    free(bufs);
    free(ctx);
    return 0;
}

/* ------------------------------------------------------------------ *
 * chroma_complexity_check_pred (product_coding_loop.c:6013, EXPORTED)
 * ------------------------------------------------------------------ */

#include "pic_buffer_desc.h"
#include "utility.h"
#include "aom_dsp_rtcd.h"

void chroma_complexity_check_pred(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_buffer,
                                  EbPictureBufferDesc* input_pic, BlockLocation* loc,
                                  uint8_t use_var);

/*
 * The variance arm goes through `svt_aom_mefn_ptr[bsize_uv].vf`, and that is
 * TWO levels of null-until-initialised, which is exactly WORKING-ON-THIS
 * section 5 trap 2 ("nm one level BELOW the function you call"):
 *
 *   1. `svt_aom_mefn_ptr` (av1me.c:24) is a COMMON symbol — zeroed at load —
 *      populated only by `init_fn_ptr` (av1me.c:26), which is exported but is
 *      NOT part of the RTCD setup. Without it `.vf` is NULL and the call
 *      segfaults; measured, this shim crashed the test binary before the call
 *      below was added.
 *   2. `init_fn_ptr` COPIES the `svt_aom_variance<W>x<H>` dispatch pointers,
 *      which the RTCD setup writes. So the RTCD setup must run FIRST;
 *      swapping these two lines would install null variance kernels.
 *
 * Both are called unconditionally rather than behind a static flag, because
 * nextest runs a binary's tests on several threads and each of these only
 * ever stores the same pointer values.
 */
/* Declared here rather than included: neither has a public header prototype. */
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);
void       init_fn_ptr(void);

static void pcl_ensure_rtcd(void) {
    svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    init_fn_ptr();
}

/*
 * `_DEBUG` note (WORKING-ON-THIS section 5, trap 3 — flags are ABI): BlockGeom
 * carries an extra `mds_idx` member under `#if _DEBUG` (utility.h:59). Neither
 * the Release archive nor this shim defines `_DEBUG`, so both see the same
 * layout; the geom below is built here rather than borrowed from the encoder's
 * table so the four fields the function reads are explicit.
 *
 * Every plane is COPIED into shim-owned memory of `stride * bheight_uv` bytes,
 * which covers every read the function makes: the SADs walk
 * `bheight_uv >> shift` rows at `stride << shift`, and the variance walks
 * `bheight_uv` rows at `stride`.
 *
 * Returns 0 on success, -1 on allocation failure.
 */
int32_t ref_pcl_chroma_complexity_check_pred(int32_t prior_chroma, int32_t prior_cfl,
                                             int32_t bwidth_uv, int32_t bheight_uv,
                                             int32_t bsize_uv, const uint8_t* in_y,
                                             int32_t in_y_stride, const uint8_t* in_u,
                                             const uint8_t* in_v, int32_t in_uv_stride,
                                             const uint8_t* pr_y, int32_t pr_y_stride,
                                             const uint8_t* pr_u, const uint8_t* pr_v,
                                             int32_t pr_uv_stride, int32_t use_var,
                                             int32_t cfl_cplx_th, int32_t* out_chroma,
                                             int32_t* out_cfl) {
    pcl_ensure_rtcd();

    const size_t in_y_sz  = (size_t)in_y_stride * (size_t)bheight_uv;
    const size_t in_uv_sz = (size_t)in_uv_stride * (size_t)bheight_uv;
    const size_t pr_y_sz  = (size_t)pr_y_stride * (size_t)bheight_uv;
    const size_t pr_uv_sz = (size_t)pr_uv_stride * (size_t)bheight_uv;

    ModeDecisionContext*         ctx  = (ModeDecisionContext*)calloc(1, sizeof(ModeDecisionContext));
    ModeDecisionCandidateBuffer* bf   = (ModeDecisionCandidateBuffer*)calloc(
        1, sizeof(ModeDecisionCandidateBuffer));
    EbPictureBufferDesc* in_pic  = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));
    EbPictureBufferDesc* pr_pic  = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));
    BlockGeom*           geom    = (BlockGeom*)calloc(1, sizeof(BlockGeom));
    uint8_t*             b_in_y  = (uint8_t*)malloc(in_y_sz);
    uint8_t*             b_in_u  = (uint8_t*)malloc(in_uv_sz);
    uint8_t*             b_in_v  = (uint8_t*)malloc(in_uv_sz);
    uint8_t*             b_pr_y  = (uint8_t*)malloc(pr_y_sz);
    uint8_t*             b_pr_u  = (uint8_t*)malloc(pr_uv_sz);
    uint8_t*             b_pr_v  = (uint8_t*)malloc(pr_uv_sz);

    int32_t rc = -1;
    if (ctx && bf && in_pic && pr_pic && geom && b_in_y && b_in_u && b_in_v && b_pr_y && b_pr_u &&
        b_pr_v) {
        memcpy(b_in_y, in_y, in_y_sz);
        memcpy(b_in_u, in_u, in_uv_sz);
        memcpy(b_in_v, in_v, in_uv_sz);
        memcpy(b_pr_y, pr_y, pr_y_sz);
        memcpy(b_pr_u, pr_u, pr_uv_sz);
        memcpy(b_pr_v, pr_v, pr_uv_sz);

        geom->bwidth_uv  = (uint8_t)bwidth_uv;
        geom->bheight_uv = (uint8_t)bheight_uv;
        geom->bsize_uv   = (BlockSize)bsize_uv;

        in_pic->y_buffer = b_in_y;
        in_pic->u_buffer = b_in_u;
        in_pic->v_buffer = b_in_v;
        in_pic->y_stride = (uint16_t)in_y_stride;
        in_pic->u_stride = (uint16_t)in_uv_stride;
        in_pic->v_stride = (uint16_t)in_uv_stride;

        pr_pic->y_buffer = b_pr_y;
        pr_pic->u_buffer = b_pr_u;
        pr_pic->v_buffer = b_pr_v;
        pr_pic->y_stride = (uint16_t)pr_y_stride;
        pr_pic->u_stride = (uint16_t)pr_uv_stride;
        pr_pic->v_stride = (uint16_t)pr_uv_stride;

        bf->pred = pr_pic;

        ctx->blk_geom            = geom;
        ctx->hbd_md              = 0;
        ctx->chroma_complexity   = (uint8_t)prior_chroma;
        ctx->cfl_complexity      = (uint8_t)prior_cfl;
        ctx->cfl_ctrls.cplx_th   = (uint8_t)cfl_cplx_th;

        BlockLocation loc;
        loc.input_origin_index       = 0;
        loc.input_cb_origin_in_index = 0;

        chroma_complexity_check_pred(ctx, bf, in_pic, &loc, (uint8_t)use_var);

        *out_chroma = (int32_t)ctx->chroma_complexity;
        *out_cfl    = (int32_t)ctx->cfl_complexity;
        rc          = 0;
    }

    free(b_pr_v);
    free(b_pr_u);
    free(b_pr_y);
    free(b_in_v);
    free(b_in_u);
    free(b_in_y);
    free(geom);
    free(pr_pic);
    free(in_pic);
    free(bf);
    free(ctx);
    return rc;
}
