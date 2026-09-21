#!/usr/bin/env bash
# 4:4:4 INTER DECODER-EQUALITY GATE — the witness for the non-funnel
# 4:4:4 inter arm (motion-compensated chroma prediction + residual, no
# `uv_mode`). The still/key-frame leg lives in chroma_444_gate.sh.
#
# WHAT IT ASSERTS: for each cell, `examples/probe_444_dup` or
# `examples/probe_444_video` encodes a multi-frame 4:4:4 stream (frame 0
# keys, every later frame inter), aomdec decodes it, and EVERY frame's
# decoded Y/U/V planes equal the encoder's own reconstruction BYTE-FOR-
# BYTE. 4:4:4 has no C oracle (SVT-AV1 refuses non-4:2:0 at
# verify_settings, enc_settings.c:470), so this gate is the entire parity
# claim — "decodes" is NOT enough, the pixels must match.
#
# Coverage mirrors the landing: dup (all-skip inter), rand (intra-forced
# inter) and shift (integer-MV inter with nonzero chroma residual) at
# aligned and SB-straddling dims across the low/mid/high preset spread —
# plus a real-motion stream (translated content + a moving 16x16 block
# with distinct chroma) and a 6-frame inter-on-inter reference chain.
#
# MEASURED 2026-09-21: 45/45 matrix cells + 4/4 moving-content frames +
# 6/6 chain frames byte-identical on all three planes.
#
# Usage: tools/chroma_444_inter_gate.sh
# Env:   AOMDEC
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
AOMDEC=${AOMDEC:-$(command -v aomdec || true)}
PDUP="$RS/target/release/examples/probe_444_dup"
PVID="$RS/target/release/examples/probe_444_video"
W=$(mktemp -d /tmp/g444i.XXXXXX)
trap 'rm -rf "$W"' EXIT

if [ -z "$AOMDEC" ]; then
  echo "SKIP: aomdec not on PATH" >&2; exit 2
fi
for p in probe_444_dup probe_444_video; do
  if [ ! -x "$RS/target/release/examples/$p" ]; then
    (cd "$RS" && cargo build -q --release -p zenav1-svt --example "$p") \
      || { echo "FAIL: $p build" >&2; exit 1; }
  fi
done

pass=0; fail=0; declare -a failed

# Compare every frame's decoded planes against the probe's recon.f{i}.
# recon.f layout: luma at true stride w x h rows, then U and V at
# 8-aligned stride aw x ah rows (the aligned chroma planes at 4:4:4).
# aomdec --rawvideo emits concatenated I444 frames at w*h*3 each.
cmp_frames() { # dir w h nframes
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import sys
d, w, h, n = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
aw, ah = ((w + 7) // 8) * 8, ((h + 7) // 8) * 8
dec = open(f'{d}/dec.yuv', 'rb').read()
fsz = w * h * 3
if len(dec) < n * fsz:
    print(f'  aomdec output short: {len(dec)} < {n * fsz}')
    sys.exit(1)
ok = True
for i in range(n):
    rec = open(f'{d}/recon.f{i}', 'rb').read()
    f = dec[i * fsz:(i + 1) * fsz]
    dy = f[:w * h]
    du = f[w * h:2 * w * h]
    dv = f[2 * w * h:]
    ry = rec[:w * h]
    ru = b''.join(rec[w * h + r * aw: w * h + r * aw + w] for r in range(h))
    rv = b''.join(rec[w * h + aw * ah + r * aw: w * h + aw * ah + r * aw + w]
                  for r in range(h))
    ny = sum(a != b for a, b in zip(dy, ry))
    nu = sum(a != b for a, b in zip(du, ru))
    nv = sum(a != b for a, b in zip(dv, rv))
    if ny or nu or nv:
        ok = False
        print(f'  f{i}: DIFF y={ny} u={nu} v={nv}')
sys.exit(0 if ok else 1)
PY
}

check() { # label probe mode size qp preset nframes
  local label=$1 probe=$2 mode=$3 size=$4 qp=$5 preset=$6 nf=$7
  local outdir="$W/$label"
  if [ "$probe" = "$PDUP" ]; then
    if ! N444=$nf "$probe" "$outdir" "$size" "$qp" "$preset" "$mode" \
        >"$W/$label.log" 2>&1; then
      if grep -q REFUSED "$W/$label.log"; then
        fail=$((fail+1)); failed+=("$label REFUSED (envelope moved?)")
      else
        fail=$((fail+1)); failed+=("$label encode-fail")
      fi
      return
    fi
  else
    if ! "$probe" "$outdir" "$size" "$qp" "$preset" "$nf" \
        >"$W/$label.log" 2>&1; then
      fail=$((fail+1)); failed+=("$label encode-fail"); return
    fi
  fi
  if ! "$AOMDEC" --rawvideo -o "$outdir/dec.yuv" "$outdir/probe.obu" \
      >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label aomdec-reject"); return
  fi
  local w=${size%x*} h=${size#*x}
  [[ "$size" != *x* ]] && h=$w
  if ! cmp_frames "$outdir" "$w" "$h" "$nf"; then
    fail=$((fail+1)); failed+=("$label recon!=aomdec"); return
  fi
  pass=$((pass+1))
}

# dup / rand / shift x dims x preset spread — 45 cells
for mode in dup rand shift; do
  for size in 64x64 128x64 64x128 128x128 256x128; do
    for p in 0 6 13; do
      check "$mode-$size-p$p" "$PDUP" "$mode" "$size" 30 "$p" 2
    done
  done
done

# real translational motion + moving chroma block, 4 frames
check "video-128-p6" "$PVID" - 128 30 6 4
# inter-on-inter reference chain, 6 frames
check "video-chain6-128-p6" "$PVID" - 128 30 6 6

echo "chroma_444_inter_gate: $pass pass / $fail fail"
for f in "${failed[@]:-}"; do [ -n "$f" ] && echo "  FAIL $f"; done
[ "$fail" -eq 0 ]
