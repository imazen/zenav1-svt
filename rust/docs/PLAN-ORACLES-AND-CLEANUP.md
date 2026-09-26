# Plan — bit-match mainline and Ghost Robot, then factor and speed up

**Live plan.** Tick items in the same change that lands them, with the
commit hash. Findings and evidence live in
[CODE-REVIEW-2026-09-25.md](CODE-REVIEW-2026-09-25.md) (items S1–S9,
T1–T7). This file only orders the work and says what "done" means for each
phase.

## Goal

1. One switch selects the C oracle: pristine **mainline** or the
   svt-av1-hdr **Ghost Robot** fork ([ORACLES.md](ORACLES.md)).
2. The port can bit-match either one, and each claim is backed by a
   standing gate that names its oracle.
3. The codebase is factored, deduplicated and fast while every step stays
   pinned. No refactor lands unless the output pins are unchanged, or the
   change is argued as a product change on RD.

## Rules that hold in every phase

- **Pin first, refactor second.** `svtav1/tests/output_pins.rs` hashes
  port output over a fixed matrix: stills and video, 8 and 10 bit, every
  reference, tunes, enhancements, tiles and threads.
  - A refactor must leave it byte-unchanged.
  - A behaviour change updates the manifest in the same commit, and the
    message says which cells moved and why.
  - Regenerating needs `UPDATE_OUTPUT_PINS=1`, set visibly by the caller,
    never inside the test.
- **Behaviour that differs by oracle is gated by `SvtReference`**, never
  by a new environment variable or a build flag. A `GhostRobot` arm and a
  `Mainline420` arm sit side by side, each citing its C commit.
- **One heavy job at a time**, under `run-heavy` with `--mem` sized to the
  host.

## Phase 0 — foundation

- [x] 0.1 (`5d949d34`) Oracle registry `rust/oracles/oracles.tsv` + `tools/oracle`
  (`list` / `build` / `libdir` / `srcdir`).
- [x] 0.2 (`5d949d34`) `SVT_ORACLE` honoured by `capture_c_trace` (build and run);
  `SVT_HDR_MODE` kept as an alias.
- [x] 0.3 (`50c683a4`) `SVT_ORACLE` honoured by `identity_run`: it selects
  `SvtReference` and the HDR mode from the registry row.
- [x] 0.4 (`c48488ae`) Ghost Robot submodule `reference/svt-av1-hdr` pinned at `9dabe3ca`.
- [x] 0.5 (`1267af21`; 227 cells, 2.6 s) Output-pin test (`output_pins`) and its committed manifest, in CI.
- [x] 0.6 (`50c683a4`) Tune numbering follows C: `TUNE_VMAF = 5`,
  `TUNE_FILM_GRAIN = 6`. Today film grain is 5 in Rust.
- [x] 0.7 (`ba07b218`; 188 targets -> 27, 4029 runs -> 2780) Each test file in exactly one test target (T1), plus a check
  that keeps it so.

Phase 0 done 2026-09-25 on `i265`: all four oracles build and encode,
`regression_spotcheck` 147/147, and the workspace suite passes (2780).
It was done when:
- `tools/oracle build` succeeds for all four oracles on `i265`;
- a still cell encodes through `capture_c_trace` under each of them;
- `output_pins` is green in CI.

## Phase 1 — API for both targets

- [x] 1.1 (`50c683a4`) `SvtReference::GhostRobot` (and `#[non_exhaustive]`), with its
  own `validate_hdr_config` envelope.
- [x] 1.2 Ghost Robot's configuration surface in `HdrForkConfig`:
  `enable_qmpsnr`, `luminance_qp_bias`, `hbd_mds`. Plus a typed facade
  builder (`AvifEncoder::with_fork(ForkConfig)`) exposing the fork knobs by
  their C names, instead of the raw `EncodePipeline.hdr` field.
  - Done (`7df9a27f`):
    - `HdrForkConfig` carries `luminance_qp_bias`, `hbd_mds`,
      `enable_qmpsnr` and `max_hierarchical_levels` at C's defaults.
      `validate_hdr_config` refuses every value the port does not implement,
      and a unit test pins that.
    - `svtav1/tests/config_surface.rs` gives each of Ghost Robot's 139
      `EbSvtAv1EncConfiguration` fields one disposition (Api / Hdr /
      Refused / Derived / DefaultOnly / NotApplicable). It parses the C
      header, so a new upstream field fails the test until it is
      classified.
    - The refusal ledger now also scans the `validate*` `Err(...)` refusals
      and the animation facade, which it had missed: 83 -> 112 rows.
  - Done (this change): the typed facade builder. `ForkConfig`
    (`svtav1_encoder::fork_config`, re-exported from `svtav1::avif`) has one
    `Option` field per `Hdr` field of the ledger, by its C name, and
    `AvifEncoder::with_fork` merges it; `with_qm`/`with_variance_boost` write
    into it, and one `resolved_hdr()` serves the encode and validation.
    `HdrForkConfig::validate_ranges` refuses what Ghost Robot's
    `svt_av1_verify_settings` refuses (22 checks; the ledger is 133 rows),
    so `with_variance_boost` no longer clamps. `config_surface` fails if an
    `Hdr` field has no `ForkConfig` field. Item 1.2 is complete.
  - Plan 1.4 narrows the facade, which needs a decision on what
    `svtav1_encoder` itself makes public: zenavif (pinned by git rev) uses
    `EncodePipeline`, `RcConfig`, `HdrForkConfig`, `FilmGrainConfig`,
    `entropy::obu` and `types::EncodeError` directly, and many integration
    tests reach internals (T2).
- [x] 1.3 (`893f6823`) `EncodingPolicy::SvtParity(GhostRobot)` resolves fork defaults
  exactly as Ghost Robot's `svt_av1_set_default_params` does.
- [ ] 1.4 Narrow the facade (S9):
  - re-export only the types its own signatures use;
  - put the rest behind the existing `__expert` feature;
  - setters stop clamping silently;
  - record each break in CHANGELOG's QUEUED BREAKING CHANGES.
  - Done (this change): `svtav1::pipeline` is the raw-OBU API, holding the
    pipeline types zenavif uses (surveyed read-only 2026-09-25); the
    whole-crate re-exports need `__expert`; the unimplemented
    `Encoder`/`EncoderConfig` scaffold is removed; `with_quality`,
    `with_speed` and `with_variance_boost` refuse instead of clamping.
  - Open: `svtav1_encoder`'s own public surface (its modules are `pub` for
    the integration tests; T2 moves those tests in-crate first).

Done when every Ghost Robot `EbSvtAv1EncConfiguration` field has either a
typed facade setter or an explicit refusal that names it.

## Phase 2 — oracle hygiene (before any parity work on a new pin)

