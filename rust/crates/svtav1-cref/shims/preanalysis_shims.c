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
