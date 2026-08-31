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

/* ---- masked-compound / wedge-search primitives ------------------------ */

int      svt_aom_is_masked_compound_type(COMPOUND_TYPE type);
uint64_t svt_av1_wedge_sse_from_residuals_c(const int16_t* r1, const int16_t* d, const uint8_t* m, int N);
int8_t   svt_av1_wedge_sign_from_residuals_c(const int16_t* ds, const uint8_t* m, int N, int64_t limit);
void     svt_av1_wedge_compute_delta_squares_c(int16_t* d, const int16_t* a, const int16_t* b, int N);
uint64_t svt_aom_sum_squares_i16_c(const int16_t* src, uint32_t n);
int64_t  svt_aom_sse_c(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, int width, int height);
int64_t  svt_aom_highbd_sse_c(const uint8_t* a8, int a_stride, const uint8_t* b8, int b_stride, int width, int height);

int ref_is_masked_compound_type(int t) { return svt_aom_is_masked_compound_type((COMPOUND_TYPE)t); }

void ref_subtract_block(int rows, int cols, int16_t* diff, int diff_stride, const uint8_t* src, int src_stride,
                        const uint8_t* pred, int pred_stride) {
    svt_aom_subtract_block_c(rows, cols, diff, diff_stride, src, src_stride, pred, pred_stride);
}

void ref_highbd_subtract_block(int rows, int cols, int16_t* diff, int diff_stride, const uint16_t* src, int src_stride,
                               const uint16_t* pred, int pred_stride, int bd) {
    svt_aom_highbd_subtract_block_c(
        rows, cols, diff, diff_stride, (const uint8_t*)src, src_stride, (const uint8_t*)pred, pred_stride, bd);
}

uint64_t ref_sum_squares_i16(const int16_t* src, uint32_t n) { return svt_aom_sum_squares_i16_c(src, n); }

int64_t ref_sse(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, int w, int h) {
    return svt_aom_sse_c(a, a_stride, b, b_stride, w, h);
}

int64_t ref_highbd_sse(const uint16_t* a, int a_stride, const uint16_t* b, int b_stride, int w, int h) {
    return svt_aom_highbd_sse_c((const uint8_t*)a, a_stride, (const uint8_t*)b, b_stride, w, h);
}

uint64_t ref_wedge_sse_from_residuals(const int16_t* r1, const int16_t* d, const uint8_t* m, int n) {
    return svt_av1_wedge_sse_from_residuals_c(r1, d, m, n);
}

int ref_wedge_sign_from_residuals(const int16_t* ds, const uint8_t* m, int n, int64_t limit) {
    return svt_av1_wedge_sign_from_residuals_c(ds, m, n, limit);
}

void ref_wedge_compute_delta_squares(int16_t* d, const int16_t* a, const int16_t* b, int n) {
    svt_av1_wedge_compute_delta_squares_c(d, a, b, n);
}

void ref_build_compound_diffwtd_mask(uint8_t* mask, int mask_type, const uint8_t* src0, int src0_stride,
                                     const uint8_t* src1, int src1_stride, int h, int w) {
    svt_av1_build_compound_diffwtd_mask_c(
        mask, (DIFFWTD_MASK_TYPE)mask_type, src0, src0_stride, src1, src1_stride, h, w);
}

void ref_build_compound_diffwtd_mask_highbd(uint8_t* mask, int mask_type, const uint16_t* src0, int src0_stride,
                                            const uint16_t* src1, int src1_stride, int h, int w, int bd) {
    svt_av1_build_compound_diffwtd_mask_highbd_c(mask,
                                                 (DIFFWTD_MASK_TYPE)mask_type,
                                                 (const uint8_t*)src0,
                                                 src0_stride,
                                                 (const uint8_t*)src1,
                                                 src1_stride,
                                                 h,
                                                 w,
                                                 bd);
}

void ref_highbd_blend_a64_hmask_16bit(uint16_t* dst, uint32_t dst_stride, const uint16_t* src0, uint32_t src0_stride,
                                      const uint16_t* src1, uint32_t src1_stride, const uint8_t* mask, int w, int h,
                                      int bd) {
    svt_aom_highbd_blend_a64_hmask_16bit_c(dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, w, h, bd);
}

/* ---- wedge mask tables ------------------------------------------------ */

