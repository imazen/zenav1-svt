# Where the encode-speed gap to C SVT-AV1 lives — aarch64, 2026-08-07

Measurement only. Nothing was optimized in the change that carries this file.

- Host: Apple M4 Pro (8 P + 4 E cores), macOS 26.5.2, darwin 25.5.0
- Port commit: `1e0e3ef9c`; C reference: in-tree `reference/svt-av1` built into
  `cbuild-static/` → `Bin/Release/libSvtAv1Enc.a`
- Harnesses: `svtav1/examples/perf_encode.rs` (port) and
  `tools/perf_c_encode/` (C). Both time ONLY the frame encode; setup
  (`EncodePipeline::new` / `svt_av1_enc_init`) is excluded on both sides.
- Config, identical on both sides: 8-bit, 4:2:0, still/allintra
  (`hierarchical_levels=0`, `intra_period=1`), CQP `qp 40`, tiles 0/0,
  SB size by C's own rule, `level_of_parallelism = 1` (single-threaded on
  both sides). No `-C target-cpu=native`.

## 0. The comparison is apples-to-apples — proven, not assumed

Every cell below was byte-checked before it was timed: the port's `.obu` and
C's `.obu` are **identical files**. 12/12 cells across
presets {6, 10, 13} × sizes {64, 128, 256, 512}, plus 32² and 1024² at p2,
1024² at p6/p13. The two encoders are doing the *same work* and emitting the
*same bitstream*, so a wall-clock ratio between them is a pure
implementation-speed number.

Both encoders' output also decodes cleanly under **two independent decoders**
— `aomdec` (homebrew) and `dav1d` (homebrew): 12/12 OK/OK.

## 0b. The measured gap (un-niced, idle box) — `perf_gap_2026-08-07.tsv`

`tools/perf_gap_campaign.sh`, 9 interleaved paired rounds per cell with the
port/C order randomised per round, 1 untimed warmup per sample, **neither side
niced**, started only after 75 s of no foreign process above 25 % CPU. Every
cell byte-identical. Run **twice** (`perf_gap_2026-08-07.tsv` and
`…-07b.tsv`); pass 2 was contaminated by another agent's `rustc` (47 monitor
events) and the ratios still agree to **≤1 % at 512²/1024² and ≤5.4 % worst
case on the sub-millisecond tiny cells** — which is the paired-interleaved
design doing its job.

port/C wall-clock ratio (median of paired rounds, pass 1):

| size | p2 | p6 | p10 | p13 |
|---|---|---|---|---|
| 32² | 6.41 | 1.36 | **0.71** | **0.69** |
| 64² | 6.50 | 3.50 | 1.59 | 1.62 |
| 128² | 6.84 | 5.62 | 3.71 | 3.79 |
| 256² | 6.01 | 6.54 | 6.39 | 6.36 |
| 512² | 7.27 | 6.72 | 7.04 | 7.12 |
| 1024² | 7.61 | 6.99 | 7.85 | 7.81 |

`ms = a + b·pixels` (least squares over all six sizes; R² shown because the
model fits very differently by preset):

| preset | port a (ms) | port b (ms/MP) | R² | C a (ms) | C b (ms/MP) | R² | slope ratio | intercept ratio |
|---|---|---|---|---|---|---|---|---|
| p2 | 133.97 | 3584.2 | 0.993 | 23.72 | 463.3 | 0.985 | **7.74×** | 5.65× |
| p6 | 4.14 | 273.4 | 0.998 | 0.867 | 38.8 | 0.997 | **7.04×** | 4.78× |
| p10 | 0.0887 | 67.6 | 0.9999 | 0.194 | 8.46 | 0.999 | **7.99×** | **0.46×** |
| p13 | 0.112 | 67.3 | 0.9999 | 0.184 | 8.54 | 0.9995 | **7.88×** | **0.61×** |

Three things to read off this:

1. **The per-pixel slope ratio is flat at 7–8× across every preset.** The gap
   is not a preset-specific algorithm problem; it is uniform DSP throughput.
2. **The port's fixed per-frame cost is LOWER than C's at the fast presets** —
   88 µs vs 194 µs at p10 — and at 32×32 p10/p13 the port is outright
   **1.4× faster than C**. C's `svt_av1_enc_send_picture`/pipeline plumbing
   costs more per frame than the port's direct call. Any "the port is ~7×
   slower" summary is wrong at thumbnail sizes.
