#!/usr/bin/env python3
"""Join the port's and C's INTER-FRAME per-function deltas (inter_delta.py
output) into one C-edge -> port-edge table, per cell.

Usage:
  inter_join.py <delta_dir> --cells gradient_p8,gradient_p6,photo_p6 [--tsv out.tsv]

Reads <delta_dir>/delta_{port,c}_<cell>.tsv (written by inter_delta.py --tsv)
and, for every edge below, SUMS self/inclusive Ir delta and call-count delta
over every symbol the side's regex matches, then prints the port/C ratio on
the metric the edge names (`self` for leaf kernels, `incl` where one side
inlines what the other calls) and the port edge's share of the port's
whole-process inter delta.

The edges are joined by C-comment citation + call-graph position, the way
callcount_join.py's are, NOT by name. Every matched symbol is listed so a
duplicate transcription cannot hide inside a sum. `class` is the reading this
record gives the row:
  per-call     same (or near-same) call count both sides, more Ir per call
  algorithmic  the port does work C does not do on this frame (or vice versa)
  control      a join that must read ~1.00x or the method is broken
  suspect      an Ir row callgrind is known to overstate (rep stosb memset)
  harness      not encoder work (perf_encode synthesises the shifted frame
               in-process; the C driver reads it from the .yuv)
"""
import argparse
import os
import re
import sys

