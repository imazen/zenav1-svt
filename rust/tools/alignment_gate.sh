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
#                green and this one does not. bd8 only — at bd10 `last_recon`
#                is the u8 chain, so there is nothing decoder-comparable to
#                compare; bd10 is covered by the byte leg (which is what caught
#                defect 2's bd10 half).
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
    for c in /opt/homebrew/bin/aomdec /usr/local/bin/aomdec /root/aomdec-build/aomdec; do
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
PINNED_DIFF=()
case "$(uname -m)" in
arm64 | aarch64) PINNED_DIFF=("${ARCH_BD10[@]}") ;;
*) CELLS+=("${ARCH_BD10[@]}") ;;
esac

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
pass=0
fail=0
recon_checked=0
recon_px=0
failed=()
echo "alignment_gate: mode=$MODE cells=${#CELLS[@]} aomdec=$AOMDEC"

for cell in "${CELLS[@]}"; do
    read -r content w h qp p bd sp <<<"$cell"
    stride=$((w + sp))
    tag="${content}_${w}x${h}_q${qp}_p${p}_bd${bd}_st${stride}"

    if ! SVTAV1_BD="$bd" SVTAV1_Y_STRIDE="$stride" \
        SVTAV1_FINAL_RECON="$OUT/rs.recon" \
        "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" \
        >"$OUT/rs.log" 2>&1; then
        fail=$((fail + 1))
        failed+=("$tag[port-err]")
        continue
    fi

    # --- BYTE leg -----------------------------------------------------------
    if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" "$bd" \
        >"$OUT/c.log" 2>&1; then
        fail=$((fail + 1))
        failed+=("$tag[c-err]")
        continue
    fi
    if ! cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        fail=$((fail + 1))
        failed+=("$tag[BYTE $(stat -f%z "$OUT/rs.obu" 2>/dev/null ||
            stat -c%s "$OUT/rs.obu")B vs C $(stat -f%z "$OUT/c.obu" 2>/dev/null ||
            stat -c%s "$OUT/c.obu")B]")
        continue
    fi

    # --- RECON leg (bd8) ----------------------------------------------------
    if [[ "$bd" == 8 ]]; then
        rm -f "$OUT/rs.y4m"
        if ! "$AOMDEC" "$OUT/rs.obu" -o "$OUT/rs.y4m" >/dev/null 2>&1; then
            fail=$((fail + 1))
            failed+=("$tag[decode-err]")
            continue
        fi
        verdict=$(python3 - "$OUT/rs.y4m" "$OUT/rs.recon" "$w" "$h" <<'PY'
import sys
y4m, recon, w, h = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
d = open(y4m, "rb").read()
hdr = d.index(b"\n")
fp = d.index(b"FRAME", hdr)
start = d.index(b"\n", fp) + 1
cw, ch = (w + 1) // 2, (h + 1) // 2
need = w * h + 2 * cw * ch
dec = d[start:start + need]
enc = open(recon, "rb").read()
if len(dec) != need:
    print(f"FAIL y4m short: {len(dec)} < {need}")
elif len(enc) != need:
    print(f"FAIL recon size {len(enc)} != {need}")
elif dec == enc:
    print(f"OK {need}")
else:
    n = sum(1 for a, b in zip(dec, enc) if a != b)
    i = next(i for i, (a, b) in enumerate(zip(dec, enc)) if a != b)
    pl = "Y" if i < w * h else ("U" if i < w * h + cw * ch else "V")
    if pl == "Y":
        pos = f"r{i // w} c{i % w}"
    else:
        j = (i - w * h) % (cw * ch)
        pos = f"r{j // cw} c{j % cw}"
    print(f"FAIL {n}px first {pl}@{pos} dec={dec[i]} enc={enc[i]}")
PY
)
        case "$verdict" in
        OK\ *)
            recon_checked=$((recon_checked + 1))
            recon_px=$((recon_px + ${verdict#OK }))
            ;;
        *)
            fail=$((fail + 1))
            failed+=("$tag[RECON ${verdict#FAIL }]")
            continue
            ;;
        esac
    fi
    pass=$((pass + 1))
done

for cell in ${PINNED_DIFF[@]+"${PINNED_DIFF[@]}"}; do
    read -r content w h qp p bd sp <<<"$cell"
    stride=$((w + sp))
    tag="PINNED ${content}_${w}x${h}_q${qp}_p${p}_bd${bd}"
    if ! SVTAV1_BD="$bd" SVTAV1_Y_STRIDE="$stride" \
        "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" \
        >"$OUT/rs.log" 2>&1; then
        fail=$((fail + 1))
        failed+=("$tag[port-err]")
        continue
    fi
    if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" "$bd" \
        >"$OUT/c.log" 2>&1; then
        fail=$((fail + 1))
        failed+=("$tag[c-err]")
        continue
    fi
    if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        fail=$((fail + 1))
        failed+=("$tag NOW MATCHES C — promote it into CELLS")
    else
        pass=$((pass + 1))
    fi
done

echo "alignment gate: $pass / $((pass + fail)) cells (incl. ${#PINNED_DIFF[@]} pinned-DIFFER)"
echo "  recon leg: $recon_checked cells compared, $recon_px samples vs aomdec"
if [[ "$fail" -gt 0 ]]; then
    printf 'FAILED: %s\n' "${failed[@]}"
fi
# Recon-leg anti-vacuity: every bd8 cell must have reached the decoder
# comparison. A run where the leg silently compared nothing is a failure even
# if no cell "failed".
if [[ "$fail" -eq 0 && "$recon_checked" -eq 0 ]]; then
    echo "VACUOUS: the recon leg compared zero cells" >&2
    exit 1
fi
[[ "$fail" -eq 0 ]]
