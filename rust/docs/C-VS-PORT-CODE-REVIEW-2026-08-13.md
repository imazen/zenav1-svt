# C vs port — a side-by-side implementation read, and where the remaining encode-speed gap can still be

Date: 2026-08-13. Port commit at review time: `29847e5d3`; `bf781a49a` landed
mid-review and its results are folded in below (aarch64 / Apple M4 Pro).
C reference: in-tree `reference/svt-av1` @ v4.2.0.

**This document contains no new measurements.** Nothing here was benchmarked; no
binary was built for it; `~/bin/measlock` was not taken. Every number quoted is
cited to an existing record in `benchmarks/` or `docs/`. Every structural claim
is cited to `file:line` on **both** sides. Where a claim is a hunch it is
labelled `SPECULATIVE` and no number is attached to it.

> **STATUS UPDATE 2026-08-13 (later the same day): R1 and R2 have been priced,
> built, measured and LANDED.** §7 asked whoever picked them up to record the
> census result here; the outcome is at the top of §4 below, and the two
> corrections to this document's own scope claims are in §4 R1/R2. Slope
> ratios moved p10 3.06x -> **2.89x**, p13 3.07x -> **2.89x**, p6 3.39x ->
> **3.27x**, p2 4.14x -> **3.93x** (`benchmarks/perf_gap_2026-08-13-r1r2.meta`).
> R3-R9 are untouched.

Written while another agent was concurrently editing `partition.rs`,
`depth_refine.rs` and `pipeline.rs` (the `BlockDecision` / arena work). Line
numbers in those three files may have moved; the symbol names have not.

---

## 0. What I did NOT examine

Read this first, so the ranking below is read with the right amount of trust.

- **Nothing was measured.** No profile was taken, no A/B was run, no build was
  made for this review. Every "cost" statement is either a citation to an
  existing record or is absent. The three top-ranked ideas below are ranked on
  *source structure* plus *previously-measured stage shares*, not on a measured
  delta for the specific change proposed.
- **The allocation/arena work in flight is out of scope by instruction.**
  `BlockDecision` clone / `drop_glue` / `encode_fixed_tree` belongs to a
  concurrent agent. I sized the rest of the gap *around* it and did not analyse
  it. (`bf781a49a` landed the first slice mid-review: 1.07–1.11× at p10,
  1.01–1.03× at p6 — see §1.)
- **The bd10 / high-bit-depth mirrors** (`tx_unit_hbd`, `predict_unit_hbd`,
  `hbd.rs`) were read only where the bd8 path forced me to. All findings below
  are stated for the **bd8 4:2:0 still** path, which is what the gap numbers
  were taken on.
- **The inter path** (motion estimation, OBMC, warp, compound, `mv_coding`) —
  not examined. It is dormant on the still gates.
- **PD0 and the depth-refinement walk** (`pd0.rs`, `depth_refine.rs`) were read
  only for the specific mechanisms named below. I did not audit them against C
  end to end, and `depth_refine.rs` is being edited concurrently.
- **The entropy-coding / bitstream-writing stage** — not examined. The
  2026-08-07 attribution measured the range coder at 1.05× C at p10
  (`benchmarks/encode_gap_attribution_2026-08-07.md` §3); I took that as settled
  and did not re-read it.
- **CDEF search, loop-restoration search, and deblock** were examined only for
  *which kernels are vectorised*, not for algorithmic structure. They are
  p ≤ 6 costs (12.5 % LR + 9.3 % CDEF of p6 self time, `docs/perf-status.md`)
  and I did not compare their search structure to C's.
- **Anything about x86-64.** The live numbers are aarch64; the AVX2 arms were
  not reviewed.
- **Issue #15 was not root-caused.** §6 is a narrowing of the search space and
  one lead, not a diagnosis.
- **I did not verify that C's `_neon` symbols are actually selected at runtime**
  by SVT-AV1's RTCD tables — only that the sources exist.

---

## 1. Measured baseline (quoted, not re-derived)

From `docs/perf-status.md` (2026-08-11 header) and
`benchmarks/perf_gap_2026-08-11.{tsv,meta}`, all cells byte-identical:

| preset | slope ratio port/C | 1024² cell ratio |
|---|---|---|
| p2 | 4.12× | 4.09 |
| p6 | 3.52× | 3.49 |
| p10 | 3.53× | 3.46 |
| p13 | 3.51× | 3.45 |

The port is **faster** than C at 64² on the fast presets (0.76×) and its
intercept ratio is 0.86×. The entire gap is per-pixel slope.

Self time at 512², `/usr/bin/sample`, from the same header:

| stage | p10 | p6 |
|---|---|---|
| MD / leaf / partition driver | 24.2 % | 16.1 % |
| TRANSFORMS | 16.7 % | 16.6 % |
| ENTROPY / coeff | 15.9 % | 12.7 % |
| alloc (malloc/free) | 14.0 % | 8.6 % |
| libc mem | 11.0 % | 6.9 % |
| QUANT / RDOQ | 4.8 % | 4.9 % |
| INTRA PRED | 3.8 % | 2.7 % |
| LOOP RESTORATION | 0 % | 12.5 % |
| CDEF | 0 % | 9.3 % |

Two older records that the ranking below leans on, both from
`benchmarks/encode_gap_attribution_2026-08-07.md` §2 (**taken before the NEON
transform / Wiener / nz-map / residual kernels landed — the *stage shares* are
stale, the *fact of which stage the work belongs to* is not**):

- INVERSE TXFM: 15.0 % of the p10 gap, 15.6 % of the p6 gap, 22.3 % of the p2 gap.
- nz-map / level contexts: 10.7 % of the p10 gap, 45.2× C's cost at p10.

And one negative result that constrains everything in the allocation family —
`benchmarks/alloc_traffic_null_2026-08-07.meta`:

> hadamard_satd + coeff_contexts as STACK arrays: 0.983×–1.008×. The same three
> as per-thread SCRATCH: 0.985×–1.016×. Every one straddles 1.0.
> "Do not spend more time on allocation removal in this encoder without first
> demonstrating a wall-clock delta on a small pilot."

