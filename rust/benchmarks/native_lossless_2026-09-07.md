# Native 10-bit lossless, 2026-09-07

Local change `snlqkkpm`, parent `9a9c4d9a`. C reference
`3115c0c1b23e860dfd75c94f6740e0298182dd13`. No push or CI run.

The native transform pipeline now applies 4x4 WHT forward/inverse transforms
at coded-lossless, bypasses RDOQ, and carries per-transform prediction and
coefficient state through MDS1, MDS3, chroma and monochrome level production.
High-preset color uses native full RD when lossless; the old lossy post-pass
cannot represent its four-transform leaves.

Before that routing correction, a guard-bypassed native gradient64 p9 encode
produced 2699 B and wrong decoded samples; C produced 4527 B. The corrected
encode is 4527 B, byte-identical and source-exact.

Palette injection used the u8 lambda for native fast costs. At screen64 p4,
mi=(6,0), its four-color candidate cost 141397 versus C 131588. Correcting the
lambda makes the costs equal and restores C's candidate admission. The stream
changes from 1285 B to C's 1318 B. Nine screen cases in the native grid changed
from byte-different/source-exact to byte-identical/source-exact.

`tree_diff.py` also needed a correction: it assumed the port had no palette
support and failed to compare the `pal` field. Its new three-test regression
checks require palette-size and palette-presence mismatches to fail.

The repeated-screen positive control then exposed a decoder failure at native
512x128 p4: the 11355-byte stream was rejected with `Invalid intrabc dv`.
The first differing arithmetic operation was an extra transform-partition bit
at mi=(18,8), not an incorrect displacement. C's `av1_code_tx_size` applies the
lossless gate before its inter/IntraBC branch. The port now applies that gate
in the writer and all five matching native/shared MD cost sites. The corrected
stream is 11349 B, C-identical and decoded-source-exact, with 386 selected
IntraBC blocks (54 with residuals). The regression requires residual-bearing IntraBC on both sides;
skip-only copies would not detect this failure.

## Completed measurements

- 336 native source comparisons: color/mono × gradient/diag/uniform/screen ×
  64x64, 96x80, 128x128 four tiles × presets 0–13. All pass.
- 168 corresponding color C-byte comparisons. All pass, no pins.
- 252 source comparisons: color/mono × low-two-bits-only/extremes/textured ×
  16x16, 65x67, 129x131 four tiles × presets 0–13. Strided inputs; recon-output
  enabled/disabled produces identical streams. All pass.
- Final workspace nextest with AVIF-container enabled: 2599/2599, zero skips.
- AVIF animation selection: 8/8 pass (229 unrelated tests filtered), including
  native lossless color, mono and alpha source equality under avifdec.

Logs and before/after captures:
`~/tmp/animation-metadata/native-lossless/` (`matrix.log`, `matrix-fixed.log`,
`native-edge-test.log`, `nextest.log`, `animation-tests.log`, `drill-p4/`).
Heavy work is serialized through the shared run-heavy wrapper, 16G cap, 4 jobs.
The reusable grid is `tools/native_lossless_gate.py <artifact-directory>`.

Final broad gates after the IntraBC correction: 120/120 regression witnesses
and 1100/1100 full 8-bit C-byte comparisons, no pins or harness errors. Logs:
`spotcheck-final.log`, `full8-final.log`, `nextest-final.log`.
The final native gate passes all 336 source / 168 C-byte checks; the 8-bit
lossless gate passes 240/240. Workspace clippy completes with existing warnings
(the new test's modulo-style warning was corrected).
Native monochrome mode selection remains upper-eight-bit; this change provides
native levels and exact lossless pixels, not full native monochrome MD.


## Quantization-option follow-up

`with_qm(true)` at QP0 matched C bytes while both encoders decoded incorrectly:
3716/6144 samples differed on gradient64 p7 at 8-bit; 5866/6144 at native10.
The decoder uses identity matrices for lossless segments. Production now
selects identity matrix levels for all lossless stages; raw C helper behavior
is preserved. This intentionally avoids the C defect (§31 in
`docs/SUSPECTED-C-BUGS.md`).

Variance boost at QP0 was different: C decoded source-exactly, while Rust was
rejected (`Failed to decode tile data`) at both depths. The planner's mere
presence was being mistaken for signaled delta-q. The planner is retained;
its output reaches MD, quantization and packing only when delta-q can be
signaled. This follows C's use of the frame quantizer when delta-q is absent,
and corrects the earlier C-bug inference in `SUSPECTED-C-BUGS.md` §1.

The new `lossless_quantization_options_match_source` test first reproduced both
failures and now passes 144 cases: QM only / variance boost only / both ×
8/10-bit × color/mono × 16x16, 65x67, 128x128 × presets 0,4,9,13. Every decoded
sample equals source and every enabled-option stream equals the default
lossless stream. Logs: `qm-probe.log`, `variance-probe-full.log`,
`qm-test-{before,after}.log`, `quant-options-{before,after}.log`.

Final checks after the quantization-option corrections:

- Variance-boost native grid: 336/336 decoded-source and 168/168 color C-byte
  comparisons, no differences (`variance-gate.log`).
- Workspace nextest with AVIF-container: 2600/2600, zero skips
  (`nextest-quant-final.log`).
- Regression witnesses: 123/123 (`spotcheck-quant-final.log`).
- Full 8-bit C gate: 1100/1100, no pins or harness errors
  (`full8-quant-final.log`).
- Workspace clippy completes with existing warnings (`clippy-quant-final.log`).
- Scoped rustfmt, diff check and refusal inventory pass (46 entries).

No push or CI run was triggered. Remaining AVIF/video scope stays tracked in
`docs/ANIMATED-AVIF-PLAN.md`.