void           svt_av1_init_wedge_masks(void);
int            svt_aom_is_interintra_wedge_used(BlockSize bsize);
int32_t        svt_aom_get_wedge_bits_lookup(BlockSize bsize);
int            svt_aom_get_wedge_params_bits(BlockSize bsize);
const uint8_t* svt_aom_get_contiguous_soft_mask(int wedge_index, int wedge_sign, BlockSize bsize);

/* `svt_av1_init_wedge_masks` fills file-scope tables; it is idempotent (it
   recomputes the same bytes from const inputs), so a racing double-init is
   wasted work, not a wrong answer -- but pthread_once keeps a reader from
   seeing a half-written table. */
static pthread_once_t g_wedge_once = PTHREAD_ONCE_INIT;
/* `svt_av1_init_wedge_masks` builds its tables with bare `svt_memcpy`
 * (inter_prediction.c:2036 et al.) — an RTCD function pointer that is NULL
 * until `svt_aom_setup_common_rtcd_internal` runs. On aarch64 NEON
 * devirtualization makes `svt_memcpy` the concrete `svt_memcpy_neon` and the
 * hazard cannot fire; on x86-64 the call lands at rip=0x0. MEASURED
 * 2026-08-31: `every_wedge_mask_matches_c` SIGSEGVs on x86_64-linux without
 * this and passes on aarch64-darwin. */
static void           init_wedge(void) {
    ensure_inter_pred_rtcd();
    svt_av1_init_wedge_masks();
}
static void           ensure_wedge(void) { pthread_once(&g_wedge_once, init_wedge); }

int ref_is_interintra_wedge_used(int bsize) { return svt_aom_is_interintra_wedge_used((BlockSize)bsize); }
int ref_get_wedge_bits_lookup(int bsize) { return svt_aom_get_wedge_bits_lookup((BlockSize)bsize); }
int ref_get_wedge_params_bits(int bsize) { return svt_aom_get_wedge_params_bits((BlockSize)bsize); }

/* Copy `n` bytes of wedge_params_lookup[bsize].masks[sign][index] out. */
void ref_get_contiguous_soft_mask(int wedge_index, int wedge_sign, int bsize, uint8_t* out, int n) {
    ensure_wedge();
    const uint8_t* m = svt_aom_get_contiguous_soft_mask(wedge_index, wedge_sign, (BlockSize)bsize);
    for (int i = 0; i < n; ++i) out[i] = m[i];
}

/* ---- inter-intra ------------------------------------------------------ */

void init_ii_masks(void);
void svt_aom_combine_interintra(InterIntraMode mode, int8_t use_wedge_interintra, int wedge_index, int wedge_sign,
                                BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred, int compstride,
                                const uint8_t* interpred, int interstride, const uint8_t* intrapred, int intrastride);
void svt_aom_combine_interintra_highbd(InterIntraMode mode, uint8_t use_wedge_interintra, uint8_t wedge_index,
                                       uint8_t wedge_sign, BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred8,
                                       int compstride, const uint8_t* interpred8, int interstride,
                                       const uint8_t* intrapred8, int intrastride, int bd);

/* init_ii_masks and svt_av1_init_wedge_masks both fill file-scope tables from
   const inputs; the blend itself dispatches through RTCD pointers, so all
   three are set up once. */
static pthread_once_t g_ii_once = PTHREAD_ONCE_INIT;
static void           init_ii(void) {
    ensure_inter_pred_rtcd();
    svt_av1_init_wedge_masks();
    init_ii_masks();
}
static void ensure_ii(void) { pthread_once(&g_ii_once, init_ii); }

void ref_combine_interintra(int mode, int use_wedge, int wedge_index, int wedge_sign, int bsize, int plane_bsize,
                            uint8_t* comppred, int compstride, const uint8_t* interpred, int interstride,
                            const uint8_t* intrapred, int intrastride) {
    ensure_ii();
    svt_aom_combine_interintra((InterIntraMode)mode,
                               (int8_t)use_wedge,
                               wedge_index,
                               wedge_sign,
                               (BlockSize)bsize,
                               (BlockSize)plane_bsize,
                               comppred,
                               compstride,
                               interpred,
                               interstride,
                               intrapred,
                               intrastride);
}

