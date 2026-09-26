/*
 * Per-oracle C API bridges for the shims — the same mechanism
 * tools/capture_c_trace/wrap_recon.c uses. build.rs passes each flag from the
 * oracle's `driver_defs` column in rust/oracles/oracles.tsv; a live/hybrid
 * build defines none of them, so the default oracle's code paths below are
 * byte-for-byte unchanged.
 *
 * ZEN_ORACLE_MAINLINE_API (mainline-4.2.0): pristine upstream — no svt-av1-hdr
 *   fork API. Missing: the noise_* / color_range_provided
 *   EbSvtAv1EncConfiguration fields, svt_av1_generate_noise_table,
 *   svt_av1_perform_noise_normalization and
 *   svt_spatial_full_distortion_kernel_facade. Their ref_* wrappers compile to
 *   loud stubs via zen_oracle_missing() — the oracle cannot answer them, and
 *   the per-oracle nextest exclude list keeps the tests out of the suite.
 * ZEN_ORACLE_NO_MDS0_DIST_TYPE (mainline-4.2.0): Mds0Ctrls.mds0_dist_type is a
 *   fork field. Wrap_recon.c tests it; no shim reads it today.
 * ZEN_ORACLE_MV_BY_VALUE (ghost-robot): the mcomp API passes Mv BY VALUE —
 *   svt_av1_mv_bit_cost[_light], svt_aom_mv_err_cost[_light],
 *   svt_av1_full_pixel_search's ref_mv, svt_av1_intrabc_hash_search's ref_mv,
 *   svt_av1_set_mv_search_range's mv. ZEN_MV_ARG adapts the call sites.
 * ZEN_ORACLE_DV_CHROMA_SS (ghost-robot): svt_aom_is_dv_valid takes a trailing
 *   `int chroma_ss` (the sub-8x8-chroma tile-edge guard; upstream passes
 *   scs->subsampling_x). The shims pass 1 — the 4:2:0 the port supports and
 *   the value hybrid hardcodes internally.
 * ZEN_ORACLE_REFMVS_NO_HP (ghost-robot): svt_av1_find_best_ref_mvs_from_stack
 *   dropped its leading allow_hp and trailing is_integer args; its body calls
 *   lower_mv_precision(mv, 0) — always high precision, never integer.
 * ZEN_ORACLE_PSY_DIST_RTCD (ghost-robot): svt_psy_distortion[_hbd] is an RTCD
 *   function pointer declared in common_dsp_rtcd.h, not a plain function —
 *   redeclaring it is an error; call the pointer after RTCD init.
 * ZEN_ORACLE_SUBPEL_NO_DIST_SSE (ghost-robot): fractional_mv_step_fp
 *   (svt_av1_find_best_sub_pixel_tree[_pruned]) dropped its distortion/sse1
 *   out-params — the oracle cannot answer them, so the shim emits sentinels
 *   and the caller's per-oracle exclude list keeps those comparisons out.
 * ZEN_ORACLE_FACADE_QM_SCAN (ghost-robot):
 *   svt_aom_picture_full_distortion32_bits_single_facade gained trailing
 *   `const QmVal* qm, const int16_t* scan` args for the qmpsnr path; NULL
 *   selects the plain 32-bit kernel, identical to the hybrid facade.
 * ZEN_ORACLE_CDEF_QP_LEVEL (ghost-robot):
 *   CdefSearchControls.use_qp_strength (bool) became qp_strength_level
 *   (CdefQpStrengthLevel: OFF/UV/YUV).
 * ZEN_ORACLE_REFS_IN_CTX (ghost-robot): setup_ref_mv_list was rewritten
 *   upstream into an all-refs form — (ref_frames, tot_refs, bsize) inputs,
 *   results written into ctx->ref_mv_stack, xd->ref_mv_count and
 *   ctx->inter_mode_ctx; the refmv_count / ref_mv_stack / gm_mv_candidates /
 *   mode_context params are gone. The shims drive it with a single ref and
 *   copy the same outputs back out.
 * ZEN_ORACLE_EC_PARENT (ghost-robot): OdEcEnc gained `end` /
 *   `buffer_parent` and every write path checks `end` / calls
 *   od_ec_ensure_capacity through the parent — a hand-malloc'd buffer with
 *   neither field set drops every byte and enc_done returns NULL. Shims wrap
 *   their owned buffer in an OutputBitstreamUnit (ZEN_ORACLE_REFS-style).
 * ZEN_ORACLE_CTX_SUBSAMP (ghost-robot): ModeDecisionContext carries
 *   `subsampling_x` (f67a0f747, High Profile support). Mainline/hybrid keep
 *   subsampling on `xd` only, so writes to the ctx field compile out.
 * ZEN_ORACLE_DLF_PICK_METHOD (ghost-robot): DlfCtrls gained `pick_method`
 *   (f9100ab22, "low-delay DLF: honor per-level pick_method on tiled frame
 *   path" — LPF_PICK_FROM_FULL_IMAGE on levels 0-4, LPF_PICK_FROM_Q on
 *   5-7). On oracles without the field the shims emit the implicit rule,
 *   `sb_based_dlf ? LPF_PICK_FROM_Q : LPF_PICK_FROM_FULL_IMAGE` — the same
 *   numbers, so the slot compares like-for-like across oracles.
 * ZEN_ORACLE_INIT_SCAN (ghost-robot): eb_av1_scan_orders was made WRITABLE
 *   upstream and is filled at runtime by svt_aom_init_iscan(); hybrid and
 *   mainline keep it const. Without the init call every get_scan_order()
 *   reader dereferences a NULL .scan — the cause of the 12 c_parity SIGSEGVs
 *   seen 2026-09-25. A load-time constructor covers every shim entry point,
 *   including the files that have no init latch of their own.
 * ZEN_ORACLE_VARIANCE_T (mainline-4.2.0, ghost-robot): PPCS `variance`
 *   element type where the in-house `SvtVarType` typedef does not exist —
 *   `uint16_t` on mainline (the same type hybrid's SVT_HDR_MODE=OFF arm uses),
 *   `double` on Ghost Robot.
 */
