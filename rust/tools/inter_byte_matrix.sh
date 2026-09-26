#!/usr/bin/env bash
# The inter campaign's byte-identity FRONTIER sweep against C.
#
# `inter_byte_gate.sh` asserts the cells that are byte-identical. It does not
# sweep, because a gate that walks its own frontier takes minutes and gets
# skipped. This is the sweep behind it. It prints ONE LINE PER CELL, always,
# so a cell that never ran is visible, and a TSV that can be diffed against
# the previous run's.
#
# The run is tools/inter_byte_matrix.py (cellrun, in parallel); its header
# has the knobs (IBM_CONTENT takes the synthetic classes AND the derf clips,
# IBM_FRAMES any count) and the verdicts: IDENTICAL, TU<k> for the first
# temporal unit that differs (TU0 = a key-frame defect, which makes every
# later unit meaningless), ERROR for a cell that did not encode.
#
# A PORT PANIC IS AN ERROR, NOT A DIVERGENCE, and the sweep exits 1 on one.
# MEASURED 2026-09-02: the old loop scored eighteen 72x72 frame-1 panics as
# frame-1 divergences, because frame 0 had already been written when the
# panic hit ("55 F1DIFF cells" were 37 divergences and 18 crashes).
exec python3 "$(dirname "$0")/inter_byte_matrix.py" "$@"
