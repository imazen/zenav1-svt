#!/usr/bin/env bash
# Partial-superblock identity gate (task #95 chunk 2): allintra KEY frames whose
# ALIGNED dims are NOT a multiple of 64, so the frame contains incomplete
# (partial) superblocks with spec-5.11.4 partition edges. Each cell BYTE-MATCHES
# real aomenc via `cmp`.
#
# What this exercises (docs/arbitrary-dims-port-map.md):
#   - the SB-extent-padded per-b64 variance source (pd0::compute_b64_variance),
#   - the PD0 forced-SPLIT at both-false corner nodes + off-frame quadrant prune,
#   - the deterministic single edge shape at a PD0-leaf boundary node — HORZ for
#     `!has_rows`, VERT for `!has_cols` — priced with the non-square PD0 block
#     cost (lvl1_block_cost_rect + the tall/wide rect transforms), coded through
#     decide_leaf_rect + encode_partition_av1's binary SPLIT-vs-{H,V} alphabet.
#
# 96x80 is the primary milestone (right edge SPLIT + bottom edge HORZ + corner
# forced-split, all in one frame). Adding a cell here means it BYTE-MATCHES; do
# NOT add a cell that diverges or matches only by coincidence.
#
# ODD TRUE DIMS (task #95 goal 1): odd -w/-h are now byte-comparable — the
# harness feeds CEILING chroma ((w+1)/2) on both sides (identity_run +
# capture_c_trace) and the loop-restoration search runs on the TRUE luma /
# CEILING chroma extent (whole_frame_rect) reading the aligned-strided recon,
# which fixed the odd-height FH lr_type divergence. Odd-WIDTH cells (right-edge
# partial SBs) byte-match robustly across qp; odd full-SB cells (e.g. 63x63)
# exercise the true-dim seq-header size bits + recon crop with no partial SB.
# STILL DIVERGENT (a pre-existing partial-SB PD0 near-tie, NOT odd-specific —
# 8-aligned 64x72 / 72x64 diverge too): the 8-tall bottom partial SB, where a
# straddling 16x16 node's edge-shape(16x8)-vs-SPLIT(2x8x8) RD tips — so
# odd-HEIGHT-with-8-tall-bottom-SB (64x65, 65x65, 65x72, 65x80) is a follow-up.
#
# Scope: preset 6 (the PD0_LVL_1 fixed-tree path) at bd8 4:2:0. Presets >= 9
# (LVL_5/6 boundary cost) are a documented follow-up.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/partialsb.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
# Each cell: "content w h qp preset" — known byte-exact partial-SB frames.
CELLS=(
  "gradient 96 80 32 6"    # milestone: right VERT-vs-SPLIT + bottom HORZ + corner
  "gradient 96 80 20 6"
  "gradient 96 80 55 6"
  "gradient 96 64 32 6"    # width-partial (right edge only)
  "gradient 96 96 32 6"    # width+height partial, 32-aligned (no straddle)
  "gradient 64 80 32 6"    # height-partial (bottom HORZ)
  "gradient 80 96 32 6"
  "gradient 200 120 32 6"  # multi-SB partial
  "gradient 48 48 32 6"    # sub-64 single partial SB
  "gradient 88 56 40 6"
  "gradient 72 72 40 6"
  # STRADDLE cases (coded block extends past the aligned extent). C codes such
  # blocks reading its SB-extent pad; the port sizes the recon + chroma-source
  # buffers to the SB-extent product so straddling writes/reads never OOB (all
  # dims decode under aomdec). These particular cells byte-match — the straddle
  # either loses RD or the padded reads coincide with C's on uniform chroma.
  "gradient 48 56 40 6"
  "gradient 40 40 40 6"
  "gradient 120 120 32 6"
  "gradient 136 136 40 6"
  # ODD TRUE DIMS (task #95 goal 1) — CEILING-chroma harness + LR true-dim
  # search. All cmp-verified byte-identical across qp. Odd width => right-edge
  # partial SBs coded from odd true dims; odd full-SB (63x63) => true-dim seq
  # header size bits + recon crop with no partial SB.
  "gradient 65 64 32 6"    # odd width (a #95 target), aligned 72x64 right-edge
  "gradient 65 64 20 6"
  "gradient 65 64 55 6"
  "gradient 65 63 40 6"    # odd BOTH w+h, aligned 72x64 (right-edge partial)
  "gradient 71 64 20 6"    # odd width, aligned 72x64
  "gradient 73 64 32 6"    # odd width, aligned 80x64
  "gradient 81 64 40 6"    # odd width, aligned 88x64
  "gradient 73 73 32 6"    # odd BOTH, aligned 80x80 partial
  "gradient 63 96 32 6"    # odd width + 32-tall bottom partial SB
  "gradient 63 48 32 6"    # odd width + bottom partial (48-tall single SB)
  "gradient 63 63 32 6"    # odd BOTH, aligned 64x64 (odd header + true crop, no partial SB)
)
for cell in "${CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1)); failed+=("$tag")
  fi
done
rm -rf "$OUT"
echo "partial-SB identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]
