#!/usr/bin/env python3
"""Aggregate demangled self-time tables into comparable functional classes."""
import re
import sys
from collections import defaultdict

# Ordered rules: first match wins. (class, regex) applied to the symbol name.
RULES = [
    # ---- allocator / libc memory ----
    ('ALLOC', r'(xzm_|_malloc|malloc_|_free\b|free\b|calloc|realloc|rdl_alloc|'
              r'no_alloc_shim|RawVec|drop_glue|rdl_dealloc|rust_alloc|SpecFromElem)'),
    ('LIBC_MEM', r'(_platform_mem|__bzero|_platform_bzero|memcpy|memmove|memset|'
                 r'svt_memcpy|svt_aom_memset|copy_wxh|block_copy|pic_copy)'),
    # ---- transforms ----
    ('FWD_TXFM', r'(fdct|fadst|fidentity|fwd_txfm|fwd_dct|fwd_4dim|estimate_transform|'
                 r'lbd_fwd_txfm|highbd_fdct|highbd_fadst|fwd_txfm2d|av1_fwd_txfm|'
                 r'transform_two_d|txfm_simd::.*fwd|pack_and_load_buffer)'),
    ('INV_TXFM', r'(idct|iadst|iidentity|inv_txfm|inv_dct|dav1d_inv|'
                 r'inverse_transform|txfm_simd::.*inv|inv_txfm_horz|inv_txfm_add|inv_transform_recon|identity_op|iidentity)'),
    # ---- quantisation / RDOQ ----
    ('QUANT_RDOQ', r'(quantize|quant_coding|optimize_b|::quant::|build_quantizer|'
                   r'build_quant|eob_cost|coeff_cost_eob|coeff_cost_general|'
                   r'rdoq|dequant)'),
    # ---- entropy: coefficient contexts + coeff writing ----
    ('COEFF_CTX', r'(nz_map|txb_init_levels|fill_levels|br_ctx|get_txb_ctx|'
                  r'coeff_contexts|cost_coeffs_txb|compute_cul_level|levels)'),
    ('COEFF_WRITE', r'(write_coeffs_txb|encode_coeff_1d|write_tx_type|coeff_c::|'
                    r'coeff_simd::|get_eob_cost|tx_size_bits|tx_size_from_dims|code_tx_size|estimate_coefficients_rate|coeff_rate|eob_extra)'),
    ('RANGE_CODER', r'(OdEcEnc|od_ec_|encode_cdf_q15|encode_bool|::normalize|'
                    r'range_coder|av1_cost_symbol|cost_symbol)'),
    ('SYNTAX_WRITE', r'(write_modes|encode_block_syntax|write_chroma_txb|write_uv_mode|'
                     r'write_partition|write_intra_mode|record_block|record_palette|'
                     r'update_partition_ctx|EntropyCtx|context::|write_sb|write_mb)'),
    # ---- prediction ----
    ('INTRA_PRED', r'(predict_dc|predict_v|predict_h|predict_smooth|predict_paeth|'
                   r'intra_pred|intra_prediction|predict_intra|intra_has_|'
                   r'predict_unit|filter_intra|cfl_|dr_prediction|intra_edge)'),
    # ---- distortion / search kernels ----
    ('DISTORTION', r'(hadamard|satd|::sse\b|sse_i32|variance|::sad|sad_|'
                   r'full_distortion|spatial_full_distortion|residual_kernel|'
                   r'residual::|recon_add_clamp|distortion_kernel)'),
    # ---- post filters ----
    ('CDEF', r'(cdef)'),
    ('LOOP_RESTORE', r'(restoration|wiener|compute_stats|sgrproj|selfguided|lr_)'),
    ('DEBLOCK', r'(deblock|loop_filter|lpf_|DeblockGeom)'),
    # ---- mode-decision driver / partition search ----
    ('MD_DRIVER', r'(evaluate_leaf|tx_unit|leaf_funnel|pd0|partition|pick_partition|'
                  r'md_encode_block|full_loop|mode_decision|md_stage|generate_md|'
                  r'init_md_scan|init_xd|init_block_data|set_nics|full_cost|'
                  r'product_full_mode|encode_fixed_tree|funnel|commit_leaf|'
                  r'into_choice|LeafEval|rs_tx_size|chroma_var|chroma_detector|'
                  r'blk_var_map|extract_neighbors|MdRates|txt_rate|md_scan|md_update|inject_intra|inject_|setup_ptree|scale_chroma_bsize|is_lossless|neighbour_arrays|neighbor_array|cand_bf|candidate_buffer|svt_aom_get_blk|blk_geom|update_mi_map|copy_neighbour)'),
    ('PIPELINE', r'(encode_frame|encode_tile|EncodePipeline|kernel_dispatcher|'
                 r'send_picture|encode_once|enc_send|get_packet|resource_|'
                 r'film_grain|nmv_component|rate_control|picture_analysis|'
                 r'picture_decision|packetization|entropy_coding_kernel|'
                 r'svt_av1_enc|encode_slice|encode_sb)'),
    ('SYSCALL', r'(mach_absolute_time|madvise|__sysctl|write_nocancel|__psynch|'
                r'semaphore|pthread|mmap|munmap|vm_)'),
]