That result is why no idea below is "move allocation X to a scratch buffer".

### 1b. Three corrections that landed mid-review (`bf781a49a`, `benchmarks/alloc_decisioncopy_ab_2026-08-13.meta`)

Read these before trusting anything written before 2026-08-13, including the
`docs/perf-status.md` header quoted above.

1. **The allocation family has a measured arithmetic ceiling of 1.24×, and it is
   not enough on its own.** The port's alloc + libc-mem excess over C is ~19.6
   points of its own self time at 512² p10 (port: malloc 11.85 % + platform
   10.91 %; C: 0.49 % + 2.70 %), so eliminating *all* of it caps out at
   `1/(1−0.196) = 1.24×`. Against the measured 3.53× p10 slope ratio that leaves
   ~2.85× (arithmetic on the two measured numbers, not an extrapolation of
   either). **The work in flight is the largest single named lever and it still
   cannot close the gap.** Everything else in this document is what is left.
2. **The traffic is NOT as diffuse as previously recorded.** The
   "largest single parent is 2.4 % of mem samples" claim is withdrawn by its own
   author; at 1 ms self-time resolution with nearest-app-ancestor attribution
   the top nine parents cover 68 % of malloc-family samples:
   `evaluate_leaf` 19.52 %, **`tx_unit_inner` 11.18 %**, `chroma_dec` drop_glue
   6.82 %, `drop_glue::<Cand>` 6.60 %, `BlockDecision::clone` 6.31 %,
   **`extract_neighbors_tiled` 6.10 %**, `pd0::lvl5_like_block_cost` 4.79 %,
   `pd0::tx_quant_core` 3.56 %, `drop_glue::<BlockDecision>` 3.27 %.
   Two of those (bold) are named directly in R1/R4 below and are outside the
   `BlockDecision` work.
3. **This repo now has a measured noise floor.** A/B arms are built with
   `-C llvm-args=-align-all-functions=4` and run against a byte-identical-binary
   identity control whose widest band is **[0.9843, 1.0190]**. *Read anything
   inside [0.984, 1.019] as NULL.* This supersedes the "establish the layout
   floor first" advice in R9 — it exists.

---

## 2. The organizing insight: byte-identity closes off "C prunes more"

This is the most useful thing in the document and it took reading both trees to
believe it.

Byte-identity to C is the standing gate. Every C pruning gate that can change
which candidate wins **must already be ported**, or the bytes would differ. And
they are — I checked the ones C's all-intra path actually arms:

| C mechanism | C site | port |
|---|---|---|
| NIC funnel 64→32→16 + preset/QP scaling | `product_coding_loop.c:1361-1390` | `leaf_funnel.rs:1113` `nic_counts` |
| post-MDS0/1/2 NIC pruning | `:7819`, `:7885`, `:7963` | `leaf_funnel.rs` `FunnelCfg::mds{1,2,3}_cand_base_th` / `mds1_class_th` |
| `enable_skipping_mds1` (collapse to MDS0→MDS3) | `:7879-7881` | `leaf_funnel.rs:1031` (`n1 == 1`) |
| TXT rate pre-screen | `:4709-4718` | `leaf_funnel.rs:8864-8872` |
| TXT SATD early exit | `:4741-4755` | `leaf_funnel.rs:8936-8958` |
| zero-coeff non-DCT rejection | `:4779-4781` | `leaf_funnel.rs:8962-8965` |
| TXS `prev_depth_coeff_exit` / quadrant projection | `:5295`, `:5376-5390` | `FunnelCfg::txs_prev_depth_exit` / `txs_quadrant_sf` |
| depth early exit / split-cost th | `:10467-10470` | `depth_refine.rs:1171-1172`, `pd0.rs:1826-1828` |
| coeff-rate closed forms (`6000 + eob*k`) | `:4915-4919`, `:5540-5544` | `leaf_funnel.rs:2088-2097`, `:6223-6232` |
| PD0 fast coeff-rate prefix loop | `rd_cost.c:327-331` | `pd0.rs:982-983` |

**So "C does less work because it prunes harder" is not available as an
explanation for the 3.5× slope.** The only places C can legitimately be doing
less *algorithmic* work while emitting the same bytes are:

**(a) work whose result is discarded** — invisible to the bitstream, therefore
invisible to the byte-identity gate; and
**(b) per-unit efficiency** — SIMD, layout, allocation, redundant recomputation
of a value that is then used.

Ideas R1–R3 are all class (a). That is why they rank above the SIMD work.

---

## 3. Questions I CLOSED by reading — do not spend time on these

Each of these looked like a candidate gap and is not one. Recorded so the next
session does not re-derive them.

1. **Partial-frequency transforms (N2 / N4 / ONLY_DC).** C has genuinely
   different reduced kernels (`transforms.c:3963-4004`) that compute only the
   top-left quadrant, and the port has none. **Not a gap**: the all-intra
   derivation sets `pf_level = 1` → `DEFAULT_SHAPE` (`enc_mode_config.c:8126`
   → `:3420-3422`), and the only override (`use_tx_shortcuts_mds3`,
   `product_coding_loop.c:4630-4632`) is gated on `tx_shortcut_ctrls`, which
   all-intra pins to level 0 (`enc_mode_config.c:9974` →
   `:6727-6732`: `bypass_tx_th = 0`, `apply_pf_on_coeffs = 0`,
   `use_mds3_shortcuts_th = 0`).
2. **`tx_shortcut_ctrls.bypass_tx_th` — skipping the transform outright at
   MDS3** (`product_coding_loop.c:6898-6903`). Same reason: level 0 at
   all-intra. Not reachable.
3. **`shut_fast_rate`.** `enc_mode_config.c:8141` sets it `false` on the
   all-intra path. Not reachable.
4. **`mds0_ctrls` distortion-to-best-cost pruning** (`:1309-1334`). The whole
   block is gated on `pruning_method_th`, and all-intra sets `mds0_level = 0`
   → `pruning_method_th = 0` (`enc_mode_config.c:10042` → `:6767-6769`). The
   port already records this at `leaf_funnel.rs:460` and `:5096`. Not a gap.