- [x] 2.1 (`2169f16f`) Shim and source citation check: `tools/citations.py check
  --to <oracle>` fingerprints every cited C span in the base oracle and
  classifies it in the target: same, moved, ambig, changed, unresolved.
  Measured hybrid-3115 -> ghost-robot: 7,386 citations; 903 changed (12%),
  42 of them in `svtav1-cref/shims`, which are the copies to refresh before
  `c_parity` means anything for that oracle; 4,432 moved. Against
  mainline-4.2.0: 162 changed. Still open: replace shim copies that exist only
  to reach a `static` with `#include` of the owning `.c` file.
- [x] 2.2 `svtav1-cref` builds against any registry oracle
  (`SVT_ORACLE`), so `c_parity_*` runs per target. — completed in change ruwlyskl
  - `build.rs` honours `SVT_ORACLE=<pinned oracle>`: builds the oracle
    through `tools/oracle` (new `builddir` command), compiles the shims
    against its headers with its `driver_defs`, links its library, and
    globalizes statics from its build tree. Live oracles and the default
    are unchanged.
  - Every shim now compiles AND LINKS under all three oracles, bridged by
    registry `ZEN_ORACLE_*` driver_defs consumed through the shared
    `shims/zen_oracle.h` (the same mechanism `wrap_recon.c` uses). Macros
    added: `MV_BY_VALUE`, `DV_CHROMA_SS`, `PSY_DIST_RTCD`,
    `REFMVS_NO_HP`, `FACADE_QM_SCAN`, `CDEF_QP_LEVEL`, `VARIANCE_T`,
    `REFS_IN_CTX`, `EC_PARENT`, `INIT_SCAN`, `MAINLINE_API`.
  - Features an oracle lacks abort loudly via `zen_oracle_missing()`; the
    caller selects them out via `oracles/excludes/<name>.txt` (fed to
    nextest `-E` by `just cparity-oracle ORACLE`). No test body skips.
  - Ghost Robot's build inlines `hme_level_2`/`check_00_center` entirely —
    no symbols to globalize; `me_statics` cfg covers the gap and the Rust
    wrappers return `None` through `me_statics_oracle_is_available()`.
  - Measured 2026-09-25 (`benchmarks/cref_oracles_2026-09-25.meta`):
    hybrid-3115 875/875, mainline-4.2.0 867/867 (8 fork-feature tests
    excluded), ghost-robot 830/875 with 45 genuine divergences — the port
    targets hybrid today, so GR mismatches are expected and NOT excluded.
- [x] 2.3 (this change) Wrap-fired check in `capture_c_trace`: report every `--wrap`
  interposer that never fired on a cell known to reach it.
  - `SVT_WRAP_REPORT=1|<file>` prints a fire count for all 47 interposers
    at exit. The one list is `tools/capture_c_trace/wrap_list.h`: `build.sh`
    generates the `--wrap` flags from it, so the flags and the report
    cannot drift. Measured on a 64x64 q30 p6 key frame: 31 fire; the 16
    at zero are the inter / global-motion paths, plus the deblock frame
    filter (level 0 on that cell). Gates add their own "must fire"
    assertions for the interposers they depend on.
- [x] 2.4 (`2169f16f`) Citation remap: `tools/citations.py remap --to <oracle>`
  rewrites `moved` citations in place. Run it in the same change that bumps
  or retires a pin, not before.
- [ ] 2.5 Mirror the pinned Ghost Robot commit into `imazen/zenav1-svt-c`.
  This needs the repo owner, because it writes outside this repo.

## Phase 3 — bit-match Ghost Robot

Ghost Robot = mainline v4.2.0, plus 144 unreleased mainline-master
commits, plus 73 fork-only commits (analysis in the review). Port by the
review's table. Each item lands behind `SvtReference::GhostRobot`, with a
C-parity witness under `SVT_ORACLE=ghost-robot`.

- [x] 3.1 (`2d96724a`) Still-grid baseline under `ghost-robot`:
  `tools/oracle_still_grid.sh` over {gradient, photo} x {64,128,256} x
  {8,10}-bit x qp {20,32,45,55} x presets {2,4,6,8,10,13} = 288 cells.
  32/288 identical (bd8 28/144, bd10 4/144); control under
  `mainline-4.2.0` is 288/288. Recorded in
  `benchmarks/ghost_robot_baseline_2026-09-25.meta` with the per-axis and
  stage histogram. The non-flat/video legs of the wider baseline remain
  open.
- [x] 3.2 Output-changing mainline-master commits (`1bdae047`,
  `5382c4e2`, `4b1f125d`, `fae4d9ac`; `507025f6` documented
  stills-inert):
  - `1e3da1d7` (`1bdae047`) HBD Hadamard in all-intra MDS0: ported
    `svt_aom_highbd_hadamard_{8x8(avx2),16x16,32x32}` as
    `aom_highbd_hadamard_*`, gated on `GhostRobot` in `hadamard_satd_hbd`.
    Grid 32/288 identical (unchanged) — the bd10 cells it moves (op 9→15
    on photo-256-b10-p8-q32, payloads closer on six more) still hit
    earlier divergences; kernel parity pinned in c_parity_hadamard
    (avx2 multiset + composed _c oracle). Mainline control 288/288.
  - `8b1f9a0d` (`5382c4e2`) in-loop-filter fixes: ported the
    `search_wiener_finish` `wiener_win` arm (luma sized by the search
    window, not `WIENER_WIN` unconditionally) behind `GhostRobot`,
    threading `SvtReference` through the three `*_with_stop` restoration
    searches. The seq-header `enable_restoration` arm is unreachable —
    the port has no `enable_restoration_filtering` knob, so the C `!=
    DEFAULT` branch has no equivalent. Grid 32/288; two cells moved to a
    later divergence (lr_type no longer first on `photo-128-b10-p2-q55`;
    `photo-64-b8-p6-q20` tile→FH). One output pin moved
    (`grad-64x64-b10-p6-q30-GhostRobot-HdrFork`), fixture regenerated.
    Mainline 288/288; nextest 2776/2776; spotcheck 147/147.
  - `507025f6` PD1/LPD1 switch: stills-inert. `svt_aom_is_ref_same_size`
    still returns false for `I_SLICE`, so all touched paths
    (`ref_list*_count`, depth-removal ref arms) stay on the zero-ref side
    on an all-intra grid.
  - `ccdfdb09` fractional CRF above 63: ported at the single `lw_bump`
    computation in `pipeline.rs` — under `GhostRobot`, when
    `sq_qp == 63 && extended_crf_qindex_offset != 0` and the frame's
    `lambda_weight` base would be zero (research preset `<= -1` bypassing
    the ladder, or a sub-16 `picture_qp` non-IQ frame — the only shapes
    `frame_lambda_weight[_for_preset]` can return 0), the bump lands on
    `LAMBDA_WEIGHT_NEUTRAL` 128 instead of 0. Stills-inert (the grid never
    sets the offset); `md_config`'s `lambda_weight` field is carried but
    unconsumed downstream, so only the live `lw_bump` path needed it.
  - `0c1c4dec` (`4b1f125d`) luma bias removed: flat `plane_rd_mult`
    ({17,13}/{16,10} for every frame) ported at the two
    `CodingQuantCfg::allintra_rd_mult` assignments in `frame_setup.rs`,
    gated on `GhostRobot`. Stills-inert (the allintra arm already read
    row [1]); fixes Ghost-Robot VIDEO chroma RDOQ weighting (20→13/10).
    Ghost grid byte-identical to the `8b1f9a0d` grid; mainline 288/288;
    pins 6/6; nextest 2772/2772; spotcheck 147/147.
  - the rest of the 35 "behaviour?" commits, triaged by the gate:
    upstream commits in the delta are SIMD/RTC perf or verified
    bit-exact (`837210f49`, `ef51f3cf0`, `05a895cd0`, `11be59e70`,
    `0115564e9`, `13438c1f4`, `309672721`, `b7292868a`, `3f972dab0`,
    `573327e31` fused hadamard-satd — fused _c calls the same RTCD
    kernels, values identical). `e6ff85f0d` and the `463dfe461`/
    `8620b075c` pair are RTC-only. `4acc70626` is ROI segmentation only.
    `f9100ab22` is low-delay DLF (video). No remaining mainline-master
    commit reaches the still grid — the residual tile-op/FH divergence
    is fork-feature territory (items 3.3+).
