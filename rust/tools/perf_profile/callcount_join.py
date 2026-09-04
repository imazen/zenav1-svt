#!/usr/bin/env python3
"""Join port and C per-function call counts (callcount.py output) into the
C-function -> port-function(s) ratio table the callcount_* records use.

Usage: callcount_join.py <cells_dir> [--cells a,b,c] [--presets 2,6,10]
                         [--tsv out.tsv]

Reads <cells_dir>/cc_{port,c}_<cell>_p<preset>.tsv (written by
callcount_cells.sh) and <cells_dir>/incl_port_<cell>_p<preset>.txt
(callgrind_annotate --inclusive=yes) for the port-side inclusive Ir of each
matched function, then prints one row per (edge, cell, preset):

  edge  cell  preset  c_count  port_count  ratio  port_incl_ir  port_incl_pct  port_fns  c_fns

Every regex below may match SEVERAL symbols on a side (the port has duplicate
transcriptions, C has .constprop/.isra clones and per-ISA kernels): the counts
are SUMMED and every matched name is listed, so a duplicate can never hide
inside a ratio. "ratio" is port/C; "no_c" when C is 0 and the port is not,
"both0" when both are 0.

The edge table is the join the 2026-09-04 records established by C-comment
citation + call-graph position; the regexes are anchored on the demangled
names as rustfilt prints them. Add an edge here when a new one is joined.
"""
import argparse
import os
import re
import sys

