#!/usr/bin/env bash
# bd10 PHOTOGRAPHIC identity gate (task #94, low-preset/real-content axis).
#
# Real 10-bit photographic content, byte-identical to the real C encoder. Every
# cell below was verified with `cmp` at the commit that added it.
#
# WHY THIS GATE EXISTS: every other bd10 gate feeds SYNTHETIC content
# (uniform/gradient/diag). Synthetic patterns exercise a narrow slice of the
# mode/tx/partition space, so a port can be synthetic-green and still diverge on
# the content users actually encode — which is exactly what happened here:
# docs/bd10-port-map.md recorded an 18/18 photographic FAILURE ("on real content
# the partition tree ALWAYS diverges at bd10"). That measurement predated the
# PD0_LVL_0 partition fix, the TXS-coupling gate, the bd10 chroma re-encode and
# the AVX2-hadamard fix; with all four landed, photographic bd10 at eff-M9 is
# byte-identical. This gate pins that so it cannot silently regress.
#
# SCOPE — every preset band except 4: eff-M9 (presets 9..13, groups A-C),
# presets 7-8 (group D), preset 6 (group E), preset 5 (group F) and presets
# 0-3 (group G). Preset 4 is the one remaining ungated photographic band —
# re-measured 2026-07-23 at 13/15 on {1001682,2119713,4666751,2738653,
# 7062227} x q{5,32,55} (DIFF: 2119713 q32, 7062227 q5); gate it when the
# p4 residual closes (docs/bd10-port-map.md REMAINING #3).
#
# Presets 7-8 were closed 2026-07-19 by the TXT rate-cost gate lambda fix
# (leaf_funnel.rs `txt_search`): C prices that gate with the SAME `full_lambda`
# it prices the tx-type cost with — `hbd_md ? full_lambda_md[EB_10_BIT_MD] :
# [EB_8_BIT_MD]` (product_coding_loop.c:4590/4714/4944) — while the port had the
# cost on the bd10 lambda and the gate on the u8 one, so at bd10 the gate
# under-fired and the port picked tx types C prunes before quantizing them.
# Photographic p6-p8 went 2/27 -> 18/27 (all of p7+p8).
#
# Preset 6 CLOSED 2026-07-19 (group E below): its residual was the CDEF and
# Wiener-LR SEARCHES still running at 8 bits — p6 is the only preset in this
# gate that runs either. Both now run at true 10 bits. (The "presets 0-5
# diverge on the 8-bit DEBLOCK-LEVEL full search" note that used to sit here
# is history: that root and the luma/chroma MD near-tie roots after it were
# closed one by one — group F = p5, group G = p0-3; see docs/bd10-port-map.md.)
#
# CORPUS: CID22-512 (250 real 512x512 photographic PNGs, natively 64-aligned).
# Override with BD10_PHOTO_CORPUS=<dir>. If the corpus is absent this gate FAILS
# LOUDLY — it never skips silently, because a gate that quietly passes without
# encoding anything is worse than one that fails.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

# Reference decoder for the RECON assert (aomdec output == the port's recon;
# until 2026-09-26 only decodability). Required, never
# skipped: byte-identity alone is blind to a cell that regresses OUT of the
# lists below, so every port OBU this gate produces must also be provably
# decodable. Same contract as bd10_nonflat_gate.sh. Override with AOMDEC=.
aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
    for _c in aomdec /root/aomdec-debug/aomdec; do
        if command -v "$_c" >/dev/null 2>&1; then aomdec=$_c; break; fi
    done
