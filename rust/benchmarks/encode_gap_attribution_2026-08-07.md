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

## 1. Profiling method

macOS `/usr/bin/sample`, 1 ms interval, 20–25 s per profile, attached to a
`perf_encode`/`perf_c_encode` process running a long untimed warmup loop
(`warmup` argument), so the whole sample window is steady-state encode.
Reported numbers are **self time** ("Sort by top of stack"), not inclusive
stacks, except where a row is explicitly labelled inclusive.

> **Caveat, stated up front:** the profile runs and their paired wall-clock
> medians in this section were taken under `nice -n 19` on both sides, because
> another agent was running timing-sensitive A/B work on this box. On macOS
> `nice` maps to background QoS, which parks the work on the E-cores. Both
> encoders were niced identically and the medians used for the absolute-ms
> arithmetic were measured in that same regime, so the attribution is
> internally consistent — but the absolute millisecond values are E-core
> values, roughly 3–4× the P-core values, and the *ratio* may shift somewhat
> because E-cores have a different cache hierarchy. The headline gap numbers
> in `perf_gap_2026-08-07.*` are un-niced.

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
