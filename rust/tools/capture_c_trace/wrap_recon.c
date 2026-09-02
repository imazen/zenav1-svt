/*
 * wrap_recon.c — ld --wrap interceptor that dumps C's PRE-DEBLOCK recon
 * distortion, so a real-content divergence can be attributed to the right
 * side of the encoder.
 *
 * WHY THIS EXISTS
 * ---------------
 * On real content the frame header's `loop_filter_level` diverges from C on
 * most M2/M3 cells, and the tile's first divergence is a Wiener-tap bit in
 * SB0's loop-restoration syntax. Both are POST-recon searches, and the
 * encoder's chain is
 *
 *   mode decision -> recon -> LF search -> CDEF search -> LR search
 *
 * so a divergence at LF/CDEF/LR is consistent with EITHER a bug in those
 * searches OR a recon that already differs (which would mean the real root is
 * mode decision). Reading the bitstream cannot separate the two: the per-SB LR
 * syntax is written BEFORE the partition symbol, so a mode-decision divergence
 * and a filter-search divergence BOTH surface first as a low tile-op flip.
 * Source-to-source inspection has shown `search_filter_level` is faithful to
 * C line-for-line, which makes its INPUT the open question.
 *
 * `ss_err[0]` is the discriminator. The search always evaluates it (filt_mid
 * starts at 0 for a KEY frame, so the first `try_filter_frame` runs at level
 * 0), and at level 0 the deblocker is a no-op — so ss_err[0] is exactly
 * SSE(source, UNFILTERED recon), with no filtering and no geometry involved.
 *   * C's ss_err[0] != the port's  => the recon already differs => the root is
 *     mode decision, and the LF/LR divergences are downstream symptoms.
 *   * They match                   => the recon agrees and the root is in the
 *     filter searches themselves.
 *
 * NOTE (evidence tier): equal SSE is strong evidence of an equal recon, not
 * proof — SSE is a summary statistic and two different planes can share one.
 * A MISMATCH, however, is proof of a differing recon. This tool is built to
 * answer the mismatch direction decisively; treat a match as "consistent with
 * identical" and confirm any recon-identity claim per-plane before relying on
 * it.
 *
 * WHY WRAP `svt_av1_loop_filter_init`
 * -----------------------------------
 * dlf_process.c:99-102 runs
 *     svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);
 *     svt_av1_loop_filter_init(pcs);
 *     svt_av1_pick_filter_level(..., LPF_PICK_FROM_FULL_IMAGE);
 * so at loop_filter_init the recon is final and NOT yet deblocked — precisely
 * the state whose SSE the search's first trial measures. It is a cross-TU call
 * (declared deblocking_filter.h:40, defined deblocking_filter.c:84), which is
 * what makes it reachable by --wrap at all: `try_filter_frame` calls
 * `picture_sse_calculations` INSIDE deblocking_filter.c, and an intra-TU call
 * is bound direct by the compiler and cannot be wrapped.
 *
 * We report by calling C's own `picture_sse_calculations` (deblocking_filter.h
 * :53) rather than reimplementing it, so the number is definitionally the one
 * the search uses (same aligned dims, same distortion kernel, same source pic).
 *
 * The C tree stays PRISTINE: this is a link-time interposer in the harness, not
 * an edit to Source/.
 *
 * Output (appended to $SVT_RECON_OUT; pure pass-through when unset):
 *   RECON_SSE call=<n> plane=<p> sse=<v>
 * `call` distinguishes the dlf_process invocation from enc_dec_process.c:3401,
 * which also calls loop_filter_init on the sb_based_dlf path.
 *
 * Additionally, if $SVT_RECON_BIN is set, call 0's planes are written raw to
 * <$SVT_RECON_BIN>.p<plane> as tightly-packed rows (stride removed). That is
 * the SSE probe's strict superset: it localizes the FIRST DIFFERING PIXEL, and
 * hence the first divergent superblock/block, instead of only proving that
 * some pixel differs. Safe because `buffer[plane]` already points at the
 * picture origin (pic_buffer_desc.h:37: "Buffer Ptrs point to the start of the
 * picture. If there are borders, the left and above borders will be accessed
 * using a negative offset"), so a row is buffer[p] + r*stride[p] and maps
 * directly onto the port's tightly-packed recon[r*w + c].
 */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common_utils.h"
#include "deblocking_filter.h"
#include "enc_inter_prediction.h"
#include "pcs.h"

#include "coding_loop.h"
#include "inv_transforms.h"
#include "md_process.h"
#include "me_context.h"
#include "motion_estimation.h"

void __real_svt_av1_loop_filter_init(PictureControlSet* pcs);
void __real_svt_av1_loop_filter_frame(EbPictureBufferDesc* frame_buffer, PictureControlSet* pcs, int32_t plane_start,
                                      int32_t plane_end);

/* ---- partition-tree interposer -----------------------------------------
 * svt_aom_pick_partition (coding_loop.h:34, defined product_coding_loop.c:11549)
 * is the depth-recursion entry, but it recurses via test_split_partition ->
 * svt_aom_pick_partition INTRA-TU (product_coding_loop.c:11362), so --wrap only
 * catches the CROSS-TU top-level SB-root call from enc_dec_process.c:3239/3342.
 * That is enough: after the root returns, the ENTIRE pc_tree is populated with
 * each node's CHOSEN partition and winning rd_cost, so we walk it here. This is
 * the C-side analogue of the port's SVTAV1_NSQDBG "TS ... chose=parent/split"
 * dump: it reveals whether C keeps a block (partition != SPLIT) where the port
 * splits it, i.e. the first partition-structure flip. Env: SVT_PICKPART_OUT.
 * Recurses only into the winning SPLIT path (the fully-searched, populated one).
 */
bool __real_svt_aom_pick_partition(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                   MdScan* mds, PC_TREE* pc_tree, int mi_row, int mi_col);

/* Final blocks per shape (Part enum order: N,H,V,H4,V4,HA,HB,VA,VB). */
static const int shape_nblk[PART_S] = {1, 2, 2, 4, 4, 3, 3, 3, 3};

static void dump_pc_tree(FILE* f, const PC_TREE* t) {
    if (!t)
        return;
    fprintf(f, "PICKPART mi=(%d,%d) bsize=%d partition=%d rd=%lld valid=%d\n", t->mi_row, t->mi_col, (int)t->bsize,
            (int)t->partition, (long long)t->rdc.rd_cost, (int)t->rdc.valid);
    /* The PART_N (square NONE) evaluation exists for every TESTED node even
     * when SPLIT wins — it feeds the skip-sub-depth cond1 gate (quad-dist
     * std-dev + nz%), so dump it for direct comparison with the port's BLK
     * records at non-chosen nodes too. */
    if (t->tested_blk[PART_N][0] && t->block_data[PART_N][0]) {
        const BlkStruct* n = t->block_data[PART_N][0];
        fprintf(f,
                "CSQ mi=(%d,%d) bsize=%d cost=%llu mode=%d uv=%d txd=%d nz=%u ye=[%u,%u,%u,%u] dcq=[%u,%u,%u,%u]"
                " ady=%d aduv=%d rate=%llu dist=%llu\n",
                t->mi_row, t->mi_col, (int)t->bsize, (unsigned long long)n->cost, (int)n->block_mi.mode,
                (int)n->block_mi.uv_mode, (int)n->block_mi.tx_depth, (unsigned)n->cnt_nz_coeff, n->eob.y[0],
                n->eob.y[1], n->eob.y[2], n->eob.y[3], (unsigned)n->quant_dc.y[0], (unsigned)n->quant_dc.y[1],
                (unsigned)n->quant_dc.y[2], (unsigned)n->quant_dc.y[3], (int)n->block_mi.angle_delta[0],
                (int)n->block_mi.angle_delta[1], (unsigned long long)n->total_rate, (unsigned long long)n->full_dist);
    }
    /* All TESTED NSQ shapes at this node (bd10 partition-flip drill): the
     * chosen-path dump only shows the winner, but a partition flip needs the
     * cost C assigned to the REJECTED shapes too (e.g. VERT when C keeps NONE).
     * Dump each tested shape's per-sub-block cost/mode + its cost sum. */
    for (int sh = 1; sh < PART_S; ++sh) {
        if (!t->tested_blk[sh][0] || !t->block_data[sh][0])
            continue;
        unsigned long long csum = 0;
        int                nb = shape_nblk[sh], ok = 1;
        for (int nsi = 0; nsi < nb; ++nsi) {
            const BlkStruct* b = t->block_data[sh][nsi];
            if (!b) {
                ok = 0;
                break;
            }
            csum += (unsigned long long)b->cost;
            fprintf(f, "CNSQ mi=(%d,%d) bsize=%d shape=%d nsi=%d cost=%llu mode=%d uv=%d txd=%d rate=%llu dist=%llu\n",
                    t->mi_row, t->mi_col, (int)t->bsize, sh, nsi, (unsigned long long)b->cost, (int)b->block_mi.mode,
                    (int)b->block_mi.uv_mode, (int)b->block_mi.tx_depth, (unsigned long long)b->total_rate,
                    (unsigned long long)b->full_dist);
        }
        if (ok)
            fprintf(f, "CNSQSUM mi=(%d,%d) bsize=%d shape=%d nblk=%d costsum=%llu\n", t->mi_row, t->mi_col,
                    (int)t->bsize, sh, nb, csum);
    }
    if (t->partition == PARTITION_SPLIT) {
        for (int i = 0; i < 4; ++i)
            dump_pc_tree(f, t->split[i]);
        return;
    }
    /* Chosen non-split shape: dump each final block's decided modes so a mode/
     * tx flip is visible without extra instrumentation. Geometry via C's own
     * partition_mi_offset (common_utils.h:239). */
    const Part shape = from_part_to_shape[t->partition];
    for (int nsi = 0; nsi < shape_nblk[shape]; ++nsi) {
        int              mi_row = t->mi_row, mi_col = t->mi_col;
        const BlockSize  sb     = partition_mi_offset(t->bsize, shape, nsi, &mi_row, &mi_col);
        const BlkStruct* b      = t->block_data[shape][nsi];
        if (!b)
            continue;
        fprintf(f,
                "CLEAF mi=(%d,%d) bsize=%d shape=%d nsi=%d mode=%d uv=%d txd=%d ady=%d aduv=%d"
                " txt=[%d,%d,%d,%d] ye=[%u,%u,%u,%u] ue=%u ve=%u\n",
                mi_row, mi_col, (int)sb, (int)shape, nsi, (int)b->block_mi.mode, (int)b->block_mi.uv_mode,
                (int)b->block_mi.tx_depth, (int)b->block_mi.angle_delta[0], (int)b->block_mi.angle_delta[1],
                (int)b->tx_type[0], (int)b->tx_type[1], (int)b->tx_type[2], (int)b->tx_type[3], b->eob.y[0],
                b->eob.y[1], b->eob.y[2], b->eob.y[3], b->eob.u[0], b->eob.v[0]);
    }
}

bool __wrap_svt_aom_pick_partition(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                   MdScan* mds, PC_TREE* pc_tree, int mi_row, int mi_col) {
    bool r = __real_svt_aom_pick_partition(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);
    const char* path = getenv("SVT_PICKPART_OUT");
    /* Dump every SB-root's chosen tree (the cross-TU top-level call fires once
     * per SB). Each node prints its mi, so grep the SB of interest. An optional
     * SVT_PICKPART_MIROW/MICOL pair narrows the dump to one SB root. */
    if (path && *path) {
        const char* mr = getenv("SVT_PICKPART_MIROW");
        const char* mc = getenv("SVT_PICKPART_MICOL");
        if (!mr || !mc || (mi_row == atoi(mr) && mi_col == atoi(mc))) {
            static FILE* f = NULL;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                dump_pc_tree(f, pc_tree);
                fflush(f);
            }
        }
    }
    return r;
}

