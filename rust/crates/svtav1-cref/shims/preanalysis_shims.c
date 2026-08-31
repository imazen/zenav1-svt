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
