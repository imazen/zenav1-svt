#!/usr/bin/env bash
# Does the port's INTER stream DECODE — on every cell of the campaign grid?
#
# `inter_decode_gate.sh` asks that of FIVE named cells. This asks it of all
# 96, and the difference is not academic: 22 of them produce a stream a
# decoder REJECTS, and the five never saw one.
#
# WHY IT EXISTS. MEASURED 2026-09-02: `aomdec` reports "Failed to decode tile
# data" on 22 of the 96 cells, and every single one is PRESET 6. The cause is
# named and traced (docs/INTER-ENCODE-PLAN.md §1z¹⁸): at p6 the frame header
# carries `allow_warped_motion = 1` (at p8 it is 0), so C's
# `motion_mode_allowed` promotes a block with overlappable neighbours to
# WARPED_CAUSAL and writes the motion mode from the THREE-symbol
# `MOTION_MODE_CDF`; the port's `num_proj_ref` is always 0 because
# `av1_find_samples` is unported, so it writes the TWO-symbol `OBMC_CDF`
# instead. The DECODER derives the sample count itself and reads three
# symbols where the port wrote two — an arithmetic-coder desync, not a
# quality difference. A byte gate cannot see the difference between "wrong
# bytes" and "bytes no decoder will accept"; this can.
#
# THE PIN IS EXACT, and it is now EMPTY. It held 22 cells for the length of
# one commit: this gate landed with the defect measured and pinned,
# `av1_find_samples` was ported in the next change, and the gate REFUSED to
# let that land quietly — it failed with "22 pinned cell(s) now decoding",
# which is the direction a pin has to work in if it is going to mean
# anything. All 96 streams decode.
# A cell that starts failing is a conformance regression; there is no count
# to nudge.
#
# The comparison is restricted to the cells this run actually SWEPT, and that
# is load-bearing rather than tidy: without it, narrowing the grid with the
# IBM_* vars reports every unswept pinned cell as "now decoding" and the gate
# fails for a reason that is purely an artefact of its own scope. Found by
# mutation-testing this script before it landed — the mutation that was
# supposed to prove ONE arm printed "21 now decoding" and proved a bug
# instead.
#
# Usage: tools/inter_decode_census.sh [outdir]
# Env: RS_AOMDEC (decoder path), plus the grid vars inter_byte_matrix.sh uses.
set -uo pipefail
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

AOMDEC="${RS_AOMDEC:-$(command -v aomdec || true)}"
if [[ -z "$AOMDEC" || ! -x "$AOMDEC" ]]; then
    echo "inter decode census: FAIL — no aomdec (set RS_AOMDEC)." >&2
    echo "  A decodability gate with no decoder is a harness failure, never a pass." >&2
    exit 2
fi

CONTENTS="${IBM_CONTENT:-uniform gradient diag screen}"
SIZES="${IBM_SIZES:-16 64 72 128}"
QPS="${IBM_QPS:-20 40 55}"
PRESETS="${IBM_PRESETS:-6 8}"
OUT="${1:-$RS_ROOT/target/inter-decode-census}"
mkdir -p "$OUT"

# The cells whose stream a decoder REJECTS, by name — EMPTY. What was here:
# 22 cells, every one preset 6, which is where `allow_warped_motion` is 1.
# The port wrote the two-symbol OBMC motion-mode symbol where the decoder
# reads the three-symbol MOTION_MODES one, because `num_proj_ref` was always
# zero. `av1_find_samples` is ported now and all 96 streams decode.
#
# Keep the format if a cell ever has to go back in: one
# `<content>_<w>x<h>_q<q>_p<p>` per line.
KNOWN_UNDECODABLE=""

