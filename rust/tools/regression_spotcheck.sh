#!/usr/bin/env bash
# REGRESSION SPOT-CHECK — one cell per bug we have actually fixed.
#
# THE POINT. The full sweeps (tools/identity_full_8bit.sh, the real-corpus and
# dims tiers) are ~1,000-2,500 cells and take 25-45 minutes. That is the right
# tool for "is the envelope still what we think it is", and the wrong tool for
# "did I just break something". This gate is the second thing: every cell here
# is the MINIMAL REPRODUCER OF A BUG THAT ONCE SHIPPED, so a red cell names its
# own regression instead of leaving you to bisect a sweep.
#
# Runs in ~1-2 minutes. Run it after every change, before the sweeps.
#
# THE RULE FOR ADDING A CELL — this is what keeps the gate honest:
#   A cell earns its place ONLY if it FAILED before its fix and PASSES after.
#   Not "it covers the area". Not "it seemed related". If you cannot state the
#   observed failure (bytes, panic message, or decoder error) the cell does not
#   go in, because a cell that never failed cannot detect the regression of a
#   fix it never witnessed. Record that observed failure in the comment.
#
# Every entry below carries: what broke, the commit that fixed it, and the
# MEASURED before/after. If you fix a bug and do not add a line here, the next
# person gets to rediscover it.
#
# Usage: tools/regression_spotcheck.sh
# Env:   RS_AOMDEC (aomdec path, for the decodability cells)
#        SCREEN_DIR / CID22_DIR (real-corpus cells; skipped LOUDLY if absent)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"

RUN="$HERE/identity_run"
CT="$HERE/capture_c_trace/capture_c_trace"
AOMDEC="${RS_AOMDEC:-$(command -v aomdec || true)}"
SCREEN_DIR="${SCREEN_DIR:-$HOME/work/zen/codec-corpus/gb82-sc}"
CID22_DIR="${CID22_DIR:-$HOME/work/zen/codec-corpus/CID22/CID22-512}"
W="${TMPDIR:-/tmp}/spotcheck.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

pass=0; fail=0; skip=0
failed=(); skipped=()

# byte <label> <content> <w> <h> <qp> <preset> <bd>
# Asserts the port's stream is byte-identical to the real C encoder's.
byte() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=${7:-8}
  if ! SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [port failed to encode]"); return
  fi
  if ! SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" "$W/rs.yuv" "$W/c.obu" "$bd" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  if cmp -s "$W/c.obu" "$W/rs.obu"; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    failed+=("$label [C=$(wc -c <"$W/c.obu"|tr -d ' ')B port=$(wc -c <"$W/rs.obu"|tr -d ' ')B]")
  fi
}

# noPanic <label> <content> <w> <h> <qp> <preset> <bd>
# Asserts the encode does not PANIC. A typed refusal (exit 3) passes: refusing an
# out-of-envelope config is the correct behaviour, and conflating it with a
# crash is a bug this gate itself once had.
noPanic() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=${7:-8}
  SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>"$W/err"
  local rc=$?
  if [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; then
    pass=$((pass+1))
  elif grep -q "panicked at" "$W/err"; then
    fail=$((fail+1)); failed+=("$label PANIC: $(grep -m1 'panicked at' "$W/err" | sed 's/.*panicked at //')")
  else
    fail=$((fail+1)); failed+=("$label [rc=$rc]")
  fi
}

# decodes <label> <content> <w> <h> <qp> <preset> <bd>
# Asserts the port's stream DECODES under the reference decoder.
#
# Byte-parity cannot express this class: a desynced stream can be the same
# LENGTH as C's and still be undecodable (the palette map-clip bug below
# produced 317B where C produced 317B). Skipped, loudly, when no aomdec is on
# PATH -- a decode assertion with no decoder must never be counted as a pass.
decodes() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=${7:-8}
  local dec=${AOMDEC:-$(command -v aomdec || true)}
  if [ -z "$dec" ]; then
    skip=$((skip+1)); skipped+=("$label (no aomdec on PATH; set AOMDEC=)")
    return
  fi
  SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1 || {
    fail=$((fail+1)); failed+=("$label rs-err"); return
  }
  if "$dec" --summary -o /dev/null "$W/rs.obu" >/dev/null 2>&1; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$label DECODE-FAIL ($(wc -c < "$W/rs.obu" | tr -d ' ')B)")
  fi
}

# ratio <label> <content> <w> <h> <qp> <preset> <bd> <max_abs_pct>
# Asserts the port's stream is within <max_abs_pct> of C's SIZE.
#
# Not every fix produces byte-identity, and pretending otherwise is how a
# registry rots. The bd10 IntraBC port took `terminal` p2 from +75.25% to
# -1.24% — an enormous, real win, and still DIFFERS byte-wise because the
# residual #71 palette/IBC decision band is open at that preset. Asserting
# bytes there would fail forever and get deleted; asserting the SIZE band
# catches the regression the fix actually prevents.
ratio() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=$7 lim=$8
  if ! SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [port failed to encode]"); return
  fi
  if ! SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" "$W/rs.yuv" "$W/c.obu" "$bd" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  local cb pb pct
  cb=$(wc -c <"$W/c.obu"|tr -d ' '); pb=$(wc -c <"$W/rs.obu"|tr -d ' ')
  pct=$(python3 -c "print(abs(100.0*($pb-$cb)/$cb))")
  if python3 -c "import sys; sys.exit(0 if $pct <= $lim else 1)"; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$label [C=${cb}B port=${pb}B, ${pct}% off, limit ${lim}%]")
  fi
}

# byteVideoKey <label> <content> <w> <h> <qp> <preset>
# Asserts the port's VIDEO-mode KEY frame is byte-identical to C's.
#
# Not the same cell as byte(): the still/AVIF path and the video path are two
# different encodes of the same pixels, because almost every C signal
# derivation forks on `scs->allintra` (see docs/INTER-ENCODE-PLAN.md §1b). A
# still cell cannot witness a video-arm regression at all.
#
# Frame 0 ONLY. The port still REFUSES the inter frame (exit 3, "still/key
# frames only"), and that refusal is correct behaviour, not a failure — §6.
# The GOP shape matches identity_diff_inter.sh: low-delay P, flat, key frame 0.
byteVideoKey() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6
  # Both sides write per-frame files; a stale one from the previous cell would
  # be compared silently if this run failed to write (§5, "a silent harness and
  # a genuine absence are indistinguishable").
  rm -f "$W"/c.obu.pts* "$W"/rs.obu.f*
  local rc=0
  SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
    $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 3 ]; then
    fail=$((fail+1)); failed+=("$label [port failed to encode, rc=$rc]"); return
  fi
  if ! SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
       SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" \
       "$W/rs.yuv" "$W/c.obu" 8 >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  if [ ! -e "$W/c.obu.pts0" ] || [ ! -e "$W/rs.obu.f0" ]; then
    fail=$((fail+1)); failed+=("$label [frame 0 missing on one side]"); return
  fi
  if cmp -s "$W/c.obu.pts0" "$W/rs.obu.f0"; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    failed+=("$label [C=$(wc -c <"$W/c.obu.pts0"|tr -d ' ')B port=$(wc -c <"$W/rs.obu.f0"|tr -d ' ')B]")
  fi
}

