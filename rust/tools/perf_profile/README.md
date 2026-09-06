# perf_profile — per-class attribution of the port-vs-C encode gap

Answers "**which** work is the gap made of", where `tools/perf_gate.sh` answers
"how big is it" and `tools/perf_ab.sh` answers "did this change help". Output of
the first run: `benchmarks/perf_class_attrib_2026-08-13.{tsv,meta}` — read the
`.meta` before trusting any number here, it documents the traps.

## Inner loop

```bash
# 1. a profiling binary (separate target dir so a concurrent agent's target/ is untouched)
CARGO_TARGET_DIR=rust/target-perf RUSTFLAGS="-C debuginfo=1 -C llvm-args=-align-all-functions=4" \
  cargo build --release -p zenav1-svt --example perf_encode
# and the C harness: tools/perf_c_encode/build.sh

# 2. sample both sides on the SAME cell (iters large enough to outlast `secs`)
tools/perf_profile/prof.sh port 512 10 40 4000 20 ~/tmp/zsvtprof/port_512_p10.txt
tools/perf_profile/prof.sh c    512 10 40 4000 20 ~/tmp/zsvtprof/c_512_p10.txt

# 3. self time per symbol, demangled
python3 tools/perf_profile/selftime.py port_512_p10.txt | rustfilt > port_512_p10.self.tsv

# 4. tables. The two ms values are the PAIRED per-encode times for that cell
#    from benchmarks/perf_gap_*.tsv — the profile gives shares, not scale.
python3 tools/perf_profile/classify.py port_512_p10.self.tsv c_512_p10.self.tsv 7.1597 2.5560 [detail]
python3 tools/perf_profile/attrib.py   port_512_p10.self.tsv c_512_p10.self.tsv 7.1597 2.5560 [detail]
```

`classify.py` buckets by functional area (transforms, entropy, intra, alloc, …).
`attrib.py` buckets by *why*: SIMD_GAP (port scalar, C ships a SET_NEON kernel) /
SIMD_QUAL (both vectorised) / ALLOC / SCALAR_BOTH.

## Traps this tooling exists to avoid

1. **`<deduplicated_symbol>` is not noise.** `sample` prints it when an address
   maps to several linker-folded names; on the C side it was 4.4 % of samples and
   290 of its 310 samples sat under `inv_txfm_*_neon`. `selftime.py` renames it
   to `dedupof_<parent>`. Leaving it unattributed overstates the transform gap ~2x.
2. **Check the setup share before trusting the profile.** Both harnesses loop
   init→encode→teardown. It happens that >99 % of samples land inside the timed
   region on both sides at these cells — that was verified, not assumed. Re-verify
   if you change cell or preset: look at the top of the call graph.
3. **The profile supplies shares only.** Multiply by a paired measurement from
   `perf_gate.sh`, never by a stopwatch on the profiling run itself.
4. **Grepping the raw call graph for a symbol you expect is a real experiment.**
   `grep -ci 'hadamard|satd' c_512_p10.txt` returning 0 over 7,126 samples is how
   the MDS0 Hadamard finding in the `.meta` was made — a whole class of work the
   port does and C does not. Look for absences, not just for hot spots.

## Exact call counts (callgrind, Linux only)

`sample`-based profiles give shares; callgrind gives EXACT per-function call
counts, which is the only way to ask "does the port call this MORE OFTEN than
C for the same bytes". Runs on r7900x (valgrind does not run on the Mac).

```bash
# both sides, per cell: identity pre-pass, callgrind port + C, cmp the
# instrumented runs' own output, then callcount.py / callgrind_annotate
CE=~/work/zen/zenav1-svt/tools/perf_c_encode/perf_c_encode \
  tools/perf_profile/callcount_cells.sh ~/tmp/cg \
  gradient:gradient:512:512 photo:raw:/abs/photo.yuv:512:512
# the C-function -> port-function(s) ratio table (sums AND lists every
# symbol a regex matches, so duplicate transcriptions cannot hide)
python3 tools/perf_profile/callcount_join.py ~/tmp/cg --tsv join.tsv
# caller edges (which caller, how many times, at what inclusive Ir)
python3 tools/perf_profile/tree_callers.py ~/tmp/cg/tree_c_photo_p2.txt '^svt_aom_quantize_inv_quantize$'
```

