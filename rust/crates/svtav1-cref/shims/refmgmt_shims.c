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
#include "aom_dsp_rtcd.h"
#include "pic_buffer_desc.h"

/* Neither geometry builder is declared in a public header. */
EbErrorType b64_geom_init(SequenceControlSet* scs, uint16_t width, uint16_t height, B64Geom** b64_geoms);
EbErrorType sb_geom_init(SequenceControlSet* scs, uint16_t width, uint16_t height, SbGeom** sb_geoms);
void        svt_aom_get_max_allocated_me_refs(uint8_t ref_count_used_list0, uint8_t ref_count_used_list1,
                                             uint8_t* max_ref_to_alloc, uint8_t* max_cand_to_alloc);
uint32_t    svt_aom_get_out_buffer_size(uint32_t picture_width, uint32_t picture_height);
int32_t     svt_aom_get_frame_update_type(PictureParentControlSet* pcs);
uint8_t     svt_aom_get_denom_idx(uint8_t scale_denom);
EbErrorType svt_av1_resize_plane_c(const uint8_t* const input, int height, int width, int in_stride,
                                   uint8_t* output, int height2, int width2, int out_stride);
EbErrorType svt_av1_highbd_resize_plane_c(const uint16_t* const input, int height, int width, int in_stride,
                                          uint16_t* output, int height2, int width2, int out_stride, int bd);

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

/* ---- resize.c: the two exported superres-decision symbols ---- */

int32_t superres_frame_update_type(int32_t is_key_frame, uint8_t hierarchical_levels, uint8_t temporal_layer_index) {
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(*pcs));
    pcs->frm_hdr.frame_type      = is_key_frame ? KEY_FRAME : INTER_FRAME;
    pcs->hierarchical_levels     = hierarchical_levels;
    pcs->temporal_layer_index    = temporal_layer_index;
    const int32_t t              = svt_aom_get_frame_update_type(pcs);
    free(pcs);
    return t;
}

uint8_t superres_denom_idx(uint8_t scale_denom) { return svt_aom_get_denom_idx(scale_denom); }

/* ---- resize.c: the TWO-dimensional plane resize (frame resize) ---- */

/* `svt_av1_down2_symeven` / `svt_av1_interpolate_core` (and their highbd
 * twins) are RTCD FUNCTION POINTERS from aom_dsp_rtcd.c, NOT plain functions.
 * Without this setup the multistep driver inside `svt_av1_resize_plane_c`
 * calls through NULL and segfaults — which it did, on the first run of this
 * shim. Same trap as `ref_resize_plane_horizontal` in ref_shims.c, which
 * documents it; the init is per-translation-unit here because that one's
 * helper is `static`. */
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);
static int g_resize_rtcd_ready = 0;
static void resize_rtcd_once(void) {
    if (!g_resize_rtcd_ready) {
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        g_resize_rtcd_ready = 1;
    }
}

int32_t resize2d_plane(const uint8_t* input, int32_t height, int32_t width, int32_t in_stride, uint8_t* output,
                       int32_t height2, int32_t width2, int32_t out_stride) {
    resize_rtcd_once();
    return (int32_t)svt_av1_resize_plane_c(input, height, width, in_stride, output, height2, width2, out_stride);
}

int32_t resize2d_highbd_plane(const uint16_t* input, int32_t height, int32_t width, int32_t in_stride,
                              uint16_t* output, int32_t height2, int32_t width2, int32_t out_stride, int32_t bd) {
    resize_rtcd_once();
    return (int32_t)svt_av1_highbd_resize_plane_c(
        input, height, width, in_stride, output, height2, width2, out_stride, bd);
}

/* ---- svt_aom_resize_frame (resize.c:881) — the 8-bit plane loop ----
 * Two synthetic EbPictureBufferDescs whose buffers point at caller memory.
 * `border` is 0, so C's border-offset arithmetic (which only runs on the
 * bd > 8 && !is_packed path) never applies and the buffer pointers ARE
 * pixel (0, 0). `bd = 8` and `is_2bcompress = 0` keep it on the 8-bit arm. */
EbErrorType svt_aom_resize_frame(const EbPictureBufferDesc* src, EbPictureBufferDesc* dst, int bd,
                                 const int num_planes, const uint32_t ss_x, const uint32_t ss_y, uint8_t is_packed,
                                 uint32_t buffer_enable_mask, uint8_t is_2bcompress);

int32_t resize2d_frame(uint8_t* sy, uint8_t* su, uint8_t* sv, uint16_t sys, uint16_t sus, uint16_t svs,
                       uint16_t src_w, uint16_t src_h, uint8_t* dy, uint8_t* du, uint8_t* dv, uint16_t dys,
                       uint16_t dus, uint16_t dvs, uint16_t dst_w, uint16_t dst_h, int32_t num_planes,
                       uint32_t ss_x, uint32_t ss_y) {
    resize_rtcd_once();
    EbPictureBufferDesc* src = (EbPictureBufferDesc*)calloc(1, sizeof(*src));
    EbPictureBufferDesc* dst = (EbPictureBufferDesc*)calloc(1, sizeof(*dst));

    src->y_buffer = sy;  src->u_buffer = su;  src->v_buffer = sv;
    src->y_stride = sys; src->u_stride = sus; src->v_stride = svs;
    src->width = src_w;  src->height = src_h; src->border = 0;

    dst->y_buffer = dy;  dst->u_buffer = du;  dst->v_buffer = dv;
    dst->y_stride = dys; dst->u_stride = dus; dst->v_stride = dvs;
    dst->width = dst_w;  dst->height = dst_h; dst->border = 0;

    const uint32_t mask = PICTURE_BUFFER_DESC_Y_FLAG | PICTURE_BUFFER_DESC_Cb_FLAG | PICTURE_BUFFER_DESC_Cr_FLAG;
    const EbErrorType rc = svt_aom_resize_frame(src, dst, 8, num_planes, ss_x, ss_y, 0, mask, 0);

    free(dst);
    free(src);
    return (int32_t)rc;
}
