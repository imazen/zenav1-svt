# Forced screen-content controls — issue #17

The existing public `HdrForkConfig::screen_content_mode` stored `Some(0)`
and `Some(1)` but ignored them at the pipeline consumer. C preserves these
values in all-intra configuration and `perform_sc_detection` explicitly
sets all six classes off/on, respectively (`pd_process.c:4773`).

The pipeline now performs that assignment before deriving palette/IntraBC
levels from the actual preset. Frame syntax, tile coding, mode-decision
rates, CDEF and restoration receive the same resolved derivation. Existing
auto detection and its IQ override are unchanged by this patch.

Enabled C witnesses (72x88, QP40, 8-bit 4:2:0):

| Mode / input / preset | Before Rust / C | After |
|---|---|---|
| Off / screen / 6 | 204 / 482 bytes, different | 482 bytes, C-identical |
| On / gradient / 6 | 279 / 279 bytes, different | 279 bytes, C-identical |
| On / gradient / 8 | 289 / 289 bytes, different | 289 bytes, C-identical |

The same three native 10-bit inputs are C-identical at 469, 277 and 285
bytes. Hashes and both sides of the regression witnesses are in the adjacent
TSV; source/encoded artifacts are under `~/tmp/svt-tracking/scm-{before,after}/`
and `scm-native10/`. The 8-bit cells are permanent enabled regressions in
`tools/regression_spotcheck.sh`. Tests do not infer correctness from two Rust
configurations differing or from header flags alone.

This fixes the pipeline controls; it does not add AVIF wrapper settings or
establish all video/GOP combinations. Further auto-detection/preset audit and
public wrapper exposure remain tracked in #21. The original #17 SCM3
measurement can legitimately match the default and is not itself a bug.

Combined validation with the tune correction: workspace nextest 2616/2616,
zero skipped; two doctests pass; C regression spotcheck 127/127. Libaom
accepts all 16 C/Rust streams from the two tune and six SCM witnesses at
the expected decoded plane lengths. Full all-target clippy remains blocked
by pre-existing repository-wide diagnostics, tracked separately in #21;
no diagnostic suppression or C literal edits are part of these fixes.
