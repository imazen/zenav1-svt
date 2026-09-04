#!/usr/bin/env bash
# Issue #9 items 3-5 gate: three MAINLINE config knobs, each driven on BOTH
# encoders by one env vector and asserted BYTE-IDENTICAL to the C oracle:
#
#   max_tx_size = 32      port SVTAV1_MAX_TX_SIZE / C SVT_MAX_TX_SIZE
#                         (enc_dec_process.c:1494-1500 caps the MD scan's
#                         max square at 32; :1815 caps the depth refinement).
#   tune = 3 (IQ)         port SVTAV1_TUNE / C SVT_TUNE — C's whole override
#                         block (enc_handle.c:4889-4915) incl. max_tx_size
#                         32 at qp <= 45 / 64 above, so BOTH sides of that
#                         threshold are cells here.
#   fractional CRF        port SVTAV1_CRF_OFFSET / C SVT_CRF_OFFSET =
#                         extended_crf_qindex_offset 1..3 (rc_crf_cqp.c:471):
#                         `--crf 20.25` == qp 20 + offset 1.
#   chroma_sample_position port SVTAV1_CSP / C SVT_CSP (entropy_coding.c:2743).
#
# ANTI-VACUITY (enforced, per rust/CLAUDE.md "Gate Discipline"): a cell only
# proves a knob is CONSUMED if the C oracle's own bytes CHANGE under it, so
# every cell also captures C WITHOUT the knob and the gate fails if any
# (knob, content, size, preset) configuration is vacuous at every qp. A knob
# that is faithfully threaded but never moves a byte would otherwise keep
# passing after being ripped out.
#
# Cells that byte-match are asserted; cells listed in PINNED are asserted to
# DIFFER (self-promoting: a pinned cell that starts matching FAILS so the
# improvement gets recorded). Nothing is skipped silently.
#
# Usage: tools/issue9_knobs_gate.sh
set -uo pipefail
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
RUN="$HERE/identity_run"
CT="$HERE/capture_c_trace/capture_c_trace"
W="${TMPDIR:-/tmp}/issue9gate.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

pass=0; fail=0; vacuous=0
failed=(); vac=()
declare -A live

# Cells known to DIFFER from C (self-promoting pins). Format: the cell label.
PINNED=(
)

is_pinned() { local x; for x in "${PINNED[@]}"; do [ "$x" = "$1" ] && return 0; done; return 1; }

# run_cell <label> <content> <w> <h> <qp> <preset> <port-env...> -- <c-env...>
# Encodes the port with the port env, C with the C env, and C WITHOUT the
# knob (baseline) for anti-vacuity. Records live[<cfg>] when C moved.
run_cell() {
  local label="$1" content="$2" w="$3" h="$4" qp="$5" preset="$6"; shift 6
  local penv=() cenv=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do penv+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  while [ $# -gt 0 ]; do cenv+=("$1"); shift; done
  local cfg="${label%% q*}" # everything before " q<qp>" = the configuration key
  local d="$W/$label"; mkdir -p "$d"
  if ! env "${penv[@]}" "$RUN" "$content" "$w" "$h" "$qp" "$preset" "$d/rs" >/dev/null 2>"$d/rs.err"; then
    fail=$((fail+1)); failed+=("$label [port-err: $(tail -1 "$d/rs.err")]"); return
  fi
  if ! env "${cenv[@]}" SVT_TRACE_OUT=/dev/null "$CT" "$w" "$h" "$qp" "$preset" "$d/rs.yuv" "$d/c.obu" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [c-err]"); return
  fi
  if ! SVT_TRACE_OUT=/dev/null "$CT" "$w" "$h" "$qp" "$preset" "$d/rs.yuv" "$d/c0.obu" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [c-baseline-err]"); return
  fi
  if cmp -s "$d/c.obu" "$d/c0.obu"; then
    vacuous=$((vacuous+1)); vac+=("$label")
  else
    live[$cfg]=$(( ${live[$cfg]:-0} + 1 ))
  fi
  local sz_rs sz_c; sz_rs=$(stat -f %z "$d/rs.obu" 2>/dev/null || stat -c %s "$d/rs.obu"); sz_c=$(stat -f %z "$d/c.obu" 2>/dev/null || stat -c %s "$d/c.obu")
  if cmp -s "$d/rs.obu" "$d/c.obu"; then
    if is_pinned "$label"; then
      fail=$((fail+1)); failed+=("$label [PINNED cell now MATCHES — promote it]")
    else
      pass=$((pass+1))
    fi
  elif is_pinned "$label"; then
    pass=$((pass+1)); echo "  pinned (still differs): $label port=${sz_rs}B C=${sz_c}B"
  else
    fail=$((fail+1)); failed+=("$label [DIFF port=${sz_rs}B C=${sz_c}B]")
  fi
}

echo "== issue #9 knobs gate: max_tx_size / tune IQ / fractional CRF / chroma_sample_position =="

# --- max_tx_size = 32 at tune 1 (PSNR): 128x128 so 64x64 squares exist to forbid.
for content in gradient; do
  for p in 2 6 10; do
    for qp in 20 40 55; do
      run_cell "maxtx32-${content}-128-p${p} q${qp}" "$content" 128 128 "$qp" "$p" \
        SVTAV1_MAX_TX_SIZE=32 -- SVT_MAX_TX_SIZE=32
    done
  done
done

# --- tune 3 (IQ): both sides of C's `max_tx_size = qp <= 45 ? 32 : 64`.
for p in 6 10; do
  for qp in 20 40 55; do
    run_cell "tuneiq-gradient-128-p${p} q${qp}" gradient 128 128 "$qp" "$p" \
      SVTAV1_TUNE=3 -- SVT_TUNE=3
  done
done

# --- fractional CRF: offsets 1..3 (quarter steps) at two integer qps.
for p in 2 6 10; do
  for qp in 20 40; do
    for off in 1 2 3; do
      run_cell "crf${qp}.${off}-gradient-128-p${p} q${qp}" gradient 128 128 "$qp" "$p" \
        SVTAV1_CRF_OFFSET=$off -- SVT_CRF_OFFSET=$off
    done
  done
done
# the qp-63 extended range (offset 2 = CRF 63.5): C's compression rule is
# inert with the default qp clamps, so this cell is EXPECTED vacuous and is
# recorded, not counted, below.
run_cell "crf63.2-gradient-64-p6 q63" gradient 64 64 63 6 SVTAV1_CRF_OFFSET=2 -- SVT_CRF_OFFSET=2

# --- chroma_sample_position 1 (vertical) and 2 (colocated): two SH bits.
for csp in 1 2; do
  run_cell "csp${csp}-gradient-64-p6 q40" gradient 64 64 40 6 SVTAV1_CSP=$csp -- SVT_CSP=$csp
done

echo
echo "issue9 knobs gate: $pass / $((pass + fail)) byte-identical (vacuous cells: $vacuous)"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$vacuous" -gt 0 ] && printf 'vacuous (C bytes unchanged by the knob): %s\n' "${vac[@]}"
# Anti-vacuity: every (knob, content, size, preset) configuration except the
# documented-inert qp-63 one must have at least one live qp.
dead=0
for cfg in $(printf '%s\n' "${vac[@]}" | sed 's/ q.*//' | sort -u); do
  [ "$cfg" = "crf63.2-gradient-64-p6" ] && continue
  if [ "${live[$cfg]:-0}" -eq 0 ]; then
    echo "VACUOUS CONFIGURATION (knob never moved the C bytes): $cfg"; dead=$((dead+1))
  fi
done
[ "$fail" -eq 0 ] && [ "$dead" -eq 0 ]
