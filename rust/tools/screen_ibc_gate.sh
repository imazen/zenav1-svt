#!/usr/bin/env bash
# IBC chunk 10 gate (docs/ibc-port-map.md §D chunk 10): end-to-end byte
# parity of the IntraBC vertical on real screen content — gb82-sc crops x
# presets 0-4 (the IBC presets: sc_class5 && M<=4) x the qp grid, port vs
# real SVT C, with a full per-cell match MAP.
#
# Semantics (the self-promoting pinned-gate house style):
#   - Cells listed in BYTE_EXACT below are ASSERTED byte-identical — any
#     divergence there is a regression (exit 1).
#   - Cells NOT listed are PINNED-DIVERGING: the gate reports their first
#     divergence (decode-level SB + pixel + per-stream IBC census). If a
#     pinned cell MATCHES, the gate FAILS (exit 4) telling you to PROMOTE
#     it into BYTE_EXACT — a fix must be locked in, never float.
#   - Anti-vacuity: the C streams must genuinely code IntraBC blocks
#     across the sweep (else this gate proves nothing about IBC): exit 3.
#   - Every PORT stream must also be SELF-CONSISTENT (decodes to the
#     port's own recon — the zero-tolerance corruption class): exit 5.
#
# Env: SIG_IMGS / SIG_PRESETS / SIG_QPS / SIG_DIM override the grid.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CT_BIN="$HERE/capture_c_trace/capture_c_trace"
DD_BIN="$HERE/decode_diff/target/release/decode-diff"
SCREEN_DIR="${SCREEN_DIR:-/root/work/codec-corpus/gb82-sc}"
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR

read -r -a IMGS <<<"${SIG_IMGS:-codec_wiki gmessages graph gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a PRESETS <<<"${SIG_PRESETS:-0 1 2 3 4}"
read -r -a QPS <<<"${SIG_QPS:-20 48}"
DIM="${SIG_DIM:-512}"
CELL_TIMEOUT="${SIG_CELL_TIMEOUT:-300}"

# The measured byte-exact set (bake cells in as they close; the gate
# FAILS if a cell here diverges OR an unlisted cell matches).
# Baked 2026-07-23 from the first full run (commit 823955fea state):
# the 20 matching cells are the two !sc_class5 control images (IBC off,
# streams carry 0 intrabc blocks) at every preset/qp. codec_wiki_p1_q48
# had been the one pinned exception (a pre-IBC near-tie, proven with the
# pre-chunk-7 build) — CLOSED 2026-07-23 by the C exchange-sort
# tie-semantics fix (c_exchange_sort_by, leaf_funnel.rs; the ind-uv
# fast-loop SAD-tie survivor cut) and promoted here per the
# self-promotion contract.
#
# 2026-07-24 IBC-pin grind (parity/ibc-pin-grind): +2 sc_class5 cells
# promoted per the self-promotion contract, 20 -> 22 byte-identical:
#   - windows95_p3_q20 (ibc=181): the class-concatenated MDS structure fix
#     (c0a39545f — C never cost-merges candidate classes; the winner-scan
#     strict-< breaks a cross-class exact-RD tie toward the earlier class).
#   - windows95_p4_q20 (ibc=2): the ind_palette_cost_diff CfL-arbitration
#     fix (58afd153b — a luma-palette DC row pays the [1][0] palette-flag
#     context the ind-uv table priced as [0][0]).
# The remaining 78 sc_class5 cells stay pinned-diverging RD near-ties
# (KB-2 family; localizations in benchmarks/screen_ibc_map_2026-07-23.txt).
BYTE_EXACT=(
  "codec_wiki_p0_q20"
  "codec_wiki_p0_q48"
  "codec_wiki_p1_q20"
  "codec_wiki_p1_q48"
  "codec_wiki_p2_q20"
  "codec_wiki_p2_q48"
  "codec_wiki_p3_q20"
  "codec_wiki_p3_q48"
  "codec_wiki_p4_q20"
  "codec_wiki_p4_q48"
  "gmessages_p0_q20"
  "gmessages_p0_q48"
  "gmessages_p1_q20"
  "gmessages_p1_q48"
  "gmessages_p2_q20"
  "gmessages_p2_q48"
  "gmessages_p3_q20"
  "gmessages_p3_q48"
  "gmessages_p4_q20"
  "gmessages_p4_q48"
  "windows95_p3_q20"
  "windows95_p4_q20"
)

is_byte_exact() {
  local t="$1"
  for c in "${BYTE_EXACT[@]:-}"; do [ "$c" = "$t" ] && return 0; done
  return 1
}

