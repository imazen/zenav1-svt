#!/usr/bin/env bash
# Screen-content IntraBC BYTE gate — the whole gb82-sc IBC band, byte-exact.
#
# The grid is gb82-sc x presets 0..4 (sc_class5 && M<=4 => C's
# `allow_intrabc = 1`; preset 5 is the first IBC-off preset) x qp {20, 40, 48}
# at 512x512 bd8, PLUS the two REAL-IMAGE cells the callcount record
# (benchmarks/callcount_realimg_2026-09-04.meta) found diverging at preset 2
# qp 40 — `terminal.png` 512x512 and `graph.png` 512x480 (the record's exact
# crops; the 512x480 one keeps a non-64-aligned height in the gate). No cell
# on the old `tools/screen_ibc_gate.sh` grid (qp 20/48) covered qp 40, and
# none of the synthetic `screen`/`screenrep` contents arms IntraBC at all
# (docs/WORKING-ON-THIS.md §5: synthetic content never codes an IntraBC
# block) — so those two divergences sat outside every gate.
#
# Byte-ONLY: unlike `screen_ibc_gate.sh` this needs no `tools/decode_diff`
# (whose Cargo.toml has a literal path dependency that exists only on the CI
# image), so it runs on every host that has the C oracle. Its decode-level
# diagnostics are the other gate's; its anti-vacuity is the PORT's pack-tree
# (`SVTAV1_PACKTREE`), which on a byte-identical cell is C's tree too.
#
# Semantics (the self-promoting pinned-gate house style, as screen_ibc_gate):
#   - every cell listed in BYTE_EXACT is ASSERTED byte-identical to C: a
#     divergence there is a regression (exit 1);
#   - a cell NOT listed is PINNED-DIVERGING (exit 0 while it differs); if it
#     MATCHES the gate FAILS (exit 4) telling you to promote it — a fix must
#     be locked in, never float. As of 2026-09-05 EVERY cell is byte-exact
#     (150/150 + the two record cells), so the pinned set is empty;
#   - anti-vacuity (exit 3): the port must code IntraBC blocks AND luma
#     palette blocks somewhere in the sweep, else this gate proves nothing
#     about the screen tools;
#   - the two record cells also assert their SIZE (5003 B / 3098 B): a
#     byte-identical stream of another size would mean the C oracle moved.
#
# History. 2026-07-23: 22/100 byte-exact, 78 pinned "RD near-ties (KB-2
# family)". 2026-09-05: 150/150 after three mechanisms —
#   1. `mds3.rs`: C keys an IntraBC candidate's tx-depth cap on
#      `is_intra_mode(mode)` (DC_PRED) — the INTRA caps, depth 2 at
#      presets 0..3 — where the port used the inter caps (depth 1);
#   2. `commit.rs`: C's MD-side txfm-context stamp is the CHOSEN tx dims for
#      every winner (no skip&&inter arm — that arm is the pack's);
#   3. `context.rs`: C's MD-side context skips the palette CDF update for
#      non-chroma-reference blocks (4x16 at an even column, 16x4 at an even
#      row); the chain sim now withholds that adaptation too.
#
# Env: SIB_IMGS / SIB_PRESETS / SIB_QPS / SIB_DIM override the grid;
#      SIB_RECORD=0 skips the two record cells (they are on by default);
#      RS_AOMDEC=<aomdec> adds a recon leg on the two record cells (the
#      port's stream must decode to the port's own final recon — the
#      zero-tolerance corruption class); without it that leg is reported as
#      SKIPPED, loudly, and the byte verdict stands.
# Exit: 0 PASS; 1 regression; 2 harness error; 3 anti-vacuity; 4 promote.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
. "$(dirname "$0")/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CT_BIN="$HERE/capture_c_trace/capture_c_trace"
SCREEN_DIR="${SCREEN_DIR:-$(corpus_dir codec-corpus/gb82-sc)}"
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR
AOMDEC="${RS_AOMDEC:-}"

read -r -a IMGS <<<"${SIB_IMGS:-codec_wiki gmessages graph gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a PRESETS <<<"${SIB_PRESETS:-0 1 2 3 4}"
read -r -a QPS <<<"${SIB_QPS:-20 40 48}"
DIM="${SIB_DIM:-512}"
RECORD="${SIB_RECORD:-1}"