/* ---- coeff-rate estimator interposer -----------------------------------
 * svt_av1_cost_coeffs_txb (rd_cost.c:355) is what the port's cost_coeffs_txb
 * transcribes, but it is called ONLY from within rd_cost.c (intra-TU), so the
 * compiler binds it direct and --wrap cannot reach it. Its cross-TU wrapper is
 * svt_aom_txb_estimate_coeff_bits (entropy_coding.h:47, defined rd_cost.c,
 * called from full_loop.c / product_coding_loop.c), which stores the very
 * value cost_coeffs_txb returns into *y_txb_coeff_bits (rd_cost.c:1214). So we
 * wrap THAT and log the per-txb luma coeff RATE.
 *
 * The port dumps its cost_coeffs_txb return per txb (SVTAV1_CCOSTDBG). On the
 * first coding block (0,0) both encoders feed identical qcoeff (no neighbours
 * => flat 128 pred => same residual => quant proven faithful), so calls
 * matched by (eob, txsize, tx_type) MUST return the same rate unless the
 * estimator diverges. This decides whether the M2/M3 partition near-tie flips
 * on RATE (this estimator) or on DISTORTION (the recon). Env: SVT_CCOST_OUT.
 */
EbErrorType __real_svt_aom_txb_estimate_coeff_bits(
    ModeDecisionContext* ctx, uint8_t allow_update_cdf, FRAME_CONTEXT* ec_ctx, PictureControlSet* pcs,
    ModeDecisionCandidateBuffer* cand_bf, uint32_t txb_origin_index, uint32_t txb_chroma_origin_index,
    EbPictureBufferDesc* coeff_buffer_sb, uint32_t y_eob, uint32_t cb_eob, uint32_t cr_eob,
    uint64_t* y_txb_coeff_bits, uint64_t* cb_txb_coeff_bits, uint64_t* cr_txb_coeff_bits, TxSize txsize,
    TxSize txsize_uv, TxType tx_type, TxType tx_type_uv, COMPONENT_TYPE component_type);

EbErrorType __wrap_svt_aom_txb_estimate_coeff_bits(
    ModeDecisionContext* ctx, uint8_t allow_update_cdf, FRAME_CONTEXT* ec_ctx, PictureControlSet* pcs,
    ModeDecisionCandidateBuffer* cand_bf, uint32_t txb_origin_index, uint32_t txb_chroma_origin_index,
    EbPictureBufferDesc* coeff_buffer_sb, uint32_t y_eob, uint32_t cb_eob, uint32_t cr_eob,
    uint64_t* y_txb_coeff_bits, uint64_t* cb_txb_coeff_bits, uint64_t* cr_txb_coeff_bits, TxSize txsize,
    TxSize txsize_uv, TxType tx_type, TxType tx_type_uv, COMPONENT_TYPE component_type) {
    EbErrorType ret = __real_svt_aom_txb_estimate_coeff_bits(
        ctx, allow_update_cdf, ec_ctx, pcs, cand_bf, txb_origin_index, txb_chroma_origin_index, coeff_buffer_sb, y_eob,
        cb_eob, cr_eob, y_txb_coeff_bits, cb_txb_coeff_bits, cr_txb_coeff_bits, txsize, txsize_uv, tx_type, tx_type_uv,
        component_type);
    /* SVT_CCOEF_OUT + SVT_CCOEF_XY="x,y": dump the FINAL coded coefficient
     * LEVELS at a pinned block. allow_update_cdf==1 marks the encdec
     * CDF-update pass (update_coeff_cdf, coding_loop.c:1543 — reading the
     * final quantized_coeff buffer at coded_area offsets), i.e. exactly
     * the coeffs the pack writes; MD candidate calls pass 0. update_coeff_
     * cdf itself is same-TU with its caller and cannot be --wrap'd — this
     * cross-TU callee sees the same buffer+offsets. Answers "same eob,
     * which LEVELS differ?" (the 1624307 class). One line per txb. */
    const char* cpath = getenv("SVT_CCOEF_OUT");
    const char* cxy   = getenv("SVT_CCOEF_XY");
    if (cpath && *cpath && allow_update_cdf) {
        /* SVT_CCOEF_XY pins ONE block; UNSET dumps EVERY coded txb of the
         * frame in coding order, which is what a whole-frame join against the
         * port's SVTAV1_PACKTREE_COEFF `PCOEF` dump needs. The pinned mode
         * was the only one until 2026-09-01, and it cannot answer "which of
         * the frame's blocks diverges" without knowing the answer first.
         *
         * BEFORE READING A ZERO HERE, GET A POSITIVE CONTROL. This dump's call
         * site is `update_coeff_cdf` (coding_loop.c:1674), gated on
         * `pcs->cdf_ctrl.update_coef`. On the VIDEO arm
         * `svt_aom_get_update_cdf_level_default` returns 0 above M8
         * (enc_mode_config.c:8517-8519) and `set_cdf_controls` case 0 clears
         * `update_coef`, so at preset >= 9 the file comes out EMPTY — which is
         * a silent probe, not "C coded no coefficients". Preset 6 (level 1,
         * nonzero rdoq_level) is the control that shows it fires. */
        int px = -1, py = -1;
        if (cxy && *cxy)
            sscanf(cxy, "%d,%d", &px, &py);
        if (!(cxy && *cxy) || ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py)) {
            static FILE* qf = NULL;
            if (!qf)
                qf = fopen(cpath, "w");
            if (qf) {
                const int32_t* qy = ((const int32_t*)coeff_buffer_sb->y_buffer) + txb_origin_index;
                const int32_t* qu = ((const int32_t*)coeff_buffer_sb->u_buffer) + txb_chroma_origin_index;
                const int32_t* qv = ((const int32_t*)coeff_buffer_sb->v_buffer) + txb_chroma_origin_index;
                const int      ny = tx_size_wide[txsize] * tx_size_high[txsize];
                const int      nc = tx_size_wide[txsize_uv] * tx_size_high[txsize_uv];
                fprintf(qf, "CCOEF org=(%u,%u) yeob=%u cbeob=%u creob=%u txt=%d txtuv=%d ynz=[", (unsigned)ctx->blk_org_x,
                        (unsigned)ctx->blk_org_y, y_eob, cb_eob, cr_eob, (int)tx_type, (int)tx_type_uv);
                /* All nonzero (raster_idx:level) pairs, capped — the full
                 * symbol content of the txb in a bounded line. */
                int emitted = 0;
                for (int i = 0; i < ny && emitted < 24; ++i)
                    if (qy[i]) fprintf(qf, "%s%d:%d", emitted++ ? "," : "", i, qy[i]);
                fprintf(qf, "] unz=[");
                emitted = 0;
                for (int i = 0; i < nc && emitted < 12; ++i)
                    if (qu[i]) fprintf(qf, "%s%d:%d", emitted++ ? "," : "", i, qu[i]);
                fprintf(qf, "] vnz=[");
                emitted = 0;
                for (int i = 0; i < nc && emitted < 12; ++i)
                    if (qv[i]) fprintf(qf, "%s%d:%d", emitted++ ? "," : "", i, qv[i]);
                fprintf(qf, "]\n");
                fflush(qf);
            }
        }
    }
    const char* path = getenv("SVT_CCOST_OUT");
    if (!path || !*path || allow_update_cdf)
        return ret;
    static int   nlog = 0;
    static FILE* cf   = NULL;
    if (nlog == 0)
        cf = fopen(path, "w");
    if (cf && nlog < 300) {
        if (y_eob > 0 && y_txb_coeff_bits)
            fprintf(cf, "CCOST i=%d plane=0 txs=%d txt=%d eob=%u cost=%llu\n", nlog, (int)txsize, (int)tx_type, y_eob,
                    (unsigned long long)*y_txb_coeff_bits);
        if (cb_eob > 0 && cb_txb_coeff_bits)
            fprintf(cf, "CCOST i=%d plane=1 txs=%d txt=%d eob=%u cost=%llu\n", nlog, (int)txsize_uv, (int)tx_type_uv,
                    cb_eob, (unsigned long long)*cb_txb_coeff_bits);
        if (cr_eob > 0 && cr_txb_coeff_bits)
            fprintf(cf, "CCOST i=%d plane=2 txs=%d txt=%d eob=%u cost=%llu\n", nlog, (int)txsize_uv, (int)tx_type_uv,
                    cr_eob, (unsigned long long)*cr_txb_coeff_bits);
        fflush(cf);
        nlog++;
    }
    return ret;
}

/* ---- partition-search interposer ---------------------------------------
 * svt_aom_partition_rate_cost (rd_cost.h:106, defined rd_cost.c, called
 * cross-TU from the partition search) is invoked per candidate partition of
 * each block C evaluates. Logging (bsize, mi_row, mi_col, partition_type)
 * reveals the SET of block sizes + partitions C's partition search visits at
 * a given SB — which the port's SVTAV1_NSQDBG dump can be diffed against. The
 * port's NSQDBG for SB(0,0) started at bsize 16x16 (not 64/32); if C visits
 * 64x64/32x32 there, the depth-refinement predicted a different depth range
 * (a partition-structure divergence upstream of the tx search). Env:
 * SVT_PART_OUT. Rate-only (no winner), but the visited-set alone localizes a
 * depth-range or shape-set divergence.
 */
int64_t __real_svt_aom_partition_rate_cost(PictureParentControlSet* pcs, const BlockSize bsize, const int mi_row,
                                           const int mi_col, MdRateEstimationContext* md_rate_est_ctx, PartitionType p,
                                           const PartitionContextType left_ctx, const PartitionContextType above_ctx);

int64_t __wrap_svt_aom_partition_rate_cost(PictureParentControlSet* pcs, const BlockSize bsize, const int mi_row,
                                           const int mi_col, MdRateEstimationContext* md_rate_est_ctx, PartitionType p,
                                           const PartitionContextType left_ctx, const PartitionContextType above_ctx) {
    int64_t ret = __real_svt_aom_partition_rate_cost(
        pcs, bsize, mi_row, mi_col, md_rate_est_ctx, p, left_ctx, above_ctx);
    const char* path = getenv("SVT_PART_OUT");
    if (path && *path) {
        static FILE* pf = NULL;
        static int   opened = 0;
        if (!opened) {
            pf     = fopen(path, "w");
            opened = 1;
        }
        /* Window filter: SVT_PART_MI="rowmin,rowmax,colmin,colmax" (mi units,
         * inclusive). Default = the original behaviour (top-left 64x64, mi<16). */
        int rmin = 0, rmax = 15, cmin = 0, cmax = 15;
        const char* win = getenv("SVT_PART_MI");
        if (win && *win)
            sscanf(win, "%d,%d,%d,%d", &rmin, &rmax, &cmin, &cmax);
        if (pf && mi_row >= rmin && mi_row <= rmax && mi_col >= cmin && mi_col <= cmax)
            fprintf(pf, "PART bsize=%d mi=(%d,%d) part=%d rate=%lld lctx=%d actx=%d\n", (int)bsize, mi_row, mi_col,
                    (int)p, (long long)ret, (int)left_ctx, (int)above_ctx);
    }
    return ret;
}

/* ---- final quantized-LEVEL interposer (task #94 bd10 coeff-level diag) ---
 * svt_aom_quantize_inv_quantize (transforms.h:97, defined full_loop.c:1649) is
 * the FULL MD quant+RDOQ entry. At eff-M9 a tx_depth-0 luma leaf's FINAL coded
 * coefficients come from perform_dct_dct_tx -> this fn (product_coding_loop.c
 * :5478, COMPONENT_LUMA), and — because bypass_encdec is on at M4+ and there is
 * NO update_coeff_cdf pass at M9 — the existing CCOEF wrap (gated on
 * allow_update_cdf) NEVER fires there. This wrap dumps `quant_coeff` (the post-
 * quant/post-RDOQ levels) directly after the real call, so C's per-leaf levels
 * are visible at ANY preset. It is cross-TU (called from product_coding_loop.c /
 * coding_loop.c), so --wrap reaches it. Env: SVT_QLEVELS_OUT (file), optional
 * SVT_QLEVELS_XY="x,y" (pin to a block origin in pixels), optional
 * SVT_QLEVELS_COMP (only that component_type; default all). One line per call:
 *   QLEV org=(x,y) comp=<c> txs=<t> txt=<T> eob=<e> enc=<b> bd=<d> qidx=<q> nz=[i:lvl,...]
 * Levels are RASTER order (quant_coeff[raster_idx]), matching the port's
 * SVTAV1_PACKTREE_COEFF dump. Pure pass-through when SVT_QLEVELS_OUT is unset —
 * the C tree stays PRISTINE (link interposer, no Source/ edit). */
