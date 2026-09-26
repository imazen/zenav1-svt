#!/usr/bin/env bash
# NATIVE 10-bit SOURCE identity gate (task #6 chunk 2): the port's u16 entry
# points (`try_encode_frame_420_hbd` / `try_encode_frame_hbd`) vs the real C
# encoder, on content whose LOW 2 BITS ARE SET — i.e. a source that is NOT a
# widened 8-bit picture.
#
# Why this is a different gate from `bd10_matrix.sh` / `bd10_nonflat_gate.sh`:
# those feed both encoders `u8 << 2`, so the low 2 bits are zero everywhere and
# a port that silently truncated them would still pass. Here `SVTAV1_HBD_SRC=1`
# makes `identity_run` generate real 10-bit samples, write them to the .yuv the
# C driver reads, AND push the same u16 planes through the port's hbd entry
# point. Byte-identical OBUs then prove the low bits survive end-to-end
# (MD funnel + coded levels + deblock/CDEF/LR searches) in BOTH encoders.
#
# ANTI-VACUITY (enforced below, not just documented): a cell only proves
# something about the u16 path if its real-10-bit stream DIFFERS from the
# widened-u8 stream of the same content. MEASURED 2026-07-24: at qp 55 they
# coincide for every content/size/preset — at that quantizer a +-3/1023
# perturbation is below the quantization step, so the low bits legitimately
# vanish. That is physics, not a port defect, so the rule is per-CONFIGURATION
# rather than per-cell: every (content, size, preset) triple must have AT LEAST
# ONE qp where the low bits change the bitstream. A triple that is vacuous at
# every qp would keep passing with the u16 threading ripped out, and fails the
# gate. Vacuous cells are always listed, never silently counted as coverage.
set -uo pipefail
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
read -r -a SIZES <<<"${HBD_SIZES:-64 128}"
read -r -a QPS <<<"${HBD_QPS:-8 20 32 40 55}"
read -r -a PRESETS <<<"${HBD_PRESETS:-6 8 9 10 13}"
read -r -a CONTENTS <<<"${HBD_CONTENTS:-uniform gradient}"
# PER-ISA cells (docs/SUSPECTED-C-BUGS.md #9). C's own encoded bitstream
# depends on the host ISA, and this gate found a FOURTH corner of that on
# 2026-08-28: three cells where the port matches C-on-x86-64 (green in CI, run
# 33219332305) and differs from C-on-aarch64 by exactly +3 bytes on C's side.
#
#   gradient_64_q8_p6    port 1605 B   C(aarch64) 1608 B
#   gradient_128_q8_p8   port 6120 B   C(aarch64) 6123 B
#   gradient_128_q20_p8  port 3015 B   C(aarch64) 3018 B
#
# The port is NOT the variable side, measured two ways: `tier_invariance.rs`
# holds the port's bytes constant across every archmage dispatch tier, and
# these three cells were re-run against the pre-change tree (bfae1b69) in a
# sibling workspace — byte-identical port output there too, so nothing in the
# port moved. A NEW corner relative to the three already in entry #9: bd10 but
# NOT screen content, NOT preset 7, and the LOW-qp end (8 / 20) rather than 55.
#
# Scoped with `uname -m`, exactly as `screen_palette_bd_gate.sh` scopes its
# two: a flat pin list cannot be right on both hosts — pinning unconditionally
# fails x86-64 (where the cell MATCHES) and not pinning fails aarch64. Pins are
# self-promoting: a pinned cell that starts matching FAILS, so the day C stops
# diverging here the gate says so instead of quietly widening.
# Per-ISA pins (SUSPECTED-C-BUGS #9): `arch` rows in the cellrun list pin
# these DIFFERS on aarch64/arm64; everywhere else they are plain cells.
ARCH_KNOWN_DIFF=("gradient_64_q8_p6" "gradient_128_q8_p8" "gradient_128_q20_p8")
in_list() {
  local needle=$1 k; shift
  for k in "$@"; do [[ "$k" == "$needle" ]] && return 0; done
  return 1
}
# The cells are a list for tools/cellrun.py (plan T3), in parallel (HBD_JOBS,
# default 4). Each native-10-bit cell has a port-only WIDENED sibling (the
# same content at bd10 without HBD_SRC); the summary compares their streams
# for the vacuity report and the per-configuration premise.
OUT="${TMPDIR:-$HOME/tmp}/bd10hbd.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
LIST="$OUT/bd10_hbd_src.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv_port\texpect\tcheck\tarch\n' >"$LIST"
add() { # cell triple content sz qp p hbd_env
  local cell=$1 triple=$2 content=$3 sz=$4 qp=$5 p=$6 envp=$7
  printf '%s__w\t%s\t%s\t%s\t%s\t%s\t10\t\t\tnone\t\n' "$cell" "$content" "$sz" "$sz" "$qp" "$p" >>"$LIST"
  if in_list "$cell" "${ARCH_KNOWN_DIFF[@]}"; then
    printf '%s\t%s\t%s\t%s\t%s\t%s\t10\t%s\tIDENTICAL\tc\t!aarch64,!arm64\n' "$cell" "$content" "$sz" "$sz" "$qp" "$p" "$envp" >>"$LIST"
    printf '%s__pin\t%s\t%s\t%s\t%s\t%s\t10\t%s\tDIFFERS\tc\taarch64,arm64\n' "$cell" "$content" "$sz" "$sz" "$qp" "$p" "$envp" >>"$LIST"
  else
    printf '%s\t%s\t%s\t%s\t%s\t%s\t10\t%s\tIDENTICAL\tc\t\n' "$cell" "$content" "$sz" "$sz" "$qp" "$p" "$envp" >>"$LIST"
  fi
  echo "$cell $triple" >>"$OUT/triples"
}
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        add "${content}_${sz}_q${qp}_p${p}" "${content}_${sz}_p${p}" "$content" "$sz" "$qp" "$p" "SVTAV1_HBD_SRC=1"
      done
    done
  done