# fhVideoKey <label> <content> <w> <h> <qp> <preset>
# Asserts every FRAME-HEADER field of the port's VIDEO-mode KEY frame equals
# C's, via tools/fh_fields.py.
#
# WEAKER than byteVideoKey ON PURPOSE, and the label says so: it does NOT look
# at the tile payload. It exists for the cells where the header is closed but
# the payload is not yet, which is the whole shape of the inter campaign — a
# header field that regresses there would otherwise be invisible until the
# payload lands, which could be many chunks away. Promote a cell from here to
# byteVideoKey the moment its payload closes; do not leave it here as the
# weaker assertion once the stronger one can hold.
#
# fh_fields.py always exits 0 (it is a differ, not a gate), so the verdict is
# read off its output. A walk that diverges prints "field counts differ" as
# well as "DIFFERS", and BOTH are failures here.
fhVideoKey() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6
  rm -f "$W"/c.obu.pts* "$W"/rs.obu.f*
  local rc=0
  SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
    $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 3 ]; then
    fail=$((fail+1)); failed+=("$label [port failed to encode, rc=$rc]"); return
  fi
  if ! SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
       SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" \
       "$W/rs.yuv" "$W/c.obu" 8 >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  if [ ! -e "$W/c.obu.pts0" ] || [ ! -e "$W/rs.obu.f0" ]; then
    fail=$((fail+1)); failed+=("$label [frame 0 missing on one side]"); return
  fi
  local out
  if ! out=$(python3 "$HERE/fh_fields.py" "$W/c.obu.pts0" "$W/rs.obu.f0" 2>&1); then
    fail=$((fail+1)); failed+=("$label [fh_fields.py could not walk the header]"); return
  fi
  # Anti-vacuity: a walk that produced no field rows must never count as a pass
  # (§5 — a silent harness and a genuine absence are indistinguishable).
  if ! printf '%s\n' "$out" | grep -q '^show_existing_frame'; then
    fail=$((fail+1)); failed+=("$label [fh_fields.py emitted no fields]"); return
  fi
  if printf '%s\n' "$out" | grep -qE 'DIFFERS|field counts differ'; then
    fail=$((fail+1))
    failed+=("$label [first diverging FH field: $(printf '%s\n' "$out" | grep -m1 'DIFFERS' | sed 's/  */ /g')]")
  else
    pass=$((pass+1))
  fi
}

# ratioVideoKey <label> <content> <w> <h> <qp> <preset> <limit_pct>
# The size counterpart of fhVideoKey: asserts the port's VIDEO-mode KEY frame
# is within <limit_pct> of C's byte count.
#
# WEAKER than byteVideoKey on purpose, like ratio() is weaker than byte(): it
# witnesses a PARTITION-SEARCH regression on a cell whose payload is not yet
# byte-identical. A partition ladder taken from the wrong arm of
# `scs->allintra` moves the coded tree and therefore the size, so the size is
# the only handle available until the payload closes. Promote to byteVideoKey
# the moment it does; do not leave the weaker assertion in place after that.
#
# Deterministic on both sides — the limits below are NOT noise bands, they are
# chosen to sit between the measured before and after of one specific fix, and
# each cell's comment states both numbers.
ratioVideoKey() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 lim=$7
  rm -f "$W"/c.obu.pts* "$W"/rs.obu.f*
  local rc=0
  SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
    $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 3 ]; then
    fail=$((fail+1)); failed+=("$label [port failed to encode, rc=$rc]"); return
  fi
  if ! SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
       SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" \
       "$W/rs.yuv" "$W/c.obu" 8 >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  if [ ! -s "$W/c.obu.pts0" ] || [ ! -s "$W/rs.obu.f0" ]; then
    fail=$((fail+1)); failed+=("$label [frame 0 missing or empty on one side]"); return
  fi
  local cb pb pct
  cb=$(wc -c <"$W/c.obu.pts0"|tr -d ' '); pb=$(wc -c <"$W/rs.obu.f0"|tr -d ' ')
  pct=$(python3 -c "print(abs(100.0*($pb-$cb)/$cb))")
  if python3 -c "import sys; sys.exit(0 if $pct <= $lim else 1)"; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$label [C=${cb}B port=${pb}B, ${pct}% off, limit ${lim}%]")
  fi
}

need_file() { [ -f "$1" ] || { skip=$((skip+1)); skipped+=("$2 — $1 absent"); return 1; }; }

echo "== regression spot-check: one cell per fixed bug =="

# ---------------------------------------------------------------------------
# 2026-08-06 — LOOP RESTORATION applied over the ALIGNED extent while the RU
# grid was counted on the TRUE one (issue #11). C sizes the grid
# (svt_av1_alloc_restoration_struct) and walks the units
# (svt_av1_loop_restoration_filter_frame, svt_av1_loop_restoration_save_boundary_lines)
# off ONE whole_frame_rect(&cm->frm_size, ..), and cm->frm_size is the
# pre-8-alignment size (pcs.c:1337). The port passed the aligned w/h to the
# apply + boundary-save while the search used the true dims, so wherever the
# 8-alignment crosses a count_units_in_tile(256, ..) boundary the walk visited
# more units than the grid holds. MEASURED before: `gradient 383x512 q40 p6`
# panicked "restoration.rs:985: index out of bounds: the len is 2 but the index
# is 2" (true 383 -> 1 horizontal unit, aligned 384 -> 2), at bd8 AND bd10;
# `gradient 766x128` the same on CHROMA (ceil(766/2)=383 vs 768/2=384), len 1
# index 1. After: byte-identical to C. Reported on 5 real renditions
# (1914x2560, 2048x1660, 2297x3072, 766x1024, 383x512), 115 of 34,200 cells.
byte "lr-align-cross-383x512-bd8"  gradient 383 512 40 6 8
byte "lr-align-cross-383x512-bd10" gradient 383 512 40 6 10
byte "lr-align-cross-chroma-766"   gradient 766 128 40 6 8

# ---------------------------------------------------------------------------
# 2026-08-03 — bd10 PALETTE was gated out of the mode-decision funnel entirely.
# 12801d936. The port coded ZERO palette blocks at 10 bits where C codes
# hundreds. MEASURED before: screen 128x128 q32 p0 -> C 327B, port 664B (2.03x);
# p6 -> C 453B, port 1110B (2.45x). After: byte-identical.
byte "bd10-palette-p0"        screen 128 128 32 0 10
byte "bd10-palette-p6"        screen 128 128 32 6 10

# 2026-08-03 — palette COLOUR LITERALS were written 8 bits wide at bd10, on both
# the writer (entropy_coding.c:4369) and the RD cost (rd_cost.c:600). 12801d936.
# An arithmetic-decoder desync on the first 10-bit palette block; covered by the
# cells above (they cannot pass with a wrong literal width) plus a low qp where
# more colours are coded.
byte "bd10-palette-colors-q5" screen 128 128 5  2 10