uint8_t __real_svt_aom_quantize_inv_quantize(PictureControlSet* pcs, ModeDecisionContext* ctx, int32_t* coeff,
                                             int32_t* quant_coeff, int32_t* recon_coeff, uint32_t qindex,
                                             int32_t segmentation_qp_offset, TxSize txsize, uint16_t* eob,
                                             uint32_t component_type, uint32_t bit_depth, TxType tx_type,
                                             int16_t txb_skip_context, int16_t dc_sign_context,
                                             PredictionMode pred_mode, uint32_t lambda, bool is_encode_pass);

uint8_t __wrap_svt_aom_quantize_inv_quantize(PictureControlSet* pcs, ModeDecisionContext* ctx, int32_t* coeff,
                                             int32_t* quant_coeff, int32_t* recon_coeff, uint32_t qindex,
                                             int32_t segmentation_qp_offset, TxSize txsize, uint16_t* eob,
                                             uint32_t component_type, uint32_t bit_depth, TxType tx_type,
                                             int16_t txb_skip_context, int16_t dc_sign_context,
                                             PredictionMode pred_mode, uint32_t lambda, bool is_encode_pass) {
    uint8_t ret = __real_svt_aom_quantize_inv_quantize(
        pcs, ctx, coeff, quant_coeff, recon_coeff, qindex, segmentation_qp_offset, txsize, eob, component_type,
        bit_depth, tx_type, txb_skip_context, dc_sign_context, pred_mode, lambda, is_encode_pass);
    const char* path = getenv("SVT_QLEVELS_OUT");
    if (!path || !*path)
        return ret;
    const char* xy   = getenv("SVT_QLEVELS_XY");
    const char* comp = getenv("SVT_QLEVELS_COMP");
    if (xy && *xy) {
        int px = -1, py = -1;
        sscanf(xy, "%d,%d", &px, &py);
        if ((int)ctx->blk_org_x != px || (int)ctx->blk_org_y != py)
            return ret;
    }
    if (comp && *comp && atoi(comp) != (int)component_type)
        return ret;
    static FILE* f = NULL;
    if (!f)
        f = fopen(path, "w");
    if (f) {
        const int n = av1_get_max_eob(txsize);
        fprintf(f, "QLEV org=(%u,%u) comp=%u txs=%d txt=%d eob=%u enc=%d bd=%u qidx=%u nz=[",
                (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, component_type, (int)txsize, (int)tx_type,
                (unsigned)*eob, (int)is_encode_pass, (unsigned)bit_depth, (unsigned)qindex);
        int emitted = 0;
        for (int i = 0; i < n && emitted < 48; ++i)
            if (quant_coeff[i])
                fprintf(f, "%s%d:%d", emitted++ ? "," : "", i, quant_coeff[i]);
        /* task #94 bd10 recon-drift: also dump recon_coeff (the DEQUANTIZED
         * coeffs that feed svt_aom_inv_transform_recon_wrapper) so the port's
         * dqcoeff can be compared directly — isolates dequant from inv-tx. */
        fprintf(f, "] dq=[");
        emitted = 0;
        for (int i = 0; i < n && emitted < 48; ++i)
            if (recon_coeff[i])
                fprintf(f, "%s%d:%d", emitted++ ? "," : "", i, recon_coeff[i]);
        /* The PRE-quant transform coefficients. Without these a levels-only
         * dump cannot separate "the residual/transform differs" from "the
         * quantizer decision differs" — the exact split the chroma
         * divergence on the video-mode reference cell turns on. */
        fprintf(f, "] co=[");
        emitted = 0;
        for (int i = 0; i < n && emitted < 48; ++i)
            if (coeff[i])
                fprintf(f, "%s%d:%d", emitted++ ? "," : "", i, coeff[i]);
        fprintf(f, "]\n");
        fflush(f);
    }
    return ret;
}

/* ---- per-SB syntax-rate SEED interposer --------------------------------
 * svt_aom_estimate_syntax_rate (md_rate_estimation.h:175) is called once per
 * SB from enc_dec_process.c:2933/3026 with the averaged FRAME_CONTEXT that
 * seeds ALL of MD's syntax rate tables for that SB. Dumping a few salient CDF
 * rows per call (call index == SB raster index on a single-tile frame) and
 * diffing against the port's SVTAV1_CHAIN_DUMP SEED lines pins the FIRST SB
 * whose rate seed diverges — the "every leaf cost in the SB shifted" class.
 * Env: SVT_SEED_OUT. */
void __real_svt_aom_estimate_syntax_rate(MdRateEstimationContext* r, bool is_i_slice, uint8_t pic_filter_intra_level,
                                         uint8_t allow_screen_content_tools, uint8_t enable_restoration,
                                         uint8_t allow_intrabc, FRAME_CONTEXT* fc);

void __wrap_svt_aom_estimate_syntax_rate(MdRateEstimationContext* r, bool is_i_slice, uint8_t pic_filter_intra_level,
                                         uint8_t allow_screen_content_tools, uint8_t enable_restoration,
                                         uint8_t allow_intrabc, FRAME_CONTEXT* fc) {
    __real_svt_aom_estimate_syntax_rate(r, is_i_slice, pic_filter_intra_level, allow_screen_content_tools,
                                        enable_restoration, allow_intrabc, fc);
    const char* path = getenv("SVT_SEED_OUT");
    if (!path || !*path)
        return;
    static FILE* sf = NULL;
    static int   call = 0;
    if (!sf)
        sf = fopen(path, "w");
    if (!sf)
        return;
    fprintf(sf,
            "SEED sb=%d part0=%u,%u,%u kf00=%u,%u,%u txs00=%u,%u skip0=%u ang0=%u,%u,%u"
            " cfls=%u,%u,%u cfla0=%u,%u,%u xtx=%u,%u,%u\n",
            call++, fc->partition_cdf[0][0], fc->partition_cdf[0][1], fc->partition_cdf[0][2], fc->kf_y_cdf[0][0][0],
            fc->kf_y_cdf[0][0][1], fc->kf_y_cdf[0][0][2], fc->tx_size_cdf[0][0][0], fc->tx_size_cdf[1][0][0],
            fc->skip_cdfs[0][0], fc->angle_delta_cdf[0][0], fc->angle_delta_cdf[0][1], fc->angle_delta_cdf[0][2],
            fc->cfl_sign_cdf[0], fc->cfl_sign_cdf[1], fc->cfl_sign_cdf[2], fc->cfl_alpha_cdf[0][0],
            fc->cfl_alpha_cdf[0][1], fc->cfl_alpha_cdf[0][2], fc->intra_ext_tx_cdf[1][0][0][0],
            fc->intra_ext_tx_cdf[1][0][0][1], fc->intra_ext_tx_cdf[1][0][0][2]);
    fflush(sf);
}

/* ---- per-candidate intra fast-cost interposer ---------------------------
 * svt_aom_intra_fast_cost (rd_cost.h, cross-TU from mode_decision.c's MDS0)
 * prices each intra candidate's SIGNALING (luma mode + fi + angle + uv).
 * Logging (block org/dims, mode, fi, angle, uv, returned cost) at a pinned
 * block quantifies C's candidate rates for direct comparison with the port's
 * SVTAV1_CANDDBG flr/fcr dump. Env: SVT_FASTCOST_OUT + SVT_FASTCOST_XY="x,y"
 * (block origin in pixels). */
uint64_t __real_svt_aom_intra_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                        ModeDecisionCandidateBuffer* cand_bf, uint64_t lambda,
                                        uint64_t luma_distortion);

uint64_t __wrap_svt_aom_intra_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                        ModeDecisionCandidateBuffer* cand_bf, uint64_t lambda,
                                        uint64_t luma_distortion) {
    uint64_t    ret  = __real_svt_aom_intra_fast_cost(pcs, ctx, cand_bf, lambda, luma_distortion);
    const char* path = getenv("SVT_FASTCOST_OUT");
    const char* xy   = getenv("SVT_FASTCOST_XY");
    if (path && *path && xy) {
        int px = -1, py = -1;
        sscanf(xy, "%d,%d", &px, &py);
        if ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py) {
            static FILE* f = NULL;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                /* task #94 bd10: also report the CANDIDATE PREDICTION this cost
                 * was computed from (pred[0], pred[1], pred[stride]) and its
                 * block mean, so the port's predict_unit_hbd output can be
                 * compared directly. hbd_md => pred buffer is uint16_t. */
                const int      bw = block_size_wide[ctx->blk_geom->bsize];
                const int      bh = block_size_high[ctx->blk_geom->bsize];
                const uint32_t ps = cand_bf->pred->y_stride;
                double         pmean = 0.0;
                int            p0 = -1, p1 = -1, pS = -1;
                if (ctx->hbd_md) {
                    const uint16_t* p = (const uint16_t*)cand_bf->pred->y_buffer;
                    p0 = p[0];
                    p1 = p[1];
                    pS = p[ps];
                    for (int r = 0; r < bh; ++r)
                        for (int c = 0; c < bw; ++c) pmean += p[r * ps + c];
                } else {
                    const uint8_t* p = cand_bf->pred->y_buffer;
                    p0 = p[0];
                    p1 = p[1];
                    pS = p[ps];
                    for (int r = 0; r < bh; ++r)
                        for (int c = 0; c < bw; ++c) pmean += p[r * ps + c];
                }
                pmean /= (double)(bw * bh);
                /* C's ACTUAL residual: hadamard_path just wrote it into
                 * cand_bf->residual->y_buffer (int16) before calling us. */
                const int16_t* rs  = (const int16_t*)cand_bf->residual->y_buffer;
                const uint32_t rst = cand_bf->residual->y_stride;
                double         rmean = 0.0;
                int            rmin = 1 << 30, rmax = -(1 << 30);
                for (int r = 0; r < bh; ++r)
                    for (int c = 0; c < bw; ++c) {
                        const int v = rs[r * rst + c];
                        rmean += v;
                        if (v < rmin) rmin = v;
                        if (v > rmax) rmax = v;
                    }
                rmean /= (double)(bw * bh);
                fprintf(f,
                        "CFAST org=(%u,%u) %ux%u mode=%d fi=%d ang=%d uv=%d uvang=%d dist=%llu lam=%llu cost=%llu "
                        "hbd=%d pred0=%d pred1=%d predS=%d predmean=%.2f dtype=%d hadblk=%d subres=%d "
                        "rawsatd=%llu res0=%d res1=%d resmean=%.2f resmin=%d resmax=%d rstride=%u\n",
                        (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, block_size_wide[ctx->blk_geom->bsize],
                        block_size_high[ctx->blk_geom->bsize], (int)cand_bf->cand->block_mi.mode,
                        (int)cand_bf->cand->block_mi.filter_intra_mode, (int)cand_bf->cand->block_mi.angle_delta[0],
                        (int)cand_bf->cand->block_mi.uv_mode, (int)cand_bf->cand->block_mi.angle_delta[1],
                        (unsigned long long)luma_distortion, (unsigned long long)lambda, (unsigned long long)ret,
                        (int)ctx->hbd_md, p0, p1, pS, pmean, (int)ctx->mds0_ctrls.mds0_dist_type,
                        (int)ctx->mds0_use_hadamard_blk, (int)ctx->mds_subres_step,
                        (unsigned long long)cand_bf->luma_fast_dist, (int)rs[0], (int)rs[1], rmean, rmin, rmax,
                        (unsigned)rst);
                fflush(f);
            }
        }
    }
    return ret;
}

/* ---- per-candidate full-cost interposer ---------------------------------
 * svt_aom_full_cost (rd_cost.h, cross-TU from full_loop.c) writes the
 * candidate's full cost at MDS1/MDS3. Logging (block org/dims, md_stage,
 * mode/fi/delta, resulting *cand_bf->full_cost) at a pinned block quantifies
 * C's per-candidate MDS1 costs for comparison with the port's PMDS1 dump.
 * Env: SVT_FULLCOST_OUT + SVT_FULLCOST_XY="x,y". */