5. **`mds_fast_coeff_est_level` — C pricing only a prefix of the coefficients**
   (`rd_cost.c:327-331`). In the funnel (PD_PASS_1) C pins it to 1 —
   `product_coding_loop.c:7026`, `:7051`, `:7132`, `:7155`, `:7386`, `:7597`
   all read `(ctx->pd_pass == PD_PASS_1) ? 1 : pd0_fast_coeff_est_level`. The
   port's hardcoded full middle loop (`leaf_funnel.rs:1463`, `:1573`) is
   correct, and PD0's level-2 prefix loop **is** ported (`pd0.rs:982-983`).
   Not a gap.
6. **Threading.** At the config the numbers were taken on (`--lp 1`,
   `tiles 0/0`), C's EncDec segment grid collapses to 1×1
   (`Globals/enc_handle.c:231-238`) and `enc_dec_process_init_count = 1`
   (`:603-613`); tiles are off by default (`:252-254`); there is no sub-SB
   fork/join anywhere in C. Both encoders are single-threaded on the measured
   cells. **Threading does not explain any part of the measured gap.**
   (Forward-looking, not a gap explanation: C's wavefront segment scheme
   *is* byte-identical intra-frame parallelism within a single tile, and the
   port's only intra-frame parallelism is tiles — `pipeline.rs:8410-8420` —
   which change the bytes. That is a real architectural difference for
   wall-clock throughput at `lp > 1`, and it is not what the slope ratio
   measures.)
7. **The 2026-08-07 NEON audit's transform findings are obsolete.**
   `NEON_FWD_MAX_DIM` and `NEON_INV_MAX_DIM` are both **64** at HEAD
   (`crates/svtav1-dsp/src/txfm_simd.rs:686`, `:688`) — the guards never fire,
   and real `int32x4_t` kernels exist for 32- and 64-point forward and inverse
   (`txfm_simd_drivers.rs:167-168`, kernels at `txfm_simd_kernels.rs:392`,
   `:506`, `:613`, `:837`). Likewise `compute_stats` is a real dot-product
   kernel now (`restoration.rs:337` → `dot_i16_neon` `:520`), and the 4-wide
   chroma CDEF arm landed (`cdef.rs:470`). Do not re-cite
   `benchmarks/neon_tier_audit_2026-08-07.md` §B2/§C as current.

---

## 4. Ranked ideas

Evidence classes: **MEASURED** (a wall-clock A/B in this repo) · **PROFILED**
(attributed self time in this repo) · **SOURCE** (read from both trees, cited)
· **SPECULATIVE** (a hunch, labelled).

Byte-identity column: **SAFE** = cannot change the OBU by construction ·
**PROVE** = safe only if a stated precondition holds and must be proven per site
· **CHANGES BYTES** = a different product, must be argued on RD.

---

### R1. Gate the inverse transform + reconstruction on C's own predicate

**LANDED 2026-08-13 (`56d19efe1`). MEASURED 1.021x-1.053x at qp40 on 12 of 12
cells (256/512/1024 x p6/p8/p10/p13), 28 of 28 cells below 1.0 across 6 presets
x 3 sizes x 2 qps against a byte-identical-binary control that split 13/15
(sign test p = 3.7e-9). Record: `benchmarks/recon_gate_r1_ab_2026-08-13.meta`.**