# (edge name, C regex, port regex). Regexes are searched (re.search) against
# the FULL demangled name; keep them specific enough not to catch closures of
# unrelated functions but loose enough to catch generic-hash suffixes.
EDGES = [
    # --- positive controls: geometry / per-leaf / MDS0 --------------------------
    ("CONTROL SB ctor (once per 64x64 SB)",
     r"^svt_aom_largest_coding_unit_ctor$", r"pipeline::merge_sb_units$"),
    ("CONTROL leaves: md_encode_block vs evaluate_leaf (md_stage_0 is per CLASS, not per leaf — 1.56x leaves on screen content)",
     r"^md_encode_block$", r"leaf_funnel::evaluate_leaf$"),
    ("CONTROL MDS0 SATD: hadamard_path vs predict::hadamard_satd",
     r"^hadamard_path$", r"leaf_funnel::predict::hadamard_satd$"),
    ("CONTROL hadamard 4x4", r"^svt_aom_hadamard_4x4_(sse2|avx2|c)$", r"hadamard::aom_hadamard_4x4$"),
    ("CONTROL hadamard 8x8", r"^svt_aom_hadamard_8x8_(sse2|avx2|c)$", r"hadamard::aom_hadamard_8x8$"),
    # The port's aom_hadamard_32x32 calls aom_hadamard_16x16 four times; C's
    # svt_aom_hadamard_32x32_avx2 does not go through the exported 16x16 symbol.
    # Compare port_16x16 - 4 * port_32x32 against C's 16x16 by hand (exact on
    # every cell measured 2026-09-04); the raw ratio here reads 1.1-3.5x.
    ("CONTROL hadamard 16x16 (port count INCLUDES 4 per 32x32 — see comment)",
     r"^svt_aom_hadamard_16x16_(sse2|avx2|c)$", r"hadamard::aom_hadamard_16x16$"),
    ("CONTROL hadamard 32x32", r"^svt_aom_hadamard_32x32_(sse2|avx2|c)$", r"hadamard::aom_hadamard_32x32$"),
    ("CONTROL PD0 recursion: pick_partition_pd0 vs Pd0Ctx::pick_q",
     r"^svt_aom_pick_partition_pd0$", r"pd0::Pd0Ctx>::pick_q$"),
    ("CONTROL PD0 quant: quantize_inv_quantize_light vs pd0::tx_quant_core (p2/p6 only; C also takes _light in MDS3 at p10)",
     r"^svt_aom_quantize_inv_quantize_light$", r"pd0::tx_quant_core$"),
    # --- MD stages ---------------------------------------------------------------
    # perform_dct_dct_tx has NO symbol in the -O3 oracle (inlined into
    # full_loop_core), so at p10 this row under-counts C; read the
    # full_loop_core row there (the mds1skip record's "502" was edge-derived).
    ("MDS3 candidates: perform_tx_partitioning(+perform_dct_dct_tx if visible) vs mds3::eval_candidate — at p10 use the full_loop_core row",
     r"^(perform_tx_partitioning|perform_dct_dct_tx)$", r"leaf_funnel::mds3::eval_candidate$"),
    ("MDS1 luma commit: av1_quantize_b_facade_ii vs quant_coding::quantize_b_raster",
     r"^av1_quantize_b_facade_ii(\.isra\.\d+)?$", r"quant_coding::quantize_b_raster$"),
    ("full_loop_core (C MDS1+MDS3 candidates) vs port eval_candidate + quantize_b_raster",
     r"^full_loop_core$", r"(leaf_funnel::mds3::eval_candidate|quant_coding::quantize_b_raster)$"),
    ("run_mds1 (port, per leaf; C: none — must be 0 where enable_skipping_mds1)",
     r"^$", r"leaf_funnel::mds1::run_mds1$"),
    ("nic::stage_mds1_to_mds3 (port, per leaf)",
     r"^$", r"leaf_funnel::nic::stage_mds1_to_mds3$"),
    ("tx_type_search (C; port txt_search is inlined)",
     r"^tx_type_search$", r"leaf_funnel::txt::txt_search$"),
    # --- transform / quantize / rate pipeline ------------------------------------
    ("trial width: estimate_transform vs tx_unit_inner (port count includes screened-out trials)",
     r"^svt_aom_estimate_transform$", r"tx_pipeline::tx_unit_inner$"),
    ("forward transform: estimate_transform vs fwd_txfm2d_dispatch",
     r"^svt_aom_estimate_transform$", r"txfm_dispatch::fwd_txfm2d_dispatch$"),
    ("residual: svt_aom_residual_kernel vs residual_i32",
     r"^svt_aom_residual_kernel$", r"residual::residual_i32$"),
    ("coefficient SATD screen: svt_aom_satd (port SatdScreen is inlined, no symbol)",
     r"^svt_aom_satd_(avx2|c)$", r"^$"),
    ("quantizer dispatch total: quantize_inv_quantize(+_light) vs quantize_{fp,b}_raster",
     r"^svt_aom_quantize_inv_quantize(_light)?$", r"quant_coding::quantize_(fp|b)_raster$"),
    ("quantize_fp: svt_av1_quantize_fp_facade vs quantize_fp_raster",
     r"^svt_av1_quantize_fp_facade$", r"quant_coding::quantize_fp_raster$"),
    ("RDOQ: svt_av1_optimize_b vs quant::optimize_b",
     r"^svt_av1_optimize_b(\.constprop\.\d+)?$", r"quant::optimize_b$"),
    ("fast RDOQ: svt_fast_optimize_b",
     r"^svt_fast_optimize_b(\.constprop\.\d+)?$", r"quant::fast_optimize_b$"),
    ("coeff rate: svt_av1_cost_coeffs_txb vs coeff_rate::cost_coeffs_txb",
     r"^svt_av1_cost_coeffs_txb$", r"coeff_rate::cost_coeffs_txb$"),
    ("coeff rate entry: txb_estimate_coeff_bits vs coeff_rate::ccost_log",
     r"^svt_aom_txb_estimate_coeff_bits$", r"coeff_rate::ccost_log$"),
    ("nz map contexts", r"^svt_av1_get_nz_map_contexts_(avx2|sse2|c)$", r"entropy::coeff_c::get_nz_map_contexts$"),
    ("init levels: txb_init_levels vs coeff_simd::fill_levels",
     r"^svt_av1_txb_init_levels_(avx2|sse4_1|c)$", r"entropy::coeff_simd::fill_levels$"),
    ("eob cost: get_eob_cost vs quant::coeff_cost_eob::<0>",
     r"^get_eob_cost$", r"quant::coeff_cost_eob::<0>$"),
    ("distortion: spatial_full_distortion_kernel_facade vs variance::sse",
     r"^svt_spatial_full_distortion_kernel_facade$", r"variance::sse$"),
    ("inverse transform: inv_transform_recon8bit vs inv_txfm2d_dispatch",
     r"^svt_aom_inv_transform_recon8bit$", r"txfm_dispatch::inv_txfm2d_dispatch$"),
    ("inverse transform (2026-09-04 record's row): lowbd_inv_txfm2d_add_ssse3 vs inv_txfm2d_c_exact_bd",
     r"^svt_av1_lowbd_inv_txfm2d_add_ssse3$", r"inv_txfm::inv_txfm2d_c_exact_bd$"),
    # --- chroma / intra tools ----------------------------------------------------
    ("chroma full loop: svt_aom_full_loop_uv vs chroma::eval_uv (C also calls it per CFL alpha)",
     r"^svt_aom_full_loop_uv$", r"leaf_funnel::chroma::eval_uv$"),
    ("CFL alpha search: av1_cost_calc_cfl (per alpha) vs md_cfl_rd_pick_alpha (per block) — NOT count-comparable",
     r"^av1_cost_calc_cfl(\.constprop\.\d+)?$", r"leaf_funnel::cfl::md_cfl_rd_pick_alpha$"),
    ("CFL predict", r"^svt_cfl_predict_lbd_(avx2|c)$", r"intra_pred::cfl_predict_lbd$"),
    ("CFL luma subsampling", r"^svt_cfl_luma_subsampling_420_lbd_(avx2|c)$", r"intra_pred::cfl_luma_subsampling_420$"),
    ("filter intra predictor", r"^svt_av1_filter_intra_predictor_(sse4_1|avx2|c)$", r"intra_pred::predict_filter_intra$"),
    ("directional predictor, all zones: dr_prediction_z{1,2,3} vs dr_predictor_edged",
     r"^svt_av1_dr_prediction_z[123]_(avx2|sse4_1|c)$", r"intra_pred::dr_predictor_edged$"),
    ("directional z2 kernel", r"^svt_av1_dr_prediction_z2_(avx2|sse4_1|c)$", r"intra_pred::dr_z2_edged_core$"),
    # --- screen-content tools -----------------------------------------------------
    ("palette search", r"^search_palette_luma$", r"palette::search_palette_luma$"),
    ("palette k-means dim1", r"^svt_av1_k_means_dim1_(avx2|c)$", r"palette::k_means_dim1$"),
    ("palette color index ctx", r"^(svt_av1_)?get_palette_color_index_context$", r"palette::palette_color_index_context$"),
    ("intrabc hash search", r"^(svt_av1_intrabc_hash_search|intra_bc_search)$", r"intrabc::"),
    # --- in-loop filters -----------------------------------------------------------
    ("CDEF find_dir: cdef_find_dir_dual (2 blocks/call) + cdef_find_dir vs cdef::cdef_find_dir",
     r"^svt_aom_cdef_find_dir(_dual)?_(avx2|sse4_1|c)$", r"cdef::cdef_find_dir$"),
    ("CDEF filter block", r"^svt_(aom_)?cdef_filter_block(_8xn_16)?_(avx2|sse4_1|c)$", r"cdef::cdef_filter_block$"),
    ("DLF frame", r"^svt_av1_loop_filter_frame$", r"deblock::loop_filter_frame$"),
    ("restoration search", r"^svt_av1_pick_filter_restoration$", r"restoration::search_restoration_still_bd::<u8>$"),
    # --- port-only helpers and the allocator ---------------------------------------
    ("port helper: tx_pipeline::rs_tx_size", r"^$", r"tx_pipeline::rs_tx_size$"),
    ("port helper: coeff_c::tx_size_from_dims", r"^$", r"entropy::coeff_c::tx_size_from_dims$"),
    ("port helper: MdRates::txt_rate", r"^$", r"rate_tables::MdRates>::txt_rate$"),
    ("allocator: malloc", r"^malloc$", r"^malloc$"),
    ("allocator: calloc", r"^calloc$", r"^calloc$"),
    ("allocator: free", r"^free$", r"^free$"),
    ("allocator: malloc+calloc+free", r"^(malloc|calloc|free)$", r"^(malloc|calloc|free)$"),
    ("memset", r"^__memset_avx2_unaligned_erms$", r"^__memset_avx2_unaligned_erms$"),
    ("memcpy", r"^__memcpy_avx_unaligned_erms$", r"^__memcpy_avx_unaligned_erms$"),
]