- [ ] 3.3 Fork determinism and correctness fixes: `f0111bae`, `d6f4b170`,
  `560f7453`, `ec7e414d`, `2c66d9ea`.
  - Findings (measured 2026-09-25): the first still divergence is a
    `PARTITION_CDF` at tile-op 0 — C splits (`s3`), the port does not
    (`s0`). Knob sweep (`SVT_FORK_*` env overrides on
    `from_env_for_reference`): `AC_BIAS=0` moves the first divergence
    from op 0 to op 1909 — ac-bias is the dominant driver;
    `ENABLE_VARIANCE_BOOST=0` and `SHARP_TX=0` change the Rust symbol to
    `B:v1` but don't match C; `ENABLE_QM=0` moves the divergence to FH
    (`cdef_y_pri_strength` 15 vs 12).
  - Root cause found and ported (this change): the port had the ac-bias
    helpers but no `svt_psy_adjust_rate_light` call sites — C applies
    the AC-energy rate subtraction inside `perform_tx_pd0`
    (product_coding_loop.c:4511/:4602, every coeff-rate arm) and
    `perform_dct_dct_tx_light_pd1` (:5731). Wired: `Pd0Ctx::ac_bias_eff`
    + `psy_adjust_rate_light` at `lvl5_like_block_cost_rect` and
    `lvl1_cost_from_pred`, the `RateMode::LightPd1` luma arm in
    `tx_unit_inner`, plus the missing `DIST_CALC_PREDICTION` psy term
    (C adds it to both prediction and residual spatial dists;
    `tx_type_search`/:5977 twin). The op-0 cell is now byte-identical
    (`gradient-128-b8-p2-q45`).
  - Also ported in the same change: `svt_psy_distortion_hbd` /
    `psy_full_dist_hbd` (the `is_hbd` arm of `get_svt_psy_full_dist`,
    `energy_gap << 2`) and the bd10 arms in `tx_unit_hbd` + the IFS
    `model_rd_for_sb` bump — replacing a latent `debug_assert` that
    fired once bd10+ac_bias configs could reach the 16-bit IFS arm.
  - Gating: every new site is `SvtReference::GhostRobot`-only; the
    chroma `plane_type == 0` restriction on the pre-existing residual
    psy applies only under GhostRobot (Hybrid3115 keeps its pinned
    chroma term). Only GR pin cells moved.
  - `f0111bae` (`!skip_intra && pd0_use_src_samples`) and `d6f4b170`
    (temporal_filtering.c) are video-only — no still-grid effect;
    `560f7453`'s chroma `effective_ac_bias` was already the port's
    behaviour (the pre-commit C bug was never ported).
  - Open: `ec7e414d` (TF `use_8bit_subpel` — video TF only).
  - `2c66d9ea` ("restoring the full 10bit PD0 path") — PORTED for the
    allintra arm (`pd0.rs`/`pd0/entry.rs`/`pd0/quantize.rs`,
    `tile_walk/{cu,tile_body}.rs`). What the port carries: `Pd0Hbd`
    (the `input_frame16bit` plane, `full_sb_lambda_md[EB_10_BIT_MD]`,
    `sharpness`, luma QM level) threads into the LVL_0 entry and the
    LVL_1-family refinement eval; `Pd0Ctx::src16` switches every block
    cost to `extract_neighbors_hbd` (16-bit neighbour arrays) +
    `predict_dc_hbd` (`vf_hbd_10`'s DC_PRED arm — the `intra_level =
    MAX_INTRA_LEVEL - 1` injects DC-only, enc_mode_config.c:7363) +
    `residual_kernel_16bit` + `tx_quant_core`'s `bit_depth == 10` arm
    (`build_quant_table_bd_sharp` = `quants_bd` at
    `static_config.sharpness`, `quantize_b_hbd`/`quantize_b_hbd_qm`,
    no INT16 clamp) + `kf_full_lambda_bd10_tuned` for the per-SB
    `full_sb_lambda_md[EB_10_BIT_MD]` (the `av1_lambda_assign_md` chain
    at bd10, including the delta-q stats factor under variance boost).
    `video_pd0_params`' `hbd_md` input got the fork's derivation
    (enc_mode_config.c:2180-2199 — I-slices at every enc_mode, inter
    <= M9 inside the TL bound). Gate state (this change): ghost-robot
    still grid **41/288** (the last recorded count before this change
    was 39/288 at `9f7c2d1d`, with `d3c2b789` and the upstream
    fmt/mono commits in between — so the net +2 cannot be attributed
    to this arm alone; the 16-bit arm is verified LIVE by
    `SVTAV1_PD0DBG` (`hbd=1` block costs on `gradient-64-q20-bd10`
    p10) and the u8/u16 PD0 trees agree on every measured cell — the
    residual bd10 divergences are downstream of the partition search,
    matching the `B:v1`-vs-`CDF10` lead below). mainline-4.2.0
    **288/288**; `just pins`: only
    `photo-64x64-b10-p6-q30-GhostRobot-HdrFork` moved (1365B -> 980B —
    the m6_eval path's PD0 eval now runs at 16 bits); nextest
    2771/2771; spotcheck 147/147. Not yet ported from the commit: the
    VIDEO arm's 16-bit PD0 (needs the u16 recon canvas —
    `pd0_use_src_samples` is `allintra`-only after the commit, so video
    PD0 predicts from 16-bit recon; its `hbd_md` ladder now resolves
    PD0_LVL_0 but still prices the 8-bit LVL_1 model) and the HBD inter
    PD0 arm (`product_prediction_fun_table_pd0[1]` at `EB_TEN_BIT`).
- [ ] 3.3a Function-level c-parity ladder (45 genuine divergences at
  `830/875`; **now 851/873 — 22 remain**):
  - Oracle identity plumbing (this batch): `svtav1-cref` emits the
    build-time oracle name as `ORACLE_NAME` (build.rs `rustc-env`), and
    `SvtReference::for_oracle_name` maps it to the matching reference —
    every `c_parity_sig_deriv_*` test now drives the port with the C
    oracle's own reference instead of a hard-coded one.
  - `507025f65` ("Fix switch between PD1 and LPD1"): `is_ref_same_size`
    drops the `is_not_scaled` shortcut and requires
    `ref_list{0,1}_count_try > 0` + a live `reference_picture`. Pipeline
    twins at `pipeline/inter_setup.rs` (depth-removal ref stats) and
    `pipeline.rs` `pd0_ref_sb` got the same arms — note the dims-only arm
    is unreachable on inter frames today (superres-inter is refused), so
    only the count_try guard can fire there.
  - `f67a0f747` + `a74cfb9ec` (`sig_deriv_enc_dec_common`): GR arms for
    `subsampling_x == 0` (`set_lpd1_ctrls` level 0 + `pd1_lvl_refinement`
    0) and `mimic_only_tx_4x4` (lpd1 off + pd1 refinement 0); new
    `CM_I_MIMIC_TX_4X4`/`CM_I_SUBSAMP_X` shim slots under
    `ZEN_ORACLE_CTX_SUBSAMP`. Pipeline twin: `resolve_sb_lpd1` in
    `pipeline/lpd1.rs` (S3: dual-derived signal — `pic_lpd1_lvl` /
    `pd1_lvl_refinement` appear in BOTH `sig_deriv_enc_dec_common` and
    the hand-assembled per-SB path; the `subsampling_x` half is inert on
    the port's 4:2:0 envelope).
  - `85842c43c` (research presets ENC_MRS=-3/ENC_MRP=-2): GR arms in
    `get_nsq_geom_level_default`, `get_nsq_search_level_default`,
    `txt_level_default`, the interpolation-search ladder and TXS ladder
    in `md_config`, `set_me_search_params` (rtc arm widened at M11 for
    `!use_flat_ipp`), `get_gm_core_level` (`<=MRP -> 1`),
    `get_max_can_count` (`<=MRS -> 2500`), `get_nic_level_default`
    (MRS->0/MRP->1), `get_inter_compound_level` (`<=MRS -> 1`), plus the
    GhostRobot-only `enc_mode > ENC_MRP` QP-band gate in the TXS ladder.
    Threaded `reference` through `funnel_arm::{txt_level,apply}`,
    `nic_arm::{nic_level,apply}`, `part_arm` nsq helpers,
    `depth_refine/nsq.rs`, `pipeline/tile_walk/{cu,tile_body}.rs` —
    every leaf the pipeline calls now carries its own reference arm, not
    just the dead orchestrator.
  - `b183a1256`: `mfmv_level` 1 arm widened through `ENC_M5`.
  - `7c4ada2e5`: `hbd_md` GR ladder (`<=M5 -> 1`; `<=M8`/`<=M9` temporal
    -layer banded; I-slice -> 2; else 0) ported in
    `sig_deriv_multi_processes_default` AND its live pipeline twin
    `build_inter_md_frame` in `pipeline/inter_md_stage.rs` (S3 dual
    derivation).
  - `70877799`/`d705ef50`: `complex_hvs == 1` forces `mds0_level = 3` —
    `PipelineMdInputs`/`MdConfigInputs` gained `complex_hvs`, wired from
    `hdr.complex_hvs`; inert by default (GR ships `complex_hvs = 0`).
  - `ccdfdb099`: `lambda_weight` initializes to 128 when the extended-CRF
    offset is active and the ladder left 0 (matches the earlier
    `pipeline.rs` `lw_bump` port in 3.2).
  - `f67a0f747` (444): `encoder_color_format == 3` forces
    `TX_MODE_SELECT`; carried on `MdConfigInputs::encoder_color_format`,
    pinned 4:2:0 in production (port's only chroma).
  - `e6ff85f0d` (cdef): `use_qp_strength` bool became
    `qp_strength_level` (OFF/UV/YUV) — levels 7/8/9 take UV and a flat
    `skip_th = 0`, level 10 takes YUV; `CdefSearchControls` carries the
    level field under every reference (old refs normalize
    `use_qp_strength ? YUV : OFF`), shim emits the level domain via
    `ZEN_ORACLE_CDEF_QP_LEVEL`. Production twin `pack_phase.rs` calls
    `set_cdef_search_controls` with `self.reference`.
  - `b4c85f8f5` (dlf): `get_dlf_level_default` rewrote the video ladder
    (`<=M2 -> 1`, new `<=M5 -> 3`, `<=M9` last-layer arm 0 -> 7, `<=M11`
    collapsed to `is_base ? 6 : 7`, tail 0 -> 7), `dlf_level_modulation`
    >95%/`>75%` arms land on/saturate at 7 instead of wrapping to 0, and
    `get_dlf_level_rtc` tails `hierarchical_levels == 0 ? 0 : 7`.
    Pipeline twin `derive_dlf_level` in `pipeline/frame_prep.rs` carries
    the reference.
  - `f9100ab22` (dlf): `pick_method` control field (FULL_IMAGE levels
    0-4, Q levels 5-7) — numerically identical to mainline's implicit
    `sb_based_dlf` rule; ported as a normalized field, shim slot
    `DLF_O_PICK_METHOD` under `ZEN_ORACLE_DLF_PICK_METHOD`.
    `loop_filters.rs` keeps reading `sb_based_dlf`, which coincides with
    `pick_method == Q` at every level.
  - Remaining 22: `md_subpel` (10), `md_pme` (2), `rd_cost` (3),
    `rc_vbr_cbr_qpick`, `noise_gen` tables, `inter_mvp` setup,
    `intrabc_search` diamond, `dist_facade`, `enc_make_pred` masked-warp
    subsampling.
- [ ] 3.4 Fork behaviour: complex-hvs (`70877799`, `d705ef50`), MDS0
  ac-bias dampening (`c65c2bfa`), chroma noise `pow(luma, 0.75)`
  (`9f54af57`), delta-q all-skip (`2f08c8e8`), lossless across tunes
  (`a74cfb9e`).
  - `c65c2bfa`: `get_effective_ac_bias_mds0` (0.05 at I-slices vs the
    general 0.3) applies only on the `mds0_dist_type == SSD` arm of
    `fast_loop_core` — stills run `mds0_level = 0` (VAR/Hadamard), so
    stills-inert; port the helper when the SSD arm exists.
  - `70877799`/`d705ef50` complex-hvs: GR default `complex_hvs = 0` —
    inert until the knob is turned on; `9f54af57` chroma noise:
    `noise_strength_chroma` auto only when noise is on — off the still
    grid; `2f08c8e8`/`a74cfb9e` unread.
- [ ] 3.5 QM-PSNR (`dff0a9f8`).
- [ ] 3.6 High Profile 4:4:4 (`f67a0f74`, `c4e1b9ce`). This is the largest
  item, and needs the 4:4:4 chroma paths completed first (S1).
  - Ghost Robot accepts `EB_YUV444` with `profile == 1`
    (`svt_av1_verify_settings`), so 4:4:4, today a decoder-verified Zen
    extension, can get a C byte oracle under `ghost-robot`. First blocker:
    `capture_c_trace.c` hardcodes `EB_YUV420`. Brief: `gr-444`.
- [ ] 3.7 Retire `hybrid-3115*`: drop the registry rows, `SvtHdrMode`,
  `Hybrid3115` (a queued break), the hybrid gates, and `3115c0c1b` as the
  submodule pin.
  - Measured 2026-09-25 (i265): the still ledger gates hold under the
    mainline pairing (`SVT_ORACLE=mainline-4.2.0` drives both encoders):
    `identity_full_8bit` 1100/1100, `bd10_photo_gate` 191/191,
    `bd10_nonflat_gate` 309/309; the video byte gates too,
    `bd10_video_gate` 24/24 and `real_video_inter_gate` 24/24, with their
    pinned tables unchanged. `regression_spotcheck` is 137/147: its 10 mono
    cells fail because `encode_frame_impl` refuses monochrome under
    `Mainline420` ("pristine mainline SVT supports 4:2:0 only"), so a default
    flip would break mono for default users.
  - Done (this change, owner-approved): `Mainline420` accepts the Rust
    extensions and only `SvtParity` refuses them; the constructors,
    `AvifEncoder::new` and `tools/oracle`'s default are `Mainline420` /
    `mainline-4.2.0` (`SVT_HDR_MODE=1` keeps the hybrid fork oracle). One pin
    moved: mono under `Mainline420` now encodes instead of refusing. CI caches
    the pinned oracle, and `tools/oracle` fetches the pin into a shallow
    submodule. nextest 2764/2764, spotcheck 147/147, hdr_bd10_gate 64/64.
  - Open: the cref C-parity suite still builds the hybrid by default (its
    fork-feature tests need a fork oracle); moves to mainline + ghost-robot
    once those tests run against Ghost Robot.

Done when each gate reports a pinned count under both `mainline-4.2.0`
and `ghost-robot`, and README states both.

## Phase 4 — structure and deduplication (every step pinned)

In dependency order:
- S5 shared vocabulary and math helpers in `svtav1-types`.
  - Done: `svtav1_types::math` replaces 43 copies of `round_power_of_two*`,
    `divide_and_round` and `clip3` in 30 files, imported under the old local
    names, with pins unchanged (`0d544f9d`).
  - Kept: the two `wrapping_add` variants (different overflow semantics), and
    the copies in `intra_pred.rs` and `hbd.rs` (owned by the intra-x86
    delegate; fold them in after it lands).
  - Done: the 13 `rdcost` copies -> `svtav1_types::math::rd` (`fc82b896`).
  - In progress — same-name types (`0ac832fc`, `981d4788`, pins
    unchanged): 24 duplicate names resolved so far. Unified to one
    definition: `SliceType`, `MotionMode`, `DistortionType`,
    `TransformationType`, `CompReferenceType`, `Part`, `PartitionType`,
    `ObuType`, `FrameType`, `Mv`, `SubpelMvLimits`, `PredStructure`,
    `ReferenceMode`, `RefList`, `FrameUpdateType`, `InterIntraMode`,
    `CompoundType`, `SearchArea`, `SearchAreaMinMax`, `Me8x8VarCtrls`,
    `InterCompCtrls`, `RedundantCandCtrls`, `AomRcMode`, `EdgeDir`.
    Remaining: the `*Ctrls` pairs with C-field-width differences
    (`MeHmeRefPruneCtrls`/`MeSrCtrls`/`MvBasedSearchAdj`/`PreHmeCtrls` —
    `inter_me/context.rs` is the C-faithful side — plus `MdPmeCtrls`,
    `WmCtrls`, `SgFilterCtrls`, `CandEliminationCtrls`,
    `InterIntraCompCtrls`, `NearCountCtrls`, `CoeffShavingCtrls`,
    `InterInterCompoundData`, `SkipModeInfo`, `RcIntervalParams`,
    `SgrprojInfo`, `MbEdges`, `TxCoeffShape`), and the same-name
    different-thing renames (`SbVariance` ×3, `Plane`, `ComponentType`,
    `Neighbour`/`Neighbours`, `ReferenceFrame`, `NeighborMi`,
    `TileBounds`, `ScClasses`, `FirstPassStats`, `SuperresParams`,
    `InterCandidate`, `SkipSubDepthCtrls`, `RcMode`, `EncodeError`,
    `ScaleFactors`, `OrderHintInfo`, `FilmGrainParams`, `Rng`).
    `TileLimits` and `PaddedPlaneT` are deferred — their twins live in
    `restoration*`, owned by a concurrent session.
  - Next: AV1 vocabulary constants (`INTRA_FRAME` ×8,
    `LAST_FRAME` ×9, `MI_SIZE` ×9).
- S4 a `Pixel` sample trait plus a `BitDepth` enum;
- S3 derive signals once through the ported orchestrators;
- S2 split `pipeline.rs` into stages. Target: every `.rs` file at 2-3 kloc
  (user, 2026-09-25). MET for the whole workspace on 2026-09-25 (largest file:
  2,958 lines), and kept by the `file_size_check.py` CI gate.
  - Done (`64e86276`): pure moves of the tests and the entropy walk into
    `pipeline/{tests, inter_tile_byte_gate, inter_decision_probe,
    entropy_ctx, lpd1, block_syntax, partition_walk, tile_walk}.rs`, which
    takes `pipeline.rs` from 23,560 to 13,351 lines. The tools are
    `tools/split_inline_mod.py` and `tools/move_items.py`; they only raise
    privacy to `pub(super)`, and the pins are unchanged.
  - Done (`394ebdb5`): `EncodePipeline` methods move by name
    (`tools/move_methods.py`) into
    `pipeline/{ra, tpl_stage, setup, entry, grain, config_check, superres,
    cbr}.rs`, taking `pipeline.rs` to 8,497 lines.
  - Done (`cd862116`): 14 stages extracted from `encode_frame_impl`
    (`tools/ra_extract.py` drives rust-analyzer's "Extract into function",
    which works out every stage's inputs and outputs) and moved into
    `pipeline/{frame_setup, inter_setup, restoration_stage,
    frame_output}.rs`. `pipeline.rs` is 6,508 lines; `encode_frame_impl` is
    5,625. Lesson from the attempt: a stage cannot take `&mut self` while
    `run_entropy_walk` (a closure holding a shared borrow of `self`) is live,
    and a stage that RETURNS data borrowing `self` pins all of `self`. Such
    stages take `&self` and hand their `self` writes back to the caller
    (`search_restoration`). `build_inter_md_frame` and `bd10_post_pass` are
    left inline until they can take the specific fields they borrow.
  - Done (`bd3962ec`): 12 more stages move to `pipeline/{md_setup,
    loop_filters, recon_output, diagnostics}.rs`, so the debug dumps sit in
    `diagnostics.rs` (S7). `pipeline.rs` is 5,788 lines;
    `encode_frame_impl` is 4,900. rust-analyzer failed on four ranges (the
    `run_entropy_walk` closure body, the TPL r0 block, the chroma and HBD
    source prep), each for its own reason: invalid code, a closure left
    `FnOnce`, moves out of borrowed values. They remain inline.
  - Done (`5d0a77ee`): the tangled three, `entropy_walk` (the body of the
    `run_entropy_walk` closure), `bd10_post_pass` and
    `build_inter_md_frame`, are ASSOCIATED fns that take the few `self` fields
    they touch as parameters (4, 6 and 8). Then a closure or a
    returned borrow pins only those fields, as the inline code did, instead
    of all of `self`. Moved to `pipeline/{walk_driver, bd10_post,
    inter_md_stage}.rs`. `pipeline.rs` is 4,950 lines; `encode_frame_impl` is
    4,055.
  - Done (`493ad07a`): 18 more stages, with the comments above each range
    moving along with it. The three constants local to `encode_frame_impl`
    that they share (`DLF_FAST_DECODE`, `SEQ_CDEF_LEVEL`, `CDEF_FAST_DECODE`)
    are now module-level. `pipeline.rs` is 4,509 lines; `encode_frame_impl`
    is 3,595.
  - Done (`438878ba`): the state types (`EncodePipeline` and its private
    helper structs), the plane helpers and the three constants move to
    `pipeline/state.rs`; `palette_cache` moves to its own file.
    `pipeline.rs` is 3,792 lines, nearly all of it `encode_frame_impl`.
  - Done (`7988f2a4`): `encode_frame_impl` is three phases: setup stays in
    `pipeline.rs`; `tile_phase::decide_and_encode_tiles` (mode decision,
    tiles, bd10 post-pass); `pack_phase::filter_and_pack_frame` (entropy
    walk, deblock/CDEF/LR, bitstream, recon outputs, reference). `pipeline.rs`
    is 2,286 lines; `encode_frame_impl` is 2,085.
  - Done (`cd33b8b5`): `tile_walk::encode_tile_rows` (the per-tile closure,
    then the per-coding-unit body) and `mds3::eval_candidate` (intra chroma,
    inter chroma, chroma detector, tx-depth search) are split too; neither file
    is over 1.8 kloc now. The 56 extracted single-call-site stages are
    `#[inline(always)]`, which puts their code back inline.
  - Perf cost, measured (`benchmarks/perf_s2_split_2026-09-25.meta`): output
    byte-identical; +0.10% to +0.39% instructions on small frames (callgrind)
    and neutral on large ones. The remainder is extra memcpy of values that the
    stage boundaries move or return by value. The FramePlan bundling below should
    remove it; re-measure with callgrind when it lands.
  - Done (this change): the 29 per-frame values both phases read are one
    `Copy` struct, `pipeline::frame_shape::FrameShape`. It is built once
    (none of them changes between the phases, checked), and each phase
    destructures it on entry, so the phase bodies are unchanged. The calls go
    from 70/78 to 41/49 parameters. Instructions are unchanged (callgrind
    within 0.04%), so the +0.1..0.4% residue is NOT in the phase parameters;
    it is still open.
  - Open: the residue itself. The 2026-09-25 dbgenv caching
    (`c9b35fe7`, `benchmarks/perf_dbgenv_cache_2026-09-25.meta`: -0.3% to
    -2.1%) more than pays it back, but its cause is not found.
- S1 retire or quarantine the pre-port encoder;
- S6 one scratch strategy;
- S7 diagnostics behind an observer and a `trace` feature;
- S8 uniform `incant!` plus the tier gate.
  - Done (this change): `docs/INCANT-TIER-GAPS.tsv` lists the 24 `incant!`
    sites that run scalar on one architecture: 20 without an x86 tier
    (10-bit intra, directional, CfL, palette, IntraBC hash) and 4 without
    NEON (SATD, warp). `incant_tiers.py --check` runs in CI and fails on a
    new gap or a closed one still listed. Closing the gaps is Phase 5 item
    1, a good delegate task: one kernel per commit, pins unchanged,
    `perf_ab` on the kernel bench.

Each is a series of small commits, each green on `output_pins`.

## Phase 5 — performance (bit-exact unless argued otherwise)

Protocol: `tools/perf_ab.sh`, interleaved and pinned to one core, with
`output_pins` unchanged; record each result in `benchmarks/*.meta`.
Order, by expected size:
1. x86 tiers for the intra, 10-bit intra and CFL kernels;
2. dispatch for convolve (10-bit, scaled), diff-weighted masks,
   `enc_make_inter_predictor` and the temporal-filter kernels; NEON for
   `port_convolve`;
3. multi-offset SAD for full-pel ME;
4. 10-bit mode decision without the 8-bit twin work;
5. the discarded per-frame work in `pipeline.rs`.
   - Done: the post-encode per-SB full-pel `mv_map` fill (its only reader
     had already run) and the `tpl_sb_qp_offsets` full-frame pass (consumed
     by `let _`) are removed; the dead homegrown temporal-filter branch is
     gone from the pipeline (`092c7e27`).
   - Closed, measured 2026-09-25 (i265, callgrind on `c9b35fe7`): the
     "third walk" is gone. A frame runs at most two walks, a `recon_only`
     one when a post-filter or a later frame reads the recon and the one
     symbol-writing walk (`pack_phase.rs`); a 256x256 4-frame gradient
     video calls `encode_block_syntax` 890 times for 445 leaves. Both walks
     together cost 6.4% of instructions at p10 and 2.1% at p6. On a still
     (one walk) the walk is 19.5% at p10, and 11.4% of that is
     `write_coeffs_txb_1d`, which is real symbol coding. The walk never
     calls `eval_uv` or `txt_search`; the 34% figure that suggested it did
     was callgrind counting the recursive `encode_partition_tree'2` twice.
   - Done (this change): the per-leaf `FunnelFrame` lambda override clones
     once, not twice, and `dv_tables` is `Arc`'d. Tune IQ on a 512x512
     screen still at p4: -22.3% instructions, 1.084x wall clock,
     byte-identical (`benchmarks/perf_funnelframe_clone_2026-09-25.meta`).
   - Done (`9409e9f6`): `temporal_filter::temporal_filter`, `TfConfig`,
     `TfResult` and the f64 `estimate_noise` are deleted. The
     `fallible-alloc` test now calls `try_vec!` inside the encoder crate
     directly, which is what it was testing;
6. per-call allocations (S6).
7. the remaining gap to C. `tools/perf_gate.sh` on 2026-09-25 (i265,
   gradient qp40 stills, `benchmarks/perf_2026-09-25-gap.meta`): 1.18x C at
   256 p6, 1.31x at 1024 p6, 1.37x at 1024 p10; faster than C at 64. At
   1024 p10 the port runs 347M instructions to C's 199M.
   - Done (this change): `residual_i16` row overhead and a per-thread PD0
     scratch, -3.1% instructions at 1024 p10
     (`benchmarks/perf_residual_pd0scratch_2026-09-25.meta`).
   - Done (`afbcf7f6`): `sse` and `variance_diff` row overhead, -1.95%
     (`benchmarks/perf_sse_variance_rows_2026-09-25.meta`).
   - Done (`d3c2b789`, `ea3a3d96`): eob from a generated inverse scan (also
     in the pack walk) and C's cul-level form, -4.8%
     (`benchmarks/perf_eob_iscan_2026-09-25.meta`); `6f7d8a2f`
     `DeblockGeom` row fills, -0.6%.
   - Measured after all of the above (`benchmarks/perf_2026-09-25-gap2.meta`):
     1.17x C at 256 p6, 1.21x at 1024 p6, 1.33x at 1024 p10 (p25 1.20);
     1024 p10 is 312M instructions to C's 199M (the port's figure includes
     about 21M of harness content generation that is not timed).
   - Open, by excess over C at 1024 p10: `optimize_b` (38.3M vs
     30.3M, same call count), PD0 (48.6M vs 36.7M, spread), memset
     (14.8M vs 3.0M), memcpy (10.3M vs 3.1M), and the residual chroma
     `eval_uv` overhead after the coefficient-rate findings below.
   - Item 7 (coefficient rate, investigated 2026-09-25, cell `perf_encode
     gradient 1024 1024 40 10 <prefix> 0` vs `perf_c_encode` on the
     mainline-4.2.0 oracle; byte-identical 18,906 B both ways; callgrind
     port 313,858,011 Ir vs C 197,547,115 Ir, 1.589x —
     `~/tmp/cg_coeff_before/`): classification is **(a)** — the exact rate
     is dead or C-equivalent at every point this level reaches, not a
     parity bug — but no code change has landed yet. Measured producer map
     (`SVTAV1_RATEPROBE` instrumentation on `tx_unit_inner`, all 6,792
     calls accounted for):
     * `txt.rs:464` via `search_tx_depths`, `end_depth == 0` (C's
       `perform_dct_dct_tx` arm): 1,312 luma calls, all `RateMode::Exact`,
       all `eob > 0` -> `cost_coeffs_txb` (the whole 1,312-call /
       ~12.5M-Ir finding). C computes `eob < (w*h)>>6 ? 6000+eob*1000 :
       6000+eob*400` there at `coeff_rate_est_lvl == 0`
       (product_coding_loop.c:5883); `svt_aom_txb_estimate_coeff_bits` is
       absent from C's profile and `get_eob_cost` appears only under
       `svt_av1_optimize_b`.
     * `txt.rs:464`, `end_depth > 0` (txs_lvl6-gated SBs; C's
       `tx_type_search` at :5540): 1,320 calls already `Lvl0Closed`.
     * `chroma.rs:127/151` (`eval_uv`): 4,160 calls, all `eob == 0` ->
       `cost_skip_txb`. C's `skip_chroma_rate_est` (full_loop.c:1787)
       writes the per-plane closed form (0 at eob 0;
       `3000+500e`/`1500+50e` otherwise); the port's computed value is
       discarded by the replication at `mds3.rs:1225-1262` anyway.
     * `coeff_rate_est_lvl == 0` is ALLINTRA-only: the video arm's
       `rate_est_level` is a flat 1 (`md_config.rs RATE_EST_LEVEL_DEFAULT`),
       so at lvl 0 there are no inter candidates (the `blk_skip_decision`
       read of cb/cr bits at mds3.rs:1152 is unreachable), MDS3 is
       single-candidate (nic 1/1/1, MDS1 skipped, CFL/ind-uv gated below
       lvl 0), and NSQ/depth gates are off -> every read of the exact rate
       is dead on the whole lvl-0 envelope. Producing C's closed forms is
       therefore byte-inert AND value-correct.
     * Next step (not landed): in `tx_pipeline.rs` `RateMode::Exact` arm
       (~:1488-1541) add `plane_type==1 && lvl==0` -> per-plane
       `skip_chroma_rate_est` form, and `plane_type==0 && lvl==0` ->
       `eob<th ? 6000+eob*1000 : (w<=64&&h<=64 ? 6000+eob*400 :
       3000+eob*100)` (the `w,h<=64` key reproduces the
       sq<=64/only_dct/end==0 dct-path dispatch because txt is off at every
       lvl-0 preset); change `mds1.rs` `lossless_mds1_txbs` to pass
       `Lvl0Closed` at lvl 0 (C's lossless 8x8 runs the partitioning form
       per 4x4 txb). Same chroma arm in the hbd rate section (`a.coeff_
       rate_est_lvl`); hbd luma left exact (dct-vs-partition distinction
       would need a TxRdArgs field touched in out-of-scope files).
       Residuals to note: lvl-2 chroma keeps exact bits because the
       `cb_leak + u_bits10` replication needs the raw estimator when
       `cr_eob >= th`; the `end_depth` vs per-candidate `cand_end_depth`
       selection at `mds3/tx_depth.rs:359` is a pre-existing video-arm
       edge (`Lvl0Closed` vs `DctClosed` differ only at `eob >= th`).
     * Chroma MD reconstruction is NOT removable: at this cell all 4,160
       chroma txbs are eob==0 -> pred copy, and C does the same
       `picture_copy` (`mds_do_spatial_sse` true at MDS3); C additionally
       runs 2,128 `av1_perform_inverse_transform_recon` winner re-inverts
       the port already avoids. No (b) cell was found; the leftover
       `eval_uv` gap (~23M vs C `full_loop_uv` 11.3M) is per-call
       predict/residual/copy overhead, not rate or recon work.

