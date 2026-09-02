#!/usr/bin/env bash
# The INTER campaign's 96-cell FRONTIER matrix.
#
# `inter_byte_gate.sh` asserts the cells that are byte-identical, and lists four
# named open ones. It deliberately does NOT sweep, because a gate that walks its
# own frontier takes minutes and gets skipped. This is the sweep behind it: the
# full grid whose counts §1z'' quotes (19 identical / 65 f0-only / 12 f0-differ).
#
# WHY IT IS COMMITTED. §1z, §1z' and §1z'' each re-derived this loop by hand in
# a scratch dir, and `docs/WORKING-ON-THIS.md` §5's "silent harness" trap is
# exactly what an inline loop invites. It prints ONE LINE PER CELL, always, so a
# cell that never ran is visible, and it writes a TSV that can be diffed against
# the previous chunk's.
#
# The three verdicts are the campaign's three states, and the ORDER matters:
#   BOTH    both frames byte-identical
#   F1DIFF  frame 0 identical, frame 1 differs  -> an INTER-decision defect
#   F0DIFF  frame 0 already differs             -> a video-KEY defect, and every
#           frame-1 reading downstream of it is meaningless
#
# Usage: tools/inter_byte_matrix.sh [outdir]
# Env: IBM_CONTENT / IBM_SIZES / IBM_QPS / IBM_PRESETS / IBM_FRAMES / IBM_SHIFT
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

CONTENTS="${IBM_CONTENT:-uniform gradient diag screen}"
SIZES="${IBM_SIZES:-16 64 72 128}"
QPS="${IBM_QPS:-20 40 55}"
PRESETS="${IBM_PRESETS:-6 8}"
FRAMES="${IBM_FRAMES:-2}"
SHIFT="${IBM_SHIFT:-3}"
OUT="${1:-$RS_ROOT/target/inter-byte-matrix}"
mkdir -p "$OUT"

printf 'content\tsize\tqp\tpreset\tc_f0\tp_f0\tc_f1\tp_f1\tverdict\n'
both=0; f1d=0; f0d=0; broke=0
for content in $CONTENTS; do
  for s in $SIZES; do
    for q in $QPS; do
      for p in $PRESETS; do
        d="$OUT/${content}_${s}x${s}_q${q}_p${p}"
        mkdir -p "$d"
        SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="$SHIFT" \
          "$HERE/identity_diff_inter.sh" "$s" "$s" "$q" "$p" "$FRAMES" "$content" "$d" \
          >"$d/diff.txt" 2>&1
        st=$?
        if [[ $st -eq 3 || ! -s "$d/c.obu.pts0" || ! -s "$d/rs.obu.f0" ]]; then
          printf '%s\t%s\t%s\t%s\t-\t-\t-\t-\tHARNESS\n' "$content" "$s" "$q" "$p"
          broke=$((broke+1)); continue
        fi
        cf0=$(wc -c <"$d/c.obu.pts0" | tr -d ' ')
        pf0=$(wc -c <"$d/rs.obu.f0" | tr -d ' ')
        cf1=$(wc -c <"$d/c.obu.pts1" 2>/dev/null | tr -d ' '); cf1=${cf1:-0}
        pf1=$(wc -c <"$d/rs.obu.f1" 2>/dev/null | tr -d ' '); pf1=${pf1:-0}
        if ! cmp -s "$d/c.obu.pts0" "$d/rs.obu.f0"; then
          v=F0DIFF; f0d=$((f0d+1))
        elif cmp -s "$d/c.obu.pts1" "$d/rs.obu.f1"; then
          v=BOTH; both=$((both+1))
        else
          v=F1DIFF; f1d=$((f1d+1))
        fi
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
          "$content" "$s" "$q" "$p" "$cf0" "$pf0" "$cf1" "$pf1" "$v"
      done
    done
  done
done
total=$((both+f1d+f0d+broke))
printf '# %d BOTH / %d F1DIFF / %d F0DIFF / %d HARNESS of %d\n' \
  "$both" "$f1d" "$f0d" "$broke" "$total"
[[ $broke -eq 0 ]]
