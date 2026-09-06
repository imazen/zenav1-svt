# Reverse EOB scan probe

Frozen scalar baseline from `0d9b1a7f`, eight-load OR candidate in `src/lib.rs`,
and the earlier per-lane bitmask candidate in `mask-lib.rs`. The benchmark
uses fixed shuffled scan permutations, lengths64/256/1024, and zero tails of
0,8,half,or all coefficients. The test witnesses every last-nonzero position
for eleven lengths and eight permutations, including signed extremes.

Run release builds with baseline CPU targeting under the shared run-heavy
wrapper, using external target and TMPDIR locations. Zenbench0.1.9 needs the
Linux own-thread workaround documented in `../hadamard_probe/README.md` on this
host. Pass the same Cargo path override to the patched local Zenbench copy.
Run the binary pinned to core2 with `--control --format=json`, then
`--format=json`. Do not use `--help`: the embedded macro starts measurements.

The September6 scratch-copy measurements are preserved in
`benchmarks/still_i265_2026-09-06-eob-probe.*`, with original source and log
hashes. Several same-function controls have nonzero difference intervals;
small effects are not clean evidence. Both candidates improve long zero tails
but regress short tails. The production candidate inlines its scalar fallback;
this probe calls the non-inlined frozen baseline. Frame timing is necessary.
