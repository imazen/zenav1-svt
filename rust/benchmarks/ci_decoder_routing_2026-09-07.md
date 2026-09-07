# Configured decoder routing in the IntraBC regression gate

CI run [34138303485](https://github.com/imazen/zenav1-svt/actions/runs/34138303485)
on main `286dcb03` failed its regression spotcheck: 121/123 passed, with
`qp0-screen-copy-p4` and `native-qp0-ibc-residual-p4` reporting source mismatch.
Windows ARM, macOS Intel and i686 jobs passed. Subsequent differential steps
were skipped after the failed spotcheck; this is not a complete CI pass.

`losslessIbc` called literal `aomdec`, while the workflow supplies
`RS_AOMDEC=$GITHUB_WORKSPACE/aom-build/aomdec`. The helper ignored the selected
decoder. Its combined failure condition also labeled a decoder invocation error
as a source-pixel mismatch.

Reproduction on the unchanged main source: put an executable named `aomdec`
that exits 127 first on PATH, and set RS_AOMDEC to the real `/usr/bin/aomdec`.
Run the entire regression spotcheck under the shared run-heavy wrapper (16G,
four jobs). Result: exactly the same 121/123 and the same two named failures.
This reproduces the configured-path defect without changing either encoder.
The local decoder is libaom 3.13.1; CI's configured build is 3.12.1.

Correction: use the resolved AOMDEC executable, request output-bit-depth equal
to the cell's coded depth, and report decoder errors with their log separately
from exact source comparisons. IntraBC selection, residual-selection, C-byte
identity and decoded-source equality assertions remain in place.

Raw before/after logs and the deliberately failing PATH executable are retained
under `~/tmp/ci-decoder-routing/`. The failed first after-run invoked a path
relative to the wrong working directory and exited before testing; it is not
counted as validation. `after-verified.log` is the correctly invoked full gate.

After correction, the same deliberately failing PATH decoder plus explicit
RS_AOMDEC passes **123/123**, exit 0. Shell syntax and whitespace checks pass.
No encoder source, golden bytes, tolerances or test skips changed.
