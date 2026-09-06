# DCT64 specialization probe

The frozen baseline copies the forward64 driver and butterfly from `e1a555f4`.
The candidate keeps runtime dispatch but specializes cosine precisions10/13,
with a runtime fallback. Both entries hold a V3 token. Real-C tests cover160
64x64 blocks, strides64/71, input offsets and output sentinels.

Run release builds with baseline CPU flags under run-heavy, using external
TMPDIR and target paths. Apply the local Zenbench own-thread workaround from
`../hadamard_probe/README.md` and pass its Cargo path override. Run pinned to
core2 with `--control --format=json`, then `--format=json`. The embedded macro
does not implement `--help`.

The default source is the final switch variant. Each patch applies separately
to that source: `const.patch` calls constant kernels directly, `fixedbuf.patch`
only gives the output a fixed array type, and `return.patch` returns the
coefficient array from the dynamic kernel through a thin wrapper. Restore the
default source before applying another patch. Original scratch-copy source
hashes and measurements are recorded in
`benchmarks/still_i265_2026-09-06-dct64-probe.*`.

Both constant variants improve isolated kernel time roughly6–8%; the fixed
buffer and returned-array variants do not improve it. Every run has zero gate
waits. Controls complete100 rounds per stride with intervals containing zero.
Production frame timing and code size are separate checks; do not translate
these isolated percentages into frame-speed claims.