### The INTER frame's own cost: N=2 minus N=1, per symbol

`perf_gate.sh` prices the inter frame by differencing wall-clock medians and
cannot resolve C's side (its p25/p75 spread is the size of the difference).
Callgrind is deterministic, so the same subtraction on Ir is exact per
function. `callcount_cells.sh` takes `FRAMES` / `VIDEO` / `SHIFT` (the same env
`perf_gate.sh` exports) and checks identity PER FRAME; run it once per
(FRAMES, VIDEO) with distinct cell names, then difference:

```bash
PRESETS="6 8" VIDEO=1 FRAMES=1 tools/perf_profile/callcount_cells.sh ~/tmp/cg gradient_vk:gradient:512:512
PRESETS="6 8"         FRAMES=2 tools/perf_profile/callcount_cells.sh ~/tmp/cg gradient_n2:gradient:512:512
python3 tools/perf_profile/inter_delta.py ~/tmp/cg --n1 gradient_vk --n2 gradient_n2 --preset 8 --side port --tsv delta_port_gradient_p8.tsv
python3 tools/perf_profile/inter_delta.py ~/tmp/cg --n1 gradient_vk --n2 gradient_n2 --preset 8 --side c    --tsv delta_c_gradient_p8.tsv
python3 tools/perf_profile/inter_join.py . --cells gradient_p8 --tsv join.tsv   # the C-edge -> port-edge table
```

`inter_delta.py` asserts that the per-function self deltas sum to the
process delta (a parser that drops rows is caught); `inter_join.py` folds
callgrind's recursion clones (`fn'2`) before matching, or a `$`-anchored
regex sees one level only. Record: `benchmarks/callcount_inter_2026-09-05.*`.
Two traps specific to the difference: `perf_encode::translate` is the PORT
harness synthesising the shifted frame (C reads it from the `.yuv`) — exclude
it; and a row present in both cells at the same count reads 0 however large it
is, so the table is what the inter frame ADDED, not what it costs.

Real-content `.yuv`s come from `identity_run crop:<png> W H qp preset prefix`
(the byte gates' BT.601 converter) and feed `perf_encode raw:<prefix>.yuv`.
Records: `benchmarks/callcount_2026-09-04.*` (gradient), `callcount_txtscreen_*`,
`callcount_mds1skip_*`, `callcount_realimg_2026-09-04.*` (six contents). Traps
the last one hit: `callgrind_annotate` drops functions below its 99 % threshold
(pass `--threshold=100` for small kernels); `md_stage_0` is per candidate class,
not per leaf; C's `perform_dct_dct_tx` has no symbol at -O3; the port's 32x32
hadamard calls its 16x16 four times.


## Linux paired real-image capture

`profile_still.py` profiles prebuilt Rust and C drivers against one row of the
`still_pairs.py` cell manifest. Run it under the shared run-heavy wrapper:

```sh
python3 rust/tools/perf_profile/profile_still.py \
  --port /absolute/perf_encode.pgo --reference /absolute/perf_c_encode.pgo \
  --cells /absolute/cells.tsv --cell screen_wiki-1024-q60-p8 \
  --cpu 2 --warmups 400 --event cpu_core/cycles/u --out /absolute/new-profile-dir
```

The output directory must be new. Add `--sudo-perf` if local perf permissions
require it and passwordless sudo is available; this elevates perf record and
its child driver, then returns ownership of the recording to the caller.
It does not change system security settings. Captures require nonzero samples,
zero reported lost samples, and byte-identical final outputs. Metadata records
commands, hashes, and sample counts; compact self-symbol TSVs accompany the raw
recordings. Whole-process samples include setup, teardown, and logging. Use
`still_pairs.py` for encode timings. Recordings can be gigabytes; keep them in
scratch storage and preserve small metadata and symbol tables in Git.

The profiler records the current checkout commit separately from binary hashes.
A prebuilt binary may come from another revision: retain its build metadata to
identify source and compiler settings. The checkout commit is not asserted to
be the binary's source revision.
