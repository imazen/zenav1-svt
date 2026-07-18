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
# Scope: preset 6 (the PD0_LVL_1 fixed-tree path) at bd8 4:2:0. Presets >= 9
# (LVL_5/6 boundary cost) and the odd-true-width DLF floor-chroma case (65x65)
# are documented follow-ups.
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
