#!/usr/bin/env bash
# Two-frame differential against C on REAL public-domain video.
#
# WHY THIS GATE EXISTS. Every other inter cell in this repo encodes synthetic
# content: `inter_byte_gate.sh`'s 96 cells are {uniform,gradient,diag,screen},
# and `video_key_matrix.sh` defaults to `gradient diag screen screenrep
# uniform`. The multi-frame path builds later frames by translating frame 0 by
# a global integer offset, which open-loop ME finds exactly -- measured
# `avg_me_sad=0` and `is_gm_on=0` across {gradient,diag,screen} x
# {64,128,256,512} (INTER-ENCODE-PLAN.md). So the entire inter surface was
# being asserted against a motion field C's search never has to work for.
#
# MEASURED 2026-09-10, and this is the whole argument for the gate. At the
# identical cell shape (128x128 q40 p6 frames=2), synthetic `gradient` is
# byte-identical on BOTH frames, with frame 1 coding to 24 bytes -- a skip.
# Real video at that same shape diverges: `johnny` codes frame 1 as 33 bytes in
# C against the port's 36, `vidyo3` 113 against 101. The synthetic cell was
# green because it was asserting a skip frame.
#
# WHAT IT PINS. The exact measured (frame0, frame1) identity pair for each of
# 24 cells, so neither a regression nor a silent improvement can pass
# unnoticed. A cell that gets WORSE fails. A cell that gets BETTER is reported
# as PROMOTED and also fails, because the table below is a measurement and an
# out-of-date measurement is the thing this repo keeps getting burned by.
#
# THE ASSETS ARE NOT SYNTHETIC AND NOT IN GIT. Twelve I420 sequences cut from
# six clips that Xiph's Derf collection marks public domain, published at the
# R2 prefix `video/pd-derf-720p/` and fetched anonymously (no secrets). The
# still corpora this repo uses -- gb82-sc, CID22, clic2025 -- stay on their
# git sparse clone; see `fetch_r2_assets.sh` for why the two are kept apart.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"