# (edge, metric, class, C regex, port regex)
EDGES = [
    ("CONTROL frame leaves: md_encode_block vs evaluate_leaf", "incl", "control",
     r"^md_encode_block$", r"leaf_funnel::evaluate_leaf$"),
    ("CONTROL RDOQ: svt_av1_optimize_b vs quant::optimize_b", "self", "control",
     r"^svt_av1_optimize_b(\.constprop\.\d+)?$", r"quant::optimize_b::\{closure#0\}$"),
    ("CONTROL(count only) PD0 light quant: quantize_inv_quantize_light vs pd0::tx_quant_core — Ir NOT like-for-like, C's row excludes the transform the port's includes", "incl", "control",
     r"^svt_aom_quantize_inv_quantize_light$", r"pd0::tx_quant_core$"),
    ("CONTROL residual: svt_aom_residual_kernel(8bit_avx2) vs residual_i32", "self", "control",
     r"^svt_residual_kernel8bit_avx2$", r"residual::__arcane_residual_i32_impl_v3$"),
    ("CONTROL 64-pt forward DCT family (parity, not a gap)", "self", "control",
     r"^(av1_fdct64_new_avx2|svt_av1_fwd_txfm2d_64x64_avx2|svt_handle_transform64x64_avx2)$",
     r"txfm_simd::v3::fdct64_x8$"),
    # --- open-loop ME (once per SB) ------------------------------------------
    ("ME whole SB: motion_estimation_b64 (64 calls both sides)", "incl", "per-call",
     r"^svt_aom_motion_estimation_b64$", r"inter_me::b64::motion_estimation_b64$"),
    ("ME: ext_all_sad_calculation_8x8_16x16 (C leaf kernel vs port dispatch that calls block_sad 512x)", "incl", "per-call",
     r"^svt_ext_all_sad_calculation_8x8_16x16_avx2$", r"inter_me::sad::__arcane_ext_all8_dispatch_v3$"),
    ("ME: sad_loop_kernel (C leaf kernel vs port dispatch that calls block_sad per point)", "incl", "per-call",
     r"^svt_sad_loop_kernel_avx2_intrin$", r"inter_me::sad::__arcane_sad_loop_dispatch_v3$"),
    ("ME: nxm_sad_kernel_helper vs ext_sad8 dispatch", "incl", "per-call",
     r"^svt_nxm_sad_kernel_helper_avx2$", r"inter_me::sad::__arcane_ext_sad8_dispatch_v3$"),
    ("ME: block_sad (port SAD leaf, ALL callers) vs C's four SAD kernels", "self", "per-call",
     r"^(svt_ext_all_sad_calculation_8x8_16x16_avx2|svt_sad_loop_kernel_avx2_intrin|svt_nxm_sad_kernel_helper_avx2|svt_aom_sad\d+x\d+_avx2)$",
     r"me_sad::__arcane_block_sad_v3$"),
    ("HOMEGROWN second full-pel search per SB (motion_est.rs; no C counterpart)", "incl", "algorithmic",
     r"^$", r"motion_est::full_pel_search$"),
    ("ME: block_sum_sse (port) vs C variance in ME", "self", "per-call",
     r"^$", r"me_sad::__arcane_block_sum_sse_v3$"),
    # --- MD motion search ------------------------------------------------------
    ("MD subpel: find_best_sub_pixel_tree_pruned", "incl", "per-call",
     r"^svt_av1_find_best_sub_pixel_tree_pruned$", r"md_subpel::find_best_sub_pixel_tree_pruned$"),
    ("MD subpel variance kernel: C sub_pixel_variance{32xh,32x32,64x64} SELF vs port subpel_variance family SELF", "self", "per-call",
     r"^svt_aom_sub_pixel_variance(32xh|32x32|64x64)_avx2$",
     r"subpel_variance::(__arcane_subpel_variance_dispatch_v3|sub_pixel_variance|variance_diff_sse)$"),
    ("MVP: setup_ref_mv_list vs inter_mvp::setup_ref_mv_list_seeded", "incl", "per-call",
     r"^setup_ref_mv_list$", r"inter_mvp::setup_ref_mv_list_seeded$"),
    ("MV rate table: build_nmv_component_cost_table (4 calls both sides)", "self", "per-call",
     r"^build_nmv_component_cost_table$", r"intrabc::build_nmv_component_cost_table$"),
    # --- inter prediction --------------------------------------------------------
    ("MD inter prediction, whole: svt_aom_inter_prediction vs av1_inter_prediction_light_pd1", "incl", "per-call",
     r"^svt_aom_inter_prediction$", r"port_pd_pred::av1_inter_prediction_light_pd1$"),
    ("convolve x (1-D horizontal)", "self", "per-call",
     r"^svt_av1_convolve_x_sr_avx2$", r"port_convolve::convolve_x_sr$"),
    ("convolve y (1-D vertical, incl. C's jnt y)", "self", "per-call",
     r"^(svt_av1_convolve_y_sr_avx2|jnt_convolve_y_6tap_avx2|svt_av1_jnt_convolve_y_avx2)$", r"port_convolve::convolve_y_sr$"),
    ("convolve 2-D (both axes subpel; C 16 calls vs port 240 — routing differs)", "self", "per-call",
     r"^(svt_av1_convolve_2d_sr_avx2|convolve_2d_sr_(hor|ver)_6tap_avx2|jnt_convolve_x_6tap_avx2|svt_av1_jnt_convolve_x_avx2)$",
     r"port_convolve::convolve_2d_sr$"),
    ("convolve copy (C copy kernels vs port dispatch_convolve_8 SELF, where the copy is inlined)", "self", "per-call",
     r"^(svt_av1_convolve_2d_copy_sr_avx2|svt_av1_jnt_convolve_2d_copy_avx2)$", r"port_inter_predictor::dispatch_convolve_8$"),
    ("PD0 inter prediction: inter_pu_prediction_av1_pd0 vs predict_inter_luma_pd0 (320 both)", "incl", "per-call",
     r"^svt_aom_inter_pu_prediction_av1_pd0$", r"inter_pred_arm::predict_inter_luma_pd0$"),
    ("Warped motion candidates: svt_av1_warp_plane (C evaluates; port has no warp arm)", "incl", "algorithmic",
     r"^svt_av1_warp_plane$", r"^$"),
    # --- PD0 / MD stages ---------------------------------------------------------
    ("PD0 whole SB: pick_partition_pd0 vs pd0::pd0_pick_sb_partition* (64->128 both)", "incl", "per-call",
     r"^svt_aom_pick_partition_pd0$", r"pd0::pd0_pick_sb_partition(_video_eval)?$"),
    ("MDS0 distortion, VAR: C variance kernels vs port variance_diff kernel", "self", "per-call",
     r"^svt_aom_(variance32x32|variance64x64|variance16x16|mse16x16)_avx2$",
     r"variance::__arcane_variance_diff_parts_impl_v3$"),
    ("MDS0 distortion, SATD: Hadamard family (C makes 0 Hadamard calls on the inter frame)", "self", "algorithmic",
     r"^(hadamard_path|svt_aom_hadamard_(8x8|16x16|32x32)_(avx2|sse2|c))$",
     r"(hadamard::(aom_hadamard_8x8_core|hadamard_col8|aom_hadamard_16x16|aom_hadamard_32x32|__arcane_aom_hadamard_8x8_impl_v3)|predict::hadamard_satd_into)$"),
    ("MDS0 breadth: fast_loop_core (C, per candidate) vs predict::predict_unit (port, per candidate) — COUNTS are the reading", "incl", "algorithmic",
     r"^fast_loop_core$", r"leaf_funnel::predict::predict_unit$"),
    ("Directional intra, ALL (self Ir): C z1+z2+z3 kernels + clones vs port dr_predictor_edged + z2 core; port wrapper count == C z1+z2+z3 count EXACTLY on the key frame; the calls column double-counts wrapper+kernel on both sides — read counts from the z2-only row", "self", "per-call",
     r"^(svt_av1_dr_prediction_z[123]_avx2|dr_prediction_z[123]_[0-9a-z_]+avx2(\.isra\.\d+)?)$",
     r"intra_pred::(dr_predictor_edged|dr_z[123]_edged_core)$"),
    ("Directional intra z2 only: C z2 kernel vs port dr_z2_edged_core (both 20,288 calls on the gradient p6 key frame; the inter-frame call deltas differ per cell — breadth AND per-call)", "self", "per-call",
     r"^svt_av1_dr_prediction_z2_avx2$", r"intra_pred::dr_z2_edged_core$"),
    ("Smooth predictor (32x32/64x64 C ssse3 vs port predict_smooth)", "self", "per-call",
     r"^svt_aom_smooth_predictor_(32x32|64x64|16x16|8x8|4x4)_ssse3$", r"intra_pred::predict_smooth$"),
    ("Paeth predictor", "self", "per-call",
     r"^svt_aom_paeth_predictor_\d+x\d+_avx2$", r"intra_pred::__arcane_predict_paeth_impl_v3$"),
    ("tx_unit_inner vs full_loop_core (granularity differs; port per txb, C per candidate)", "self", "per-call",
     r"^full_loop_core$", r"tx_pipeline::tx_unit_inner$"),
    # --- frame-level passes ---------------------------------------------------------
    ("Screen-content detector (C: 1 call total, key frame only; port: 2 per frame, every frame)", "incl", "algorithmic",
     r"^svt_aom_is_screen_content_antialiasing_aware$", r"sc_detect::is_screen_content_antialiasing_aware$"),
    ("Wiener LR search on the inter frame (C runs it: wn=5, is_not_last_layer; port does not)", "incl", "algorithmic",
     r"^restoration_seg_search$", r"restoration::search_restoration_still_bd::<u8>$"),
    ("Film-grain estimate (port only, result discarded: pipeline.rs `_grain_params`)", "self", "algorithmic",
     r"^$", r"film_grain::estimate_film_grain$"),
    ("TPL per-SB qp offsets (port only, inter frames)", "self", "algorithmic",
     r"^$", r"rate_control::tpl_sb_qp_offsets$"),
    ("Downsample 2D (2 planes/frame both sides)", "self", "per-call",
     r"^svt_aom_downsample_2d_avx2$", r"port_preanalysis::downsample_2d$"),
    ("PaPlane::refill_from_plane (port padded-plane copy for ME)", "incl", "algorithmic",
     r"^$", r"inter_me_arm::PaPlane>::refill_from_plane$"),
    ("Harness frame synthesis: perf_encode::translate (EXCLUDE — C reads the .yuv)", "self", "harness",
     r"^$", r"^perf_encode::translate$"),
    ("memset (SUSPECT: rep stosb is 1 Ir/byte under callgrind; largest port caller is the allocator's unnamed frame)", "self", "suspect",
     r"^__memset_avx2_unaligned_erms", r"^__memset_avx2_unaligned_erms"),
    ("memcpy", "self", "suspect",
     r"^(__memcpy_avx_unaligned_erms|svt_memcpy_intrin_sse)", r"^__memcpy_avx_unaligned_erms"),
    ("allocator (malloc/calloc/free internals)", "self", "algorithmic",
     r"^(_int_malloc|_int_free|__libc_calloc2|__libc_malloc2|malloc|calloc|free|realloc|_int_realloc|unlink_chunk|malloc_consolidate)",
     r"^(_int_malloc|_int_free|__libc_calloc2|__libc_malloc2|malloc|calloc|free|realloc|_int_realloc|unlink_chunk|malloc_consolidate)"),
]