void __real_svt_aom_full_cost(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                              uint64_t lambda, uint64_t y_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cb_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cr_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* y_coeff_bits,
                              uint64_t* cb_coeff_bits, uint64_t* cr_coeff_bits);

void __wrap_svt_aom_full_cost(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                              uint64_t lambda, uint64_t y_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cb_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cr_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* y_coeff_bits,
                              uint64_t* cb_coeff_bits, uint64_t* cr_coeff_bits) {
    __real_svt_aom_full_cost(
        pcs, ctx, cand_bf, lambda, y_distortion, cb_distortion, cr_distortion, y_coeff_bits, cb_coeff_bits,
        cr_coeff_bits);
    const char* path = getenv("SVT_FULLCOST_OUT");
    const char* xy   = getenv("SVT_FULLCOST_XY");
    if (path && *path && xy) {
        int px = -1, py = -1;
        sscanf(xy, "%d,%d", &px, &py);
        if ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py) {
            static FILE* f = NULL;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                fprintf(f,
                        "CFULL org=(%u,%u) %ux%u st=%d mode=%d fi=%d ang=%d uv=%d ibc=%d ycb=%llu ydist=%llu "
                        "cost=%llu\n",
                        (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, block_size_wide[ctx->blk_geom->bsize],
                        block_size_high[ctx->blk_geom->bsize], (int)ctx->md_stage, (int)cand_bf->cand->block_mi.mode,
                        (int)cand_bf->cand->block_mi.filter_intra_mode, (int)cand_bf->cand->block_mi.angle_delta[0],
                        (int)cand_bf->cand->block_mi.uv_mode, (int)cand_bf->cand->block_mi.use_intrabc,
                        (unsigned long long)*y_coeff_bits,
                        (unsigned long long)y_distortion[0][0], (unsigned long long)*(cand_bf->full_cost));
                fflush(f);
            }
        }
    }
}

/* ---- POST-DEBLOCK recon interposer (SVT_LFRECON_BIN / SVT_LFRECON_OUT) ---
 * `svt_av1_loop_filter_frame` is declared in deblocking_filter.h:47 and called
 * from dlf_process.c:114 — a CROSS-TU call, so --wrap reaches it. The search's
 * own calls (try_filter_frame, deblocking_filter.c:824) are intra-TU and are
 * bound direct, so this fires exactly ONCE per picture: the FINAL application
 * at the picked levels, on the recon restored from `temp_lf_recon_buffer`.
 *
 * That is what makes it a usable oracle for the port's deblock KERNEL: the
 * port can be asked (SVTAV1_DLF_TRY_BIN + SVTAV1_DLF_TRY_LEVEL) to dump its
 * search trial at C's picked level, and the two planes must be byte-identical
 * given a byte-identical pre-filter recon (which SVT_RECON_BIN proves
 * separately). Without this, a level-search divergence cannot be split into
 * "the kernel differs" vs "the search control differs" — the level number
 * alone is one bit of evidence for a 64-wide landscape.
 *
 * Dumps the ALIGNED plane extent, tightly packed (stride removed), same
 * convention as SVT_RECON_BIN so the two dumps diff against each other and
 * against the port's.
 */
void __wrap_svt_av1_loop_filter_frame(EbPictureBufferDesc* frame_buffer, PictureControlSet* pcs, int32_t plane_start,
                                      int32_t plane_end) {
    __real_svt_av1_loop_filter_frame(frame_buffer, pcs, plane_start, plane_end);

    const char* binpath = getenv("SVT_LFRECON_BIN");
    const char* outpath = getenv("SVT_LFRECON_OUT");
    if ((!binpath || !*binpath) && (!outpath || !*outpath))
        return;

    static int lf_call_idx = 0;
    const int  n           = lf_call_idx++;
    if (n != 0)
        return;

    const bool           is_16bit = pcs->ppcs->scs->is_16bit_pipeline;
    EbPictureBufferDesc* recon    = NULL;
    svt_aom_get_recon_pic(pcs, &recon, is_16bit);
    if (!recon)
        return;

    const uint32_t ss_x    = pcs->ppcs->scs->subsampling_x;
    const uint32_t ss_y    = pcs->ppcs->scs->subsampling_y;
    FrameHeader*   frm_hdr = &pcs->ppcs->frm_hdr;
    FILE*          f       = (outpath && *outpath) ? fopen(outpath, "a") : NULL;
    if (f) {
        fprintf(f,
                "LFRECON_LEVELS y0=%d y1=%d u=%d v=%d sharpness=%d plane_start=%d plane_end=%d\n",
                (int)frm_hdr->loop_filter_params.filter_level[0], (int)frm_hdr->loop_filter_params.filter_level[1],
                (int)frm_hdr->loop_filter_params.filter_level_u, (int)frm_hdr->loop_filter_params.filter_level_v,
                (int)frm_hdr->loop_filter_params.sharpness_level, (int)plane_start, (int)plane_end);
    }
    for (int p = 0; p < 3; ++p) {
        const uint32_t pw  = p ? (pcs->ppcs->aligned_width >> ss_x) : pcs->ppcs->aligned_width;
        const uint32_t ph  = p ? (pcs->ppcs->aligned_height >> ss_y) : pcs->ppcs->aligned_height;
        const uint64_t sse = picture_sse_calculations(pcs, recon, p);
        if (f)
            fprintf(f, "LFRECON_SSE plane=%d sse=%llu w=%u h=%u\n", p, (unsigned long long)sse, pw, ph);
        if (!binpath || !*binpath)
            continue;
        char path[4096];
        snprintf(path, sizeof(path), "%s.p%d", binpath, p);
        FILE* bf = fopen(path, "wb");
        if (!bf)
            continue;
        for (uint32_t r = 0; r < ph; ++r) {
            if (is_16bit)
                fwrite((const uint16_t*)recon->buffer[p] + (size_t)r * recon->stride[p], sizeof(uint16_t), pw, bf);
            else
                fwrite(recon->buffer[p] + (size_t)r * recon->stride[p], 1, pw, bf);
        }
        fclose(bf);
    }
    if (f) {
        fflush(f);
        fclose(f);
    }
}

void __wrap_svt_av1_loop_filter_init(PictureControlSet* pcs) {
    __real_svt_av1_loop_filter_init(pcs);

    const char* path = getenv("SVT_RECON_OUT");
    if (!path || !*path)
        return;
    FILE* f = fopen(path, "a");
    if (!f)
        return;

    /* Per-picture adaptivity inputs the depth refinement (and other levels)
     * key off — C selects pic_block_based_depth_refinement_level per picture
     * from coeff_lvl (+ r0), NOT per preset. One line per call. */
    fprintf(f, "PICCFG coeff_lvl=%d depth_refine_lvl=%d r0_gen=%d r0=%.4f pic_avg_variance=%u qp=%u\n",
            (int)pcs->coeff_lvl, (int)pcs->pic_block_based_depth_refinement_level, (int)pcs->ppcs->r0_gen,
            pcs->ppcs->r0, (unsigned)pcs->ppcs->pic_avg_variance, (unsigned)pcs->scs->static_config.qp);

    static int call_idx = 0;
    const int  n        = call_idx++;

    const bool           is_16bit = pcs->ppcs->scs->is_16bit_pipeline;
    EbPictureBufferDesc* recon    = NULL;
    svt_aom_get_recon_pic(pcs, &recon, is_16bit);
    if (recon) {
        for (int p = 0; p < 3; ++p) {
            const uint64_t sse = picture_sse_calculations(pcs, recon, p);
            fprintf(f, "RECON_SSE call=%d plane=%d sse=%llu\n", n, p, (unsigned long long)sse);
        }

        /* Raw planes for the first (dlf_process) call only — the state whose
         * SSE the search's level-0 trial measures. */
        const char* binpath = getenv("SVT_RECON_BIN");
        if (n == 0 && binpath && *binpath) {
            const uint32_t ss_x = pcs->ppcs->scs->subsampling_x;
            const uint32_t ss_y = pcs->ppcs->scs->subsampling_y;
            /* task #94 bd10: at the 16-bit pipeline the recon buffer is PLAIN
             * PACKED u16 (2 B/px), buffer[p] pre-offset to the frame origin,
             * stride[p] in SAMPLES (uint16_t units) — exactly what
             * picture_sse_calculations feeds svt_full_distortion_kernel16_bits.
             * Dump u16 LE per pixel so the file diffs the port's
             * SVTAV1_BD10_RECON dump (last_recon10_y, tightly packed u16 LE).
             * bd8 (is_16bit false) is byte-UNCHANGED (1 B/px). */
            for (int p = 0; p < 3; ++p) {
                const uint32_t pw = p ? (pcs->ppcs->aligned_width >> ss_x) : pcs->ppcs->aligned_width;
                const uint32_t ph = p ? (pcs->ppcs->aligned_height >> ss_y) : pcs->ppcs->aligned_height;
                char           path[4096];
                snprintf(path, sizeof(path), "%s.p%d", binpath, p);
                FILE* bf = fopen(path, "wb");
                if (!bf)
                    continue;
                /* SELF-CHECK (added while root-causing the bd10 post-pass
                 * recon): compute the SSE from the EXACT walk this dump uses
                 * and print it beside C's own picture_sse_calculations. If the
                 * two disagree, the dumped file is garbage and any "recon
                 * divergence" read off it is an artifact of the walk, not a
                 * port defect. Without this the two failure modes are
                 * indistinguishable — which is how `4*u8+24` got recorded. */
                uint64_t walk_sse = 0;
                if (is_16bit) {
                    const uint16_t* base = (const uint16_t*)recon->buffer[p];
                    const uint16_t* sbase =
                        (const uint16_t*)pcs->input_frame16bit->buffer[p];
                    const uint32_t  sstride = pcs->input_frame16bit->stride[p];
                    for (uint32_t r = 0; r < ph; ++r) {
                        const uint16_t* row = base + (size_t)r * recon->stride[p];
                        const uint16_t* srow = sbase + (size_t)r * sstride;
                        for (uint32_t cc = 0; cc < pw; ++cc) {
                            const int64_t d = (int64_t)srow[cc] - (int64_t)row[cc];
                            walk_sse += (uint64_t)(d * d);
                        }
                        fwrite(row, sizeof(uint16_t), pw, bf);
                    }
                } else {
                    for (uint32_t r = 0; r < ph; ++r)
                        fwrite(recon->buffer[p] + (size_t)r * recon->stride[p], 1, pw, bf);
                }
                fclose(bf);
                fprintf(f, "RECON_WALKSSE plane=%d walk_sse=%llu\n", p, (unsigned long long)walk_sse);
                /* stride/geometry alongside the file: a dump whose stride does
                 * not match the buffer's is silently garbage, and the only way
                 * to tell that from a genuine recon divergence is to print the
                 * walk parameters next to the data. (Measured: at fast presets
                 * the recon desc's `width` is the ALIGNED width while `stride`
                 * carries padding, so a reader assuming stride==w is wrong.) */
                fprintf(f,
                        "RECON_BIN plane=%d w=%u h=%u b16=%d stride=%u desc_w=%u desc_h=%u "
                        "border=%u packed=%d bd=%d -> %s\n",
                        p, pw, ph, (int)is_16bit, (unsigned)recon->stride[p], (unsigned)recon->width,
                        (unsigned)recon->height, (unsigned)recon->border, (int)recon->packed_flag,
                        (int)recon->bit_depth, path);
            }
        }
    }
    fflush(f);
    fclose(f);
}

/* ---- final coded tree interposer ----------------------------------------
 * svt_aom_update_mi_map (adaptive_mv_pred.c:1541, exported T) stamps each
 * FINAL coded block's BlockModeInfo into the mi grid — called once per
 * coded block at EVERY preset (product_coding_loop.c:670 <=M5 walk, :10544
 * M6+ path). One compact line per block to $SVT_CTREE_OUT gives C's final
 * coded tree with zero stderr noise; tools/tree_diff.py joins it against
 * the port's SVTAV1_PACKTREE dump and prints only the flips. */
