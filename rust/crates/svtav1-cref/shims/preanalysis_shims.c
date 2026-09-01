/*
 * Differential oracles for `svtav1_encoder::port_preanalysis` and
 * `svtav1_encoder::port_temporal_filtering` — the wholesale port of
 * Source/Lib/Codec/pic_analysis_process.c and Codec/temporal_filtering.c.
 *
 * Every entry point below calls the REAL exported C symbol (evidence tier 1,
 * docs/WORKING-ON-THIS.md §4). None of the logic is transcribed here.
 *
 * Own translation unit so this lane never shares an editable C file with the
 * concurrent C0/C2/C3/C4 lanes.
 *
 * Shim state lives on the STACK only: cargo runs a test binary's tests on
 * several threads and a `static` scratch buffer would race.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "pic_buffer_desc.h"
#include "av1me.h"
#include "pcs.h"

void       svt_aom_setup_common_rtcd_internal(uint64_t flags);
void       svt_aom_setup_rtcd_internal(EbCpuFlags flags);
EbCpuFlags svt_aom_get_cpu_flags_to_use(void);

static int pre_rtcd_done = 0;
static void pre_ensure_rtcd(void) {
    if (!pre_rtcd_done) {
        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
        pre_rtcd_done = 1;
    }
}

/* ---- pic_analysis_process.c / pic_operators.c leaves (all EXPORTED) ---- */

void svt_aom_downsample_2d_c(uint8_t* input_samples, uint32_t input_stride, uint32_t input_area_width,
                             uint32_t input_area_height, uint8_t* decim_samples, uint32_t decim_stride,
                             uint32_t decim_step);
void calculate_histogram(uint8_t* input_samples, uint32_t input_area_width, uint32_t input_area_height,
                         uint32_t stride, uint8_t decim_step, uint32_t* histogram, uint64_t* sum);
void svt_aom_generate_padding(uint8_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                              uint32_t original_src_height, uint32_t padding_width, uint32_t padding_height);
void pad_input_picture(uint8_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                       uint32_t original_src_height, uint32_t pad_right, uint32_t pad_bottom);
bool svt_aom_is_input_luma_dominant(const EbPictureBufferDesc* input_pic);

void ref_pre_downsample_2d(uint8_t* input_samples, uint32_t input_stride, uint32_t w, uint32_t h,
                           uint8_t* decim_samples, uint32_t decim_stride, uint32_t decim_step) {
    svt_aom_downsample_2d_c(input_samples, input_stride, w, h, decim_samples, decim_stride, decim_step);
}

void ref_pre_calculate_histogram(uint8_t* input_samples, uint32_t w, uint32_t h, uint32_t stride, uint8_t decim_step,
                                 uint32_t* histogram, uint64_t* sum) {
    calculate_histogram(input_samples, w, h, stride, decim_step, histogram, sum);
}

/* `buf` is the whole allocation; `origin` the byte offset of C's `src_pic`. */
void ref_pre_generate_padding(uint8_t* buf, uint32_t origin, uint32_t src_stride, uint32_t w, uint32_t h,
                              uint32_t padding_width, uint32_t padding_height) {
    pre_ensure_rtcd();
    svt_aom_generate_padding(buf + origin, src_stride, w, h, padding_width, padding_height);
}

void ref_pre_pad_input_picture(uint8_t* src, uint32_t src_stride, uint32_t w, uint32_t h, uint32_t pad_right,
                               uint32_t pad_bottom) {
    pre_ensure_rtcd();
    pad_input_picture(src, src_stride, w, h, pad_right, pad_bottom);
}

/*
 * `svt_aom_is_input_luma_dominant` takes an EbPictureBufferDesc*. Only six
 * fields are read (color_format, width, height, u_buffer/v_buffer and their
 * strides), so the shim builds a real descriptor on the STACK and fills
 * exactly those — the facade-over-the-real-function pattern the IntraBC and
 * inter-ME shims already use. The struct is declared in pic_buffer_desc.h.
 */
int ref_pre_is_input_luma_dominant(uint32_t color_format, uint32_t width, uint32_t height, uint8_t* u_buffer,
                                   uint32_t u_stride, uint8_t* v_buffer, uint32_t v_stride) {
    EbPictureBufferDesc desc;
    memset(&desc, 0, sizeof(desc));
    desc.color_format = color_format;
    desc.width        = width;
    desc.height       = height;
    desc.u_buffer     = u_buffer;
    desc.u_stride     = u_stride;
    desc.v_buffer     = v_buffer;
    desc.v_stride     = v_stride;
    return svt_aom_is_input_luma_dominant(&desc) ? 1 : 0;
}

