/*
 * C shims for the inter-prediction / MC surface
 * (Source/Lib/Codec/inter_prediction.c, enc_inter_prediction.c).
 *
 * Its own translation unit so the wholesale inter-prediction lane never shares
 * a shim file with the C2 (inter MVP) or C3 (inter ME) lanes.
 *
 * RULE, inherited from ref_shims.c: A SHIM MUST NOT KEEP PER-CALL STATE IN A
 * `static`. cargo runs a test binary's tests on several threads, so a `static`
 * scratch buffer races and fails as an occasional wrong NUMBER. Every scratch
 * here is a local (the CONV_BUF intermediates are caller-owned and arrive by
 * pointer).
 *
 * The convolve kernels take `const InterpFilterParams*` and `ConvolveParams*`
 * by pointer; rather than mirror those structs in Rust, each shim builds them
 * from scalars with the same header helpers the encoder uses
 * (`av1_get_interp_filter_params_with_block_size`, `get_conv_params_no_round`).
 */
#include <stddef.h>
#include <stdint.h>
#include <pthread.h>
#include <string.h>

#include "convolve.h"
#include "definitions.h"
#include "inter_prediction.h"

/* ---- filter-parameter selection -------------------------------------- */

/* `av1_get_interp_filter_params_with_block_size` (inter_prediction.h:128).
   `size` is the block width for the x filter and the height for the y one. */
static InterpFilterParams pick_params(int filt, int size) {
    return av1_get_interp_filter_params_with_block_size((InterpFilter)filt, size);
}

/* Copy one 8-tap phase of the params the encoder would select, so the Rust
   table + selection rule can be gated against the C tables themselves. */
void ref_interp_filter_kernel(int filt, int size, int subpel, int16_t out[8]) {
    const InterpFilterParams p = pick_params(filt, size);
    const int16_t*           k = av1_get_interp_filter_subpel_kernel(p, subpel);
    for (int i = 0; i < 8; ++i) out[i] = k[i];
}

int ref_interp_filter_taps(int filt, int size) { return pick_params(filt, size).taps; }

/* Expose `get_conv_params_no_round`'s round_0 / round_1 derivation. */
void ref_get_conv_params_no_round(int do_average, int dst_stride, int is_compound, int bd, int out[2]) {
    ConvolveParams cp = get_conv_params_no_round(do_average, NULL, dst_stride, is_compound, bd);
    out[0]            = cp.round_0;
    out[1]            = cp.round_1;
}

/* ---- single-prediction (sr) kernels ---------------------------------- */

void ref_convolve_2d_sr(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w, int32_t h,
                        int filt_x, int fx_size, int filt_y, int fy_size, int32_t subpel_x_q4, int32_t subpel_y_q4,
                        int bd) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_convolve_2d_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fy, subpel_x_q4, subpel_y_q4, &cp);
}

void ref_convolve_x_sr(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w, int32_t h,
                       int filt_x, int fx_size, int32_t subpel_x_q4, int bd) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_x, fx_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_convolve_x_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fy, subpel_x_q4, 0, &cp);
}

void ref_convolve_y_sr(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w, int32_t h,
                       int filt_y, int fy_size, int32_t subpel_y_q4, int bd) {
    const InterpFilterParams fx = pick_params(filt_y, fy_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_convolve_y_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fy, 0, subpel_y_q4, &cp);
}

void ref_convolve_2d_copy_sr(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                             int32_t h, int bd) {
    const InterpFilterParams fx = pick_params(0, 8);
    const InterpFilterParams fy = pick_params(0, 8);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_convolve_2d_copy_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fy, 0, 0, &cp);
}

/* ---- compound (jnt) kernels ------------------------------------------ */

static ConvolveParams jnt_params(uint16_t* conv_buf, int conv_stride, int bd, int do_average, int use_jnt_comp_avg,
                                 int fwd_offset, int bck_offset) {
    ConvolveParams cp        = get_conv_params_no_round(do_average, conv_buf, conv_stride, 1, bd);
    cp.use_jnt_comp_avg      = use_jnt_comp_avg;
    cp.use_dist_wtd_comp_avg = use_jnt_comp_avg;
    cp.fwd_offset            = fwd_offset;
    cp.bck_offset            = bck_offset;
    return cp;
}