## Maintenance backlog

- [ ] `obmc_gate.sh` is red in CI shard 3 (owner: leave red until
  explained). It had never run in CI: `global_motion_gate.sh` stopped the
  step before it. Three p0 cells select fewer OBMC blocks than their
  2026-09-11 floors: vidyo3 256 118 (138), vidyo1 256 58 (76), vidyo3 128 40
  (62); recon == dav1d everywhere. Bisected to `606c4accd` (wm_level-1 warp
  injection, the same commit as the GM re-pin). Frame 1 of all three is NOT
  byte-identical to C, so parity does not settle what C's OBMC count is.
  Next: count C's motion modes on those cells (temporary C instrumentation or
  a C-side trace of `motion_mode`), then re-pin to C's behaviour or fix the
  port. Brief: `obmc-count`.

- [x] A red gate no longer hides the rest of its shard (2026-09-26). A
  step's default condition is `success()`, so from the day
  `global_motion_gate.sh` first ran red, every later shard-3 step was
  SKIPPED: video, RA and 10-bit self-consistency, superres, tiles, SB128,
  lossless and more, 24 steps. Gate steps now carry
  `!cancelled() && matrix.shard == N`, which `ci_shard_check.py` requires.
  The C-oracle, pinned-oracle and aomdec caches are restore + explicit save
  after their builds, since `actions/cache` only saves when the whole job
  passes; rust-cache has `cache-on-failure`.