void __real_svt_aom_update_mi_map(PictureControlSet* pcs, ModeDecisionContext* ctx, const PartitionType part,
                                  const BlockSize bsize, const int mi_row, const int mi_col);

void __wrap_svt_aom_update_mi_map(PictureControlSet* pcs, ModeDecisionContext* ctx, const PartitionType part,
                                  const BlockSize bsize, const int mi_row, const int mi_col) {
    __real_svt_aom_update_mi_map(pcs, ctx, part, bsize, mi_row, mi_col);
    const BlkStruct*     b = ctx->blk_ptr;
    const BlockModeInfo* m = &b->block_mi;
    const char*          path = getenv("SVT_CTREE_OUT");
    static FILE*         f    = NULL;
    if (path && *path && !f)
        f = fopen(path, "w");
    if (f)
        fprintf(f,
            "CTREE mi=(%d,%d) bsize=%d part=%d mode=%d uv=%d fi=%d ady=%d aduv=%d txd=%d pal=%d skip=%d cflidx=%d "
            "cflsgn=%d ibc=%d aibc=%d\n",
            mi_row, mi_col, (int)bsize, (int)part, (int)m->mode, (int)m->uv_mode, (int)m->filter_intra_mode,
            (int)m->angle_delta[0], (int)m->angle_delta[1], (int)m->tx_depth, (int)b->palette_size[0], (int)m->skip,
            (int)m->cfl_alpha_idx, (int)m->cfl_alpha_signs, (int)m->use_intrabc,
            (int)pcs->ppcs->frm_hdr.allow_intrabc);
    if (f)
        fflush(f);

    /* ---- committed per-block INTER DECISION (SVT_CINTER_OUT) -------------
     * The exact `InterModeInfo` the port's `write_inter_mode_info` takes
     * (crates/svtav1-encoder/src/port_entropy_inter/block.rs), so a byte gate
     * on the inter TILE can be built from C's measured decision before the
     * port's own inter mode decision exists. Without this, the decision would
     * have to be GUESSED from the tile bytes, and a guess that reproduces
     * three bytes is not evidence about a decision.
     *
     * `predmv` is what `svt_av1_find_best_ref_mvs_from_stack` left (already
     * lower_mv_precision-rounded), NOT a raw ref-MV-stack entry — the MV the
     * bitstream diffs against.
     *
     * Only INTER blocks are printed (`mode >= NEARESTMV`), so the dump is the
     * inter frames' decisions with no intra noise to cut out. */
    if (m->mode >= NEARESTMV) {
        const char*  ipath = getenv("SVT_CINTER_OUT");
        static FILE* cif   = NULL;
        if (ipath && *ipath && !cif)
            cif = fopen(ipath, "w");
        if (cif) {
            fprintf(cif,
                "CINTER poc=%u mi=(%d,%d) bsize=%d part=%d mode=%d rf=%d,%d "
                "mv0=%d,%d mv1=%d,%d pmv0=%d,%d pmv1=%d,%d "
                "interp=0x%x mm=%d npr=%d ovl=%u imc=%d drl=%d drlctx=%d,%d drlnear=%d,%d "
                "iiu=%d skip=%d skipmode=%d cgi=%d cidx=%d\n",
                (unsigned)pcs->picture_number, mi_row, mi_col, (int)bsize, (int)part, (int)m->mode,
                (int)m->ref_frame[0], (int)m->ref_frame[1], (int)m->mv[0].y, (int)m->mv[0].x,
                (int)m->mv[1].y, (int)m->mv[1].x, (int)b->predmv[0].y, (int)b->predmv[0].x,
                (int)b->predmv[1].y, (int)b->predmv[1].x, (unsigned)m->interp_filters,
                (int)m->motion_mode, (int)m->num_proj_ref, (unsigned)b->overlappable_neighbors,
                (int)b->inter_mode_ctx, (int)b->drl_index, (int)b->drl_ctx[0], (int)b->drl_ctx[1],
                (int)b->drl_ctx_near[0], (int)b->drl_ctx_near[1], (int)m->is_interintra_used,
                (int)m->skip, (int)m->skip_mode, (int)m->comp_group_idx, (int)m->compound_idx);
            fflush(cif);
        }
    }

    /* ---- committed per-block RECON EDGES (SVT_CEDGE_OUT) -----------------
     * blk_ptr->neigh_top_recon_16bit[p] is the block's BOTTOM row and
     * neigh_left_recon_16bit[p] its RIGHT column (:8552-8578) — exactly the
     * samples the below/right neighbours intra-predict from. Dumping them
     * here, right after the block is committed, gives C's MD recon state per
     * block WITHOUT touching the static cfl_prediction family, and joins
     * against the port's committed winner recon. Luma as sums (localisation
     * only); chroma right columns RAW, since the chroma DC base is literally
     * their average. */
    const char* epath = getenv("SVT_CEDGE_OUT");
    if (epath && *epath && ctx->hbd_md) {
        static FILE* ef = NULL;
        if (!ef)
            ef = fopen(epath, "w");
        if (ef) {
            const BlockGeom* g   = ctx->blk_geom;
            unsigned long    lyb = 0, lyr = 0;
            for (int i = 0; i < g->bwidth; ++i) lyb += b->neigh_top_recon_16bit[0][i];
            for (int j = 0; j < g->bheight; ++j) lyr += b->neigh_left_recon_16bit[0][j];
            fprintf(ef, "CEDGE org=(%u,%u) %dx%d lyb=%lu lyr=%lu", (unsigned)ctx->blk_org_x,
                    (unsigned)ctx->blk_org_y, g->bwidth, g->bheight, lyb, lyr);
            /* Raw luma edges for one pinned block (SVT_CEDGE_XY="x,y"): which
             * SAMPLES differ localises the divergence to a single TX unit. */
            const char* rxy = getenv("SVT_CEDGE_XY");
            if (rxy && *rxy) {
                int rx = -1, ry = -1;
                sscanf(rxy, "%d,%d", &rx, &ry);
                if ((int)ctx->blk_org_x == rx && (int)ctx->blk_org_y == ry) {
                    fprintf(ef, " lyB=");
                    for (int i = 0; i < g->bwidth; ++i)
                        fprintf(ef, "%s%u", i ? "," : "", (unsigned)b->neigh_top_recon_16bit[0][i]);
                    fprintf(ef, " lyR=");
                    for (int j = 0; j < g->bheight; ++j)
                        fprintf(ef, "%s%u", j ? "," : "", (unsigned)b->neigh_left_recon_16bit[0][j]);
                }
            }
            if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                fprintf(ef, " uvr=%dx%d cu=", g->bwidth_uv, g->bheight_uv);
                for (int j = 0; j < g->bheight_uv; ++j)
                    fprintf(ef, "%s%u", j ? "," : "", (unsigned)b->neigh_left_recon_16bit[1][j]);
                fprintf(ef, " cv=");
                for (int j = 0; j < g->bheight_uv; ++j)
                    fprintf(ef, "%s%u", j ? "," : "", (unsigned)b->neigh_left_recon_16bit[2][j]);
            }
            fprintf(ef, "\n");
            fflush(ef);
        }
    }
}

/* ---- chroma FAST-RATE interposer (issue #15, the last 2 unaligned cells) --
 * svt_aom_get_intra_uv_fast_rate (rd_cost.c:476, exported T).
 *
 * `search_best_mds3_uv_mode` (product_coding_loop.c:7452-7501) is `static`, so
 * its per-candidate argmin cannot be wrapped directly. But every input to that
 * argmin IS reachable:
 *   * coeff_rate / distortion per (uv_mode, uv angle delta) -> SVT_UVLOOP_OUT
 *     (the interposer below);
 *   * `cand_bf->fast_chroma_rate` -> THIS wrap, which is the exact call at
 *     :7485, once per (luma intra mode) x (uv candidate) pair, in list order;
 *   * `full_lambda` -> ctx->full_lambda_md[hbd_md] (:7307), dumped here too.
 * Join them and C's `uv_cost = RDCOST(full_lambda, coeff_rate + fast_chroma_
 * rate, distortion)` is reproducible to the bit, which is what comparing the
 * port's `NSQDBG UVTAB2` rows against C requires.
 *
 * The same function is also called from :3739 / :3903 / :3934 / :7000 / :7095 /
 * :7797 and rd_cost.c:621, so the dump carries `acc=` (use_accurate_cfl) and a
 * call ordinal; the :7485 rows are the contiguous run with acc=0 that walks the
 * uv list once per distinct luma mode.
 *
 * Env: SVT_UVRATE_OUT + optional SVT_UVRATE_XY="x,y" (unset / "all" = every
 * block). Pure pass-through when unset — the C tree stays PRISTINE. */
uint64_t __real_svt_aom_get_intra_uv_fast_rate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                               ModeDecisionCandidateBuffer* cand_bf, bool use_accurate_cfl);

uint64_t __wrap_svt_aom_get_intra_uv_fast_rate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                               ModeDecisionCandidateBuffer* cand_bf, bool use_accurate_cfl) {
    const uint64_t r = __real_svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, use_accurate_cfl);
    const char*    path = getenv("SVT_UVRATE_OUT");
    if (path && *path) {
        const char* xy  = getenv("SVT_UVRATE_XY");
        int         px = -1, py = -1;
        const int   all = !xy || !*xy || !strcmp(xy, "all");
        if (!all)
            sscanf(xy, "%d,%d", &px, &py);
        if (all || ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py)) {
            static FILE*    f = NULL;
            static unsigned n = 0;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                fprintf(f,
                        "UVRATE n=%u org=(%u,%u) %ux%u luma=%d uv=%d uvd=%d acc=%d rate=%llu lambda=%llu "
                        "cflsigns=%d cflidx=%d indavail=%d hasuv=%d uvmode=%d indlast=%d skipdc=%d ivith=%d "
                        "mds3n=%u ibc=%d\n",
                        n++, (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y,
                        block_size_wide[ctx->blk_geom->bsize], block_size_high[ctx->blk_geom->bsize],
                        (int)cand_bf->cand->block_mi.mode, (int)cand_bf->cand->block_mi.uv_mode,
                        (int)cand_bf->cand->block_mi.angle_delta[1], (int)use_accurate_cfl,
                        (unsigned long long)r,
                        (unsigned long long)ctx->full_lambda_md[ctx->hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD],
                        (int)cand_bf->cand->block_mi.cfl_alpha_signs, (int)cand_bf->cand->block_mi.cfl_alpha_idx,
                        (int)ctx->ind_uv_avail, (int)ctx->has_uv, (int)ctx->uv_ctrls.uv_mode,
                        (int)ctx->uv_ctrls.ind_uv_last_mds, (int)ctx->uv_ctrls.skip_ind_uv_if_only_dc,
                        (int)ctx->uv_ctrls.inter_vs_intra_cost_th, (unsigned)ctx->md_stage_3_total_count,
                        (int)cand_bf->cand->block_mi.use_intrabc);
                fflush(f);
            }
        }
    }
    return r;
}

/* ---- chroma full-loop interposer ----------------------------------------
 * svt_aom_full_loop_uv (full_loop.c:2024, exported T; cross-TU callers in
 * product_coding_loop.c incl. the mds3 independent-uv search's per-uv
 * evaluations). Logging (cand uv/uvd + accumulated cb/cr bits+dist) at a
 * pinned block reveals the per-(uv) RD pairs C's uv-table argmin consumes.
 * Env: SVT_UVLOOP_OUT + SVT_UVLOOP_XY="x,y". One line per call.
 * SVT_UVLOOP_XY is OPTIONAL: unset (or "all") dumps EVERY block, which is what
 * localizing a neighbour-recon drift needs (the first divergent block is not
 * known in advance, so a pinned x,y cannot find it).
 *
 * `pu=/pv=` are cand_bf->pred's chroma ORIGIN samples, read BEFORE the real
 * call. cfl_prediction passes blk_chroma_origin_index == 0 (:6938), so index 0
 * IS the block's prediction origin. On the CfL-search calls (av1_cost_calc_cfl,
 * :3411/:3441) cand_bf->pred holds the DC BASE that svt_cfl_predict_* reads —
 * constant across every alpha of a block — so `pu/pv` is a direct, per-block
 * readout of the chroma DC prediction, i.e. of the chroma recon NEIGHBOUR state
 * feeding it. That is the one number needed to bisect a chroma recon drift from
 * outside the (static, un-wrappable) cfl_prediction family. */