void ref_combine_interintra_highbd(int mode, int use_wedge, int wedge_index, int wedge_sign, int bsize,
                                   int plane_bsize, uint16_t* comppred, int compstride, const uint16_t* interpred,
                                   int interstride, const uint16_t* intrapred, int intrastride, int bd) {
    ensure_ii();
    svt_aom_combine_interintra_highbd((InterIntraMode)mode,
                                      (uint8_t)use_wedge,
                                      (uint8_t)wedge_index,
                                      (uint8_t)wedge_sign,
                                      (BlockSize)bsize,
                                      (BlockSize)plane_bsize,
                                      (uint8_t*)comppred,
                                      compstride,
                                      (const uint8_t*)interpred,
                                      interstride,
                                      (const uint8_t*)intrapred,
                                      intrastride,
                                      bd);
}

/* ---- fast RD models --------------------------------------------------- */

void svt_av1_model_rd_from_var_lapndz(int64_t var, uint32_t n_log2, uint32_t qstep, int32_t* rate, int64_t* dist);
void model_rd_from_sse(BlockSize bsize, int16_t quantizer, uint8_t bit_depth, uint64_t sse, uint32_t* rate,
                       uint64_t* dist, uint8_t simple_model_rd_from_var);

void ref_model_rd_from_var_lapndz(int64_t var, uint32_t n_log2, uint32_t qstep, int32_t* rate, int64_t* dist) {
    svt_av1_model_rd_from_var_lapndz(var, n_log2, qstep, rate, dist);
}

void ref_model_rd_from_sse(int bsize, int quantizer, int bit_depth, uint64_t sse, int simple, uint32_t* rate,
                           uint64_t* dist) {
    model_rd_from_sse(
        (BlockSize)bsize, (int16_t)quantizer, (uint8_t)bit_depth, sse, rate, dist, (uint8_t)simple);
}

/* `svt_log2f_safe` is `get_msb(x | 1)` -- an INTEGER msb, not log2f. Exposed so
   the port's claim about it is checked against the header, not assumed. */
int ref_log2f_safe(uint32_t x) { return svt_log2f_safe(x); }
int ref_get_msb(uint32_t x) { return get_msb(x); }

/* ---- OBMC wsrc/mask producer ------------------------------------------ */

#include "enc_inter_prediction.h"
#include "coding_unit.h"

void svt_av1_calc_target_weighted_pred_above_c(uint8_t is16bit, MacroBlockD* xd, int rel_mi_col, uint8_t nb_mi_width,
                                               MbModeInfo* nb_mi, void* fun_ctxt);
void svt_av1_calc_target_weighted_pred_left_c(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height,
                                              MbModeInfo* nb_mi, void* fun_ctxt);
int  svt_av1_skip_u4x4_pred_in_obmc(BlockSize bsize, int dir, int subsampling_x, int subsampling_y);

/* Both accumulators read exactly two things off the MacroBlockD: nothing at
   all in the left case beyond `n4_w`, and `n4_w` in the above case. A
   stack-local MacroBlockD with n4_w set is therefore a complete stand-in --
   no per-call `static`, per ref_shims.c's rule. */
void ref_calc_target_weighted_pred_above(int n4_w, int rel_mi_col, int nb_mi_width, int32_t* mask_buf,
                                         int32_t* wsrc_buf, const uint8_t* tmp, int tmp_stride, int overlap) {
    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.n4_w                                    = (uint8_t)n4_w;
    struct calc_target_weighted_pred_ctxt ctxt = {mask_buf, wsrc_buf, tmp, tmp_stride, overlap};
    svt_av1_calc_target_weighted_pred_above_c(0, &xd, rel_mi_col, (uint8_t)nb_mi_width, NULL, &ctxt);
}

void ref_calc_target_weighted_pred_left(int n4_w, int rel_mi_row, int nb_mi_height, int32_t* mask_buf,
                                        int32_t* wsrc_buf, const uint8_t* tmp, int tmp_stride, int overlap) {
    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.n4_w                                    = (uint8_t)n4_w;
    struct calc_target_weighted_pred_ctxt ctxt = {mask_buf, wsrc_buf, tmp, tmp_stride, overlap};
    svt_av1_calc_target_weighted_pred_left_c(0, &xd, rel_mi_row, (uint8_t)nb_mi_height, NULL, &ctxt);
}

int ref_skip_u4x4_pred_in_obmc(int bsize, int dir, int ssx, int ssy) {
    return svt_av1_skip_u4x4_pred_in_obmc((BlockSize)bsize, dir, ssx, ssy);
}

/* `get_plane_block_size` (common_utils.h:135); -1 stands in for BLOCK_INVALID. */
int ref_get_plane_block_size(int bsize, int ssx, int ssy) {
    BlockSize b = get_plane_block_size((BlockSize)bsize, ssx, ssy);
    return b == BLOCK_INVALID ? -1 : (int)b;
}