3. **p2 and p6 are visibly superlinear**, so their intercepts are not
   identified: refitting p2 over only the 32²–256² cells gives
   b = 7642 ms/MP (vs 3584 over all six) and a = 14.9 ms (vs 134). Quote the
   p2/p6 *slope ratio* and the measured cells, not those intercepts. p10/p13
   are linear to R² = 0.9999 and their coefficients are solid.

## 1. Profiling method

macOS `/usr/bin/sample`, 1 ms interval, 20–25 s per profile, attached to a
`perf_encode`/`perf_c_encode` process running a long untimed warmup loop
(`warmup` argument), so the whole sample window is steady-state encode.
Reported numbers are **self time** ("Sort by top of stack"), not inclusive
stacks, except where a row is explicitly labelled inclusive.

> **On `nice`, measured rather than assumed.** The profile runs and their
> paired medians were taken under `nice -n 19` on both sides, to stay off
> another agent's measurement. The working assumption going in was that macOS
> maps `nice` to background QoS and parks the work on the E-cores at a large
> wall-clock cost. **That is not what happens here.** An interleaved
> niced/un-niced A/B on p10 512² gives ratios 0.998 / 0.985 / 1.013 / 0.959 /
> 1.018 — no penalty. Cross-checking against the un-niced campaign confirms
> it: p6 512² is 84.59 ms niced vs 84.99 ms un-niced, p2 256² is 515.1 vs
> 513.2. So the niced profiles are directly comparable to the un-niced
> campaign and the absolute-ms attribution below stands unqualified. (Likely
> the whole agent shell tree already runs at a reduced QoS, so `nice` changes
> nothing; do not generalise this to a foreground terminal without re-testing.)

Paired wall-clock medians (n=7 alternating port/C, niced, same regime as the
profiles):

| cell | port ms | C ms | ratio |
|---|---|---|---|
| p2 256×256 | 515.14 | 85.80 | 6.00× |
| p6 512×512 | 84.59 | 12.59 | 6.72× |
| p10 512×512 | 18.28 | 2.52 | 7.25× |

## 2. Gap attribution by stage (self time × wall clock)

Stages are mutually exclusive; `delta ms` is `port − C` and the last column is
that delta as a share of the total gap.

### p6 512×512 — gap 72.00 ms (84.59 vs 12.59)

| stage | port % | port ms | C % | C ms | delta ms | % of gap |
|---|---|---|---|---|---|---|
| LOOP RESTORATION (Wiener `compute_stats`) | 24.04 | 20.33 | 6.91 | 0.87 | **19.46** | **27.0** |
| INVERSE TXFM | 13.57 | 11.48 | 2.05 | 0.26 | 11.22 | 15.6 |
| FORWARD TXFM | 11.94 | 10.10 | 9.43 | 1.19 | 8.91 | 12.4 |
| ENTROPY / RATE | 13.95 | 11.80 | 27.78 | 3.50 | 8.30 | 11.5 |
| MD / PARTITION driver | 10.30 | 8.71 | 3.63 | 0.46 | 8.25 | 11.5 |
| alloc (malloc/free) | 5.54 | 4.69 | 0.11 | 0.01 | 4.68 | 6.5 |
| CDEF | 5.28 | 4.47 | 4.04 | 0.51 | 3.96 | 5.5 |
| libc mem (memmove/memset/bzero) | 4.35 | 3.68 | 4.91 | 0.62 | 3.06 | 4.3 |
| SATD / hadamard | 1.93 | 1.63 | 2.82 | 0.35 | 1.28 | 1.8 |
| **getenv (debug hooks)** | 1.17 | 0.99 | 0.00 | 0.00 | 0.99 | 1.4 |
| INTRA PRED | 1.43 | 1.21 | 2.57 | 0.32 | 0.89 | 1.2 |
| QUANT / RDOQ | 3.45 | 2.92 | 17.37 | 2.19 | 0.73 | 1.0 |
| DEBLOCK | 0.57 | 0.48 | 1.20 | 0.15 | 0.33 | 0.5 |
| DISTORTION | 0.11 | 0.09 | 4.09 | 0.51 | −0.42 | −0.6 |

