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
