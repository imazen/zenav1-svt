# Lossless screen-content source contract — 2026-09-07

C reference: `3115c0c1b23e860dfd75c94f6740e0298182dd13`, local x86_64,
still I420 8-bit CQP. No CI or push.

With the former QP0 screen-content refusal bypassed, `identity_run screen
64 64 0 4` panicked at `intrabc_hash.rs:422`:
`index out of bounds: the len is 0 but the index is 0`.
The fixed partition walk passed a block-relative source slice to the funnel,
while IntraBC's hash query and motion search use full-frame coordinates.

Both pipeline callers and recursive fixed-tree children now retain the full
source plane. The funnel receives the block's absolute source offset; the
older non-funnel monochrome encoder still receives a local slice. Changing
only the hash query would leave the motion search reading the wrong source.

The reproducer now emits 151 bytes, identical to C. All 96 screen/screenrep
lossless cases (four frame geometries, twelve presets) match C and decode
exactly to source. This includes presets 0–5, whose refusal was removed only
after those checks passed. The previously divergent preset-5 cases also
match C with the source and lossless MD context corrections.

The default lossless gate now includes both screen patterns. The regression
suite replaces the p4/p5 rejection witnesses with byte comparisons and adds a
multi-superblock repeated-content case. Public animation tests also exercise
lower-preset screen content with alpha and odd dimensions.

Evidence: `~/tmp/animation-metadata/lossless-screen/` contains the before/after
logs, 96-cell matrix, expanded lossless gate, regression suite, full 8-bit
comparison, and workspace test/lint logs.

A premise check found that `screenrep` at 128x128/256x256 does not enable
IntraBC at QP0. It remains useful content coverage, but cannot prove block-copy
coding. The new `screencopy` pattern repeats the low-color panels at 256 pixels.
At 512x128 QP0 p4, C selects 320 IntraBC blocks and the port produces identical
bytes. The regression witness explicitly requires IntraBC selections in both
traces, so disabling the tool cannot make the test pass vacuously.

Final local verification: **2,598/2,598 workspace tests, zero skips;
116/116 regression cases; 240/240 lossless C-byte/source-pixel cases;
1,100/1,100 full 8-bit C comparisons, no pins or harness errors**. The
animated screen-content cases decode with exact color and alpha source pixels.
The 512x128 `screencopy` regression requires IntraBC selections in both C and
the port and exact source pixels under aomdec, in addition to byte equality.
Workspace clippy completes with existing warnings; scoped formatting and diff
checks pass. The refusal inventory has 47 entries, including 13 capability
refusals. No push or CI run was triggered.