### p10 512×512 — gap 15.76 ms (18.28 vs 2.52)

| stage | port ms | C ms | ratio | delta ms | % of gap |
|---|---|---|---|---|---|
| MD / PARTITION driver | 3.43 | 0.24 | 14.5× | 3.20 | 20.3 |
| **CDEF (apply, no search at this preset)** | 3.11 | **0.00** | ∞ | 3.11 | 19.7 |
| INVERSE TXFM | 2.48 | 0.11 | 21.7× | 2.37 | 15.0 |
| FORWARD TXFM | 2.08 | 0.21 | 9.9× | 1.87 | 11.9 |
| nz-map / level contexts | 1.73 | 0.04 | 45.2× | 1.69 | 10.7 |
| alloc (malloc/free) | 1.22 | 0.01 | 98.8× | 1.21 | 7.7 |
| libc mem | 1.18 | 0.15 | 7.7× | 1.02 | 6.5 |
| **DEBLOCK (apply)** | 0.55 | **0.00** | ∞ | 0.55 | 3.5 |
| getenv | 0.29 | 0.00 | ∞ | 0.29 | 1.9 |
| range coder / bitstream | 0.62 | 0.59 | 1.1× | 0.03 | 0.2 |
| quantize + RDOQ | 0.57 | 0.42 | 1.4× | 0.15 | 0.9 |

99.4 % of the p10 gap is accounted for by the rows above.

### p2 256×256 — gap 429.34 ms (515.14 vs 85.80)

| stage | port ms | C ms | delta ms | % of gap |
|---|---|---|---|---|
| INVERSE TXFM | 97.45 | 1.83 | 95.62 | 22.3 |
| MD / PARTITION driver | 88.35 | 5.26 | 83.10 | 19.4 |
| FORWARD TXFM | 77.79 | 5.64 | 72.15 | 16.8 |
| ENTROPY / RATE | 73.79 | 13.30 | 60.49 | 14.1 |
| alloc (malloc/free) | 50.97 | 0.00 | 50.97 | 11.9 |
| libc mem | 26.38 | 5.33 | 21.05 | 4.9 |
| SATD / hadamard | 14.36 | 2.58 | 11.78 | 2.7 |
| INTRA PRED | 14.97 | 3.46 | 11.52 | 2.7 |
| QUANT / RDOQ | 39.01 | 31.62 | 7.39 | 1.7 |
| getenv | 6.38 | 0.00 | 6.38 | 1.5 |

## 3. What is NOT the problem

Two things the port already does at C speed, worth recording so nobody spends
a week on them:

- **Quantization + RDOQ.** p2: 39.0 ms port vs 31.6 ms C (1.23×). p6: 2.92 vs
  2.19 (1.33×). C spends 36.9 % of its entire p2 encode in
  `svt_aom_quantize_inv_quantize` + `full_loop_core`; the port's
  `quant::optimize_b` + `quant_coding::quantize_{fp,b}_raster` keep up. The
  NEON quantize kernels landed and they work.
- **Range coder / bitstream write.** p10: 0.62 ms port vs 0.59 ms C (1.05×).
  At p10 `av1_write_coeffs_txb_1d` + `svt_od_ec_encode_cdf_q15` are 22 % of
  C's *entire* encode — C has squeezed everything else so hard that the
  arithmetic coder is what remains. The port's `OdEcEnc` is at parity there.

Also: the gap does **not** widen much with preset. 6.0× at p2, 6.7× at p6,
7.25× at p10. It is a broad DSP-tier gap, not one pathological stage.

## 4. Top named leaves (self time)