# The measured byte-exact set. 2026-09-05: the full grid (150) + the two
# record cells. Bake cells in as they close; the gate FAILS if a listed cell
# diverges OR an unlisted cell matches.
BYTE_EXACT=()
for img in codec_wiki gmessages graph gui imac_dark imac_g3 imessage terminal windows windows95; do
  for p in 0 1 2 3 4; do
    for qp in 20 40 48; do BYTE_EXACT+=("${img}_p${p}_q${qp}"); done
  done
done
BYTE_EXACT+=("record_terminal_512x512_q40_p2" "record_graph_512x480_q40_p2")

is_byte_exact() {
  local t="$1"
  for c in "${BYTE_EXACT[@]}"; do [ "$c" = "$t" ] && return 0; done
  return 1
}

echo "priming builds..." >&2
( cd "$RS_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" $LOWPRI \
    cargo build --release -p zenav1-svt --features symtrace --example identity_run ) >&2 \
  || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CT_BIN" ] || { echo "binaries missing" >&2; exit 2; }

OUT="$RS_ROOT/target/screen_ibc_byte_gate"
mkdir -p "$OUT"
match=0; diff=0; errs=0; ibc_total=0; pal_total=0; size_bad=0; recon_fail=0; recon_skipped=0
regressions=(); promotions=(); map_lines=()

# cell <tag> <png> <w> <h> <qp> <preset> [expected-size]
cell() {
  local tag=$1 png=$2 w=$3 h=$4 qp=$5 p=$6 expect=${7:-}
  local d="$OUT/$tag"; mkdir -p "$d"; rm -f "$d/rs.ptree"
  if ! SVTAV1_PACKTREE="$d/rs.ptree" SVTAV1_BD=8 $LOWPRI \
        "$RUN_BIN" "crop:$png" "$w" "$h" "$qp" "$p" "$d/rs" >/dev/null 2>/dev/null; then
    errs=$((errs+1)); map_lines+=("$tag PORT-ENCODE-ERR"); echo "ERR  $tag port-encode"; return
  fi
  if ! SVT_NO_AUTO_CMAKE=1 $LOWPRI \
        "$CT_BIN" "$w" "$h" "$qp" "$p" "$d/rs.yuv" "$d/c.obu" 8 >/dev/null 2>/dev/null; then
    errs=$((errs+1)); map_lines+=("$tag C-ENCODE-ERR"); echo "ERR  $tag c-encode"; return
  fi
  # anti-vacuity census from the port's pack tree (PTREE lines only; the
  # dump carries the chain walk and the real pack, so count unique blocks).
  local ibc pal
  ibc=$(grep '^PTREE' "$d/rs.ptree" 2>/dev/null | awk '!s[$2]++' | grep -c 'ibc=1' || true)
  pal=$(grep '^PTREE' "$d/rs.ptree" 2>/dev/null | awk '!s[$2]++' | grep -cE 'pal=[1-9]' || true)
  ibc_total=$((ibc_total + ${ibc:-0})); pal_total=$((pal_total + ${pal:-0}))
  local rb cb; rb=$(wc -c <"$d/rs.obu" | tr -d ' '); cb=$(wc -c <"$d/c.obu" | tr -d ' ')
  if cmp -s "$d/rs.obu" "$d/c.obu"; then
    match=$((match+1))
    local line="$tag MATCH bytes=$rb ibc=$ibc pal=$pal"
    if [ -n "$expect" ] && [ "$rb" != "$expect" ]; then
      size_bad=$((size_bad+1)); line="$line SIZE-MISMATCH expected=$expect"
    fi
    map_lines+=("$line"); echo "OK   $line"
    is_byte_exact "$tag" || promotions+=("$tag")
  else
    diff=$((diff+1))
    local off; off=$(cmp "$d/rs.obu" "$d/c.obu" 2>/dev/null | awk '{print $5}' | tr -d ,)
    local line="$tag DIFF port=${rb}B c=${cb}B first-byte=${off:-?} ibc=$ibc pal=$pal"
    map_lines+=("$line"); echo "DIFF $line"
    is_byte_exact "$tag" && regressions+=("$tag")
  fi
  rm -f "$d/rs.yuv"
}

