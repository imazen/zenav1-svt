# Hadamard 8x8 transpose probe

The frozen baseline uses the generic i16x8 butterflies and array transposes
from `e711ee08`. The candidate changes only the transpose to existing safe
unpack16/unpack32/unpack64 intrinsics, with generic raw/from_m128i conversions.
Both benchmark entries hold the same V3 token. The C test compares both paths
on1,200 padded/full-range cases.

Use the commands and Linux Zenbench own-thread workaround documented in
`../hadamard_probe/README.md`, substituting this manifest path and an external
target directory. Build release with baseline CPU flags, then run the binary
on core2 with `--control --format=json` and `--format=json`, under run-heavy.
Do not use the embedded binary's unsupported `--help` option.

Original scratch-copy results and source hashes are in
`benchmarks/still_i265_2026-09-06-hadamard-transpose-probe.*`.
All control groups completed90 rounds with difference intervals containing
zero, and A/B groups30 rounds, with zero gate waits. Baseline23.17–23.22ns
became6.41–6.49ns. The baseline was15–16ns in earlier differently laid-out
builds, so this isolated72% reduction is not a production speed claim.
Frame A/B against the saved production binary determines the gain.
