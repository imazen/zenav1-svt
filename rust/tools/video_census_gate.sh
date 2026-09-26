#!/usr/bin/env bash
# Real-video byte-parity RATCHET against mainline C (plan 3.8).
#
# WHY THIS GATE EXISTS. The 2026-09-26 census (tools/inter_byte_matrix.sh
# over six public-domain derf clips x {128,256} x qp {20,40,55} x presets
# -1..13, 8 low-delay frames) found 244 of 540 cells byte-identical to C
# through every frame — but only the 2-frame p6/p8 cells were pinned
# (real_video_inter_gate.sh). Parity work at other presets could regress the
# rest unnoticed, and a closed cell had nowhere to land.
#
# WHAT IT ASSERTS. Every cell's verdict — IDENTICAL, or the first temporal
# unit that differs (TU<k>) — equals the pinned one in
# tools/pins/video_census.tsv. A cell that differs EARLIER than pinned is a
# regression; one that differs LATER, or became IDENTICAL, is an improvement
# and also fails until the pin moves with it (`VCG_WRITE=1` rewrites the
# pins; say what moved in the commit). An ERROR cell always fails.
#
# Usage: tools/video_census_gate.sh     Env: VCG_JOBS (4), VCG_WRITE, AOMDEC n/a
# ~320 s on i265 at 4 jobs.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
PINS="$HERE/pins/video_census.tsv"
work="${TMPDIR:-$HOME/tmp}/video-census.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

IBM_CONTENT="fourpeople johnny kristenandsara vidyo1 vidyo3 vidyo4" \
IBM_SIZES="128 256" IBM_QPS="20 40 55" \
IBM_PRESETS="-1 0 1 2 3 4 5 6 7 8 9 10 11 12 13" IBM_FRAMES=8 \
IBM_JOBS="${VCG_JOBS:-4}" \
    "$HERE/inter_byte_matrix.sh" "$work/cells" >"$work/now.tsv" 2>"$work/err.txt"
rc=$?
if [ ! -s "$work/now.tsv" ]; then
    tail -20 "$work/err.txt" >&2
    echo "video census gate: the sweep produced nothing (rc=$rc)" >&2
    exit 2
fi
if [ "${VCG_WRITE:-0}" = 1 ]; then
    grep -v '^#' "$work/now.tsv" | cut -f1-5,8 >"$PINS"
    echo "video census gate: wrote $(($(wc -l <"$PINS") - 1)) pins to $PINS"
    exit 0
fi
python3 - "$PINS" "$work/now.tsv" <<'PY'
import csv, sys
def load(p):
    lines = [l for l in open(p) if not l.startswith("#")]
    head = next(i for i, l in enumerate(lines) if l.startswith("content\t"))
    rows = list(csv.DictReader(lines[head:], delimiter="\t"))
    return {(r["content"], r["size"], r["qp"], r["preset"], r["frames"]): r["verdict"] for r in rows}
def rank(v):  # later divergence is better; IDENTICAL best; ERROR worst
    return 1000 if v == "IDENTICAL" else (-1 if v == "ERROR" else int(v[2:]))
pins, now = load(sys.argv[1]), load(sys.argv[2])
worse, better, missing = [], [], sorted(set(pins) - set(now))
for k, v in sorted(now.items()):
    p = pins.get(k)
    if p is None or rank(v) < rank(p) or v == "ERROR":
        worse.append((k, p, v))
    elif rank(v) > rank(p):
        better.append((k, p, v))
for k, p, v in worse:
    print(f"  REGRESSED  {' '.join(k)}: pinned {p}, now {v}")
for k, p, v in better:
    print(f"  PROMOTED   {' '.join(k)}: pinned {p}, now {v} (move the pin: VCG_WRITE=1)")
for k in missing:
    print(f"  NOT RUN    {' '.join(k)}")
ident = sum(v == "IDENTICAL" for v in now.values())
print(f"video census gate: {ident}/{len(now)} identical; {len(worse)} regressed, "
      f"{len(better)} promoted, {len(missing)} not run")
sys.exit(1 if worse or better or missing else 0)
PY
