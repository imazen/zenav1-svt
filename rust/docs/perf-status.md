# Performance status — G4 baseline (port vs C wall clock)

Measured baseline for **G4** (docs/ACCEPTANCE-CRITERIA.md → "Performance"): the
port's per-frame still-image encode wall time against the real C reference,
on the byte-identical envelope. This is the honest starting point of the
ratchet — the port has **not** been performance-tuned yet (G4 is deliberately
the last gate: "a fast encoder that emits different bytes is worthless").

**Verdict: the port is currently 1.5×–11× C on the tested cells; nothing is at
≤1.2× yet.** The gap is almost entirely **per-pixel compute** (slope), not fixed
overhead — at the fast presets the port's fixed per-frame cost is already *below*
C's. See the numbers below.

## The honest caveat

G4 per the criteria is measured *"once parity holds."* Parity holds on the
tested envelope — bd8 4:2:0, still-picture CQP, byte-exact presets — but not yet
across the whole matrix (10-bit, all presets, real content at speed ≥ 1, …). So
this is a baseline on the **byte-identical subset**, not the final gate. Every
cell here is verified byte-identical (port `.obu` == C `.obu`) before its ratio
is trusted; a comparison of two encoders doing *different* work would be
meaningless. All 15 cells below are byte-identical.

## How to run

```
tools/perf_gate.sh [date-suffix]        # default suffix: today's date
```

Env-overridable grid: `PERF_SIZES PERF_PRESETS PERF_CONTENT PERF_QP PERF_ROUNDS
PERF_WARMUP`. It builds the port release (no `target-cpu=native`), builds/links
the C reference harness, runs the interleaved paired sweep, verifies byte
identity per cell, and writes `benchmarks/perf_<suffix>.{tsv,raw.tsv,meta}`.
Intentionally **not** in CI — shared runners are too noisy for a wall-time gate
(rust-gates.yml says so); it runs on fixed hardware and the result is committed.

## Method (the binding rules, and how they're met)

- **Interleaved paired statistics.** Each round runs port and C back-to-back in
  *randomized* order (coin flip per round), so thermal/turbo drift cancels
  within the pair. The headline ratio per cell is the median of the per-round
  paired ratios; the spread is its [p25, p75]. Not back-to-back isolated blocks.
- **No `-C target-cpu=native`.** The port release is built with runtime SIMD
  dispatch (what ships). The C lib is the same Release build; it selects up to
  `avx512icl` at runtime on this host.
- **`total = intercept + slope · pixels`, fit across tiny → large.** So fixed
  per-call cost never hides inside one "ms/MP" number. Both coefficients are
  reported, per preset, for port and C. Nothing is extrapolated — every size is
  measured directly.
- **Setup excluded on both sides, symmetrically.** Only the per-frame encode is
  timed: the port times `encode_frame_420` on a fresh pipeline (`EncodePipeline::
  new` excluded); the C harness times `send_picture` + drain (`svt_av1_enc_init`
  excluded). The two harnesses are `svtav1/examples/perf_encode.rs` and
  `tools/perf_c_encode/perf_c_encode.c`; they consume the identical `.yuv`.

## Results — 2026-07-20, commit `d4c75a762`, host `dev-32gb` (16 cores)

Content `gradient`, qp 40, 20 interleaved paired rounds/cell, warmup 1. All
cells byte-identical.

### Per-cell ratio (port / C, median of paired rounds)

| size | preset | port ms | C ms | ratio | [p25, p75] |
|-----:|:------:|--------:|-----:|------:|:-----------|
| 64   | 6  |   4.635 |  1.177 |  3.95 | [3.86, 4.07] |
| 64   | 10 |   0.963 |  0.616 |  1.58 | [1.51, 1.61] |
| 64   | 13 |   0.969 |  0.636 |  1.55 | [1.33, 1.65] |
| 128  | 6  |  16.956 |  2.264 |  7.46 | [6.87, 8.12] |
| 128  | 10 |   3.081 |  0.873 |  3.55 | [3.45, 3.62] |
| 128  | 13 |   3.087 |  0.872 |  3.55 | [3.49, 3.63] |
| 256  | 6  |  79.400 |  9.026 |  8.76 | [8.61, 8.89] |
| 256  | 10 |  11.325 |  1.769 |  6.40 | [6.11, 6.77] |
| 256  | 13 |  11.421 |  1.768 |  6.38 | [6.16, 6.79] |
| 512  | 6  | 266.275 | 26.859 |  9.92 | [9.51, 10.17] |
| 512  | 10 |  47.408 | 13.814 |  3.52 | [3.27, 3.89] |
| 512  | 13 |  47.051 | 14.145 |  3.35 | [3.06, 3.51] |
| 1024 | 6  | 917.730 | 82.326 | 11.25 | [10.88, 11.59] |
| 1024 | 10 | 177.751 | 19.563 |  9.22 | [8.86, 9.26] |
| 1024 | 13 | 177.261 | 19.531 |  9.05 | [8.66, 9.18] |