# recon_leg <tag> <w> <h>: the port's stream decodes (aomdec) to the port's
# own FINAL recon. Needs the cell's rs.obu and an SVTAV1_FINAL_RECON dump.
recon_leg() {
  local tag=$1 png=$2 w=$3 h=$4 qp=$5 p=$6 d="$OUT/$1"
  if [ -z "$AOMDEC" ] || ! command -v "$AOMDEC" >/dev/null 2>&1; then
    recon_skipped=$((recon_skipped+1)); echo "SKIP $tag recon leg (no RS_AOMDEC)"; return
  fi
  SVTAV1_FINAL_RECON="$d/rs.recon" SVTAV1_BD=8 $LOWPRI \
    "$RUN_BIN" "crop:$png" "$w" "$h" "$qp" "$p" "$d/rs2" >/dev/null 2>/dev/null || { recon_fail=$((recon_fail+1)); echo "BAD  $tag recon leg: port re-encode failed"; return; }
  cmp -s "$d/rs.obu" "$d/rs2.obu" || { recon_fail=$((recon_fail+1)); echo "BAD  $tag recon leg: re-encode not byte-stable"; return; }
  "$AOMDEC" "$d/rs2.obu" -o "$d/rs.y4m" >/dev/null 2>&1 || { recon_fail=$((recon_fail+1)); echo "BAD  $tag recon leg: aomdec refused the stream"; return; }
  local verdict
  verdict=$(python3 - "$d/rs.y4m" "$d/rs.recon" "$w" "$h" <<'PY'
import sys
y4m, recon, w, h = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
d = open(y4m, "rb").read(); hdr = d.index(b"\n"); fp = d.index(b"FRAME", hdr); start = d.index(b"\n", fp) + 1
cw, ch = (w + 1) // 2, (h + 1) // 2; need = w * h + 2 * cw * ch
dec = d[start:start + need]; enc = open(recon, "rb").read()
print("OK" if (len(dec) == need and dec == enc) else f"FAIL {sum(1 for a, b in zip(dec, enc) if a != b)}px")
PY
)
  case "$verdict" in
    OK) echo "OK   $tag recon leg: decode == final recon" ;;
    *) recon_fail=$((recon_fail+1)); echo "BAD  $tag recon leg: $verdict" ;;
  esac
  rm -f "$d/rs2.yuv" "$d/rs.y4m" "$d/rs.recon"
}

for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { echo "SKIP-MISSING $img ($png)"; errs=$((errs+1)); continue; }
  for p in "${PRESETS[@]}"; do
    for qp in "${QPS[@]}"; do
      cell "${img}_p${p}_q${qp}" "$png" "$DIM" "$DIM" "$qp" "$p"
    done
  done
done
if [ "$RECORD" = 1 ]; then
  if [ -f "$SCREEN_DIR/terminal.png" ] && [ -f "$SCREEN_DIR/graph.png" ]; then
    cell record_terminal_512x512_q40_p2 "$SCREEN_DIR/terminal.png" 512 512 40 2 5003
    cell record_graph_512x480_q40_p2 "$SCREEN_DIR/graph.png" 512 480 40 2 3098
    recon_leg record_terminal_512x512_q40_p2 "$SCREEN_DIR/terminal.png" 512 512 40 2
    recon_leg record_graph_512x480_q40_p2 "$SCREEN_DIR/graph.png" 512 480 40 2
  else
    echo "SKIP-MISSING record cells (terminal.png / graph.png)"; errs=$((errs+1))
  fi
fi

total=$((match + diff + errs))
echo
echo "==== screen IBC byte gate map (${DIM}x${DIM} bd8 + record cells) ===="
printf '%s\n' "${map_lines[@]}"
echo
echo "screen_ibc_byte_gate: $match / $total byte-identical, $diff diverging, $errs errors; port IBC blocks: $ibc_total, palette blocks: $pal_total; recon legs: $recon_fail bad, $recon_skipped skipped"

rc=0
if [ "$ibc_total" -eq 0 ] || [ "$pal_total" -eq 0 ]; then
  echo "ANTI-VACUITY FAIL: no IntraBC ($ibc_total) or no palette ($pal_total) block coded anywhere" >&2; rc=3
fi
if [ "${#regressions[@]}" -gt 0 ]; then
  printf 'REGRESSION (asserted cell diverged): %s\n' "${regressions[@]}" >&2; rc=1
fi
if [ "$size_bad" -gt 0 ] || [ "$recon_fail" -gt 0 ]; then
  echo "FAIL: $size_bad record-cell size mismatch(es), $recon_fail recon-leg failure(s)" >&2; rc=1
fi
if [ "${#promotions[@]}" -gt 0 ]; then
  printf 'PROMOTE (pinned cell now matches — add to BYTE_EXACT): %s\n' "${promotions[@]}" >&2
  [ "$rc" -eq 0 ] && rc=4
fi
if [ "$errs" -gt 0 ] && [ "$rc" -eq 0 ]; then rc=2; fi
[ "$rc" -eq 0 ] && echo "PASS screen_ibc_byte_gate"
exit "$rc"
