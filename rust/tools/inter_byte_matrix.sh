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
#   CRASH   the port PANICKED                   -> not a divergence at all
#   F1DIFF  frame 0 identical, frame 1 differs  -> an INTER-decision defect
#   F0DIFF  frame 0 already differs             -> a video-KEY defect, and every
#           frame-1 reading downstream of it is meaningless
#
# CRASH IS NOT F1DIFF, AND IT USED TO BE. MEASURED 2026-09-02: the port
# panicked on EIGHTEEN 72x72 cells (`md_search.rs`'s source read, off the end
# of an unpadded plane on a partial superblock) and this script reported every
# one of them as F1DIFF, because frame 0 had already been written when the
# frame-1 panic hit and the only crash check here was "does rs.obu.f0 exist".
# §1z15's "55 F1DIFF cells" was therefore 37 divergences and 18 crashes. A
# crash gets its own column and its own nonzero exit.
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
both=0; f1d=0; f0d=0; broke=0; crash=0
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
        # A CRASH first: `identity_diff_inter.sh` exits 4 for it, and the test
        # must come BEFORE the file checks, because a frame-1 panic leaves a
        # perfectly good rs.obu.f0 behind and would otherwise be scored.
        if [[ $st -eq 4 ]]; then
          printf '%s\t%s\t%s\t%s\t-\t-\t-\t-\tCRASH\n' "$content" "$s" "$q" "$p"
          crash=$((crash+1)); continue
        fi
        if [[ $st -eq 3 || ! -s "$d/c.obu.pts0" || ! -s "$d/rs.obu.f0" ]]; then
          printf '%s\t%s\t%s\t%s\t-\t-\t-\t-\tHARNESS\n' "$content" "$s" "$q" "$p"
          broke=$((broke+1)); continue
        fi
        cf0=$(wc -c <"$d/c.obu.pts0" | tr -d ' ')
        pf0=$(wc -c <"$d/rs.obu.f0" | tr -d ' ')
        # A MISSING frame-1 file is a legitimate state (the port refused, or a
        # CONTROL such as SVTAV1_PD0_NOSPLIT made the cell unencodable), so ask
        # before reading. `wc -c < missing` writes to STDERR, and this script's
        # stdout IS the TSV: without the guard those lines land INSIDE the
        # table and every keyed join against it silently mis-aligns. MEASURED
        # 2026-09-02 on the PD0_NOSPLIT control run.
        cf1=0; [[ -f "$d/c.obu.pts1" ]] && cf1=$(wc -c <"$d/c.obu.pts1" | tr -d ' ')
        pf1=0; [[ -f "$d/rs.obu.f1" ]] && pf1=$(wc -c <"$d/rs.obu.f1" | tr -d ' ')
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
total=$((both+f1d+f0d+broke+crash))
printf '# %d BOTH / %d F1DIFF / %d F0DIFF / %d CRASH / %d HARNESS of %d\n' \
  "$both" "$f1d" "$f0d" "$crash" "$broke" "$total"
# A crash is a DEFECT, not a frontier state: this sweep fails on one, the same
# way it fails on a broken harness.
[[ $broke -eq 0 && $crash -eq 0 ]]
