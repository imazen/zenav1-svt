> **Re-ported onto v4.2.0-FINAL in the port monorepo (branch `hdr-hybrid`, 2026-07-16).**
> This document was written for the standalone rc-based hybrid repo
> (`/root/svt-av1-hdr-on-4.2`); the C-side changes now live in THIS repo's `Source/`
> tree on top of the final v4.2.0 baseline. Differences from the rc-based hybrid:
> the deleted-by-macro-cleanup fork paths (TUNE_CHROMA_SSIM / TUNE_CQP_CHROMA_SSIM
> flips, VLPD0 dead inter branch) are expressed directly as `#if SVT_HDR_MODE`
> blocks. Verified: MODE0 == stock v4.2.0-final 108/108 cells incl single+multi
> tile-group paths; MODE1 byte-matches the rc-hybrid's mode1 on all-intra; the
> Rust differential suite is 669/669 green vs the MODE0 lib.

# svt-av1-hdr (Chromedome) rebased onto mainline v4.2.0-rc

Repo: `/root/svt-av1-hdr-on-4.2`, branch `hdr-on-4.2`, base `v4.2.0-rc` (`0da2ed9`).
Built 2026-07-15. Companion diff reference: `/root/svtav1-diff-reference/REFERENCE.md`.
Purpose: produce a **hybrid C oracle** (`v4.2 + fork features`) so the Rust port
(`svtav1-rs`) can differentially test HDR-fork features under its bit-identity mandate.

## TL;DR — the headline result

The rebase **works and builds**, but the goal of "**fork features off ⇒ byte-identical to
mainline v4.2**" is **NOT achievable as-is**, and this is a property of the fork, not of the
merge. **Measured: 0 / 36 configs byte-identical** vs stock v4.2.0-rc (presets 2/6/8/10 ×
crf 20/40/60 × {default, `--tune 0`, `--pred-struct 1`}), with every fork feature defaulted
to neutral.

**Root cause: the fork makes unconditional, un-flag-gated changes to mainline code paths.**
It is not an additive/opt-in feature layer; it is a divergent branch. Two proven examples:

1. **`deblocking_filter.c`** — the fork **deletes the guard**
   `if ((cdef_search_ctrls.enabled && !use_qp_strength && !use_reference_cdef_fs) ||
   enable_restoration || is_ref || recon_enabled)` and applies the loop filter
   **unconditionally** (fork commit `3f3568a` "Unconditionally apply loop filter"). This
   changes reconstruction on frames where mainline skips the filter → everything downstream
   diverges. No config flag controls it.
2. **The variance pipeline is retyped `uint16_t` → `double`.** `PictureParentControlSet::variance`
   is `uint16_t** variance` in **both v4.1 and v4.2**, and `double** variance` in the fork.
   This propagates through `rc_aq.c`, `segmentation.c`, `pic_analysis_process.c`,
   `mode_decision.c`, `product_coding_loop.c`, `enc_dec_process.c`, `pcs.h` — including
   integer→float semantic changes such as `(a + b) >> 1` → `(a + b) / 2` in
   `get_variance_for_cu()`. Every variance-driven decision (AQ, segmentation, mds0) shifts,
   unconditionally.

Other un-gated fork changes (0 references to any fork config flag in their diffs):
`pic_analysis_process.c`, `enc_dec_process.c`, `mode_decision.c`, plus retuned variance-boost
curve constants in `rc_aq.c` (e.g. octile boost @var 64: mainline 1.481 → fork 1.330).

### Consequence for the Rust port (`svtav1-rs`)

You cannot have a single Rust codebase that is simultaneously bit-identical to **mainline
v4.2** *and* carries the fork's features, because the fork itself does not gate its changes.
The options are:

| Option | Meaning | Cost |
|---|---|---|
| **A. Stay mainline-exact** (recommended) | Port fork features later, each behind an explicit flag *we* add | The Rust encoder then matches mainline C exactly with flags off, but **no C oracle exists** for our flagged variants (the fork doesn't have the flags) — differential testing covers only the mainline path |
| **B. Track the fork instead** | Retarget the port's oracle to the fork | Loses mainline v4.2 parity entirely (fork is v4.1-based, un-rebased); the whole v4.2 identity campaign would be invalidated |
| **C. Flag-gate the fork in C first** | Extend *this* repo so every fork change is switchable, then build the hybrid oracle | Real work (deblock guard, `double` variance pipeline behind a typedef/flag, curve constants), but yields an oracle that is byte-identical to v4.2 with flags off AND exercises fork features with flags on |

**Recommendation: A now, C when HDR features are actually scheduled.** The `double`-variance
retype is the expensive part of C — it is a pervasive representation change, not a branch.

## What was rebased, and how

**Range:** the *pure* fork delta `c04f9515 (mainline v4.1.0) .. 8d13912d (hdr-v4.1.0
"Chromedome")` — 41 files under `Source/`, 5109 diff lines.
**Explicitly NOT** `v4.1.0..hdr/main`: that range also contains the fork's cherry-picks of
mainline 4.2-cycle work (`initial_display_delay` = MR !2682, the EC/CABAC refactor, Neon
SSIM/SAD/qm kernels), which would double-apply against a v4.2 tree.

**Why not merge the fork wholesale:** `hdr/main` reports `SVT_AV1_VERSION_MINOR 1`;
`git merge-base --is-ancestor v4.2.0-rc hdr/main` = **NO**; merge-base(`hdr/main`,
`v4.2.0-rc`) = `c04f9515` = mainline v4.1.0. A merge would regress the C baseline off v4.2.

**Apply result:** 28/41 files clean, 11 conflicts.

### Per-conflict decisions (policy: mainline semantics win; fork features additive/opt-in)

| File | Collision | Decision |
|---|---|---|
| `rc_aq.c` | fork rewrote variance-boost; v4.2 rewrote cyclic-refresh (327 lines) — same file, different functions | **Splice.** v4.1≡v4.2 for lines 1–279 (verified byte-identical), so fork[1..329] (variance boost, incl. **PQ curve 3**) + v4.2[280..end] (cyclic refresh). Fork's only sub-boundary change (`svt_av1_variance_adjust_qp(pcs, true)`) carried over. |
| `definitions.h` | **Both claimed `= 5`**: v4.2 `TUNE_VMAF`, fork `TUNE_FILM_GRAIN` | Mainline owns 5 (`FTR_TUNE_VMAF=1`, wired through CLI + SIMD kernels). Fork's renumbered → **`TUNE_FILM_GRAIN = 6`**. ⚠️ `--tune 5` means VMAF here, **not** film grain as in the fork. |
| `pcs.h`, `EbSvtAv1Enc.h` | Both added `hbd_mds` (v4.2 `int` via MR !2644; fork `uint8_t`) | Keep mainline's `int hbd_mds`; drop fork duplicate. ABI `padding[]` recomputed for both field sets (fork's 10 uint8 → 9). |
| `full_loop.c` | v4.2 coeff-shaving (271 lines) vs fork `svt_av1_perform_noise_normalization` (118 lines) — both additive, same location | **Union — kept both.** |
| `product_coding_loop.c` | v4.2 `CLN_RENAME_PD0` + unconditional VAR fast-cost; fork gates on `mds0_dist_type` + spatial-distortion facade w/ `ac_bias`/`tx_bias` | v4.2's renamed table + fork's VAR/else branching. (Note: `svt_spatial_full_distortion_kernel_facade` is **fork-invented** — absent from both v4.1 and v4.2.) |
| `enc_mode_config.c` | 25 hunks, ~all fork preset re-banding vs v4.2's rewritten ladder (+8688 lines) | **v4.2 ladder kept for all 25.** Fork's preset re-tuning would change output for every `--preset N`. Only fork feature wired here — `complex_hvs → mds0_level = 3` — re-injected into `sig_deriv_mode_decision_config_default` (matching the fork's own site). |
| `enc_settings.c` | 7 hunks: preset range, tune validation, param validation, **defaults**, info print, token tables | Mainline `MIN_ENC_PRESET` (fork research presets **−2/−3 dropped**); tune bound widened to accept 6; validation unioned; **defaults neutralized** (below); token tables unioned. |
| `enc_handle.c`, `app_config.c`, `app_main.c` | Both added fields/tokens/args | Unions with dedup; fork's `NO_COLOR` `color` arg kept (already threaded cleanly elsewhere). |

### Fork defaults neutralized (intent: mainline behavior unless opted in)

| Field | Fork default | Set here | Note |
|---|---|---|---|
| `ac_bias` | 1.0 | **0.0** | mainline v4.2 default |
| `noise_norm_strength` | 1 | **0** | off (code early-returns < 1) |
| `kf_tf_strength` | 1 | **3** | fork docs mark 3 = mainline |
| `alt_lambda_factors` | 1 | **0** | regular lambda |
| `sharp_tx` | 1 | **0** | off |
| `qp_scale_compress_strength` | 1.0 | **0.0** | fork docs mark 0.0 = mainline |
| `hbd_mds` | 0 | **DEFAULT (−1)** | mainline auto |
| `cdef_scaling` | 15 | 15 | 1× = neutral |
| `noise_adaptive_filtering` | 2 | 2 | "default tune behavior" |
| `tx_bias`, `complex_hvs`, `alt_ssim_tuning`, `noise_strength`, `noise_chroma_from_luma` | 0/false | same | already neutral |
| `noise_strength_chroma`, `noise_size` | −1 | −1 | auto |
| default preset | 4 (fork) | **12 (mainline)** | fork default-change dropped |

These make the *config* mainline-intent — but per the TL;DR they do **not** restore byte
parity, because the divergences are un-gated.

## Verification performed

- **Builds clean** (`cmake -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF`, gcc, `-j8`).
- **Both feature sets present in the CLI**: `--variance-boost-curve [0-3]` (3 = PQ),
  `--tune [0-6]` (5 = VMAF mainline, 6 = Film Grain fork), `--hbd-mds`, `--enable-intrabc`,
  `--enable-kf-tf`, `--noise`, `--noise-chroma`, `--noise-size`, `--qp-scale-compress-strength`.
- **Fork params function**: `--cdef-scaling 8` and `--complex-hvs 1` parse, encode, and change
  output size (473 vs 468 B on the smoke clip).
- **Parity matrix**: 36 configs vs stock v4.2.0-rc (`/root/svt-av1-stock-4.2`, same flags,
  same build) → **0 identical**. This is the documented, expected-after-diagnosis result.

## Known gaps in this rebase (honest list)

1. **9 fork params are registered and functional but missing from `--help`**
   (`--noise-norm-strength`, `--kf-tf-strength`, `--alt-lambda-factors`, `--sharp-tx`,
   `--alt-ssim-tuning`, `--tx-bias`, `--complex-hvs`, `--noise-adaptive-filtering`,
   `--cdef-scaling`). Cosmetic: my `app_config.c` help-table union dropped their description
   rows. They parse and take effect.
2. **Post-Chromedome fork work is NOT included** (this rebase is Chromedome-only). Notably
   `80b48b9` "Allow complex-hvs for all-intra mode" — **relevant to our AVIF/all-intra target**
   (here `complex_hvs` is wired only into `sig_deriv_mode_decision_config_default`, not
   `_allintra`). Also `4889de3` noise-chroma auto strength, `ce5178a` MDS0 ac-bias dampening,
   `981fe12` sharpness default, `5caa3e3` LPD1 skip-inter-tx. A follow-up pass should
   cherry-pick fork-only post-Chromedome commits while **skipping** its mainline-4.2
   cherry-picks (which we already have).
3. **Fork research presets −2/−3 dropped**; fork default preset 4 dropped (mainline 12).
4. **`--tune` numbering deliberately diverges from the fork** (5 = VMAF, 6 = Film Grain).
5. Fork's preset re-tuning in `enc_mode_config.c` dropped wholesale — if fork *tuning* (not
   just features) is ever wanted, that is a separate, large decision.