/* `svt_av1_get_obmc_mask(overlap)` -- copy the mask out. */
const uint8_t* svt_av1_get_obmc_mask(int length);
void           ref_get_obmc_mask(int overlap, uint8_t* out) {
    const uint8_t* m = svt_av1_get_obmc_mask(overlap);
    for (int i = 0; i < overlap; ++i) out[i] = m[i];
}

/* ---- scaled-reference kernels ----------------------------------------- */

void svt_av1_convolve_2d_scale_c(const uint8_t* src, int src_stride, uint8_t* dst8, int dst8_stride, int w, int h,
                                 const InterpFilterParams* fx, const InterpFilterParams* fy, int subpel_x_qn,
                                 int x_step_qn, int subpel_y_qn, int y_step_qn, ConvolveParams* conv_params);
void svt_av1_highbd_convolve_2d_scale_c(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w,
                                        int h, const InterpFilterParams* fx, const InterpFilterParams* fy,
                                        int subpel_x_qn, int x_step_qn, int subpel_y_qn, int y_step_qn,
                                        ConvolveParams* conv_params, int bd);

void ref_convolve_2d_scale_full(const uint8_t* src, int src_stride, uint8_t* dst8, int dst8_stride, uint16_t* conv_buf,
                                int conv_stride, int w, int h, int filt_x, int fx_size, int filt_y, int fy_size,
                                int subpel_x_qn, int x_step_qn, int subpel_y_qn, int y_step_qn, int bd,
                                int is_compound, int do_average, int use_jnt, int fwd, int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    cp.use_jnt_comp_avg         = use_jnt;
    cp.use_dist_wtd_comp_avg    = use_jnt;
    cp.fwd_offset               = fwd;
    cp.bck_offset               = bck;
    svt_av1_convolve_2d_scale_c(src,
                                src_stride,
                                dst8,
                                dst8_stride,
                                w,
                                h,
                                &fx,
                                &fy,
                                subpel_x_qn,
                                x_step_qn,
                                subpel_y_qn,
                                y_step_qn,
                                &cp);
}

void ref_highbd_convolve_2d_scale_full(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride,
                                       uint16_t* conv_buf, int conv_stride, int w, int h, int filt_x, int fx_size,
                                       int filt_y, int fy_size, int subpel_x_qn, int x_step_qn, int subpel_y_qn,
                                       int y_step_qn, int bd, int is_compound, int do_average, int use_jnt, int fwd,
                                       int bck) {
    const InterpFilterParams fx = pick_params(filt_x, fx_size);
    const InterpFilterParams fy = pick_params(filt_y, fy_size);
    ConvolveParams           cp = get_conv_params_no_round(do_average, conv_buf, conv_stride, is_compound, bd);
    cp.use_jnt_comp_avg         = use_jnt;
    cp.use_dist_wtd_comp_avg    = use_jnt;
    cp.fwd_offset               = fwd;
    cp.bck_offset               = bck;
    svt_av1_highbd_convolve_2d_scale_c(src,
                                       src_stride,
                                       dst,
                                       dst_stride,
                                       w,
                                       h,
                                       &fx,
                                       &fy,
                                       subpel_x_qn,
                                       x_step_qn,
                                       subpel_y_qn,
                                       y_step_qn,
                                       &cp,
                                       bd);
}

/* ---- subpel-param derivation via tf_inter_predictor -------------------- */

#include "sequence_control_set.h"

void tf_inter_predictor(SequenceControlSet* scs, uint8_t* src_ptr, uint8_t* dst_ptr, int16_t pre_y, int16_t pre_x,
                        Mv mv, const struct ScaleFactors* const sf, ConvolveParams* conv_params,
                        InterpFilters interp_filters, uint16_t frame_width, uint16_t frame_height, uint8_t blk_width,
                        uint8_t blk_height, MacroBlockD* av1xd, int32_t src_stride, int32_t dst_stride,
                        uint8_t bit_depth, uint8_t subsamling_shift);

/* `tf_inter_predictor` is the only EXPORTED caller of the `static`
   `compute_subpel_params` that a shim can synthesise the arguments for: it
   reads exactly `scs->super_block_size` off the SequenceControlSet and the
   four `mb_to_*_edge` fields off the MacroBlockD. The SCS is calloc/free per
   call (it is large, and per ref_shims.c's rule "large" is a reason for
   calloc, never for a `static`). */