#ifndef ZEN_ORACLE_H
#define ZEN_ORACLE_H

#ifdef ZEN_ORACLE_MV_BY_VALUE
#define ZEN_MV_ARG(mv) (mv)
#define ZEN_MV_FIELD(mv, f) ((mv).f)
#else
#define ZEN_MV_ARG(mv) (&(mv))
#define ZEN_MV_FIELD(mv, f) ((mv)->f)
#endif

#ifdef ZEN_ORACLE_DV_CHROMA_SS
#define ZEN_DV_CHROMA_SS_ARG , 1
#else
#define ZEN_DV_CHROMA_SS_ARG
#endif

#ifdef ZEN_ORACLE_VARIANCE_T
#define ZEN_VAR_TYPE ZEN_ORACLE_VARIANCE_T
#else
#define ZEN_VAR_TYPE SvtVarType
#endif

#ifdef ZEN_ORACLE_MAINLINE_API
#include <stdio.h>
#include <stdlib.h>
/* The oracle lacks this feature: fail LOUDLY if a non-excluded test reaches
 * the stub. Never return a plausible value. */
static void zen_oracle_missing(const char* fn) {
    fprintf(stderr, "%s: feature does not exist in this C oracle\n", fn);
    abort();
}
#endif

#ifdef ZEN_ORACLE_INIT_SCAN
void svt_aom_init_iscan(void);
/* Runs before main() in every TU that includes this header; the repeated
   init writes identical table contents, so duplicates are harmless. */
__attribute__((constructor)) static void zen_oracle_scan_ctor(void) { svt_aom_init_iscan(); }
#endif

#endif /* ZEN_ORACLE_H */