# 2026-08-03 — bd10 INTRABC gated out of the funnel. 89b9c18ce. The frame header
# signalled allow_intrabc=1 while every block coded use_intrabc=0. MEASURED on
# the gb82-sc corpus: mean size delta vs C +23.58% -> +0.42%; terminal p2
# +75.2% -> -1.2%. Needs the real corpus: synthetic content NEVER codes an
# IntraBC block (measured 0 at every preset), so no synthetic cell can guard it.
# NOT a byte cell: this fix was a SIZE-parity win, not byte-identity. MEASURED
# terminal p2 bd10: before C=7611B port=13338B (+75.25%); after port=7517B
# (-1.24%) and still DIFFERS, because the #71 palette/IBC decision band is open
# at that preset. A 5% limit fails loudly if IntraBC is ever gated out again
# (which would put it back near +75%) without demanding a byte-identity the fix
# never delivered.
if need_file "$SCREEN_DIR/terminal.png" "bd10-intrabc"; then
  ratio "bd10-intrabc-terminal-p2" "crop:$SCREEN_DIR/terminal.png" 512 512 20 2 10 5
fi

# 2026-08-03 — CDEF screen-content qp-strength arm unported. 4be53b11c.
# svt_pick_cdef_from_qp has three arms (enc_cdef.c:837-844); the port only had
# the intra one and took it unconditionally. Reachable at preset 7 exactly
# (use_qp_strength needs cdef_search_level 10 = M7+, screen detection dies at
# M8+). MEASURED: with the arm's flag forced false, 10 of 12 preset-7 screen
# cells FAIL at an IDENTICAL byte count — the strengths are fixed-width header
# fields, so only a byte compare can see it.
byte "cdef-screen-arm-p7-q20" screen 128 128 20 7
byte "cdef-screen-arm-p7-q48" screen 64  64  48 7

# 2026-08-03 — intrabc_hash `usize` UNDERFLOW, two OOB panics on the PUBLIC API.
# 2b8a6e251. C computes `x_end = pic_width - block_size + 1` SIGNED
# (hash_motion.c:195-196, :222-223) so a picture smaller than the hash block
# yields an empty loop; `usize` wrapped to ~2^64 and indexed off the end.
# MEASURED: 32x32 screen at preset 0 panicked "len is 1024 but the index is
# 1024" and then "index 2048". Needs SCREEN content — gradient never arms the
# screen tools, which is exactly why the panic-freedom gate missed it.
noPanic "intrabc-hash-underflow-32"  screen 32 32 20 0
noPanic "intrabc-hash-underflow-48"  screen 48 48 45 1
noPanic "intrabc-hash-underflow-16"  screen 16 16 20 0

# 2026-08-03 — cropped-TX RD distortion: frame_geom::cropped_tx_dims existed
# with ZERO call sites for a month. dde88dbf6. Boundary blocks were priced over
# the whole transform block instead of the in-frame part. MEASURED: with the
# crop reverted these three straddle cells diverge (C 114/154/99B vs port
# 119/157/103B); with it, byte-identical.
byte "cropped-tx-80x88"  gradient 80  88 55 6
byte "cropped-tx-104x88" gradient 104 88 55 6
byte "cropped-tx-72x88"  gradient 72  88 55 6

# 2026-08-03 — end_tx_depth frame-boundary rule (product_coding_loop.c:6710-6717)
# unported: a leaf straddling the aligned edge was searched at tx depth 1 where
# C pins 0. 4eca22119.
#
# DELIBERATELY NO CELL HERE. The rule is live at preset 7 (measured: leaf
# (32,64) 32x32 on an 80x88 frame, txs_active, end_tx_depth would be 1) but it
# is byte-INERT on all 48 partial-SB cells A/B'd — it changes the searched depth
# set without flipping a verdict. Per this file's own rule, a cell that did not
# fail before the fix cannot detect its regression, so adding one would be
# decoration. `gradient 80 88 55 7` in particular DIFFERS with AND without the
# term and would have been a false guard. Tracked in
# docs/arbitrary-dims-port-map.md instead; give it a cell if a witness ever
# appears.

# 2026-08-04 — PROMOTED. `screen 64x64 q63 p1` and `screen 128x128 q63 p1` were
# the #71 palette-over-picking pins (C=64B/port=71B and C=185B/port=193B) and
# this script used to assert they still DIFFER, so that a fix would announce
# itself rather than silently change the envelope. They now byte-match, the
# anti-assertion fired exactly as designed, and the cells are promoted here into
# ordinary regression guards.
#
# Root: the MDS3 ind-uv chroma rewrite rebuilt EVERY candidate's uv fast rate
# with `palette_uv_mode_fac_bits[0][0]`, including luma-palette candidates,
# which C prices with the `[1][0]` row (rd_cost.c:518-520, `use_palette_y` read
# off the REAL candidate). That under-costed a palette candidate's chroma flag.
#
# This is the self-promoting contract completing its full cycle: pinned as
# known-diff -> asserted-differing here -> fix lands -> gate demands promotion ->
# promoted in identity_full_8bit.sh and here. A pin that can never be promoted
# is just a permanent excuse.
byte "sc-q63-p1-64"   screen  64  64 63 1
byte "sc-q63-p1-128"  screen 128 128 63 1

# ---------------------------------------------------------------------------
# Earlier fixes, kept because they are cheap and each once shipped.
#
# 2026-07-18 — palette blocks coded an EXTRA use_filter_intra flag on 4:2:0
# (missing C's `palette_size == 0` gate, mode_decision.c:107): a whole-tile
# arithmetic-decoder desync, 99/1260 streams. Decodability is the assertion.
if [ -n "$AOMDEC" ]; then
  if $LOWPRI "$RUN" screen 128 128 20 2 "$W/dec" >/dev/null 2>&1 &&
     "$AOMDEC" --summary -o /dev/null "$W/dec.obu" >/dev/null 2>&1; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("palette-filter-intra-desync [stream did not decode]")
  fi
else
  skip=$((skip+1)); skipped+=("palette-filter-intra-desync — no aomdec (set RS_AOMDEC)")
fi

# 2026-07-13 — intra edge fill used 128 where the decoder replicates edges, and
# dr_prediction_z2 had dx/dy swapped. Edge content at a low qp is the witness.
# 2026-08-04 — palette map tokens were written over the WHOLE block, not the
# part inside the frame. C writes `rows_within_bounds x cols_within_bounds`
# (svt_aom_get_block_dimensions, palette.c:217-245) on BOTH the pack side
# (entropy_coding.c:5083) and the RD side (get_palette_params_rate, :569-580);
# the port passed the full block width/height to both. A straddling palette
# block therefore emitted color-index symbols the decoder never reads ->
# whole-tile desync.
#
# Latent until the edge-aware PD1 walk landed the same day: 64-aligned frames
# have no straddling block, and before that walk a partial SB never reached
# palette at presets 0..5. Enabling it produced three DECODE-FAILs in CI --
# screen 56x56 / 120x120 / 65x257 at q20 p0. Note 65x257 and 56x56 were the
# SAME LENGTH as C's stream while undecodable, which is exactly why these are
# decode cells and not byte cells: byte-parity cannot see this class.
decodes "palette-map-clip-56"   screen  56  56 20 0
decodes "palette-map-clip-120"  screen 120 120 20 0
decodes "palette-map-clip-65"   screen  65 257 20 0