def read_delta(path):
    rows = {}
    total = None
    with open(path, errors="replace") as f:
        for line in f:
            if line.startswith("# total Ir"):
                m = re.search(r"delta=(-?\d+)", line)
                total = int(m.group(1))
                continue
            if line.startswith("#") or line.startswith("rank\t"):
                continue
            p = line.rstrip("\n").split("\t")
            if len(p) < 13:
                continue
            # callgrind names recursion levels `fn'2`, `fn'3`; fold them into
            # `fn` so a `$`-anchored regex sums every level.
            fn = re.sub(r"'\d+$", "", p[2])
            def i(x):
                return int(x) if x not in ("na", "") else None
            r = rows.setdefault(fn, dict(self_d=0, incl_d=0, c1=0, c2=0, cd=0))
            r["self_d"] += int(p[5]); r["incl_d"] += int(p[9])
            r["c1"] += i(p[10]) or 0; r["c2"] += i(p[11]) or 0
    if total is None:
        sys.exit(f"{path}: no total line")
    return total, rows


def match(rows, rx):
    r = re.compile(rx)
    if rx == r"^$":
        return [], 0, 0, 0, 0
    names = [k for k in rows if r.search(k)]
    s = sum(rows[k]["self_d"] for k in names)
    i = sum(rows[k]["incl_d"] for k in names)
    c1 = sum((rows[k]["c1"] or 0) for k in names)
    c2 = sum((rows[k]["c2"] or 0) for k in names)
    return names, s, i, c1, c2


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("delta_dir")
    ap.add_argument("--cells", required=True)
    ap.add_argument("--tsv")
    a = ap.parse_args()
    out = open(a.tsv, "w") if a.tsv else None
    hdr = ("cell\tedge\tmetric\tclass\tc_self_d\tc_incl_d\tc_calls_n1\tc_calls_n2\t"
           "port_self_d\tport_incl_d\tport_calls_n1\tport_calls_n2\tratio\tport_share_pct\tc_share_pct")
    fns_out = open(a.tsv[:-4] + ".fns.tsv", "w") if a.tsv else None
    fns = {}  # (edge, side) -> set of symbols, union over cells
    if fns_out:
        fns_out.write("# every symbol each edge's regex matched (union over the cells), per side — the guarantee that a duplicate transcription cannot hide inside a sum\n")
        fns_out.write("edge\tside\tsymbols\n")
    if out:
        out.write("# inter_join.py — inter-frame (N=2 minus N=1) per-edge deltas, port vs C\n")
        out.write(hdr + "\n")
    for cell in a.cells.split(","):
        ct, cr = read_delta(os.path.join(a.delta_dir, f"delta_c_{cell}.tsv"))
        pt, pr = read_delta(os.path.join(a.delta_dir, f"delta_port_{cell}.tsv"))
        print(f"\n== {cell}: whole-process inter delta  port {pt:,}  C {ct:,}  ratio {pt / ct:.2f}x")
        print(f"{'ratio':>8} {'port%':>6} {'C%':>6} {'port Ir':>13} {'C Ir':>12} {'calls P':>9} {'calls C':>9}  edge")
        for edge, metric, cls, crx, prx in EDGES:
            cn, cs, ci, cc1, cc2 = match(cr, crx)
            pn, ps, pi, pc1, pc2 = match(pr, prx)
            cv = cs if metric == "self" else ci
            pv = ps if metric == "self" else pi
            ratio = f"{pv / cv:.2f}x" if cv > 0 and pv > 0 else ("port-only" if pv and not cv else ("C-only" if cv and not pv else "both0"))
            psh = 100.0 * pv / pt
            csh = 100.0 * cv / ct
            print(f"{ratio:>8} {psh:>5.1f}% {csh:>5.1f}% {pv:>13,} {cv:>12,} {pc2 - pc1:>9,} {cc2 - cc1:>9,}  [{cls}] {edge}")
            if out:
                out.write(f"{cell}\t{edge}\t{metric}\t{cls}\t{cs}\t{ci}\t{cc1}\t{cc2}\t{ps}\t{pi}\t{pc1}\t{pc2}\t"
                          f"{ratio}\t{psh:.2f}\t{csh:.2f}\n")
                fns.setdefault((edge, "c"), set()).update(cn)
                fns.setdefault((edge, "port"), set()).update(pn)
    if out:
        out.close()
        for edge, _m, _c, _crx, _prx in EDGES:
            for side in ("c", "port"):
                fns_out.write(f"{edge}\t{side}\t{';'.join(sorted(fns.get((edge, side), ())))}\n")
        fns_out.close()


if __name__ == "__main__":
    main()