void ref_jnt_convolve_2d(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, uint16_t* conv_buf,
                         int conv_stride, int32_t w, int32_t h, int filt_x, int fx_size, int filt_y, int fy_size,
                         int32_t subpel_x_q4, int32_t subpel_y_q4, int bd, int do_average, int use_jnt, int fwd,
                         int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_jnt_convolve_2d_c(src, src_stride, dst8, dst8_stride, w, h, &fx, &fy, subpel_x_q4, subpel_y_q4, &cp);
}

void ref_jnt_convolve_x(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, uint16_t* conv_buf,
                        int conv_stride, int32_t w, int32_t h, int filt_x, int fx_size, int32_t subpel_x_q4, int bd,
                        int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_jnt_convolve_x_c(src, src_stride, dst8, dst8_stride, w, h, &fx, &fx, subpel_x_q4, 0, &cp);
}

void ref_jnt_convolve_y(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, uint16_t* conv_buf,
                        int conv_stride, int32_t w, int32_t h, int filt_y, int fy_size, int32_t subpel_y_q4, int bd,
                        int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_jnt_convolve_y_c(src, src_stride, dst8, dst8_stride, w, h, &fy, &fy, 0, subpel_y_q4, &cp);
}

void ref_jnt_convolve_2d_copy(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride,
                              uint16_t* conv_buf, int conv_stride, int32_t w, int32_t h, int bd, int do_average,
                              int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(0, 8);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_jnt_convolve_2d_copy_c(src, src_stride, dst8, dst8_stride, w, h, &fx, &fx, 0, 0, &cp);
}

/* ---- scale factors ---------------------------------------------------- */

/* `svt_av1_setup_scale_factors_for_frame` (inter_prediction.c:201) writes four
   ints plus two function pointers; only the ints are observable from Rust. */
void ref_setup_scale_factors_for_frame(int other_w, int other_h, int this_w, int this_h, int out[4]) {
    ScaleFactors sf;
    memset(&sf, 0, sizeof(sf));
    /* x_step_q4 / y_step_q4 are left untouched on the invalid-size early
       return, so seed them with a sentinel the caller can recognise. */
    sf.x_step_q4 = -1;
    sf.y_step_q4 = -1;
    svt_av1_setup_scale_factors_for_frame(&sf, other_w, other_h, this_w, this_h);
    out[0] = sf.x_scale_fp;
    out[1] = sf.y_scale_fp;
    out[2] = sf.x_step_q4;
    out[3] = sf.y_step_q4;
}

/* `av1_is_scaled(sf)` — the predicate `has_scale`'s dispatch rests on. */
int ref_av1_is_scaled(int other_w, int other_h, int this_w, int this_h) {
    ScaleFactors sf;
    memset(&sf, 0, sizeof(sf));
    svt_av1_setup_scale_factors_for_frame(&sf, other_w, other_h, this_w, this_h);
    return av1_is_scaled(&sf);
}

/* ---- compound weight assignment --------------------------------------- */

/* `svt_av1_dist_wtd_comp_weight_assign` (inter_prediction.c:290). The
   SeqHeader is stack-local (per the no-static rule) and carries only the
   order-hint fields `svt_aom_get_relative_dist_enc` reads. */
void ref_dist_wtd_comp_weight_assign(int enable_order_hint, int order_hint_bits, int cur_frame_index,
                                     int bck_frame_index, int fwd_frame_index, int compound_idx, int order_idx,
                                     int is_compound, int out[3]) {
    SeqHeader sh;
    memset(&sh, 0, sizeof(sh));
    sh.order_hint_info.enable_order_hint = (uint8_t)enable_order_hint;
    sh.order_hint_info.order_hint_bits   = (uint8_t)order_hint_bits;
    int fwd = -1, bck = -1, use = -1;
    svt_av1_dist_wtd_comp_weight_assign(
        &sh, cur_frame_index, bck_frame_index, fwd_frame_index, compound_idx, order_idx, &fwd, &bck, &use, is_compound);
    out[0] = fwd;
    out[1] = bck;
    out[2] = use;
}

