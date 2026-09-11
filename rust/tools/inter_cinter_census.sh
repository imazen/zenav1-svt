#!/usr/bin/env bash
# WHICH inter features does C's CODED output actually use, per cell?
#
# `inter_byte_matrix.sh` says WHICH cells differ. This says WHAT IS IN C's
# frame-1 decision on each of them — one row per cell, joined against that
# same BOTH / F1DIFF / F0DIFF verdict.
#
# WHY IT EXISTS. `inter_md_arm`'s header lists eight suppressed controls
# (compound, warped motion, OBMC, inter-intra, 3x3 refinements, NEAR DRL, NSQ
# ME...). Each is a plausible "next mechanism" and picking between them by
# reading the C source is exactly the guess this campaign keeps having to
# retract. C's own `SVT_CINTER_OUT` line already carries the answer for every
# one of them — `rf=%d,%d` names compound, `mm=%d` names warped/OBMC, `iiu=%d`
# names inter-intra, `bsize` names NSQ, `drl=%d` names the DRL index — so this
# script COUNTS them instead of arguing about them.
#
# A ZERO here is a real finding and the reason the anti-vacuity check below
# exists: "C never codes a compound block on any cell that differs" would mean
# compound prediction cannot be the next mechanism, and that verdict must not
# be reachable by a harness that simply never ran (docs/WORKING-ON-THIS.md §5,
# "a silent harness and a genuine absence are indistinguishable").
#
# REQUIRES a `-Wl,--wrap` linker: the dump lives in
# `capture_c_trace/wrap_recon.c`'s `__wrap_svt_aom_update_mi_map`, and Apple's
# ld64 has no --wrap, so on macOS `capture_c_trace` is the byte-only driver and
# NO dump is produced. That is a harness failure, not "C codes nothing", and
# this script FAILS on it rather than printing 96 rows of zeros.
#
# Usage: tools/inter_cinter_census.sh [outdir]
# Env: same grid vars as inter_byte_matrix.sh (IBM_CONTENT/SIZES/QPS/PRESETS/
#      FRAMES/SHIFT), plus ICC_POC (default 1, the inter frame).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

CONTENTS="${IBM_CONTENT:-uniform gradient diag screen}"
SIZES="${IBM_SIZES:-16 64 72 128}"
QPS="${IBM_QPS:-20 40 55}"
PRESETS="${IBM_PRESETS:-6 8}"
FRAMES="${IBM_FRAMES:-2}"
SHIFT="${IBM_SHIFT:-3}"
POC="${ICC_POC:-1}"
OUT="${1:-$RS_ROOT/target/inter-cinter-census}"
mkdir -p "$OUT"

# The linker probe, spelled exactly as capture_c_trace/build.sh spells it, so
# the two can never disagree about which driver exists.
probe=$(mktemp -d)
printf 'void __wrap_probe_fn(void){} int probe_fn(void); int main(void){return 0;}\n' >"$probe/p.c"
if ! cc -o "$probe/p" "$probe/p.c" -Wl,--wrap=probe_fn >/dev/null 2>&1; then
    rm -rf "$probe"
    echo "inter_cinter_census: FAIL — this linker has no -Wl,--wrap, so" >&2
    echo "  capture_c_trace is the byte-only driver and emits no SVT_CINTER_OUT." >&2
    echo "  Run this on a GNU-ld host (ssh r7900x) or in tools/ctrace-linux." >&2
    echo "  This is a HARNESS failure, not a measurement." >&2
    exit 2
fi
rm -rf "$probe"

# C `BlockSize`: the SQUARES are 4x4/8x8/16x16/32x32/64x64/128x128. Everything
# else is an NSQ shape, which is what `md_nsq_motion_search` keys on
# (`bwidth != bheight`).
is_sq() { case "$1" in 0|3|6|9|12|15) return 0 ;; *) return 1 ;; esac; }

