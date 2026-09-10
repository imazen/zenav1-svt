# Working on the port

Read [the handoff](../../CONTEXT-HANDOFF.md), then the relevant source and
[identity/support records](IDENTITY-STATUS.md). The previous performance and
campaign chronology is [archived](history/2026-09-08/rust/docs/WORKING-ON-THIS.md).
Old priorities and timing tables there are historical, not fresh measurements.

## Local workflow

Use `jj`, fetch main, inspect `.workongoing` and preserve concurrent work. Run
heavy jobs one at a time through `~/work/claudehints/scripts/run-heavy` (the
`~/work/zen/scripts/run-heavy` path is a compatibility location). Example from
`rust/`, with an existing writable task-specific TMPDIR:

```sh
TMPDIR="$HOME/tmp" ~/work/claudehints/scripts/run-heavy --mem 16G --jobs 4 -- \
  cargo nextest run --workspace --locked --test-threads 4
```

For encoding changes, run the relevant regressions and
`bash tools/regression_spotcheck.sh`. For mode-decision, partition, quantizer or
entropy changes, use the full named-reference matrix before a parity landing.
Do not rerun expensive codec gates for prose-only edits: check links and
source-generated inventories instead. Nextest isolates process-global SIMD
token permutations; doctests run separately. Never relax expectations or
turn missing fixtures into passing cells. Push each locally verified coherent
change to main, and verify remote ancestry. Do not wait on CI unless requested.

## The three measurement harnesses added 2026-09-10

Each is a committed script, not a scratch one-liner, because the numbers they
produce end up in commit messages and `benchmarks/*.meta`.

- `tools/heaptrack_alloc_cell.sh <arm>` — allocator CALL COUNT and peak heap on
  the canonical alloc cell, with `--c` for the C reference. It prints the sha256
  of the emitted bitstream on every run, so an arm whose allocations moved
  because its OUTPUT moved can never be reported as an allocation win. For
  file:line inside the inlining, rebuild with
  `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_TARGET_DIR=target-heapdbg`
  — the stripped release binary gives mangled names and nothing else.
- `tools/bd10_video_gate.sh` — 10-bit two-frame differential on real
  public-domain clips. Asserts ENCODES and DECODES hard, and pins the
  byte-identity table exactly as `real_video_inter_gate.sh` does.
- `tools/identity_diff_inter.sh` grew an `IDI_BD` axis (8 or 10) that drives
  BOTH sides: the port writes the widened 16-bit samples it actually encoded to
  `rs.yuv` and the C driver reads that same file at the matching depth, so a
  divergence cannot be the source.

**Measure runtime with `perf stat -e instructions` PINNED to one P-core**
(`taskset -c 2`). This fleet's `i265` is a hybrid Core Ultra 7 265K; unpinned
wall clock moves several percent on core placement alone, and a same-binary
null arm through `tools/perf_ab.sh` is what tells you whether a wall-clock
delta is real (measured null spread on that host: ratio 0.9996, p25/p75
0.9970/1.0017).

## Build and corpus prerequisites

Product dependencies need Rust, not C. Test oracles require the
`reference/svt-av1` submodule, a C/C++ compiler, CMake and NASM where the C build
uses x86 assembly. `zenav1-svt-cref/build.rs` owns mainline/HDR builds and cache
stamps; do not replace a source-matched oracle with an arbitrary local library.
See its documented `SVT_CREF_LIB_DIR`, `SVT_CREF_SKIP_HDR`, `SVT_CREF_JOBS` and
strict object-capture options. An explicitly skipped HDR oracle is not HDR
coverage. The manifest floor is 1.98, which is what the ARM dotprod intrinsics
require; build with 1.98 or newer.

`just` is an optional recipe runner; underlying scripts/commands are visible in
`justfile`. Some gates need `tools/decode_diff` and independently installed
`aomdec`/`dav1d`; read each script's prerequisites and configured decoder path.
Do not assume a historical binary path exists on another machine.

Corpus names used by historical gates: CID22 photos, CLIC photographs and
gb82-sc screenshots. Resolution is centralized in `tools/lib_corpus.sh`;
use the shared codec-corpus registry/docs for acquisition and actual layouts.
Common overrides include `PHOTO_P0_CORPUS`, `PHOTO_P0_CLIC`, `BD10_PHOTO_CORPUS`,
`SC_CORPUS`, `SCREEN_DIR` and `RIM_CORPUS` (check the chosen script for its exact
variable). No current full-corpus availability was verified in this handoff.
`photo_p0_gate.sh` still drops two CLIC cells when that secondary corpus is
absent; its warning is not a full eight-cell pass. Removing caller-invisible
fallbacks and recording gate runtime/disk budgets remains issue #8.

## Evidence rules

Use C byte differentials inside the named C envelope, independent decoding and
encoder-reconstruction checks throughout, and source-exact checks for lossless.
Prove enabled-tool reachability with positive controls; a tool-off pass is not
coverage. Keep native-u16 input distinct from widened u8 and native HDR distinct
from synthetic PQ values. Never pool timings from different CPU/build cohorts.

`tools/coverage_matrix.py` inventories the historical files it understands;
it does not automatically include later JSONL research/reference sweeps.
`tools/refusal_inventory.sh --check` and `tools/portnote_index.sh --check` check
source-derived ledgers. Counts in dated artifacts stay attached to their
original build. The full imazen-26 scout and held-out RD/time validation remain
required before selecting a representative subset or enabling adaptive defaults.