Best case ~1.55× (tiny + fast preset, where fixed cost dominates and the port's
is small); worst ~11.25× (1024², preset 6, where per-pixel work dominates).

### Intercept + slope fit (`ms = intercept + slope · pixels`; slope as ms/megapixel)

| preset | port intercept | port slope | C intercept | C slope | slope ratio | intercept ratio | port R² | C R² |
|:------:|---------------:|-----------:|------------:|--------:|:-----------:|:---------------:|:-------:|:----:|
| 6  | 14.767 ms | 867.14 ms/MP | 2.909 ms | 76.68 ms/MP | **11.31×** | 5.08× | 0.998 | 0.995 |
| 10 |  0.841 ms | 169.20 ms/MP | 2.318 ms | 17.93 ms/MP | **9.44×**  | 0.36× | 1.000 | 0.813 |
| 13 |  0.834 ms | 168.69 ms/MP | 2.394 ms | 17.89 ms/MP | **9.43×**  | 0.35× | 1.000 | 0.801 |

Reading the fit:

- **The gap is the slope, not the intercept.** The port does ~9.4× (fast
  presets) to ~11.3× (preset 6) the per-pixel work of C. At presets 10/13 the
  port's *intercept* — fixed per-frame cost — is actually **below** C's (0.36×);
  the port is not losing on startup/dispatch, it is losing on the hot loops.
- **The port scales cleanly with pixels** (R² 0.998–1.000): its cost is a clean
  `a + b·pixels`, which makes the slope a trustworthy per-pixel figure and means
  a per-pixel win propagates to every size. The C reference at presets 10/13 is
  less pixel-linear (R² ≈ 0.80 — the 512² point is high; its encode time is small
  enough, 1–20 ms, that content statistics and the threaded pipeline shape it as
  much as pixel count does), so C's fitted fast-preset slope/intercept carry more
  uncertainty than the port's. Preset 6 fits both sides well (R² > 0.99); the
  per-cell ratios are the firmer view at the fast presets.
- **Preset 6's port intercept (14.8 ms) is a fit artifact of mild
  super-linearity**, not a real 15 ms floor (64² p6 is only 4.6 ms). Read p6 as
  slope-dominated with a large per-pixel constant.

## Top hotspots (where the future work is)

1. **SIMD on the hot per-pixel kernels — the dominant lever.** The ~9–11× slope
   gap is the port's mostly-scalar mode-decision / transform / SAD / quant paths
   against C's `avx512icl` runtime dispatch. This is per-pixel, so it is exactly
   what the slope measures, and a win here moves every size and preset.
2. **Per-SB rebuild overhead** (suspected: `MdRates` / `CoeffCostTables` rebuilt
   per superblock rather than once per frame). Consistent with preset 6's larger
   per-pixel constant and elevated small-size cost.
3. **Per-frame allocation discipline** (secondary). The port allocates its
   working set inside `encode_frame_420`; C pre-allocates in `init`. It shows up
   as part of the port's honest per-frame cost but is dwarfed by (1) and (2) —
   the near-perfect linear fit says allocation is not the scaling problem.

Approach order per the criteria: algorithmic parity (done on this envelope),
then allocation discipline, then SIMD. On these numbers, SIMD on the hot loops
is the biggest single lever.

## Reproducibility / provenance

- Harness: `tools/perf_gate.sh`, `svtav1/examples/perf_encode.rs`,
  `tools/perf_c_encode/` (`.c` + `build.sh`; binary rebuilt on demand).
- Data: `benchmarks/perf_2026-07-20.tsv` (per-cell summary),
  `benchmarks/perf_2026-07-20.raw.tsv` (every paired sample),
  `benchmarks/perf_2026-07-20.meta` (provenance + fits).
- C oracle: the in-tree `libSvtAv1Enc.a` (mainline-equivalent, HDR mode off) —
  the same reference the identity campaign validates against.