void __real_svt_aom_full_loop_uv(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                 ModeDecisionCandidateBuffer* cand_bf, EbPictureBufferDesc* input_pic,
                                 COMPONENT_TYPE component_type, uint32_t chroma_qindex,
                                 uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                                 uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* cb_coeff_bits,
                                 uint64_t* cr_coeff_bits, bool is_full_loop);

void __wrap_svt_aom_full_loop_uv(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                 ModeDecisionCandidateBuffer* cand_bf, EbPictureBufferDesc* input_pic,
                                 COMPONENT_TYPE component_type, uint32_t chroma_qindex,
                                 uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                                 uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* cb_coeff_bits,
                                 uint64_t* cr_coeff_bits, bool is_full_loop) {
    /* Prediction origin samples, captured BEFORE the real call so nothing the
     * full loop does can perturb them. */
    unsigned pu = 0, pv = 0;
    {
        const char* p0 = getenv("SVT_UVLOOP_OUT");
        if (p0 && *p0 && cand_bf->pred) {
            if (ctx->hbd_md) {
                pu = ((const uint16_t*)cand_bf->pred->u_buffer)[0];
                pv = ((const uint16_t*)cand_bf->pred->v_buffer)[0];
            } else {
                pu = cand_bf->pred->u_buffer[0];
                pv = cand_bf->pred->v_buffer[0];
            }
        }
    }
    __real_svt_aom_full_loop_uv(pcs, ctx, cand_bf, input_pic, component_type, chroma_qindex, cb_full_distortion,
                                cr_full_distortion, cb_coeff_bits, cr_coeff_bits, is_full_loop);
    const char* path = getenv("SVT_UVLOOP_OUT");
    const char* xy   = getenv("SVT_UVLOOP_XY");
    if (path && *path) {
        int px = -1, py = -1;
        /* xy unset / "all" => every block. */
        const int all = !xy || !*xy || !strcmp(xy, "all");
        if (!all)
            sscanf(xy, "%d,%d", &px, &py);
        if (all || ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py)) {
            static FILE* f = NULL;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                fprintf(f,
                        "UVLOOP org=(%u,%u) %ux%u mode=%d uv=%d uvd=%d full=%d cbb=%llu crb=%llu cbd=%llu crd=%llu "
                        "pu=%u pv=%u\n",
                        (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, block_size_wide[ctx->blk_geom->bsize],
                        block_size_high[ctx->blk_geom->bsize], (int)cand_bf->cand->block_mi.mode,
                        (int)cand_bf->cand->block_mi.uv_mode, (int)cand_bf->cand->block_mi.angle_delta[1],
                        (int)is_full_loop, (unsigned long long)*cb_coeff_bits, (unsigned long long)*cr_coeff_bits,
                        (unsigned long long)cb_full_distortion[0][0], (unsigned long long)cr_full_distortion[0][0], pu,
                        pv);
                fflush(f);
            }
        }
    }
}

/* ---- PD0 full-cost interposer (task #95 partial-SB PD0 near-tie) ----------
 * svt_aom_full_cost_pd0 (rd_cost.c:1330) computes the LPD0 per-block RD used by
 * the partition pick (test_split_partition_pd0). The port models it in
 * pd0::lvl1_block_cost_rect; a straddling bottom-edge 16x16 node's edge-shape
 * (16x8) vs SPLIT (2x8x8) RD near-tie flips the partition on some cells. This
 * dumps C's (org, bsize, dist, coeff_bits, full_cost) per tested PD0 block so
 * the port's NSQDBG PD0 costs can be compared unit-for-unit. Env: SVT_PD0COST_
 * OUT (file) + optional SVT_PD0COST_SBY (only blocks whose SB row == that y).
 * Pure pass-through when unset — the C tree stays PRISTINE (link interposer). */
EbErrorType __real_svt_aom_full_cost_pd0(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                         uint64_t* y_distortion, uint64_t lambda, uint64_t* y_coeff_bits);

EbErrorType __wrap_svt_aom_full_cost_pd0(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                         uint64_t* y_distortion, uint64_t lambda, uint64_t* y_coeff_bits) {
    EbErrorType ret = __real_svt_aom_full_cost_pd0(ctx, cand_bf, y_distortion, lambda, y_coeff_bits);
    const char* path = getenv("SVT_PD0COST_OUT");
    if (path && *path) {
        const char* sby = getenv("SVT_PD0COST_SBY");
        const int   sb_y_filter = sby ? atoi(sby) : -1;
        const int   org_y = (int)ctx->blk_org_y;
        if (sb_y_filter < 0 || (org_y & ~63) == sb_y_filter) {
            static FILE* f = NULL;
            if (!f)
                f = fopen(path, "w");
            if (f) {
                fprintf(f, "PD0COST org=(%u,%u) %ux%u dist=%llu ybits=%llu cost=%llu lambda=%llu\n",
                        (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, block_size_wide[ctx->blk_geom->bsize],
                        block_size_high[ctx->blk_geom->bsize], (unsigned long long)y_distortion[0],
                        (unsigned long long)*y_coeff_bits, (unsigned long long)*(cand_bf->full_cost),
                        (unsigned long long)lambda);
                fflush(f);
            }
        }
    }
    return ret;
}

/* ---- PD0 CONFIG interposer (video-arm PD0 localization, 2026-09-01) -------
 * `svt_aom_sig_deriv_enc_dec_pd0` (enc_mode_config.c:7207) resolves, per SB and
 * per PD0 pass, everything the PD0 partition search then runs with: the level
 * the detector left in `pd0_ctrls`, the subres step, the depth early-exit
 * thresholds, the rate-estimation level, and `pd0_use_src_samples`. §1h of
 * `docs/INTER-ENCODE-PLAN.md` measured four GUESSES at that configuration on
 * the video arm and none were good; this dumps what C actually resolved so the
 * port's PD0 can be compared field for field instead.
 *
 * Env: SVT_PD0CFG_OUT (file). Pure pass-through when unset; the C tree stays
 * pristine (link-time interposer, not a source edit).
 *
 * Output, one line per call (append across SBs and frames — cut at the frame
 * boundary yourself, same as SVT_CTREE_OUT):
 *   PD0CFG sb=<idx> org=(x,y) islice=<0|1> lvl=<Pd0Level> subres=<step>
 *          dev_th=<odd_to_even_deviation_th> split_th=<..> exit_th=<..>
 *          rate_lvl=<coeff_rate_est_lvl> qpoff=<lpd0_qp_offset>
 *          fastcoef=<pd0_fast_coeff_est_level> srcsamp=<0|1>
 *          pred_only=<pic_pred_depth_only> d4=<disallow_4x4> d8=<disallow_8x8>
 *          maxbs=<max_block_size> cb64=<is_complete_b64> bias=<parent_cost_bias>
 *          intra=<enable_intra>/<intra_mode_end>/<angular_pred_level>
 *          nsq=<md_disallow_nsq_search> subsafe=<is_subres_safe>
 *          dr=<depth_removal enabled>/<disallow_below_64x64>/<..32x32>/<..16x16>
 *          drlvl=<pcs->pic_depth_removal_level> fastlam=<fast_lambda_md[8bit]>
 *          pqp=<ppcs->picture_qp>
 *          med=<me_64x64_dist>/<me_32x32>/<me_16x16>/<me_8x8>
 *          mev=<me_8x8_cost_variance> refmin=<ref l0 sb_min_sq_size, 255 = none>
 *
 * The trailing block is exactly `set_depth_removal_level_controls`' input set
 * (enc_mode_config.c:2965), so the PORT's derivation can be joined field for
 * field instead of being fitted to the three output flags.
 */
void __real_svt_aom_sig_deriv_enc_dec_pd0(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx);

void __wrap_svt_aom_sig_deriv_enc_dec_pd0(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    __real_svt_aom_sig_deriv_enc_dec_pd0(scs, pcs, ctx);
    const char* path = getenv("SVT_PD0CFG_OUT");
    if (!path || !*path) {
        return;
    }
    static FILE* f = NULL;
    if (!f) {
        f = fopen(path, "a");
    }
    if (!f) {
        return;
    }
    const B64Geom* b64 = &pcs->ppcs->b64_geom[ctx->sb_index];
    fprintf(f,
            "PD0CFG sb=%u org=(%u,%u) islice=%d lvl=%d subres=%u dev_th=%u split_th=%u exit_th=%u "
            "rate_lvl=%u qpoff=%d fastcoef=%u srcsamp=%d pred_only=%d d4=%d d8=%d maxbs=%u cb64=%d "
            "bias=%u intra=%u/%u/%u nsq=%d subsafe=%u dr=%d/%d/%d/%d "
            "drlvl=%u fastlam=%u pqp=%u med=%u/%u/%u/%u mev=%u refmin=%u\n",
            (unsigned)ctx->sb_index,
            (unsigned)ctx->sb_origin_x,
            (unsigned)ctx->sb_origin_y,
            (int)(pcs->slice_type == I_SLICE),
            (int)ctx->pd0_ctrls.pd0_level,
            (unsigned)ctx->subres_ctrls.step,
            (unsigned)ctx->subres_ctrls.odd_to_even_deviation_th,
            (unsigned)ctx->depth_early_exit_ctrls.split_cost_th,
            (unsigned)ctx->depth_early_exit_ctrls.early_exit_th,
            (unsigned)ctx->rate_est_ctrls.coeff_rate_est_lvl,
            (int)ctx->rate_est_ctrls.lpd0_qp_offset,
            (unsigned)ctx->rate_est_ctrls.pd0_fast_coeff_est_level,
            (int)ctx->pd0_use_src_samples,
            (int)ctx->pic_pred_depth_only,
            (int)ctx->disallow_4x4,
            (int)ctx->disallow_8x8,
            (unsigned)ctx->max_block_size,
            (int)b64->is_complete_b64,
            (unsigned)ctx->parent_cost_bias,
            (unsigned)ctx->intra_ctrls.enable_intra,
            (unsigned)ctx->intra_ctrls.intra_mode_end,
            (unsigned)ctx->intra_ctrls.angular_pred_level,
            (int)ctx->md_disallow_nsq_search,
            (unsigned)ctx->is_subres_safe,
            (int)ctx->depth_removal_ctrls.enabled,
            (int)ctx->depth_removal_ctrls.disallow_below_64x64,
            (int)ctx->depth_removal_ctrls.disallow_below_32x32,
            (int)ctx->depth_removal_ctrls.disallow_below_16x16,
            (unsigned)pcs->pic_depth_removal_level,
            (unsigned)ctx->fast_lambda_md[EB_8_BIT_MD],
            (unsigned)pcs->ppcs->picture_qp,
            (unsigned)pcs->ppcs->me_64x64_distortion[ctx->sb_index],
            (unsigned)pcs->ppcs->me_32x32_distortion[ctx->sb_index],
            (unsigned)pcs->ppcs->me_16x16_distortion[ctx->sb_index],
            (unsigned)pcs->ppcs->me_8x8_distortion[ctx->sb_index],
            (unsigned)pcs->ppcs->me_8x8_cost_variance[ctx->sb_index],
            (unsigned)(pcs->slice_type != I_SLICE && pcs->ref_pic_ptr_array[0][0]
                           ? ((EbReferenceObject*)pcs->ref_pic_ptr_array[0][0]->object_ptr)
                                 ->sb_min_sq_size[ctx->sb_index]
                           : 255));
    fflush(f);
}

/* ---------------------------------------------------------------------------
 * svt_av1_reset_cdf_symbol_counters — the END-OF-FRAME CDF STATE ORACLE.
 *
 * WHY THIS ONE FUNCTION. `packetization_process.c:741-744` is the only place
 * C saves a frame context for a LATER frame to start from:
 *
 *     svt_av1_reset_cdf_symbol_counters(pcs->ec_info[tile_idx]->ec->fc);
 *     ((EbReferenceObject*)...->object_ptr)->frame_context = *(...->ec->fc);
 *
 * so wrapping it and dumping the fc AFTER the real call gives EXACTLY the
 * bytes that land in the reference object — the state the next frame's
 * `reset_entropy_coding_picture` (ec_process.c:101-112) and
 * `init_frame_rate_tables` (md_config_process.c:299-310) copy back when
 * `primary_ref_frame != PRIMARY_REF_NONE`.
 *
 * That makes this the oracle for CDF CONTINUATION: the port's own saved state
 * can be compared field-for-field against it without decoding a bitstream, and
 * without needing the inter tile walk to work first.
 *
 * The counters are part of the answer, not noise: `update_cdf`'s adaptation
 * RATE reads `cdf[nsymbs]`, so a save that skipped the reset would make the
 * next frame adapt at the wrong speed from its first symbol.
 *
 * Output (SVT_FCTX_OUT, appended; one line per CDF array):
 *     FCTX <call#> <field> <count> <v0> <v1> ...
 * `<call#>` counts calls, so a 2-frame encode where only frame 0 is a
 * reference has a single block numbered 0. The port's twin dump
 * (SVTAV1_FCTX_OUT) uses the same field names and the same flat order.
 * ------------------------------------------------------------------------- */
void __real_svt_av1_reset_cdf_symbol_counters(FRAME_CONTEXT *fc);

#define FCTX_DUMP(f, n, name, arr)                                            \
    do {                                                                      \
        const AomCdfProb *_p = (const AomCdfProb *)(arr);                     \
        size_t            _n = sizeof(arr) / sizeof(AomCdfProb);              \
        fprintf((f), "FCTX %u %s %zu", (unsigned)(n), (name), _n);            \
        for (size_t _i = 0; _i < _n; _i++) fprintf((f), " %u", (unsigned)_p[_i]); \
        fputc('\n', (f));                                                     \
    } while (0)

static void fctx_dump_nmv(FILE *f, unsigned n, const char *prefix, const NmvContext *nmv) {
    char nm[96];
    snprintf(nm, sizeof(nm), "%s.joints", prefix);
    FCTX_DUMP(f, n, nm, nmv->joints_cdf);
    for (int i = 0; i < 2; i++) {
        snprintf(nm, sizeof(nm), "%s.comp%d.classes", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].classes_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.class0_fp", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].class0_fp_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.fp", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].fp_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.sign", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].sign_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.class0_hp", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].class0_hp_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.hp", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].hp_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.class0", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].class0_cdf);
        snprintf(nm, sizeof(nm), "%s.comp%d.bits", prefix, i);
        FCTX_DUMP(f, n, nm, nmv->comps[i].bits_cdf);
    }
}

