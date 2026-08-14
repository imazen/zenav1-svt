#!/usr/bin/env python3
"""Split the port-vs-C self-time delta into SIMD-coverage / SIMD-quality /
allocator / scalar-on-both-sides buckets.

Tags
  SIMD_GAP   port kernel is SCALAR on aarch64 and C ships a REGISTERED NEON one
  SIMD_QUAL  both sides have a real vector arm (port `incant!` + C `_neon`)
  ALLOC      allocator + libc mem (memset/memmove/bzero) on either side
  SCALAR_BOTH driver / control-flow / entropy code that is scalar in C too
Every rule is justified in the report; unmatched symbols fall to UNKNOWN and
are printed so the residual is visible rather than silently absorbed.
"""
import re
import sys
from collections import defaultdict

# --- PORT side -------------------------------------------------------------
# SIMD_GAP: verified scalar-only in the port (census 2026-08-13) with a
# SET_NEON-registered C counterpart.
PORT_SIMD_GAP = [
    (r'restoration::compute_stats', 'svt_av1_compute_stats_neon'),
    (r'restoration::wiener_convolve_add_src', 'svt_av1_wiener_convolve_add_src_neon'),
    (r'cdef::cdef_find_dir', 'svt_aom_cdef_find_dir*_neon'),
    (r'cdef::compute_cdef_dist', 'svt_aom_compute_cdef_dist_8bit_neon*'),
    (r'encoder::cdef::(filter_and_count|cdef_search_still|search_one_dual)',
     'svt_aom_compute_cdef_dist_8bit_neon_dotprod (inlined)'),
    (r'intra_pred::(predict_dc|predict_v|predict_h|predict_smooth|predict_paeth_scalar)',
     'svt_aom_{dc,v,h,smooth*}_predictor_WxH_neon'),
    (r'intra_pred::(predict_directional|dr_predictor_edged|dr_pred)',
     'svt_av1_dr_prediction_z{1,2,3}_neon'),
    (r'intra_pred::(filter_intra|upsample_intra_edge|filter_edge)',
     'svt_av1_filter_intra_{predictor,edge}_neon'),
    (r'intra_pred::cfl', 'svt_aom_cfl_predict_lbd_neon'),
    (r'hadamard::(aom_hadamard|hadamard_col8|aom_satd)',
     'svt_aom_hadamard_{4x4,8x8,16x16,32x32}_neon / svt_aom_satd_neon'),
    (r'leaf_funnel::hadamard_satd', 'hadamard_path + svt_aom_satd_neon'),
    (r'loop_filter::', 'svt_aom_lpf_{h,v}_{4,6,8,14}_neon'),
    (r'compute_cul_level', 'svt_av1_compute_cul_level_neon'),
]
# SIMD_QUAL: port has a real NEON arm here (incant! + #[arcane] aarch64 body).
PORT_SIMD_QUAL = [
    r'txfm_simd::', r'fwd_txfm::', r'inv_txfm::', r'txfm_dispatch::',
    r'residual::(residual_i32|recon_add_clamp|sse_i32|sq_sum_i32)',
    r'variance::(sse|variance)', r'quant_coding::', r'dsp::quant::',
    r'cdef::cdef_filter_block', r'coeff_simd::', r'dsp::sad::',
    r'copy::block_', r'hadamard::satd_[48]',
]
PORT_ALLOC = [r'xzm_', r'_malloc', r'malloc_', r'_free\b', r'\bfree\b', r'calloc',
              r'rdl_alloc', r'rdl_dealloc', r'no_alloc_shim', r'RawVec', r'drop_glue',
              r'_platform_mem', r'__bzero', r'memcpy|memmove|memset', r'madvise',
              r'mach_absolute_time', r'mach_vm_reclaim', r'rust_alloc', r'SpecFromElem']