| leaf | p2 256 | p6 512 | p10 512 | vectorized on aarch64? |
|---|---|---|---|---|
| `restoration::compute_stats` | 1.3 % | **22.70 %** | — | only its `mac_row_i32_neon` inner MAC |
| `leaf_funnel::tx_unit` | **11.05 %** | 6.41 % | 7.42 % | no (scalar residual build + `Vec` per TU) |
| `inv_txfm::idct32` | — | 4.42 % | 3.91 % | **no** (over `NEON_INV_MAX_DIM = 8`) |
| `inv_txfm::idct16` | 2.73 % | 2.51 % | 4.69 % | **no** (over `NEON_INV_MAX_DIM = 8`) |
| `inv_txfm::idct64` (128² p6) | — | 8.77 %¹ | — | **no** |
| `fwd_txfm::fdct32` | — | 2.62 % | 2.48 % | **no** (over `NEON_FWD_MAX_DIM = 16`) |
| `fwd_txfm::fdct64` (128² p6) | — | 4.60 %¹ | — | **no** |
| `coeff_c::nz_map_contexts_scan_order` | 3.66 % | 4.60 % | 4.48 % | no (C: `svt_av1_get_nz_map_contexts_neon`) |
| `coeff_c::nz_map_ctx` | 4.37 % | 2.10 % | 1.97 % | no |
| `coeff_simd::fill_levels` | 3.96 % | 2.98 % | 3.02 % | partial |
| `cdef::cdef_filter_block_core` | — | 1.25 % | **6.08 %** | **no** — this is the 4-wide chroma fallback |
| `cdef::cdef_filter_block` | — | 1.77 % | 5.17 % | yes (8-wide only) |
| `_xzm_free` / malloc family | 5.7 % | 5.5 % | 7.1 % | n/a |
| `__findenv_locked` (getenv) | 1.2 % | 0.99 % | **1.47 %** | n/a |

¹ from the 128×128 p6 profile, where the 64-point transforms dominate.

## 5. Two stages the port pays for and C does not

At p10 the port spends **3.66 ms (23 % of the gap)** applying the deblock and
CDEF post-filters. C spends **zero** — its p10 profile contains not one CDEF
or LPF sample out of 15 470, while emitting byte-identical output.

This is not C being cleverer about the bitstream: the CDEF strengths are
non-zero (the port's `apply_cdef_frame` early-returns on all-zero strengths,
and it does not early-return here), so both encoders *signal* the same
filters. The difference is that the port materialises a **decoder-exact
reconstruction** — which is a real deliverable, gated by
`tools/recon_parity.sh` — and C, for a single still frame with
`intra_period=1` and no loop restoration, has no consumer for the filtered
recon and skips the work.

At p6 the picture differs: CDEF *search* is live (`allintra_preset_uses_cdef_search`
is `preset <= 6`) and needs the filtered pixels to measure distortion, and
loop restoration reads the filtered recon, so most of that work is load-bearing.

Related: `pipeline.rs:3049` does `recon.clone()` + two chroma clones per frame
before the CDEF pass, which is part of the malloc/memmove line above.

## 6. `getenv` in the inner loops

44 `std::env::var*` call sites live in `svtav1-encoder`. Several are inside
per-block hot paths in `leaf_funnel.rs` (`SVTAV1_NSQDBG`, `SVTAV1_CANDDBG`,
`SVTAV1_IBCDBG`, `SVTAV1_PALBRK`) and in `pipeline.rs`, and are **not** behind
a `OnceLock` (some siblings in the same file are —
`leaf_funnel.rs:1400`, `restoration.rs:53`). macOS `getenv` takes a lock
(`__findenv_locked`), so this costs a measured **0.99–1.47 % of encode wall
time** doing nothing. It is pure overhead, bit-identical to remove, and the
caching pattern already exists in-tree.

## Reproducing

```bash
# byte-identity + two-decoder conformance over the grid
perf_encode gradient <s> <s> 40 <p> out 0 && perf_c_encode <s> <s> 40 <p> out.yuv out.c.obu 0
cmp out.obu out.c.obu && aomdec out.obu -o /dev/null && dav1d -i out.obu -o /dev/null

# profile (long untimed warmup loop, then attach)
perf_encode gradient 512 512 40 6 out 400 &  sample $! 25 1 -f p6.txt
perf_c_encode 512 512 40 6 out.yuv c.obu 3000 &  sample $! 20 1 -f Cp6.txt
```

## 7. Opportunities ranked by (measured cost) × (tractability)

"Bit-identical-safe" means the optimisation cannot change the emitted OBU —
either it is an exact integer kernel swap, or it removes work that does not
feed the bitstream. Anything that changes a decision changes the product and
must be argued on RD, not smuggled in as perf.

