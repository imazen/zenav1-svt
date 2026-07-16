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

#include "common_utils.h"
#include "deblocking_filter.h"
#include "enc_inter_prediction.h"
#include "pcs.h"

#include "coding_loop.h"
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

static void dump_pc_tree(FILE* f, const PC_TREE* t) {
    if (!t)
        return;
    fprintf(f, "PICKPART mi=(%d,%d) bsize=%d partition=%d rd=%lld valid=%d\n", t->mi_row, t->mi_col, (int)t->bsize,
            (int)t->partition, (long long)t->rdc.rd_cost, (int)t->rdc.valid);
    if (t->partition == PARTITION_SPLIT) {
        for (int i = 0; i < 4; ++i)
            dump_pc_tree(f, t->split[i]);
    }
}

bool __wrap_svt_aom_pick_partition(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                   MdScan* mds, PC_TREE* pc_tree, int mi_row, int mi_col) {
    bool r = __real_svt_aom_pick_partition(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);
    const char* path = getenv("SVT_PICKPART_OUT");
    /* First SB only (top-level root call is at mi=(0,0)). */
    if (path && *path && mi_row == 0 && mi_col == 0) {
        static FILE* f = NULL;
        if (!f)
            f = fopen(path, "w");
        if (f) {
            dump_pc_tree(f, pc_tree);
            fflush(f);
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

void __wrap_svt_av1_loop_filter_init(PictureControlSet* pcs) {
    __real_svt_av1_loop_filter_init(pcs);

    const char* path = getenv("SVT_RECON_OUT");
    if (!path || !*path)
        return;
    FILE* f = fopen(path, "a");
    if (!f)
        return;

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
            for (int p = 0; p < 3; ++p) {
                const uint32_t pw = p ? (pcs->ppcs->aligned_width >> ss_x) : pcs->ppcs->aligned_width;
                const uint32_t ph = p ? (pcs->ppcs->aligned_height >> ss_y) : pcs->ppcs->aligned_height;
                char           path[4096];
                snprintf(path, sizeof(path), "%s.p%d", binpath, p);
                FILE* bf = fopen(path, "wb");
                if (!bf)
                    continue;
                for (uint32_t r = 0; r < ph; ++r)
                    fwrite(recon->buffer[p] + (size_t)r * recon->stride[p], 1, pw, bf);
                fclose(bf);
                fprintf(f, "RECON_BIN plane=%d w=%u h=%u -> %s\n", p, pw, ph, path);
            }
        }
    }
    fflush(f);
    fclose(f);
}