The census this item asked for (`benchmarks/txunit_census_2026-08-13.tsv`,
feature `__txcensus`) put the DISCARDED share of inverse-transform PIXEL work
at 40-50 % (p10/p13), 36-50 % (p8), 43-51 % (p7), 28-53 % (p6), 24-44 % (p2) —
so the falsifier ("a call census showing the `spatial_dist == false` population
is a small share") did not fire, and the doubling arm was not needed. All three
sites were proved dead by an exhaustive scan of every occurrence of the binding
in its own scope; the proofs are in the commit message. **One correction to the
site table below:** it implies three contributing sites at the fast presets. At
p10/p13 only MDS1 contributes — the CfL and non-CfL-chroma sites both sit
behind `cfg.cfl_enabled`, which `FunnelCfg::for_preset` sets false from M7 up.

**Evidence: SOURCE (both sides) + PROFILED (stage share).** Byte-identity:
**PROVE** (per call site).

C runs the inverse transform only when it needs it:

```c
// product_coding_loop.c:4783-4784
// Perform inverse TX if using spatial SSE or INTRA and tx_depth > 0
if (ctx->mds_do_spatial_sse || (!is_inter && cand_bf->cand->block_mi.tx_depth)) {
```

and the same gate on chroma (`full_loop.c:2313`, `:2532`:
`if (is_full_loop && ctx->mds_do_spatial_sse)`) and on the single-TX luma path
(`product_coding_loop.c:5727`). `mds_do_spatial_sse` is staged per MD stage:
`level <= SSSE_MDS1` at MDS1 (`:7025`), `<= SSSE_MDS2` at MDS2 (`:7047`),
`<= SSSE_MDS3` at MDS3 (`:7154`) — and the all-intra derivation sets
`spatial_sse_full_loop_level = 3` (`enc_mode_config.c:10010`). **So at all-intra
C's MDS1 and MDS2 do no inverse transform at all**; they take the
coefficient-domain distortion branch (`product_coding_loop.c:4879`).

The port has no such gate. `tx_unit_inner` allocates `recon` and inverse-
transforms unconditionally:

```rust
// leaf_funnel.rs:1955-1988
// Reconstruction (needed for spatial dist AND for depth-1 neighbor
// prediction — C inverts whenever spatial SSE or intra tx_depth > 0).
let mut recon = vec![0u8; n];
if eob > 0 {
    ...
    let ok = svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(dq_full, inv, w, ...);
    svtav1_dsp::residual::recon_add_clamp(...);
```

The comment names C's condition exactly and then does not implement it.

Three call sites pass `spatial_dist = false` and provably discard `out.recon`:

| site | port | what it reads |
|---|---|---|
| MDS1 luma full loop | `leaf_funnel.rs:5286` (`false, // freq-domain dist`) | only `out.eob/.bits/.dist` — I grepped 5286..5665 for `.recon`/`.qcoeff`: **no hits**. The code even says so at `:5313-5315` |
| CfL alpha search | `leaf_funnel.rs:2877`, inside `md_cfl_rd_pick_alpha`'s `plane_cost` closure | returns `(out.dist, out.bits)` only — `:2882` |
| non-CfL chroma re-cost | `leaf_funnel.rs:6668`, `:6689` (`u_nc` / `v_nc`) | only `.dist` — `:6710-6714` |

At MDS1 the discarded work per candidate per leaf is: one inverse transform,
one `recon_add_clamp`, one `vec![0u8; n]`, one `vec![0i32; pw*ph]` for `qcoeff`
(also unread there), and one `compute_cul_level` pass. MDS1 runs for every
candidate that survives MDS0, at every preset. The CfL site runs inside a
`2 planes × 2 signs × 16 magnitudes` search (`leaf_funnel.rs:2911-2929`) and is
p ≤ 6 only (`cfl_enabled: false` at M7+, `leaf_funnel.rs:1048`, `:1081`).

Why it ranks first: it is the only lever I found that is (i) algorithmic rather
than micro-optimisation, (ii) present at **every** preset, and (iii) lands on a
stage the repo has already measured as large — inverse transforms were 15.0 %
of the p10 gap and 15.6 % of the p6 gap
(`encode_gap_attribution_2026-08-07.md` §2), and transforms are still 16.7 % /
16.6 % of current self time. **I have not measured what fraction of that belongs
to the discarding call sites, and I am not going to guess.**

**Byte-identity:** safe *if and only if* the recon is genuinely unread at the
site. That is a per-site proof obligation, not an assumption — and it is
exactly the kind of thing that must be re-checked when a new caller appears.
The right shape is a `need_recon: bool` (or a `TxWant` flag set) threaded from
the caller, defaulting to `true`, with the three sites above opting out; **not**
inferring it from `spatial_dist`, because `spatial_dist == false` and
`need_recon == true` is a legal combination the moment `tx_depth > 0` matters.

**What would falsify it:** a call census showing the `spatial_dist == false`
population is a small share of total `tx_unit` pixel-work; or a doubling arm
showing the inverse transform at those sites prices near zero.

**Cheapest experiment (one binary, no refactor):**
1. A census behind a cargo feature, in the `residual::ovf_probe` style that
   already exists (`benchmarks/sse_i32_width_2026-08-11.meta` §2 — including
   its positive-control discipline): count `tx_unit` calls and sum `w*h`,
   bucketed by `(spatial_dist, eob > 0)`. This tells you the share of
   inverse-transform pixel-work that is dead, exactly, for ~20 lines.
2. If the share is material, a `TXUNIT_INV_DOUBLE` arm — run the inverse
   transform **twice** at the discarding sites in one binary — gives the
   marginal price of one, the way `RAV1D_LF_DOUBLE` did. No API change, no
   byte risk (the second result is overwritten), and it prices the change
   before anyone writes it.

---

### R2. Stop computing the exact coefficient rate at the sites that discard it

**LANDED 2026-08-13 (`8179a7d94`). MEASURED 1.038x-1.060x at p10/p13 qp20 on 6
of 6 cells (quartile-disjoint from the control); 1.018x-1.022x at 256 qp40;
NULL at 512/1024 qp40 and null at p2/p6/p7/p8. Record:
`benchmarks/ratemode_r2_ab_2026-08-13.meta`.**

Two things this item got wrong, both settled by the census:

1. **The scope note treats the level-2 tier (presets 7-8) as a live arm. It is
   effectively dead.** It requires `eob < (w*h)>>6`, which fired on 164 of
   8,404 calls in ONE of eighteen p7/p8 cells (328 coefficients in total) and
   on zero calls in the other seventeen. R2's whole value is the LEVEL-0 tier
   at p10/p13. The level-2 reorder still landed — it is a branch swap — but it
   prices at zero, and the p7/p8 A/B rows confirm it.
2. **The framing "the value is dead" was unnecessarily weak.** On the level-0
   branch `tx_unit` can produce the closed form ITSELF from the same inputs
   (`eob`, `w*h`) that the depth loop applies afterwards, so the shipped change
   is an arithmetic identity and needs no deadness argument at all. The one
   place the substituted value IS observed is `txt_search`'s per-tx-type cost
   compare — inert only under `only_dct`, which `coeff_rate_est_lvl == 0`
   implies; `txt_search` now demotes to `Exact` otherwise, structurally.

The falsifier ("a counter showing the discarding branch is rarely taken") half
fired: it is taken on 71-74 % of calls at qp20, 31-56 % at qp40, and **0 %** at
qp55, where `end_depth == 0` puts the frame off the `perform_tx_partitioning`
path entirely. The wall-clock win tracks that share, which is the evidence that
it is the mechanism and not code placement.

**Evidence: SOURCE (both sides) + PROFILED (stage share).** Byte-identity:
**SAFE** (the value is dead).

C's coefficient-rate tiers are an `if / else if / else` — when the closed form
applies, the real estimator is **never called**:

```c
// product_coding_loop.c:4914-4934  (and identically :5540-5564, :5883-5890)
uint64_t th = (txbwidth * txbheight_original) >> 6;
if ((coeff_rate_est_lvl >= 2 || coeff_rate_est_lvl == 0) && (eob_txt[tx_type] < th)) {
    y_txb_coeff_bits_txt[tx_type] = 6000 + eob_txt[tx_type] * 1000;
} else if (coeff_rate_est_lvl == 0) {
    y_txb_coeff_bits_txt[tx_type] = 3000 + eob_txt[tx_type] * 100;
} else {
    svt_aom_txb_estimate_coeff_bits(...);
}
```

The port computes the expensive branch **first**, then discards it:

```rust
// leaf_funnel.rs:2060-2097
let real_bits = if eob > 0 { cost_coeffs_txb(...) } else { cost_skip_txb(...) };
...
let bits = if plane_type == 0 && frame.cfg.coeff_rate_est_lvl == 2 {
    let th = (w * h) >> 6;
    if (eob as usize) < th { 6000 + eob as i32 * 1000 } else { real_bits }
} else { real_bits };
```

and again one level up, at `coeff_rate_est_lvl == 0`:

```rust
// leaf_funnel.rs:6223-6232
let txb_bits = if cfg.coeff_rate_est_lvl == 0 && end_depth > 0 {
    let th = (txw * txh) >> 6;
    if (dec_eob as usize) < th { 6000 + dec_eob as u64 * 1000 } else { 3000 + dec_eob as u64 * 100 }
} else { dec_bits_raw as u64 };
```

Here `dec_bits_raw` **is** `out.bits` from `tx_unit`, i.e. the full
`cost_coeffs_txb` result, and on that branch it is dropped entirely.

`cost_coeffs_txb` (`leaf_funnel.rs:1466-1599`) is not cheap: `txb_init_levels`,
a `vec![0i8; width * height]` at `:1512`, `get_nz_map_contexts` at `:1513`, and
the full per-coefficient walk at `:1573-1593` with a `br_ctx` lookup per
above-base level.

Scope, from `FunnelCfg::for_preset`:

- **presets ≥ 9 (p10 / p13)**: `coeff_rate_est_lvl: 0` (`leaf_funnel.rs:1078`)
  with `txs_on: true` (`:1072`), so `end_depth > 0` and the `:6223` branch is
  live. This is the fast-preset regime where the gap is 3.5×.
- **presets 7–8**: `coeff_rate_est_lvl: 2` (`:1044`), the `:2088` branch.
- **preset 6**: level 1 — C calls the real estimator too. **Not a gap at p6.**

The port's own comment at `:6218` says "The real bits still drove RDOQ/eob
inside `tx_unit` (unchanged)". Worth re-reading carefully: RDOQ consumes the
rate **tables** through `OptimizeCtx` (`leaf_funnel.rs:1907-1925`) and runs at
`:1926`, *before* `real_bits` is computed at `:2060`. `real_bits` is not an
input to RDOQ. So on the discarding branch it is dead, full stop.

Note this idea does **not** contradict the 2026-08-07 allocation null: the win
claimed here is the *entropy work* (`get_nz_map_contexts` + the coefficient
walk), not the `vec![0i8; ...]`. The null result explicitly tested moving
`coeff_contexts` to a stack array / scratch and found nothing; it did not test
**not calling the function at all**.

**What would falsify it:** a counter showing the discarding branch is rarely
taken — i.e. `eob >= th` almost always, so the real cost is almost always used
anyway.

**Cheapest experiment:** two counters (`cost_coeffs_txb` calls; of those, calls
whose result is discarded) behind the same probe feature as R1 — an hour's work
and it settles the whole idea. If the dead fraction is large, a
`COST_COEFFS_DOUBLE` arm prices it. The fix itself is a `want_exact_rate: bool`
parameter and a branch reorder; there is no refactor.

---

### R3. Give the TXT SATD early exit C's ordering (p ≤ 6 only)

**Evidence: SOURCE (both sides).** Byte-identity: **SAFE** (identical skip
decision; only the order of computation changes).

C computes the SATD from the coefficients it has *just* produced, **between**
the forward transform and the quantizer, and `continue`s before doing anything
else:

```c
// product_coding_loop.c:4729-4756
svt_aom_estimate_transform(..., &(((int32_t*)ctx->tx_coeffs->y_buffer)[ctx->txb_1d_offset]), ...);
if (satd_early_exit_th) {
    int satd = svt_aom_satd(&(((int32_t*)ctx->tx_coeffs->y_buffer)[ctx->txb_1d_offset]),
                            (txbwidth * txbheight)) << ctx->mds_subres_step;
    if (satd < best_satd_tx_search) { best_satd_tx_search = satd; }
    else if ((satd - best_satd_tx_search) * 100 > best_satd_tx_search * satd_early_exit_th) { continue; }
}
quantized_dc_txt[tx_type] = svt_aom_quantize_inv_quantize(...);
```

The port runs the **whole** `tx_unit` first, then computes the SATD in a
separate pass, and its own comment says so:

```rust
// leaf_funnel.rs:8875   — full tx_unit: residual, fwd txfm, quantize(+RDOQ),
//                          INVERSE txfm, recon, spatial SSE, cost_coeffs_txb
// leaf_funnel.rs:8933-8935
// SATD early exit between transform and quantize in C; we
// apply it post-hoc on the transform coefficients via a
// dedicated pass only when the th is armed.
```

and that dedicated pass rebuilds the residual **and re-runs the forward
transform**:

```rust
// leaf_funnel.rs:9039-9070
fn txb_coeff_satd(...) -> i64 {
    let mut residual = vec![0i32; n];
    for r in 0..h { for c in 0..w { residual[r*w+c] = src[..] as i32 - pred[..] as i32; } }
    let mut coeffs = vec![0i32; n];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(&residual, &mut coeffs, w, ...);
    ...
}
```

Per non-DCT TX type that survives the rate pre-screen, the difference is:

| | C | port |
|---|---|---|
| always | 1× fwd txfm, 1× SATD | 1× fwd txfm **+ a 2nd fwd txfm** and a 2nd residual build (scalar loop, not the NEON `residual_i32`), 2 more heap Vecs |
| when the gate fires | nothing more | quantize (+RDOQ), inverse txfm, recon, spatial SSE, `cost_coeffs_txb`, 3 heap Vecs — all discarded |

The second forward transform is pure waste **even when the gate does not fire**,
because C reads the SATD out of the transform buffer it already filled.

**Scope, honestly:** `satd_th` is 0 unless `txt_on` and `txt_satd_th != 0`
(`leaf_funnel.rs:8782-8828`). `txt_on` is `false` at presets ≥ 9
(`leaf_funnel.rs:1080`), so **this is a p ≤ 6/8 item and contributes nothing at
p10 / p13**. That is why it ranks below R1 and R2 despite being the largest
per-unit ratio in the review.

**What would falsify it:** the SATD gate almost never fires *and* the non-DCT
TX-type population is small — in which case the doubled forward transform is a
rounding error.

**Cheapest experiment:** count, per frame at p6, (a) TX types entered, (b) TX
types skipped by the SATD gate, (c) `txb_coeff_satd` calls. Then a
`TXB_SATD_DOUBLE` arm prices the redundant forward transform alone. Restructuring
is a real refactor (the SATD has to read `TxScratch::coeffs` before the
quantizer runs, which means splitting `tx_unit_inner` at the quantize
boundary) — so price it before building it.

---

### R4. The three per-call allocations in `tx_unit_inner` — 11.18 % of malloc samples, and outside the `BlockDecision` work

**Evidence: PROFILED (named parent) + SOURCE.** Byte-identity: **SAFE**.

I first wrote this item as "recorded but don't bet on it", on the strength of
`alloc_traffic_null_2026-08-07.meta`. **The 2026-08-13 attribution changes that
and I am promoting it.** `leaf_funnel::tx_unit_inner` is the *second* largest
nearest-app-ancestor parent of malloc-family self samples at **11.18 %**, behind
only `evaluate_leaf` — and the 2026-08-07 null tested `hadamard_satd` and
`coeff_contexts`, which are different call sites. The null does not cover this.

`tx_unit_inner` allocates three `Vec`s on every call
(`leaf_funnel.rs:1893`, `:1894`, `:1957`) on top of the five buffers already
hoisted into `TxScratch` (`:1673-1702`):

| buffer | line | escapes? | disposition |
|---|---|---|---|
| `qcoeff` | `:1893` | **yes** (`TxUnitOut.qcoeff`) | needs a pool or a caller-supplied slice |
| `dqcoeff` | `:1894` | **no** — only read by `sse_i32(&packed, &dqcoeff)` at `:2051` and the recon fill at `:1964` | **can move into `TxScratch` today**, no newtype needed |
| `recon` | `:1957` | **yes** (`TxUnitOut.recon`) | R1 removes it entirely at the three discarding sites |

The old note in `alloc_traffic_null_2026-08-07.meta` — "a recycling pool would
need a `PooledVec` newtype and a 5-site refactor … neither is likely to pay" —
is right about `qcoeff` and wrong about `dqcoeff`, which is purely local and
costs one line. Do `dqcoeff` first; it is the cheapest test of whether this
parent converts to wall clock at all.

**Coupling with R1:** at the MDS1 / CfL / non-CfL-chroma sites both `recon` and
`qcoeff` are dead, so R1 deletes two of the three allocations there for free.
Land R1 first and re-take the attribution before deciding about `qcoeff` pooling.

Same shape elsewhere on the hot path, now with a measured parent:
`partition::extract_neighbors_tiled` is **6.10 %** of malloc samples and returns
two fresh `Vec<u8>` per intra prediction (`partition.rs:235-244`) where C slices
one persistent per-plane allocation (`md_process.c:426-473`).
`drop_glue::<leaf_funnel::Cand>` is 6.60 % — that is the per-candidate
`pred: Vec<u8>` / `pred10: Vec<u16>` / `txb_q: Vec<Vec<i32>>` in the `Cand`
struct (`leaf_funnel.rs:2995-3025`). Also unpooled: `md_cfl_rd_pick_alpha`'s
`cfl_pred` per alpha (`:2863`) and `predict_unit`'s `above_c` per filter-intra
call (`:1286`).

C's contrast is structural and worth stating once: `ModeDecisionContext`
allocates **zero bytes per block** (`md_process.c:214-682`), residual and recon
are a **single shared buffer for every candidate** (`mode_decision.c:649-653`,
with the comment at `md_process.c:614-615` saying so in as many words), the
candidate buffers come from one pooled allocation per buffer *kind*
(`md_process.c:620-679`), and even the coefficient-cost level buffer is a
context field zeroed once at construction rather than a stack array
(`md_process.h:1301-1302`, `md_process.c:234-236`).

**Ceiling, stated honestly:** per §1b the entire allocation family is bounded at
1.24×. `tx_unit_inner`'s 11.18 % share of it is a fraction of that. This is a
real item with a named parent, not a lever.

---

### R5. Intra prediction is 1-of-18 vectorised

**Evidence: SOURCE (census) + PROFILED (small).** Byte-identity: **SAFE**
(exact integer kernels, `c_parity` gate pattern already exists).

`crates/svtav1-dsp/src/intra_pred.rs` contains exactly **one** `incant!`, at
line 163, for PAETH (`predict_paeth_impl_neon`, `:199`). DC / DC_LEFT / DC_TOP /
DC_128 (`:18`), V (`:54`), H (`:61`), SMOOTH (`:75`), SMOOTH_V (`:106`),
SMOOTH_H (`:129`), the three directional predictors (`:362`, `:391`, `:425` and
the edged variants `:679`, `:722`, `:767`), filter-intra (`:989`), both edge
helpers (`:627`, `:658`) and all three CfL kernels (`:1089`, `:1111`, `:1138`)
are scalar. C ships `ASM_NEON/intra_prediction_neon.c` (~3,100 lines) covering
DC×4 / V / H / SMOOTH×3 / PAETH across all 22 block sizes, plus
`svt_av1_dr_prediction_z{1,2,3}_neon`, `svt_av1_filter_intra_predictor_neon`,
`svt_av1_filter_intra_edge_neon`, `svt_av1_upsample_intra_edge_neon` and
`svt_aom_cfl_predict_lbd_neon` (`cfl_neon.c`).

**Ceiling is small and I am saying so**: INTRA PRED is 3.8 % of p10 self time
and 2.7 % of p6 (`docs/perf-status.md`). This is a grind item, not a lever.
Pick the modes the profile actually shows (the port funnels to DC / V / H /
SMOOTH* / PAETH at `leaf_funnel.rs:1300-1309`, and PAETH is already done).

**Falsifier / experiment:** a per-mode call census before writing any kernel —
this is the `itx_shape_census` lesson from `rav1d-safe/docs/AGENT_BRIEF.md` §2
("prove the code runs, and prove it runs *often*, before optimising the symbol
it hides behind"). One counter array indexed by mode, one frame, done.

---

### R6. The p6-only kernels C vectorises and the port does not

**Evidence: SOURCE (census) + PROFILED (stage totals only).** Byte-identity:
**SAFE**.

At p6 the port carries LOOP RESTORATION 12.5 % and CDEF 9.3 % of self time. The
dominant LR piece (`compute_stats`) is already a NEON dot-product kernel. What
is still scalar in those stages:

| port function | file:line | C counterpart |
|---|---|---|
| `wiener_convolve_add_src` (LR **apply**) | `svtav1-dsp/src/restoration.rs:138` | `svt_av1_wiener_convolve_add_src_neon` (`ASM_NEON/convolve_neon.c`) |
| `compute_cdef_dist_8bit` (CDEF **search**) | `svtav1-dsp/src/cdef.rs:1207` | `svt_aom_compute_cdef_dist_8bit_neon` + a `_neon_dotprod` variant |
| `compute_cul_level` | `svtav1-encoder/src/leaf_funnel.rs:1705` | `svt_av1_compute_cul_level_neon` |
| all deblock kernels (`lpf_*`) | `svtav1-dsp/src/loop_filter.rs:307-405` (zero NEON in the file) | `svt_aom_lpf_{horizontal,vertical}_{4,6,8,14}_neon` |

**Do not budget any of these from the stage totals.** The 12.5 % and 9.3 % are
whole-stage numbers that include already-vectorised work; the residual belonging
to these specific functions is **not measured**. The deblock line in particular
is largely moot at p10/p13 now that the post-filter apply is skipped
(`benchmarks/perf_postfilter_2026-08-11.meta`).

**Cheapest experiment:** re-read the existing p6 self-time profile at function
granularity (the data is already in the repo — no new run needed) and take the
top named leaf. Only then port one kernel.

---

### R7. No `dotprod` / `i8mm` tier anywhere

**Evidence: SOURCE.** Byte-identity: **SAFE** if the kernel is exact-integer.
Value: **not measured, SPECULATIVE.**

All 36 `incant!` sites across `svtav1-dsp` + `svtav1-entropy` are
`[v3, neon, scalar]`. C ships `ASM_NEON_DOTPROD/`, `ASM_NEON_I8MM/`, `ASM_SVE/`
and `ASM_SVE2/` trees. The M4 Pro has dotprod and i8mm (it does **not** have
SVE, so C's `_sve` kernels are irrelevant on this box — do not cite them as a
gap here).

Open question I did not resolve: whether archmage exposes a dotprod/i8mm token
at all. That is the first thing to check; if it does not, this idea is a
different project.

**Cheapest experiment:** one kernel where the dot-product shape is obvious
(`sad_impl_neon`, or `compute_cdef_dist_8bit` if R6 puts it on the list), one
dotprod arm, `kernel_tiers` for the ratio and **one** `tools/perf_ab.sh`
encoder-level A/B for the decision — per `benchmarks/kernel_tiers_neon_2026-08-07.md`'s
own method note, the microbench cannot settle an encoder-level question.

---

### R8. Hygiene: three things that make the next audit cost more than it should

**Evidence: SOURCE.** No perf value claimed. Byte-identity: **SAFE** (deletions
and comments).

- `copy.rs:53` `block_copy_impl_neon` has **no vector content** — its body is
  byte-identical to `_impl_scalar` and `_impl_v3`. A naming artefact.
- `restoration.rs:749` `mac_row_i32_neon` is real NEON but **dead on aarch64**
  since the dot-product rewrite; only the `_v3` twin (`:771`) still has callers.
- Seven `*_impl_neon` transform wrappers (`fwd_txfm.rs:2142, 2181, 2220, 2259`;
  `inv_txfm.rs:2089, 2128, 2167`) have bodies byte-identical to their `_impl_scalar`
  siblings. Behaviour is fine (they route one level down into a real NEON arm)
  but the names mislead — this is the §B1 pattern from the 2026-08-07 audit,
  still present.
- `txfm_simd.rs:30-32` still says *"Only the AVX2 (`v3`) arm is vectorized; the
  `neon`/`scalar` arms report 'not handled'"*. That is false at HEAD (see §3.7).
  A stale doc that contradicts the code is exactly the failure mode
  `~/.claude/CLAUDE.md`'s "DOCS: SEARCH + UPDATE" rule exists to prevent.

---

### R9. `evaluate_leaf` is one 4,352-line function

**Evidence: SPECULATIVE.** Byte-identity: **SAFE** in principle.

`leaf_funnel.rs:3589-7934` is a single function of 4,352 lines containing 62
`vec![...]` sites. The workspace has **zero** uses of `#[inline(never)]` and
does not use the fixed-size-array (`try_into::<&[T; N]>()`) bounds-check
elimination pattern anywhere, both of which the global performance guidance
calls for in hot loops. It is plausible that register pressure, spill traffic
and I-cache/BTB behaviour in a function this size cost something.

**I have no evidence for this and it may well be worth nothing.** Two reasons to
be careful before chasing it:

1. `alloc_traffic_null_2026-08-07.meta` measured the stack-array form of exactly
   this instinct as **worse** (0.983× at 128² p6) — a 6 KB zero-fill per call
   cost more than the malloc it replaced.
2. Per `rav1d-safe/docs/AGENT_BRIEF.md` §2, any t=1 A/B on a large-working-set
   cell is drawn from a **±1.4 % code-placement lottery** unless both arms are
   built with `-C llvm-args=-align-all-functions=4`.

**The control this item used to ask for now exists** (§1b): arms are built with
the alignment flag and the byte-identical-binary identity band is
**[0.9843, 1.0190]**. So the bar is set — anything inside [0.984, 1.019] is
null. Given that a whole-function restructure of `evaluate_leaf` is a large,
risky change and the plausible effect size sits close to that band, this stays
last. If anyone does try it, the honest first step is a *measurement*, not a
refactor: build one arm with `#[inline(never)]` on the three or four largest
inlined helpers inside `evaluate_leaf` and see whether it moves outside the band
at all.

---

## 5. Which ideas could change the bytes

Called out explicitly, as instructed:

- **R1 is the only one with real byte risk**, and it is a *proof obligation*,
  not an inherent risk: skipping the inverse transform is byte-inert exactly
  where the reconstruction is unread. If the flag is derived from
  `spatial_dist` instead of from an explicit `need_recon`, a future caller with
  `tx_depth > 0` silently loses its depth-1 neighbour prediction and the bytes
  move. Gate it with an explicit parameter and the full identity matrix.
- **R2, R3, R4, R5, R6, R7, R8** are byte-inert by construction: dead-value
  elimination, computation reordering with an identical decision, exact-integer
  kernel swaps, and deletions.
- **R9** is byte-inert but its *measurement* is the fragile part.
- Nothing in this review proposes changing a decision. Anything that did would
  have to be argued on RD, not smuggled in as perf.

---

## 6. Issue #15 — a narrowing, not a diagnosis

Partial-superblock RD divergence, 67 of 648 real-content cells, every one with
an aligned extent not a multiple of 64. The `sse_i32` overflow hypothesis is
refuted (`benchmarks/sse_i32_width_2026-08-11.meta`: 0 wraps in 59,088,480
elements). This section adds a search-space bound and one lead. **I did not root
cause it.**

**The bound.** I enumerated every use of `aligned_width` / `aligned_height` in
C's mode-decision translation units (`product_coding_loop.c`, `full_loop.c`,
`md_process.c`, `coding_loop.c`). On the bd8 still all-intra path there are
exactly **three** alignment-dependent quantities:

1. `cropped_tx_width` / `cropped_tx_height`, luma — `product_coding_loop.c:4664-4665`
   (`tx_type_search`) and **a second, differently-written site** at `:5752-5754`
   (`perform_dct_dct_tx`), which casts `(uint8_t)tx_width` and applies
   `>> ctx->mds_subres_step` to the height. Port: `frame_geom.rs:264-282`.
2. The chroma twin — `full_loop.c:2229-2232`, which subtracts in the **chroma**
   domain from a `ROUND_UV`-anchored origin, not from luma coordinates. Port:
   `frame_geom.rs:310-321`.
3. The `end_tx_depth` frame-boundary rule — `product_coding_loop.c:6712-6713`:
   `if (blk_org_x + blk_geom->bwidth <= aligned_width && blk_org_y + blk_geom->bheight <= aligned_height)`.

Everything else keyed on `aligned_*` in those files is TPL (inter) or hbd
packing. So the divergence is in one of those three, in something the port
computes that C does not, or in a decision path that reads a *derived* extent.

**The lead.** The port has more than one definition of the frame extent, and
`frame_geom.rs` says so itself:

```
// frame_geom.rs:11-15
//! [`sb_geom`] and the `mi_*`/`sb_*` accessors are still unwired — the pipeline
//! re-derives those inline; route them through here as the remaining chunk-2
//! work lands, so the frame extent has ONE definition.
```

An inline re-derivation that agrees with the canonical one at 64-aligned extents
and disagrees at partial superblocks is precisely the shape of a bug that
appears only on the #15 axis. Worth an hour: diff every inline SB/mi extent
computation in `pipeline.rs` against `frame_geom`'s and see whether any of them
uses the SB-grid extent (`ceil(aligned/sb)*sb`) where C uses `aligned`, or vice
versa. Note also that C maintains **two** `sb_geom` arrays built from different
bases — `scs->sb_geom` from `max_input_luma_width/height`
(`resource_coordination_process.c:1061`) and, under resize only,
`pcs->sb_geom` from `aligned_width/height` (`resize.c:1487`) — with
`sb_geom->width = MIN(width - org_x, sb_size)` (`pcs.c:1550-1551`). The two
agree whenever true == aligned, which is the case for the 96×88 reproducer, so
this is not itself the bug — but it is a reminder that "the extent" is three
different numbers in C too, and the port must pick the same one per consumer.

**The history worth re-checking.** `rust/CLAUDE.md` records that C's
`end_tx_depth` frame-boundary rule (item 3 above) was ported, judged
unreachable, **reverted**, documented as "MEASURED UNREACHABLE", and then
re-measured as **live at preset 7** within the hour. The #15 reproducer in the
census is `terminal 96x88 p4 qp33`. A boundary rule that was once wrongly
declared dead, on the one axis where the port diverges, is worth confirming is
present *and firing* at p4 before looking anywhere else. Per that same CLAUDE.md
entry: use a script file with a per-cell print, not an inline shell loop — a
zero from a probe that never ran is indistinguishable from a real zero.

---

## 7. Honest summary of the ranking

The allocation/data-layout work in flight is the largest single named lever
**and it is measurably not enough**: §1b puts its arithmetic ceiling at 1.24×
against a 3.53× p10 gap, leaving ~2.85× even if it were driven to zero. So the
question "what else is there?" is not optional, and the answer this review
found is: **three specific pieces of computed-and-discarded work, then a long
tail.**

R1 and R2 are the only ideas here that are both algorithmic and present at the
fast presets where the 3.5× lives. R3 has the largest per-unit ratio in the
review — C does one forward transform where the port does two plus a full
quantize/inverse/rate chain — but it is confined to p ≤ 6/8 and needs a real
refactor. R4 now has a named 11.18 % malloc parent and one line of it
(`dqcoeff`) is free. Everything from R5 down is a grind against stage shares in
the single digits, and I would not start there.

There is no second 1.35× sitting in the open the way the post-filter skip was.
What makes R1–R3 worth doing first is not their size — which I have **not**
measured — but that each is settled by a call census plus a doubling arm that
together cost well under a day, and R1 and R2 are a parameter and a branch
reorder rather than a restructure. Price them, and if the censuses come back
small, say so in this file and go do the SIMD grind instead.

**The one framing worth carrying forward** is §2: byte-identity forces every
decision-affecting prune to be ported, so the port cannot be losing because C
prunes harder. It can only be losing on work whose result is thrown away, or on
per-unit efficiency. That is a small enough space to search exhaustively, and
this review searched maybe two thirds of it — the parts I did not search are
listed in §0.
