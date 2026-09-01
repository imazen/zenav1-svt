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
#include "entropy_coding.h"

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

/* ---------------------------------------------------------------------------
 * The small EXPORTED helpers of entropy_coding.c — all tier 1.
 *
 * Three of them read `xd->above_mbmi` / `xd->left_mbmi` (the POINTERS) and
 * `svt_aom_get_kf_y_mode_ctx` reads the mi GRID through `xd->mi`, so each
 * builds the neighbour state the encoder would have built rather than a
 * stand-in: real `MbModeInfo` values, and a real `xd->mi` pointing into a
 * three-entry grid whose [-1] and [-mi_stride] slots are the neighbours.
 * ------------------------------------------------------------------------ */

int32_t  svt_aom_partition_cdf_length(BlockSize bsize);
uint8_t  av1_get_skip_context(const MacroBlockD* xd);
void     svt_aom_get_kf_y_mode_ctx(const MacroBlockD* xd, uint8_t* above_ctx, uint8_t* left_ctx);
size_t   svt_aom_uleb_size_in_bytes(uint64_t value);
int32_t  svt_aom_uleb_encode(uint64_t value, size_t available, uint8_t* coded_value, size_t* coded_size);
int      svt_aom_allow_palette(int allow_screen_content_tools, BlockSize bsize);
int      svt_aom_get_palette_bsize_ctx(BlockSize bsize);
int      svt_aom_get_palette_mode_ctx(const MacroBlockD* xd);
int      svt_aom_write_uniform_cost(int n, int v);
int32_t  svt_aom_count_primitive_quniform(uint16_t n, uint16_t v);
int32_t  svt_aom_count_primitive_subexpfin(uint16_t n, uint16_t k, uint16_t v);
int32_t  svt_aom_wb_is_byte_aligned(const AomWriteBitBuffer* wb);
uint32_t svt_aom_wb_bytes_written(const AomWriteBitBuffer* wb);
void     svt_aom_wb_write_bit(AomWriteBitBuffer* wb, int32_t bit);
void     svt_aom_wb_write_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits);
void     svt_aom_wb_write_inv_signed_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits);

int ref_eb_partition_cdf_length(int bsize) { return (int)svt_aom_partition_cdf_length((BlockSize)bsize); }

int ref_eb_allow_palette(int allow_sc, int bsize) {
    return svt_aom_allow_palette(allow_sc, (BlockSize)bsize);
}

int ref_eb_palette_bsize_ctx(int bsize) { return svt_aom_get_palette_bsize_ctx((BlockSize)bsize); }

int ref_eb_write_uniform_cost(int n, int v) { return svt_aom_write_uniform_cost(n, v); }

int ref_eb_count_primitive_quniform(int n, int v) {
    return (int)svt_aom_count_primitive_quniform((uint16_t)n, (uint16_t)v);
}

int ref_eb_count_primitive_subexpfin(int n, int k, int v) {
    return (int)svt_aom_count_primitive_subexpfin((uint16_t)n, (uint16_t)k, (uint16_t)v);
}

uint64_t ref_eb_uleb_size_in_bytes(uint64_t value) { return (uint64_t)svt_aom_uleb_size_in_bytes(value); }

/* out must hold at least 16 bytes. Returns C's rc; *out_size gets the size. */
int ref_eb_uleb_encode(uint64_t value, uint64_t available, uint8_t* out, uint64_t* out_size) {
    size_t  sz = 0;
    int32_t rc = svt_aom_uleb_encode(value, (size_t)available, out, &sz);
    *out_size  = (uint64_t)sz;
    return (int)rc;
}

/* A neighbour pair as the three xd-pointer readers see it. `*_valid == 0`
 * means C's NULL pointer, which is a DIFFERENT input from a present
 * neighbour whose field happens to be zero. */
static void eb_fill_neighbors(MacroBlockD* xd, MbModeInfo* above, MbModeInfo* left, int above_valid,
                              int left_valid) {
    xd->above_mbmi = above_valid ? above : NULL;
    xd->left_mbmi  = left_valid ? left : NULL;
}

int ref_eb_get_skip_context(int above_valid, int above_skip, int left_valid, int left_skip) {
    MacroBlockD xd    = {0};
    MbModeInfo  above = {0};
    MbModeInfo  left  = {0};
    above.block_mi.skip = (uint8_t)above_skip;
    left.block_mi.skip  = (uint8_t)left_skip;
    eb_fill_neighbors(&xd, &above, &left, above_valid, left_valid);
    return (int)av1_get_skip_context(&xd);
}

int ref_eb_get_palette_mode_ctx(int above_valid, int above_pal, int left_valid, int left_pal) {
    MacroBlockD xd    = {0};
    MbModeInfo  above = {0};
    MbModeInfo  left  = {0};
    above.palette_mode_info.palette_size = (uint8_t)above_pal;
    left.palette_mode_info.palette_size  = (uint8_t)left_pal;
    eb_fill_neighbors(&xd, &above, &left, above_valid, left_valid);
    return svt_aom_get_palette_mode_ctx(&xd);
}

/* svt_aom_get_kf_y_mode_ctx reads the mi GRID (xd->mi[-1], xd->mi[-stride]),
 * NOT above_mbmi/left_mbmi, and gates on up_available / left_available. The
 * grid here is [left, above-slot..., self] with mi_stride = 2 so that
 * xd->mi[-1] is the left neighbour and xd->mi[-2] is the above one. */
void ref_eb_get_kf_y_mode_ctx(int up_available, int up_mode, int left_available, int left_mode, int32_t out[2]) {
    MacroBlockD  xd     = {0};
    MbModeInfo   above  = {0};
    MbModeInfo   left   = {0};
    MbModeInfo*  grid[3];
    above.block_mi.mode = (PredictionMode)up_mode;
    left.block_mi.mode  = (PredictionMode)left_mode;
    grid[0]             = &above; /* xd->mi[-2] */
    grid[1]             = &left; /* xd->mi[-1] */
    grid[2]             = &left; /* xd->mi[0]  (unread) */
    xd.mi               = &grid[2];
    xd.mi_stride        = 2;
    xd.up_available     = (int8_t)up_available;
    xd.left_available   = (int8_t)left_available;
    uint8_t a = 0, l = 0;
    svt_aom_get_kf_y_mode_ctx(&xd, &a, &l);
    out[0] = a;
    out[1] = l;
}

/* Drive the header bit-buffer primitives over a scripted op list.
 * ops[i] = (kind, data, bits): 0 = write_bit(data), 1 = write_literal(data,
 * bits), 2 = write_inv_signed_literal(data, bits). Returns bytes_written;
 * `*aligned` gets svt_aom_wb_is_byte_aligned. `buf` must hold `cap` bytes and
 * receives the produced bytes. */
uint32_t ref_eb_wb_run(const int32_t* ops, int n_ops, uint8_t* buf, int cap, int32_t* aligned) {
    AomWriteBitBuffer wb = {buf, 0};
    memset(buf, 0, (size_t)cap);
    for (int i = 0; i < n_ops; ++i) {
        const int32_t kind = ops[3 * i];
        const int32_t data = ops[3 * i + 1];
        const int32_t bits = ops[3 * i + 2];
        if (kind == 0) {
            svt_aom_wb_write_bit(&wb, data);
        } else if (kind == 1) {
            svt_aom_wb_write_literal(&wb, data, bits);
        } else {
            svt_aom_wb_write_inv_signed_literal(&wb, data, bits);
        }
    }
    *aligned = svt_aom_wb_is_byte_aligned(&wb);
    return svt_aom_wb_bytes_written(&wb);
}