/* ---- the picture drivers (all EXPORTED; facade over the real function) ---- */

#include "pcs.h"
#include "sequence_control_set.h"

void svt_aom_downsample_filtering_input_picture(PictureParentControlSet* pcs, EbPictureBufferDesc* input_padded_pic,
                                                EbPictureBufferDesc* quarter_picture_ptr,
                                                EbPictureBufferDesc* sixteenth_picture_ptr);
void svt_aom_pad_input_pictures(SequenceControlSet* scs, EbPictureBufferDesc* input_pic);
void svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(SequenceControlSet*  scs,
                                                                EbPictureBufferDesc* input_pic);

/*
 * PictureParentControlSet is ~37 KB and SequenceControlSet ~2.8 KB, so both
 * are heap-allocated per call (calloc/free) rather than put on the stack.
 * Per-call allocation is also what keeps the shim thread-safe: cargo runs a
 * test binary's tests on several threads and a `static` scratch struct races.
 * Only the fields the C function actually reads are filled; everything else
 * is zero, which is what calloc gives.
 */

void ref_pre_downsample_filtering_input_picture(int enable_hme, int tf_enable_hme, int enable_hme_l0,
                                                int tf_enable_hme_l0, int enable_hme_l1, int tf_enable_hme_l1,
                                                uint8_t* in_buf, uint32_t in_origin, uint32_t in_stride, uint32_t in_w,
                                                uint32_t in_h, uint8_t* q_buf, uint32_t q_origin, uint32_t q_stride,
                                                uint32_t q_w, uint32_t q_h, uint32_t q_border, uint8_t* s_buf,
                                                uint32_t s_origin, uint32_t s_stride, uint32_t s_w, uint32_t s_h,
                                                uint32_t s_border) {
    pre_ensure_rtcd();
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));
    EbPictureBufferDesc      in, q, s;
    memset(&in, 0, sizeof(in));
    memset(&q, 0, sizeof(q));
    memset(&s, 0, sizeof(s));

    pcs->enable_hme_flag           = enable_hme != 0;
    pcs->tf_enable_hme_flag        = tf_enable_hme != 0;
    pcs->enable_hme_level0_flag    = enable_hme_l0 != 0;
    pcs->tf_enable_hme_level0_flag = tf_enable_hme_l0 != 0;
    pcs->enable_hme_level1_flag    = enable_hme_l1 != 0;
    pcs->tf_enable_hme_level1_flag = tf_enable_hme_l1 != 0;

    in.y_buffer = in_buf + in_origin;
    in.y_stride = in_stride;
    in.width    = in_w;
    in.height   = in_h;

    q.y_buffer = q_buf + q_origin;
    q.y_stride = q_stride;
    q.width    = q_w;
    q.height   = q_h;
    q.border   = q_border;

    s.y_buffer = s_buf + s_origin;
    s.y_stride = s_stride;
    s.width    = s_w;
    s.height   = s_h;
    s.border   = s_border;

    svt_aom_downsample_filtering_input_picture(pcs, &in, &q, &s);
    free(pcs);
}

/*
 * `svt_aom_pad_input_pictures` and its min-blk-pad callee are driven through
 * one shim so the two-step order (min-blk pad, THEN the border replicate) is
 * exercised exactly as the encoder runs it. `min_blk_only` selects just the
 * first step so the port's two functions can be gated independently.
 */
void ref_pre_pad_input_pictures(int min_blk_only, uint32_t bit_depth, uint32_t color_format, uint32_t subsampling_x,
                                uint32_t subsampling_y, uint32_t pad_right, uint32_t pad_bottom, uint8_t* y_buf,
                                uint32_t y_origin, uint32_t y_stride, uint32_t width, uint32_t height, uint32_t border,
                                uint8_t* u_buf, uint32_t u_origin, uint32_t u_stride, uint8_t* v_buf,
                                uint32_t v_origin, uint32_t v_stride) {
    pre_ensure_rtcd();
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));

    scs->static_config.encoder_bit_depth = (EbBitDepth)bit_depth;
    scs->subsampling_x                   = (uint16_t)subsampling_x;
    scs->subsampling_y                   = (uint16_t)subsampling_y;
    scs->pad_right                       = pad_right;
    scs->pad_bottom                      = pad_bottom;

    pic.color_format = color_format;
    pic.y_buffer     = y_buf + y_origin;
    pic.y_stride     = y_stride;
    pic.width        = width;
    pic.height       = height;
    pic.border       = border;
    pic.u_buffer     = u_buf ? u_buf + u_origin : NULL;
    pic.u_stride     = u_stride;
    pic.v_buffer     = v_buf ? v_buf + v_origin : NULL;
    pic.v_stride     = v_stride;

    if (min_blk_only) {
        svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(scs, &pic);
    } else {
        svt_aom_pad_input_pictures(scs, &pic);
    }
    free(scs);
}

