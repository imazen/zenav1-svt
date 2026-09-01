#!/usr/bin/env python3
"""Per-function coverage for `Codec/entropy_coding.c`, with the mapping made explicit.

`tools/c_surface_inventory.py` matches C function names against `fn <name>` in
port source and says, correctly, that a name miss is only EVIDENCE of a gap.
On `entropy_coding.c` that evidence is bad: the port renames
(`encode_skip_coeff_av1` -> `write_skip`), inlines (`write_cdef` into
`encode_block_syntax`) and replaces by design (every ctor/dtor). On
2026-08-31 the tool called 132 of 191 rows MISSING; auditing all 132 by hand
found ZERO unported.

This script holds that audit as DATA and re-runs it against the live
inventory, so the claim in `docs/entropy-coding-port-map.md` cannot rot
silently when the C submodule moves or a lane renames a port function.

  tools/entropy_coding_coverage.py

Exit status is NONZERO when any inventory-MISSING row is unclassified — that
is the whole point: a new C function, or a renamed Rust one, shows up as an
unclassified row instead of quietly disappearing into a stale total.
"""
import os
import re
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # rust/
C_FILE = "Codec/entropy_coding.c"

# ---------------------------------------------------------------------------
# The audit, as data. Every entry was confirmed by READING the Rust
# counterpart, not by matching its name.
# ---------------------------------------------------------------------------