# No silent skip: if the assets are absent, FETCH them. A gate that reports
# 0/0 and exits 0 reads as a pass, which is precisely the failure mode
# `lib_corpus.sh` documents three separate instances of.
if [ ! -f "$ASSETS/vidyo3_128x128_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "real video inter gate: could not obtain assets" >&2; exit 1; }
fi

# clip size preset  expected_f0/expected_f1
# MEASURED 2026-09-11 at cli_qp 40, frames=2, on x86-64; johnny p6 and
# vidyo4-128 p8 frame-1 promoted 2026-09-13 after the inter-MD fix set,
# and five more frame-1 cells promoted 2026-09-14 when the reference-
# picture border moved from the input-picture `scs->border` (68) to C's
# actual `super_block_size + 32` (enc_handle.c:1212-1217) with the
# contiguous [u][v] buffer_alloc layout: a maximally UMV-clamped chroma
# MV reads past the padded margin, and the 68-pixel border turned C's
# legal read into either a panic or wrong prediction pixels.
# `merge_inter_cands` class-2 merging (`md_me_dist`/`md_pme_dist`),
# `gm_ctrls.enabled` gating GLOBALMV injection, the video MDS0 variance
# arm (`mds0_use_hadamard_sb = 0` outside all-intra), and the level-2
# `dist_to_cost_th = 0` prune applied to the inter lane.
# 2026-09-19: three more frame-1s promoted (johnny 256x256 p8, vidyo3
# 128x128/256x256 p8), measured under the arm+bit-depth `bypass_encdec`
# stamp — though the promotions themselves predate it, since bd8 p6/p8
# derive bypass=1 under both ladders. vidyo1 256x256 p6 frame 1 promoted
# 2026-09-21 when `set_start_end_depth` gained C's per-SB
# `depth_removal_ctrls` e-depth clamp (enc_dec_process.c:1799-1813): the
# pipeline already computed `disallow_below_16x16` per superblock, but the
# value never reached `RefineEnv`, so a 16x16 leaf tested the 8x8 children
# C never evaluates. vidyo1 256x256 p8 frame 1 closed the same day when
# `updated_enable_pme` became per-BLOCK (product_coding_loop.c:9418-9422):
# C zeroes it when `is_intra_bordered &&
# use_neighbouring_mode_ctrls.enabled`, but the port stamped it from the
# frame-level `md_pme_ctrls.enabled`, so an intra-bordered block ran
# `pme_search` C skipped and `inject_pme_candidates` added the extra
# NEWMV/NEWNEWMV candidates that flipped the block to inter.
#
# The frame-0 column is the interesting one: it is a KEY frame. All SIX
# preset-6 key
# frames closed when the video arm's `skip_sub_depth_lvl` ladder was wired
# (`encdec_arm::apply` stamps `enc_mode <= M1 -> 1 else 2`; the still bake
# had pinned level 1's `coeff_perc` 15 where video M2+ derives 25), and three
# more frame-1 cells closed when loop restoration stopped being gated on
# `is_key` — `ppcs->enable_restoration` is picture-level in C, and a flat
# GOP is `is_not_last_layer` on every frame.
# That is not the same surface `real_image_matrix.sh` covers (180/180 on
# CID22-512 at presets {2,6,10}), which runs the all-intra path at 512x512.
CELLS=(
    "fourpeople     128x128 6 1/1"
    "fourpeople     128x128 8 1/1"
    "fourpeople     256x256 6 1/1"
    "fourpeople     256x256 8 1/1"
    "johnny         128x128 6 1/1"
    "johnny         128x128 8 1/1"
    "johnny         256x256 6 1/1"
    "johnny         256x256 8 1/1"
    "kristenandsara 128x128 6 1/1"
    "kristenandsara 128x128 8 1/1"
    "kristenandsara 256x256 6 1/1"
    "kristenandsara 256x256 8 1/1"
    "vidyo1         128x128 6 1/1"
    "vidyo1         128x128 8 1/1"
    "vidyo1         256x256 6 1/1"
    "vidyo1         256x256 8 1/1"
    "vidyo3         128x128 6 1/1"
    "vidyo3         128x128 8 1/1"
    "vidyo3         256x256 6 1/1"
    "vidyo3         256x256 8 1/1"
    "vidyo4         128x128 6 1/1"
    "vidyo4         128x128 8 1/1"
    "vidyo4         256x256 6 1/1"
    "vidyo4         256x256 8 1/1"
)

QP="${RVIG_QP:-40}"
work="${TMPDIR:-$HOME/tmp}/real-video-inter.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; promoted=0; err=0; ran=0
echo "== real video inter gate (public-domain derf clips, qp $QP) =="
printf '%-16s %-8s %-3s %-7s %-7s %s\n' clip size p expect got note
for spec in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $spec
    clip=$1; size=$2; preset=$3; expect=$4
    w=${size%x*}; h=${size#*x}
    asset="$ASSETS/${clip}_${size}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; err=$((err + 1)); continue
    fi
    out="$work/${clip}_${size}_p${preset}"
    mkdir -p "$out"
    # SVTAV1_FRAME_SHIFT/_ZOOM_* are the SYNTHETIC motion model; identity_run
    # refuses them alongside `rawseq:` rather than silently layering a warp on
    # top of real motion, so they are cleared here.
    env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        SVTAV1_INTER_EXPERIMENTAL=1 \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$QP" "$preset" 2 \
        "rawseq:$asset" "$out" >"$out/diff.txt" 2>&1
    st=$?
    if [ $st -ne 0 ] && [ $st -ne 1 ]; then
        printf '  %-16s %-8s %-3s %-7s %-7s %s\n' "$clip" "$size" "$preset" "$expect" ERR "exit $st"
        err=$((err + 1)); continue
    fi
    a=0; b=0
    cmp -s "$out/c.obu.pts0" "$out/rs.obu.f0" && a=1
    cmp -s "$out/c.obu.pts1" "$out/rs.obu.f1" && b=1
    got="$a/$b"
    ran=$((ran + 1))
    note=""
    if [ "$got" = "$expect" ]; then
        note="ok"
    elif [ $((a + b)) -gt $(( ${expect%/*} + ${expect#*/} )) ]; then
        note="PROMOTED — update the table"; promoted=$((promoted + 1))
    else
        note="REGRESSED"; fail=$((fail + 1))
    fi
    printf '  %-16s %-8s %-3s %-7s %-7s %s\n' "$clip" "$size" "$preset" "$expect" "$got" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   regressed: $fail   promoted: $promoted   errors: $err"
if [ "$ran" -ne "${#CELLS[@]}" ]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
[ $((fail + promoted + err)) -eq 0 ] || exit 1
echo "real video inter gate: OK"
