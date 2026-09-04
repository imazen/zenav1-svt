#!/usr/bin/env bash
# NATIVE 10-bit source, REAL PHOTOGRAPHIC content, PQ transfer curve —
# byte-identity vs the real C encoder (issue #7 / task #6).
#
# WHY THIS IS A DIFFERENT GATE FROM `bd10_hbd_src_gate.sh`. That one proves the
# caller's low 2 bits survive end-to-end, but on SYNTHETIC content
# (uniform/gradient) whose low bits are a deterministic `(3r + 5c + v) % 4`
# pattern. Two things that pattern cannot represent:
#
#   * PHOTOGRAPHIC structure. `bd10_photo_gate.sh` exists for exactly this
#     reason at bd10 generally — docs/bd10-port-map.md records an 18/18
#     photographic FAILURE at a time when the synthetic bd10 gates were green.
#     Synthetic-green is not content-green.
#   * A REAL TRANSFER CURVE. `SVTAV1_HBD_PQ` linearizes the 8-bit sRGB luma,
#     maps it onto a 1000-nit display, runs the SMPTE ST 2084 (PQ) OETF and
#     quantizes to 10-bit LIMITED range (64..940); chroma is rescaled 8-bit
#     limited (16..240) -> 10-bit limited (64..960). The resulting low bits are
#     a consequence of a nonlinear curve — no `<< 2` can produce them — and the
#     code-value HISTOGRAM is PQ-shaped (dense in the shadows, sparse in the
#     highlights), which is what an HDR photo hands an encoder.
#
# HONEST SCOPE: this is a PQ-shaped 10-bit code-value distribution derived from
# an 8-bit photographic master, NOT a native HDR capture — highlight detail the
# 8-bit master already clipped does not come back. What the gate tests is the
# 10-bit SAMPLE path (u16 entry -> MD funnel -> coded levels -> deblock/CDEF/LR
# searches) against C on realistic code values, and for that the distribution
# is the load-bearing part. CICP is not varied: in MAINLINE v4.2.0 the encode
# is CICP-independent apart from the header bits (the only reads of
# transfer_characteristics / color_primaries are the chroma-q boosts at
# rc_crf_cqp.c:573-586, inside `#if SVT_HDR_MODE`), and capture_c_trace sets no
# CICP either.
#
# ANTI-VACUITY (enforced): every cell is also encoded from the WIDENED-u8
# source of the same image, and the gate FAILS if any (image, preset) pair is
# vacuous at every qp — i.e. if the PQ low bits never changed the bitstream,
# which is what a silently-truncating port would look like.
#
# CORPUS: CID22-512 (real 512x512 photographic PNGs, natively 64-aligned — the
# bd10 path requires 64-aligned dims). Override with BD10_PQ_CORPUS=<dir>.
# ABSENT CORPUS FAILS LOUDLY; this gate never skips silently.
set -uo pipefail
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_corpus.sh
. "$HERE/lib_corpus.sh"

CORPUS="${BD10_PQ_CORPUS:-$(corpus_dir codec-corpus/CID22/CID22-512/training)}"
if [ ! -d "$CORPUS" ]; then
  echo "bd10 PQ gate: corpus MISSING at $CORPUS" >&2
  echo "  set BD10_PQ_CORPUS=<dir of 512x512 PNGs> or ZENAV1_CORPUS_ROOT=<root>" >&2
  exit 2
fi
# A FIXED, NAMED subset — not \"the first N of a glob\", which silently changes
# meaning when the corpus does.
read -r -a IMAGES <<<"${BD10_PQ_IMAGES:-1001682 2119713 4666751 2738653 7062227}"
read -r -a QPS <<<"${BD10_PQ_QPS:-8 20 32 55}"
# eff-M9 (9) and the two other bands the bd10 photographic gate covers.
read -r -a PRESETS <<<"${BD10_PQ_PRESETS:-6 8 9}"
SZ="${BD10_PQ_SIZE:-512}"