void ref_tf_inter_predictor(uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int pre_y, int pre_x,
                            int mv_x, int mv_y, int other_w, int other_h, int this_w, int this_h, int super_block_size,
                            int frame_width, int frame_height, int blk_width, int blk_height, int mb_to_left,
                            int mb_to_right, int mb_to_top, int mb_to_bottom, uint32_t interp_filters, int bit_depth,
                            int subsampling_shift) {
    ensure_inter_pred_rtcd();
    SequenceControlSet* scs = (SequenceControlSet*)calloc(1, sizeof(SequenceControlSet));
    scs->super_block_size   = (uint16_t)super_block_size;

    MacroBlockD xd;
    memset(&xd, 0, sizeof(xd));
    xd.mb_to_left_edge   = mb_to_left;
    xd.mb_to_right_edge  = mb_to_right;
    xd.mb_to_top_edge    = mb_to_top;
    xd.mb_to_bottom_edge = mb_to_bottom;

    ScaleFactors sf;
    memset(&sf, 0, sizeof(sf));
    svt_av1_setup_scale_factors_for_frame(&sf, other_w, other_h, this_w, this_h);

    Mv mv;
    mv.x = (int16_t)mv_x;
    mv.y = (int16_t)mv_y;

    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 0, bit_depth);

    tf_inter_predictor(scs,
                       src,
                       dst,
                       (int16_t)pre_y,
                       (int16_t)pre_x,
                       mv,
                       &sf,
                       &cp,
                       interp_filters,
                       (uint16_t)frame_width,
                       (uint16_t)frame_height,
                       (uint8_t)blk_width,
                       (uint8_t)blk_height,
                       &xd,
                       src_stride,
                       dst_stride,
                       (uint8_t)bit_depth,
                       (uint8_t)subsampling_shift);
    free(scs);
}

/* ---- masked-compound blend in the CONV_BUF domain ---------------------- */

void svt_aom_build_masked_compound_no_round(uint8_t* dst, int dst_stride, const CONV_BUF_TYPE* src0, int src0_stride,
                                            const CONV_BUF_TYPE* src1, int src1_stride,
                                            const InterInterCompoundData* const comp_data, uint8_t* seg_mask,
                                            BlockSize bsize, int h, int w, ConvolveParams* conv_params,
                                            uint8_t bit_depth, bool is_16bit);

/* The blend dispatches through svt_aom_{lowbd,highbd}_blend_a64_d16_mask, and
   a WEDGE compound reads svt_aom_get_contiguous_soft_mask, so both RTCD and
   the wedge tables must be up. */
void ref_build_masked_compound_no_round(uint8_t* dst, int dst_stride, const uint16_t* src0, int src0_stride,
                                        const uint16_t* src1, int src1_stride, int comp_type, int wedge_index,
                                        int wedge_sign, int mask_type, uint8_t* seg_mask, int bsize, int h, int w,
                                        int bd, int is_compound, int is_16bit) {
    ensure_inter_pred_rtcd();
    ensure_wedge();
    InterInterCompoundData comp;
    comp.type        = (CompoundType)comp_type;
    comp.wedge_index = (uint8_t)wedge_index;
    comp.wedge_sign  = (uint8_t)wedge_sign;
    comp.mask_type   = (DIFFWTD_MASK_TYPE)mask_type;
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, is_compound, bd);
    svt_aom_build_masked_compound_no_round(dst,
                                           dst_stride,
                                           src0,
                                           src0_stride,
                                           src1,
                                           src1_stride,
                                           &comp,
                                           seg_mask,
                                           (BlockSize)bsize,
                                           h,
                                           w,
                                           &cp,
                                           (uint8_t)bd,
                                           is_16bit != 0);
}

/* Direct (non-RTCD) call to the pure-C d16 mask blends, so a differential can
   say whether a mismatch is the port or C's dispatched SIMD tier. */
void svt_aom_lowbd_blend_a64_d16_mask_c(uint8_t* dst, uint32_t dst_stride, const CONV_BUF_TYPE* src0,
                                        uint32_t src0_stride, const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                        const uint8_t* mask, uint32_t mask_stride, int w, int h, int subw, int subh,
                                        ConvolveParams* conv_params);
void svt_aom_highbd_blend_a64_d16_mask_c(uint8_t* dst_8, uint32_t dst_stride, const CONV_BUF_TYPE* src0,
                                         uint32_t src0_stride, const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                         const uint8_t* mask, uint32_t mask_stride, int w, int h, int subw, int subh,
                                         ConvolveParams* conv_params, const int bd);

