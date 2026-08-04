#!/usr/bin/env bash
# COMPREHENSIVE 8-bit byte-parity gate — port vs the real C encoder.
#
# WHY THIS EXISTS. Until now there was NO 8-bit byte-vs-C identity gate in CI at
# any preset. `identity_matrix.sh` is a tracking SCOREBOARD — its own header says
# "Exit 0 always" — and it is not in .github/workflows/rust-gates.yml anyway. So
# every "byte-identical to C" claim about the 8-bit path, which is the port's
# PRIMARY product surface, rested on hand-run measurements that nothing
# re-checked. This is the gate that fixes that.
#
# It differs from identity_matrix.sh in three ways that matter:
#   1. It EXITS NONZERO on an unexpected result. It is a gate, not a report.
#   2. It sweeps the axes the project's own sweep discipline requires — size
#      (tiny/small/medium/large + partial-SB + odd), quality across the WHOLE
#      qp range with low-q density, EVERY preset 0..13, and four content
#      classes including screen content.
#   3. Divergences are PINNED individually and SELF-PROMOTINGLY: a pinned cell
#      that starts MATCHING fails the gate until it is promoted. A fix can never
#      land unnoticed, and a regression can never hide behind a stale exclusion.
#
# TIERS (select with IF_TIER=synthetic|dims|real|all; default synthetic+dims,
# which is what CI runs — `real` needs corpora that are not in-tree):
#
#   synthetic  4 content x 2 sizes x 5 qp x 14 presets = 560 cells   (~4 min)
#   dims       partial-SB / odd / tiny / large geometry sweep        (~4 min)
#   real       CID22 photo + gb82 photo + gb82-sc screen corpora     (~45 min)
#
# Usage:  tools/identity_full_8bit.sh [outfile.tsv]
# Env:    IF_TIER, IF_PRESETS, IF_QPS, IF_SIZES, IF_CONTENTS,
#         IF_REAL_N (images per real corpus, default 6),
#         CID22_DIR / GB82_DIR / SCREEN_DIR (real-tier corpora),
#         IF_CELL_TIMEOUT (default 180s)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"

OUT="${1:-$RS_ROOT/benchmarks/identity_full_8bit_latest.tsv}"
TIER="${IF_TIER:-synthetic dims}"
CELL_TIMEOUT="${IF_CELL_TIMEOUT:-180}"
REAL_N="${IF_REAL_N:-6}"
CID22_DIR="${CID22_DIR:-$HOME/work/zen/codec-corpus/CID22/CID22-512}"
GB82_DIR="${GB82_DIR:-$HOME/work/zen/codec-corpus/gb82}"
SCREEN_DIR="${SCREEN_DIR:-$HOME/work/zen/codec-corpus/gb82-sc}"

RUN="$HERE/identity_run"
CT="$HERE/capture_c_trace/capture_c_trace"
W="${TMPDIR:-/tmp}/idfull.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

# Warm BOTH drivers once, up front.
#
# `identity_run` and `capture_c_trace` are freshness wrappers: each runs its own
# build/relink check on EVERY invocation. That is the staleness contract and
# must not be bypassed — but it means the first cell of a sweep pays a build,
# and, worse, two sweeps running CONCURRENTLY can race on the driver binary and
# make a cell fail for a reason that has nothing to do with parity. (Measured:
# a `[c]` harness error on `uniform 64 64 63 0` while another sweep was mid-run.)
# Warming here collapses the per-cell build to a no-op stat check and makes the
# race window a single serialized step instead of one per cell.
echo "== warming the port and C drivers (freshness check runs per invocation) ==" >&2
$LOWPRI "$RUN" uniform 64 64 40 13 "$W/warm" >/dev/null 2>&1 || true
SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" 64 64 40 13 "$W/warm.yuv" "$W/warm.obu" 8 >/dev/null 2>&1 || true

