# Zone-2 split-loop experiment

Preserved source for the September 6 i265 microbenchmark. The measured
baseline dependency was encoder tree `fda1ed60`; the candidate body is copied
in this standalone crate. Running against a later tree changes the baseline.
The archived hashes and ratios are in `benchmarks/still_i265_2026-09-06-dr-zone2-probe.*`.

Run tests with `cargo test --manifest-path tools/perf_profile/dr_zone2_probe/Cargo.toml`
from `rust/`, under the shared run-heavy wrapper. Build and run the release
binary with baseline target CPU and CPU affinity 2 to reproduce the experiment.
The paired loop uses 21 recorded rounds after warmup. This preserves an
exploratory harness; production frame comparisons use `still_pairs.py`.

The candidate receives a held AVX2 token while the baseline enters the public
dispatcher. These timings include different entry paths and are not a claim
about abstraction overhead or whole-frame performance. The test requires
AVX2 hardware and compares 2,394 padded-buffer cases to the real C oracle.
