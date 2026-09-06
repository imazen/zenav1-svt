# Coefficient reduction probe

The baseline uses ordinary scalar-target Rust loops. The candidate uses the
same loops under V3 target features, including runtime incant dispatch. Both
wide (i64) and narrow (i32) accumulators are measured at lengths16/32/64/256/1024.
Tests compare both to real C within its documented range and check the wide
sum against closed-form signed-extreme cases beyond i32 accumulation.

Use release, baseline CPU flags, core2, external TMPDIR and target paths, and
the shared run-heavy wrapper. The Zenbench0.1.9 own-thread workaround and Cargo
path override are documented in `../hadamard_probe/README.md`. Run the binary
with `--control --format=json`, then `--format=json`. Its embedded macro does
not implement `--help`.

The original microbench candidate dispatches narrow16 as well, and that case
regresses. Production keeps lengths below32 on the original narrow core;
disassembly shows the AVX2 main loop begins at32. The wide candidate improves
all measured lengths. Small16-entry controls have roughly2% biases; other
control difference intervals contain zero. Original source hashes, all rows,
and gate counts are recorded in
`benchmarks/still_i265_2026-09-06-satd-probe.*`. These are isolated timings;
production frame measurements decide whether to keep the change.