# PER-ISA pins (docs/SUSPECTED-C-BUGS.md #9). MEASURED 2026-08-28 on
# macOS/clang/aarch64: presets 8 and 9 are 40/40 byte-identical and EVERY
# preset-6 cell differs. That is NOT recorded as a port gap here, because on
# this host the C ORACLE is the variable at bd10 on non-flat content — same
# commit, same port binary:
#
#   tools/bd10_nonflat_gate.sh   CI x86-64 309/309   local aarch64 197/309
#   tools/bd10_photo_gate.sh     (not in CI)         local aarch64  53/191
#
# The port is provably not the variable side: `tier_invariance.rs` holds its
# bytes constant across every archmage dispatch tier, and photographic bd10
# cells were re-run against the pre-session tree (bfae1b69) in a sibling
# workspace with byte-identical port output.
#
# So the pins are scoped to aarch64 and x86-64 CI runs the p6 cells UNPINNED —
# which is what decides whether p6 is that same C-per-host divergence or a real
# gap in the port's 10-bit CDEF / Wiener searches (p6 is the only preset in
# this grid that runs either; docs/bd10-port-map.md group E). Pins are
# self-promoting: a pinned cell that starts matching FAILS.
KNOWN_DIFF=()
case "$(uname -m)" in
  arm64 | aarch64)
    # The EXACT cells measured to differ — not "all of p6", which the
    # self-promotion check immediately rejected (8 of the 20 p6 cells DO
    # match on aarch64). The split is by qp: q8/q20 differ everywhere,
    # q32 differs on two images, q55 matches everywhere.
    KNOWN_DIFF+=(
      1001682_q8_p6  1001682_q20_p6
      2119713_q8_p6  2119713_q20_p6
      4666751_q8_p6  4666751_q20_p6  4666751_q32_p6
      2738653_q8_p6  2738653_q20_p6
      7062227_q8_p6  7062227_q20_p6  7062227_q32_p6
    )
    ;;
esac
is_known_diff() {
  local needle=$1 k
  for k in "${KNOWN_DIFF[@]}"; do
    [[ "$k" == "$needle" ]] && return 0
  done
  return 1
}

OUT="${TMPDIR:-/tmp}/bd10pq.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT

pass=0; fail=0; vacuous=0; missing=0; isa_pinned=0
failed=(); vac=(); isa_pins=()
declare -A live

for img in "${IMAGES[@]}"; do
  png="$CORPUS/$img.png"
  if [ ! -f "$png" ]; then
    echo "  MISSING IMAGE: $png"; missing=$((missing + 1)); continue
  fi
  for p in "${PRESETS[@]}"; do
    pair="${img}_p${p}"
    : "${live[$pair]:=0}"
    for qp in "${QPS[@]}"; do
      cell="${img}_q${qp}_p${p}"
      if ! SVTAV1_BD=10 SVTAV1_HBD_SRC=1 SVTAV1_HBD_PQ=1 "$HERE/identity_run" \
           "file:$png" "$SZ" "$SZ" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
      fi
      if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
           "$SZ" "$SZ" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
      fi
      # The widened-u8 stream of the SAME image, for anti-vacuity.
      if ! SVTAV1_BD=10 "$HERE/identity_run" \
           "file:$png" "$SZ" "$SZ" "$qp" "$p" "$OUT/w" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[widen-err]"); continue
      fi
      if cmp -s "$OUT/rs.obu" "$OUT/w.obu"; then
        vacuous=$((vacuous + 1)); vac+=("$cell")
      else
        live[$pair]=$(( ${live[$pair]} + 1 ))
      fi
      rs=$(stat -f %z "$OUT/rs.obu" 2>/dev/null || stat -c %s "$OUT/rs.obu")
      cb=$(stat -f %z "$OUT/c.obu" 2>/dev/null || stat -c %s "$OUT/c.obu")
      if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        if is_known_diff "$cell"; then
          fail=$((fail + 1)); failed+=("$cell[PER-ISA pin now MATCHES — remove it]")
        else
          pass=$((pass + 1))
        fi
      elif is_known_diff "$cell"; then
        isa_pinned=$((isa_pinned + 1)); isa_pins+=("$cell port=${rs}B C=${cb}B")
      else
        fail=$((fail + 1)); failed+=("$cell[port=${rs}B C=${cb}B]")
      fi
    done
  done
done

echo "bd10 PQ-10bit PHOTOGRAPHIC identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf '  FAILED: %s\n' "${failed[*]}"
[ "$isa_pinned" -gt 0 ] && printf '  per-ISA pinned (aarch64 only, SUSPECTED-C-BUGS #9): %s\n' "${isa_pins[*]}"
[ "$vacuous" -gt 0 ] && printf '  vacuous cells (PQ stream == widened-u8 stream): %s\n' "${vac[*]}"
dead=()
for k in "${!live[@]}"; do
  [ "${live[$k]}" -eq 0 ] && dead+=("$k")
done
if [ "${#dead[@]}" -gt 0 ]; then
  echo "  GATE PREMISE FAILED — vacuous at EVERY qp, so these would pass with"
  echo "  the u16 threading removed: ${dead[*]}"
fi
echo "  (image, preset) pairs with at least one PQ-live qp: $(( ${#live[@]} - ${#dead[@]} )) / ${#live[@]}"
[ "$fail" -eq 0 ] && [ "${#dead[@]}" -eq 0 ] && [ "$missing" -eq 0 ]