PORTED = {
    "av1_get_skip_context": "entropy/context.rs::get_skip_context",
    "encode_skip_coeff_av1": "entropy/context.rs::write_skip",
    "encode_partition_av1": "entropy/context.rs::write_partition{,_edge}",
    "encode_intra_luma_mode_kf_av1": "entropy/context.rs::write_intra_mode_kf",
    "encode_intra_luma_mode_nonkey_av1": "port_entropy_inter/modes.rs::encode_intra_luma_mode_nonkey",
    "encode_intra_chroma_mode_av1": "entropy/context.rs::write_uv_mode",
    "encode_skip_mode_av1": "port_entropy_inter/modes.rs::encode_skip_mode",
    "av1_get_skip_mode_context": "port_entropy_inter/modes.rs::skip_mode_context",
    "av1_write_delta_q_index": "entropy/mv_coding.rs::write_delta_q_index",
    "write_selected_tx_size": "entropy/context.rs::write_tx_depth",
    "get_tx_size_context": "pipeline.rs::EntropyCtx::tx_size_ctx",
    "set_txfm_ctx": "pipeline.rs::EntropyCtx::record_txfm_dims",
    "set_txfm_ctxs": "pipeline.rs::EntropyCtx::record_txfm_dims",
    "get_sqr_tx_size": "vartx.rs::sqr_tx_size_of_dim",
    "txfm_partition_update": "vartx.rs::VarTxWalk::update",
    "av1_code_tx_size": "vartx.rs::write_tx_size_vartx (inter) + pipeline.rs (intra)",
    "code_tx_size": "pipeline.rs (the per-block wrapper)",
    "pack_map_tokens": "entropy/context.rs::write_palette_map_tokens",
    "delta_encode_palette_colors": "entropy/context.rs::write_delta_encoded_colors",
    "svt_aom_get_palette_bsize_ctx": "entropy/context.rs::palette_bsize_ctx",
    "svt_aom_get_palette_mode_ctx": "port_entropy_inter/primitives.rs::palette_mode_ctx",
    "svt_aom_allow_palette": "entropy/context.rs::allow_palette",
    "svt_aom_allow_intrabc": "intrabc.rs",
    "svt_av1_encode_dv": "intrabc.rs",
    "av1_get_mv_joint_diff": "entropy/mv_coding.rs (MvJointType derivation)",
    "av1_write_tx_type": "entropy/coeff_c.rs::write_tx_type_{intra,inter}",
    "av1_write_coeffs_txb_1d": "entropy/coeff_c.rs",
    "av1_encode_coeff_1d": "pipeline.rs + entropy/coeff_c.rs",
    "av1_encode_tx_coef_y": "pipeline.rs + entropy/coeff_c.rs",
    "av1_encode_tx_coef_uv": "pipeline.rs + entropy/coeff_c.rs",
    "ec_update_neighbors": "pipeline.rs::EntropyCtx update methods",
    "svt_aom_write_modes_sb": "pipeline.rs::encode_partition_tree",
    "write_modes_b": "pipeline.rs::encode_block_syntax (intra) + port_entropy_inter/block.rs::write_inter_mode_info (inter)",
    "loop_restoration_write_sb_coeffs": "entropy/lr.rs + restoration.rs",
    "mem_put_varsize": "entropy/obu.rs::put_varsize",
    "svt_aom_wb_write_bit": "entropy/obu.rs::BitWriter::write_bit",
    "svt_aom_wb_write_bit_inlined": "entropy/obu.rs::BitWriter::write_bit",
    "svt_aom_wb_write_literal": "entropy/obu.rs::BitWriter::write_bits",
    "svt_aom_wb_write_literal_inlined": "entropy/obu.rs::BitWriter::write_bits",
    "svt_aom_wb_write_inv_signed_literal": "entropy/obu.rs::write_inv_signed_literal",
    "svt_aom_wb_bytes_written": "entropy/obu.rs::BitWriter::bytes_written",
    "svt_aom_wb_is_byte_aligned": "entropy/obu.rs::BitWriter::bit_len",
    "aom_wb_write_primitive_quniform": "port_entropy_inter/gm.rs::wb_write_primitive_quniform",
    "aom_wb_write_primitive_subexpfin": "port_entropy_inter/gm.rs::wb_write_primitive_subexpfin",
    "aom_wb_write_primitive_refsubexpfin": "port_entropy_inter/gm.rs::wb_write_primitive_refsubexpfin",
    "svt_aom_encode_sps_av1": "entropy/obu.rs::write_sequence_header",
    "svt_aom_encode_td_av1": "entropy/obu.rs::write_temporal_delimiter",
    "svt_aom_write_frame_header_av1": "entropy/obu.rs::write_key_frame_header* / write_inter_frame_header",
    "svt_aom_write_metadata_av1": "port_entropy_inter/metadata.rs::write_metadata_obus",
    "svt_aom_get_kf_y_mode_ctx": "port_entropy_inter/primitives.rs::kf_y_mode_ctx",
    "svt_aom_get_comp_group_idx_context_enc": "port_entropy_inter/modes.rs::comp_group_idx_context",
    "svt_aom_get_comp_index_context_enc": "port_entropy_inter/modes.rs::comp_index_context",
    "svt_aom_get_comp_reference_type_cdf": "port_entropy_inter/refframe.rs::pred_cdf_comp_reference_type",
    "svt_aom_get_comp_reference_type_context_new": "port_entropy_inter/refframe.rs::comp_reference_type_context",
    "svt_aom_get_reference_mode_cdf": "port_entropy_inter/refframe.rs::pred_cdf_reference_mode",
    "svt_aom_get_reference_mode_context_new": "port_entropy_inter/refframe.rs::reference_mode_context",
    "svt_aom_collect_neighbors_ref_counts_new": "port_entropy_inter/refframe.rs::collect_neighbors_ref_counts",
    "svt_aom_get_pred_context_switchable_interp": "port_entropy_inter/interp.rs::pred_context_switchable_interp",
    "av1_is_interp_needed": "port_entropy_inter/interp.rs::is_interp_needed",
    "svt_av1_get_tile_limits": "entropy/obu.rs::TileGrid::resolve",
    "svt_av1_calculate_tile_cols": "entropy/obu.rs::TileGrid::resolve",
    "svt_av1_calculate_tile_rows": "entropy/obu.rs::TileGrid::resolve",
    "svt_aom_set_tile_info": "entropy/obu.rs::TileGrid",
    "svt_av1_reset_loop_restoration": "restoration.rs",
    "svt_av1_update_segmentation_map": "entropy/context.rs (SegmentationMap::update)",
    # inlined into a caller
    "write_cdef": "pipeline.rs::encode_block_syntax (inline)",
    "encode_cdef": "entropy/obu.rs header writers (inline)",
    "encode_loopfilter": "entropy/obu.rs header writers (inline)",
    "encode_quantization": "entropy/obu.rs header writers (inline)",
    "encode_restoration_mode": "entropy/obu.rs header writers (inline)",
    "encode_segmentation": "entropy/obu.rs::write_segmentation_params",
    "write_delta_q": "entropy/obu.rs header writers (inline)",
    "write_profile": "entropy/obu.rs::write_sequence_header_inner (inline)",
    "write_bitdepth": "entropy/obu.rs::write_sequence_header_inner (inline)",
    "write_color_config": "entropy/obu.rs::write_sequence_header_inner (inline)",
    "write_render_size": "entropy/obu.rs header writers (inline)",
    "write_superres_scale": "entropy/obu.rs::SuperresParams::write",
    "write_frame_size": "entropy/obu.rs header writers (inline)",
    "write_bitstream_level": "entropy/obu.rs::compute_seq_level_idx (inline)",
    "set_bitstream_level_tier": "entropy/obu.rs::compute_seq_level_idx + does_level_match",
    "write_tile_info_max_tile": "entropy/obu.rs::write_tile_info",
    "write_tile_group_header": "entropy/tile.rs::write_tile_group",
    "write_frame_header_obu": "entropy/obu.rs::write_obu(FrameHeader, ...)",
    "write_sequence_header_obu": "entropy/obu.rs::write_sequence_header",
    "write_uncompressed_header_obu": "entropy/obu.rs::key_frame_header_bits / write_inter_frame_header",
    "write_uleb_obu_size": "entropy/obu.rs::write_obu (uleb_encode)",
    "block_signals_txsize": "leaf_funnel/tx_geom.rs::block_signals_txsize",
}

