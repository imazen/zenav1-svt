#!/usr/bin/env bash
# 4:4:4 DECODER-EQUALITY GATE — the witness for ChromaFormat::Yuv444.
#
# WHAT IT ASSERTS: for each cell, `examples/probe_444` encodes a key frame at
# 4:4:4, aomdec decodes it, and the decoded Y/U/V planes equal the encoder's
# own reconstruction BYTE-FOR-BYTE. 4:4:4 has no C oracle (SVT-AV1 refuses
# non-4:2:0 at verify_settings, enc_settings.c:470), so this gate is the
# entire parity claim — "decodes" is NOT enough, the pixels must match.
#
# Coverage mirrors the landing: aligned + non-aligned dims (36..200 px
# square exercises SB-edge clipping and multi-TXB chroma blocks), lossy and
# coded-lossless qindex-0 (the WHT transform arm in encode_chroma_block_dc).
#
# RD/SSIM2 quality is a separate measurement — tools/rd_ext_sweep.sh.
#
# Usage: tools/chroma_444_gate.sh
# Env:   AOMDEC, DAV1D (dav1d leg runs when the binary is present)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
AOMDEC=${AOMDEC:-$(command -v aomdec || true)}
DAV1D=${DAV1D:-$(command -v dav1d || true)}
PROBE="$RS/target/release/examples/probe_444"
W=$(mktemp -d /tmp/g444.XXXXXX)
trap 'rm -rf "$W"' EXIT

if [ -z "$AOMDEC" ]; then
  echo "SKIP: aomdec not on PATH" >&2; exit 2
fi
if [ ! -x "$PROBE" ]; then
  (cd "$RS" && cargo build -q --release -p zenav1-svt --example probe_444) \
    || { echo "FAIL: probe_444 build" >&2; exit 1; }
fi

pass=0; fail=0; skipped=0; declare -a failed skipped_msgs

check() { # label size qp
  local label=$1 w=$2 qp=$3
  if ! "$PROBE" "$W/c" "$w" "$qp" >"$W/c.log" 2>&1; then
    if grep -q REFUSED "$W/c.log"; then
      fail=$((fail+1)); failed+=("$label REFUSED (envelope moved?)")
    else
      fail=$((fail+1)); failed+=("$label encode-fail")
    fi
    return
  fi
  if ! "$AOMDEC" --rawvideo -o "$W/c.dec" "$W/c/probe.obu" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label aomdec-reject"); return
  fi
  # recon.raw layout: luma at true stride w, then U+V at 8-aligned stride.
  if ! python3 - "$w" "$W/c.dec" "$W/c/recon.raw" <<'PY'
import sys
w = int(sys.argv[1])
aw = (w + 7) // 8 * 8
d = open(sys.argv[2], 'rb').read()
r = open(sys.argv[3], 'rb').read()
rec_u = r[w*w : w*w + aw*aw]
rec_v = r[w*w + aw*aw : w*w + 2*aw*aw]
dec_u = d[w*w : 2*w*w]
dec_v = d[2*w*w : 3*w*w]
def cd(dd, rr):
    n = 0
    for row in range(w):
        for c in range(w):
            if dd[row*w + c] != rr[row*aw + c]:
                n += 1
    return n
sys.exit(0 if d[:w*w] == r[:w*w] and cd(dec_u, rec_u) == 0 and cd(dec_v, rec_v) == 0 else 1)
PY
  then
    fail=$((fail+1)); failed+=("$label recon!=aomdec"); return
  fi
  if [ -n "$DAV1D" ]; then
    if ! "$DAV1D" -i "$W/c/probe.obu" -o "$W/c.dd.yuv" >/dev/null 2>&1; then
      fail=$((fail+1)); failed+=("$label dav1d-reject"); return
    fi
    if ! cmp -s "$W/c.dd.yuv" "$W/c.dec"; then
      fail=$((fail+1)); failed+=("$label dav1d!=aomdec"); return
    fi
  else
    skipped=$((skipped+1)); skipped_msgs+=("dav1d leg (no binary)")
  fi
  pass=$((pass+1))
}

# aligned + non-aligned, multi-SB, lossy
for s in 36 44 60 64 100 120 128 200; do check "sz${s}-q30" "$s" 30; done
# coded-lossless (qindex 0 -> WHT chroma arm) + mid/high qp spread
check "sz64-q0"  64 0
check "sz128-q0" 128 0
check "sz120-q20" 120 20
check "sz44-q45" 44 45
check "sz200-q35" 200 35

echo "chroma_444_gate: $pass pass / $fail fail / $skipped skipped"
for f in "${failed[@]:-}"; do [ -n "$f" ] && echo "  FAIL $f"; done
for s in "${skipped_msgs[@]:-}"; do [ -n "$s" ] && echo "  skip $s"; done
[ "$fail" -eq 0 ]