void __wrap_svt_av1_reset_cdf_symbol_counters(FRAME_CONTEXT *fc) {
    __real_svt_av1_reset_cdf_symbol_counters(fc);
    const char *path = getenv("SVT_FCTX_OUT");
    if (!path || !*path)
        return;
    static unsigned call_no = 0;
    FILE          *f        = fopen(path, "a");
    if (!f)
        return;
    unsigned n = call_no++;
    FCTX_DUMP(f, n, "txb_skip", fc->txb_skip_cdf);
    FCTX_DUMP(f, n, "eob_extra", fc->eob_extra_cdf);
    FCTX_DUMP(f, n, "dc_sign", fc->dc_sign_cdf);
    FCTX_DUMP(f, n, "eob_flag16", fc->eob_flag_cdf16);
    FCTX_DUMP(f, n, "eob_flag32", fc->eob_flag_cdf32);
    FCTX_DUMP(f, n, "eob_flag64", fc->eob_flag_cdf64);
    FCTX_DUMP(f, n, "eob_flag128", fc->eob_flag_cdf128);
    FCTX_DUMP(f, n, "eob_flag256", fc->eob_flag_cdf256);
    FCTX_DUMP(f, n, "eob_flag512", fc->eob_flag_cdf512);
    FCTX_DUMP(f, n, "eob_flag1024", fc->eob_flag_cdf1024);
    FCTX_DUMP(f, n, "coeff_base_eob", fc->coeff_base_eob_cdf);
    FCTX_DUMP(f, n, "coeff_base", fc->coeff_base_cdf);
    FCTX_DUMP(f, n, "coeff_br", fc->coeff_br_cdf);
    FCTX_DUMP(f, n, "newmv", fc->newmv_cdf);
    FCTX_DUMP(f, n, "zeromv", fc->zeromv_cdf);
    FCTX_DUMP(f, n, "refmv", fc->refmv_cdf);
    FCTX_DUMP(f, n, "drl", fc->drl_cdf);
    FCTX_DUMP(f, n, "inter_compound_mode", fc->inter_compound_mode_cdf);
    FCTX_DUMP(f, n, "compound_type", fc->compound_type_cdf);
    FCTX_DUMP(f, n, "wedge_idx", fc->wedge_idx_cdf);
    FCTX_DUMP(f, n, "interintra", fc->interintra_cdf);
    FCTX_DUMP(f, n, "wedge_interintra", fc->wedge_interintra_cdf);
    FCTX_DUMP(f, n, "interintra_mode", fc->interintra_mode_cdf);
    FCTX_DUMP(f, n, "motion_mode", fc->motion_mode_cdf);
    FCTX_DUMP(f, n, "obmc", fc->obmc_cdf);
    FCTX_DUMP(f, n, "palette_y_size", fc->palette_y_size_cdf);
    FCTX_DUMP(f, n, "palette_uv_size", fc->palette_uv_size_cdf);
    FCTX_DUMP(f, n, "palette_y_color_index", fc->palette_y_color_index_cdf);
    FCTX_DUMP(f, n, "palette_uv_color_index", fc->palette_uv_color_index_cdf);
    FCTX_DUMP(f, n, "palette_y_mode", fc->palette_y_mode_cdf);
    FCTX_DUMP(f, n, "palette_uv_mode", fc->palette_uv_mode_cdf);
    FCTX_DUMP(f, n, "comp_inter", fc->comp_inter_cdf);
    FCTX_DUMP(f, n, "single_ref", fc->single_ref_cdf);
    FCTX_DUMP(f, n, "comp_ref_type", fc->comp_ref_type_cdf);
    FCTX_DUMP(f, n, "uni_comp_ref", fc->uni_comp_ref_cdf);
    FCTX_DUMP(f, n, "comp_ref", fc->comp_ref_cdf);
    FCTX_DUMP(f, n, "comp_bwdref", fc->comp_bwdref_cdf);
    FCTX_DUMP(f, n, "txfm_partition", fc->txfm_partition_cdf);
    FCTX_DUMP(f, n, "compound_index", fc->compound_index_cdf);
    FCTX_DUMP(f, n, "comp_group_idx", fc->comp_group_idx_cdf);
    FCTX_DUMP(f, n, "skip_mode", fc->skip_mode_cdfs);
    FCTX_DUMP(f, n, "skip", fc->skip_cdfs);
    FCTX_DUMP(f, n, "intra_inter", fc->intra_inter_cdf);
    fctx_dump_nmv(f, n, "nmvc", &fc->nmvc);
    fctx_dump_nmv(f, n, "ndvc", &fc->ndvc);
    FCTX_DUMP(f, n, "intrabc", fc->intrabc_cdf);
    FCTX_DUMP(f, n, "seg.tree", fc->seg.tree_cdf);
    FCTX_DUMP(f, n, "seg.pred", fc->seg.pred_cdf);
    FCTX_DUMP(f, n, "seg.spatial_pred", fc->seg.spatial_pred_seg_cdf);
    FCTX_DUMP(f, n, "filter_intra", fc->filter_intra_cdfs);
    FCTX_DUMP(f, n, "filter_intra_mode", fc->filter_intra_mode_cdf);
    FCTX_DUMP(f, n, "switchable_restore", fc->switchable_restore_cdf);
    FCTX_DUMP(f, n, "wiener_restore", fc->wiener_restore_cdf);
    FCTX_DUMP(f, n, "sgrproj_restore", fc->sgrproj_restore_cdf);
    FCTX_DUMP(f, n, "y_mode", fc->y_mode_cdf);
    FCTX_DUMP(f, n, "uv_mode", fc->uv_mode_cdf);
    FCTX_DUMP(f, n, "partition", fc->partition_cdf);
    FCTX_DUMP(f, n, "switchable_interp", fc->switchable_interp_cdf);
    FCTX_DUMP(f, n, "kf_y", fc->kf_y_cdf);
    FCTX_DUMP(f, n, "angle_delta", fc->angle_delta_cdf);
    FCTX_DUMP(f, n, "tx_size", fc->tx_size_cdf);
    FCTX_DUMP(f, n, "delta_q", fc->delta_q_cdf);
    FCTX_DUMP(f, n, "delta_lf_multi", fc->delta_lf_multi_cdf);
    FCTX_DUMP(f, n, "delta_lf", fc->delta_lf_cdf);
    FCTX_DUMP(f, n, "intra_ext_tx", fc->intra_ext_tx_cdf);
    FCTX_DUMP(f, n, "inter_ext_tx", fc->inter_ext_tx_cdf);
    FCTX_DUMP(f, n, "cfl_sign", fc->cfl_sign_cdf);
    FCTX_DUMP(f, n, "cfl_alpha", fc->cfl_alpha_cdf);
    fflush(f);
    fclose(f);
}


/* ---------------------------------------------------------------------------
 * SVT_HME_OUT — C's per-b64 HME pyramid state, straight out of `MeContext`.
 *
 * WHY: `docs/INTER-ENCODE-PLAN.md` §1z12 localized the port's open-loop ME
 * defect to HME LEVEL 0 on the SECOND superblock column, but nothing had ever
 * measured C's own level-0 search centres — `hme_level_0` and `hme_level0_b64`
 * are both `static`, so the only exported vantage point is the per-b64 entry
 * `svt_aom_motion_estimation_b64`, which returns with the whole pyramid still
 * live in `me_ctx`.
 *
 * Env: SVT_HME_OUT (file). Pure pass-through when unset.
 *
 * One line per b64 per call (appends across frames — cut at the frame boundary
 * yourself, same as SVT_CTREE_OUT):
 *   HME b64=<idx> org=(x,y) bw=<b64_width> bh=<b64_height> meth=<hme_search_method>
 *       nw=<num_hme_sa_w> nh=<num_hme_sa_h>
 *       l0sa=<min_w>x<min_h>/<max_w>x<max_h>
 *       q[<sr_w>][<sr_h>] l0=(x,y):<sad> l1=(x,y):<sad> l2=(x,y):<sad>  (one group per quadrant)
 *       fin=(hme_sc_x,hme_sc_y):<hme_sad> doref=<0|1>
 *       src16=<16 bytes of row 0 of sixteenth_b64_buffer, hex>
 *       src16sum=<sum of the block's block_width*block_height sixteenth samples>
 *
 * The `src16` fields exist because the port's sixteenth pyramid is built by its
 * OWN two-stage decimation; a level-0 disagreement is only a SEARCH defect if
 * the two sides are searching the same planes, and this is the cheapest way to
 * prove that rather than assume it.
 * ------------------------------------------------------------------------- */
EbErrorType __real_svt_aom_motion_estimation_b64(PictureParentControlSet* pcs, uint32_t b64_index,
                                                 uint32_t b64_origin_x, uint32_t b64_origin_y, MeContext* me_ctx,
                                                 EbPictureBufferDesc* input_ptr);