## Reproduce

```bash
git -C /root/svt-av1-hdr-on-4.2 log --oneline -1        # fdea1cd
cmake -S /root/svt-av1-hdr-on-4.2 -B cbuild -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF
cmake --build cbuild -j8
# stock reference:
git -C /root/svt-av1-hdr-on-4.2 worktree add /root/svt-av1-stock-4.2 v4.2.0-rc
```

# Rust-side port status (branch `hdr-hybrid`)

The Rust mirror of the switch is `svtav1_encoder::hdr_mode::{SvtHdrMode, HdrForkConfig}`
(RUNTIME config; `EncodePipeline.hdr`, default Mainline). Byte-identity targets:
Mainline mode → stock v4.2.0-final; HdrFork mode → the C hybrid's MODE1 lib
(build with `-DSVT_HDR_MODE=ON -DSVT_AV1_LTO=OFF`, point `SVT_CREF_LIB_DIR` at it).

| Fork behavior | Rust status | Witness |
|---|---|---|
| Config surface + per-mode defaults | **DONE** (`hdr_mode.rs`) | unit pins vs enc_settings.c branches |
| light-RDOQ (low-DC chroma, encode pass) | **DONE** (`quant.rs light_rdoq_low_dc_chroma`) | fires only in fork mode; formula per C |
| RDOQ rweight/rshift incl. sharpness | **DONE** (`rdoq_rdmult_sharp`/`rdoq_rdmult_full`) | sharp-tx rweight=0 path live via `sharp_tx_active` |
| **sharp-tx** (rweight=0 + eob-shortening disable) | **DONE + ACTIVE** — `sharpness_flag` gates the two trellis eob-shorten sites (`quant.rs`, C full_loop.c:822/955) and rweight=0 in the rdmult (C :1070-1078, unconditional for sharp_tx=1 given delta_q_present + luma); wired through FunnelFrame AND the c_quant still path | `hdr_fork_e2e.rs sharp_tx_is_live_in_fork_mode` (streams differ + never smaller with sharp_tx=1); decode gate 24/24 (presets 2/4/6/8 x both modes x qp 20/40/55) |
| loop_filter_sharpness (fork default 1) | **DONE** — search trials + application + FH bits agree | suite green in mainline mode (sharpness 0 = prior bytes) |
| variance-boost math, curves 0–3 + PQ dark attenuation | **DONE** (`var_boost.rs`) | helpers C-parity-tested vs the linked lib (c_parity_var_boost.rs); curve table unit-pinned |
| **per-SB delta-q** | **L1+L2+L3a DONE** — symbol writer (a0cb40279), variance producer + boost plan w/ C-parity (6cd720c8b), FH delta_q_params + per-SB delta symbols LIVE with uniform plan (53969e329; decode-gate-found the placeholder delta_q_cdf default, fixed to AOM_CDF4(28160,32120,32677)). **L3b DONE** (6a387d500): variance plan + per-SB quant/lambda threading live; sharp_tx activated on top | decode gate 6/6 with syntax live; producers parity-tested vs exported C |
| **QM** (fork default ON, luma 6..10 / chroma 8..15) | **DONE + ACTIVE** (see the detailed row below) | c_parity_qm 13,680 cells; decode gate |
| fork chroma-qindex path (boosts, Cb +12, separate_uv_delta_q/diff_uv_delta, per-plane dequant) | **DONE + ACTIVE** (d0c308e1b): per-plane qindexes threaded through FunnelFrame/ChromaPass + all 10 funnel chroma tx sites incl CfL; SH bit + FH deltas signaled in fork mode from the SAME derivation the quantizer uses | mainline 701/701 unchanged; **aomdec decode gate 6/6** (examples/hdr_fork_smoke.rs, both modes x qp 20/40/55, recon byte-exact) |
| ac_bias/tx_bias distortion facade | **DONE + WIRED** (see the detailed rows below) | liveness witnesses + decode gate |
| **QM (quant matrices, fork default ON — luma 6..10, chroma 8..15)** | **DONE + ACTIVE** — tables transcribed (qm_tables.rs via xtask/transcribe_qm.py), level derivation (linear aom_get_qmlevel for the fork's default tune=PSNR; still-image polynomial ported for TUNE_IQ use), FH using_qmatrix + qm_y/u/v syntax (qm_v gated on separate_uv_delta_q), QM quantize_b/fp kernels + trellis get_dqv iwt + noise-norm dqv routed through EVERY quantize path (tx_unit, light psq, still/PD0, chroma DC) with per-plane levels on QuantTable/CodingQuantCfg | c_parity_qm.rs: 13,680 cells vs exported svt_aom_quantize_b_c / svt_av1_quantize_fp_qm_c fed the transcribed tables; `hdr_fork_e2e.rs qm_is_live_in_fork_mode`; decode gate 24/24 (aomdec dequantizes with the signaled matrices — recon byte-exact) |
| **photon-noise synthesis (`--noise*`, fork knob, default 0)** | **DONE + ACTIVE** — `noise_gen.rs` ports noise_generation.c verbatim (14 AR coeff tables, grain-size resolution ramp, studio/full-range scaling ramps, incl. the C int-truncation in get_output_noise and the cb[1][1] never-assigned quirk); SH film_grain_params_present + FH film_grain_params writer (spec 5.9.30, KEY form); seed 7391+3381/frame | c_parity_noise_gen.rs: 1,440 cells vs the exported svt_av1_generate_noise_table (real-config shim); `hdr_fork_e2e.rs photon_noise_is_live`; **grain gate 3/3** (examples/noise_gate.rs: aomdec --skip-film-grain == recon AND plain decode != recon — the decoder synthesizes grain from our table) |
| noise-norm AC boost (fork default 1) | **DONE + ACTIVE** — `noise_norm.rs` kernel applied in `tx_unit` (leaf funnel) after quantize/RDOQ, before recon, so dist/recon/coded levels stay consistent (C runs it in the encode pass on the winner, full_loop.c:2017; this single-pass port applies it at MD quantization — fork mode carries no byte-vs-C gate); also in `quantize_inv_quantize_still` + fork knobs now stamped onto `c_quant` | c_parity_noise_norm.rs: 7,200 randomized cells vs the exported C fn; `hdr_fork_e2e.rs noise_norm_is_live_in_fork_mode`; decode gate 24/24 |
| ac_bias psy kernels (mainline v4.2 feature; fork default 1.0) | **DONE + WIRED** — psy_full_dist added at the funnel spatial-dist site (4fc367b85) | c_parity_ac_bias.rs vs exported C (both RTCD tables must be inited — common for hadamard, aom_dsp for satd) |
| fork mds0 tx-bias facade | **DONE + WIRED** — `facade_bias` applied at the funnel spatial-dist site (tx_unit, before the psy add — the C facade IS the SSE producer at product_coding_loop.c:5990/6018 + the chroma full-loop sites; luma/chroma class index sets are identical). Non-default knob (fork tx_bias=0); with the fork's ac_bias=1.0 only the tx-size scales fire (C gates the class biases on ac_bias==0). C's remaining facade sites are light-PD0/LPD1 fast loops the port does not implement (no lpd1 path) — psy_adjust_rate_light stays a kernel-only port until an LPD1 path exists | c_parity_tx_bias.rs 2,160 cells; `hdr_fork_e2e.rs tx_bias_is_live_in_fork_mode` (ac_bias=0 isolation, flips bytes); decode gate 12/12 |
| photon-noise synthesis (`--noise*`) | **DONE + ACTIVE** (see the detailed row below) | grain gate 3/3 |
| kf_tf_strength / TF formula | OPEN — needs TF (all-intra immune; wave2 owns TF parity) | |
| **alt_lambda_factors (fork default ON)** | **DONE + ACTIVE** — KF frame-type lambda factor 140 (rd_frame_type_factor_alt) vs mainline 150, plus the per-SB delta-q qdiff stats factor {<=-8:90,<0:115,<=8:135,>8:150} (rc_process.c:437) that activates once delta_q_present (kf_full_lambda_8bit_ex, pd0.rs) | `hdr_fork_e2e.rs alt_lambda_factors_is_live`; decode gate 12/12 |
| **complex_hvs (fork knob, default 0)** | **DONE + ACTIVE** — mds0_level 3: MDS0 fast-loop luma dist switches Hadamard SATD(<<4) -> whole-block spatial SSD unshifted (fork set_mds0_controls case 3 + fast_loop_core SSD-precedence arm; pruning_method_th stays 0 = the level the funnel already models). PORT-NOTE(unverified) carried: C-side dump pending the hybrid growing case 3 (it assert(0)s today) | `hdr_fork_e2e.rs complex_hvs_is_live`; decode gate 6/6 |
| alt_ssim_tuning, cdef_scaling, tune 6 policy | OPEN — inert at fork defaults; alt_ssim_tuning needs the tune-SSIM lambda machinery the port lacks | |

Recommended landing order for e2e HdrFork identity on the all-intra path:
chroma-qindex (FH-witnessable now) → ac_bias facade → noise-norm →
per-SB delta-q wiring (+ sharp-tx activation) → QM → full e2e matrix vs MODE1.