# ---------------------------------------------------------------------------
# KNOWN-DIVERGING cells. Format: "<content> <w> <h> <qp> <preset>".
# Self-promoting: a listed cell that MATCHES fails the gate (exit 4).
# Every entry must carry a reason and a tracking pointer.
# ---------------------------------------------------------------------------
KNOWN_DIFF=(
  # ---------------------------------------------------------------------
  # screen q48 p7 on two partial-SB geometries — the #71 screen class at a
  # partial superblock. MEASURED 2026-08-03 when p7 was added to the dims
  # tier: p7 is 24/24 on ALIGNED geometry and 34/36 on partial, and these are
  # the only two misses. Both are `screen` at q48, +-0.5%; every `gradient`
  # cell at p7 passes at every geometry. So this is the palette/IBC decision
  # band, not a p7 geometry defect.
  "screen 72 88 48 7"
  "screen 80 88 48 7"
  # ---------------------------------------------------------------------
  # PROMOTED 2026-08-04 — "screen 64 64 63 1" and "screen 128 128 63 1" now
  # byte-match. They were the #71 palette-over-picking pins (C=64B/port=71B and
  # C=185B/port=193B). Root: the MDS3 ind-uv chroma rewrite
  # (leaf_funnel.rs, C `update_intra_chroma_mode` product_coding_loop.c:7095)
  # rebuilt EVERY candidate's uv fast rate with palette_uv_mode_fac_bits[0][0],
  # including luma-palette candidates, which C prices with the [1][0] row
  # (rd_cost.c:518-520, `use_palette_y` off the REAL candidate). That
  # under-costed a palette candidate's chroma flag at every ind_uv_last_mds
  # preset and tipped the palette-vs-regular RD tie. Promoted with the fix,
  # not by loosening.
)
is_known() {
  local n=$1 k
  for k in "${KNOWN_DIFF[@]}"; do [[ "$k" == "$n" ]] && return 0; done
  return 1
}

pass=0; fail=0; pinned=0; promoted=0; errs=0
failed=(); promoted_cells=(); err_cells=()

printf 'content\twidth\theight\tqp\tpreset\tc_bytes\tport_bytes\tverdict\n' >"$OUT"

# cell <content> <w> <h> <qp> <preset>
cell() {
  local content=$1 w=$2 h=$3 qp=$4 p=$5
  local name="$content $w $h $qp $p"
  if ! timeout "$CELL_TIMEOUT" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" \
       >/dev/null 2>&1 &&
     ! timeout "$CELL_TIMEOUT" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" \
       >/dev/null 2>&1; then
    errs=$((errs+1)); err_cells+=("$name[rs]")
    printf '%s\t%s\t%s\t%s\t%s\t-\t-\tRS_ERR\n' "$content" "$w" "$h" "$qp" "$p" >>"$OUT"
    return
  fi
  # Retried ONCE: the only observed harness error was a transient driver-build
  # race. A genuine failure reproduces, and still fails the gate.
  if ! timeout "$CELL_TIMEOUT" env SVT_TRACE_OUT=/dev/null $LOWPRI \
       "$CT" "$w" "$h" "$qp" "$p" "$W/rs.yuv" "$W/c.obu" 8 >/dev/null 2>&1 &&
     ! timeout "$CELL_TIMEOUT" env SVT_TRACE_OUT=/dev/null $LOWPRI \
       "$CT" "$w" "$h" "$qp" "$p" "$W/rs.yuv" "$W/c.obu" 8 >/dev/null 2>&1; then
    errs=$((errs+1)); err_cells+=("$name[c]")
    printf '%s\t%s\t%s\t%s\t%s\t-\t-\tC_ERR\n' "$content" "$w" "$h" "$qp" "$p" >>"$OUT"
    return
  fi
  local cb pb v
  cb=$(wc -c <"$W/c.obu" | tr -d ' ')
  pb=$(wc -c <"$W/rs.obu" | tr -d ' ')
  if cmp -s "$W/c.obu" "$W/rs.obu"; then
    if is_known "$name"; then
      promoted=$((promoted+1)); promoted_cells+=("$name"); v=PROMOTE
    else
      pass=$((pass+1)); v=IDENTICAL
    fi
  elif is_known "$name"; then
    pinned=$((pinned+1)); v=PINNED
  else
    fail=$((fail+1)); failed+=("$name [C=$cb port=$pb]"); v=DIFFERS
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$content" "$w" "$h" "$qp" "$p" "$cb" "$pb" "$v" >>"$OUT"
}

