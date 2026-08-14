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
