/*
 * C shims for the full-mode-decision port (lane wx-md, mode_decision.c).
 *
 * Its own translation unit alongside this lane's other two, so each C file
 * this lane ports keeps a separate oracle.
 *
 *   svt_av1_is_lossless_segment   mode_decision.c:71   (nm -g: T)
 *
 * `svt_aom_product_full_mode_decision` itself is NOT driven here: it takes an
 * array of ModeDecisionCandidateBuffer* whose costs are pointers into
 * per-candidate scratch and writes into a BlkStruct that owns an av1xd, a
 * palette buffer and a coefficient-buffer graph. Assembling that in a shim
 * would produce an artifact larger than the ordering rule under test, so the
 * port pins that rule with hand-derived vectors and says tier 4 (see
 * port_md_winner.rs's module doc).
 *
 * State discipline: per-call calloc, never a file-scope buffer.
 */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "EbSvtAv1.h"
#include "md_process.h"
#include "pcs.h"
#include "sys_resource_manager.h"

bool svt_av1_is_lossless_segment(PictureControlSet* pcs, int8_t segment_id);

/* `lossless` is MAX_SEGMENTS bools. */
int32_t ref_mdw_segments(void) { return MAX_SEGMENTS; }

int32_t ref_mdw_is_lossless_segment(int32_t segmentation_enabled, const int32_t* lossless,
                                    int32_t segment_id) {
    PictureControlSet*       pcs  = (PictureControlSet*)calloc(1, sizeof(*pcs));
    PictureParentControlSet* ppcs = (PictureParentControlSet*)calloc(1, sizeof(*ppcs));
    pcs->ppcs = ppcs;
    ppcs->frm_hdr.segmentation_params.segmentation_enabled = (uint8_t)segmentation_enabled;
    for (int k = 0; k < MAX_SEGMENTS; ++k) {
        pcs->lossless[k] = (bool)lossless[k];
    }
    const int32_t r = (int32_t)svt_av1_is_lossless_segment(pcs, (int8_t)segment_id);
    free(ppcs);
    free(pcs);
    return r;
}
