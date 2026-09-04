#!/usr/bin/env bash
# The port's full-pel MD motion vector against C's, per (block, shape, list, ref).
#
# WHAT IT ASSERTS. For every block C and the port BOTH searched at the same
# origin, shape and (list, ref), C's full-pel ME MV and the port's must be
# EQUAL. C's is `SVT_SUBPEL_OUT`'s `start=` at `stage=0` — the MV
# `read_refine_me_mvs` hands the sub-pel tree, i.e. the output of the whole
# full-pel chain INCLUDING `md_nsq_motion_search`. The port's is
# `PMEDBG`'s `fpme=` (`SVTAV1_CANDDBG` + `SVTAV1_NSQDBG`), which is
# `inter_search_arm`'s `fp_me_mv` at the same point.
#
# WHY IT EXISTS — the blindness this repo wrote down and could not see.
# `docs/INTER-ENCODE-PLAN.md` §1z¹⁵: "`md_nsq_motion_search` is ported and NOT
# called ... an NSQ block therefore takes the square path, and NO ASSERTION IN
# THE REPO CAN SEE THE DIFFERENCE." A gap no test can see is how four inter
# gates went unwired and eighteen panicking cells read as byte divergences.
# This is the assertion. The moment the port's NSQ full-pel MV stops matching
# C's — because `md_nsq_motion_search` is missing, or because
# `read_refine_me_mvs`' `blk_avail_sqi && b_w_ne_h` arm seeds from
# `(sq_sb_me_mv + 4) & ~7` where the port uses `raw_me_mv * 8` — this gate
# says so, on the exact block, instead of a cell's byte count moving by one.
#
# THE KNOWN-OPEN SET IS EXACT, NOT A THRESHOLD, and it is EMPTY. It was not:
# when this gate first ran, two NSQ rows disagreed and were pinned here by
# key. Wiring C's NSQ seed (`sq_sb_me_mv`) and `md_nsq_motion_search` closed
# both, and the gate REFUSED to let that land quietly — it failed with "2
# pinned row(s) now agreeing", which is the direction a pin has to work in if
# it is going to mean anything. The set is emptied in the same commit that
# closed them.
# The gate fails on ANY disagreement now, and it would fail again on a pinned
# row that started agreeing. It is a pin, never a tolerance: there is no count
# to nudge.
#
# IT REPORTS ITS OWN COVERAGE, and that is load-bearing. The two dumps do not
# cover the same block set: C's fires only for blocks that REACH the sub-pel
# tree (`md_subpel_search` has early exits), the port's for every leaf whose
# inter candidates get built. So the gate prints how many rows joined, how
# many were NSQ shapes, and FAILS on zero joined rows — because "0 of 0
# disagree" is the vacuous pass this whole campaign keeps having to unlearn.
# When the NSQ column reads 0, the NSQ path is still unobservable HERE and the
# gate says so in those words rather than implying it was checked.
#
# REQUIRES a `-Wl,--wrap` linker for the C side (macOS ld64 has none — run it
# on a GNU-ld host). That is a harness failure, exit 2, never a pass.
#
# Usage: tools/inter_me_join_gate.sh [outdir]
# Env: IMJ_CELLS — "content w h qp preset" specs, one per line.
set -uo pipefail
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

OUT="${1:-$RS_ROOT/target/inter-me-join}"
mkdir -p "$OUT"

probe=$(mktemp -d)
printf 'void __wrap_probe_fn(void){} int probe_fn(void); int main(void){return 0;}\n' >"$probe/p.c"
if ! cc -o "$probe/p" "$probe/p.c" -Wl,--wrap=probe_fn >/dev/null 2>&1; then
    rm -rf "$probe"
    echo "inter me join gate: FAIL — this linker has no -Wl,--wrap, so C emits no" >&2
    echo "  SVT_SUBPEL_OUT. Run on a GNU-ld host. HARNESS failure, not a result." >&2
    exit 2
fi
rm -rf "$probe"

# The cells. Chosen for SHAPE VARIETY, not for byte verdict: 72x72 is the only
# grid size that is not a multiple of 64, so it is the only one whose frame
# edge forces the partial-superblock shapes an NSQ search would act on, and
# 64/128 are here so a regression on the aligned path cannot hide behind them.
# Cells may be open on bytes — this gate is about the MOTION VECTOR, and an
# open cell's MV is exactly the interesting case.
DEFAULT_CELLS="diag 72 72 40 6
uniform 72 72 40 6
screen 72 72 20 6
gradient 72 72 40 8
diag 128 128 40 8
gradient 64 64 40 6"
CELLS="${IMJ_CELLS:-$DEFAULT_CELLS}"

# The pinned disagreements: `<cell> <SQ|NSQ> org_x org_y bw bh li ri Cy Cx Py Px`.
#
# MEASURED 2026-09-02, and now EMPTY. What used to be here:
#
#   uniform_72x72_q40_p6 NSQ  0 64 64 32 li=1  C=(0,0)  port=(-8,-32)
#   uniform_72x72_q40_p6 NSQ 64  0 32 64 li=1  C=(0,0)  port=(-8,-32)
#
# Both were NSQ shapes straddling the frame edge of a 72x72 picture, both on
# LIST 1, and both were the divergence `docs/INTER-ENCODE-PLAN.md` §1z¹⁵ named
# and could not see. TWO unported things showed through that one observable:
# C's `read_refine_me_mvs` SEEDS an NSQ block whose square parent was tested
# from `(sq_sb_me_mv[list][ref] + 4) & ~0x07`
# (product_coding_loop.c:2857-2862), and then runs `md_nsq_motion_search` on
# the result. Both are wired now and both rows agree.
#
# Keep the format if a row ever has to go back in:
#   <cell> <SQ|NSQ> org_x org_y bw bh li ri Cy Cx Py Px
KNOWN_OPEN=""