done
read -r -a PQ_QPS <<<"${HBD_PQ_QPS:-8 20 32}"
read -r -a PQ_PRESETS <<<"${HBD_PQ_PRESETS:-6 8 9}"
for sz in "${SIZES[@]}"; do
  for qp in "${PQ_QPS[@]}"; do
    for p in "${PQ_PRESETS[@]}"; do
      add "pq_gradient_${sz}_q${qp}_p${p}" "pq_gradient_${sz}_p${p}" gradient "$sz" "$qp" "$p" "SVTAV1_HBD_SRC=1;SVTAV1_HBD_PQ=1"
    done
  done
done
python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${HBD_JOBS:-4}"
rc=$?
CELLDIR="$RS_ROOT/target/cells/bd10_hbd_src.cells.${SVT_ORACLE:-default}"
python3 - "$OUT/result.tsv" "$CELLDIR" "$OUT/triples" <<'PY'
import csv, sys
from pathlib import Path
rows = {r["name"]: r for r in csv.DictReader(open(sys.argv[1]), delimiter="\t")}
celldir = Path(sys.argv[2])
triples = [l.split() for l in open(sys.argv[3])]
cells = [r for n, r in rows.items() if not n.endswith("__w") and not n.endswith("__pin")]
pins = [r for n, r in rows.items() if n.endswith("__pin")]
ok = [r for r in cells if r["ok"] == "yes"]
print(f"bd10 NATIVE-10-bit-source identity: {len(ok)} / {len(cells)} byte-identical")
bad = [r for r in cells + pins if r["ok"] != "yes"]
for r in bad:
    why = "PER-ISA pin now MATCHES — remove it" if r["name"].endswith("__pin") and r["verdict"] == "IDENTICAL" else f"{r['verdict']} {r['detail']}"
    print(f"  FAILED: {r['name']} [{why}]")
if pins:
    print(f"  per-ISA pinned (C diverges from itself across hosts, SUSPECTED-C-BUGS #9): "
          + " ".join(r["name"] for r in pins if r["ok"] == "yes"))
live, vac = {}, []
for cell, triple in triples:
    live.setdefault(triple, 0)
    a, b = celldir / cell / "rs.obu", celldir / f"{cell}__w" / "rs.obu"
    if a.exists() and b.exists() and a.read_bytes() == b.read_bytes():
        vac.append(cell)
    elif a.exists() and b.exists():
        live[triple] += 1
if vac:
    print("  vacuous cells (real-10-bit stream == widened-u8 stream — they verify")
    print("  parity but do NOT exercise the u16 path): " + " ".join(vac))
dead = sorted(t for t, n in live.items() if n == 0)
if dead:
    print("  GATE PREMISE FAILED — these configurations are vacuous at EVERY qp,")
    print("  so they would pass with the u16 threading removed: " + " ".join(dead))
print(f"  configurations with at least one low-bits-live qp: {len(live) - len(dead)} / {len(live)}")
sys.exit(1 if bad or dead else 0)
PY
st=$?
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]