# 2026-08-04 — an out-of-set tx type must cost ZERO in the IntraBC coeff rate.
# C's `{intra,inter}_tx_type_fac_bits` are TX-TYPE-indexed and
# `svt_aom_get_syntax_rate_from_cdf(..., av1_ext_tx_inv[set])`
# (md_rate_estimation.c:225-243) SCATTERS costs into only the types belonging to
# that set, leaving every other entry at its zero init. A query for a type
# outside the row's set therefore reads a literal 0 — not a symbol lookup. This
# port keeps the tables SYMBOL-indexed, so it charged symbol 0's (large) rate
# instead. Only the IntraBC coeff cost can query out-of-set (the type comes from
# the INTER search set while the row read is the INTRA set).
#
# OBSERVED before the fix: gb82-sc graph.png 512x512 q63 p2 was C=252B /
# port=252B — the SAME LENGTH with different bytes from OBU offset 160. At block
# mi(8,80), luma txb (16,0) 16x16 V_DCT, C priced the tx type at 0 for a txb
# cost of 2808 while the port charged 2489 for 5297, flipping the TXT winner to
# DCT_DCT/eob=0 where C codes V_DCT/eob=1. Fixed by 71b90b97e.
#
# Needs the real screen corpus: no synthetic content codes an IntraBC block.
if [ -f "$SCREEN_DIR/graph.png" ]; then
  byte "ibc-outofset-txtype" "crop:$SCREEN_DIR/graph.png" 512 512 63 2
else
  skip=$((skip+1)); skipped+=("ibc-outofset-txtype (no $SCREEN_DIR/graph.png)")
fi

# 2026-08-04 — extended partitions (H4/V4/HA/HB/VA/VB) panicked when a
# STRADDLING node's tail sub-blocks fell outside the aligned frame. Such a node
# is NOT a boundary node (has_rows && has_cols are both true) but its block
# still reaches past the extent, so its last H4/V4 children start outside and
# code nothing. The pack's offset table matched only the full child count and
# `panic!("unsupported partition shape (Horz4, 3)")`.
#
# OBSERVED on a 512x481 luma crop of gb82-sc/graph.png at preset 2 — a PUBLIC
# API panic on legal input, which this project's contract forbids outright.
#
# It survived every gate because the identity harness rejects odd dims for
# `crop:` content ("I420 needs even dims"), so no cell could reach an
# odd-height REAL-content frame. It was found by the IntraBC tier-invariance
# test, which builds its own planes and never goes through that check — a
# reminder that a harness precondition is also a coverage hole.
#
# Asserted here on synthetic content at the same geometry (the panic is in the
# PACK and content-independent once the shape is picked); the real-content
# reproducer lives in the tier gate.
noPanic "h4-straddle-tail-481" gradient 512 481 32 2
noPanic "h4-straddle-tail-200" gradient 256 200 32 2

byte "intra-edge-fill"   diag 64 64 20 6
byte "intra-edge-dr-z2"  diag 128 128 32 4

# 2026-08-13 (issue #15) — the palette SEARCH ran over the whole block on a
# block that straddles the aligned frame edge, where C searches only the
# in-frame part (`svt_aom_get_block_dimensions`' rows/cols_within_bounds,
# palette.c:217-245, consumed at :401-439). The padded rows beyond the picture
# edge voted in the colour histogram, the dominant-colour scan and the k-means
# seed range, so a straddling block got different palette COLOURS than C's.
# The rate side and the pack side already cropped; only the search did not.
#
# OBSERVED: gb82-sc/terminal.png cropped to 96x88 (a 24-tall bottom SB) at
# preset 4 qp 33 was C=523B / port=521B, first differing byte at offset 11.
# The Linux `--wrap` op trace (rust/tools/ctrace-linux) put the first divergent
# symbol at op 4626 of 6299 — inside a 37-bit literal run right after a
# `palette_y_size` CDF — i.e. the palette colour literals, with the partition
# and mode symbols before it all matching. Fixed by cropping the search.
#
# Needs the real screen corpus: synthetic content never codes a palette block.
if [ -f "$SCREEN_DIR/terminal.png" ]; then
  byte "palette-straddle-search-96x88" "crop:$SCREEN_DIR/terminal.png" 96 88 33 4
else
  skip=$((skip+1)); skipped+=("palette-straddle-search-96x88 (no $SCREEN_DIR/terminal.png)")
fi

# 2026-08-14 (issue #15, the last 2 cells) — the MDS3 independent-chroma search
# ran where C SKIPS it. C's gate is `perform_ind_uv_search_last_mds`
# (product_coding_loop.c:1472-1504); its second arm zeroes the intra count when
# `best_inter_cost * inter_vs_intra_cost_th < best_intra_cost * 100`
# (th = 100 at chroma_level 4), and `is_inter` there is
# `is_inter_mode(mode) || use_intrabc` — so a winning IntraBC candidate on
# SCREEN CONTENT makes C skip the search, leave `ind_uv_avail = 0`, and code
# every MDS3 candidate's injected uv-follows-luma chroma. The port had no such
# arm and always ran the search, so its per-luma-mode uv table overrode the
# injected pair with UV_DC.
#
# OBSERVED: gb82-sc/terminal.png cropped to 188x256, both cells the same byte
# count as C and a single chroma-mode field flip in the final tree —
#   p2 q55  772 B, 35 differing bytes from 737: uv=D113/aduv=-1 (C) vs UV_DC,
#           mi=(50,42); C MDS1 best intra 97,762,561 vs best IntraBC 84,376,537
#   p4 q12  2698 B, 319 differing bytes from 2377: UV_CFL_PRED (C) vs UV_DC,
#           mi=(46,46); C MDS1 best intra 163,691 vs best IntraBC 148,994
# Both measured on the C side with the `svt_aom_get_intra_uv_fast_rate`
# interposer (`indavail=0`) + the `svt_aom_full_cost` one (the two minima).
#
# Needs the real screen corpus: only screen content wins an IntraBC candidate.
if [ -f "$SCREEN_DIR/terminal.png" ]; then
  byte "ind-uv-ibc-cost-gate-188x256" "crop:$SCREEN_DIR/terminal.png" 188 256 55 2
else
  skip=$((skip+1)); skipped+=("ind-uv-ibc-cost-gate-188x256 (no $SCREEN_DIR/terminal.png)")
fi

