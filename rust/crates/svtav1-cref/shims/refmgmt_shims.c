/*
 * C shims for the long-term reference-management surface of
 * `Codec/pd_process.c` (evidence tier 1).
 *
 * Both functions here are real exported symbols (`nm -g
 * Bin/Release/libSvtAv1Enc.a` reports `T _svt_aom_ref_mgmt_storeable_slots_mask`
 * and `T _svt_aom_is_pic_skipped`). Each shim builds the minimum synthetic
 * control set the callee reads, calls the exported function, and returns the
 * result.
 *
 * `svt_aom_ref_mgmt_storeable_slots_mask` calls the FILE-STATIC
 * `exclusive_write_slots_mask_ld_cbr`, so driving the wrapper drives that
 * helper too — the static gets tier-1 coverage without needing a symbol of
 * its own.
 *
 * RULE (see ref_shims.c): NO PER-CALL STATE IN A `static`. cargo runs a test
 * binary's tests on several threads; a `static` scratch here is a data race
 * that shows up as an occasional wrong NUMBER, not a crash. Everything below
 * is calloc/free per call.
 *
 * Own translation unit so this lane never shares an editable file with a
 * concurrent lane.
 */
#include <stdint.h>
#include <stdlib.h>

#include "pcs.h"
#include "pd_process.h"
#include "sequence_control_set.h"

/* Neither geometry builder is declared in a public header. */
EbErrorType b64_geom_init(SequenceControlSet* scs, uint16_t width, uint16_t height, B64Geom** b64_geoms);
EbErrorType sb_geom_init(SequenceControlSet* scs, uint16_t width, uint16_t height, SbGeom** sb_geoms);
void        svt_aom_get_max_allocated_me_refs(uint8_t ref_count_used_list0, uint8_t ref_count_used_list1,
                                             uint8_t* max_ref_to_alloc, uint8_t* max_cand_to_alloc);
uint32_t    svt_aom_get_out_buffer_size(uint32_t picture_width, uint32_t picture_height);

/* ---- svt_aom_ref_mgmt_storeable_slots_mask (pd_process.c:1259) ---- */

uint8_t refmgmt_storeable_slots_mask(int32_t rtc, uint8_t hierarchical_levels, uint8_t pred_structure,
                                     uint8_t ld_reduce_ref_buffs) {
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(*scs));

    scs->static_config.rtc                 = rtc != 0;
    scs->static_config.hierarchical_levels = hierarchical_levels;
    scs->static_config.pred_structure      = (PredStructure)pred_structure;
    scs->mrp_ctrls.ld_reduce_ref_buffs     = ld_reduce_ref_buffs;

    const uint8_t mask = svt_aom_ref_mgmt_storeable_slots_mask(scs);
    free(scs);
    return mask;
}

/* ---- svt_aom_is_pic_skipped (pd_process.c:996) ---- */

int32_t refmgmt_is_pic_skipped(int32_t is_ref, uint8_t rc_stat_gen_pass_mode, uint8_t first_frame_in_minigop) {
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(*scs));

    pcs->scs                     = scs;
    pcs->is_ref                  = is_ref != 0;
    pcs->first_frame_in_minigop  = first_frame_in_minigop;
    scs->rc_stat_gen_pass_mode   = rc_stat_gen_pass_mode;

    const int32_t skipped = svt_aom_is_pic_skipped(pcs) ? 1 : 0;
    free(scs);
    free(pcs);
    return skipped;
}

/* ---- pcs.c block-grid geometry + allocation sizing ---- */

void pcsgeom_max_allocated_me_refs(uint8_t l0, uint8_t l1, uint8_t* max_ref, uint8_t* max_cand) {
    svt_aom_get_max_allocated_me_refs(l0, l1, max_ref, max_cand);
}

uint32_t pcsgeom_out_buffer_size(uint32_t w, uint32_t h) { return svt_aom_get_out_buffer_size(w, h); }

/* `b64_geom_init` / `sb_geom_init` allocate the array through SVT's own
 * EB_MALLOC_ARRAY and free the previous one through EB_FREE_ARRAY, so the
 * shim hands them a NULL pointer to fill and frees the result itself. Fields
 * are copied into flat arrays because the caller must not depend on the C
 * struct layout. */

uint32_t pcsgeom_b64_geom_init(uint8_t b64_size, uint16_t width, uint16_t height, uint32_t cap,
                               uint16_t* org_x, uint16_t* org_y, uint8_t* w, uint8_t* h, uint8_t* complete) {
    SequenceControlSet* scs      = (SequenceControlSet*)calloc(1, sizeof(*scs));
    B64Geom*            geoms    = NULL;
    scs->b64_size                = b64_size;

    b64_geom_init(scs, width, height, &geoms);

    const uint32_t cols = (uint32_t)((width + b64_size - 1) / b64_size);
    const uint32_t rows = (uint32_t)((height + b64_size - 1) / b64_size);
    uint32_t       n    = cols * rows;
    if (n > cap) {
        n = cap;
    }
    for (uint32_t i = 0; i < n; ++i) {
        org_x[i]    = geoms[i].org_x;
        org_y[i]    = geoms[i].org_y;
        w[i]        = geoms[i].width;
        h[i]        = geoms[i].height;
        complete[i] = geoms[i].is_complete_b64;
    }
    free(geoms);
    free(scs);
    return n;
}

uint32_t pcsgeom_sb_geom_init(uint16_t sb_size, uint16_t width, uint16_t height, uint32_t cap, uint16_t* org_x,
                              uint16_t* org_y, uint8_t* w, uint8_t* h) {
    SequenceControlSet* scs   = (SequenceControlSet*)calloc(1, sizeof(*scs));
    SbGeom*             geoms = NULL;
    scs->sb_size              = sb_size;

    sb_geom_init(scs, width, height, &geoms);

    const uint32_t cols = (uint32_t)((width + sb_size - 1) / sb_size);
    const uint32_t rows = (uint32_t)((height + sb_size - 1) / sb_size);
    uint32_t       n    = cols * rows;
    if (n > cap) {
        n = cap;
    }
    for (uint32_t i = 0; i < n; ++i) {
        org_x[i] = geoms[i].org_x;
        org_y[i] = geoms[i].org_y;
        w[i]     = geoms[i].width;
        h[i]     = geoms[i].height;
    }
    free(geoms);
    free(scs);
    return n;
}
