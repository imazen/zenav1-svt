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
