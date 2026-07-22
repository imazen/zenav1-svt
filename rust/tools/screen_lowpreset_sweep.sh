#!/usr/bin/env bash
# Screen-content LOW-PRESET (p0-p5) byte-parity SWEEP — reporting, not a gate.
#
# IBC chunk 1 companion (docs/ibc-port-map.md §D chunk 1): the gb82-sc crops
# at presets 0-4 are the IBC envelope (sc_class5 && M<=4 => C sets
# frm_hdr.allow_intrabc=1); preset 5 is the first IBC-off preset (level 0)
# and rides along as the structural control. This sweep prints per-cell
# OK/DIFF so the before->after effect of IBC landings can be measured
# cell-by-cell. It exits 0 as long as every cell ENCODES (divergence is
# expected while IBC is unported/partial); a Rust- or C-side encode error
# exits 1.
#
# Output: one "OK <tag> <bytes>" / "DIFF <tag> <first-diff-byte> <rs-size>
# <c-size>" line per cell + a summary; machine-greppable.
#
# Conventions copied from screen_palette_gate.sh (crop:, 512x512, bd8).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CT_BIN="$HERE/capture_c_trace/capture_c_trace"
SCREEN_DIR="${SCREEN_DIR:-/root/work/codec-corpus/gb82-sc}"
# Default to the REPO-root Bin symlink (rust/Bin does not exist; the other
# gates are invoked with SVT_CREF_LIB_DIR set explicitly).
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR

read -r -a IMGS <<<"${SL_IMGS:-graph codec_wiki gmessages gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a QPS  <<<"${SL_QPS:-5 20 32 48 63}"
read -r -a PRESETS <<<"${SL_PRESETS:-0 1 2 3 4 5}"
DIM="${SL_DIM:-512}"
OUT="${SL_OUT:-$RS_ROOT/target/screen_lowpreset_sweep}"

echo "priming builds..." >&2
( cd "$RS_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c3 \
    cargo build --release -p zenav1-svt --features symtrace --example identity_run ) >&2 \
  || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CT_BIN" ] || { echo "binaries missing" >&2; exit 2; }

mkdir -p "$OUT"
ok=0; diff=0; err=0
for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { echo "SKIP-MISSING $img"; continue; }
  for p in "${PRESETS[@]}"; do
    for qp in "${QPS[@]}"; do
      tag="${img}_p${p}_q${qp}"
      d="$OUT/$tag"; mkdir -p "$d"
      if ! SVTAV1_BD=8 nice -n 19 ionice -c3 \
            "$RUN_BIN" "crop:$png" "$DIM" "$DIM" "$qp" "$p" "$d/rs" >/dev/null 2>&1; then
        err=$((err+1)); echo "ERR  $tag rs-encode"; continue
      fi
      if ! SVT_NO_AUTO_CMAKE=1 nice -n 19 ionice -c3 \
            "$CT_BIN" "$DIM" "$DIM" "$qp" "$p" "$d/rs.yuv" "$d/c.obu" 8 >/dev/null 2>&1; then
        err=$((err+1)); echo "ERR  $tag c-encode"; continue
      fi
      if cmp -s "$d/rs.obu" "$d/c.obu"; then
        ok=$((ok+1)); echo "OK   $tag $(stat -c%s "$d/rs.obu")"
      else
        fd=$(cmp "$d/rs.obu" "$d/c.obu" 2>/dev/null | awk '{print $5}' | tr -d ',')
        diff=$((diff+1)); echo "DIFF $tag ${fd:-len} $(stat -c%s "$d/rs.obu") $(stat -c%s "$d/c.obu")"
      fi
    done
  done
done
echo "SUMMARY ok=$ok diff=$diff err=$err total=$((ok+diff+err))"
[ "$err" -eq 0 ] || exit 1
exit 0
