/*
 * C shims for the full_loop.c MD-side port (lane wx-md).
 *
 * Its own translation unit, alongside this lane's rd_cost_shims.c, so the
 * two C files this lane ports keep separate oracles and neither shares an
 * editable file with a concurrent lane.
 *
 *   svt_aom_do_md_recon        full_loop.c:2739  (nm -g: T)
 *
 * State discipline: per-call calloc, never a file-scope buffer — cargo runs
 * a test binary's tests on several threads.
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

uint8_t svt_aom_do_md_recon(PictureParentControlSet* pcs, ModeDecisionContext* ctx);

/* i[] layout:
 *  0 bypass_encdec        1 pd_pass_1           2 skip_intra
 *  3 inter_intra_enabled  4 is_ref              5 recon_enabled
 *  6 dlf_enabled          7 cdef_enabled        8 cdef_use_qp_strength
 *  9 cdef_use_reference_fs 10 enable_restoration
 * 11 compute_psnr        12 compute_ssim
 */
#define RD_RECON_FIELDS 13

int32_t ref_fl_recon_fields(void) { return RD_RECON_FIELDS; }

int32_t ref_fl_do_md_recon(const int32_t* i) {
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(*pcs));
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(*scs));
    ModeDecisionContext*     ctx = (ModeDecisionContext*)calloc(1, sizeof(*ctx));

    pcs->scs = scs;
    ctx->bypass_encdec                    = (uint8_t)i[0];
    ctx->pd_pass                          = i[1] ? PD_PASS_1 : PD_PASS_0;
    ctx->skip_intra                       = (uint8_t)i[2];
    ctx->inter_intra_comp_ctrls.enabled   = (uint8_t)i[3];
    pcs->is_ref                           = (bool)i[4];
    scs->static_config.recon_enabled      = (bool)i[5];
    pcs->dlf_ctrls.enabled                = (uint8_t)i[6];
    pcs->cdef_search_ctrls.enabled        = (uint8_t)i[7];
    pcs->cdef_search_ctrls.use_qp_strength      = (uint8_t)i[8];
    pcs->cdef_search_ctrls.use_reference_cdef_fs = (uint8_t)i[9];
    pcs->enable_restoration               = (uint8_t)i[10];
    pcs->compute_psnr                     = (bool)i[11];
    pcs->compute_ssim                     = (bool)i[12];

    const int32_t r = (int32_t)svt_aom_do_md_recon(pcs, ctx);
    free(ctx);
    free(scs);
    free(pcs);
    return r;
}
