#!/usr/bin/env bash
# ALIGNMENT / STRIDE GATE — the axis every other gate in this repo misses.
#
# WHY THIS EXISTS (issue #15, three defects deep).
# `recon_parity.sh` (432 cases) and `decode_gate_grid.sh` (120 cells) are
# ENTIRELY 64-ALIGNED SQUARES. `partial_sb_gate.sh` does run partial
# superblocks but is byte-identity-only on synthetic content. Nothing crossed
# "the true frame size differs from the encode grid" with "does the encoder's
# own reconstruction still equal a conforming decoder's", which is how 67 of
# 648 cells could diverge silently — including a real encoder/decoder
# PREDICTION MISMATCH (intra reference samples read from recon rows a decoder
# replicates instead) that byte-identity to C only happened to surface.
#
# Every defect #15 found lived on one of these axes, and each was invisible to
# every pass/fail gate at the time:
#   1. palette SEARCH ran over the whole block instead of the in-frame part
#      (84e3c8627) — needs a STRADDLING block that PICKS palette;
#   2. intra reference samples unclamped to the frame extent (215af947d bd8,
#      0163004cc bd10) — needs a straddling block at both bit depths;
#   3. deblocking filtered past the true frame width (this session) — needs
#      `true % 8 == 4` on an axis, and only shows up in the deblock LEVEL
#      search, whose SSE window is the ALIGNED plane.
#
# WHAT IT VARIES
#   * TRUE-vs-ALIGNED on each axis INDEPENDENTLY, at EVERY residue mod 8
#     (0..7). Residue 4 is the one that puts a 4x4 mi unit at the true edge
#     (defect 3); residues 1..7 straddle; residue 0 is the aligned control.
#   * dims straddling a 64 boundary by +-1 and by odd amounts (63/65/127/129/
#     191/193), so a partial superblock and a full one both occur.
#   * ODD true dims, which take 4:2:0 chroma to CEILING ((w+1)/2) and make the
#     chroma edge land half a luma unit away from the luma edge.
#   * LUMA STRIDE independent of width, with the slack POISONED (identity_run's
#     SVTAV1_Y_STRIDE). A padded stride is exactly what hid
#     `frame_h = y_recon.len() / y_stride` in defect 2, and it is the project's
#     pixel-buffer rule (a multi-row function handles stride != width).
#   * BOTH BIT DEPTHS. Defect 2 needed a SEPARATE bd10 fix (0163004cc) after
#     the bd8 one landed, so a bd8-only gate would have shipped half of it.
#   * CONTENT: `gradient` (photographic character) and `screen` (few distinct
#     luma values -> the screen-content detector arms and palette blocks get
#     picked, which is the only way to reach defect 1).
#
# TWO ORACLES, because one is not enough
#   BYTE leg   — the stream must be byte-identical to the C reference
#                (tools/capture_c_trace on the very .yuv the port just wrote).
#   RECON leg  — the encoder's OWN final reconstruction, cropped to the true
#                coded dims, must equal `aomdec`'s output bit-exactly.
#                This is the leg that makes an encoder/decoder mismatch a
#                CORRECTNESS failure rather than a "we differ from C" failure:
#                if the port and C were wrong the same way, the byte leg stays
#                green and this one does not. BOTH bit depths: at bd8 the
#                dump is `last_recon` (u8); at bd10 it is `last_recon10_final`
#                (u16 LE, issue #13 — the 10-bit canvas with deblock -> CDEF
#                -> LR applied), compared against aomdec's C420p10 y4m sample
#                for sample. Until 2026-08-27 this leg was bd8-only because
#                no post-filter 10-bit recon existed to compare.
#
# TEETH (measured, not asserted — see benchmarks/alignment_gate_teeth_*.md):
# each of the three fixes was reverted one at a time and this gate FAILED.
#
# Usage: tools/alignment_gate.sh
# Env:
#   ALIGN_GATE_MODE   fast (default, CI) | full
#   AOMDEC            path to aomdec (default /opt/homebrew/bin/aomdec, then
#                     /root/aomdec-build/aomdec)
#   ALIGN_GATE_KEEP   keep the scratch dir
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

MODE="${ALIGN_GATE_MODE:-fast}"
case "$MODE" in
fast | full) ;;
*)
    echo "ALIGN_GATE_MODE must be fast or full, got '$MODE'" >&2
    exit 2
    ;;