# --- TIER: synthetic -------------------------------------------------------
# EVERY preset 0..13. C clamps all-intra presets above M9 to M9
# (enc_handle.c:4415-4419) but the PORT does not, so 10..13 are genuinely
# distinct configurations here and must be swept, not assumed.
if [[ " $TIER " == *" synthetic "* || " $TIER " == *" all "* ]]; then
  read -r -a S_CONTENTS <<<"${IF_CONTENTS:-uniform gradient diag screen}"
  read -r -a S_SIZES    <<<"${IF_SIZES:-64 128}"
  # Low-q density per the project's sweep discipline: q5 and q12 are where the
  # structural problems hide, and a grid denser at high q than low q is wrong.
  read -r -a S_QPS      <<<"${IF_QPS:-5 20 32 48 63}"
  read -r -a S_PRESETS  <<<"${IF_PRESETS:-0 1 2 3 4 5 6 7 8 9 10 11 12 13}"
  echo "== tier synthetic: ${#S_CONTENTS[@]}c x ${#S_SIZES[@]}sz x ${#S_QPS[@]}q x ${#S_PRESETS[@]}p ==" >&2
  for c in "${S_CONTENTS[@]}"; do for sz in "${S_SIZES[@]}"; do
    for q in "${S_QPS[@]}"; do for p in "${S_PRESETS[@]}"; do
      cell "$c" "$sz" "$sz" "$q" "$p"
    done; done
  done; done
fi

# --- TIER: dims ------------------------------------------------------------
# Tiny -> large, plus partial-SB and ODD geometry. Separates fixed per-frame
# cost from per-pixel behaviour and exercises the edge rules.
if [[ " $TIER " == *" dims "* || " $TIER " == *" all "* ]]; then
  DIMS=(
    "32 32" "48 48" "60 60"          # tiny / sub-SB
    "64 64" "72 88" "80 88" "96 80"  # partial-SB + straddle
    "65 65" "104 72" "120 104"       # odd + both-partial
    "128 128" "192 192" "256 256"    # medium
    "384 256" "512 512"              # large
  )
  # DEFAULT = EVERY preset >= 6, the band at which partial-SB support is
  # CLAIMED. (`partial_sb_gate.sh` has zero cells below preset 6 for the same
  # reason.) Widened from {6,9,13} to all of 6..13 on 2026-08-03 once every
  # preset had been measured individually rather than assumed to follow its
  # neighbours — p8/p10/p11/p12 are 36/36 on partial geometry and 24/24 on
  # aligned, so gating them is a measurement, not a hope.
  #
  # MEASURED 2026-08-03 with p0 and p4 added (benchmarks/
  # identity_full_8bit_dims_2026-08-03.tsv, reproduce with IF_PRESETS="0 4"):
  #   p6/p9/p13  60/60 each — EVERY geometry, including 32x32, odd 65x65,
  #              both-partial 120x104, straddle 80x88, and 512x512
  #   p0         30/60,  p4  34/60
  # and the 56 divergences split cleanly into two ALREADY-KNOWN classes:
  #   53 partial-SB at p0/p4 — presets 0-5 skip the C-faithful PD1 walk on a
  #      non-64-aligned SB entirely (pipeline.rs `refined` requires full_sb),
  #      so the search is structurally different. Tracked as the partial-SB
  #      preset-0..5 restructure in docs/arbitrary-dims-port-map.md.
  #    3 ALIGNED, all `screen` at 256/384/512 p0/p4 — the screen-content
  #      RD class (#71 palette/IBC over-picking at low preset), the same one
  #      the production-corpus sweep sees on its M0 screen classes.
  # No unexplained class. Raise IF_PRESETS when either lands.
  # PRESET 5 ADDED 2026-08-04: it is now 60/60 on this tier (36/36 partial +
  # 24/24 aligned) after the two partial-SB roots were fixed — the walk not
  # running on a partial SB, and a boundary PD0 leaf being refined when C's
  # `tested_blk[PART_N][0]` says it must not be. Gating it is a measurement.
  #
  # p0..p4 are NOT in the default yet, and the reason is the ALIGNED column, not
  # geometry: partial-SB is 36/36 at p0/p1/p2/p3 and 34/36 at p4, but each still
  # misses 1-3 ALIGNED `screen` cells at 256/384/512 — the #71 over-picking RD
  # class. Raise this to `0 1 2 3 4 5 6 ...` when those close, or pin them.
  read -r -a D_PRESETS <<<"${IF_PRESETS:-5 6 7 8 9 10 11 12 13}"
  read -r -a D_QPS     <<<"${IF_QPS:-20 48}"
  echo "== tier dims: ${#DIMS[@]} geometries x ${#D_QPS[@]}q x ${#D_PRESETS[@]}p x 2 content ==" >&2
  for d in "${DIMS[@]}"; do
    set -- $d
    for c in gradient screen; do
      for q in "${D_QPS[@]}"; do for p in "${D_PRESETS[@]}"; do
        cell "$c" "$1" "$2" "$q" "$p"
      done; done
    done
  done
