# Held-token large Hadamard probe

Frozen baseline is the C-shaped Hadamard family from encoder source136e779c
(same DSP code at a2041a90). The candidate dispatches once and carries a V3
token through the8x8 sub-transforms and16/32 combine stages. All arithmetic
is copied unchanged, including full-range i16 wrapping. No new Archmage API.

`cargo test --lib` checks600 padded cases: exact positional agreement with
the frozen Rust baseline, full coefficient multiset parity against C AVX2,
and output sentinels. C AVX2 permutes coefficients, consumed only by SATD.

Run `cargo run --release -- --control --format=json`, then the same command
without `--control`, on a pinned AVX2 core under run-heavy. Dependencies and
compiler flags must match the recorded baseline-CPU run. The original run
patched zenbench to the preserved gate-tuned scratch checkout using Cargo
`--config`; its source/hash provenance is in the earlier Hadamard probe record.

The first experiment used the production DSP dependency for the baseline.
The frozen rerun removes that dependency so future production changes cannot
move the baseline. Both logs are preserved; frozen results are primary.