/* ---- svt_aom_gathering_picture_statistics (EXPORTED) ---- */

void svt_aom_gathering_picture_statistics(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                          EbPictureBufferDesc* input_padded_pic,
                                          EbPictureBufferDesc* sixteenth_decimated_picture_ptr);

/*
 * Drives the real exported gate. The histogram arm
 * (sub_sample_luma_generate_pixel_intensity_histogram_bins) is `static` in C,
 * so calling it through this exported parent is how the port reaches it at
 * TIER 1 rather than against hand-derived vectors.
 *
 * The `calculate_variance` arm is deliberately NOT driven: it walks
 * pcs->b64_geom calling compute_b64_variance and divides by
 * pcs->b64_total_count, so a facade PCS with no block geometry would divide by
 * zero. That arm's leaf is gated elsewhere in the port; only
 * calculate_variance == 0 is exercised here, which is also the value the
 * VIDEO-mode configuration actually uses (enc_handle.c:4361-4366).
 *
 * pcs->picture_histogram is a uint32_t*** — allocated here as a real 4x4x256
 * array-of-pointers so the C writes land somewhere the caller can read back.
 */
int ref_pre_gathering_picture_statistics(int calc_hist, int calculate_variance, uint32_t regions_w,
                                         uint32_t regions_h, int scene_change_detection, uint8_t* sixteenth,
                                         uint32_t s_origin, uint32_t s_stride, uint32_t s_w, uint32_t s_h,
                                         uint32_t* out_histogram /* [4][4][256] flattened */,
                                         uint64_t* out_avg_intensity /* [4][4] flattened */,
                                         uint64_t* out_avg_luma, uint16_t* out_pic_avg_variance) {
    if (calculate_variance) {
        /* Not drivable through a facade PCS — see the comment above. */
        return 0;
    }
    pre_ensure_rtcd();

    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));

    scs->calc_hist                                    = (uint8_t)(calc_hist != 0);
    scs->calculate_variance                           = 0;
    scs->picture_analysis_number_of_regions_per_width  = regions_w;
    scs->picture_analysis_number_of_regions_per_height = regions_h;
    scs->static_config.scene_change_detection          = (uint32_t)(scene_change_detection != 0);

    /* Real 3-level pointer array over one flat 4*4*256 block of storage. */
    uint32_t*  hist_storage = (uint32_t*)calloc((size_t)MAX_NUMBER_OF_REGIONS_IN_WIDTH *
                                                    MAX_NUMBER_OF_REGIONS_IN_HEIGHT * HISTOGRAM_NUMBER_OF_BINS,
                                                sizeof(uint32_t));
    uint32_t** row_ptrs     = (uint32_t**)calloc((size_t)MAX_NUMBER_OF_REGIONS_IN_WIDTH *
                                                 MAX_NUMBER_OF_REGIONS_IN_HEIGHT,
                                             sizeof(uint32_t*));
    uint32_t*** col_ptrs = (uint32_t***)calloc(MAX_NUMBER_OF_REGIONS_IN_WIDTH, sizeof(uint32_t**));
    for (int w = 0; w < MAX_NUMBER_OF_REGIONS_IN_WIDTH; w++) {
        col_ptrs[w] = &row_ptrs[(size_t)w * MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
        for (int h = 0; h < MAX_NUMBER_OF_REGIONS_IN_HEIGHT; h++) {
            col_ptrs[w][h] = hist_storage +
                (((size_t)w * MAX_NUMBER_OF_REGIONS_IN_HEIGHT + h) * HISTOGRAM_NUMBER_OF_BINS);
        }
    }
    pcs->picture_histogram = col_ptrs;

    EbPictureBufferDesc sixteenth_desc, padded_desc;
    memset(&sixteenth_desc, 0, sizeof(sixteenth_desc));
    memset(&padded_desc, 0, sizeof(padded_desc));
    sixteenth_desc.y_buffer = sixteenth + s_origin;
    sixteenth_desc.y_stride = s_stride;
    sixteenth_desc.width    = s_w;
    sixteenth_desc.height   = s_h;

    svt_aom_gathering_picture_statistics(scs, pcs, &padded_desc, &sixteenth_desc);

    memcpy(out_histogram,
           hist_storage,
           (size_t)MAX_NUMBER_OF_REGIONS_IN_WIDTH * MAX_NUMBER_OF_REGIONS_IN_HEIGHT * HISTOGRAM_NUMBER_OF_BINS *
               sizeof(uint32_t));
    for (int w = 0; w < MAX_NUMBER_OF_REGIONS_IN_WIDTH; w++) {
        for (int h = 0; h < MAX_NUMBER_OF_REGIONS_IN_HEIGHT; h++) {
            out_avg_intensity[(size_t)w * MAX_NUMBER_OF_REGIONS_IN_HEIGHT + h] =
                pcs->average_intensity_per_region[w][h];
        }
    }
    *out_avg_luma          = pcs->avg_luma;
    *out_pic_avg_variance  = pcs->pic_avg_variance;

    free(col_ptrs);
    free(row_ptrs);
    free(hist_storage);
    free(pcs);
    free(scs);
    return 1;
}