esac

AOMDEC="${AOMDEC:-}"
if [[ -z "$AOMDEC" ]]; then
    # PATH first: this list alone missed /usr/bin/aomdec (2026-09-26).
    AOMDEC=$(command -v aomdec || true)
    for c in /opt/homebrew/bin/aomdec /usr/local/bin/aomdec /root/aomdec-build/aomdec; do
        [[ -n "$AOMDEC" ]] && break
        [[ -x "$c" ]] && AOMDEC="$c" && break
    done
fi
# The recon leg is REQUIRED, not best-effort: a gate that quietly drops its
# second oracle when a tool is missing is the "graceful skip" this project
# bans. Fail loudly and name the fix.
[[ -x "$AOMDEC" ]] || {
    echo "alignment_gate: no aomdec found — set AOMDEC=<path>." >&2
    echo "  The recon leg is half this gate; running without it is not offered." >&2
    exit 2
}

OUT="${TMPDIR:-$HOME/tmp}/aligngate.$$"
mkdir -p "$OUT"
trap '[[ -n "${ALIGN_GATE_KEEP:-}" ]] || rm -rf "$OUT"' EXIT

# ---------------------------------------------------------------------------
# CELLS: "content true_w true_h qp preset bd stride_pad"
#
# `stride_pad` is added to the true width to form the LUMA stride handed to
# the encoder (0 = tightly packed). The .yuv the C oracle reads is always
# tight, so a nonzero pad proves stride-independence and byte-identity at once.
# ---------------------------------------------------------------------------
CELLS=()

# --- 1. residue sweep, WIDTH axis (height a 64-aligned control) -------------
# 121..128 covers every residue mod 8 with the aligned width landing on 128
# (a full superblock) or 124..127 -> 128; 185..192 puts the aligned width on
# 192 = 3 full SBs, which is where defect 3 lives (188 % 8 == 4).
for w in 121 122 123 124 125 126 127 128 185 186 187 188 189 190 191 192; do
    CELLS+=("gradient $w 128 33 2 8 0")
done
# --- 2. residue sweep, HEIGHT axis (width a 64-aligned control) -------------
for h in 121 122 123 124 125 126 127 128 185 186 187 188 189 190 191 192; do
    CELLS+=("gradient 128 $h 33 2 8 0")
done
# --- 3. BOTH axes unaligned, and unaligned by DIFFERENT amounts ------------
# (a width-only fix passes an axis sweep; a shared-code fix that swaps the two
# extents passes a square sweep. Neither passes this.)
CELLS+=(
    "gradient 188 124 33 2 8 0"
    "gradient 124 188 33 2 8 0"
    "gradient 65 63 33 2 8 0"
    "gradient 63 65 33 2 8 0"
    "gradient 129 127 33 2 8 0"
    "gradient 127 129 33 2 8 0"
    "gradient 193 191 33 2 8 0"
)
# --- 4. STRIDE independent of width ----------------------------------------
# Includes an ALIGNED cell (stride is the only variable there) and unaligned
# ones (stride composes with the true->aligned padding).
CELLS+=(
    "gradient 128 128 33 2 8 64"
    "gradient 128 128 33 6 8 7"
    "gradient 188 256 33 2 8 68"
    "gradient 124 128 33 6 8 33"
    "screen 188 256 33 6 8 64"
)
# --- 5. SCREEN content on straddling dims (the palette-search axis) ---------
# `screen` arms the screen-content detector in BOTH encoders, so palette blocks
# are actually PICKED — without this the palette crop (defect 1) is unreachable
# and every cell above would pass with it reverted.
for wh in "188 256" "124 128" "125 129" "96 88" "190 130" "65 65"; do
    read -r w h <<<"$wh"
    CELLS+=("screen $w $h 33 2 8 0" "screen $w $h 12 4 8 0")
