#!/usr/bin/env bash
# Real-image identity matrix: sweep (real photo x preset x cli_qp), run the
# bitstream-identity differ on each cell, and tally byte-identical vs not with
# the first-divergence STAGE classified (SH / FH / tile-op / tile-count ...).
#
# This mirrors tools/identity_matrix.sh but feeds REAL photographic content
# (CID22-512 by default) instead of synthetic uniform/gradient. Both encoders
# consume the SAME .yuv: identity_run.rs decodes the PNG, converts to I420 with
# one fixed deterministic BT.601 transform, and writes the shared .yuv that the
# C driver (tools/capture_c_trace) then encodes. Real photos exercise
# modes / tx-types / partitions / chroma that synthetic content never does, so
# NEW divergences here are the valuable signal (a port that passes synthetic
# identity may still diverge on real content).
#
# NOTE: the named production corpus is imazen26, but it lives on /mnt/v which is
# not mounted on this box. CID22-512 (250 real 512x512 photographic PNGs,
# natively 64-aligned) is the stand-in. When /mnt/v is mounted, swap it in with
#   RIM_CORPUS=/mnt/v/collections/imazen26 tools/real_image_matrix.sh
# (any dir of PNGs works; dims are rounded up to a multiple of 64 per image).
#
# Writes a scoreboard to benchmarks/real_image_identity_<date>.tsv (pass a date
# suffix as $1; default 'latest') and prints a summary. Exit 0 always (this is a
# measurement/tracking gate, not pass/fail — identity on real content is a
# long ratchet, and divergences are expected findings, not test failures).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
. "$(dirname "$0")/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)
SUFFIX="${1:-latest}"
OUT="$RS_ROOT/benchmarks/real_image_identity_${SUFFIX}.tsv"
mkdir -p "$RS_ROOT/benchmarks"

# --- Config (all env-overridable, like identity_matrix.sh) --------------------
CORPUS="${RIM_CORPUS:-$(corpus_dir codec-corpus/CID22/CID22-512/training)}"
read -r -a PRESETS <<<"${RIM_PRESETS:-2 6 10}"
read -r -a QPS <<<"${RIM_QPS:-20 40 55}"
# Number of images to sample (evenly spaced across the sorted corpus for a
# deterministic, content-varied subset), OR set RIM_IMAGE_LIST to a space-
# separated list of explicit PNG paths to override selection entirely.
RIM_IMAGES="${RIM_IMAGES:-20}"
# Per-cell wall-clock guard: a slow preset (M2) on a 512x512 photo is far
# heavier than the 64/128 synthetic cells; an unported path could also hang.
# Cap each cell so the sweep still finishes and classifies the timeout.
CELL_TIMEOUT="${RIM_CELL_TIMEOUT:-300}"
# Wall-clock guard for the decode-diff triage pass (a plain AV1 decode of two
# already-encoded small streams -- should be sub-second; this is defensive,
# not expected to fire).
DECODE_DIFF_TIMEOUT="${RIM_DECODE_TIMEOUT:-30}"

# --- Deterministic image selection -------------------------------------------
if [[ -n "${RIM_IMAGE_LIST:-}" ]]; then
  read -r -a IMAGES <<<"$RIM_IMAGE_LIST"
