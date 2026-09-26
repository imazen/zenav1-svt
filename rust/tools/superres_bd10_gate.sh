#!/usr/bin/env bash
# SUPERRES bd10 identity + conformance gate (the 10-bit arm of
# `superres_gate.sh`).
#
# C's native-10-bit superres flow (svt_aom_resize_frame, resize.c) is pack ->
# `svt_av1_highbd_resize_plane_horizontal` -> unpack: the u8 canvas the rest
# of the encoder reads is `filtered_u16 >> 2`, NOT the already-truncated u8
# resized. The port mirrors that order (`superres_downscale_420_hbd`), and
# the normative recon upscale runs the u16 twin
# (`highbd_upscale_normative_row`, pinned byte-exact by
# `c_parity_superres::upscale_row_hbd_matches_c_across_denominators`).
#
# Per cell this gate asserts FOUR things:
#
#   1. BYTE-PARITY — the port's OBU == the real C encoder's OBU at the same
#      config (SVT_SUPERRES_KF_DENOM=D, bd 10, the SAME u16 source .yuv).
#   2. DECODABILITY + OUTPUT GEOMETRY — the stream decodes under aomdec and
#      the frame comes out at the FULL (upscaled) size as u16 I420.
#   3. RECON PARITY — the port's `last_recon10_final` (SVTAV1_FINAL_RECON)
#      equals aomdec's decoded output byte-for-byte. This is the leg that
#      pins the u16 normative upscale + the whole 10-bit filter chain at the
#      output geometry; a byte-identical stream already implies it, but the
#      explicit comparison keeps the check meaningful if byte-parity ever
#      regresses first.
#   4. ANTI-VACUITY — the superres stream must DIFFER from the same cell at
#      bd10 without superres.
#
# The source is identity_run's SVTAV1_HBD_SRC content: a REAL 10-bit source
# whose low 2 bits carry signal, so the u16 downscale's extra precision is
# observable in the output bytes (u8<<2 input would let a u8-resize pass as
# "parity" — a vacuous gate).
#
# Presets 7..13 (7 re-measured 128/128 on 2026-09-26, as in the 8-bit gate).
# Env: AOMDEC (path to aomdec; required — no graceful skip). SR_* filters
# match the 8-bit gate's.
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
# The grid is a cell list for tools/cellrun.py (plan T3): each superres cell
# checks C bytes and recon (aomdec's 10-bit output == the port's final
# recon, which also pins the upscaled size), and names its no-superres
# sibling as the stream it must differ from.
OUT="${TMPDIR:-$HOME/tmp}/srgate10.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
CELLS="$OUT/superres_bd10.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv_port\tenv_c\tcheck\tdiffers_from\n' >"$CELLS"
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        base="${content}_${sz}_q${qp}_p${p}"
        printf '%s_ns\t%s\t%s\t%s\t%s\t%s\t10\tSVTAV1_HBD_SRC=1\t\tnone\t\n' \
          "$base" "$content" "$sz" "$sz" "$qp" "$p" >>"$CELLS"
        for d in "${DENOMS[@]}"; do
          printf '%s_d%s\t%s\t%s\t%s\t%s\t%s\t10\tSVTAV1_HBD_SRC=1;SVTAV1_SUPERRES=%s\tSVT_SUPERRES_KF_DENOM=%s\tc,recon\t%s_ns\n' \
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
allrows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
rows = [r for r in allrows if not r["name"].endswith("_ns")]
bad = [r for r in rows if r["ok"] != "yes"]
print(f"superres bd10 identity + conformance + recon: {len(rows) - len(bad)} / {len(rows)} cells")
for r in bad:
    print(f"  FAILED: {r['name']} [{r['verdict']} {r['detail']}; {r['checks']}]")
for r in allrows:
    if r["name"].endswith("_ns") and r["verdict"] == "ERROR":
        print(f"  ERROR (sibling {r['name']}): {r['detail']}")
PY
exit "$rc"
