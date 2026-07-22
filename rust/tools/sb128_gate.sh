#!/usr/bin/env bash
# SB128 (128x128 superblock) identity gate — task #91.
#
# ============================================================ THE GEOMETRY
# There is NO `super_block_size` field in EbSvtAv1EncConfiguration and no
# `--sb-size` CLI option in SvtAv1EncApp. C DERIVES the superblock size in
# `Globals/enc_handle.c:4071-4111` and the port replays that same rule in
# `sb128_geom::derive_super_block_size`, so both encoders agree with no
# harness flag at all. Two clauses decide every cell:
#
#   1. AREA. `input_resolution == INPUT_SIZE_240p_RANGE` forces 64
#      UNCONDITIONALLY, and that bucket is "aligned luma area <
#      INPUT_SIZE_240p_TH = 0x28500 = 165,120" (Codec/definitions.h:1834,
#      classified on the 8-ALIGNED dims — enc_handle.c:3920 folds the pad in
#      before enc_handle.c:3992 derives the bucket).
#   2. PRESET. In the allintra branch only `enc_mode <= ENC_M1` selects 128.
#
# MEASURED on the real encoder (`Bin/Release/SvtAv1EncApp`, SH bit read back
# with tools/sb128_seqhdr.py):
#      512x384 (196,608 px) preset 0 -> 1     preset 1 -> 1
#      512x384                preset 2 -> 0     preset 3 -> 0
#      256x256  (65,536 px)   preset 0 -> 0    <-- every legacy cell
#      512x320 (163,840 px)   preset 0 -> 0    <-- just under the threshold
#      512x336 (172,032 px)   preset 0 -> 1    <-- just over
#
# So the "cells >= 128px" intuition is WRONG: a 128x128 or 256x256 frame can
# never exercise SB128, because C forces 64 below 165,120 px. Every cell here
# is >= 165,120 aligned luma samples AND preset 0 or 1. Cells are kept just
# over the threshold to hold encode time down.
#
# ============================================================ WHAT IT ASSERTS
#   A. ANTI-VACUITY (hard): every sb128 cell's C oracle really did emit
#      `use_128x128_superblock=1`. Without this the gate could "pass" while
#      silently re-proving the SB64 gate. Mirrors hdr_bd10_gate.sh:145-158.
#   B. CONTROL (hard): a same-preset, same-content cell BELOW the area
#      threshold byte-matches. This is the harness-faithfulness proof — it
#      shows preset 0/1 identity is real, so an sb128 MISMATCH is about the
#      superblock geometry and not about preset 0 being unported.
#   C. BYTE-EXACT (hard): every cell listed in SB128_BYTE_EXACT must `cmp`
#      clean. Adding a cell there means it byte-matches; never add one that
#      merely decodes or falls back.
#   E. DECODABILITY (hard, EVERY cell incl. pinned ones): the port's own
#      stream must decode with the AV1 reference decoder. A byte-comparison
#      gate is blind here — a PINNED cell is expected to differ from C, so
#      "differs" hides "is corrupt". That is not hypothetical: the SB128
#      landing shipped a stream that aomdec rejected with "Failed to decode
#      tile data" at six of ten qps, because C emits `cdef_idx` once per
#      64x64 FILTER BLOCK (four per SB128 superblock, latched by
#      `cdef_transmitted[4]`, entropy_coding.c:4009-4016) while the port
#      emitted one per SB — so the decoder expected literals the encoder
#      never wrote. Byte-identity alone would never have caught it.
#      Set AOMDEC=/path/to/aomdec; the check is skipped (loudly) if absent.
#   D. PIN (hard, self-promoting): every cell in SB128_PINNED must still
#      DIVERGE. When the SB128 encode path lands, those flip to matching and
#      this gate FAILS — that is the signal to move them into
#      SB128_BYTE_EXACT. Same pattern the sibling aom-rs port uses for its
#      pinned near-ties.
#
# Exit 0 iff A, B, C, D and E all hold.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/sb128gate.$$"
mkdir -p "$OUT"

pass=0
fail=0
failed=()