COMPILED = [(c, re.compile(r, re.I)) for c, r in RULES]


# Symbols whose ownership is decided by the BINARY they live in, or by a
# verified caller relationship, not by their own name.
def classify(sym, binr=''):
    # xzone allocator internals time-stamp via mach_absolute_time and reclaim
    # traps (verified in the call tree: mach_absolute_time's parent is _xzm_free).
    if binr in ('libsystem_malloc.dylib',):
        return 'ALLOC'
    if sym in ('mach_absolute_time', 'DYLD-STUB$$mach_absolute_time',
               'mach_vm_reclaim_try_cancel', 'madvise',
               'mach_vm_reclaim_update_kernel_accounting_trap',
               'DYLD-STUB$$mach_vm_reclaim_try_cancel') and binr.startswith('libsystem'):
        return 'ALLOC'
    for c, rx in COMPILED:
        if rx.search(sym):
            return c
    return 'OTHER'


def load(path):
    per = defaultdict(int)
    syms = defaultdict(list)
    tot = 0
    for line in open(path):
        if line.startswith('#'):
            continue
        parts = line.rstrip('\n').split('\t')
        if len(parts) < 3:
            continue
        v = int(parts[0])
        sym = parts[2]
        c = classify(sym, parts[1])
        per[c] += v
        syms[c].append((v, sym))
        tot += v
    return per, syms, tot


if __name__ == '__main__':
    pport, sport, tport = load(sys.argv[1])
    pc, sc, tc = load(sys.argv[2])
    port_ms = float(sys.argv[3])
    c_ms = float(sys.argv[4])
    classes = sorted(set(pport) | set(pc),
                     key=lambda k: -(pport.get(k, 0) / tport * port_ms
                                     - pc.get(k, 0) / tc * c_ms))
    gap_ms = port_ms - c_ms
    print(f'# port total {tport} samples = {port_ms:.3f} ms/encode | '
          f'C total {tc} samples = {c_ms:.3f} ms/encode | ratio {port_ms/c_ms:.2f}x')
    print('class\tport_ms\tC_ms\tdelta_ms\tshare_of_gap\tport/C')
    for k in classes:
        pm = pport.get(k, 0) / tport * port_ms
        cm = pc.get(k, 0) / tc * c_ms
        r = (pm / cm) if cm > 1e-9 else float('inf')
        print(f'{k}\t{pm:.4f}\t{cm:.4f}\t{pm-cm:+.4f}\t{(pm-cm)/gap_ms*100:+.1f}%\t'
              + (f'{r:.2f}x' if r != float('inf') else 'inf'))
    if len(sys.argv) > 5:
        print('\n# per-class symbol detail')
        for k in classes:
            print(f'\n## {k}')
            print('  PORT:', ', '.join(f'{s}({v})' for v, s in
                                       sorted(sport.get(k, []), reverse=True)[:12]))
            print('  C   :', ', '.join(f'{s}({v})' for v, s in
                                       sorted(sc.get(k, []), reverse=True)[:12]))