- [x] CI green (2026-09-25; the global_motion_gate re-pin below was
  owner-approved and landed with its evidence in the gate). Was: shard 3's
  `global_motion_gate.sh` (running in CI for the first time, now that the
  sparse corpus fetches its photo) counts 14 and 2054 GLOBALMV blocks where
  it pins at least 22 and 2738. Bisected to `606c4accd` (wm_level-1 warp
  injection), which made the port commit WARPED_CAUSAL where C does and moved
  four frame-1 cells to byte-identical; these p2 cells sit on the same
  wm_level-1 ladder.

- [ ] CHANGELOG `[Unreleased]` is 3,800 lines of repeated
  Added/Changed/Fixed blocks appended per campaign, with QUEUED BREAKING
  CHANGES buried at line ~280. Fold it into one set of categories, and cut a
  dated section per released version.
  - Done (`53107d71`): folded into one QUEUED BREAKING / Added / Changed /
    Removed / Fixed set, with the queued breaks first. Every entry is
    preserved verbatim (the line multiset was checked).
  - Open: cut a dated section at the next release.

## Phase 6 — test structure

- [x] T5 (`978df514`) never-panics sweep: `svtav1/tests/never_panics.rs`,
  536 tiny encodes across geometry, depth, chroma, preset, qp, every
  reference and mode, fork knobs, LD/RA video and all Zen enhancements, in
  3 s. Measured 2026-09-25: 453 Ok, 83 explicit refusals, 0 panics; it
  asserts an Ok floor of 440 against vacuity.