# Reference decoder for assert (E). Optional but loudly reported when
# missing — a silently-skipped corruption check is worse than none.
aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  for cand in /root/aomdec-build/aomdec /root/aomdec-debug/aomdec \
              /root/aom-rs/upstream/build/aomdec; do
    [ -x "$cand" ] && { aomdec="$cand"; break; }
  done
fi
command -v "$aomdec" >/dev/null 2>&1 || [ -x "$aomdec" ] || {
  echo "WARNING: aomdec not found (set AOMDEC=...) — assert (E) DECODABILITY is SKIPPED" >&2
  aomdec=""
}
[ -n "$aomdec" ] && echo "decodability check: $aomdec"

# Cells: "content w h qp preset". Every one is >= 165,120 aligned luma px
# and preset <= 1, i.e. SB128 in C (asserted per-cell below, never assumed).
#
#   512x384 = 196,608 px — an EXACT 128 grid (4x3 SBs), no partial SBs.
#   448x384 = 172,032 px — 3.5x3 SBs: a partial 128 COLUMN (448 = 3*128+64),
#                          which is also a whole number of 64s, so it isolates
#                          the SB128 right-edge case from partial-b64 coding.
#   512x448 = 229,376 px — 4x3.5 SBs: a partial 128 ROW.
SB128_CELLS=(
  "uniform  512 384 32 0"
  "uniform  512 384 55 0"
  "uniform  512 384 63 0"
  "uniform  512 384 32 1"
  "gradient 512 384 32 0"
  "gradient 512 384 55 0"
  "gradient 512 384 20 0"
  "gradient 512 384 32 1"
  "diag     512 384 32 0"
  "diag     512 384 55 0"
  "uniform  448 384 32 0"
  "gradient 448 384 32 0"
  "uniform  512 448 32 0"
  "gradient 512 448 32 0"
)

# Cells that BYTE-MATCH today. A cell only ever moves here after `cmp` says
# so. Landed by task #91 chunk 3 (the 128 partition root + the b64
# coding-unit walk):
#   - all four `uniform 512x384` cells (q32/q55/q63, p0 AND p1) — the exact
#     4x3 SB128 grid, no partial SBs;
#   - `diag 512x384 q32 p0` — TEXTURED content, so the forced-SPLIT 128 root
#     is not a uniform-only artefact;
#   - `uniform 448x384 q32 p0` — a partial 128 COLUMN (448 = 3*128 + 64), so
#     the right-edge `has_cols == false` binary partition arm (the
#     H4/V4-free `partition_gather_horz_alike` with is_128) is exercised;
#   - `uniform 512x448 q32 p0` — a partial 128 ROW, the `has_rows == false`
#     vert_alike counterpart.
#   - `diag 512x384 q55 p0` and `gradient 512x384 q55 p0` — closed by the
#     lr_params fix (the `unit_size > 64` bit is SB64-only). Both were
#     already TILE-identical, so they are pure header witnesses.
#   - `gradient 512x384 q32 p1` and `gradient 512x448 q32 p0` — closed by
#     threading C's `seq_header.sb_mi_size` (16 SB64 / 32 SB128) into the
#     intra availability tables, which had been hardcoded to the SB64 value.
#     `has_top_right` / `has_bottom_left` index blocks by
#     `mi & (sb_mi_size - 1)`, so a block at mi_col 16 reads as the SB's LEFT
#     column at SB64 but its RIGHT half at SB128 — different top-right /
#     bottom-left availability, hence different directional prediction. This
#     is the one genuinely SB128-specific sub-64 defect found so far.
#   - `gradient 512x384 q20 p0` — closed by the CDEF quadrant fix (below).
SB128_BYTE_EXACT=(
  "uniform  512 384 32 0"
  "uniform  512 384 55 0"
  "uniform  512 384 63 0"
  "uniform  512 384 32 1"
  "gradient 512 384 32 0"
  "gradient 512 384 55 0"
  "gradient 512 384 20 0"
  "gradient 512 384 32 1"
  "diag     512 384 32 0"
  "diag     512 384 55 0"
  "uniform  448 384 32 0"
  "gradient 448 384 32 0"
  "uniform  512 448 32 0"
  "gradient 512 448 32 0"
)

