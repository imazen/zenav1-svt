#!/usr/bin/env bash
# Does THE PORT emit the same bytes on aarch64 and x86-64?
#
#   tools/cross_isa_port_check.sh            # default cells
#   CELLS="48 48 20 9|96 80 20 4" tools/cross_isa_port_check.sh
#
# WHY THIS EXISTS. CI runs ONE architecture, so every cross-ISA question has
# been answered by inference. The inference had a hole: tier_invariance.rs walks
# the SIMD tiers present on the host it runs on, which proves nothing about a
# difference that is uniform across tiers on each host and differs between them
# (a per-ISA libm, or a compile-time-selected kernel variant). This runs the
# SAME cells on both and compares hashes — no inference at all.
#
# It needs no C oracle: to ask "is the PORT the variable side?" you only need the
# port's own bytes on two ISAs.
#
# RESULT 2026-08-05: byte-identical on all cells, including the three that
# byte-match C on x86-64 and differ on aarch64. The port is ISA-invariant; C is
# the variable side (docs/SUSPECTED-C-BUGS.md #9).
#
# NOTE the emulated build shares the host `target/` (no --target is passed), so
# it leaves an x86-64 ELF at target/release/examples/identity_run. That is why
# this script rebuilds natively at the end — otherwise the next gate run would
# silently use a foreign binary, or rebuild and look like a spurious cache miss.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

PROFILE=${COLIMA_PROFILE:-x86}
SOCK="$HOME/.colima/$PROFILE/docker.sock"
if [[ ! -S "$SOCK" ]]; then
    echo "cross_isa_port_check: no colima profile '$PROFILE'." >&2
    echo "  colima start --profile $PROFILE --arch x86_64 --cpu 4 --memory 6 --vm-type qemu" >&2
    echo "  (needs: brew install qemu lima-additional-guestagents)" >&2
    exit 3
fi

DEFAULT="48 48 20 9|96 80 20 4|65 65 20 2|96 80 32 6|64 64 20 6"
IFS='|' read -r -a CELL_LIST <<<"${CELLS:-$DEFAULT}"
BD=${SVTAV1_BD:-10}
W=${TMPDIR:-/tmp}/xisa.$$; mkdir -p "$W"; trap 'rm -rf "$W"' EXIT

echo "== building for emulated x86-64 (shares target/, restored at the end) =="
DOCKER_HOST="unix://$SOCK" nice -n 19 docker run --rm -v "$RS_ROOT/..:/src" -w /src/rust \
    rust:1-slim sh -c 'cargo build --release --example identity_run -p zenav1-svt 2>&1 | tail -2' || exit 2
cp target/release/examples/identity_run "$HOME/tmp/identity_run.x86"

echo "== running cells on x86-64 =="
{
    echo 'set -u'
    for c in "${CELL_LIST[@]}"; do
        set -- $c
        echo "SVTAV1_BD=$BD /w/identity_run.x86 gradient $1 $2 $3 $4 /tmp/o >/dev/null 2>&1; printf '%s\t%s\t%s\n' '$1x$2 q$3 p$4' \"\$(wc -c </tmp/o.obu|tr -d ' ')\" \"\$(sha256sum /tmp/o.obu|cut -c1-32)\""
    done
} > "$HOME/tmp/xisa_cells.sh"
DOCKER_HOST="unix://$SOCK" nice -n 19 docker run --rm -v "$HOME/tmp:/w" -w /w rust:1-slim \
    sh /w/xisa_cells.sh > "$W/x86.txt" || exit 2

echo "== rebuilding native and running the same cells =="
nice -n 19 cargo build --release --example identity_run -p zenav1-svt >/dev/null 2>&1 || exit 2
: > "$W/native.txt"
for c in "${CELL_LIST[@]}"; do
    set -- $c
    SVTAV1_BD=$BD ./target/release/examples/identity_run gradient "$1" "$2" "$3" "$4" "$W/n" >/dev/null 2>&1
    printf '%s\t%s\t%s\n' "$1x$2 q$3 p$4" "$(wc -c <"$W/n.obu" | tr -d ' ')" \
        "$(shasum -a 256 "$W/n.obu" | cut -c1-32)" >> "$W/native.txt"
done

echo
paste "$W/native.txt" "$W/x86.txt" | awk -F'\t' '{
    same = ($3 == $6) ? "IDENTICAL" : "*** DIFFERS ***"
    printf "  %-18s aarch64 %6s  x86_64 %6s  %s\n", $1, $2, $5, same
    if ($3 != $6) bad++
} END { exit (bad > 0) }'
rc=$?
echo
if [[ $rc -eq 0 ]]; then
    echo "The PORT is ISA-invariant on these cells. A cell that disagrees with C"
    echo "on only one host is therefore C varying, not the port."
else
    echo "The PORT is ISA-DEPENDENT. This is a shipping bug: every byte-parity"
    echo "claim becomes a per-host claim, and the ISA-scoped pins are papering"
    echo "over it. Bisect by kernel/tier, not by pinning."
fi
exit $rc
