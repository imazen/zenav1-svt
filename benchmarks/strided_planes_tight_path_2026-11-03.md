# Tight-path cost of the strided plane entry points (2026-11-03)

Snapshot, not a live claim. Host r7900x (Ryzen 9 7900X, Zen 4); **not measured on
Zen 5**. No `-C target-cpu=native`. Base `e10fafdf`, change `50f0881a` (stack
`e6d14804`, `eb98c3e0`, `50f0881a`). Both built with
`cargo build --release -p zenav1-svt --example perf_encode` (fat LTO).

zenbench in this repo covers dsp kernels only, and this change touches none of
them, so the entry point is measured with `perf_encode`, the repo's performance
gate harness: a fresh-pipeline `encode_frame_420` with tightly packed planes,
base and change interleaved, one warmup encode per sample.

`perf_encode gradient <w> <h> 32 <preset> <prefix> 1`, n = 31 per cell:

| cell | base median ms | change median ms | ratio | base min ms | change min ms | ratio | bytes equal |
|---|---|---|---|---|---|---|---|
| gradient-512x512-q32-p6 | 19.618 | 19.965 | 1.0176 | 19.345 | 19.515 | 1.0088 | True |
| gradient-512x512-q32-p10 | 5.443 | 5.438 | 0.9991 | 5.378 | 5.347 | 0.9941 | True |
| gradient-64x64-q32-p10 | 0.126 | 0.126 | 1.0037 | 0.124 | 0.124 | 0.9966 | True |
| gradient-72x40-q32-p10 | 0.130 | 0.129 | 0.9924 | 0.128 | 0.128 | 0.9978 | True |
| gradient-1024x1024-q32-p10 | 17.334 | 17.489 | 1.0089 | 16.952 | 17.060 | 1.0064 | True |

Repeat of the 512x512 p6 cell with the order reversed (change first), n = 101:
median 19.538 -> 19.682 ms (1.0073), min 19.177 -> 19.401 ms (1.0117).

`perf stat -e instructions:u,cycles:u`, one run each (whole process):

| cell | instructions base | instructions change | cycles base | cycles change |
|---|---|---|---|---|
| 512x512 q32 p6 | 754,088,712 | 754,053,492 | 227,463,879 | 221,971,886 |
| 512x512 q32 p10 | 182,644,078 | 182,629,534 | not recorded | not recorded |
| 1024x1024 q32 p10 | 615,124,948 | 615,108,919 | 191,541,274 | 192,236,215 |

Reading: instructions moved by under 0.01% in every cell and the OBU output is
byte-identical. Wall-clock ratios run from 0.99 to 1.02 with no consistent
direction across cells, and the one cycle count that moved most went down. Those
timing differences are not attributed to any cause; code layout under fat LTO was
not investigated. Gradient content only. The strided path itself and the 10-bit
entry points have no timing here: not measured.

## After removing the tight forms (one strided form per entry point)

The numbers above measure the first form of this branch, where the tight entry
points were wrappers. These measure the single strided form against the same
base, with the same harness and the planes still tightly packed, n = 31 per cell:

| cell | base median ms | change median ms | ratio | base min ms | change min ms | ratio | bytes equal |
|---|---|---|---|---|---|---|---|
| gradient-512x512-q32-p6 | 19.503 | 19.471 | 0.9983 | 19.288 | 19.289 | 1.0001 | True |
| gradient-512x512-q32-p10 | 5.445 | 5.420 | 0.9954 | 5.357 | 5.331 | 0.9951 | True |
| gradient-64x64-q32-p10 | 0.126 | 0.126 | 0.9946 | 0.124 | 0.123 | 0.9927 | True |
| gradient-72x40-q32-p10 | 0.130 | 0.129 | 0.9939 | 0.127 | 0.127 | 0.9985 | True |
| gradient-1024x1024-q32-p10 | 17.287 | 17.247 | 0.9977 | 16.990 | 16.991 | 1.0001 | True |

`perf stat -e instructions:u`, one run each: 512x512 p6 754,052,936 -> 754,026,794;
512x512 p10 182,626,644 -> 182,608,412; 1024x1024 p10 615,158,992 -> 615,101,991.
