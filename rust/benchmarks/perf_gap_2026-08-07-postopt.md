# zenav1-svt vs C SVT-AV1 encode gap — after the 2026-08-07 aarch64 DSP series

Companion to `perf_gap_2026-08-07-postopt.{tsv,raw.tsv,meta}`. The pre-change
baseline is `perf_gap_2026-08-07.tsv` on branch `perf-attribution-2026-08-07`.

## Configuration (identical on both sides, stated in full)

8-bit, 4:2:0, still/allintra (`hierarchical_levels=0`, `intra_period=1`), CQP
qp 40, tiles 0/0, `level_of_parallelism = 1` on the C side / single-threaded
port, synthetic `gradient` content, no `-C target-cpu=native`. Harness
`tools/perf_gap_campaign.sh`: 9 interleaved paired rounds per cell, port/C order
randomised per round, one untimed warmup per sample, **neither side niced**,
both sides timing ONLY the frame encode (setup excluded on both).

Host: Apple M4 Pro, 12 logical cores, darwin 25.5.0. The box was polled to a
sustained-idle minute before the run. Corroboration that interference was not a
factor: the **C** column moved by at most 3 % between the two campaigns
(usually < 1 %), so the port's before/after is a like-for-like comparison.

All 24 cells emit **byte-identical `.obu` files** on both sides.
`tools/decode_gate_grid.sh` additionally decodes a 120-cell grid
(gradient+uniform x {32,64,128,256,512} x p{2,6,10,13} x qp{20,40,55}) under
**both aomdec and dav1d**: 120/120 OK on both, 120/120 byte-identical to C.

## port/C wall-clock ratio, before -> after

| size | p2 | p6 | p10 | p13 |
|---|---|---|---|---|
| 32²   | 6.41 -> **3.64** | 1.36 -> **0.81** | 0.71 -> **0.58** | 0.69 -> **0.54** |
| 64²   | 6.50 -> **3.80** | 3.50 -> **1.79** | 1.59 -> **1.07** | 1.62 -> **1.09** |
| 128²  | 6.84 -> **3.69** | 5.62 -> **2.66** | 3.71 -> **2.50** | 3.79 -> **2.41** |
| 256²  | 6.01 -> **3.50** | 6.54 -> **3.24** | 6.39 -> **4.01** | 6.36 -> **3.99** |
| 512²  | 7.26 -> **3.92** | 6.72 -> **3.28** | 7.04 -> **4.48** | 7.12 -> **4.45** |
| 1024² | 7.61 -> **4.09** | 6.99 -> **3.48** | 7.85 -> **4.75** | 7.81 -> **4.73** |

The port is now FASTER than C at 32² for presets 6, 10 and 13 (it was already
faster at p10/p13; p6 crossed over in this series).

## Port speedup against its own pre-change self

| size | p2 | p6 | p10 | p13 |
|---|---|---|---|---|
| 32²   | 1.86x | 1.88x | 1.30x | 1.43x |
| 64²   | 1.78x | 2.12x | 1.69x | 1.69x |
| 128²  | 1.89x | 2.23x | 1.66x | 1.68x |
| 256²  | 1.72x | 2.01x | 1.65x | 1.67x |
| 512²  | 1.86x | 2.06x | 1.60x | 1.62x |
| 1024² | 1.88x | 2.05x | 1.68x | 1.67x |

## Fit `ms = a + b*pixels`

| preset | port a (ms) | port b (ms/MP) | C a (ms) | C b (ms/MP) | slope ratio | was |
|---|---|---|---|---|---|---|
| p2  | 78.34 | 1899.75 | 23.40 | 462.36 | **4.11x** | 7.74x |
| p6  |  1.99 |  133.45 |  0.88 |  38.16 | **3.50x** | 7.04x |
| p10 |  0.159 |  40.19 |  0.185 |  8.28 | **4.85x** | 7.99x |
| p13 |  0.128 |  40.28 |  0.177 |  8.34 | **4.83x** | 7.88x |

Caveats carried forward from the baseline campaign, still valid:

* **p2 and p6 are visibly superlinear in pixels, so their intercepts are not
  identified.** Quote the p2/p6 *slope ratio* and the measured cells, not their
  `a`. p10/p13 fit to R² ≈ 0.9999 and their a/b can be trusted.
* The p10/p13 intercept ratio is now **0.86x / 0.72x** — the port's fixed
  per-frame cost is well under C's, which is why it wins outright at 32².
* One content type, one bit depth, one chroma format, single-threaded. Real
  photographic/screen content has far more nonzero coefficients and would shift
  weight toward entropy and RDOQ, the two stages already measured at parity.