echo "priming builds..." >&2
( cd "$RS_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c3 \
    cargo build --release -p zenav1-svt --example identity_run ) >&2 \
  || { echo "port build failed" >&2; exit 2; }
( cd "$HERE/decode_diff" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c3 \
    cargo build --release ) >&2 || { echo "decode-diff build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CT_BIN" ] && [ -x "$DD_BIN" ] || { echo "binaries missing" >&2; exit 2; }

OUT="$RS_ROOT/target/screen_ibc_gate"
mkdir -p "$OUT"
match=0; diff=0; errs=0; c_ibc_total=0; selfdesync=0
declare -a regressions=() promotions=() map_lines=()

ysz=$((DIM * DIM)); csz=$((DIM * DIM / 4))

for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { echo "SKIP-MISSING $img"; continue; }
  for p in "${PRESETS[@]}"; do
    for qp in "${QPS[@]}"; do
      tag="${img}_p${p}_q${qp}"
      d="$OUT/$tag"; mkdir -p "$d"
      if ! SVTAV1_RECON_DUMP="$d/rs" SVTAV1_BD=8 timeout "$CELL_TIMEOUT" nice -n 19 ionice -c3 \
            "$RUN_BIN" "crop:$png" "$DIM" "$DIM" "$qp" "$p" "$d/rs" >/dev/null 2>&1; then
        errs=$((errs+1)); map_lines+=("$tag PORT-ENCODE-ERR"); echo "ERR  $tag port-encode"; continue
      fi
      if ! SVT_NO_AUTO_CMAKE=1 timeout "$CELL_TIMEOUT" nice -n 19 ionice -c3 \
            "$CT_BIN" "$DIM" "$DIM" "$qp" "$p" "$d/rs.yuv" "$d/c.obu" 8 >/dev/null 2>&1; then
        errs=$((errs+1)); map_lines+=("$tag C-ENCODE-ERR"); echo "ERR  $tag c-encode"; continue
      fi
      # self-consistency: the port stream must decode to the port's own recon.
      python3 - "$d/rs.pre.bin" "$d/rs" "$ysz" "$csz" <<'PYEOF'
import sys
d = open(sys.argv[1], "rb").read()
y, c = int(sys.argv[3]), int(sys.argv[4])
open(sys.argv[2] + ".p0", "wb").write(d[:y])
open(sys.argv[2] + ".p1", "wb").write(d[y:y+c])
open(sys.argv[2] + ".p2", "wb").write(d[y+c:y+2*c])
PYEOF
      "$DD_BIN" --vs-raw-prefilter "$d/rs.obu" "$d/rs" >/dev/null 2>&1
      sc_rc=$?
      if [ "$sc_rc" -eq 1 ]; then
        selfdesync=$((selfdesync+1)); map_lines+=("$tag SELF-DESYNC")
        echo "BAD  $tag SELF-DESYNC (port stream does not decode to its own recon)"
        continue
      elif [ "$sc_rc" -ge 2 ]; then
        # The aom-decode oracle refused the stream. Known oracle gap: it
        # rejects some REAL-SVT intrabc streams that libaom 3.14.1 decodes
        # fine (is_dv_valid/dv_ref derivation too strict — reported
        # upstream). Byte-compare below is decode-free, so the cell still
        # gates; only the decode-level diagnostics are n/a.
        map_lines+=("$tag ORACLE-REJECT(port-stream)")
        echo "NOTE $tag oracle cannot decode the port stream (diagnostics n/a)"
      fi
      # IBC censuses (count real intrabc blocks in each stream).
      c_ibc=$("$DD_BIN" --ibc-debug "$d/c.obu" - 2>/dev/null | awk '/^IBC-TOTAL/{print $2}')
      rs_ibc=$("$DD_BIN" --ibc-debug "$d/rs.obu" - 2>/dev/null | awk '/^IBC-TOTAL/{print $2}')
      # An empty census (oracle-reject) reports NA, never 0 — the known
      # aom-decode gap on some REAL-SVT streams must not read as "no IBC".
      c_ibc=${c_ibc:-NA}; rs_ibc=${rs_ibc:-NA}
      [ "$c_ibc" != "NA" ] && c_ibc_total=$((c_ibc_total + c_ibc))
      if cmp -s "$d/rs.obu" "$d/c.obu"; then
        match=$((match+1))
        line="$tag MATCH bytes=$(stat -c%s "$d/rs.obu") ibc=$c_ibc"
        map_lines+=("$line"); echo "OK   $line"
        if ! is_byte_exact "$tag"; then promotions+=("$tag"); fi
      else
        diff=$((diff+1))
        loc=$("$DD_BIN" "$d/c.obu" "$d/rs.obu" 128 2>/dev/null | head -2 | tr '\n' ' ')
        line="$tag DIFF port=$(stat -c%s "$d/rs.obu")B c=$(stat -c%s "$d/c.obu")B c_ibc=$c_ibc rs_ibc=$rs_ibc ${loc}"
        map_lines+=("$line"); echo "DIFF $line"
        if is_byte_exact "$tag"; then regressions+=("$tag"); fi
      fi
      rm -f "$d"/rs.p0 "$d"/rs.p1 "$d"/rs.p2 "$d"/rs.pre.bin "$d"/rs.post.bin "$d"/rs.yuv
    done
  done
done

total=$((match + diff + errs + selfdesync))
echo
echo "==== screen IBC gate map (${DIM}x${DIM} bd8) ===="
printf '%s\n' "${map_lines[@]}"
echo
echo "screen_ibc_gate: $match / $total byte-identical, $diff diverging, $errs errors, $selfdesync self-desync; C IBC blocks total: $c_ibc_total"

rc=0
if [ "$selfdesync" -gt 0 ]; then
  echo "FAIL: $selfdesync port stream(s) corrupt vs their own recon (zero-tolerance)" >&2; rc=5
fi
if [ "$c_ibc_total" -eq 0 ]; then
  echo "ANTI-VACUITY FAIL: C coded no IntraBC block anywhere" >&2; rc=3
fi
if [ "${#regressions[@]}" -gt 0 ]; then
  printf 'REGRESSION (asserted cell diverged): %s\n' "${regressions[@]}" >&2; rc=1
fi
if [ "${#promotions[@]}" -gt 0 ]; then
  printf 'PROMOTE (pinned cell now matches — add to BYTE_EXACT): %s\n' "${promotions[@]}" >&2
  [ "$rc" -eq 0 ] && rc=4
fi
if [ "$errs" -gt 0 ] && [ "$rc" -eq 0 ]; then rc=2; fi
[ "$rc" -eq 0 ] && echo "PASS screen_ibc_gate"
exit "$rc"
