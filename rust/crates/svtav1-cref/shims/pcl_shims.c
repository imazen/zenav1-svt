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
