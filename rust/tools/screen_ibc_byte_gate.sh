#!/usr/bin/env bash
# Screen-content IntraBC BYTE gate — the whole gb82-sc IBC band, byte-exact.
#
# The grid is gb82-sc x presets 0..4 (sc_class5 && M<=4 => C's
# `allow_intrabc = 1`; preset 5 is the first IBC-off preset) x qp {20, 40, 48}
# at 512x512 bd8, PLUS the two REAL-IMAGE cells the callcount record
# (benchmarks/callcount_realimg_2026-09-04.meta) found diverging at preset 2
# qp 40 — `terminal.png` 512x512 and `graph.png` 512x480 (the record's exact
# crops; the 512x480 one keeps a non-64-aligned height in the gate). No cell
# on the old `tools/screen_ibc_gate.sh` grid (qp 20/48) covered qp 40, and
# none of the synthetic `screen`/`screenrep` contents arms IntraBC at all
# (docs/WORKING-ON-THIS.md §5: synthetic content never codes an IntraBC
# block) — so those two divergences sat outside every gate.
#
# Byte-ONLY: unlike `screen_ibc_gate.sh` this needs no `tools/decode_diff`
# (whose Cargo.toml has a literal path dependency that exists only on the CI
# image), so it runs on every host that has the C oracle. Its decode-level
# diagnostics are the other gate's; its anti-vacuity is the PORT's pack-tree
# (`SVTAV1_PACKTREE`), which on a byte-identical cell is C's tree too.
#
# Semantics (the self-promoting pinned-gate house style, as screen_ibc_gate):
#   - every cell listed in BYTE_EXACT is ASSERTED byte-identical to C: a
#     divergence there is a regression (exit 1);
#   - a cell NOT listed is PINNED-DIVERGING (exit 0 while it differs); if it
#     MATCHES the gate FAILS (exit 4) telling you to promote it — a fix must
#     be locked in, never float. As of 2026-09-05 EVERY cell is byte-exact
#     (150/150 + the two record cells), so the pinned set is empty;
#   - anti-vacuity (exit 3): the port must code IntraBC blocks AND luma
#     palette blocks somewhere in the sweep, else this gate proves nothing
#     about the screen tools;
#   - the two record cells also assert their SIZE (5003 B / 3098 B): a
#     byte-identical stream of another size would mean the C oracle moved.
#
# History. 2026-07-23: 22/100 byte-exact, 78 pinned "RD near-ties (KB-2
# family)". 2026-09-05: 150/150 after three mechanisms —
#   1. `mds3.rs`: C keys an IntraBC candidate's tx-depth cap on
#      `is_intra_mode(mode)` (DC_PRED) — the INTRA caps, depth 2 at
#      presets 0..3 — where the port used the inter caps (depth 1);
#   2. `commit.rs`: C's MD-side txfm-context stamp is the CHOSEN tx dims for
#      every winner (no skip&&inter arm — that arm is the pack's);
#   3. `context.rs`: C's MD-side context skips the palette CDF update for
#      non-chroma-reference blocks (4x16 at an even column, 16x4 at an even
#      row); the chain sim now withholds that adaptation too.
#
# Env: SIB_IMGS / SIB_PRESETS / SIB_QPS / SIB_DIM override the grid;
#      SIB_RECORD=0 skips the two record cells (they are on by default);
#      RS_AOMDEC=<aomdec> adds a recon leg on the two record cells (the
#      port's stream must decode to the port's own final recon — the
#      zero-tolerance corruption class); without it that leg is reported as
#      SKIPPED, loudly, and the byte verdict stands.
# Exit: 0 PASS; 1 regression; 2 harness error; 3 anti-vacuity; 4 promote.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
. "$(dirname "$0")/lib_corpus.sh"
# Issue #23: a flat crop makes these gates pass while reaching neither
# palette nor IntraBC. Fail loudly instead of silently (identity_run only
# honours this when set, so synthetic uniform cells are unaffected).
export SVTAV1_ASSERT_NONFLAT=1
RS_ROOT=$(cd "$HERE/.." && pwd)

