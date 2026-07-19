#!/usr/bin/env bash
# Recon-parity gate: encoder reconstruction must equal aomdec output
# bit-exactly for every stream (mono + 4:2:0 matrix).
set -u
cd "$(dirname "$0")/.."
outdir="${1:-target/recon_parity}"
exec cargo run --release -p zenav1-svt --example recon_parity -- "$outdir"
