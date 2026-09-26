#!/usr/bin/env bash
# SUPERRES identity + conformance gate (superres chunk B.3).
#
# Superres encodes the frame at a REDUCED width `coded_w = w * 8 / denom` and
# the decoder normatively upscales it back to `w`. Three things must hold, and
# this gate checks all three per cell:
#
#   1. BYTE-PARITY — the port's OBU == the real C encoder's OBU at the same
#      config (`SVT_SUPERRES_KF_DENOM=D`, which the C driver maps to
#      `superres_mode = SUPERRES_FIXED` + `superres_kf_denom = D`).
#   2. RECON — the stream decodes under the AV1 reference decoder to exactly
#      the port's final (upscaled) reconstruction. Byte-parity alone cannot
#      catch a header that describes a geometry neither encoder actually
#      produced; the upscale is normative, so this is not optional. (Until
#      2026-09-26 this leg only checked the decoded frame's SIZE; the recon
#      comparison was then measured 512/512 and replaced it.)
#   3. ANTI-VACUITY — the superres stream must DIFFER from the same cell
#      encoded without superres. A cell where they coincide proves nothing.
#
# MEASURED (2026-07-24): for a STILL (KEY) frame the denominator that takes
# effect in C is `--superres-kf-denom` / `superres_kf_denom`. Setting only
# `superres_denom` signals `enable_superres = 1` but leaves `use_superres = 0`
# on the key frame.
#
# SCOPE (the cells this gate CLAIMS byte-parity for): allintra presets 7..13, every
# denominator 9..=16, both contents, both sizes, qp {20,32,40,55}. Adding a cell
# means it byte-matches — do NOT add one that only decodes.
#
# One documented exclusion, with a measured root cause:
#
# * presets <= 6 — the port REFUSES superres there (`superres_config_error`):
#   loop restoration is on (`seq_tools_for_preset`: wn > 0) and C runs LR on the
#   UPSCALED frame (`svt_av1_superres_upscale_frame` sits between CDEF and LR,
#   cdef_process.c:152) while this port still searches/applies it at the coded
#   width. Refusing beats emitting a stream whose LR geometry disagrees with the
#   signalled one.
# * (closed) preset 7 was excluded for ONE divergent cell,
#   `gradient_64_q32_p7_d10`, until re-measured 2026-09-26 at 128/128
#   byte-identical with recon == aomdec (i265); it is in the default set.
#
#   History worth keeping: that sweep was 507/640 before chunk B.4. The 133
#   divergences were all partition-symbol (`CDF10`) flips on textured content,
#   and encoding the port's OWN downscaled pixels at the coded dims WITHOUT
#   superres was byte-identical to C (gradient 128x128 q32 p10 d16: 724B ==
#   724B, 6390 tile ops) — so neither the downscale nor the coded-width MD was
#   at fault. Root cause was in C: `scale_pcs_params` (resize.c:1434) re-inits
#   the b64/SB geometry for the coded size but does NOT recompute
#   `pcs->variance`, so C's PD0 keeps reading picture-analysis variances
#   computed on the FULL-RESOLUTION b64 grid through the new coded-grid
#   indices. Chunk B.4 reproduces that indexing deliberately.
#
# Env: AOMDEC (path to aomdec; required — no graceful skip), SR_JOBS (4),
# SR_SIZES, SR_QPS, SR_PRESETS, SR_DENOMS, SR_CONTENTS.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  echo "aomdec not found (set AOMDEC=/path/to/aomdec) — this gate needs the" >&2
  echo "reference decoder to check the normative upscale" >&2
  exit 2
fi
read -r -a SIZES <<<"${SR_SIZES:-64 128}"
read -r -a QPS <<<"${SR_QPS:-20 32 40 55}"
read -r -a PRESETS <<<"${SR_PRESETS:-7 8 9 10 13}"
read -r -a DENOMS <<<"${SR_DENOMS:-9 10 11 12 13 14 15 16}"
read -r -a CONTENTS <<<"${SR_CONTENTS:-uniform gradient}"
# The grid is a cell list for tools/cellrun.py (plan T3). Each superres cell
# checks C bytes and recon, and names its no-superres sibling (encoded once
# per tuple, port only) as the stream it must differ from.
OUT="${TMPDIR:-$HOME/tmp}/srgate.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
CELLS="$OUT/superres.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tenv\tcheck\tdiffers_from\n' >"$CELLS"
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        base="${content}_${sz}_q${qp}_p${p}"
        printf '%s_ns\t%s\t%s\t%s\t%s\t%s\t\tnone\t\n' \
          "$base" "$content" "$sz" "$sz" "$qp" "$p" >>"$CELLS"
        for d in "${DENOMS[@]}"; do
          printf '%s_d%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_SUPERRES=%s;SVT_SUPERRES_KF_DENOM=%s\tc,recon\t%s_ns\n' \
            "$base" "$d" "$content" "$sz" "$sz" "$qp" "$p" "$d" "$d" "$base" >>"$CELLS"
        done
      done
    done
  done
done
AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$CELLS" --out "$OUT/result.tsv" \
  --bytes-only --jobs "${SR_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" <<'PY'
import csv, sys
rows = [r for r in csv.DictReader(open(sys.argv[1]), delimiter="\t") if not r["name"].endswith("_ns")]
bad = [r for r in rows if r["ok"] != "yes"]
print(f"superres identity + recon: {len(rows) - len(bad)} / {len(rows)} cells")
for r in bad:
    print(f"  FAILED: {r['name']} [{r['verdict']} {r['detail']}; {r['checks']}]")
PY
exit "$rc"
