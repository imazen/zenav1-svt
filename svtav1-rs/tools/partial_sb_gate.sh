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
# The 8-tall bottom partial SB near-tie is now FIXED (PD0 boundary-node cost:
# rectangular TX-type rate + binary alike split rate) — single-edge partial
# cells (bottom OR right) byte-match at every qp.
#
# PRESETS >= 9 (M9 LPD0 / PD0_LVL_5/6) now byte-match too — two LPD0-only roots
# were fixed: (1) subres forced off on incomplete b64s (was over-costing +
# over-splitting), and (2) one-false boundary nodes force-split (NSQ geom is
# disabled at allintra enc_mode > M6, so the edge shape is not injected). Both
# are byte-neutral for full SBs and for LVL_1 presets 0-6. A representative p9/
# p10/p13 slice is gated in the last block below.
#
# STILL DIVERGENT: aligned-72x72 BOTH-partial cells (65x65 / 65x72 / 65x80) at
# PRESET 6 ONLY hit a separate full-PD1 intra MODE near-tie (V_PRED vs DC at a
# bottom-SB 16x8 leaf) — they byte-match at p9+ (lighter PD1). That p6 near-tie
# is the remaining follow-up.
#
# Scope: bd8 4:2:0. preset 6 (PD0_LVL_1 fixed tree) + presets 9/10/13 (LPD0).
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
  # BOTTOM-EDGE / 8-tall bottom partial SB — unblocked by the PD0 boundary-node
  # cost fix (rect TX-type rate + binary alike split rate). These are the
  # single-edge partial cells the boundary near-tie used to flip; now byte-exact
  # at every qp. Includes the two 8-ALIGNED partial cells (64x72, 72x64) that
  # pinned the bugs (they are not odd-dim — pure partial-SB).
  "gradient 64 65 32 6"    # odd height (#95 target), bottom-edge 8-tall SB
  "gradient 64 65 20 6"
  "gradient 64 65 55 6"
  "gradient 63 65 40 6"    # odd BOTH, aligned 64x72 bottom-partial
  "gradient 64 72 52 6"    # 8-aligned bottom partial (pinned the boundary bugs)
  "gradient 72 64 62 6"    # 8-aligned right partial (pinned the tx-type rate bug)
  # STRADDLE-WIN cells (C keeps a straddling boundary block as a leaf) — the
  # port-map's documented goal-2 divergences (80x88 / 104x88 / 72x88), now
  # byte-exact after the PD0 boundary-node cost fix (they share the partial-SB
  # RD path). Gated at the qps that byte-match; high-qp both-partial hits the
  # separate PD1 mode near-tie (follow-up).
  "gradient 80 88 32 6"
  "gradient 104 88 32 6"
  "gradient 72 88 32 6"
  "gradient 80 104 40 6"   # all-qp match
  "gradient 104 80 48 6"   # all-qp match
  # PRESETS >= 9 (M9 LPD0 / PD0_LVL_5/6) — the higher-LVL boundary path. Two
  # LPD0-only roots (both byte-neutral for full SBs and for LVL_1 presets 0-6):
  #   1. subres forced OFF on an INCOMPLETE b64 (enc_mode_config.c:7326,
  #      `!is_complete_b64`) — the port was applying subres (step 1) on partial
  #      SBs, over-costing the LVL_5 block distortion and over-splitting;
  #   2. one-false boundary nodes are FORCED SPLIT at LPD0 (nsq_geom_level 0 for
  #      allintra enc_mode > M6 => enabled 0 => set_blocks_to_test tot_shapes 0;
  #      the edge shape is NOT injected, so a thin 8-wide/8-tall edge descends
  #      to the fitting 8x8s). LVL_1 (preset <= 6) keeps the injected edge shape.
  # Every partial-SB cell above byte-matches at p9/p10/p13 too; a representative
  # slice is gated here (thin edges, multi-SB, both-partial, straddle, all qp).
  "gradient 96 80 32 9"    # the documented target, p9
  "gradient 96 80 32 10"   # documented target p10 (was over-split)
  "gradient 96 80 32 13"   # documented target p13
  "gradient 96 80 20 10"
  "gradient 96 80 55 13"
  "gradient 88 56 32 9"
  "gradient 72 64 32 10"   # thin 8-wide right edge (forced-split root)
  "gradient 72 64 55 13"
  "gradient 65 64 20 10"   # odd width, thin right edge (diverged all-qp pre-fix)
  "gradient 65 64 40 13"
  "gradient 64 72 40 10"   # thin 8-tall bottom edge
  "gradient 64 65 32 13"   # odd height, thin bottom edge
  "gradient 200 120 32 10" # multi-SB thin 8-wide right edge
  "gradient 200 120 40 13"
  "gradient 65 65 32 10"   # both-partial (diverges at p6 = follow-up 2; matches p9+)
  "gradient 65 65 32 13"
  "gradient 104 80 40 10"  # straddle-win at higher qp
  "gradient 120 120 32 13"
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