# ---------------------------------------------------------------------------
# 2026-08-27 — MONOCHROME partial SBs at preset 6 coded a PARTITION_NONE
# square at a frame edge. The M6 PD0 keeps NSQ geometry on, so a one-false
# edge node is TESTED (rect edge-shape cost) instead of force-split; the
# funnel arm of `encode_fixed_tree` (4:2:0) turned that leaf into the single
# legal HORZ/VERT rect, the MONO arm did not and coded the full square. The
# pack's debug_assert names it ("PARTITION_NONE leaf at a frame edge (64,0)
# 64x64: has_rows=true has_cols=false — illegal per spec 5.11.4"); a release
# build emitted it. Presets >= 7 (NSQ geometry off -> forced SPLIT) were never
# affected, and 4:2:0 is untouched (byte-neutral by construction). Found by
# zenavif's seam canary. MEASURED before (release, aomdec): 96x80 2852B, 128x80
# 3808B, 200x136 8099B all "Corrupt frame detected"; after: 2525B / 3363B /
# 7877B, all decode (and 96x80 round-trips at 56 dB under rav1d-safe on the
# zenavif side). No C oracle: C v4.2.0 cannot encode mono (see CLAUDE.md
# envelope guard 6), so these are decode cells, not byte cells.
SVTAV1_MONO=1 decodes "mono-partial-sb-p6-96x80"   gradient  96  80 10 6
SVTAV1_MONO=1 decodes "mono-partial-sb-p6-128x80"  gradient 128  80 10 6
SVTAV1_MONO=1 decodes "mono-partial-sb-p6-200x136" gradient 200 136 10 6