# The whole `svt_a{om,v1}_get_pred_{cdf,context}_*` / `get_pred_context_*`
# family is one module; listing 25 near-identical rows by hand would only
# invite a typo.
PRED_CTX_RE = re.compile(r"^(svt_av1_get_pred_context_|svt_aom_get_pred_cdf_|get_pred_context_)")
PRED_CTX_HOME = "port_entropy_inter/refframe.rs"

NOT_TRANSLATABLE = {
    "svt_aom_entropy_coder_ctor": "ctor for a POOLED EntropyCoder; AomWriter owns its buffer",
    "entropy_coder_dctor": "dtor for the same",
    "svt_aom_entropy_tile_info_ctor": "ctor for EntropyTileInfo",
    "entropy_tile_info_dctor": "dtor for the same",
    "svt_aom_bitstream_ctor": "ctor for Bitstream",
    "bitstream_dctor": "dtor for the same",
    "svt_aom_bitstream_reset": "Vec::clear on an owned buffer",
    "svt_aom_bitstream_get_bytes_count": "Vec::len",
    "svt_aom_bitstream_copy": "extend_from_slice",
    "svt_aom_reset_entropy_coder": "re-seeds a pooled coder object the port does not pool",
    "svt_aom_encode_slice_finish": "AomWriter::done is the coder half; the SVT Bitstream flush has no object to flush",
    "tx_size_to_depth": "adapter from TxSize back to a depth; the port stores tx_depth DIRECTLY",
    "get_vartx_max_txsize": "folded into vartx.rs's walk, which starts from the block dims",
}


def inventory_missing():
    """The rows c_surface_inventory.py calls MISSING for entropy_coding.c."""
    with tempfile.TemporaryDirectory() as d:
        tsv = os.path.join(d, "inv.tsv")
        subprocess.run(
            [sys.executable, os.path.join(REPO, "tools", "c_surface_inventory.py"), "--tsv", tsv],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        rows = []
        with open(tsv) as fh:
            next(fh)
            for line in fh:
                path, fn, status = line.rstrip("\n").split("\t")
                if path == C_FILE:
                    rows.append((fn, status))
    return rows


def main():
    rows = inventory_missing()
    if not rows:
        print(f"no rows for {C_FILE} — the C submodule is missing, not the port", file=sys.stderr)
        return 2
    matched = [fn for fn, st in rows if st == "ported"]
    missing = [fn for fn, st in rows if st == "MISSING"]

    ported, infra, unclassified = [], [], []
    for fn in missing:
        if fn in PORTED:
            ported.append((fn, PORTED[fn]))
        elif PRED_CTX_RE.match(fn):
            ported.append((fn, PRED_CTX_HOME))
        elif fn in NOT_TRANSLATABLE:
            infra.append((fn, NOT_TRANSLATABLE[fn]))
        else:
            unclassified.append(fn)

    total = len(rows)
    print(f"{C_FILE}: {total} function definitions")
    print(f"  name-matched by c_surface_inventory.py : {len(matched)}")
    print(f"  name-MISSING, audited here             : {len(missing)}")
    print(f"      ported under another name / inlined: {len(ported)}")
    print(f"      not translatable (SVT plumbing)    : {len(infra)}")
    print(f"      UNCLASSIFIED                       : {len(unclassified)}")
    print()
    print(f"  => ported            : {len(matched) + len(ported)} / {total}")
    print(f"  => replaced by design: {len(infra)} / {total}")
    print(f"  => unported          : {len(unclassified)} / {total}")

    if unclassified:
        print("\nUNCLASSIFIED rows — a new C function, or a port rename this audit")
        print("has not caught up with. Classify each in this script and update")
        print("docs/entropy-coding-port-map.md in the SAME change:")
        for fn in unclassified:
            print(f"  {fn}")
        return 1

    # Stale entries are the other direction of rot: a name this audit claims
    # is missing-but-ported, which the inventory now matches by name anyway.
    stale = [fn for fn in list(PORTED) + list(NOT_TRANSLATABLE) if fn not in missing]
    if stale:
        print("\nNote — classified but no longer reported MISSING (the port now")
        print("matches these by name, so the entry is redundant, not wrong):")
        for fn in sorted(stale):
            print(f"  {fn}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
