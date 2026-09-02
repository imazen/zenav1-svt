/*
 * C shims for CDF CONTINUATION — the FRAME_CONTEXT the encoder saves onto a
 * reference and restores from it (`crate::port_frame_cdf`).
 *
 * Its own translation unit, like inter_mvp_shims.c and entropy_inter_shims.c,
 * so this lane never shares an editable file with a concurrent lane.
 *
 * Every entry point drives REAL exported SVT-AV1 symbols (evidence tier 1,
 * docs/WORKING-ON-THIS.md section 4):
 *   svt_aom_init_mode_probs            cabac_context_model.c:740
 *   svt_av1_default_coef_probs         cabac_context_model.c (qindex bucket)
 *   svt_av1_reset_cdf_symbol_counters  cabac_context_model.c:1970
 *
 * WHY A NAME->FIELD LOOKUP RATHER THAN A STRUCT MIRROR. The port does not lay
 * FRAME_CONTEXT out the way C does: the coefficient and ext-tx tables live in
 * `entropy::coeff_c::CoeffFc`, the inter tables in
 * `port_entropy_inter::InterCdfs`, the rest in `entropy::context::FrameContext`.
 * Mirroring the C struct in Rust would add a third layout that must agree with
 * the other two. Asking C for ONE NAMED FIELD at a time compares the two sides
 * where they actually agree — the AV1 syntax element — and lets the Rust side
 * enumerate from its own single field list
 * (`FrameCdfs::for_each_field_mut`), so a field the port forgot to carry is a
 * field the test never asks for, and the test says which ones those are.
 *
 * Shim state is per-call and on the stack (never a `static` buffer): cargo
 * runs a test binary's tests on several threads.
 */
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "cabac_context_model.h"

/* RTCD FIRST — this shim re-hit a trap `entropy_inter_shims.c:107-118` had
 * already solved and commented, and it cost a cross-ISA run to find.
 *
 * `svt_aom_init_mode_probs` and `svt_av1_default_coef_probs` copy their tables
 * with `COPY_CDF`, which is `svt_memcpy` (cabac_context_model.c:735) — an RTCD
 * FUNCTION POINTER in .bss (common_dsp_rtcd.h:1083), NULL until
 * `svt_aom_setup_common_rtcd_internal` runs. On aarch64 the hazard CANNOT
 * fire: NEON devirtualization rewrites `svt_memcpy` to the concrete
 * `svt_memcpy_neon` (common_dsp_rtcd_neon_devirt.h:266), so there is no
 * pointer to be NULL. On x86-64 the call lands at rip=0x0.
 *
 * MEASURED 2026-09-01: without this, `the_default_frame_context_matches_c_
 * field_for_field` and `the_reset_is_observable_at_all` SIGSEGV on
 * x86_64-linux and pass on aarch64-darwin — while the two tests that only use
 * the PAINTED modes (which call neither initializer) pass on both. That split
 * is the fingerprint: a NULL RTCD pointer, not a buffer bug.
 *
 * `g_fctx_rtcd_ready` is an idempotent one-shot, not per-call state — a racing
 * double-init lands the same pointers. */
typedef uint64_t FctxCpuFlags;
FctxCpuFlags svt_aom_get_cpu_flags_to_use(void);
void         svt_aom_setup_common_rtcd_internal(uint64_t flags);

static int  g_fctx_rtcd_ready = 0;
static void fctx_ensure_rtcd(void) {
    if (!g_fctx_rtcd_ready) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_fctx_rtcd_ready = 1;
    }
}

typedef struct {
    const char       *name;
    const AomCdfProb *ptr;
    size_t            count;
} FctxField;

#define FF(nm, arr)                                                           \
    do {                                                                      \
        tbl[n].name  = (nm);                                                  \
        tbl[n].ptr   = (const AomCdfProb *)(arr);                             \
        tbl[n].count = sizeof(arr) / sizeof(AomCdfProb);                      \
        n++;                                                                  \
    } while (0)

/* One NmvContext contributes 17 named runs. */
static size_t fctx_add_nmv(FctxField *tbl, size_t n, const NmvContext *nmv, const char *const *names) {
    FF(names[0], nmv->joints_cdf);
    for (int i = 0; i < 2; i++) {
        FF(names[1 + i * 8 + 0], nmv->comps[i].classes_cdf);
        FF(names[1 + i * 8 + 1], nmv->comps[i].class0_fp_cdf);
        FF(names[1 + i * 8 + 2], nmv->comps[i].fp_cdf);
        FF(names[1 + i * 8 + 3], nmv->comps[i].sign_cdf);
        FF(names[1 + i * 8 + 4], nmv->comps[i].class0_hp_cdf);
        FF(names[1 + i * 8 + 5], nmv->comps[i].hp_cdf);
        FF(names[1 + i * 8 + 6], nmv->comps[i].class0_cdf);
        FF(names[1 + i * 8 + 7], nmv->comps[i].bits_cdf);
    }
    return n;
}