EbErrorType __wrap_svt_aom_motion_estimation_b64(PictureParentControlSet* pcs, uint32_t b64_index,
                                                 uint32_t b64_origin_x, uint32_t b64_origin_y, MeContext* me_ctx,
                                                 EbPictureBufferDesc* input_ptr) {
    EbErrorType rc = __real_svt_aom_motion_estimation_b64(pcs, b64_index, b64_origin_x, b64_origin_y, me_ctx,
                                                          input_ptr);
    const char* path = getenv("SVT_HME_OUT");
    if (!path || !*path) {
        return rc;
    }
    static FILE* f = NULL;
    if (!f) {
        f = fopen(path, "a");
    }
    if (!f) {
        return rc;
    }
    fprintf(f,
            "HME b64=%u org=(%u,%u) bw=%u bh=%u meth=%u nw=%u nh=%u l0sa=%ux%u/%ux%u",
            (unsigned)b64_index,
            (unsigned)b64_origin_x,
            (unsigned)b64_origin_y,
            (unsigned)me_ctx->b64_width,
            (unsigned)me_ctx->b64_height,
            (unsigned)me_ctx->hme_search_method,
            (unsigned)me_ctx->num_hme_sa_w,
            (unsigned)me_ctx->num_hme_sa_h,
            (unsigned)me_ctx->hme_l0_sa.sa_min.width,
            (unsigned)me_ctx->hme_l0_sa.sa_min.height,
            (unsigned)me_ctx->hme_l0_sa.sa_max.width,
            (unsigned)me_ctx->hme_l0_sa.sa_max.height);
    for (uint32_t h = 0; h < me_ctx->num_hme_sa_h; h++) {
        for (uint32_t w = 0; w < me_ctx->num_hme_sa_w; w++) {
            fprintf(f,
                    " q[%u][%u] l0=(%d,%d):%llu l1=(%d,%d):%llu l2=(%d,%d):%llu",
                    (unsigned)w,
                    (unsigned)h,
                    (int)me_ctx->x_hme_level0_search_center[0][0][w][h],
                    (int)me_ctx->y_hme_level0_search_center[0][0][w][h],
                    (unsigned long long)me_ctx->hme_level0_sad[0][0][w][h],
                    (int)me_ctx->x_hme_level1_search_center[0][0][w][h],
                    (int)me_ctx->y_hme_level1_search_center[0][0][w][h],
                    (unsigned long long)me_ctx->hme_level1_sad[0][0][w][h],
                    (int)me_ctx->x_hme_level2_search_center[0][0][w][h],
                    (int)me_ctx->y_hme_level2_search_center[0][0][w][h],
                    (unsigned long long)me_ctx->hme_level2_sad[0][0][w][h]);
        }
    }
    fprintf(f,
            " fin=(%d,%d):%llu doref=%u",
            (int)me_ctx->search_results[0][0].hme_sc_x,
            (int)me_ctx->search_results[0][0].hme_sc_y,
            (unsigned long long)me_ctx->search_results[0][0].hme_sad,
            (unsigned)me_ctx->search_results[0][0].do_ref);
    if (me_ctx->sixteenth_b64_buffer) {
        const uint32_t bw   = me_ctx->b64_width >> 2;
        const uint32_t bh   = me_ctx->b64_height >> 2;
        const uint32_t strd = me_ctx->sixteenth_b64_buffer_stride;
        fprintf(f, " src16=");
        for (uint32_t i = 0; i < bw; i++) {
            fprintf(f, "%02x", (unsigned)me_ctx->sixteenth_b64_buffer[i]);
        }
        unsigned long long sum = 0;
        for (uint32_t r = 0; r < bh; r++) {
            for (uint32_t c = 0; c < bw; c++) {
                sum += me_ctx->sixteenth_b64_buffer[r * strd + c];
            }
        }
        fprintf(f, " src16sum=%llu src16stride=%u", sum, (unsigned)strd);
    }
    fprintf(f, "\n");
    /* The whole ME signal set `svt_aom_sig_deriv_me` resolved, so the port's
     * own derivation can be JOINED field for field instead of guessed at. */
    fprintf(f,
            "MESIG b64=%u hme=%u/%u/%u/%u hmeth=%u meth=%u"
            " mesa=%ux%u/%ux%u l1sa=%ux%u l2sa=%ux%u"
            " prehme=%u/%u/%u sa0=%ux%u/%ux%u sa1=%ux%u/%ux%u"
            " earlyexit=%u staticth=%u prevexit=%u redl0=%u/%u"
            " mesr=%u/%u/%u/%u/%u/%u/%u me8x8var=%u/%u/%u/%u"
            " mvadj=%u/%u/%u/%u prune=%u/%u/%u/%u/%u/%u/%u"
            " tli=%u isref=%u nlist=%u nref=%u/%u prunecand=%d ubuc=%u scboost=%u\n",
            (unsigned)b64_index,
            (unsigned)me_ctx->enable_hme_flag,
            (unsigned)me_ctx->enable_hme_level0_flag,
            (unsigned)me_ctx->enable_hme_level1_flag,
            (unsigned)me_ctx->enable_hme_level2_flag,
            (unsigned)me_ctx->hme_search_method,
            (unsigned)me_ctx->me_search_method,
            (unsigned)me_ctx->me_sa.sa_min.width,
            (unsigned)me_ctx->me_sa.sa_min.height,
            (unsigned)me_ctx->me_sa.sa_max.width,
            (unsigned)me_ctx->me_sa.sa_max.height,
            (unsigned)me_ctx->hme_l1_sa.width,
            (unsigned)me_ctx->hme_l1_sa.height,
            (unsigned)me_ctx->hme_l2_sa.width,
            (unsigned)me_ctx->hme_l2_sa.height,
            (unsigned)me_ctx->prehme_ctrl.enable,
            (unsigned)me_ctx->prehme_ctrl.skip_search_line,
            (unsigned)me_ctx->prehme_ctrl.l1_early_exit,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[0].sa_min.width,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[0].sa_min.height,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[0].sa_max.width,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[0].sa_max.height,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[1].sa_min.width,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[1].sa_min.height,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[1].sa_max.width,
            (unsigned)me_ctx->prehme_ctrl.prehme_sa_cfg[1].sa_max.height,
            (unsigned)me_ctx->me_early_exit_th,
            (unsigned)me_ctx->me_static_b64_th,
            (unsigned)me_ctx->prev_me_stage_based_exit_th,
            (unsigned)me_ctx->reduce_hme_l0_sr_th_min,
            (unsigned)me_ctx->reduce_hme_l0_sr_th_max,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.enable_me_sr_adjustment,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.reduce_me_sr_based_on_mv_length_th,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.stationary_hme_sad_abs_th,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.stationary_me_sr_divisor,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.reduce_me_sr_based_on_hme_sad_abs_th,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad,
            (unsigned)me_ctx->me_sr_adjustment_ctrls.distance_based_hme_resizing,
            (unsigned)me_ctx->me_8x8_var_ctrls.enabled,
            (unsigned)me_ctx->me_8x8_var_ctrls.me_sr_div4_th,
            (unsigned)me_ctx->me_8x8_var_ctrls.me_sr_div2_th,
            (unsigned)me_ctx->me_8x8_var_ctrls.me_sr_mult2_th,
            (unsigned)me_ctx->mv_based_sa_adj.enabled,
            (unsigned)me_ctx->mv_based_sa_adj.nearest_ref_only,
            (unsigned)me_ctx->mv_based_sa_adj.mv_size_th,
            (unsigned)me_ctx->mv_based_sa_adj.sa_multiplier,
            (unsigned)me_ctx->me_hme_prune_ctrls.enable_me_hme_ref_pruning,
            (unsigned)me_ctx->me_hme_prune_ctrls.prune_ref_if_hme_sad_dev_bigger_than_th,
            (unsigned)me_ctx->me_hme_prune_ctrls.prune_ref_if_me_sad_dev_bigger_than_th,
            (unsigned)me_ctx->me_hme_prune_ctrls.zz_sad_th,
            (unsigned)me_ctx->me_hme_prune_ctrls.zz_sad_pct,
            (unsigned)me_ctx->me_hme_prune_ctrls.phme_sad_th,
            (unsigned)me_ctx->me_hme_prune_ctrls.phme_sad_pct,
            (unsigned)me_ctx->temporal_layer_index,
            (unsigned)me_ctx->is_ref,
            (unsigned)me_ctx->num_of_list_to_search,
            (unsigned)me_ctx->num_of_ref_pic_to_search[0],
            (unsigned)me_ctx->num_of_ref_pic_to_search[1],
            (int)me_ctx->prune_me_candidates_th,
            (unsigned)me_ctx->use_best_unipred_cand_only,
            (unsigned)me_ctx->sc_class_me_boost);
    fprintf(f,
            "PHME b64=%u p0=(%d,%d):%llu v=%u sa=%ux%u p1=(%d,%d):%llu v=%u sa=%ux%u"
            " done=%u/%u zz=%u\n",
            (unsigned)b64_index,
            (int)me_ctx->prehme_data[0][0][0].best_mv.x,
            (int)me_ctx->prehme_data[0][0][0].best_mv.y,
            (unsigned long long)me_ctx->prehme_data[0][0][0].sad,
            (unsigned)me_ctx->prehme_data[0][0][0].valid,
            (unsigned)me_ctx->prehme_data[0][0][0].sa.width,
            (unsigned)me_ctx->prehme_data[0][0][0].sa.height,
            (int)me_ctx->prehme_data[0][0][1].best_mv.x,
            (int)me_ctx->prehme_data[0][0][1].best_mv.y,
            (unsigned long long)me_ctx->prehme_data[0][0][1].sad,
            (unsigned)me_ctx->prehme_data[0][0][1].valid,
            (unsigned)me_ctx->prehme_data[0][0][1].sa.width,
            (unsigned)me_ctx->prehme_data[0][0][1].sa.height,
            (unsigned)me_ctx->performed_phme[0][0][0],
            (unsigned)me_ctx->performed_phme[0][0][1],
            (unsigned)me_ctx->zz_sad[0][0]);
    {
        const uint32_t mv64 = me_ctx->p_sb_best_mv[0][0][0];
        fprintf(f,
                "MERES b64=%u d64=%u d32=%u/%u/%u/%u bestsad64=%u bestmv64=(%d,%d)"
                " redivisor=%u mecand=%u\n",
                (unsigned)b64_index,
                (unsigned)me_ctx->me_distortion[0],
                (unsigned)me_ctx->me_distortion[1],
                (unsigned)me_ctx->me_distortion[2],
                (unsigned)me_ctx->me_distortion[3],
                (unsigned)me_ctx->me_distortion[4],
                (unsigned)me_ctx->p_sb_best_sad[0][0][0],
                (int)(int16_t)(mv64 & 0xFFFF),
                (int)(int16_t)(mv64 >> 16),
                (unsigned)me_ctx->reduce_me_sr_divisor[0][0],
                (unsigned)pcs->pa_me_data->me_results[b64_index]->total_me_candidate_index[0]);
        const uint32_t mv64l1 = me_ctx->p_sb_best_mv[1][0][0];
        fprintf(f,
                "MEL1 b64=%u l1sad64=%u l1mv64=(%d,%d) l1doref=%u l1hme=(%d,%d):%llu"
                " l1ref=%p l0ref=%p l1div=%u mv0=(%d,%d) mvl1=(%d,%d)\n",
                (unsigned)b64_index,
                (unsigned)me_ctx->p_sb_best_sad[1][0][0],
                (int)(int16_t)(mv64l1 & 0xFFFF),
                (int)(int16_t)(mv64l1 >> 16),
                (unsigned)me_ctx->search_results[1][0].do_ref,
                (int)me_ctx->search_results[1][0].hme_sc_x,
                (int)me_ctx->search_results[1][0].hme_sc_y,
                (unsigned long long)me_ctx->search_results[1][0].hme_sad,
                (void*)me_ctx->me_ds_ref_array[1][0].picture_ptr,
                (void*)me_ctx->me_ds_ref_array[0][0].picture_ptr,
                (unsigned)me_ctx->reduce_me_sr_divisor[1][0],
                (int)pcs->pa_me_data->me_results[b64_index]
                    ->me_mv_array[0 * pcs->pa_me_data->max_refs + 0]
                    .x,
                (int)pcs->pa_me_data->me_results[b64_index]
                    ->me_mv_array[0 * pcs->pa_me_data->max_refs + 0]
                    .y,
                (int)pcs->pa_me_data->me_results[b64_index]
                    ->me_mv_array[0 * pcs->pa_me_data->max_refs + pcs->pa_me_data->max_l0]
                    .x,
                (int)pcs->pa_me_data->me_results[b64_index]
                    ->me_mv_array[0 * pcs->pa_me_data->max_refs + pcs->pa_me_data->max_l0]
                    .y);
    }
    fflush(f);
    return rc;
}