done
# The PALETTE-CROP cells specifically (defect 1). Found by measurement, not by
# reasoning: with 84e3c8627 reverted the six cells above at q33/q12 all still
# PASSED — the padded columns only change the colour histogram / k-means seed
# when the block's in-frame part is a MINORITY of its colours, which on this
# content needs a coarse quantizer (q55) and a preset whose search reaches
# palette (4 and 6). Every one of these FAILS with the crop reverted; every one
# passes with it. Aligned height 128 with true 88 => a 40-row bottom straddle.
CELLS+=(
    "screen 96 88 55 4 8 0"
    "screen 96 88 55 6 8 0"
    "screen 104 88 55 4 8 0"
    "screen 88 88 55 6 8 0"
    "screen 72 88 55 4 8 0"
    "screen 80 88 55 6 8 0"
)
# --- 6. bd10 -------------------------------------------------------------
# Defect 2 needed a SEPARATE bd10 fix after the bd8 one landed. Byte leg only.
for wh in "188 256" "124 128" "125 129" "96 88"; do
    read -r w h <<<"$wh"
    CELLS+=("screen $w $h 33 6 10 0")
done
for wh in "124 128" "125 129" "96 88" "188 192" "192 192"; do
    read -r w h <<<"$wh"
    CELLS+=("gradient $w $h 33 2 10 0")
done

# ---------------------------------------------------------------------------
# PINNED RESIDUAL — cells that must DIFFER from C.
#
# Same self-promoting shape as `bd10_partial_sb_gate.sh`: if one of these
# starts matching, this gate goes RED telling you to promote it, so a fix
# cannot land unnoticed and the residual cannot silently grow either.
#
# These are NOT alignment failures. MEASURED 2026-08-14, bd10 qp33:
#
#   dims      p2 gradient   p6 gradient   p10 screen
#   192x192   IDENTICAL     IDENTICAL     DIFFERS
#   192x256   DIFFERS       IDENTICAL     DIFFERS
#   256x256   DIFFERS       IDENTICAL     DIFFERS
#   128x256   DIFFERS       IDENTICAL     DIFFERS
#   188x256   DIFFERS       IDENTICAL     DIFFERS
#
# 192x256 and 256x256 are FULLY 64-ALIGNED and diverge exactly like 188x256,
# so the axis is bit depth x preset x frame size, not alignment. The
# gradient/p2 divergence is a LUMA INTRA MODE flip at op 41 of 2539 (C picks a
# directional mode + angle delta, the port picks DC) in the FIRST superblock —
# nowhere near a frame edge. No bd10 gate reached these shapes:
# `bd10_matrix.sh` sweeps BD10_SIZES=64 128 and `bd10_nonflat_gate.sh` only
# 64x64/128x128.
#
# ARCHITECTURE-SCOPED, and that is itself a measurement. All three of these
# DIFFER on aarch64 and MATCH C on the x86-64 CI runner (run 31770480641:
# "71 / 74 cells", the three pinned rows reporting "NOW MATCHES C"), with every
# other cell green on both. The port is not the variable side —
# `svtav1/tests/tier_invariance.rs` pins identical bytes across every archmage
# dispatch tier and the scalar tier is portable integer Rust — so C emits
# different bytes for the same input on the two hosts. That is a THIRD instance
# of docs/SUSPECTED-C-BUGS.md #9, and the cleanest witness yet: three cells that
# flip verdict on host architecture alone, at bd10, on both gradient and screen
# content, at presets 2 and 10, on aligned AND unaligned dims.
#
# So each host asserts something real: aarch64 pins them as expected-DIFFER,
# x86-64 gates them as expected-MATCH.
ARCH_BD10=(
    "gradient 188 256 33 2 10 0"
    "gradient 192 256 33 2 10 0" # the ALIGNED control — the evidence above
    "screen 192 192 33 10 10 0"  # aligned, square, and still divergent
)
# ARCH_BD10 rows carry an `arch` column in the cellrun list below: pinned
# DIFFERS (a match fails as "promote it") on aarch64/arm64, full cells
# (bytes + recon) elsewhere.

if [[ "$MODE" == full ]]; then
    # --- 7. FULL: preset and qp breadth over the shapes that matter --------
    for wh in "188 256" "124 128" "125 129" "63 65" "190 130" "96 88" "200 120"; do
        read -r w h <<<"$wh"
        for p in 0 4 6 8 10 13; do
            for q in 12 55; do
                CELLS+=("gradient $w $h $q $p 8 0" "screen $w $h $q $p 8 0")
            done
        done
    done
    # --- 8. FULL: the width/height residue sweeps at a second preset -------
    for w in 121 124 125 188 189 191; do
        CELLS+=("gradient $w 128 55 6 8 0" "gradient 128 $w 55 6 8 0")
    done
    # --- 9. FULL: bd10 breadth --------------------------------------------
    for wh in "188 256" "124 128" "63 65" "190 130"; do
        read -r w h <<<"$wh"
        for p in 2 6 10; do
            CELLS+=("gradient $w $h 12 $p 10 0" "screen $w $h 55 $p 10 0")
        done
    done
