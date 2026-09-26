#!/usr/bin/env bash
# 10-BIT VIDEO: two-frame differential against C on real public-domain clips.
#
# WHY THIS GATE EXISTS. Until 2026-09-10 a 10-bit INTER frame was not merely
# non-identical, it was UNREACHABLE, in three stacked ways:
#
#   1. `try_encode_frame_420_hbd` refused every non-key frame outright, and the
#      refusal had no 10-bit counterpart in the 8-bit differential harness, so
#      the surface could not be measured at all.
#   2. Past that, `tx_unit_hbd` indexed a zero-length slice: an inter
#      candidate's `Cand::pred10` had NO producer. The bd10 full-RD funnel
#      residuals every candidate against a 10-bit prediction, an intra
#      candidate got one from `predict_unit_hbd`, and an inter candidate got
#      nothing, because the DPB stored only 8-bit reference planes.
#   3. Past THAT, the bd10 chroma full loop was a literal `panic!` saying it
#      "has no INTER arm".
#
# All three are closed: the DPB carries a 10-bit twin of each reference
# (`PaddedRef::hbd`), `av1_inter_prediction_light_pd1_hbd` produces the 10-bit
# luma AND chroma prediction in one call, and `chroma::eval_uv_inter_hbd`
# scores it. Every cell below now ENCODES and DECODES.
#
# WHAT IT ASSERTS. The table pins the per-cell measurement exactly as
# `real_video_inter_gate.sh` does, so neither a regression nor a silent
# improvement passes unnoticed:
#
#     48 of 48 frames byte-identical, 24 of 24 streams decodable (2026-09-18)
#
# Byte-identity came from modelling C's `hbd_md = 2` bump
# (product_coding_loop.c — at `bd10 && bypass_encdec && perform_md_recon`,
# C re-runs all of MDS3 in the 10-bit domain: prediction, quantization,
# `full_lambda_md[EB_10_BIT_MD]`, `blk_skip_decision` arbitration and the
# interpolation-filter search, then converts the winner's recon back to
# 8-bit for MD). The port mirrors that as `mds3_hbd`: the funnel keeps
# MDS0/MDS1 in the 8-bit domain while `run_mds3`/`ifs_at_mds3` evaluate and
# arbitrate on the 10-bit sources, and commit converts the winner recon.
#
# WHAT "DECODES" MEANS HERE, AND WHAT IT DOES NOT. The decode leg runs
# `dav1d -o /dev/null` and asserts only that the stream PARSES. It does NOT
# compare the port's reconstruction with the decoder's, and the difference is
# not academic: an inter frame predicts from the previous frame's recon, so an
# encoder can emit a parseable stream while holding a canvas the decoder never
# rebuilds, and the error then compounds down the GOP invisibly to this gate.
# Read the pass line as "24 streams parsed", never as "24 streams reconstruct".
#
# THE GAP THAT STOOD HERE IS CLOSED. 2026-09-11, `SVTAV1_FINAL_RECON` against
# `aomdec` over three of these clips x qp {20,40} x presets {6,8,10} x 4
# frames measured only 8 of 18 cells reconstructing identically — the reason
# `encode_frame_impl` used to REFUSE a 10-bit inter frame. The `hbd_md = 2`
# mirror (above) closed it: re-measured 2026-09-18 at 396/396 cells x 8
# frames across the whole preset ladder, and `bd10_video_selfcheck_gate.sh`
# now pins the recon leg exactly as `video_selfcheck_gate.sh` does for
# 8 bits. `AvifEncoder` still codes 10-bit animation all-intra — a deliberate
# policy in `animation_keyframes`, not a correctness fallback.
#
# (The first attempt at that measurement read 0 of 18, from frame 0 onward,
# because `identity_run`'s MULTI-frame `SVTAV1_FINAL_RECON` wrote the 8-BIT
# canvas whatever the depth. It now refuses that substitution. A tool that can
# report a confidently wrong number is a defect like any other.)
#
# WHERE THE DIVERGENCE WAS NOT (historical, kept because the reasoning still
# holds). While frame 0 still diverged, bd10 STILLS on this same content and
# geometry were 16/16 byte-identical to C (johnny + vidyo3 x {128,256} x
# presets {6,8,9,10}), and the 8-bit VIDEO key frame of this very cell was
# identical (1176 B on both sides at johnny 128x128 q40 p6) — which is what
# pinned the residual on bd10 mode decision under the VIDEO configuration
# rather than on stills plumbing or video plumbing.
set -uo pipefail
# The cell list and checks live in real_video_inter_gate.sh (tools/cellrun.py,
# plan T3); this gate is its 10-bit run. The decode leg is recon == aomdec and
# dav1d == aomdec on every frame (until 2026-09-26: "dav1d accepts it").
HERE=$(cd "$(dirname "$0")" && pwd)
RVIG_BD=10 RVIG_CHECK="${BVG_CHECK:-c,recon,dav1d}" RVIG_LABEL="bd10 video gate" \
    RVIG_QP="${BVG_QP:-40}" exec "$HERE/real_video_inter_gate.sh" "$@"