else
  mapfile -t ALL < <(ls "$CORPUS"/*.png 2>/dev/null | sort)
  n=${#ALL[@]}
  if [[ $n -eq 0 ]]; then
    echo "error: no PNGs in $CORPUS" >&2
    exit 2
  fi
  want=$RIM_IMAGES
  [[ $want -gt $n ]] && want=$n
  stride=$((n / want))
  [[ $stride -lt 1 ]] && stride=1
  IMAGES=()
  for ((k = 0; k < want; k++)); do
    IMAGES+=("${ALL[$((k * stride))]}")
  done
fi

# Pre-build the Rust runner once (identity_diff.sh also builds, but priming here
# keeps per-cell logs clean and surfaces build errors before the sweep starts).
(cd "$RS_ROOT" && nice -n 19 cargo build --release -p zenav1-svt --features symtrace \
    --example identity_run) >&2 || { echo "build failed" >&2; exit 2; }

# Pre-build decode-diff too: it decodes each DIFFERS cell's (c.obu, rs.obu)
# pair with the bit-exact aom-decoder-rs oracle to find the first divergent
# SB in DECODED PIXELS, which auto-classifies "streams differ but recon is
# identical" (a signaling-only divergence, e.g. entropy/header noise with no
# pixel impact) separately from a real pixel divergence.
DECODE_DIFF_BIN="$RS_ROOT/tools/decode_diff/target/release/decode-diff"
(cd "$RS_ROOT/tools/decode_diff" && $LOWPRI env CARGO_BUILD_JOBS=8 \
    cargo build --release -q) >&2 || {
    echo "decode-diff build FAILED." >&2
    echo "  Most likely cause: tools/decode_diff/Cargo.toml has a hard-coded" >&2
    echo "  path dependency on /root/aom-rs/crates/aom-decode, which exists" >&2
    echo "  only on the Linux CI image. There is no env override -- Cargo" >&2
    echo "  path deps are literal -- so on another host this gate cannot build" >&2
    echo "  its pixel-classification oracle." >&2
    echo "  This is a HARNESS portability failure, NOT a parity result: no" >&2
    echo "  conclusion about the port may be drawn from it." >&2
    exit 2
}

# Round a PNG's dimension up to a multiple of RIM_ALIGN (identity_run
# edge-replicates the image into that box).
#
# CORRECTION 2026-08-11 (issue #15): the old comment here said "the pipeline
# requires 64-aligned encode dims". It does not — partial_sb_gate.sh encodes
# 65x63 — and that false premise is why this matrix, the ONLY real-content
# identity gate, has never once tested a partial superblock. Crossing real
# content with unaligned dims finds divergences at every preset:
# tools/unaligned_identity_scan.sh, 67 of 648 cells, e.g. 96x88 p4 q33 at
# 403 B (port) vs 392 B (C) where the same dims on `gradient` byte-match.
# RIM_ALIGN=1 runs each image at its TRUE dimensions; 64 (the default) keeps
# this scoreboard's historical baseline comparable.
png_dims_aligned() {
  python3 - "$1" "${RIM_ALIGN:-64}" <<'PY'
import struct, sys
d = open(sys.argv[1], "rb").read(24)
w, h = struct.unpack(">II", d[16:24])
a = max(1, int(sys.argv[2]))
up = lambda x: (x + a - 1) // a * a
print(up(w), up(h))
PY
}

# Decode-diff triage for a DIFFERS cell: sets $first_sb / $ndiff_y.
# Callers only invoke this for DIFFERS cells (verdict != IDENTICAL) -- a
# byte-identical stream pair can't have a decoded difference, so identical
# cells skip this entirely and get "-"/"-" without spending a decode.
#
#   first_sb=<mi_row,mi_col>  a real pixel divergence was located
#   first_sb=dec-same         streams differ but decoded pixels are IDENTICAL
#                             (a recon-invisible signaling divergence)
#   first_sb=dec-noobu        c.obu/rs.obu missing or empty (e.g. a HANG cell
#                             with an incomplete encode) -- decode not attempted
#   first_sb=dec-timeout      decode-diff itself exceeded DECODE_DIFF_TIMEOUT
#   first_sb=dec-suspect      aom-decoder-rs's known Wiener-restoration decode
#                             bug fired (see decode_diff/src/main.rs) -- the
#                             locate would be unreliable, so it's withheld
#   first_sb=dec-err          decode error (bad OBU, dimension mismatch, ...)
# ndiff_y is the plane0 (luma) differing-pixel count when a real divergence
# was located, 0 for dec-same, "-" for every other case above.
decode_diff_cols() {
  local d="$1" c_obu rs_obu out rc sb_line ndiff_line mr mc
  first_sb="-"; ndiff_y="-"
  c_obu="$d/c.obu"; rs_obu="$d/rs.obu"
  if [[ ! -s "$c_obu" || ! -s "$rs_obu" ]]; then
    first_sb="dec-noobu"
    return
  fi
  out=$(timeout "$DECODE_DIFF_TIMEOUT" "$DECODE_DIFF_BIN" "$c_obu" "$rs_obu" 2>&1)
  rc=$?
  case $rc in
    0)
      first_sb="dec-same"
      ndiff_y=0
      ;;
    1)
      sb_line=$(grep -m1 "^SB " <<<"$out" || true)
      ndiff_line=$(grep -m1 "^NDIFF " <<<"$out" || true)
      if [[ -n "$sb_line" ]]; then
        mr=$(sed -n 's/.*mi_row=\([0-9]*\).*/\1/p' <<<"$sb_line")
        mc=$(sed -n 's/.*mi_col=\([0-9]*\).*/\1/p' <<<"$sb_line")
        first_sb="${mr},${mc}"
      else
        first_sb="dec-err"  # e.g. the DIMS mismatch report, no SB to locate
      fi
      if [[ -n "$ndiff_line" ]]; then
        ndiff_y=$(sed -n 's/.*plane0=\([0-9]*\).*/\1/p' <<<"$ndiff_line")
      fi
      ;;
    124) first_sb="dec-timeout" ;;
    3) first_sb="dec-suspect" ;;
    *) first_sb="dec-err" ;;
  esac
}