fi

# ---------------------------------------------------------------------------
# Static coverage assertions. These do NOT prove the gate has teeth (only the
# revert experiments do), but they DO prove the cell list still spans the axes
# the header claims — an edit that quietly drops the bd10 block or the stride
# block fails here instead of silently narrowing the gate.
# ---------------------------------------------------------------------------
declare -a wres hres
for cell in "${CELLS[@]}"; do
    read -r _c w h _q _p bd sp <<<"$cell"
    wres[$((w % 8))]=1
    hres[$((h % 8))]=1
    [[ "$bd" == 10 ]] && saw_bd10=1
    [[ "$sp" != 0 ]] && saw_stride=1
    [[ "$_c" == screen ]] && saw_screen=1
done
cov_fail=0
for r in 0 1 2 3 4 5 6 7; do
    [[ -n "${wres[$r]:-}" ]] || {
        echo "COVERAGE FAIL: no cell with true_width % 8 == $r" >&2
        cov_fail=1
    }
    [[ -n "${hres[$r]:-}" ]] || {
        echo "COVERAGE FAIL: no cell with true_height % 8 == $r" >&2
        cov_fail=1
    }
done
for v in saw_bd10 saw_stride saw_screen; do
    [[ -n "${!v:-}" ]] || {
        echo "COVERAGE FAIL: $v — the cell list no longer spans that axis" >&2
        cov_fail=1
    }
done
[[ "$cov_fail" -eq 0 ]] || exit 1

# ---------------------------------------------------------------------------
# The cells are a list for tools/cellrun.py (plan T3): C bytes plus the
# recon leg (aomdec's output == the port's final recon, 16-bit at bd10) on
# every cell; the stride is port-only (the .yuv C reads stays tight).
echo "alignment_gate: mode=$MODE cells=$((${#CELLS[@]} + ${#ARCH_BD10[@]})) aomdec=$AOMDEC"
LIST="$OUT/alignment.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv_port\texpect\tcheck\tarch\n' >"$LIST"
row() { # expect check arch cell
    local expect=$1 check=$2 arch=$3
    read -r content w h qp p bd sp <<<"$4"
    local stride=$((w + sp))
    printf '%s_%sx%s_q%s_p%s_bd%s_st%s%s\t%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_Y_STRIDE=%s\t%s\t%s\t%s\n' \
        "$content" "$w" "$h" "$qp" "$p" "$bd" "$stride" "${arch:+_$([[ $expect == DIFFERS ]] && echo pin || echo x86)}" \
        "$content" "$w" "$h" "$qp" "$p" "$bd" "$stride" "$expect" "$check" "$arch" >>"$LIST"
}
# The width and height sweeps both contain 128x128; the old loop ran it twice.
declare -A seen_cell
for cell in "${CELLS[@]}"; do
    [[ -n "${seen_cell[$cell]:-}" ]] && continue
    seen_cell[$cell]=1
    row IDENTICAL c,recon "" "$cell"
done
for cell in "${ARCH_BD10[@]}"; do
    row IDENTICAL c,recon '!aarch64,!arm64' "$cell"
    row DIFFERS c 'aarch64,arm64' "$cell"
done
AOMDEC="$AOMDEC" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${ALIGN_GATE_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
pins = [r for r in rows if r["expect"] == "DIFFERS"]
ok = [r for r in rows if r["ok"] == "yes"]
recon = [r for r in rows if "recon=ok" in r["checks"]]
print(f"alignment gate: {len(ok)} / {len(rows)} cells (incl. {len(pins)} pinned-DIFFER)")
print(f"  recon leg: {len(recon)} cells compared vs aomdec")
for r in rows:
    if r["ok"] != "yes":
        why = "NOW MATCHES C — promote it" if r["expect"] == "DIFFERS" and r["verdict"] == "IDENTICAL" else f"{r['verdict']} {r['detail']}; {r['checks']}"
        print(f"FAILED: {r['name']} [{why}]")
if not recon:
    print("VACUOUS: the recon leg compared zero cells")
    sys.exit(1)
PY
vac=$?
[[ "$rc" -eq 0 && "$vac" -eq 0 ]]
