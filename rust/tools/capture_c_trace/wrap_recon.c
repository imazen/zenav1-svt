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

void __real_svt_av1_loop_filter_init(PictureControlSet* pcs);

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
    if (cpath && *cpath && cxy && allow_update_cdf) {
        int px = -1, py = -1;
        sscanf(cxy, "%d,%d", &px, &py);
        if ((int)ctx->blk_org_x == px && (int)ctx->blk_org_y == py) {
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
        /* Only the first SB (mi_row,mi_col within the top-left 64x64 = mi<16). */
        if (pf && mi_row < 16 && mi_col < 16)
            fprintf(pf, "PART bsize=%d mi=(%d,%d) part=%d rate=%lld\n", (int)bsize, mi_row, mi_col, (int)p,
                    (long long)ret);
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
                        "CFULL org=(%u,%u) %ux%u st=%d mode=%d fi=%d ang=%d uv=%d ycb=%llu ydist=%llu cost=%llu\n",
                        (unsigned)ctx->blk_org_x, (unsigned)ctx->blk_org_y, block_size_wide[ctx->blk_geom->bsize],
                        block_size_high[ctx->blk_geom->bsize], (int)ctx->md_stage, (int)cand_bf->cand->block_mi.mode,
                        (int)cand_bf->cand->block_mi.filter_intra_mode, (int)cand_bf->cand->block_mi.angle_delta[0],
                        (int)cand_bf->cand->block_mi.uv_mode, (unsigned long long)*y_coeff_bits,
                        (unsigned long long)y_distortion[0][0], (unsigned long long)*(cand_bf->full_cost));
                fflush(f);
            }
        }
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
            "cflsgn=%d\n",
            mi_row, mi_col, (int)bsize, (int)part, (int)m->mode, (int)m->uv_mode, (int)m->filter_intra_mode,
            (int)m->angle_delta[0], (int)m->angle_delta[1], (int)m->tx_depth, (int)b->palette_size[0], (int)m->skip,
            (int)m->cfl_alpha_idx, (int)m->cfl_alpha_signs);
    if (f)
        fflush(f);

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