static const char *const NMVC_NAMES[17] = {
    "nmvc.joints",
    "nmvc.comp0.classes",   "nmvc.comp0.class0_fp", "nmvc.comp0.fp",  "nmvc.comp0.sign",
    "nmvc.comp0.class0_hp", "nmvc.comp0.hp",        "nmvc.comp0.class0", "nmvc.comp0.bits",
    "nmvc.comp1.classes",   "nmvc.comp1.class0_fp", "nmvc.comp1.fp",  "nmvc.comp1.sign",
    "nmvc.comp1.class0_hp", "nmvc.comp1.hp",        "nmvc.comp1.class0", "nmvc.comp1.bits",
};
static const char *const NDVC_NAMES[17] = {
    "ndvc.joints",
    "ndvc.comp0.classes",   "ndvc.comp0.class0_fp", "ndvc.comp0.fp",  "ndvc.comp0.sign",
    "ndvc.comp0.class0_hp", "ndvc.comp0.hp",        "ndvc.comp0.class0", "ndvc.comp0.bits",
    "ndvc.comp1.classes",   "ndvc.comp1.class0_fp", "ndvc.comp1.fp",  "ndvc.comp1.sign",
    "ndvc.comp1.class0_hp", "ndvc.comp1.hp",        "ndvc.comp1.class0", "ndvc.comp1.bits",
};

/* Build the whole name table over `fc`. Returns the entry count. */
static size_t fctx_build(FctxField *tbl, const FRAME_CONTEXT *fc) {
    size_t n = 0;
    FF("txb_skip", fc->txb_skip_cdf);
    FF("eob_extra", fc->eob_extra_cdf);
    FF("dc_sign", fc->dc_sign_cdf);
    FF("eob_flag16", fc->eob_flag_cdf16);
    FF("eob_flag32", fc->eob_flag_cdf32);
    FF("eob_flag64", fc->eob_flag_cdf64);
    FF("eob_flag128", fc->eob_flag_cdf128);
    FF("eob_flag256", fc->eob_flag_cdf256);
    FF("eob_flag512", fc->eob_flag_cdf512);
    FF("eob_flag1024", fc->eob_flag_cdf1024);
    FF("coeff_base_eob", fc->coeff_base_eob_cdf);
    FF("coeff_base", fc->coeff_base_cdf);
    FF("coeff_br", fc->coeff_br_cdf);
    FF("newmv", fc->newmv_cdf);
    FF("zeromv", fc->zeromv_cdf);
    FF("refmv", fc->refmv_cdf);
    FF("drl", fc->drl_cdf);
    FF("inter_compound_mode", fc->inter_compound_mode_cdf);
    FF("compound_type", fc->compound_type_cdf);
    FF("wedge_idx", fc->wedge_idx_cdf);
    FF("interintra", fc->interintra_cdf);
    FF("wedge_interintra", fc->wedge_interintra_cdf);
    FF("interintra_mode", fc->interintra_mode_cdf);
    FF("motion_mode", fc->motion_mode_cdf);
    FF("obmc", fc->obmc_cdf);
    FF("palette_y_size", fc->palette_y_size_cdf);
    FF("palette_uv_size", fc->palette_uv_size_cdf);
    FF("palette_y_color_index", fc->palette_y_color_index_cdf);
    FF("palette_uv_color_index", fc->palette_uv_color_index_cdf);
    FF("palette_y_mode", fc->palette_y_mode_cdf);
    FF("palette_uv_mode", fc->palette_uv_mode_cdf);
    FF("comp_inter", fc->comp_inter_cdf);
    FF("single_ref", fc->single_ref_cdf);
    FF("comp_ref_type", fc->comp_ref_type_cdf);
    FF("uni_comp_ref", fc->uni_comp_ref_cdf);
    FF("comp_ref", fc->comp_ref_cdf);
    FF("comp_bwdref", fc->comp_bwdref_cdf);
    FF("txfm_partition", fc->txfm_partition_cdf);
    FF("compound_index", fc->compound_index_cdf);
    FF("comp_group_idx", fc->comp_group_idx_cdf);
    FF("skip_mode", fc->skip_mode_cdfs);
    FF("skip", fc->skip_cdfs);
    FF("intra_inter", fc->intra_inter_cdf);
    n = fctx_add_nmv(tbl, n, &fc->nmvc, NMVC_NAMES);
    n = fctx_add_nmv(tbl, n, &fc->ndvc, NDVC_NAMES);
    FF("intrabc", fc->intrabc_cdf);
    FF("seg.tree", fc->seg.tree_cdf);
    FF("seg.pred", fc->seg.pred_cdf);
    FF("seg.spatial_pred", fc->seg.spatial_pred_seg_cdf);
    FF("filter_intra", fc->filter_intra_cdfs);
    FF("filter_intra_mode", fc->filter_intra_mode_cdf);
    FF("switchable_restore", fc->switchable_restore_cdf);
    FF("wiener_restore", fc->wiener_restore_cdf);
    FF("sgrproj_restore", fc->sgrproj_restore_cdf);
    FF("y_mode", fc->y_mode_cdf);
    FF("uv_mode", fc->uv_mode_cdf);
    FF("partition", fc->partition_cdf);
    FF("switchable_interp", fc->switchable_interp_cdf);
    FF("kf_y", fc->kf_y_cdf);
    FF("angle_delta", fc->angle_delta_cdf);
    FF("tx_size", fc->tx_size_cdf);
    FF("delta_q", fc->delta_q_cdf);
    FF("delta_lf_multi", fc->delta_lf_multi_cdf);
    FF("delta_lf", fc->delta_lf_cdf);
    FF("intra_ext_tx", fc->intra_ext_tx_cdf);
    FF("inter_ext_tx", fc->inter_ext_tx_cdf);
    FF("cfl_sign", fc->cfl_sign_cdf);
    FF("cfl_alpha", fc->cfl_alpha_cdf);
    return n;
}

