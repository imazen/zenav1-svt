#!/usr/bin/env bash
# Recon-parity + anti-vacuity gate for the `__expert` chroma delta-q override
# (EncodePipeline::chroma_q_override / AvifEncoder::with_chroma_q_override).
#
# WHAT IT ASSERTS (examples/chroma_q_override_recon.rs):
#   - every cell's encoder reconstruction equals aomdec's output
#     byte-for-byte (and dav1d's when the binary is present) — including
#     U != V overrides, which signal separate_uv_delta_q = 1 on a MAINLINE
#     pipeline, overrides replacing tune-IQ's derived deltas, and the fork;
#   - positive overrides raise chroma error vs the source, and a one-plane
#     override hurts that plane more than the other (exit 2 = VACUOUS);
#   - a monochrome frame with an override is refused, an override that flips
#     U/V separation after the key frame is refused, and a same-separation
#     change mid-sequence encodes and decodes.
#
# LIMIT: no C oracle exists — C has no such override — so this is decoder
# parity plus behaviour, never byte parity. RD/quality of the override is not
# measured here; it is a research-stimulus knob, not a tuning knob.
#
# Usage: tools/chroma_q_override_gate.sh [outdir]
# Env:   AOMDEC (default: aomdec on PATH), DAV1D (dav1d leg when present)
set -u
cd "$(dirname "$0")/.."
outdir="${1:-target/chroma_q_override_recon}"
AOMDEC="${AOMDEC:-$(command -v aomdec || true)}"
if [ -z "$AOMDEC" ]; then
    echo "SKIP: aomdec not on PATH (set AOMDEC)" >&2
    exit 2
fi
export AOMDEC
export DAV1D="${DAV1D:-$(command -v dav1d || true)}"
[ -n "$DAV1D" ] || echo "note: dav1d leg skipped (no binary; set DAV1D)" >&2
mkdir -p "$outdir"
exec cargo run --release -p zenav1-svt --features __expert \
    --example chroma_q_override_recon -- "$outdir"
