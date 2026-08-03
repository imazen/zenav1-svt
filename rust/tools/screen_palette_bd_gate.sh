#!/usr/bin/env bash
# Screen-content palette byte-parity gate — bd8 AND bd10.
#
# WHY THIS EXISTS. Every other synthetic content the identity harness can
# generate (uniform / gradient / diag) is photographic in character, so the
# screen-content detector never arms and NO gate cell could reach the palette
# path. That blind spot let a real defect ship: palette candidates were gated
# out of the bd10 mode-decision funnel entirely (`!bd10_funnel`), so at 10 bits
# the port coded ZERO palette blocks where C codes hundreds. The cost was
# measured, not guessed — `screen 128x128 q32`:
#
#     preset 0   C 327 B   port 664 B    (2.03x)
#     preset 6   C 453 B   port 1110 B   (2.45x)
#
# and on the production corpus it showed up as preset 6 bd10 = 380/515
# byte-identical (vs 515/515 at bd8), with all 135 failures on the eight
# screen-detecting content classes. A gate that cannot reach a feature cannot
# guard it, so this one drives the `screen` content at BOTH depths.
#
# ANTI-VACUITY (enforced in the script, per rust/CLAUDE.md "Gate Discipline"):
# a palette gate that passes because nothing coded a palette is worthless. Each
# cell dumps the port's own partition tree and asserts the frame actually
# CONTAINS palette leaves; a cell that codes none FAILS even if its bytes match.
#
# Usage: screen_palette_bd_gate.sh
# Env:   SP_SIZES SP_QPS SP_PRESETS SP_BDS  (space-separated overrides)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

read -r -a SIZES <<<"${SP_SIZES:-64 128}"
read -r -a QPS <<<"${SP_QPS:-20 32 55}"
# Palette is live at preset <= 7 on sc_class5 content (sc_detect.rs, C
# enc_mode_config.c:2374-2390); above that palette_level is 0, so those presets
# would be vacuous by construction and are deliberately not swept here.
read -r -a PRESETS <<<"${SP_PRESETS:-0 2 4 6}"
read -r -a BDS <<<"${SP_BDS:-8 10}"

OUT="${TMPDIR:-/tmp}/screenpal.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT

pass=0
fail=0
vacuous=0
failed=()
vacuous_cells=()

for bd in "${BDS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        cell="screen_${sz}_q${qp}_p${p}_bd${bd}"

        if ! SVTAV1_BD="$bd" SVTAV1_PACKTREE="$OUT/tree.txt" \
            "$HERE/identity_run" screen "$sz" "$sz" "$qp" "$p" "$OUT/rs" \
            >/dev/null 2>&1; then
          fail=$((fail + 1)); failed+=("${cell}[rs-err]"); continue
        fi
        if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
            "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" "$bd" \
            >/dev/null 2>&1; then
          fail=$((fail + 1)); failed+=("${cell}[c-err]"); continue
        fi

        # Anti-vacuity: this cell must actually exercise palette.
        pal=$(awk '{for (i = 1; i <= NF; i++) if ($i ~ /^pal=/) {
                      split($i, a, "="); if (a[2] > 0) n++
                    }} END {print n + 0}' "$OUT/tree.txt" 2>/dev/null)
        if [[ "${pal:-0}" -eq 0 ]]; then
          vacuous=$((vacuous + 1)); vacuous_cells+=("$cell")
        fi

        if cmp -s "$OUT/c.obu" "$OUT/rs.obu"; then
          pass=$((pass + 1))
        else
          fail=$((fail + 1))
          failed+=("${cell}[C=$(wc -c <"$OUT/c.obu") port=$(wc -c <"$OUT/rs.obu")]")
        fi
      done
    done
  done
done

total=$((pass + fail))
echo "screen-content palette identity: $pass / $total byte-identical"
if ((${#failed[@]})); then
  printf '  FAILED: %s\n' "${failed[@]}"
fi

# A vacuous cell is a DEFECT, not a note: it means the gate would keep passing
# with the palette path deleted. Report every one and fail the gate.
if ((vacuous)); then
  echo "  VACUOUS (no palette leaf coded — these cells guard nothing):"
  printf '    %s\n' "${vacuous_cells[@]}"
  echo "  A palette gate whose cells code no palette is a defect; fix the"
  echo "  content or the preset range rather than accepting the pass."
fi

if ((fail || vacuous)); then
  exit 1
fi
exit 0