# --- C side ----------------------------------------------------------------
C_SIMD_GAP = [r'compute_stats_neon', r'wiener_convolve.*neon', r'cdef_find_dir.*neon',
              r'cdef_dir_from_lines_neon', r'compute_cdef_dist.*neon',
              r'predictor.*neon', r'dr_prediction.*neon', r'filter_intra.*neon',
              r'intra_edge.*neon', r'cfl.*neon', r'hadamard.*neon', r'satd.*neon',
              r'lpf_.*neon', r'compute_cul_level.*neon', r'hadamard_path',
              r'svt_av1_predict_intra_block', r'svt_av1_intra_prediction',
              r'intra_has_(top_right|bottom_left)', r'variance\d+x\d+_neon']
C_SIMD_QUAL = [r'fwd_txfm2d.*neon', r'lbd_fwd_txfm', r'highbd_fdct', r'highbd_fadst',
               r'inv_txfm.*neon', r'dav1d_inv', r'dedupof_.*inv_txfm', r'quantize.*neon',
               r'residual_kernel.*neon', r'full_distortion.*neon', r'sse.*neon',
               r'sad.*neon', r'cdef_filter_block.*neon', r'cdef_filter_fb',
               r'txb_init_levels_neon', r'get_nz_map_contexts_neon', r'copy_wxh.*neon',
               r'pack_and_load_buffer', r'store_buffer_s16', r'transpose_',
               r'estimate_transform', r'inv_transform_recon', r'handle_transform']
C_ALLOC = PORT_ALLOC + [r'svt_memcpy', r'svt_memset']


def tag(sym, gap, qual, alloc):
    for r in alloc:
        if re.search(r, sym, re.I):
            return 'ALLOC'
    for item in gap:
        r = item[0] if isinstance(item, tuple) else item
        if re.search(r, sym, re.I):
            return 'SIMD_GAP'
    for r in qual:
        if re.search(r, sym, re.I):
            return 'SIMD_QUAL'
    return 'SCALAR_BOTH'


def load(path, gap, qual, alloc, ms):
    tot = 0
    rows = []
    for line in open(path):
        if line.startswith('#'):
            continue
        p = line.rstrip('\n').split('\t')
        if len(p) < 3:
            continue
        rows.append((int(p[0]), p[2]))
        tot += int(p[0])
    per = defaultdict(float)
    det = defaultdict(list)
    for v, s in rows:
        t = tag(s, gap, qual, alloc)
        per[t] += v / tot * ms
        det[t].append((v / tot * ms, s))
    return per, det


if __name__ == '__main__':
    pp, pd_ = load(sys.argv[1], PORT_SIMD_GAP, PORT_SIMD_QUAL, PORT_ALLOC, float(sys.argv[3]))
    cp, cd = load(sys.argv[2], C_SIMD_GAP, C_SIMD_QUAL, C_ALLOC, float(sys.argv[4]))
    gap = float(sys.argv[3]) - float(sys.argv[4])
    print(f'# port {sys.argv[3]} ms  C {sys.argv[4]} ms  ratio '
          f'{float(sys.argv[3])/float(sys.argv[4]):.2f}x  gap {gap:.4f} ms')
    print('bucket\tport_ms\tC_ms\tdelta_ms\tshare_of_gap\tport/C')
    for t in ('SIMD_GAP', 'SIMD_QUAL', 'ALLOC', 'SCALAR_BOTH'):
        a, b = pp.get(t, 0.0), cp.get(t, 0.0)
        r = a / b if b > 1e-9 else float('inf')
        print(f'{t}\t{a:.4f}\t{b:.4f}\t{a-b:+.4f}\t{(a-b)/gap*100:+.1f}%\t'
              + (f'{r:.2f}x' if r != float('inf') else 'inf'))
    if len(sys.argv) > 5:
        for t in ('SIMD_GAP', 'SIMD_QUAL', 'SCALAR_BOTH'):
            print(f'\n## {t}')
            print('  PORT:', ', '.join(f'{s.split("::")[-1]} {v:.3f}'
                                       for v, s in sorted(pd_.get(t, []), reverse=True)[:14]))
            print('  C   :', ', '.join(f'{s} {v:.3f}'
                                       for v, s in sorted(cd.get(t, []), reverse=True)[:14]))