int ref_get_relative_dist_enc(int enable_order_hint, int order_hint_bits, int ref_hint, int order_hint) {
    SeqHeader sh;
    memset(&sh, 0, sizeof(sh));
    sh.order_hint_info.enable_order_hint = (uint8_t)enable_order_hint;
    sh.order_hint_info.order_hint_bits   = (uint8_t)order_hint_bits;
    return svt_aom_get_relative_dist_enc(&sh, ref_hint, order_hint);
}

/* ---- 10/12-bit (highbd) kernels --------------------------------------- */

void ref_highbd_convolve_2d_sr(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride, int32_t w,
                               int32_t h, int filt_x, int fx_size, int filt_y, int fy_size, int32_t subpel_x_q4,
                               int32_t subpel_y_q4, int bd) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_highbd_convolve_2d_sr_c(
        src, src_stride, dst, dst_stride, w, h, &fx, &fy, subpel_x_q4, subpel_y_q4, &cp, bd);
}

void ref_highbd_convolve_x_sr(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride, int32_t w,
                              int32_t h, int filt_x, int fx_size, int32_t subpel_x_q4, int bd) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_highbd_convolve_x_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fx, subpel_x_q4, 0, &cp, bd);
}

void ref_highbd_convolve_y_sr(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride, int32_t w,
                              int32_t h, int filt_y, int fy_size, int32_t subpel_y_q4, int bd) {
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_highbd_convolve_y_sr_c(src, src_stride, dst, dst_stride, w, h, &fy, &fy, 0, subpel_y_q4, &cp, bd);
}

void ref_highbd_convolve_2d_copy_sr(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                    int32_t w, int32_t h, int bd) {
    const InterpFilterParams fx = pick_params(0, 8);
    ConvolveParams           cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    svt_av1_highbd_convolve_2d_copy_sr_c(src, src_stride, dst, dst_stride, w, h, &fx, &fx, 0, 0, &cp, bd);
}

void ref_highbd_jnt_convolve_2d(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                                uint16_t* conv_buf, int conv_stride, int32_t w, int32_t h, int filt_x, int fx_size,
                                int filt_y, int fy_size, int32_t subpel_x_q4, int32_t subpel_y_q4, int bd,
                                int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_highbd_jnt_convolve_2d_c(
        src, src_stride, dst16, dst16_stride, w, h, &fx, &fy, subpel_x_q4, subpel_y_q4, &cp, bd);
}

void ref_highbd_jnt_convolve_x(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                               uint16_t* conv_buf, int conv_stride, int32_t w, int32_t h, int filt_x, int fx_size,
                               int32_t subpel_x_q4, int bd, int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_highbd_jnt_convolve_x_c(src, src_stride, dst16, dst16_stride, w, h, &fx, &fx, subpel_x_q4, 0, &cp, bd);
}

void ref_highbd_jnt_convolve_y(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                               uint16_t* conv_buf, int conv_stride, int32_t w, int32_t h, int filt_y, int fy_size,
                               int32_t subpel_y_q4, int bd, int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_highbd_jnt_convolve_y_c(src, src_stride, dst16, dst16_stride, w, h, &fy, &fy, 0, subpel_y_q4, &cp, bd);
}

void ref_highbd_jnt_convolve_2d_copy(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                                     uint16_t* conv_buf, int conv_stride, int32_t w, int32_t h, int bd, int do_average,
                                     int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(0, 8);
    ConvolveParams           cp = jnt_params(conv_buf, conv_stride, bd, do_average, use_jnt, fwd, bck);
    svt_av1_highbd_jnt_convolve_2d_copy_c(src, src_stride, dst16, dst16_stride, w, h, &fx, &fx, 0, 0, &cp, bd);
}

/* ---- MC dispatchers ---------------------------------------------------- */

/* The `svt_aom_convolve` / `svt_aom_convolveHbd` tables are function-pointer
   arrays filled by `svt_aom_asm_set_convolve_*_asm_table()` from the RTCD
   pointers, so RTCD must run first. Both are idempotent one-shot inits (the
   same CPU flags land the same pointers), which is why the ready flag may stay
   `static` under the no-per-call-state rule. */
uint64_t svt_aom_get_cpu_flags_to_use(void);
void     svt_aom_setup_common_rtcd_internal(uint64_t flags);
void     svt_aom_setup_rtcd_internal(uint64_t flags);
void     svt_aom_asm_set_convolve_asm_table(void);
void     svt_aom_asm_set_convolve_hbd_asm_table(void);