: >"$OUT/observed.txt"
: >"$OUT/swept.txt"
ok=0; bad=0; harness=0
echo "== inter decode census =="
for content in $CONTENTS; do
  for s in $SIZES; do
    for q in $QPS; do
      for p in $PRESETS; do
        name="${content}_${s}x${s}_q${q}_p${p}"
        d="$OUT/$name"
        mkdir -p "$d"
        echo "$name" >>"$OUT/swept.txt"
        SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="${IBM_SHIFT:-3}" \
          "$HERE/identity_diff_inter.sh" "$s" "$s" "$q" "$p" "${IBM_FRAMES:-2}" "$content" "$d" \
          >"$d/diff.txt" 2>&1
        st=$?
        if [[ $st -eq 4 ]]; then
            echo "  CRASH  $name — the encoder PANICKED"
            harness=$((harness + 1)); continue
        fi
        if [[ ! -s "$d/rs.obu" ]]; then
            echo "  HARNESS  $name — the port produced no stream"
            harness=$((harness + 1)); continue
        fi
        if "$AOMDEC" --summary -o /dev/null "$d/rs.obu" >/dev/null 2>&1; then
            ok=$((ok + 1))
        else
            bad=$((bad + 1)); echo "$name" >>"$OUT/observed.txt"
        fi
      done
    done
  done
done

# Restrict the pin to what this run swept — see the header.
sort -u "$OUT/swept.txt" >"$OUT/swept.sorted.txt"
printf '%s\n' "$KNOWN_UNDECODABLE" | grep -v '^[[:space:]]*$' | sort \
    | comm -12 - "$OUT/swept.sorted.txt" >"$OUT/pinned.txt"
sort "$OUT/observed.txt" >"$OUT/observed.sorted.txt"
mapfile -t NEW  < <(comm -13 "$OUT/pinned.txt" "$OUT/observed.sorted.txt")
mapfile -t GONE < <(comm -23 "$OUT/pinned.txt" "$OUT/observed.sorted.txt")

# bash 3.2 (`/bin/bash` on macOS) treats an EMPTY array as unset under `set -u`;
# count through the `[@]+` guard so the report survives a clean run.
n_new=${NEW[@]+${#NEW[@]}}; n_new=${n_new:-0}
n_gone=${GONE[@]+${#GONE[@]}}; n_gone=${n_gone:-0}
if [[ $n_new -gt 0 ]]; then
    echo "-- streams a decoder REJECTS that are NOT pinned --"
    printf '  %s\n' ${NEW[@]+"${NEW[@]}"}
fi
if [[ $n_gone -gt 0 ]]; then
    echo "-- pinned cells that now DECODE (remove them from KNOWN_UNDECODABLE) --"
    printf '  %s\n' ${GONE[@]+"${GONE[@]}"}
fi
echo "-- known-undecodable (pinned) --"
while read -r row; do [[ -n "$row" ]] && echo "  open  $row"; done <"$OUT/pinned.txt"

echo
echo "inter decode census: $((ok + bad)) streams, $ok decode, $bad rejected \
($(wc -l <"$OUT/pinned.txt" | tr -d ' ') pinned, $n_new unexpected, $n_gone now decoding), \
$harness harness"
# ANTI-VACUITY: a run that produced no decodable stream at all asserts nothing.
if [[ $ok -eq 0 ]]; then
    echo "inter decode census: FAIL — ZERO streams decoded, so the pin below" >&2
    echo "  asserts nothing. Anti-vacuity, not a parity result." >&2
    exit 1
fi
if [[ $harness -gt 0 ]]; then
    echo "inter decode census: FAIL — $harness harness failure(s)/crash(es)." >&2
    exit 1
fi
if [[ $n_new -gt 0 || $n_gone -gt 0 ]]; then
    echo "inter decode census: FAIL — $n_new unpinned rejection(s), $n_gone pinned" >&2
    echo "  cell(s) now decoding. The pin is EXACT: a NEW undecodable stream is a" >&2
    echo "  conformance regression, and one that starts decoding means the defect" >&2
    echo "  moved and KNOWN_UNDECODABLE must shrink in the same commit." >&2
    exit 1
fi
echo "inter decode census: PASS"