joined_total=0; nsq_total=0; differ_total=0; cells=0; unexpected=0; promoted=0
: >"$OUT/observed.txt"
: >"$OUT/report.txt"
echo "== inter ME join gate =="
while read -r content w h qp preset; do
    [[ -n "${content:-}" ]] || continue
    d="$OUT/${content}_${w}x${h}_q${qp}_p${preset}"
    mkdir -p "$d"
    rm -f "$d/subpel.txt"
    SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="${IMJ_SHIFT:-3}" \
        SVTAV1_NSQDBG=1 SVTAV1_CANDDBG=1 SVT_SUBPEL_OUT="$d/subpel.txt" \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$qp" "$preset" 2 "$content" "$d" \
        >"$d/diff.txt" 2>&1
    st=$?
    if [[ $st -eq 4 ]]; then
        echo "  CRASH  $content ${w}x${h} q$qp p$preset — the encoder PANICKED" | tee -a "$OUT/report.txt"
        differ_total=$((differ_total + 1))
        cells=$((cells + 1))
        continue
    fi
    if [[ ! -s "$d/subpel.txt" || ! -s "$d/rs.trace" ]]; then
        echo "  HARNESS  $content ${w}x${h} q$qp p$preset — a dump is missing" | tee -a "$OUT/report.txt" >&2
        exit 2
    fi
    line=$(python3 "$HERE/inter_me_join.py" "$d/subpel.txt" "$d/rs.trace")
    read -r j n dd <<<"$line"
    joined_total=$((joined_total + j)); nsq_total=$((nsq_total + n)); differ_total=$((differ_total + dd))
    cells=$((cells + 1))
    printf '  %-8s %sx%s q%-2s p%-2s  joined=%-3s nsq=%-3s differ=%s\n' \
        "$content" "$w" "$h" "$qp" "$preset" "$j" "$n" "$dd" | tee -a "$OUT/report.txt"
    if [[ "$dd" != "0" ]]; then
        python3 "$HERE/inter_me_join.py" "$d/subpel.txt" "$d/rs.trace" --verbose \
            | sed "s|^|${content}_${w}x${h}_q${qp}_p${preset} |" >>"$OUT/observed.txt"
    fi
done <<<"$CELLS"

# The EXACT set comparison. `comm` needs both sides sorted; an empty pin is
# handled by the `grep -v '^$'` (a bare empty line would join the set and make
# every run look like a promotion).
printf '%s\n' "$KNOWN_OPEN" | grep -v '^[[:space:]]*$' | sort >"$OUT/pinned.txt"
sort "$OUT/observed.txt" >"$OUT/observed.sorted.txt"
mapfile -t NEW < <(comm -13 "$OUT/pinned.txt" "$OUT/observed.sorted.txt")
mapfile -t GONE < <(comm -23 "$OUT/pinned.txt" "$OUT/observed.sorted.txt")
unexpected=${#NEW[@]}
promoted=${#GONE[@]}
if [[ $unexpected -gt 0 ]]; then
    echo "-- disagreements NOT in the pinned set --"
    printf '  %s\n' "${NEW[@]}"
fi
if [[ $promoted -gt 0 ]]; then
    echo "-- pinned rows that now AGREE (remove them from KNOWN_OPEN) --"
    printf '  %s\n' "${GONE[@]}"
fi
echo "-- known-open (pinned) disagreements --"
while read -r row; do
    [[ -n "$row" ]] && echo "  open  $row"
done <"$OUT/pinned.txt"

echo
echo "inter ME join gate: $cells cells, $joined_total joined (block,shape,list,ref) rows, \
$nsq_total NSQ, $differ_total disagree ($(wc -l <"$OUT/pinned.txt" | tr -d ' ') pinned, \
$unexpected unexpected, $promoted now agreeing)"
if [[ $joined_total -eq 0 ]]; then
    echo "inter ME join gate: FAIL — ZERO rows joined, so '0 disagree' asserts nothing." >&2
    echo "  Either no cell reached C's sub-pel tree or the two dumps stopped sharing" >&2
    echo "  a key. This is anti-vacuity, not a parity result." >&2
    exit 1
fi
if [[ $nsq_total -eq 0 ]]; then
    echo "inter ME join gate: FAIL — 0 NSQ rows joined, so the NSQ full-pel path" >&2
    echo "  (md_nsq_motion_search, still uncalled) is NOT observed by this run and" >&2
    echo "  the pinned set below asserts nothing. Anti-vacuity: this gate exists to" >&2
    echo "  SEE that path." >&2
    exit 1
fi
if [[ $unexpected -gt 0 || $promoted -gt 0 ]]; then
    echo "inter ME join gate: FAIL — $unexpected disagreement(s) not pinned," >&2
    echo "  $promoted pinned row(s) now agreeing. The pin is EXACT: a new" >&2
    echo "  divergence is a regression, and a row that starts matching means the" >&2
    echo "  NSQ path moved and KNOWN_OPEN must shrink in the same commit." >&2
    exit 1
fi
echo "inter ME join gate: PASS"