def read_cc(path):
    rows = []
    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line or "\t" not in line:
                continue
            c, name = line.split("\t", 1)
            rows.append((int(c), name))
    return rows


def read_incl(path):
    """callgrind_annotate --inclusive=yes: `<Ir> (<pct>%)  file:function [...]`.
    Returns {function_name: (ir, pct)} keyed on the symbol only (after the
    last ':' that separates file from function; Rust names contain '::' so
    split on the FIRST ':' followed by a non-':' instead)."""
    out = {}
    total = None
    if not os.path.exists(path):
        return out, total
    pat = re.compile(r"^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s+(\S+?):(.+?)(?:\s+\[.*\])?\s*$")
    with open(path, errors="replace") as f:
        for line in f:
            if total is None:
                m = re.match(r"^\s*([\d,]+)\s+\(100\.0%\)\s+PROGRAM TOTALS", line)
                if m:
                    total = int(m.group(1).replace(",", ""))
                    continue
            m = pat.match(line)
            if not m:
                continue
            ir = int(m.group(1).replace(",", ""))
            pct = float(m.group(2))
            fn = m.group(4).strip()
            if fn not in out:
                out[fn] = (ir, pct)
    return out, total


REC = re.compile(r"'\d+$")


def match(rows, rx):
    if rx == r"^$":
        return 0, []
    r = re.compile(rx)
    tot = 0
    names = []
    for c, name in rows:
        name = REC.sub("", name)
        if r.search(name):
            tot += c
            names.append(f"{name}={c}")
    return tot, names


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--cells", default=None)
    ap.add_argument("--presets", default="2,6,10")
    ap.add_argument("--tsv", default=None)
    a = ap.parse_args()
    presets = a.presets.split(",")
    if a.cells:
        cells = a.cells.split(",")
    else:
        cells = sorted({re.match(r"cc_port_(.+)_p\d+\.tsv", f).group(1)
                        for f in os.listdir(a.dir)
                        if re.match(r"cc_port_(.+)_p\d+\.tsv", f)})
    out = open(a.tsv, "w") if a.tsv else sys.stdout
    print("edge\tcell\tpreset\tc_count\tport_count\tratio\tport_incl_ir\tport_incl_pct\tport_fns\tc_fns", file=out)
    for cell in cells:
        for p in presets:
            pc = os.path.join(a.dir, f"cc_port_{cell}_p{p}.tsv")
            cc = os.path.join(a.dir, f"cc_c_{cell}_p{p}.tsv")
            if not (os.path.exists(pc) and os.path.exists(cc)):
                continue
            prow = read_cc(pc)
            crow = read_cc(cc)
            incl, total = read_incl(os.path.join(a.dir, f"incl_port_{cell}_p{p}.txt"))
            for edge, crx, prx in EDGES:
                cn, cnames = match(crow, crx)
                pn, pnames = match(prow, prx)
                if cn == 0 and pn == 0:
                    ratio = "both0"
                elif cn == 0:
                    ratio = "no_c"
                else:
                    ratio = f"{pn / cn:.3f}"
                # inclusive Ir of the port side: sum over the matched symbols
                ir = 0
                pct = 0.0
                for nm in pnames:
                    fn = nm.rsplit("=", 1)[0]
                    if fn in incl:
                        ir += incl[fn][0]
                        pct += incl[fn][1]
                print(f"{edge}\t{cell}\t{p}\t{cn}\t{pn}\t{ratio}\t{ir}\t{pct:.2f}\t"
                      f"{' + '.join(pnames) or '-'}\t{' + '.join(cnames) or '-'}", file=out)
    if a.tsv:
        out.close()


if __name__ == "__main__":
    main()