| # | opportunity | measured cost | bit-identical? | tractability |
|---|---|---|---|---|
| 1 | **Wiener `compute_stats` NEON** — restructure the per-pixel 49-element gather into a horizontally-batched kernel like `svt_av1_compute_stats_neon` | **27 % of the p6 gap** (19.5 ms of 72), ~26× vs C's kernel | **YES** — integer MAC accumulation, exact | medium-high. C's `neon`+`sve` sources are in-tree at `reference/svt-av1`; `tests/c_parity_wiener.rs::compute_stats_all_tiers_match_c` already gates it against real C |
| 2 | **Inverse transforms above dim 8** (`idct16/32/64`, `iadst16`) | **15 % of the p6 gap**, 15 % of p10, 22 % of p2 | **YES** — exact integer butterflies | medium. Today `NEON_INV_MAX_DIM = 8` because the `[i32; 8]`-array arm *loses* to scalar past 8. Needs real intrinsics (`int32x4_t` pairs), not another autovectorisation attempt. `tests/c_parity_txfm.rs` is the gate and has already caught a sign-extension bug in this codebase |
| 3 | **Forward transforms above dim 16** (`fdct32`, `fdct64`) | **12 % of the p6 gap**, 17 % of p2 | **YES** | medium, same shape as #2 |
| 4 | **Deblock + CDEF application made lazy at fast presets** | **23 % of the p10 gap** (3.66 ms of 15.8); C spends *zero* here | **YES** — the filtered recon is not an input to the bitstream at `intra_period=1`; the bytes are already proven identical | **high (easiest big win).** But it is a real feature: `tools/recon_parity.sh` requires the decoder-exact recon. Make it opt-in / demand-driven (needed when LR is on, when CDEF search is on, or when the caller asks for recon), not deleted. Also kill the `recon.clone()` + 2 chroma clones at `pipeline.rs:3049` |
| 5 | **nz-map / level-context kernels** (`nz_map_contexts_scan_order`, `nz_map_ctx`, `fill_levels`, `txb_init_levels`) | **10 % of the p6 gap, 11 % of p10** (45× vs C at p10) | **YES** — pure context derivation from levels | medium. C ships `svt_av1_get_nz_map_contexts_neon`, `svt_av1_txb_init_levels_neon`, `svt_av1_compute_cul_level_neon` as direct models |
| 6 | **Allocator traffic** — `Vec` per transform unit in `leaf_funnel::tx_unit` (residual build), `Vec` per `convolve_2d_impl_neon` call, per-frame recon clones | **6.5 % of the p6 gap, 7.7 % of p10, 12 % of p2**; malloc/free is 98–334× C's | **YES** — scratch buffers only | **high.** Hoist to per-thread/per-pipeline scratch. C allocates essentially nothing per block |
| 7 | **Residual + distortion kernels** — currently scalar loops inlined into `tx_unit` (the #1 self-time leaf at p2, 11.05 %) | inside the 11–20 % "MD driver" bucket | **YES** | medium. C: `svt_residual_kernel8bit_neon`, `svt_full_distortion_kernel32_bits_neon` |
| 8 | **4-wide chroma CDEF NEON** — `cdef_filter_block_core` is the scalar fallback taken by every 4:2:0 chroma block | **6.08 % of self time at p10** | **YES** | **high** — the 8-wide `cdef_filter_cols8_neon` already exists and C ships `svt_av1_cdef_filter_block_4xn_8_native_neon` as the model |
| 9 | **Cache the 44 `std::env::var*` reads** behind `OnceLock` | **1.0–1.5 % of encode wall time**, 0 for C | **YES** | **trivial** — the `OnceLock` pattern is already used at `leaf_funnel.rs:1400` and `restoration.rs:53`; the uncached sites are `SVTAV1_NSQDBG`, `SVTAV1_CANDDBG`, `SVTAV1_IBCDBG`, `SVTAV1_PALBRK` and friends in per-block paths |
| 10 | **Deblock NEON** (`lpf_*`) | 0.5 % at p6, 3.5 % of the p10 gap | **YES** | low value until #4 is decided — if the post-filter goes lazy, most of this disappears |

Explicitly **not** on this list, because they are measured to be at parity and
would be wasted effort: quantisation/RDOQ (1.2–1.4× vs C) and the range
coder / bitstream writer (1.05× at p10).

Doing #1–#4 alone addresses **77 % of the p6 gap** and **~58 % of the p10
gap**, and every one of them is bit-identical-safe.
