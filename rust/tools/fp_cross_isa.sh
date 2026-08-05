#!/usr/bin/env bash
# Cross-ISA floating-point determinism check, run LOCALLY under emulation.
#
#   tools/fp_cross_isa.sh          # dump on this host, and on emulated x86-64, diff
#
# WHY. Six cells byte-match C on x86-64 and differ on aarch64. The first
# explanation offered was "C is ISA-dependent", supported by tier_invariance.rs.
# That support has a HOLE: `for_each_token_permutation` walks the tiers present
# on the machine it runs on, so if a transcendental resolves to Apple's libm on
# one host and glibc's on the other, every tier on each host agrees with itself
# and the gate is green on both while the hosts disagree with each other.
#
# This closes the hole by running the same expressions on both, for real.
#
# TWO THINGS THIS GETS RIGHT, both of which a naive version gets wrong:
#   1. `black_box` on every input. With -O and loop-constant inputs LLVM folds
#      these at COMPILE time with its own host-independent evaluator, so a naive
#      dump compares LLVM against itself and reports "identical" no matter what
#      the libms do. Verified the folded and unfolded aarch64 runs agree, so the
#      comparison is of runtime libm calls.
#   2. glibc, not musl. The CI runner is Ubuntu; a musl container would compare
#      the wrong libm and could report a difference that CI never sees.
#
# RESULT 2026-08-05: 402/402 values bit-identical (Apple libm vs glibc).
# Transcendentals are RULED OUT as the cause of the cross-ISA divergence.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${TMPDIR:-/tmp}/fpxisa.$$; mkdir -p "$OUT"; trap 'rm -rf "$OUT"' EXIT

echo "== native ($(uname -m)) =="
rustc -O -o "$OUT/native" "$HERE/fp_cross_isa_dump.rs" || exit 2
"$OUT/native" > "$OUT/native.txt"
echo "  $(wc -l < "$OUT/native.txt" | tr -d ' ') values"

PROFILE=${COLIMA_PROFILE:-x86}
SOCK="$HOME/.colima/$PROFILE/docker.sock"
if [[ ! -S "$SOCK" ]]; then
  echo "  SKIPPED the x86-64 half: no colima profile '$PROFILE' running." >&2
  echo "  Start it with:  colima start --profile $PROFILE --arch x86_64 --vm-type qemu" >&2
  echo "  (this is a LOCAL emulator check; it is not part of CI)" >&2
  exit 3
fi
echo "== emulated x86_64 (glibc) =="
cp "$HERE/fp_cross_isa_dump.rs" "$HOME/tmp/" 2>/dev/null || true
DOCKER_HOST="unix://$SOCK" docker run --rm -v "$HOME/tmp:/w" -w /w rust:1-slim \
    sh -c 'rustc -O -o /tmp/x /w/fp_cross_isa_dump.rs && /tmp/x' > "$OUT/x86.txt" || exit 2
echo "  $(wc -l < "$OUT/x86.txt" | tr -d ' ') values"

if diff -q "$OUT/native.txt" "$OUT/x86.txt" >/dev/null; then
  echo "IDENTICAL — every transcendental agrees bit-for-bit across ISAs"
  exit 0
fi
echo "DIFFERENCES — these expressions are NOT portable:"
diff "$OUT/native.txt" "$OUT/x86.txt" | grep '^[<>]' | head -40
exit 1
