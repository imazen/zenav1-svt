#!/usr/bin/env bash
# Still-grid byte-parity RATCHET for a named C oracle (plan 3.3s).
#
# tools/oracle_still_grid.sh is a scoreboard: it records how far the port is
# from an oracle and always exits 0. For an oracle being ported to (Ghost
# Robot: 41/288 identical on 2026-09-26) that leaves the identical cells
# unguarded — a change can lose one and nothing notices — and gives a
# closed cell nowhere to land. This pins every cell's verdict in
# tools/pins/still_grid.<oracle>.tsv: an IDENTICAL cell that starts to
# differ fails as REGRESSED; a DIFFERS cell that becomes IDENTICAL fails as
# PROMOTED until the pin moves (`SGG_WRITE=1`, and say which cells in the
# commit). An ERROR cell always fails.
#
# Usage: tools/still_grid_gate.sh <oracle>     Env: SGG_WRITE, OSG_JOBS (4)
# ~30 s for Ghost Robot on i265 at 4 jobs.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ORACLE="${1:?usage: still_grid_gate.sh <oracle>}"
PINS="$HERE/pins/still_grid.$ORACLE.tsv"
work="${TMPDIR:-$HOME/tmp}/still-grid-gate.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

OSG_JOBS="${OSG_JOBS:-4}" "$HERE/oracle_still_grid.sh" "$ORACLE" "$work/now.tsv" >"$work/log.txt" 2>&1
if [ ! -s "$work/now.tsv" ]; then
    tail -20 "$work/log.txt" >&2
    echo "still grid gate: the grid produced nothing" >&2
    exit 2
fi
if [ "${SGG_WRITE:-0}" = 1 ]; then
    cut -f2-7 "$work/now.tsv" >"$PINS"
    echo "still grid gate: wrote $(($(wc -l <"$PINS") - 1)) pins to $PINS"
    exit 0
fi
python3 - "$PINS" "$work/now.tsv" "$ORACLE" <<'PY'
import csv, sys
pins_path, now_path, oracle = sys.argv[1:4]
key = ("content", "size", "preset", "qp", "bd")
def load(p):
    return {tuple(r[k] for k in key): r["verdict"] for r in csv.DictReader(open(p), delimiter="\t")}
pins, now = load(pins_path), load(now_path)
worse = sorted(k for k, v in now.items() if v == "ERROR" or (pins.get(k) == "IDENTICAL" and v != "IDENTICAL") or k not in pins)
better = sorted(k for k, v in now.items() if v == "IDENTICAL" and pins.get(k) == "DIFFERS")
missing = sorted(set(pins) - set(now))
for k in worse:
    print(f"  REGRESSED  {' '.join(k)}: pinned {pins.get(k)}, now {now[k]}")
for k in better:
    print(f"  PROMOTED   {' '.join(k)}: pinned DIFFERS, now IDENTICAL (move the pin: SGG_WRITE=1)")
for k in missing:
    print(f"  NOT RUN    {' '.join(k)}")
ident = sum(v == "IDENTICAL" for v in now.values())
print(f"still grid gate {oracle}: {ident}/{len(now)} identical; {len(worse)} regressed, "
      f"{len(better)} promoted, {len(missing)} not run")
sys.exit(1 if worse or better or missing else 0)
PY