void ref_lowbd_blend_a64_d16_mask_c(uint8_t* dst, int dst_stride, const uint16_t* src0, int src0_stride,
                                    const uint16_t* src1, int src1_stride, const uint8_t* mask, int mask_stride, int w,
                                    int h, int subw, int subh, int bd) {
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 1, bd);
    svt_aom_lowbd_blend_a64_d16_mask_c(
        dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, mask_stride, w, h, subw, subh, &cp);
}

void ref_highbd_blend_a64_d16_mask_c(uint16_t* dst, int dst_stride, const uint16_t* src0, int src0_stride,
                                     const uint16_t* src1, int src1_stride, const uint8_t* mask, int mask_stride,
                                     int w, int h, int subw, int subh, int bd) {
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 1, bd);
    svt_aom_highbd_blend_a64_d16_mask_c(
        (uint8_t*)dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, mask_stride, w, h, subw, subh, &cp, bd);
}

/* The RTCD-dispatched d16 mask blend, so a differential can compare C's own
   dispatched tier against C's `_c` kernel on identical inputs. */
extern void (*svt_aom_lowbd_blend_a64_d16_mask)(uint8_t* dst, uint32_t dst_stride, const CONV_BUF_TYPE* src0,
                                                uint32_t src0_stride, const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                const uint8_t* mask, uint32_t mask_stride, int w, int h, int subw,
                                                int subh, ConvolveParams* conv_params);

void ref_lowbd_blend_a64_d16_mask_rtcd(uint8_t* dst, int dst_stride, const uint16_t* src0, int src0_stride,
                                       const uint16_t* src1, int src1_stride, const uint8_t* mask, int mask_stride,
                                       int w, int h, int subw, int subh, int bd) {
    ensure_inter_pred_rtcd();
    ConvolveParams cp = get_conv_params_no_round(0, NULL, 0, 1, bd);
    svt_aom_lowbd_blend_a64_d16_mask(
        dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, mask_stride, w, h, subw, subh, &cp);
}

/* The CompoundType enum's numeric values, so the port's discriminants are
   checked against the header instead of assumed. */
int ref_compound_type_value(int which) {
    switch (which) {
    case 0: return (int)COMPOUND_AVERAGE;
    case 1: return (int)COMPOUND_DISTWTD;
    case 2: return (int)COMPOUND_WEDGE;
    case 3: return (int)COMPOUND_DIFFWTD;
    default: return -1;
    }
}

/* Same for DIFFWTD_MASK_TYPE. */
int ref_diffwtd_mask_type_value(int which) {
    switch (which) {
    case 0: return (int)DIFFWTD_38;
    case 1: return (int)DIFFWTD_38_INV;
    default: return -1;
    }
}

/* ---- wedge search ------------------------------------------------------ */

#include "md_process.h"

int64_t pick_wedge_fixed_sign(PictureControlSet* pcs, struct ModeDecisionContext* ctx, const BlockSize bsize,
                              const int16_t* const residual1, const int16_t* const diff10, const int8_t wedge_sign,
                              int8_t* const best_wedge_index);

/* `pick_wedge_fixed_sign` is exported but takes a ModeDecisionContext. With
   `inter_intra_comp_ctrls.use_rd_model = 0` it reads NOTHING else off it (no
   md_rate_est_ctx, no PictureControlSet), so a zeroed calloc'd context is a
   complete stand-in for that arm. The `use_rd_model = 1` arm needs
   md_rate_est_ctx->wedge_idx_fac_bits and a PCS, and is NOT bound.
   calloc/free per call, per ref_shims.c's no-per-call-static rule. */
int64_t ref_pick_wedge_fixed_sign(int bsize, const int16_t* residual1, const int16_t* diff10, int wedge_sign,
                                  int* best_wedge_index) {
    ensure_inter_pred_rtcd();
    ensure_wedge();
    ModeDecisionContext* ctx = (ModeDecisionContext*)calloc(1, sizeof(ModeDecisionContext));
    ctx->hbd_md                                = 0;
    ctx->inter_intra_comp_ctrls.use_rd_model   = 0;
    int8_t  best = -1;
    int64_t rd   = pick_wedge_fixed_sign(NULL, ctx, (BlockSize)bsize, residual1, diff10, (int8_t)wedge_sign, &best);
    *best_wedge_index = best;
    free(ctx);
    return rd;
}
