#!/usr/bin/env bash
# bd10 VIDEO SELFCHECK — the 10-bit twin of video_selfcheck_gate.sh.
#
# WHAT IT ASSERTS. For every cell below, the port encodes an 8-frame
# low-delay-P stream at bit depth 10, `aomdec` decodes it, and the ENCODER's
# final reconstruction (SVTAV1_FINAL_RECON, u16 LE) must equal the decoder's
# output frame for frame — the property a wrong inter stream actually
# violates. This is the leg `bd10_video_gate.sh` documents as its gap:
# byte-identity to C cannot see an encoder/decoder recon mismatch, because
# an inter frame predicts from a canvas the bitstream never describes.
#
# WHY IT EXISTS. The `encode_frame_impl` refusal of a 10-bit inter frame
# stood on a 2026-09-11 measurement — 8 of 18 cells reconstructing as
# aomdec does. The `hbd_md = 2` MDS3 bump mirror (product_coding_loop.c:9649;
# see bd10_video_gate.sh's header) closed the residual: re-measured
# 2026-09-18 on this exact grid — 270 cells at 256x256 (all presets -1..13)
# plus 126 at 128x128 (presets -1..5, the band the 8-bit gate also covers
# there) — 396/396 cells, all 8 frames byte-identical to aomdec. This gate
# pins that so the refusal cannot silently need reinstating.
#
# KNOWN OUT-OF-ENVELOPE DEFECT (shared with bd8, not gated here): at
# 200x120 / 232x200 preset 13 the encoder panics in `eob_cost` on an eob-0
# call (quant.rs `eob_pt - 1` underflow) — IDENTICALLY at bd8, so it is a
# pre-existing u8-path bug, not a bd10 regression. Tracked separately.
#
# ASSETS: the same public-domain Derf clips `video_selfcheck_gate.sh` uses,
# fetched the same way; there is no silent skip.
#
# Usage: tools/bd10_video_selfcheck_gate.sh
# Env:   VSG_SIZE / VSG_FRAMES / VSG_PRESETS override the grid (same names as
#        the 8-bit gate); AOMDEC points at the decoder.
set -uo pipefail
# The cell list, checks and anti-vacuity live in video_selfcheck_gate.sh
# (tools/cellrun.py, plan T3); this gate is its 10-bit run. VSG_SIZE,
# VSG_PRESETS, VSG_FRAMES, VSG_JOBS and AOMDEC pass through.
HERE=$(cd "$(dirname "$0")" && pwd)
VSG_BD=10 exec "$HERE/video_selfcheck_gate.sh" "$@"