SCREEN_DIR="${SCREEN_DIR:-$(corpus_dir codec-corpus/gb82-sc)}"
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR
AOMDEC="${RS_AOMDEC:-}"

read -r -a IMGS <<<"${SIB_IMGS:-codec_wiki gmessages graph gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a PRESETS <<<"${SIB_PRESETS:-0 1 2 3 4}"
read -r -a QPS <<<"${SIB_QPS:-20 40 48}"
DIM="${SIB_DIM:-512}"
RECORD="${SIB_RECORD:-1}"

# The measured byte-exact set. 2026-09-05: the full grid (150) + the two
# record cells. Bake cells in as they close; the gate FAILS if a listed cell
# diverges OR an unlisted cell matches.
BYTE_EXACT=()
for img in codec_wiki gmessages graph gui imac_dark imac_g3 imessage terminal windows windows95; do
  for p in 0 1 2 3 4; do
    for qp in 20 40 48; do BYTE_EXACT+=("${img}_p${p}_q${qp}"); done
  done
done
BYTE_EXACT+=("record_terminal_512x512_q40_p2" "record_graph_512x480_q40_p2")

is_byte_exact() {
  local t="$1"
  for c in "${BYTE_EXACT[@]}"; do [ "$c" = "$t" ] && return 0; done
  return 1
}

# The cells are a list for tools/cellrun.py (plan T3), in parallel
# (SIB_JOBS, default 4). Every cell writes its packed tree to @CELL@ and the
# summary totals IBC and palette blocks from those files (run-level
# anti-vacuity, as before). Record cells: a second row encodes the same cell
# with SVTAV1_FINAL_RECON on, must be byte-identical to the first
# (`same_as`: the dump must not change a byte) and must decode to its recon
# (`recon`); the summary also checks the pinned byte size. A missing
# screenshot or decoder FAILS (SIB_ALLOW_NO_RECON=1 accepts no decoder).
OUT="${TMPDIR:-$HOME/tmp}/screen_ibc_byte.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
if [ -z "$AOMDEC" ] || ! command -v "$AOMDEC" >/dev/null 2>&1; then
  if [ "${SIB_ALLOW_NO_RECON:-0}" != 1 ]; then
    echo "screen_ibc_byte_gate: RS_AOMDEC not set or not runnable; the record cells' recon leg needs it (SIB_ALLOW_NO_RECON=1 to skip)" >&2
    exit 2
  fi
fi
LIST="$OUT/screen_ibc_byte.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tenv_port\tenv_c\texpect\tcheck\tsame_as\n' >"$LIST"
row() { # name png w h qp p expect check same_as
  printf '%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_PACKTREE=@CELL@/rs.ptree\tSVT_NO_AUTO_CMAKE=1\t%s\t%s\t%s\n' \
    "$1" "$(screen_crop_spec "$2")" "$3" "$4" "$5" "$6" "$7" "$8" "$9" >>"$LIST"
}
missing=()
for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { missing+=("$img"); continue; }
  for p in "${PRESETS[@]}"; do
    for qp in "${QPS[@]}"; do
      tag="${img}_p${p}_q${qp}"
      if is_byte_exact "$tag"; then row "$tag" "$png" "$DIM" "$DIM" "$qp" "$p" IDENTICAL c ""
      else row "$tag" "$png" "$DIM" "$DIM" "$qp" "$p" DIFFERS c ""; fi
    done
  done