fi

# --- TIER: real ------------------------------------------------------------
# Photographic AND screen corpora at a 512x512 centre crop. Not in CI: the
# corpora are not in-tree. Skips a corpus that is absent, LOUDLY.
if [[ " $TIER " == *" real "* || " $TIER " == *" all "* ]]; then
  read -r -a R_PRESETS <<<"${IF_PRESETS:-0 4 6 10 13}"
  read -r -a R_QPS     <<<"${IF_QPS:-5 20 32 48 63}"
  add_corpus() {
    local label=$1 dir=$2
    if [[ ! -d "$dir" ]]; then
      echo "== tier real: SKIPPING $label — $dir not present ==" >&2
      return
    fi
    local imgs
    # shellcheck disable=SC2207
    imgs=($(find "$dir" -iname '*.png' | sort | head -n "$REAL_N"))
    if ((${#imgs[@]} == 0)); then
      echo "== tier real: SKIPPING $label — no PNGs under $dir ==" >&2
      return
    fi
    echo "== tier real: $label, ${#imgs[@]} images ==" >&2
    for img in "${imgs[@]}"; do
      for q in "${R_QPS[@]}"; do for p in "${R_PRESETS[@]}"; do
        cell "crop:$img" 512 512 "$q" "$p"
      done; done
    done
  }
  add_corpus CID22-photo "$CID22_DIR"
  add_corpus gb82-photo  "$GB82_DIR"
  add_corpus gb82-screen "$SCREEN_DIR"
fi

total=$((pass + fail + promoted))
echo
echo "8-bit identity: $pass / $total byte-identical  (+$pinned pinned, $errs harness errors)"
echo "scoreboard: $OUT"

if ((errs)); then
  echo "  HARNESS ERRORS (neither encoder produced a stream — NOT a parity result):"
  printf '    %s\n' "${err_cells[@]}"
fi
if ((${#promoted_cells[@]})); then
  echo "  PINNED CELLS NOW MATCH — promote them out of KNOWN_DIFF:"
  printf '    %s\n' "${promoted_cells[@]}"
fi
if ((${#failed[@]})); then
  echo "  UNEXPECTED DIVERGENCES:"
  printf '    %s\n' "${failed[@]}"
fi

# Harness errors fail too: a cell that could not run proves nothing, and a
# silently-skipped cell is exactly how a gate rots into a green no-op.
if ((fail || promoted || errs)); then
  exit 1
fi
exit 0
