#!/usr/bin/env bash
# Every cheap static check CI runs, in one command, before you push.
#
# Each of these is its own CI step, and each one has gone red on main because
# nobody ran it locally first (2026-09-26: the refusal ledger went stale when a
# refusal was deleted). None of them encodes anything; the whole set takes
# seconds after the clippy build is warm.
#
# Usage: tools/precheck.sh        (also `just precheck`)
# Exit:  0 all current, 1 one or more stale (each failure is printed)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
fail=()
run() { # label command...
    local label=$1; shift
    if "$@" >"${TMPDIR:-$HOME/tmp}/precheck.$$.log" 2>&1; then
        printf '  ok     %s\n' "$label"
    else
        printf '  STALE  %s\n' "$label"
        tail -8 "${TMPDIR:-$HOME/tmp}/precheck.$$.log" | sed 's/^/         /'
        fail+=("$label")
    fi
}
mkdir -p "${TMPDIR:-$HOME/tmp}"
run "cargo fmt --all --check"          cargo fmt --all --check
run "clippy inventory"                 tools/clippy_inventory.sh --check
run "refusal ledger (REFUSED-CONFIGS)" bash tools/refusal_inventory.sh --check
run "PORT-NOTE index"                  tools/portnote_index.sh --check
run "test targets"                     python3 tools/test_targets_check.py
run "SIMD tier ledger"                 python3 tools/review/incant_tiers.py --check docs/INCANT-TIER-GAPS.tsv
run "dead-code ledger"                 python3 tools/dead_code_ledger.py
run "file sizes (<= 3000 lines)"       python3 tools/file_size_check.py
run "env names have readers"           python3 tools/env_names_check.py
run "CI shard partition"               python3 tools/ci_shard_check.py
rm -f "${TMPDIR:-$HOME/tmp}/precheck.$$.log"
if [ "${#fail[@]}" -gt 0 ]; then
    echo "precheck: ${#fail[@]} stale: ${fail[*]}"
    exit 1
fi
echo "precheck: all current"