done
declare -A RECORD_BYTES=([record_terminal_512x512_q40_p2]=5003 [record_graph_512x480_q40_p2]=3098)
if [ "$RECORD" = 1 ]; then
  for spec in "record_terminal_512x512_q40_p2 terminal 512 512" "record_graph_512x480_q40_p2 graph 512 480"; do
    read -r tag img w h <<<"$spec"
    png="$SCREEN_DIR/$img.png"
    [ -f "$png" ] || { missing+=("$tag"); continue; }
    row "$tag" "$png" "$w" "$h" 40 2 IDENTICAL c ""
    if [ -n "$AOMDEC" ] && command -v "$AOMDEC" >/dev/null 2>&1; then
      row "${tag}__recon" "$png" "$w" "$h" 40 2 "" recon "$tag"
    fi
  done
fi
AOMDEC="$AOMDEC" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${SIB_JOBS:-4}"
rc=$?
CELLDIR="$RS_ROOT/target/cells/screen_ibc_byte.cells.${SVT_ORACLE:-default}"
python3 - "$OUT/result.tsv" "$CELLDIR" "${missing[@]+"${missing[@]}"}" <<'PY'
import csv, sys
from pathlib import Path
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
celldir, missing = Path(sys.argv[2]), sys.argv[3:]
record_bytes = {"record_terminal_512x512_q40_p2": 5003, "record_graph_512x480_q40_p2": 3098}
def counts(name):
    t = celldir / name / "rs.ptree"
    if not t.exists():
        return 0, 0
    seen, ibc, pal = set(), 0, 0
    for l in t.read_text().splitlines():
        if not l.startswith("PTREE"):
            continue
        f = l.split()
        if len(f) < 2 or f[1] in seen:
            continue
        seen.add(f[1])
        ibc += " ibc=1" in f" {l}" and 1 or 0
        pal += any(x.startswith("pal=") and x[4:5] in "123456789" for x in f)
    return ibc, pal
enc = [r for r in rows if not r["name"].endswith("__recon")]
ibc_total = pal_total = 0
for r in enc:
    i, p = counts(r["name"]); ibc_total += i; pal_total += p
match = [r for r in enc if r["verdict"] == "IDENTICAL"]
diff = [r for r in enc if r["verdict"] == "DIFFERS"]
errs = [r for r in rows if r["verdict"] == "ERROR"]
size_bad = [n for n, b in record_bytes.items()
            if (celldir / n / "rs.obu").exists() and (celldir / n / "rs.obu").stat().st_size != b]
recon_bad = [r for r in rows if r["name"].endswith("__recon") and r["ok"] != "yes"]
print(f"screen_ibc_byte_gate: {len(match)} / {len(enc) + len(missing)} byte-identical, {len(diff)} diverging, "
      f"{len(errs) + len(missing)} errors; port IBC blocks: {ibc_total}, palette blocks: {pal_total}; "
      f"recon legs: {len(recon_bad)} bad")
rc = 0
if ibc_total == 0 or pal_total == 0:
    print(f"ANTI-VACUITY FAIL: no IntraBC ({ibc_total}) or no palette ({pal_total}) block coded anywhere", file=sys.stderr); rc = 3
reg = [r["name"] for r in enc if r["expect"] == "IDENTICAL" and r["verdict"] != "IDENTICAL"]
promo = [r["name"] for r in enc if r["expect"] == "DIFFERS" and r["verdict"] == "IDENTICAL"]
if reg: print("REGRESSION (asserted cell diverged): " + " ".join(reg), file=sys.stderr); rc = rc or 1
if size_bad or recon_bad:
    print(f"FAIL: record size mismatch {size_bad}, recon legs {[r['name'] + ' ' + r['checks'] for r in recon_bad]}", file=sys.stderr); rc = rc or 1
if promo: print("PROMOTE (pinned cell now matches — add to BYTE_EXACT): " + " ".join(promo), file=sys.stderr); rc = rc or 4
if (errs or missing) and rc == 0:
    print("errors: " + " ".join([r["name"] for r in errs] + missing), file=sys.stderr); rc = 2
if rc == 0: print("PASS screen_ibc_byte_gate")
sys.exit(rc)
PY
exit $?