/* ------------------------------------------------------------------------
 * Screen-content detection (pic_analysis_process.c), all EXPORTED.
 *
 * TWO inits are needed here, not one. `svt_av1_get_sby_perpixel_variance`
 * dereferences `fn_ptr->vf`, which lives in `svt_aom_mefn_ptr` — a plain
 * global that `init_fn_ptr()` fills, NOT something the RTCD setup touches.
 * And the value `init_fn_ptr` stores is itself the RTCD pointer
 * `svt_aom_variance16x16`, which `nm` reports as a COMMON symbol (`C`) and
 * which is NULL until `svt_aom_setup_common_rtcd_internal` runs. Calling only
 * one of the two leaves a NULL two levels down from the function being
 * called — the trap documented in WORKING-ON-THIS §5. So: RTCD first, then
 * `init_fn_ptr`.
 * ---------------------------------------------------------------------- */

void         init_fn_ptr(void);
unsigned int svt_av1_get_sby_perpixel_variance(const AomVarianceFnPtr* fn_ptr, const uint8_t* src, int stride,
                                               BlockSize bs);
void         svt_aom_is_screen_content_antialiasing_aware(PictureParentControlSet* pcs);
void         svt_aom_is_screen_content(PictureParentControlSet* pcs);

static int sc_fnptr_done = 0;
static void sc_ensure_fn_ptr(void) {
    pre_ensure_rtcd();
    if (!sc_fnptr_done) {
        init_fn_ptr();
        sc_fnptr_done = 1;
    }
}

/*
 * `fn_bs` selects which `svt_aom_mefn_ptr[]` entry supplies `vf`; `norm_bs` is
 * the BlockSize the callee normalises by. They are SEPARATE parameters on
 * purpose: `svt_aom_is_screen_content` binds `fn_ptr` to BLOCK_16X16 once and
 * never rebinds it, so its 8x8 pass calls this with fn_bs=BLOCK_16X16 and
 * norm_bs=BLOCK_8X8. Exposing both lets that exact call be driven at tier 1
 * instead of argued about.
 */
uint32_t ref_pre_sby_perpixel_variance(const uint8_t* src, int32_t stride, int32_t fn_bs, int32_t norm_bs) {
    sc_ensure_fn_ptr();
    return svt_av1_get_sby_perpixel_variance(&svt_aom_mefn_ptr[fn_bs], src, stride, (BlockSize)norm_bs);
}