# ---------------------------------------------------------------------------
# REAL-CONTENT SB128 cell (task #91 codec_wiki) — a 512x512 CENTER-CROP of the
# gb82-sc `codec_wiki` screenshot (2560x1664). 262,144px >= 165,120 AND preset 0
# -> C codes SB128 (asserted per-cell by (A)). This is the cell that pinned the
# whole-128-SB PD0 max/min bug: C's `get_max_min_pd0_depths` (enc_dec_process.c:
# 1943) folds max/min PD0 block sizes over ALL FOUR 64x64 coding-unit quadrants,
# but the port computed them PER quadrant. On this frame SB(0,0)'s TR quadrant PD0
# max was 16x16 while the other three reached 32x32, so C's whole-SB max_pd0=32
# tested the 32x32 depth in the TR quadrant (`set_start_end_depth` s_depth=-1) but
# the port's per-quadrant max_pd0=16 capped it and force-split those 32x32 nodes.
# Closed by folding whole-128-SB max/min in `build_refined_scan_at`. q48 and q63
# byte-match; q32 STILL DIVERGES — a SEPARATE tx-type near-tie (the port picks
# ADST_ADST where C picks DCT_ADST on the 3rd 8x8 txb of the mi(4,24) 16x16 NONE,
# inflating NONE's rate so VERT wins), NOT the depth root — so it is deliberately
# not asserted (never assert a non-matching cell).
#
# The corpus is a LOCAL resource (not fetched in CI, exactly like
# coverage_combos_gate.sh's real cells). SC_CORPUS overrides the dir; when it is
# absent the cells are dropped with a LOUD warning — the corpus-presence decision
# is made HERE at the caller, never buried inside a silently-skipping cell.
SC_CORPUS="${SC_CORPUS:-/root/work/codec-corpus/gb82-sc}"
_wiki_png="$SC_CORPUS/codec_wiki.png"
if [ -f "$_wiki_png" ]; then
  SB128_CELLS+=("crop:$_wiki_png 512 512 48 0" "crop:$_wiki_png 512 512 63 0")
  SB128_BYTE_EXACT+=("crop:$_wiki_png 512 512 48 0" "crop:$_wiki_png 512 512 63 0")
else
  echo "WARNING: $_wiki_png not found (set SC_CORPUS) — codec_wiki SB128 cells SKIPPED" >&2
fi

# ---------------------------------------------------------------------------
# THE 2 FORMERLY-PINNED CELLS — CLOSED 2026-07-22 (bd8 ind_uv fast-loop SAD).
#
# `gradient 512x384 q32 p0` and `gradient 448x384 q32 p0` diverged at a 32x32
# node where C coded `s=9` (PARTITION_VERT_4) and the port coded `s=0`
# (PARTITION_NONE). It was NEVER an SB128 bug: it reproduced at SB64 on
# `gradient 424x384 q32 p0` (below the area threshold) at the same node.
#
# ROOT CAUSE (per-candidate leaf-RD dump, C `SVT_PICKPART_OUT` vs the port's
# `SVTAV1_NSQDBG`/`SVTAV1_UVDBG`): the VERT_4 divergence was ENTIRELY in the
# first 8x32 sub-block's CHROMA mode — C coded UV_PAETH (12), the port coded
# UV_DC (0). Luma was bit-identical. C's V4 leaf-sum 69441889 + partrate =
# 69889912 < NONE 69986899 (C keeps V4); the port's UV_DC inflated that
# sub-block's RD by 241726, pushing V4 to 70131638 > NONE (port kept NONE).
#
# The port's bd8 `search_best_independent_uv_mode` fast loop scored candidates
# by residual VARIANCE, but C's `mds0_dist_type` is zero-initialized = SAD
# (never assigned anywhere in `Source/Lib`), so C scores plain SAD. Variance is
# DC-invariant, so on the flat 4x16 chroma many candidates tied at 0 and pushed
# UV_PAETH just past the nfl=32 survivor cut; SAD keeps it, matching C. (The
# bd10 path already used SAD; the bd8 arm was the straggler.) Fix:
# `crates/svtav1-encoder/src/leaf_funnel.rs` `residual_sad` (was
# `residual_variance`). Chroma is now byte-identical to C on these cells; both
# pins byte-match end-to-end.