/* MEASURED 2026-08-31: the usual `static int ready` idiom is NOT safe here,
   unlike in the other shims. Those call C functions directly; these read a
   function-pointer TABLE. A second thread that sees `ready == 0` re-runs
   `svt_aom_asm_set_convolve_asm_table()`, which copies the RTCD pointer
   variables into the table -- and if the first thread has not finished
   `svt_aom_setup_common_rtcd_internal` those variables are still NULL, so the
   table gets NULLs installed over working entries and the next call
   segfaults. The whole c_parity_port_inter_predictor binary SIGSEGV'd on every
   multi-threaded run and passed under `--test-threads=1`, which is exactly the
   shape of the shim-state race ref_shims.c's header warns about. `pthread_once`
   makes it a true one-shot; there is still no per-call state. */
static pthread_once_t g_inter_pred_rtcd_once = PTHREAD_ONCE_INIT;
static void           init_inter_pred_rtcd(void) {
    svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    svt_aom_setup_rtcd_internal(svt_aom_get_cpu_flags_to_use());
    svt_aom_asm_set_convolve_asm_table();
    svt_aom_asm_set_convolve_hbd_asm_table();
}
static void ensure_inter_pred_rtcd(void) { pthread_once(&g_inter_pred_rtcd_once, init_inter_pred_rtcd); }

/* Report which kernel the RTCD dispatch actually installed, so a test can
   record whether it compared against the C or the SIMD variant. Returns 1 when
   every entry of both tables is the plain `_c` kernel. */
int ref_convolve_tables_are_pure_c(void) {
    ensure_inter_pred_rtcd();
    return svt_aom_convolve[0][0][0] == svt_av1_convolve_2d_copy_sr_c &&
        svt_aom_convolve[0][0][1] == svt_av1_jnt_convolve_2d_copy_c &&
        svt_aom_convolve[0][1][0] == svt_av1_convolve_y_sr_c && svt_aom_convolve[0][1][1] == svt_av1_jnt_convolve_y_c &&
        svt_aom_convolve[1][0][0] == svt_av1_convolve_x_sr_c && svt_aom_convolve[1][0][1] == svt_av1_jnt_convolve_x_c &&
        svt_aom_convolve[1][1][0] == svt_av1_convolve_2d_sr_c && svt_aom_convolve[1][1][1] == svt_av1_jnt_convolve_2d_c;
}

void ref_inter_predictor_pd0(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                             int32_t h, int32_t xs, int32_t ys, int32_t subpel_x, int32_t subpel_y,
                             uint16_t* conv_buf, int conv_stride, int is_compound, int do_average, int bd) {
    ensure_inter_pred_rtcd();
    SubpelParams sp = {.xs = xs, .ys = ys, .subpel_x = subpel_x, .subpel_y = subpel_y};
    ConvolveParams cp = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    svt_inter_predictor_pd0(src, src_stride, dst, dst_stride, w, h, &sp, &cp);
}

void ref_inter_predictor(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t xs,
                         int32_t ys, int32_t subpel_x, int32_t subpel_y, int other_w, int other_h, int this_w,
                         int this_h, int32_t w, int32_t h, uint16_t* conv_buf, int conv_stride, int is_compound,
                         int do_average, int use_jnt, int fwd, int bck, uint32_t interp_filters, int is_intrabc,
                         int bd) {
    ensure_inter_pred_rtcd();
    SubpelParams sp = {.xs = xs, .ys = ys, .subpel_x = subpel_x, .subpel_y = subpel_y};
    ScaleFactors sf;
    memset(&sf, 0, sizeof(sf));
    svt_av1_setup_scale_factors_for_frame(&sf, other_w, other_h, this_w, this_h);
    ConvolveParams cp        = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    cp.use_jnt_comp_avg      = use_jnt;
    cp.use_dist_wtd_comp_avg = use_jnt;
    cp.fwd_offset            = fwd;
    cp.bck_offset            = bck;
    svt_inter_predictor(src, src_stride, dst, dst_stride, &sp, &sf, w, h, &cp, interp_filters, is_intrabc);
}

