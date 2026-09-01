/*
 * C shim for the CDEF SEARCH signal derivation of
 * `Source/Lib/Codec/enc_mode_config.c` (port:
 * `port_enc_mode_config::cdef_search`'s `cdef_search_level_{default,allintra}`
 * ladders + `set_cdef_search_controls`).
 *
 * WHY ITS OWN TU: per-lane shim-file ownership (see `build.rs`). The sigderiv
 * lane owns `sigderiv_shims.c` and its `ref_sig_deriv_multi_processes_default`
 * reads back `pcs->cdef_level` but NOT `pcs->cdef_search_ctrls`; this lane
 * needs the controls AND the allintra arm, so it gets its own file rather than
 * growing that one's output enum.
 *
 * EVIDENCE TIER 1. `set_cdef_search_controls` is file-`static` and so is each
 * ladder (they are inline in their callers), so none can be called directly --
 * but the EXPORTED `svt_aom_sig_deriv_multi_processes_{default,allintra}` run
 * both and leave the result in `pcs->cdef_level` + `pcs->cdef_search_ctrls`,
 * which this shim reads back. So the differential drives the real C ladder AND
 * the real C controls table; it is not a transcription agreeing with a
 * transcription. Same route as `dlf_shims.c`.
 *
 * `set_cdef_search_controls` keys two of its fields on `frame_is_boosted(pcs)`
 * and `!frame_is_leaf(pcs)` -- i.e. on `pcs->frm_hdr.frame_type` and
 * `pcs->update_type`, NOT on `temporal_layer_index`. Both are inputs here so
 * the four combinations are reachable.
 *
 * RULE, inherited from ref_shims.c: A SHIM MUST NOT KEEP PER-CALL STATE IN A
 * `static`. Every synthetic struct below is calloc/free per call.
 *
 * Deref safety: `pcs->av1_cm` is dereferenced by `svt_aom_set_wn_filter_ctrls`
 * / `svt_aom_set_sg_filter_ctrls` in both arms, so an Av1Common is allocated;
 * everything else either arm touches lives in `scs`/`pcs` themselves
 * (`set_intrabc_level` and `set_palette_level` write only `pcs->*_ctrls`).
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "enc_mode_config.h"

enum {
    CDS_I_ENC_MODE = 0, CDS_I_IS_ISLICE, CDS_I_TEMPORAL_LAYER, CDS_I_INPUT_RES,
    CDS_I_FAST_DECODE, CDS_I_SC_CLASS5, CDS_I_IS_HIGHEST_LAYER, CDS_I_TF_HME_LEVEL,
    CDS_I_ENABLE_INTRABC, CDS_I_SEQ_CDEF_LEVEL, CDS_I_CFG_CDEF_LEVEL,
    CDS_I_SEQ_ENABLE_RESTORATION, CDS_I_INIT_LUMA_W, CDS_I_INIT_LUMA_H,
    CDS_I_ENCODER_BIT_DEPTH, CDS_I_CFG_HBD_MDS, CDS_I_HBD_MODE_DECISION,
    CDS_I_FRAME_TYPE, CDS_I_UPDATE_TYPE,
    CDS_I_COUNT
};

/* Scalars first, then the four 64-entry candidate arrays. */
#define CDS_ARR 64
enum {
    CDS_O_LEVEL = 0, CDS_O_ENABLED, CDS_O_FIRST_NUM, CDS_O_SECOND_NUM,
    CDS_O_USE_REF_FS, CDS_O_SUBSAMPLING, CDS_O_BEST_REF_FS, CDS_O_SKIP_TH,
    CDS_O_UV_FROM_Y, CDS_O_USE_QP_STRENGTH,
    CDS_O_ALLOW_INTRABC, CDS_O_PALETTE_LEVEL,
    CDS_O_FIRST_FS = 16,
    CDS_O_SECOND_FS = CDS_O_FIRST_FS + CDS_ARR,
    CDS_O_FIRST_FS_UV = CDS_O_SECOND_FS + CDS_ARR,
    CDS_O_SECOND_FS_UV = CDS_O_FIRST_FS_UV + CDS_ARR,
    CDS_O_COUNT = CDS_O_SECOND_FS_UV + CDS_ARR
};