/* Six sc_class bits, in order 0..5, written into `out`. */
static void sc_run_detector(int32_t antialiasing_aware, int32_t fast_detection, uint8_t* y_buf, uint32_t y_origin,
                            uint32_t y_stride, uint32_t width, uint32_t height, int32_t* out) {
    sc_ensure_fn_ptr();
    SequenceControlSet*      scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    PictureParentControlSet* pcs = (PictureParentControlSet*)calloc(1, sizeof(PictureParentControlSet));
    EbPictureBufferDesc*     in  = (EbPictureBufferDesc*)calloc(1, sizeof(EbPictureBufferDesc));

    in->y_buffer = y_buf + y_origin;
    in->y_stride = y_stride;
    in->width    = width;
    in->height   = height;

    scs->fast_aa_aware_screen_detection_mode = (uint8_t)(fast_detection != 0);
    pcs->scs                                 = scs;
    pcs->enhanced_pic                        = in;

    if (antialiasing_aware) {
        svt_aom_is_screen_content_antialiasing_aware(pcs);
    } else {
        svt_aom_is_screen_content(pcs);
    }

    out[0] = pcs->sc_class0;
    out[1] = pcs->sc_class1;
    out[2] = pcs->sc_class2;
    out[3] = pcs->sc_class3;
    out[4] = pcs->sc_class4;
    out[5] = pcs->sc_class5;

    free(in);
    free(pcs);
    free(scs);
}

void ref_pre_is_screen_content_aa(int32_t fast_detection, uint8_t* y_buf, uint32_t y_origin, uint32_t y_stride,
                                  uint32_t width, uint32_t height, int32_t* out) {
    sc_run_detector(1, fast_detection, y_buf, y_origin, y_stride, width, height, out);
}

void ref_pre_is_screen_content(uint8_t* y_buf, uint32_t y_origin, uint32_t y_stride, uint32_t width, uint32_t height,
                               int32_t* out) {
    sc_run_detector(0, 0, y_buf, y_origin, y_stride, width, height, out);
}

/* ------------------------------------------------------------------------
 * src_ops_process.c per-block variance measures. All three are EXPORTED.
 *
 * `svt_aom_get_perpixel_variance` reaches `svt_aom_mefn_ptr[bs].vf`, so it
 * needs the same two-stage init as the screen-content detectors above — RTCD
 * first, then `init_fn_ptr()`. The other two are plain scalar loops with no
 * dispatch, but they go through `sc_ensure_fn_ptr()` too because it is
 * idempotent and one entry condition is easier to keep right than three.
 * ---------------------------------------------------------------------- */

unsigned int svt_aom_get_perpixel_variance(const uint8_t* buf, uint32_t stride, const int block_size);
void         svt_aom_get_mean_and_perpixel_variance(const uint8_t* buf, uint32_t stride, const int block_size,
                                                    uint32_t* perpixel_var, uint32_t* mean);
unsigned int svt_aom_get_perceptual_perpixel_variance(const uint8_t* buf, uint32_t stride, const int block_size);

uint32_t ref_sops_get_perpixel_variance(const uint8_t* buf, uint32_t stride, int32_t block_size) {
    sc_ensure_fn_ptr();
    return svt_aom_get_perpixel_variance(buf, stride, block_size);
}

void ref_sops_get_mean_and_perpixel_variance(const uint8_t* buf, uint32_t stride, int32_t block_size,
                                             uint32_t* perpixel_var, uint32_t* mean) {
    sc_ensure_fn_ptr();
    svt_aom_get_mean_and_perpixel_variance(buf, stride, block_size, perpixel_var, mean);
}

uint32_t ref_sops_get_perceptual_perpixel_variance(const uint8_t* buf, uint32_t stride, int32_t block_size) {
    sc_ensure_fn_ptr();
    return svt_aom_get_perceptual_perpixel_variance(buf, stride, block_size);
}

/* ------------------------------------------------------------------------
 * The remaining pic_analysis_process.c padding entry points.
 *
 * `pad_2b_compressed_input_picture` is `static`, but its ONLY caller is the
 * exported `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions`, which
 * reaches it when the bit depth is above 8 and the descriptor carries
 * `*_buffer_bit_inc`. Driving it that way is tier 1; hand-derived vectors for
 * its eight `pad_right` arms would have been tier 4.
 * ---------------------------------------------------------------------- */

void svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions_16bit(SequenceControlSet*  scs,
                                                                      EbPictureBufferDesc* input_pic);
void svt_aom_pad_picture_to_multiple_of_sb_dimensions(EbPictureBufferDesc* input_padded_pic);
void svt_aom_down_sample_chroma(EbPictureBufferDesc* input_pic, EbPictureBufferDesc* output_pic);

/* 10-bit min-block padding: the planes are uint16_t, so the strides and the
 * origins below are in SAMPLES, and the shim converts once. */
