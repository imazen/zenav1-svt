#!/usr/bin/env bash
# Function-level C parity against a named oracle, as a RATCHET.
#
# Runs the c_parity suites against ORACLE (`just cparity-oracle` does the same
# with no verdict) and compares the failing set with the pinned list
# `oracles/divergent/<ORACLE>.txt`:
#   - a failure NOT on the list is a regression             -> exit 1;
#   - a listed test that now PASSES must leave the list     -> exit 1;
#   - otherwise                                             -> exit 0.
# So the list only shrinks, and it is always exactly the open divergences.
# Tests the oracle cannot run at all stay in oracles/excludes/<ORACLE>.txt.
#
# Why a ratchet rather than "all green": Ghost Robot parity is being ported
# (plan phase 3). Until it is complete, "all green" is unreachable, and a run
# with no verdict let nothing catch a Ghost Robot arm that broke after it
# landed.
#
# Usage: tools/cparity_ratchet.sh <oracle>     (run it under run-heavy)
# Env:   CARGO_TARGET_DIR (default target/oracle-<oracle> in THIS checkout, as
#        the justfile: a directory shared between jj workspaces runs the other
#        workspace's binaries, because cargo names artifacts by relative path)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
ORACLE=${1:?usage: tools/cparity_ratchet.sh <oracle>}
LIST="oracles/divergent/$ORACLE.txt"
[[ -f "$LIST" ]] || { echo "cparity_ratchet: no pinned list $LIST" >&2; exit 2; }

filt='(binary(/c_parity/) | test(/c_parity/))'
excl="oracles/excludes/$ORACLE.txt"
if [[ -f "$excl" ]]; then
    while IFS= read -r t; do
        case "$t" in ''|'#'*) continue ;; esac
        filt="$filt & not test(/^${t}$/)"
    done <"$excl"
fi
LOG="${TMPDIR:-$HOME/tmp}/cparity_ratchet.$ORACLE.$$.log"
mkdir -p "$(dirname "$LOG")"
SVT_ORACLE=$ORACLE CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/oracle-$ORACLE}" \
    cargo nextest run --no-fail-fast -p zenav1-svt-encoder -p zenav1-svt-dsp \
    -E "$filt" >"$LOG" 2>&1
rc=$?
summary=$(grep -E '^\s+Summary ' "$LOG" | tail -1)
if [[ -z "$summary" ]]; then
    tail -30 "$LOG" >&2
    echo "cparity_ratchet: nextest produced no summary (build failure?) — log: $LOG" >&2
    exit 2
fi
python3 - "$LOG" "$LIST" "$ORACLE" "$summary" <<'PY'
import re, sys
log, lst, oracle, summary = sys.argv[1:5]
failed = set()
for line in open(log, errors="replace"):
    # "        FAIL [   0.007s] (139/875) crate::binary module::test"
    if re.match(r"\s+(?:FAIL|SIGSEGV|SIGABRT|SIGKILL|TIMEOUT)\s+\[", line):
        failed.add(line.split()[-1])
pinned = {l.strip() for l in open(lst) if l.strip() and not l.startswith("#")}
new = sorted(failed - pinned)
fixed = sorted(pinned - failed)
print(f"cparity_ratchet {oracle}: {summary.strip()}; {len(failed)} failing, "
      f"{len(pinned)} pinned divergent")
for t in new:
    print(f"  REGRESSION (not in {lst}): {t}")
for t in fixed:
    print(f"  NOW PASSES — remove from {lst}: {t}")
sys.exit(1 if new or fixed else 0)
PY