# ---------------------------------------------------------------------------
# 2026-08-27 — MONOCHROME straddling edge block WRAPPED its recon into the next
# row (the second half of the fix above). Once the edge leaf is coded as the
# single legal rect, a THIN right edge makes it straddle the aligned width (VERT
# 32x64 at x=192 on aligned-200: 8 in-frame columns); `encode_single_block`
# stored the full width at the aligned stride, so the 24 off-aligned columns
# landed in the NEXT row's columns 0..24 and overwrote the already-committed
# top-left SB's recon — the encoder then predicted the second SB row from
# pixels the decoder never had. Decodability cannot see it (aomdec decodes the
# stream) and byte-parity cannot either (C cannot encode mono), so this cell
# uses the recon oracle: the encoder's FINAL reconstruction must equal the
# decoder's output. Content matters: on the synthetic `gradient` the PD0
# resolves that node to SPLIT and nothing straddles (bytes identical with and
# without the clip), so the cell feeds the (x+y) ramp that makes the rect win.
# MEASURED before the clip (release, this exact cell): 14,720 of 27,200 luma
# bytes differ — encoder recon 56.97 dB vs source, aomdec output 27.89 dB;
# after: byte-equal, 56.97 dB both. 96x80 (32-wide edge, no straddle) is
# byte-equal either way, which is the control that proves the oracle live.
ramp_yuv() { # <w> <h> <path>: 8-bit I420, luma (x+y)*255/(w+h), flat chroma
  python3 - "$1" "$2" "$3" <<'PY'
import sys
w, h, p = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
y = bytes((((x + yy) * 255) // (w + h)) for yy in range(h) for x in range(w))
c = ((w + 1) // 2) * ((h + 1) // 2)
open(p, "wb").write(y + bytes([128]) * (2 * c))
PY
}
# monoReconEq <label> <content> <w> <h> <qp> <preset>
# MONO encode; asserts the port's FINAL reconstruction (SVTAV1_FINAL_RECON, luma
# plane at the true dims) equals aomdec's --rawvideo output byte-for-byte.
monoReconEq() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6
  local dec=${AOMDEC:-$(command -v aomdec || true)}
  if [ -z "$dec" ]; then
    skip=$((skip+1)); skipped+=("$label (no aomdec on PATH; set AOMDEC=)")
    return
  fi
  if ! SVTAV1_MONO=1 SVTAV1_FINAL_RECON="$W/rs.recon" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label rs-err"); return
  fi
  if ! "$dec" --rawvideo -o "$W/rs.dec.yuv" "$W/rs.obu" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label DECODE-FAIL"); return
  fi
  head -c $((w * h)) "$W/rs.recon" > "$W/rs.recon.y"
  head -c $((w * h)) "$W/rs.dec.yuv" > "$W/rs.dec.y"
  if cmp -s "$W/rs.recon.y" "$W/rs.dec.y"; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    failed+=("$label RECON!=DECODE ($(cmp -l "$W/rs.recon.y" "$W/rs.dec.y" | wc -l | tr -d ' ') of $((w * h)) luma bytes differ)")
  fi
}
ramp_yuv 200 136 "$W/ramp_200x136.yuv"
monoReconEq "mono-straddle-wrap-p6-200x136" "raw:$W/ramp_200x136.yuv" 200 136 10 6
ramp_yuv 96 80 "$W/ramp_96x80.yuv"
monoReconEq "mono-straddle-control-p6-96x80" "raw:$W/ramp_96x80.yuv" 96 80 10 6

# ---------------------------------------------------------------------------
# 2026-08-28 — MAINLINE tune-IQ chroma delta-q was emitted in the FORK's
# four-delta form, under a sequence header that signalled
# separate_uv_delta_q = 0. Spec 5.9.12 reads `diff_uv_delta` ONLY when that SH
# bit is 1, so the extra bit (and the two extra V deltas) shifted every
# following bit of the frame header. Latent until the mainline chroma-q
# derivation (rc_crf_cqp.c's `#else` arm) was ported and started producing
# non-zero deltas at tune 3.
#
# OBSERVED BEFORE the fix: `tools/variance_boost_recon.sh` 0 passed / 60
# failed, every cell "DECODE FAILED" (CI run 33220828356); a plain tune-IQ
# 128x128 q40 p6 encode was 1069 B and rejected by aomdec AND dav1d. AFTER:
# 60/60, and the same cell is 1067 B and decodes under both.
#
# The fix is a type, not a branch: `entropy::obu::ChromaQSignal` is `Shared`
# (SH bit 0, one dc/ac pair, no diff_uv_delta) or `Separate` (SH bit 1, the
# fork's four), so a frame header that disagrees with its sequence header no
# longer type-checks.
SVTAV1_TUNE=3 decodes "tuneiq-chromaq-fh-128"    gradient 128 128 40 6
SVTAV1_TUNE=3 decodes "tuneiq-chromaq-fh-64-q20" gradient  64  64 20 6

# ---------------------------------------------------------------------------
# 2026-08-31 — the VIDEO-mode key frame took the ALLINTRA CDEF policy. C picks
# the CDEF policy in two steps that both fork on `scs->allintra`: a
# `cdef_search_level` ladder (allintra enc_mode_config.c:2396, video :2083)
# and then `set_cdef_search_controls` (:891), whose `use_qp_strength` selects
# the qp closed form over the RDO search. The port carried only the allintra
# ladder's RESOLVED candidate sets, flattened per preset and gated on
# `is_single_frame`, so a video key frame fell onto the qp fast path at EVERY
# preset — where C searches at every preset (`is_base ? 5 : 6` at M6..M7,
# 7 above).
#
# OBSERVED BEFORE the fix, `uniform 64x64 q40` frames=2 frame 0: DIFFERS at
# presets 0/3/6/8 at IDENTICAL byte counts (28/28/28/30 B) — the divergence is
# the signalled cdef_y/uv strength inside a same-length header, which is
# exactly the shape a size-based gate cannot see. AFTER: identical at all four.
# On `gradient 64x64 q40 p6` the same fix moved cdef_y_pri 1->0 and
# cdef_y_sec 0->2, matching C, and the first diverging FH field to
# cdef_uv_pri_strength[0].
#
# These are the first VIDEO-mode cells in this gate. They are cheap because
# flat content is where the video arm is already byte-clean; the gradient
# cells still differ in the tile payload for reasons upstream of CDEF, so they
# deliberately are NOT here.
byteVideoKey "video-key-cdef-p0"  uniform 64 64 40 0
byteVideoKey "video-key-cdef-p3"  uniform 64 64 40 3
byteVideoKey "video-key-cdef-p6"  uniform 64 64 40 6
byteVideoKey "video-key-cdef-p8"  uniform 64 64 40 8

# --- video-arm screen-content tool ladders (allow_intrabc), 2026-08-31 ------
# `sc_detect.rs` derived the picture-level screen-content tool levels from C's
# ALLINTRA ladders on EVERY frame; C picks per `scs->allintra` and the intra-BC
# ladders disagree at every preset (docs/ibc-port-map.md, "VIDEO-MODE ARM
# WIRED"). A video-mode key frame therefore signalled the wrong
# `frm_hdr->allow_intrabc`, which also suppresses the LF/CDEF/LR parameter
# blocks — so the header did not just carry a wrong bit, it carried a
# different SHAPE.
#
# OBSERVED BEFORE the fix, frames=2 frame 0 (the video-mode key frame):
#   screen 64x64 q40 p6: first diverging FH field `allow_intrabc` C=1 port=0
#                        (C 92 B, port 143 B; fh walk A=28 B=39 fields)
#   screen 64x64 q40 p8: first diverging FH field
#                        `allow_screen_content_tools` C=1 port=0
#                        (C 114 B, port 697 B)
# AFTER: every frame-header field identical on both, port 138 B / 691 B.
#
# PROMOTED to byteVideoKey 2026-09-01 by the palette-arm chunk below, which
# closed its payload. It was fhVideoKey while the tile still differed by a wide
# margin — C 114 B against the port's 568 — with the note "promote it when the
# payload closes". The payload closed: the port emits C's 114 B to the byte.
byteVideoKey "video-key-ibc-arm-p8" screen 64 64 40 8

# --- the p6 payload CLOSED, 2026-09-01 (the depth-refinement arm chunk)
# `video-key-ibc-arm-p6` was fhVideoKey with the note above ("Promote both to
# byteVideoKey when it closes"). Wiring C's VIDEO arm of
# `pic_block_based_depth_refinement_level` closed it, so it is promoted here
# rather than left as the weaker assertion.
#
# OBSERVED, screen 64x64 frames=2 frame 0, C 92 B at all three qp:
#   q20: before port 119 B — after BYTE-IDENTICAL
#   q40: before port 118 B — after BYTE-IDENTICAL
#   q55: before port 116 B — after BYTE-IDENTICAL
# ("before" = the same build with `DrCtrls::for_arm` forced to its Allintra
# branch, i.e. the pre-chunk `matches!(preset, 0..=5)` refinement gate.)
#
# These are the FIRST byte-identical video-mode key frames on non-degenerate
# content — `uniform` has been identical since the CDEF arm chunk, but it codes
# 28 B and reaches almost nothing.
byteVideoKey "video-key-dr-arm-screen-p6-q20" screen 64 64 20 6
byteVideoKey "video-key-dr-arm-screen-p6-q40" screen 64 64 40 6
byteVideoKey "video-key-dr-arm-screen-p6-q55" screen 64 64 55 6

# --- video-arm partition ladders (max_block_size + nsq geom/search), 2026-08-31
# `pipeline.rs` flattened C's ALLINTRA arm of three partition ladders into
# inline predicates and ran them on every frame:
#   `preset >= 8 && full_sb`  for `get_max_block_size_allintra` (:7042)
#   `preset <= 6`             for `svt_aom_get_nsq_geom_level_allintra` (:8240)
#   NsqCfg's base table       for `svt_aom_get_nsq_search_level_allintra` (:8363)
# A video-mode frame takes the `_default` twins instead, which disagree at
# every preset — NSQ search is OFF from M4 up on the allintra arm and ON to
# M13 on the video arm; NSQ geometry is OFF above M6 on the allintra arm and
# never off on the video arm. See docs/nsq-port-map.md.
#
# The partial-SB cell is the witness: `nsq_geom_enabled` only changes what a
# ONE-FALSE boundary node does, so a 64-aligned frame cannot see it at all
# (MEASURED: gradient 64x64 q40 is byte-identical before and after at p6/p7/p8).
#
# OBSERVED, gradient 72x88 q40, frames=2 frame 0 (the video-mode key frame):
#   preset 4:  before port 1492 B vs C 1403 B = 6.34% off; after 1398 B = 0.36%
#   preset 5:  before port 1499 B vs C 1485 B = 0.94% off; after 1484 B = 0.07%
#   preset 7:  before port 1502 B vs C 1539 B = 2.40% off; after 1511 B = 1.82%
# Each limit sits between that cell's before and after.
#
# ratioVideoKey and not byteVideoKey because the payload is still open on all
# three (the closest, p5, is 1 byte away but not identical).
#
# THE PRESET-7 CELL MOVED CONTENT, 2026-09-01 (the rate-arm chunk). It was
# `gradient 72 88 40 7`, limit 2.0. Wiring the video arm of rdoq_level /
# rate_est_level / update_cdf_level made that cell VACUOUS: with the partition
# arms forced back to Allintra the port emits 1499 B, and with them wired it
# emits 1499 B — the SAME stream, so no limit can make the cell witness the
# partition wiring any more (measured both ways on the same build; the gradient
# p4/p5 cells above still separate 1492/1398 and 1499/1484 and are untouched).
# Per the anti-vacuity rule in `rust/CLAUDE.md`, a gate that would pass without
# the feature is a defect, so the cell is REPLACED — not re-limited — by one
# that does separate, at a TIGHTER bound:
#   screenrep 72x88 q40 p7, C 2388 B: partition arms Allintra 2414 B = 1.089%
#   off; wired 2386 B = 0.084% off. Limit 0.5 sits between.
#
# PROMOTED to byteVideoKey 2026-09-01 — all three payloads CLOSED by the video
# PD0 chunk (docs/INTER-ENCODE-PLAN.md §1i). The ratio form was the weaker
# assertion while the payload was open; §"ratioVideoKey" above says not to
# leave it in place once the stronger one holds, so it does not.
#
# They still separate the partition arms exactly as the ratio cells did — with
# the arms forced back to Allintra the port emits 1492 / 1499 / 2414 B against
# C's 1403 / 1485 / 2388, so a regression fails on the first byte rather than
# on a percentage.
byteVideoKey "video-key-nsq-arm-p4-72x88" gradient 72 88 40 4
byteVideoKey "video-key-nsq-arm-p5-72x88" gradient 72 88 40 5
byteVideoKey "video-key-nsq-arm-p7-screenrep-72x88" screenrep 72 88 40 7

# --- video-arm RATE ladders (rdoq_level + rate_est_level + update_cdf_level),
# 2026-09-01. `pipeline.rs` ran the ALLINTRA arm of all three on every frame:
#   `quant::rdoq_level_allintra(preset.min(9), coeff_lvl)`  for :9904
#   `FunnelCfg::for_preset`'s baked (coeff_rate_est_lvl, real_coeff_ctx) for :9917
#   `matches!(preset, 0..=6)` as the per-SB CDF-chain gate    for :8534
# The video arm assigns a flat rdoq 1 (to M10), a flat rate_est 1, and keeps
# CDF adaptation ON at M7/M8 where the still arm switches it off. See
# `docs/rate-arm-port-map.md`.
#
# OBSERVED, gradient 72x88 q40 p9, frames=2 frame 0 (the video-mode key frame):
#   before port 1630 B vs C 1589 B = 2.580% off; after 1587 B = 0.126%.
# p9 is the cleanest witness because the eff-mode clamp does not move there
# (allintra M9 == video M9), so the whole delta is the two rate ladders.
#
# STAYS ratioVideoKey, and the attempt to promote it is recorded because the
# measurement is the interesting part. Wiring C's
# `cdef_recon_ctrls.zero_fs_cost_bias` (video `cdef_recon_level` 1 at M9..M10,
# unported on both arms) moved this cell from 1586 B to 1589 B — C's EXACT byte
# COUNT — but the payload is still not byte-identical, so byteVideoKey fails
# here where it passes on the three cells above. Same length, different bytes:
# the ratio cell cannot see that, which is exactly why the promotion was tried.
# Promote it when a byteVideoKey run passes, not when the percentage hits zero.
#
# PROMOTED to byteVideoKey 2026-09-01 — a byteVideoKey run passes, which is the
# bar this note set. Wiring C's `pd0_use_src_samples` on the fixed-tree path
# (docs/INTER-ENCODE-PLAN.md §1l) took it 1587 B -> 1589 B, C's exact stream.
# It still separates on its original fix: with the rate arm forced back to
# Allintra the port emits 1630 B against C's 1589.
byteVideoKey "video-key-rate-arm-p9-72x88" gradient 72 88 40 9

# --- video-arm intra EDGE FILTER + the txs ladder, 2026-09-01.
#
# TWO defects, one root: `FunnelCfg::for_preset` baked `edge_filter = (preset
# == 5)` — C's ALLINTRA derivation of `scs->seq_header.enable_intra_edge_filter`
# (enc_mode_config.c:2815) — and ran it on every frame, while
# `speed_config::seq_tools_video` correctly signalled the bit as 1 at EVERY
# preset (C's `else` arm, :2820). So on a video-mode key frame the SEQUENCE
# HEADER told the decoder to edge-filter and upsample directional predictions
# and the encoder predicted UNFILTERED: an encoder/decoder mismatch, not just
# a parity gap. Both now read one function, `intra_arm::intra_edge_filter`.
#
# The second is `frm_hdr->tx_mode`: the writer emitted a literal
# tx_mode_select = 1 (C's allintra rule, "even when txs_level == 0", :10025),
# where the video arm signals it only while `pcs->txs_level != 0` (:9194) —
# false from preset 10 up, so the port declared TX_MODE_SELECT and then coded
# per-block tx_depth symbols TX_MODE_LARGEST forbids.
#
# OBSERVED, frames=2 frame 0 (the video-mode key frame), before -> after,
# measured on ONE build by forcing each fix off:
#   diag 64x64 q40 p11:     port 203 B vs C 401 = 49.377% off -> 398 B = 0.748%
#   gradient 64x64 q40 p11: FH walk 1 diverging field (tx_mode_select C=0
#                           port=1) -> 0; byte count 961 unchanged either way,
#                           so this cell isolates the header bit.
# The p11 ratio limit 2.0 sits between 49.377 and 0.748.
#
# PROMOTED to byteVideoKey 2026-09-01 (the tx_mode-symbol chunk). The ratio form
# was the weaker assertion while the payload was open; it is now byte-identical
# (C 401 B, port 401 B), and the ratioVideoKey doc above says not to leave the
# weaker one in place once the stronger holds. It still witnesses the original
# bug by a mile: with the edge filter forced back to the allintra rule the port
# emits 203 B against C's 401.
byteVideoKey "video-key-edge-filter-diag-p11" diag 64 64 40 11
#
# THIS CELL EARNED ITS KEEP A SECOND TIME, 2026-09-01. The held
# wip/video-md-arms bundle passed all five ratioVideoKey cells and broke THIS
# one, and nobody had re-run it against the bundle. OBSERVED on the bundle head
# 59458226: first diverging FH field `cdef_uv_pri_strength[0]` C=0 port=15
# (C 1024 B, port 1026 B) where main passes. Its coded tree is EXACT there
# (tree_diff: 7 blocks joined, 0 field flips, 0 C-only / 0 port-only), which is
# what said the divergence is downstream of mode decision: C's
# `cdef_recon_ctrls.zero_fs_cost_bias`, unported on BOTH arms. See
# docs/INTER-ENCODE-PLAN.md §1i.
#
# PROMOTED to byteVideoKey 2026-09-01 (the tx_mode-symbol chunk). The header
# half of `frm_hdr->tx_mode` was fixed on 2026-09-01 and the WALK half was not:
# `encode_block_syntax` gated the per-block `tx_size_cdf` symbol on `is_key`
# (the allintra arm's rule) rather than on the bit the header actually wrote, so
# at video preset >= 10 the port announced TX_MODE_LARGEST and then coded one
# tx_depth symbol per block anyway — a stream a decoder cannot parse.
# OBSERVED on ONE build by forcing the gate back to `is_key` (the RDOQ fix
# below left ON, so this isolates the tx_size symbol):
#   gradient 64x64 q40 p11: port 1025 B vs C 1024 -> BYTE-IDENTICAL
#   diag     64x64 q40 p11: port  403 B vs C  401 -> BYTE-IDENTICAL
# With BOTH of the day's fixes forced off — i.e. `main` — the same two cells
# are 1026 B and 403 B.
byteVideoKey "video-key-txs-arm-tx-mode-p11" gradient 64 64 40 11

# --- the RDOQ plane rate weight, 2026-09-01. `svt_av1_optimize_b`'s rdmult is
# `plane_rd_mult[allintra || rtc][is_inter][plane_type]` (full_loop.c:1085), and
# the port hardcoded the ALLINTRA row (17 luma / 13 chroma). A video-mode frame
# takes index 0, where CHROMA is **20** — so C's RDOQ zeroes chroma coefficients
# the port keeps. Luma (17) is the same on both arms, which is why this shows up
# as a chroma-only divergence.
#
# OBSERVED, gradient 64x64 q40 p6 video, frames=2 frame 0, C 961 B: before
# port 965 B (0.416%), after BYTE-IDENTICAL. The port's four coded 32x32 blocks
# already had C's tree, modes, uv modes, angle deltas and LUMA levels; every
# chroma txb differed (C kept at most one DC coefficient of -1, the port kept
# DC + AC). Measured with the C `svt_aom_txb_estimate_coeff_bits` --wrap
# interposer (SVT_CCOEF_OUT) against the port's SVTAV1_PACKTREE_COEFF dump.
byteVideoKey "video-key-rdoq-plane-rd-mult-p6-64x64" gradient 64 64 40 6

# --- the LIGHT-PD0 boundary SHAPE, 2026-09-01. `pd0.rs` priced a one-false
# boundary node as its fitting PART_H/PART_V rectangle only for the LVL_1
# family; LVL_5 (light PD0, the fixed-tree path at preset >= 9) got the SQUARE
# cost, which prices twice the pixels that fit and therefore loses to SPLIT.
# That could not matter on the ALLINTRA arm — `nsq_geom_level` is 0 above M6,
# so an LVL_5 boundary node force-splits before it is costed — and the VIDEO
# arm never turns NSQ geometry off.
#
# C prices the RECTANGLE, measured directly rather than inferred: the
# `svt_aom_full_cost_pd0` --wrap dump (SVT_PD0COST_OUT) on `gradient 72x88 q40
# p9` video reports `32x64`, `16x32` and `8x16` blocks in the x=64 superblock of
# that 72-wide frame and never a square.
#
# OBSERVED, frames=2 frame 0, before -> after on ONE build:
#   screenrep 72x88 q40 p9:  port 2420 B vs C 2402 = 0.749% -> 2405 B = 0.125%
#   screenrep 72x88 q40 p11: port 2438 B vs C 2418 = 0.827% -> 2422 B = 0.165%
#   gradient  72x88 q40 p9:  0.189% -> 0.126%; p10 0.125% -> 0.063%
# The limit 0.5 sits between 0.749 and 0.125.
#
# NOT uniformly closer in BYTES, and the trees say why: gradient 72x88 q40
# p11..p13 move from ~1.04% to ~1.29% off — while that cell's coded tree goes
# from 9 field flips / 7 port-only blocks to **1 flip / 3 port-only**
# (tools/tree_diff.py against C's CTREE, measured both ways). It is the §1f
# pattern — a worse tree that landed nearer in size — so the cells that witness
# this fix are the ones above, not p11.
#
# PROMOTED to byteVideoKey 2026-09-01 by the chunk below, which closed it. The
# ratio form was the weaker assertion while the payload was open. It still
# witnesses the boundary-shape fix by a wide margin: with `prices_edge_shape()`
# forced back to `is_lvl1_family()` the port emits 2420 B against C's 2402.
byteVideoKey "video-key-lpd0-edge-shape-p9-screenrep" screenrep 72 88 40 9

# --- `pd0_use_src_samples` on the FIXED-TREE path, 2026-09-01. C's video PD0
# predicts each block from the RECON it generates per block
# (`ctx->pd0_use_src_samples = allintra || hbd_md`, enc_mode_config.c:7309;
# product_coding_loop.c:8430); the port's LVL_5 predicted from the SOURCE.
#
# This experiment had been run and REJECTED once — over the light-PD0
# boundary-shape defect fixed just above, which was still splitting every edge
# node underneath it. Re-run over the fixed premise it closes six cells.
#
# OBSERVED, 72x88 q40 video frame 0, before -> after on ONE build each side
# (a 45-cell matrix: 28 -> 34 byte-identical, nothing worse):
#   gradient  p11: port 1613 B vs C 1634 = 1.285% -> BYTE-IDENTICAL
#   screenrep p11: port 2422 B vs C 2418 = 0.165% -> BYTE-IDENTICAL
#   gradient  p10: 0.063% -> BYTE-IDENTICAL; screenrep p10 0.125% -> ditto
# p11 is the cell to keep: it is the one whose byte count moved AWAY from C when
# the boundary shape was fixed (1.040% -> 1.285%) while its tree went from 9
# field flips to 1 — so it is the cell that proves the two chunks belong
# together, and it fails loudly if either is reverted.
byteVideoKey "video-key-pd0-recon-pred-p11-72x88" gradient 72 88 40 11
byteVideoKey "video-key-pd0-recon-pred-p11-screenrep" screenrep 72 88 40 11

# --- the VIDEO arm's PALETTE ladder, 2026-09-01. C has a PAIR of palette
# ladders (`enc_mode_config.c:2056-2075` video vs `:2374-2390` allintra) and
# `sc_detect::derive_sc` ran the ALLINTRA one on both arms, by a PORT-NOTE that
# argued it could not move the frame header. That was true and beside the
# point: it moves the TILE, because `palette_level` is what MD searches with.
#
# The M8 row is the cliff — video asks for level 6, allintra for 0 — so a
# video-mode key frame of screen content coded palette blocks in C and NONE in
# the port. A second defect compounded it: `PaletteCtrls::for_level` carried
# only the allintra-reachable rows {0,2,3,4,5,7}, so even a correctly derived
# level 6 fell through to `enabled: false`. All nine C rows are transcribed now,
# and level 1's `cache_based_centroid_refinement` (palette.c:330) with them —
# reached 74 times per frame at video preset 0, counted with a probe.
#
# OBSERVED, screen 72x88 q40 video frame 0, before -> after:
#   p7: C 168 B, port 190 B (13.095%) -> BYTE-IDENTICAL
#   p8: C 179 B, port 911 B (408.939%) -> BYTE-IDENTICAL
# and screen 64x64 q40 p8, C 114 B port 568 B -> BYTE-IDENTICAL (the cell
# promoted from fhVideoKey above). p8 was by a wide margin the worst video-key
# cell in the campaign.
byteVideoKey "video-key-palette-arm-p7-screen-72x88" screen 72 88 40 7
byteVideoKey "video-key-palette-arm-p8-screen-72x88" screen 72 88 40 8

# ---------------------------------------------------------------------------
total=$((pass + fail))
echo
echo "regression spot-check: $pass / $total"
if ((skip)); then
  echo "  SKIPPED (corpus/tool absent — these cells guarded NOTHING this run):"
  printf '    %s\n' "${skipped[@]}"
fi
if ((fail)); then
  echo "  FAILED:"
  printf '    %s\n' "${failed[@]}"
  echo
  echo "  Each line above names the bug it is guarding. Read the comment beside"
  echo "  that cell in this script for the original failure and its fix commit."
fi
((fail == 0))