void ref_highbd_inter_predictor(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride, int32_t xs,
                                int32_t ys, int32_t subpel_x, int32_t subpel_y, int other_w, int other_h, int this_w,
                                int this_h, int32_t w, int32_t h, uint16_t* conv_buf, int conv_stride, int is_compound,
                                int do_average, int use_jnt, int fwd, int bck, uint32_t interp_filters, int is_intrabc,
                                int bd) {
    ensure_inter_pred_rtcd();
    SubpelParams sp = {.xs = xs, .ys = ys, .subpel_x = subpel_x, .subpel_y = subpel_y};
    ScaleFactors sf;
    memset(&sf, 0, sizeof(sf));
    svt_av1_setup_scale_factors_for_frame(&sf, other_w, other_h, this_w, this_h);
    ConvolveParams cp        = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    cp.use_jnt_comp_avg      = use_jnt;
    cp.use_dist_wtd_comp_avg = use_jnt;
    cp.fwd_offset            = fwd;
    cp.bck_offset            = bck;
    svt_highbd_inter_predictor(src, src_stride, dst, dst_stride, &sp, &sf, w, h, &cp, interp_filters, is_intrabc, bd);
}

/* 8-bit arm of `svt_inter_predictor_light_pd1` only. The `bd > 8` arm packs
   `src` + `src_2b` into a 10-bit scratch via `svt_aom_pack_block`; this port
   carries plain u16 planes by design (bd10.rs), so that arm is deliberately
   out of scope and is not bound here. */
void ref_inter_predictor_light_pd1_8bit(uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                        int32_t h, uint32_t interp_filters, int32_t xs, int32_t ys, int32_t subpel_x,
                                        int32_t subpel_y, uint16_t* conv_buf, int conv_stride, int is_compound,
                                        int do_average, int use_jnt, int fwd, int bck) {
    ensure_inter_pred_rtcd();
    SubpelParams sp = {.xs = xs, .ys = ys, .subpel_x = subpel_x, .subpel_y = subpel_y};
    ConvolveParams cp        = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, 8);
    cp.use_jnt_comp_avg      = use_jnt;
    cp.use_dist_wtd_comp_avg = use_jnt;
    cp.fwd_offset            = fwd;
    cp.bck_offset            = bck;
    svt_inter_predictor_light_pd1(src, NULL, src_stride, dst, dst_stride, w, h, interp_filters, &sp, &cp, 8);
}

/* `convolve_2d_for_intrabc` (inter_prediction.c:1194) and its highbd twin —
   both exported, both non-static despite the un-prefixed names. */
void ref_convolve_2d_for_intrabc(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                                 int subpel_x_q4, int subpel_y_q4, int bd) {
    /* convolve_2d_for_intrabc dispatches through the RTCD pointers
       svt_av1_convolve_{2d,x,y}_sr, which are NULL until RTCD runs -- calling
       it without this segfaults. */
    ensure_inter_pred_rtcd();
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    convolve_2d_for_intrabc(src, src_stride, dst, dst_stride, w, h, subpel_x_q4, subpel_y_q4, &cp);
}

void ref_highbd_convolve_2d_for_intrabc(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w,
                                        int h, int subpel_x_q4, int subpel_y_q4, int bd) {
    ensure_inter_pred_rtcd();
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 0, bd);
    highbd_convolve_2d_for_intrabc(src, src_stride, dst, dst_stride, w, h, subpel_x_q4, subpel_y_q4, &cp, bd);
}

/* `av1_get_convolve_filter_params` (inter_prediction.h:139) — which filter
   index each axis extracts from a packed `InterpFilters`. TRAP: the X filter
   lives in the HIGH 16 bits and Y in the LOW, because
   `av1_extract_interp_filter(filters, x_filter)` shifts by 16 when its second
   argument is TRUTHY, and the caller passes 1 for X / 0 for Y. */
void ref_get_convolve_filter_params(uint32_t interp_filters, int w, int h, int out[2]) {
    InterpFilterParams px, py;
    av1_get_convolve_filter_params(interp_filters, &px, &py, w, h);
    out[0] = px.interp_filter;
    out[1] = py.interp_filter;
}

uint32_t ref_make_interp_filters(int y_filter, int x_filter) {
    return av1_make_interp_filters((InterpFilter)y_filter, (InterpFilter)x_filter);
}
