/*
 * C shims for the PER-BLOCK bitstream-emission differential — the pieces of
 * `write_modes_b` / `svt_aom_write_modes_sb` (Codec/entropy_coding.c) that the
 * wx-entropy lane ports.
 *
 * Its own translation unit, like entropy_inter_shims.c beside it, so this lane
 * never shares an editable file with a concurrent lane.
 *
 * TWO evidence tiers live here and the entry points say which they are:
 *
 *  - tier 1 (`docs/WORKING-ON-THIS.md` §4): a call into a REAL exported symbol,
 *    i.e. into the RELEASE ARCHIVE's compiled code.
 *      svt_aom_is_masked_compound_type      inter_prediction.c:34
 *      svt_aom_get_wedge_params_bits        inter_prediction.c:2053
 *      svt_aom_get_wedge_bits_lookup        inter_prediction.c:2019
 *      svt_aom_is_interintra_wedge_used     inter_prediction.c:2015
 *
 *  - tier 1-header: `static INLINE` in a HEADER, so there is no symbol to call.
 *    This TU compiles the header's own source text, which is the C source but
 *    NOT the archive's copy of it — a compiler-flag difference between this TU
 *    and the archive could in principle diverge (see WORKING-ON-THIS §5 trap 3,
 *    `-DNDEBUG` as an ABI flag). Stronger than a transcription, weaker than a
 *    call into the archive; labelled as such in the test.
 *      is_interinter_compound_used          inter_prediction.h:288
 *      is_any_masked_compound_used          inter_prediction.h:303
 *      block_signals_txsize                 entropy_coding.c:4418 (see below)
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
#include "common_utils.h"
#include "inter_prediction.h"
#include "coding_unit.h"
#include "pcs.h"

/* ---- the exported declarations this TU drives ---- */
int     svt_aom_is_masked_compound_type(COMPOUND_TYPE type);
int     svt_aom_get_wedge_params_bits(BlockSize bsize);
int32_t svt_aom_get_wedge_bits_lookup(BlockSize bsize);
int     svt_aom_is_interintra_wedge_used(BlockSize bsize);

/* ---------------------------------------------------------------------------
 * Compound-type predicates (write_modes_b step 9 / step 7 gates).
 * ------------------------------------------------------------------------ */

/* tier 1 — exported. */
int ref_eb_is_masked_compound_type(int comp_type) {
    return svt_aom_is_masked_compound_type((COMPOUND_TYPE)comp_type);
}

/* tier 1 — exported. */
int ref_eb_wedge_params_bits(int bsize) { return svt_aom_get_wedge_params_bits((BlockSize)bsize); }

/* tier 1 — exported. */
int ref_eb_wedge_bits_lookup(int bsize) {
    return (int)svt_aom_get_wedge_bits_lookup((BlockSize)bsize);
}

/* tier 1 — exported. */
int ref_eb_is_interintra_wedge_used(int bsize) {
    return svt_aom_is_interintra_wedge_used((BlockSize)bsize);
}

/* tier 1-header — `static INLINE` in inter_prediction.h:284. */
int ref_eb_is_comp_ref_allowed(int bsize) { return (int)is_comp_ref_allowed((BlockSize)bsize); }

/* tier 1-header — `static INLINE` in inter_prediction.h:288. */
int ref_eb_is_interinter_compound_used(int comp_type, int bsize) {
    return is_interinter_compound_used((CompoundType)comp_type, (BlockSize)bsize);
}

/* tier 1-header — `static INLINE` in inter_prediction.h:303. */
int ref_eb_is_any_masked_compound_used(int bsize) {
    return is_any_masked_compound_used((BlockSize)bsize);
}

/* ---------------------------------------------------------------------------
 * set_mi_row_col (entropy_coding.c:4681) — tier 1, EXPORTED.
 *
 * `nm -g libSvtAv1Enc.a` prints `T _set_mi_row_col`, so this reaches the
 * release archive's own compiled copy. Two contract details the shim has to
 * honour rather than approximate (WORKING-ON-THIS §5 trap 4 — the shim must
 * hand C what the ENCODER hands it):
 *
 *  - It reads exactly one field of `PictureControlSet` (`mi_grid_base`), but
 *    it reads it at the ARCHIVE's offset, so the struct must be the real one.
 *    A `calloc(1, sizeof(PictureControlSet))` gives that (the pattern
 *    inter_mvp_shims.c already uses for `setup_ref_mv_list`); a hand-rolled
 *    stand-in would put the pointer somewhere else.
 *  - It stores `xd->mi[-mi_stride]` / `xd->mi[-1]` — VALUES out of the mi
 *    grid, not indices. To report them back as indices the grid is filled
 *    with `(MbModeInfo*)(uintptr_t)(i + 1)` sentinels. They are never
 *    dereferenced by `set_mi_row_col` or by this shim; the `+ 1` keeps index
 *    0 distinguishable from the NULL an unavailable neighbour gets.
 * ------------------------------------------------------------------------ */

void set_mi_row_col(PictureControlSet* pcs, MacroBlockD* xd, TileInfo* tile, int mi_row, int bh, int mi_col, int bw,
                    uint32_t mi_stride, int mi_rows, int mi_cols);

/* out[12]: top, bottom, left, right, up_avail, left_avail, above_idx,
 * left_idx, n8_w, n8_h, is_sec_rect, mi_offset.  A neighbour index of -1 is
 * C's NULL. Returns 0 on success, -1 if the shim could not allocate. */
int ref_eb_set_mi_row_col(int mi_row, int bh, int mi_col, int bw, int mi_stride, int mi_rows, int mi_cols,
                          int tile_mi_row_start, int tile_mi_col_start, int32_t out[12]) {
    PictureControlSet* pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    MacroBlockD*       xd   = (MacroBlockD*)calloc(1, sizeof(*xd));
    TileInfo*          tile = (TileInfo*)calloc(1, sizeof(*tile));
    const size_t       n    = (size_t)mi_rows * (size_t)mi_stride + (size_t)mi_cols + 1u;
    MbModeInfo**       grid = (MbModeInfo**)calloc(n, sizeof(MbModeInfo*));
    if (!pcs || !xd || !tile || !grid) {
        free(pcs);
        free(xd);
        free(tile);
        free(grid);
        return -1;
    }
    for (size_t i = 0; i < n; ++i) {
        grid[i] = (MbModeInfo*)(uintptr_t)(i + 1u);
    }
    pcs->mi_grid_base   = grid;
    tile->mi_row_start  = tile_mi_row_start;
    tile->mi_col_start  = tile_mi_col_start;

    set_mi_row_col(pcs, xd, tile, mi_row, bh, mi_col, bw, (uint32_t)mi_stride, mi_rows, mi_cols);

    out[0]  = xd->mb_to_top_edge;
    out[1]  = xd->mb_to_bottom_edge;
    out[2]  = xd->mb_to_left_edge;
    out[3]  = xd->mb_to_right_edge;
    out[4]  = xd->up_available;
    out[5]  = xd->left_available;
    out[6]  = xd->above_mbmi ? (int32_t)((uintptr_t)xd->above_mbmi - 1u) : -1;
    out[7]  = xd->left_mbmi ? (int32_t)((uintptr_t)xd->left_mbmi - 1u) : -1;
    out[8]  = xd->n8_w;
    out[9]  = xd->n8_h;
    out[10] = xd->is_sec_rect;
    out[11] = (int32_t)(mi_row * mi_stride + mi_col);

    free(grid);
    free(tile);
    free(xd);
    free(pcs);
    return 0;
}