printf 'image\twidth\theight\tcli_qp\tpreset\tverdict\tstage\tdetail\tc_bytes\trust_bytes\tfirst_sb\tndiff_y\n' >"$OUT"
identical=0
total=0
cells=$((${#IMAGES[@]} * ${#PRESETS[@]} * ${#QPS[@]}))
echo "real-image matrix: ${#IMAGES[@]} images x ${#PRESETS[@]} presets x ${#QPS[@]} qps = $cells cells" >&2
echo "corpus: $CORPUS" >&2
echo "progress log streams to: $OUT (tail -f it)" >&2

start_ts=$(date +%s)
for img in "${IMAGES[@]}"; do
  stem=$(basename "$img" .png)
  read -r W H < <(png_dims_aligned "$img")
  for preset in "${PRESETS[@]}"; do
    for qp in "${QPS[@]}"; do
      total=$((total + 1))
      d="$RS_ROOT/target/real_identity/${stem}_q${qp}_p${preset}"
      rep="$d/report.txt"
      cell_t0=$(date +%s)
      timeout "$CELL_TIMEOUT" "$HERE/identity_diff.sh" "$W" "$H" "$qp" "$preset" \
          "file:$img" "$d" >/dev/null 2>&1
      rc=$?
      cell_dt=$(( $(date +%s) - cell_t0 ))

      # Extract byte counts from the differ's VERDICT line (report.txt):
      #   IDENTICAL (NNNB, ... )                 -> both = NNN
      #   NOT IDENTICAL (C=NNNB Rust=MMMB)       -> C=NNN Rust=MMM
      cb="-"; rb="-"
      if [[ -f "$rep" ]]; then
        vl=$(grep -m1 "^VERDICT: " "$rep" || true)
        if [[ "$vl" == *"NOT IDENTICAL"* ]]; then
          cb=$(sed -n 's/.*C=\([0-9]*\)B.*/\1/p' <<<"$vl")
          rb=$(sed -n 's/.*Rust=\([0-9]*\)B.*/\1/p' <<<"$vl")
        elif [[ "$vl" == *"IDENTICAL"* ]]; then
          cb=$(sed -n 's/.*IDENTICAL (\([0-9]*\)B.*/\1/p' <<<"$vl")
          rb="$cb"
        fi
      fi

      if [[ $rc -eq 0 ]]; then
        # Byte-identical streams can't decode to different pixels -- skip
        # decode-diff entirely so identical cells stay fast.
        printf '%s\t%s\t%s\t%s\t%s\tIDENTICAL\t-\t-\t%s\t%s\t-\t-\n' \
          "$stem" "$W" "$H" "$qp" "$preset" "$cb" "$rb" >>"$OUT"
        identical=$((identical + 1))
        echo "[$total/$cells] $stem q$qp p$preset -> IDENTICAL (${cell_dt}s)" >&2
        continue
      fi
      if [[ $rc -eq 124 ]]; then
        decode_diff_cols "$d"
        printf '%s\t%s\t%s\t%s\t%s\tDIFFERS\tHANG\t(cell timed out at %ss)\t%s\t%s\t%s\t%s\n' \
          "$stem" "$W" "$H" "$qp" "$preset" "$CELL_TIMEOUT" "$cb" "$rb" "$first_sb" "$ndiff_y" >>"$OUT"
        echo "[$total/$cells] $stem q$qp p$preset -> HANG (${CELL_TIMEOUT}s cap)" >&2
        continue
      fi
      # Classify from the differ's concise `STAGE: <stage> | <detail>` line.
      stage="unknown"; detail="-"
      if [[ -f "$rep" ]]; then
        line=$(grep -m1 "^STAGE: " "$rep" || true)
        if [[ -n "$line" ]]; then
          rest="${line#STAGE: }"
          stage="${rest%% | *}"
          detail="${rest#* | }"
        fi
      fi
      decode_diff_cols "$d"
      printf '%s\t%s\t%s\t%s\t%s\tDIFFERS\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$stem" "$W" "$H" "$qp" "$preset" "$stage" "$detail" "$cb" "$rb" "$first_sb" "$ndiff_y" >>"$OUT"
      echo "[$total/$cells] $stem q$qp p$preset -> DIFFERS/$stage (${cell_dt}s)" >&2
    done
  done
done

elapsed=$(( $(date +%s) - start_ts ))
echo ""
echo "real-image identity: $identical / $total byte-identical (${elapsed}s)"
echo "first-divergence stage histogram (non-identical cells):"
awk -F'\t' 'NR>1 && $6=="DIFFERS" {print $7}' "$OUT" | sort | uniq -c | sort -rn
echo "mean Rust/C size ratio (non-identical cells):"
awk -F'\t' 'NR>1 && $6=="DIFFERS" && $9!="-" && $10!="-" && $9>0 {s+=$10/$9; n++}
           END{if(n>0)printf "  %.4f over %d cells\n", s/n, n; else print "  (none)"}' "$OUT"
echo "scoreboard: $OUT"
