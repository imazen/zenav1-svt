# Reproduce the still PGO experiment

Run heavy commands serially through `~/work/claudehints/scripts/run-heavy`
with `--mem 16G --jobs 4` on i265. Set `TMPDIR=$HOME/tmp`. The examples
below omit that prefix for readability. Each output directory must be new.

From `rust/`, prepare separate training and evaluation inputs:

```bash
python3 tools/perf_profile/prepare_still_inputs.py --dataset training \
  --corpus "$HOME/work/zen/codec-corpus" --out "$HOME/tmp/still-training"
python3 tools/perf_profile/prepare_still_inputs.py --dataset evaluation \
  --corpus "$HOME/work/zen/codec-corpus" --out "$HOME/tmp/still-evaluation"
```

The named source sets are disjoint. Metadata records the original PNG hashes,
crop coordinates, decoding command, and I420 hashes. The grids contain 108
training cells and 135 evaluation cells, all QP20/40/60 and presets2/6/8.
The corpus is an external prerequisite; the tools fail if sources are absent.

Install `llvm-tools-preview` through rustup for matching profile tools. Build
Rust's three binaries and train the instrumented one:

```bash
python3 tools/perf_profile/pgo_still.py \
  --cells "$HOME/tmp/still-training/cells.tsv" \
  --out "$HOME/tmp/still-rust-pgo" --cpu 2
```

Keep encoder sources frozen throughout. The script uses opt3, 16 codegen
units, no LTO, and baseline x86 target CPU. It checks every instrumented
training output against its ordinary binary, keeps the outputs and profiles,
and records every command. Profile-use warnings are retained in the build log.

Train C too before making a PGO Rust/C claim. Supply the corrected ordinary
C timing driver built from the pinned reference, with Release-O3, NATIVE=OFF,
AVX512 runtime dispatch, no LTO, and HDR off:

```bash
python3 tools/perf_profile/pgo_c_still.py \
  --cells "$HOME/tmp/still-training/cells.tsv" \
  --baseline tools/perf_c_encode/perf_c_encode \
  --out "$HOME/tmp/still-c-pgo" --cpu 2
```

This creates an isolated CMake build, verifies C instrumentation preserves
all training outputs, and keeps GCC's profile data. It does not rebuild the
ordinary oracle. Compare on the held-out grid:

```bash
python3 tools/perf_profile/still_pairs.py \
  --port "$HOME/tmp/still-rust-pgo/perf_encode.pgo" \
  --reference "$HOME/tmp/still-c-pgo/perf_c_encode.pgo" \
  --reference-kind c --cells "$HOME/tmp/still-evaluation/cells.tsv" \
  --out "$HOME/tmp/still-pgo-position.tsv" --rounds 9 --cpu 2
```

For Rust PGO/ordinary comparisons use `--reference-kind port`. For C
PGO/ordinary comparisons use both `--port-kind c` and `--reference-kind c`.
The historical `port`/`reference` column names identify slots; metadata records
the driver kind and exact binary hashes. Always include a same-binary control.

The September 6 results are in `docs/STILL-PERF-2026-09-06.md`. PGO does not
alter the repository's release profile, and these training profiles do not
cover every public encoder configuration or architecture.