- [ ] T2 in-crate differentials, shrinking the public API.
- [ ] T3 one cell harness instead of 39 bash copies. 46 of the 100 shell
  tools drive the C side directly (survey 2026-09-25). Chunks, each landing
  with its gate's verdicts unchanged:
  1. Done (this change): `tools/cellrun.py` runs a cell list (TSV: name,
     content, w, h, qp, preset, bd, env, expect) through the three steps of
     `identity_diff.sh`, with `--jobs` and pinned verdicts.
     `oracle_still_grid.sh` now writes its cross product as a cell list and
     calls it; on a 16-cell ghost-robot subset its output is identical to the
     old loop's, serial and at `--jobs 4`. Since 2026-09-26 a cell can
     also ask for `lossless`, `recon` (aomdec == the port's final recon) and
     `dav1d` checks, name a `differs_from` sibling (anti-vacuity), and run
     `--bytes-only`; each check was shown to fail on a cell built to fail
     it. `lossless_gate.sh` is the first gate ported: 240/240 as before,
     148 s -> 32 s on i265. Next: the other ~44 scripts, one gate per change,
     each landing with its verdicts unchanged.
  2. Done (2026-09-26): `identity_run` is `examples/identity_run/`, and
     `cell.rs`'s `CellSpec` parses every variable once, strictly: a value
     that does not parse, a flag other than 0/1, and a still-only knob on a
     multi-frame cell are refused. 32 cells, one per knob and dump, give
     byte-identical .obu/.yuv/dumps/symtrace before and after. What it
     exposed: `SVTAV1_SB` was ignored on the multi-frame path (the sb128
     cells of `mono_inter_gate` and `qp0_inter_gate` coded 64 px; they now
     code 128 and pass), `frontier_sweep.sh`'s arm `i` and `mem_bisect.sh`'s
     `MB_THREADS` set variables nothing read (removed; the new
     `tools/env_names_check.py`, a CI step, refuses any such name), and
     `film_grain_gate.py` was red (see CHANGELOG). The three gates joined
     CI shard 3.
  3. Run the ledger gates in CI. `bd10_nonflat_gate.sh` and the synthetic
     tier of `identity_full_8bit.sh` already ran there; `bd10_photo_gate.sh`
     joins shard 4 (its 14 CID22 photos and 2 CLIC crops join the sparse corpus
     fetch). Re-verified by hand 2026-09-25: 191/191, 309/309 and 1100/1100.
     Open: identity_full_8bit's real tier (gb82 + CID22, ~45 min).
- [ ] T4 stage-boundary differentials.
- [ ] T6 no_std, tier and dead-code gates.
  - Done: no_std. `just nostd-check` (and a CI step in shard 3) checks
    `types`, `dsp` and `encoder` one at a time with `--no-default-features`
    and `-D warnings`. It must be per crate: at workspace level, feature
    unification through dev-dependencies turns `std` back on, which is how
    the encoder carried 121 no_std errors past `test-minimal`. The encoder's
    `std` feature now forwards to `types/std` and `dsp/std`. The raw
    `std::env` debug reads (DISPDBG, QTRACE, BLKDBG, CRDBG, PSYM, and the
    `SC_TOOLS` bisect knob) moved into `dbgenv`, which reads each one once
    and returns `false` or `None` without std. OBMC's thread-local buffers
    fall back to fresh buffers on every call.
  - Done: the tier gate, as S8 (`deff9b73`).
  - Open: a dead-code gate.
- [ ] T7 inline tests out of product files.
  - Done (this change): 38 inline `#[cfg(test)]` modules in the 23 files
    over 2 kloc moved to sibling files (`<file>/tests.rs` and so on) with
    `tools/split_inline_mod.py`. The test count is unchanged (2770).
  - Open: the smaller files, and `crates/*/tests/` binaries that test
    private internals, which belong in-crate (T2).
