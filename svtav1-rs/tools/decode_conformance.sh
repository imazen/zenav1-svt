#!/usr/bin/env bash
# Decode-conformance gate: encode the (content x size x qp x speed) matrix
# with the Rust encoder, then require the AV1 reference decoder (aomdec) to
# decode every stream. Any failure exits nonzero and lists the streams.
#
# Usage:
#   tools/decode_conformance.sh [outdir]           # mono matrix (default)
#   tools/decode_conformance.sh <outdir> chroma    # 4:2:0 matrix
#
# Env:
#   AOMDEC  path to the aomdec binary (default: `aomdec` on PATH)
set -u
cd "$(dirname "$0")/.."

outdir="${1:-target/decode_conformance}"
mode="${2:-}"
aomdec="${AOMDEC:-aomdec}"

command -v "$aomdec" >/dev/null 2>&1 || {
    echo "aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}

cargo run --release -p svtav1 --example decode_conformance -- "$outdir" $mode \
    >"$outdir.manifest" || {
    echo "encode step failed" >&2
    exit 2
}

pass=0
fail=0
failed=()
for f in "$outdir"/*.obu; do
    if "$aomdec" "$f" -o /dev/null >/dev/null 2>&1; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
        failed+=("$(basename "$f")")
    fi
done

echo "decode conformance${mode:+ ($mode)}: $pass passed, $fail failed"
if [ "$fail" -gt 0 ]; then
    printf '%s\n' "${failed[@]}"
    exit 1
fi