#define FCTX_MAX_FIELDS 128

/*
 * Fill a FRAME_CONTEXT and copy out ONE named field.
 *
 * `mode` selects what the context holds, and the four modes exist so a test
 * can separate "the defaults agree" from "the counter reset agrees" — two
 * different claims that a single mode would conflate:
 *
 *   0  C's PRIMARY_REF_NONE arm (`md_config_process.c:307-309`):
 *      svt_av1_default_coef_probs(qindex) then svt_aom_init_mode_probs.
 *   1  mode 0, then svt_av1_reset_cdf_symbol_counters.
 *   2  every byte painted 0x12 (so every AomCdfProb is 0x1212).
 *   3  mode 2, then svt_av1_reset_cdf_symbol_counters.
 *
 * Mode 3 vs mode 2 is the only way to see the reset's (nsymbs, stride) map for
 * a field: on a DEFAULT context every counter is already 0, so mode 1 equals
 * mode 0 and a test built on that pair would pass with the reset deleted.
 *
 * Returns the field's element count (0 if `name` is not a FRAME_CONTEXT
 * field), copying min(count, cap) elements into `out`. A caller that gets a
 * nonzero return larger than its `cap` knows its own array is the wrong size
 * rather than silently comparing a prefix.
 */
size_t ref_frame_ctx_field(int32_t qindex, int32_t mode, const char *name, uint16_t *out, size_t cap) {
    fctx_ensure_rtcd();
    FRAME_CONTEXT fc;
    if (mode >= 2) {
        memset(&fc, 0x12, sizeof(fc));
    } else {
        memset(&fc, 0, sizeof(fc));
        svt_av1_default_coef_probs(&fc, qindex);
        svt_aom_init_mode_probs(&fc);
    }
    if (mode == 1 || mode == 3)
        svt_av1_reset_cdf_symbol_counters(&fc);

    FctxField tbl[FCTX_MAX_FIELDS];
    size_t    n = fctx_build(tbl, &fc);
    for (size_t i = 0; i < n; i++) {
        if (strcmp(tbl[i].name, name) != 0)
            continue;
        size_t take = tbl[i].count < cap ? tbl[i].count : cap;
        memcpy(out, tbl[i].ptr, take * sizeof(uint16_t));
        return tbl[i].count;
    }
    return 0;
}

/* How many named fields the C FRAME_CONTEXT exposes — the anti-vacuity counter
 * for a test that walks the PORT's list and would otherwise never notice a C
 * field nobody asked about. */
size_t ref_frame_ctx_field_count(void) {
    FRAME_CONTEXT fc;
    memset(&fc, 0, sizeof(fc));
    FctxField tbl[FCTX_MAX_FIELDS];
    return fctx_build(tbl, &fc);
}

/* The i-th field's name, so a test can enumerate C's side too. */
const char *ref_frame_ctx_field_name(size_t idx) {
    FRAME_CONTEXT fc;
    memset(&fc, 0, sizeof(fc));
    FctxField tbl[FCTX_MAX_FIELDS];
    size_t    n = fctx_build(tbl, &fc);
    return idx < n ? tbl[idx].name : NULL;
}