# Cells PINNED as still-diverging (currently NONE). Everything in SB128_CELLS
# that is not in SB128_BYTE_EXACT is implicitly pinned (see the loop) — the pin
# is what makes this gate fail-forward when the port starts matching.

# CONTROL cells: same presets and content, BELOW the area threshold, so C
# codes them at SB64 and the port must byte-match them exactly.
CONTROL_CELLS=(
  "uniform  512 320 32 0"
  "gradient 512 320 32 0"
  "gradient 512 320 32 1"
  "gradient 256 256 32 0"
)

in_list() {
  local needle="$1"; shift
  local e
  for e in "$@"; do [ "$e" = "$needle" ] && return 0; done
  return 1
}

# ---------------------------------------------------------------- CONTROL
echo "--- control cells (below the 165,120px threshold -> C codes SB64) ---"
for cell in "${CONTROL_CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="ctl_${content}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi
  # The control must be SB64 on BOTH sides, or it is not a control.
  sb=$(python3 "$HERE/sb128_seqhdr.py" "$OUT/c.obu" | grep -o 'use_128x128_superblock=[01]' | cut -d= -f2)
  if [ "$sb" != "0" ]; then
    fail=$((fail + 1)); failed+=("$tag[control-is-sb128-not-a-control]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1)); echo "  OK       $tag (sb64)"
  else
    fail=$((fail + 1)); failed+=("$tag[control-MISMATCH]")
    echo "  MISMATCH $tag  <-- harness/preset regression, not an sb128 issue"
  fi
done

# ------------------------------------------------------------- SB128 CELLS
echo "--- sb128 cells (>= 165,120px, preset <= 1 -> C codes SB128) ---"
for cell in "${SB128_CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  # `crop:/abs/foo.png` / `file:/abs/foo.png` -> a clean `foo` tag stem.
  case "$content" in
  crop:* | file:*) cname=$(basename "${content#*:}" .png) ;;
  *) cname="$content" ;;
  esac
  tag="${cname}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi

  # (A) ANTI-VACUITY — the oracle must genuinely be an SB128 encode.
  sb=$(python3 "$HERE/sb128_seqhdr.py" "$OUT/c.obu" | grep -o 'use_128x128_superblock=[01]' | cut -d= -f2)
  if [ "$sb" != "1" ]; then
    fail=$((fail + 1)); failed+=("$tag[VACUOUS: C emitted sb64, cell proves nothing]")
    echo "  VACUOUS  $tag"
    continue
  fi

  # (E) DECODABILITY — the port's own bytes must be a legal AV1 stream,
  # whether or not they equal C's. See the header note.
  if [ -n "$aomdec" ]; then
    if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
      fail=$((fail + 1)); failed+=("$tag[UNDECODABLE: aomdec rejected the port stream]")
      echo "  CORRUPT  $tag  <-- port stream does not decode"
      continue
    fi
  fi

  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    if in_list "$cell" "${SB128_BYTE_EXACT[@]+"${SB128_BYTE_EXACT[@]}"}"; then
      pass=$((pass + 1)); echo "  OK       $tag (sb128 byte-exact)"
    else
      # (D) the self-promoting pin fired: this is GOOD news that must not be
      # silently absorbed. Fail loudly so the cell gets promoted.
      fail=$((fail + 1)); failed+=("$tag[PIN-BROKEN: now byte-exact -> move to SB128_BYTE_EXACT]")
      echo "  PROMOTE  $tag  <-- now byte-exact! add it to SB128_BYTE_EXACT"
    fi
  else
    if in_list "$cell" "${SB128_BYTE_EXACT[@]+"${SB128_BYTE_EXACT[@]}"}"; then
      fail=$((fail + 1)); failed+=("$tag[REGRESSION: was byte-exact]")
      echo "  REGRESS  $tag"
    else
      pass=$((pass + 1))
      cb=$(stat -c%s "$OUT/c.obu"); rb=$(stat -c%s "$OUT/rs.obu")
      echo "  pinned   $tag (C=${cb}B port=${rb}B — sb128 path unported)"
    fi
  fi
done

rm -rf "$OUT"
total=$((pass + fail))
echo
echo "sb128 gate: $pass / $total"
if [ "$fail" -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
fi
[ "$fail" -eq 0 ]
