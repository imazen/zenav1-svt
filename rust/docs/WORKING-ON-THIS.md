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
- `tools/bd10_video_selfcheck_gate.sh` — 10-bit multi-frame reconstruction
  parity: the port's final recon must equal `aomdec`'s decode of its own
  stream, per frame, on a pinned cell grid. The 10-bit twin of
  `video_selfcheck_gate.sh`, on the shipped path (no env flags).
- `tools/ra_selfcheck_gate.sh` — the random-access twin: `SVT_PRED_STRUCT=2`
  across hierarchical_levels 1..5 on complete mini-GOPs plus trailing partial
  windows, every display frame's recon byte-identical to `aomdec`. Hidden
  frames (coded but shown via show_existing) are counted by the gate's
  anti-vacuity pin.
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

## Code-review tooling

`tools/review/run.sh [OUT_DIR]` (default `~/tmp/zenav1-review`) builds the
inputs for a whole-codebase review: test- and comment-stripped reading bundles
(`prep.py`), Rust-vs-C size and complexity per C file (`c_compare.py`,
`cmp/c_modules.tsv`), near-duplicate detection on both sides (`clones.py`,
`jscpd`), transitively unreachable functions (`deadfns.py`, which still reports
`Deref`/`Index` impls as dead), and `incant!` sites missing an x86 or neon tier
(`incant_tiers.py --missing`). It refuses to run with a tool missing; each
script's header says how to install it. The last review built from these is
[CODE-REVIEW-2026-09-25.md](CODE-REVIEW-2026-09-25.md).

## Keeping files small

Every `.rs` file stays at most 3,000 lines, and 2,000 is the aim. The
`file_size_check.py` gate enforces the limit (`just file-size`, CI shard 3).
Split along real seams, as pure moves whose output pins stay unchanged:

- `tools/split_inline_mod.py FILE MOD`: moves an inline `mod x { .. }` block
  (usually tests) into `FILE-stem/x.rs`.
- `tools/move_items.py FILE CHILD FIRST END`: moves a run of top-level items
  into a child module.
  - `FIRST` and `END` are signature regexes; `@N` gives a line number,
    which is safe when applied bottom-up.
  - It raises private items to `pub(super)` and leaves `use` and `mod`
    lines in the parent.
  - It formats the child before editing the parent, so a failed move
    changes nothing.
- `tools/move_methods.py FILE IMPL-REGEX CHILD NAMES...`: moves methods of a
  big `impl` into a child's own `impl` block, appending if the child exists.
- `tools/ra_extract.py FILE FIRST LAST NAME ...`: drives rust-analyzer's
  "Extract into function" to cut a long function into stages. It works out
  every stage's inputs and outputs. Fix-ups the S2 series needed:
  - rust-analyzer gives every stage `&mut self`; a stage that only reads
    should take `&self`.
  - A stage that holds a borrow across other `self` writes should become an
    associated fn over just the fields it touches.
  - Non-`Copy` captures passed by value should be borrowed, or a closure
    turns `FnOnce`.
  - `std::` paths and bare `Box` must become `alloc`/`core` for no_std.
  - Mark single-call-site stages `#[inline(always)]` so the codegen stays
    as it was. The measured residue is in
    `benchmarks/perf_s2_split_2026-09-25.meta`.
- `tools/drop_unused_super_glob.py < check.log`: removes the `use super::*;`
  lines that rustc reports unused.

Moving code between files changes the `PORT-NOTE` index and the refusal
ledger, which are keyed by file. Regenerate both
(`tools/portnote_index.sh`, `tools/refusal_inventory.sh`) in the same
commit. Generated tables are split by their generators, never by hand:
`xtask/transcribe_qm.py` and cref's `gen_default_cdfs`.

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