static void cds_populate(const int32_t* in, SequenceControlSet* scs, PictureParentControlSet* pcs,
                         Av1Common* cm) {
    scs->static_config.fast_decode     = (uint8_t)in[CDS_I_FAST_DECODE];
    scs->static_config.enable_intrabc  = (int8_t)in[CDS_I_ENABLE_INTRABC];
    scs->static_config.cdef_level      = in[CDS_I_CFG_CDEF_LEVEL];
    scs->static_config.hbd_mds         = in[CDS_I_CFG_HBD_MDS];
    scs->enable_hbd_mode_decision      = (int8_t)in[CDS_I_HBD_MODE_DECISION];
    scs->seq_header.cdef_level         = (uint8_t)in[CDS_I_SEQ_CDEF_LEVEL];
    scs->seq_header.enable_restoration = (uint8_t)in[CDS_I_SEQ_ENABLE_RESTORATION];
    scs->max_initial_input_luma_width  = (uint16_t)in[CDS_I_INIT_LUMA_W];
    scs->max_initial_input_luma_height = (uint16_t)in[CDS_I_INIT_LUMA_H];
    scs->encoder_bit_depth             = (uint32_t)in[CDS_I_ENCODER_BIT_DEPTH];

    pcs->scs                   = scs;
    pcs->av1_cm                = cm;
    pcs->enc_mode              = (EncMode)in[CDS_I_ENC_MODE];
    pcs->slice_type            = in[CDS_I_IS_ISLICE] ? I_SLICE : B_SLICE;
    pcs->temporal_layer_index  = (uint8_t)in[CDS_I_TEMPORAL_LAYER];
    pcs->input_resolution      = (ResolutionRange)in[CDS_I_INPUT_RES];
    pcs->sc_class5             = (uint8_t)in[CDS_I_SC_CLASS5];
    pcs->is_highest_layer      = (bool)in[CDS_I_IS_HIGHEST_LAYER];
    pcs->tf_ctrls.hme_me_level = (uint8_t)in[CDS_I_TF_HME_LEVEL];
    /* The two inputs of frame_is_boosted / frame_is_leaf. */
    pcs->frm_hdr.frame_type    = (FrameType)in[CDS_I_FRAME_TYPE];
    pcs->update_type           = (SvtAv1FrameUpdateType)in[CDS_I_UPDATE_TYPE];
}

static void cds_read_out(PictureParentControlSet* pcs, int64_t* out) {
    const CdefSearchControls* c = &pcs->cdef_search_ctrls;
    int i;
    out[CDS_O_LEVEL]           = pcs->cdef_level;
    out[CDS_O_ENABLED]         = c->enabled;
    out[CDS_O_FIRST_NUM]       = c->first_pass_fs_num;
    out[CDS_O_SECOND_NUM]      = c->default_second_pass_fs_num;
    out[CDS_O_USE_REF_FS]      = c->use_reference_cdef_fs;
    out[CDS_O_SUBSAMPLING]     = c->subsampling_factor;
    out[CDS_O_BEST_REF_FS]     = c->search_best_ref_fs;
    out[CDS_O_SKIP_TH]         = c->skip_th;
    out[CDS_O_UV_FROM_Y]       = c->uv_from_y;
    out[CDS_O_USE_QP_STRENGTH] = c->use_qp_strength;
    out[CDS_O_ALLOW_INTRABC]   = pcs->frm_hdr.allow_intrabc;
    out[CDS_O_PALETTE_LEVEL]   = pcs->palette_level;
    for (i = 0; i < CDS_ARR; i++) {
        out[CDS_O_FIRST_FS + i]     = c->default_first_pass_fs[i];
        out[CDS_O_SECOND_FS + i]    = c->default_second_pass_fs[i];
        out[CDS_O_FIRST_FS_UV + i]  = c->default_first_pass_fs_uv[i];
        out[CDS_O_SECOND_FS_UV + i] = c->default_second_pass_fs_uv[i];
    }
}

/* The VIDEO arm: svt_aom_sig_deriv_multi_processes_default
 * (enc_mode_config.c:1973) -> its cdef_search_level ladder (:2083). */
void ref_cdef_search_ctrls_default(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(*pcs));
    Av1Common*               cm  = (Av1Common*)calloc(1, sizeof(*cm));

    cds_populate(in, scs, pcs, cm);
    svt_aom_sig_deriv_multi_processes_default(scs, pcs);
    cds_read_out(pcs, out);

    free(cm); free(pcs); free(scs);
}

/* The STILL/AVIF arm: svt_aom_sig_deriv_multi_processes_allintra
 * (enc_mode_config.c:2337) -> its cdef_search_level ladder (:2396). */
void ref_cdef_search_ctrls_allintra(const int32_t* in, int64_t* out) {
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(*pcs));
    Av1Common*               cm  = (Av1Common*)calloc(1, sizeof(*cm));

    cds_populate(in, scs, pcs, cm);
    svt_aom_sig_deriv_multi_processes_allintra(scs, pcs);
    cds_read_out(pcs, out);

    free(cm); free(pcs); free(scs);
}

int32_t ref_cdef_search_ctrls_in_slots(void) { return CDS_I_COUNT; }
int32_t ref_cdef_search_ctrls_out_slots(void) { return CDS_O_COUNT; }
int32_t ref_cdef_search_ctrls_arr_len(void) { return CDS_ARR; }