void ref_pre_pad_min_blk_16bit(uint32_t color_format, uint32_t pad_right, uint32_t pad_bottom, uint32_t width,
                               uint32_t height, uint16_t* y_buf, uint32_t y_stride, uint16_t* u_buf, uint32_t u_stride,
                               uint16_t* v_buf, uint32_t v_stride) {
    pre_ensure_rtcd();
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));

    scs->static_config.encoder_bit_depth = EB_TEN_BIT;
    scs->pad_right                       = pad_right;
    scs->pad_bottom                      = pad_bottom;

    pic.color_format = color_format;
    pic.y_buffer     = (EbByte)y_buf;
    pic.y_stride     = y_stride;
    pic.width        = width;
    pic.height       = height;
    pic.u_buffer     = (EbByte)u_buf;
    pic.u_stride     = u_stride;
    pic.v_buffer     = (EbByte)v_buf;
    pic.v_stride     = v_stride;

    svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions_16bit(scs, &pic);
    free(scs);
}

void ref_pre_pad_to_sb(uint8_t* y_buf, uint32_t y_origin, uint32_t y_stride, uint32_t width, uint32_t height,
                       uint32_t border) {
    pre_ensure_rtcd();
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));
    pic.y_buffer = y_buf + y_origin;
    pic.y_stride = y_stride;
    pic.width    = width;
    pic.height   = height;
    pic.border   = border;
    svt_aom_pad_picture_to_multiple_of_sb_dimensions(&pic);
}

void ref_pre_down_sample_chroma(uint32_t in_color_format, uint32_t out_color_format, uint32_t out_width,
                                uint32_t out_height, uint8_t* u_in, uint32_t u_in_stride, uint8_t* v_in,
                                uint32_t v_in_stride, uint8_t* u_out, uint32_t u_out_stride, uint8_t* v_out,
                                uint32_t v_out_stride) {
    pre_ensure_rtcd();
    EbPictureBufferDesc in, out;
    memset(&in, 0, sizeof(in));
    memset(&out, 0, sizeof(out));
    in.color_format = in_color_format;
    in.u_buffer     = u_in;
    in.u_stride     = u_in_stride;
    in.v_buffer     = v_in;
    in.v_stride     = v_in_stride;
    out.color_format = out_color_format;
    out.width        = out_width;
    out.height       = out_height;
    out.u_buffer     = u_out;
    out.u_stride     = u_out_stride;
    out.v_buffer     = v_out;
    out.v_stride     = v_out_stride;
    svt_aom_down_sample_chroma(&in, &out);
}

/*
 * `pad_2b_compressed_input_picture`, reached through its exported caller
 * `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions`.
 *
 * The main planes here are EIGHT-BIT with a BYTE stride, not 16-bit: in SVT's
 * unpacked 10-bit layout `y_buffer` holds the high 8 bits and
 * `y_buffer_bit_inc` the packed low 2, four samples to a byte. The caller pads
 * the main planes through the 8-bit `pad_input_picture` and derives the
 * compressed stride itself as `y_stride / 4`, so only the byte stride is
 * passed and the relation is left to C.
 *
 * (Handing it `uint16_t` planes instead — the shape the `_16bit` sibling
 * takes — makes C walk them at a sample stride interpreted as bytes and
 * SIGBUS. That was this shim's first draft.)
 */
void ref_pre_pad_2b_compressed(uint32_t color_format, uint32_t pad_right, uint32_t pad_bottom, uint32_t width,
                               uint32_t height, uint8_t* y_buf, uint32_t y_stride, uint8_t* u_buf, uint32_t u_stride,
                               uint8_t* v_buf, uint32_t v_stride, uint8_t* y_inc, uint8_t* u_inc, uint8_t* v_inc) {
    pre_ensure_rtcd();
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    EbPictureBufferDesc pic;
    memset(&pic, 0, sizeof(pic));

    scs->static_config.encoder_bit_depth = EB_TEN_BIT;
    scs->pad_right                       = pad_right;
    scs->pad_bottom                      = pad_bottom;

    pic.color_format     = color_format;
    pic.y_buffer         = y_buf;
    pic.y_stride         = y_stride;
    pic.width            = width;
    pic.height           = height;
    pic.u_buffer         = u_buf;
    pic.u_stride         = u_stride;
    pic.v_buffer         = v_buf;
    pic.v_stride         = v_stride;
    pic.y_buffer_bit_inc = y_inc;
    pic.u_buffer_bit_inc = u_inc;
    pic.v_buffer_bit_inc = v_inc;

    svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(scs, &pic);
    free(scs);
}