fi
command -v "$aomdec" >/dev/null 2>&1 || {
    echo "bd10 photo gate: aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}

CORPUS="${BD10_PHOTO_CORPUS:-$(corpus_dir codec-corpus/CID22/CID22-512/training)}"
if [ ! -d "$CORPUS" ]; then
    echo "bd10 photo gate: corpus not found at $CORPUS" >&2
    echo "  set BD10_PHOTO_CORPUS=<dir of 512x512 PNGs> to point at it." >&2
    exit 1
fi

# Images verified byte-identical at every qp x preset in the grids below.
# Named explicitly (NOT selected by index into `ls`) so the gate is stable
# against corpus additions/removals.
IMAGES_A=(1001682 1484678 2119713 2738653 4666751)
QPS_A=(12 32 55)
IMAGES_B=(1080721 1454613116 167491 2208891 2666598 3571065 708587 pexels-photo-3214683)
QPS_B=(5 20 40 63)
PRESETS=(10 13)
# The rest of the eff-M9 band. Presets 9/11/12 share the closed envelope but
# were never gated (the synthetic gate was pinned on 10 and 13 only); measured
# 18/18 byte-identical on the group-C images below.
IMAGES_C=(1001682 2119713 4666751)
QPS_C=(12 40)
PRESETS_C=(9 11 12)
# Group D — presets 7-8, closed by the TXT rate-cost gate lambda fix (header).
# These are the first photographic cells below the eff-M9 band to be
# byte-identical: at p7/p8 `bd10_full_rd_supported` is TRUE, so unlike groups
# A-C they exercise the whole bd10 full-RD leaf (10-bit TXT search, tx-depth
# loop, chroma full loop and the bd10 CfL arm), not just the level re-encode.
# 18/18 verified with `cmp` at the commit that added them.
IMAGES_D=(1001682 2119713 4666751)
QPS_D=(12 32 55)
PRESETS_D=(7 8)
# Group E — preset 6, closed 2026-07-19 by running the CDEF strength search
# AND the Wiener loop-restoration search at TRUE 10 bits.
#
# p6 is the ONLY preset in this gate that runs those two searches at all:
# `cdef::allintra_preset_uses_cdef_search` is `preset <= 6` (presets 7-13 take
# the closed-form qp picker, already bd10-aware) and
# `restoration::wn_filter_ctrls_allintra` disables LR above preset 6. Both
# searches read the reconstructed frame and write frame-header syntax, so
# running them on the u8 recon at bd10 was a bitstream divergence, not a recon
# approximation — measured as exactly two things on 2119713 q32 p6:
# `cdef_y_sec_strength[1] C=2 Rust=0` plus the Wiener taps
# (docs/bd10-port-map.md, "p6's residual is the 8-bit CDEF/LR SEARCH").
# They now run on the 10-bit post-deblock / post-CDEF canvas with
# coeff_shift = 2 and the bd10 lambdas. 9/9 verified with `cmp`.
IMAGES_E=(1001682 2119713 4666751)
QPS_E=(5 12 32 55)
PRESETS_E=(6)
# q5 added 2026-07-23 with the C exchange-sort tie-semantics fix
# (c_exchange_sort_by, leaf_funnel.rs): near-lossless qindex makes exact
# 64-bit MDS cost TIES common, and C's swap-on-`<` exchange sort orders a
# tie group differently from a stable sort whenever a strictly-smaller
# cost follows it — which decides the MDS3 survivor set. The wider-corpus
# sweep caught it on clic2025 8426ed... bd10 p6 q5 (group G below); the
# three group-E images at q5 verified `cmp`-identical at the same commit.
# Group F — preset 5, closed 2026-07-19 by running the M2..M5 CHROMA mode
# decision at TRUE 10 bits. `search_best_mds3_uv_mode` (the M2..M5 uv-mode
# search) and `check_best_indepedant_cfl` (the ind-uv CfL arbitration) ran
# ENTIRELY at 8 bits at bd10: the port priced the uv full loop with the u8
# `chroma_eval` + u8 lambda and gated the CfL arm on `bd10_rd.is_none()`. C
# runs BOTH at hbd_md — `full_lambda_md[EB_10_BIT_MD]` with 10-bit prediction /
# residual / full-loop distortion (product_coding_loop.c:7307/7397/7443,
# :3899). Deciding the uv mode / CfL on u8 chroma flipped near-ties (uv V-vs-DC
# at block (0,0), CfL-vs-non-CfL at (16,80) on 1001682 q12 p5) which cascaded
# into partition/mode divergence across the frame. Fixed by running the M2..M5
# uv search + the ind-uv CfL arbitration on `chroma_eval10` + `b.lambda` at
# bd10 (leaf_funnel.rs). ALL 5 group-A images x q{12,32,55} at p5 are now
# byte-identical (15/15, verified `cmp`). p4 is 11/15 and p2/p3 diverge on a
# SEPARATE, still-open LUMA partition RD near-tie (docs/bd10-port-map.md).
IMAGES_F=(1001682 1484678 2119713 2738653 4666751)
QPS_F=(12 32 55)
PRESETS_F=(5)
# Group G — the wider-corpus q5 near-tie repro (2026-07-23). clic2025
# `8426ed...` (~2.7MP photo, CENTER-CROPPED to 512 via crop:, the sweep's
# convention) at bd10 p6 q5: the ONLY non-p0 photographic divergence in the
# 2026-07-22 wider-corpus sweep. Root: the MDS1 full-cost sort hit an EXACT
# tie (SMOOTH 2710447 == DC 2710447 at blk(472,208) 8x8) and the port's
# stable sort kept SMOOTH into MDS3 where C's exchange sort keeps DC — 305
# coded-tree flips downstream, first divergent tile symbol the chroma Wiener
# taps (the LR search itself was exact; only its recon inputs differed).
# Byte-identical since the c_exchange_sort_by fix. The corpus is a LOCAL
# resource (like sb128_gate's SC_CORPUS cells): override with
# BD10_CLIC_CORPUS; if absent the cell FAILS LOUDLY as missing — never a
# silent skip.
CLIC_CORPUS="${BD10_CLIC_CORPUS:-$(corpus_dir codec-corpus/clic2025)}"
IMAGE_G="$CLIC_CORPUS/final-test/8426ed2245c791232862b0a0b2a62a1f17031e8e6e38921fe939df0b3a05ac41.png"
# Group H — presets 0-3 (the last photographic band), closed 2026-07-23 by
# TWO C-exact NON-STABLE sorts at bd10 (leaf_funnel.rs): C's candidate sorts
# (`sort_full_cost_based_candidates` :1438, `sort_fast_cost_based_candidates`
# :1415) are swap-on-`<` selection sorts — when a strictly-cheaper candidate
# bubbles up from position k, the displaced element lands at k BEHIND a tied
# rival it originally preceded, so on an EXACT cost tie the survivor order
# differs from the port's old stable `sort_by_key`.
#   * post-MDS1 (`order1`): tie 1297503 between modes 12/11 on 7062227 q5 p1
#     4x4 mi=(69,56) -> port coded mode 12/SPLIT where C codes mode 8/NONE at
#     the parent 8x8. Closed 7062227 q5 p1 + q5 p2 + the CLIC crop q5 p2: the
#     540-cell p0-p3 x q{5,20,32,48,63} sweep went 537/540 -> 540/540.
#   * MDS0 per-class (`sort_lane`): 2119713 q5 p1 (an image OUTSIDE that
#     sweep) still diverged — two angle deltas of one y_mode predicting
#     identically tie at BOTH fast and full cost, so the post-MDS1 tie-break
#     inherits the MDS0 ORDER (op 66843: C angle_delta 0, port -1;
#     decode-identical streams). The exchange sort at `sort_lane` closed it.
# Every cell below byte-verified on the final build (this gate 187/187).
IMAGES_H=(1001682 2119713 4666751 7062227)
QPS_H=(5 32)
PRESETS_H=(0 1 2 3)
# The third proven-flipped cell is a CLIC center-crop (crop: spec, not file:).
# Full content-spec cells: "spec|W|H|qp|preset".
CLIC_DIR_G="${BD10_PHOTO_CLIC_DIR:-$(corpus_dir codec-corpus/clic2025)}"
SPEC_CELLS_H=(
    "crop:$CLIC_DIR_G/final-test/02809272b4ca9b08af45771501b741296187c7e26907efb44abbbfcb6cd804f7.png|512|512|5|2"
)

# The cells are a list for tools/cellrun.py (plan T3), in parallel
# (BD10_PHOTO_JOBS, default 4). CHECK is `recon` by default: aomdec's 10-bit
# output must equal the port's final recon, which subsumes "aomdec accepts
# it" (this gate's check until 2026-09-26); BD10_PHOTO_CHECK=decodes
# restores the weaker one. A missing PNG fails its cell.
OUT="${TMPDIR:-$HOME/tmp}/bd10photo.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
LIST="$OUT/bd10_photo.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tcheck\n' >"$LIST"
missing=()
spec_row() { # tag spec qp preset
    printf '%s\t%s\t512\t512\t%s\t%s\t10\tc,%s\n' "$1" "$2" "$3" "$4" "${BD10_PHOTO_CHECK:-recon}" >>"$LIST"
}
grid() { # images-array-name qps-array-name presets-array-name
    local -n imgs=$1 qps=$2 ps=$3
    local stem qp p
    for stem in "${imgs[@]}"; do
        for qp in "${qps[@]}"; do
            for p in "${ps[@]}"; do
                if [ -f "$CORPUS/$stem.png" ]; then
                    spec_row "${stem}_q${qp}_p${p}" "file:$CORPUS/$stem.png" "$qp" "$p"
                else
                    missing+=("${stem}_q${qp}_p${p}[no-png]")
                fi
            done
        done
    done
}
grid IMAGES_A QPS_A PRESETS
grid IMAGES_B QPS_B PRESETS
grid IMAGES_C QPS_C PRESETS_C
grid IMAGES_D QPS_D PRESETS_D
grid IMAGES_E QPS_E PRESETS_E
grid IMAGES_F QPS_F PRESETS_F
if [ -f "$IMAGE_G" ]; then
    spec_row clic8426ed_q5_p6 "crop:$IMAGE_G" 5 6
else
    missing+=("clic8426ed_q5_p6[no-png: set BD10_CLIC_CORPUS]")
fi
grid IMAGES_H QPS_H PRESETS_H
for spec in "${SPEC_CELLS_H[@]}"; do
    IFS='|' read -r content sw sh sqp sp <<<"$spec"
    tag="$(basename "${content#*:}" .png | cut -c1-12)_q${sqp}_p${sp}"
    if [ -f "${content#*:}" ]; then spec_row "$tag" "$content" "$sqp" "$sp"; else missing+=("${tag}[no-png]"); fi
done
AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${BD10_PHOTO_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" "${missing[@]+"${missing[@]}"}" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
missing = sys.argv[2:]
ok = [r for r in rows if r["ok"] == "yes" and r["verdict"] == "IDENTICAL"]
print(f"bd10 photographic identity: {len(ok)} / {len(rows) + len(missing)} byte-identical")
bad = [f"{r['name']} [{r['verdict']} {r['detail']}; {r['checks']}]" for r in rows if r not in ok] + missing
if bad:
    print("FAILED: " + "\nFAILED: ".join(bad))
sys.exit(1 if bad else 0)
PY
st=$?
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]