printf 'content\tsize\tqp\tpreset\tverdict\tblocks\tcompound\tnsq\tmotmode\tinterintra\tdrl_nz\tglobalmv\tnearmv\trefs\tmodes\n'
tot_cells=0; tot_blocks=0; tot_comp=0; tot_nsq=0; tot_mm=0; tot_ii=0; tot_drl=0; tot_gmv=0; tot_near=0
for content in $CONTENTS; do
  for s in $SIZES; do
    for q in $QPS; do
      for p in $PRESETS; do
        d="$OUT/${content}_${s}x${s}_q${q}_p${p}"
        mkdir -p "$d"
        rm -f "$d/cinter.txt"
        SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="$SHIFT" \
          SVT_CINTER_OUT="$d/cinter.txt" \
          "$HERE/identity_diff_inter.sh" "$s" "$s" "$q" "$p" "$FRAMES" "$content" "$d" \
          >"$d/diff.txt" 2>&1
        st=$?
        # A CRASH is not a verdict about bytes. The C-side counts below are
        # still meaningful (C ran fine), so the row is kept — but it says
        # CRASH, because a frame-1 panic leaves rs.obu.f0 on disk and would
        # otherwise be scored as an ordinary F1DIFF.
        if [[ $st -eq 4 ]]; then v=CRASH
        elif [[ $st -eq 3 || ! -s "$d/c.obu.pts0" || ! -s "$d/rs.obu.f0" ]]; then
          printf '%s\t%s\t%s\t%s\tHARNESS\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\n' "$content" "$s" "$q" "$p"
          continue
        elif ! cmp -s "$d/c.obu.pts0" "$d/rs.obu.f0"; then v=F0DIFF
        elif [[ -f "$d/c.obu.pts$POC" && -f "$d/rs.obu.f$POC" ]] \
             && cmp -s "$d/c.obu.pts$POC" "$d/rs.obu.f$POC"; then v=BOTH
        else v=F1DIFF; fi

        n=0; comp=0; nsq=0; mm=0; ii=0; drl=0; gmv=0; near=0
        refs=""; modes=""
        while read -r line; do
          # `poc=N` filter first — frame 0's intra blocks never reach this dump
          # (it prints only `mode >= NEARESTMV`), but a future GOP might.
          [[ "$line" == *"poc=$POC "* ]] || continue
          bs=$(sed -n 's/.* bsize=\([0-9-]*\) .*/\1/p' <<<"$line")
          rf0=$(sed -n 's/.* rf=\([0-9-]*\),.*/\1/p' <<<"$line")
          rf1=$(sed -n 's/.* rf=[0-9-]*,\([0-9-]*\) .*/\1/p' <<<"$line")
          md=$(sed -n 's/.* mode=\([0-9-]*\) .*/\1/p' <<<"$line")
          mmv=$(sed -n 's/.* mm=\([0-9-]*\) .*/\1/p' <<<"$line")
          iiv=$(sed -n 's/.* iiu=\([0-9-]*\) .*/\1/p' <<<"$line")
          dl=$(sed -n 's/.* drl=\([0-9-]*\) .*/\1/p' <<<"$line")
          n=$((n+1))
          [[ "$rf1" != "-1" ]] && comp=$((comp+1))
          is_sq "$bs" || nsq=$((nsq+1))
          [[ "$mmv" != "0" ]] && mm=$((mm+1))
          [[ "$iiv" != "0" ]] && ii=$((ii+1))
          [[ "$dl" != "0" ]] && drl=$((drl+1))
          [[ "$md" == "15" ]] && gmv=$((gmv+1))
          [[ "$md" == "14" ]] && near=$((near+1))
          [[ ",$refs," == *",$rf0/$rf1,"* ]] || refs="${refs:+$refs,}$rf0/$rf1"
          [[ ",$modes," == *",$md,"* ]] || modes="${modes:+$modes,}$md"
        done <"$d/cinter.txt" 2>/dev/null
        printf '%s\t%s\t%s\t%s\t%s\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%s\t%s\n' \
          "$content" "$s" "$q" "$p" "$v" "$n" "$comp" "$nsq" "$mm" "$ii" "$drl" "$gmv" "$near" \
          "${refs:--}" "${modes:--}"
        tot_cells=$((tot_cells+1)); tot_blocks=$((tot_blocks+n)); tot_comp=$((tot_comp+comp))
        tot_nsq=$((tot_nsq+nsq)); tot_mm=$((tot_mm+mm)); tot_ii=$((tot_ii+ii))
        tot_drl=$((tot_drl+drl)); tot_gmv=$((tot_gmv+gmv)); tot_near=$((tot_near+near))
      done
    done
  done
done

printf '\n# cells=%d blocks=%d compound=%d nsq=%d motion_mode=%d interintra=%d drl_nz=%d globalmv=%d nearmv=%d\n' \
  "$tot_cells" "$tot_blocks" "$tot_comp" "$tot_nsq" "$tot_mm" "$tot_ii" "$tot_drl" "$tot_gmv" "$tot_near"

# ANTI-VACUITY. Every count above may legitimately be zero — that IS the
# finding this tool exists to produce — but `blocks` may not be, because a
# zero there means no dump was parsed and every other zero is meaningless.
if [[ $tot_blocks -eq 0 ]]; then
  echo "inter_cinter_census: FAIL — parsed ZERO coded inter blocks across $tot_cells cells." >&2
  echo "  Every feature count above is therefore vacuous. Check that" >&2
  echo "  capture_c_trace is the --wrap build and that poc=$POC exists." >&2
  exit 1
fi
echo "inter_cinter_census: OK — $tot_blocks coded inter blocks parsed across $tot_cells cells."
