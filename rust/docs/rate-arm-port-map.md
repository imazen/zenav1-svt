# The `scs->allintra` fork for the three RATE ladders

`rdoq_level`, `rate_est_level` and `update_cdf_level` — wired 2026-09-01,
inter campaign chunk C1a-3. Module: `crates/svtav1-encoder/src/rate_arm.rs`.

## What was wrong

`pipeline.rs` ran the **allintra** arm of all three ladders on every frame,
video-mode key frames included, flattened into three inline expressions:

| flattened expression | C it stood for |
|---|---|
| `preset.min(9)` | the allintra preset clamp, `enc_handle.c:4416` |
| `quant::rdoq_level_allintra(eff, coeff_lvl)` | `sig_deriv_mode_decision_config_allintra`, `enc_mode_config.c:9904` |
| `FunnelCfg::for_preset`'s baked `(coeff_rate_est_lvl, real_coeff_ctx)` | `:9917` -> `set_rate_est_ctrls`, `:6428` |
| `matches!(preset, 0..=6)` as the per-SB CDF-chain gate | `svt_aom_get_update_cdf_level_allintra`, `:8534` |

C dispatches each on `scs->allintra` (`enc_handle.c:4406`) and the `_default`
twins disagree at every preset from M6 up.

## The two arms, for a video KEY frame (`is_islice`, `is_base`)

| preset | rdoq allintra | rdoq video | rate_est allintra | rate_est video | update_cdf allintra | update_cdf video |
|---|---|---|---|---|---|---|
| 0..=5 | 1 | 1 | 1 | 1 | 1 (0..=3) / 2 (4,5) | 1 |
| 6 | f(coeff_lvl) | 1 | 1 | 1 | 2 | 1 |
| 7..=8 | f(coeff_lvl) | 1 | **4** | **1** | **0** | **1** |
| 9 | f(coeff_lvl) | 1 | **0** | **1** | 0 | 0 |
| 10 | f(coeff_lvl), clamped M9 | 1 | 0 | 1 | 0 | 0 |
| 11..=13 | f(coeff_lvl), clamped M9 | **2**, clamped M11 | 0 | 1 | 0 | 0 |

`f(coeff_lvl)` is `HIGH -> 0, NORMAL -> 3, else 2`: the allintra ladder never
returns 1 above M5, and the video ladder never returns anything but 1 below
M11.

Three things worth stating because each is easy to get wrong:

- **The preset clamp is per-arm.** allintra clamps `> M9` to M9; video, non-RTC,
  clamps `> M11` to M11 (`enc_handle.c:4433`). So presets 10..13 are NOT a
  second measurement of M9 on the video arm the way `rust/CLAUDE.md` envelope
  guard 5b says they are on the still arm. RTC's `> M13 -> M13` and the
  RANDOM_ACCESS 4K clamp are not modelled — the port's video envelope is
  LOW_DELAY with `static_config.rtc == 0`.
- **`update_cdf_level` 1 and 2 are the SAME frame for an I-slice.** They differ
  only in `update_mv`, and `set_cdf_controls` (`:8495`) forces that to 0 on an
  I_SLICE. That is why M4..M6 is byte-inert across the fork even though the
  levels differ, and why the rows that bite are 7 and 8 — where the allintra
  arm switches CDF adaptation off entirely (`cdf_ctrl.enabled == 0`, so C never
  writes `ec_ctx_array` at all) and the video arm keeps the per-SB chain
  running. `cdf_ctrl_arms_diverge_at_m7_m8_and_coincide_below` pins both halves
  against C.
- **The video arm never reads `coeff_lvl`.** C leaves `pcs->coeff_lvl` at
  `INVALID_LVL` for a video I-slice (`md_config_process.c:898-902` runs
  `derive_intra_coeff_level` only when `scs->allintra`), which is sound
  precisely because `rdoq_level_default` does not consult it.

## Why the three are wired together

They are one C function's output and `set_cdf_controls` couples them:
`update_coef = (rate_est_level || rdoq_level) ? 1 : 0` (`:8479`). Wiring
`update_cdf_level` alone would run the per-SB CDF chain at M7/M8 under
`rate_est_level = 4`'s controls — a state C never produces on either arm. The
chunk brief named only `rdoq_level` and `update_cdf_level`; `rate_est_level` is
the third and is included for that reason.

## Evidence — tier 1 on BOTH arms

- **video**: `svt_aom_sig_deriv_mode_decision_config_default` is exported and
  `tests/c_parity_sig_deriv_md_config.rs` already drove it through
  `sigderiv_shims.c`'s `ref_sig_deriv_md_config_default`, reading back
  `pcs->rdoq_level`, `pcs->rate_est_level` and all four `cdf_ctrl` fields. The
  wiring calls the same `md_config::rdoq_level_default` /
  `RATE_EST_LEVEL_DEFAULT` / `leaf::get_update_cdf_level_default` that
  function's body calls — one transcription, two consumers.
- **allintra**: NEW. `svt_aom_sig_deriv_mode_decision_config_allintra` is
  likewise exported (`nm -g` shows it GLOBAL in the aarch64/macOS and
  x86-64/Linux archives, checked on both hosts before the shim was written), so
  the new `ref_sig_deriv_md_config_allintra` shim drives the real C function
  and reads the same six fields back. That upgrades
  `quant::rdoq_level_allintra` from a hand-transcription with unit tests
  (tier 4) to tier 1, and pins `FunnelCfg::for_preset`'s baked rate-estimation
  pair to the real ladder. Mutation-verified: flipping the ladder's `<= M5` to
  `<= M4` fails `allintra_rdoq_ladder_matches_c` at M5.

The still path is byte-neutral **by construction** — the allintra arm evaluates
the ladder the flattening was transcribed from — and
`rate_arm::allintra_flattening_matches_the_ladder` pins that entry-for-entry
over presets 0..=13 x all four `coeff_lvl`s, with the old inline expressions
kept verbatim as the regression oracle.

## Measured

Still envelope, unchanged: `identity_diff.sh` gradient 64x64 q40 p6 = 290 B,
q20 p3 = 839 B, q55 p0 = 63 B, 128x128 q55 p8 = 171 B, 64x64 q30 p13 = 580 B,
screenrep 64x64 q35 p4 = 693 B — all VERDICT: IDENTICAL. `identity_full_8bit.sh`
1100 / 1100. `regression_spotcheck.sh` 45 / 45. Workspace 2390 / 2390.

Video-mode KEY frame (`identity_diff_inter.sh <w> <h> <q> <p> 2 <content>`,
frame 0), before -> after:

| cell | C | before | after |
|---|--:|--:|--:|
| gradient 72x88 q40 p6 | 1523 | 1509 | 1511 |
| gradient 72x88 q40 p7 | 1539 | 1511 | 1499 |
| gradient 72x88 q40 p8 | 1554 | 1541 | 1532 |
| gradient 72x88 q40 p9 | 1589 | 1630 | **1587** |
| gradient 72x88 q40 p10 | 1599 | 1630 | 1587 |
| gradient 72x88 q40 p11 | 1634 | 1630 | 1592 |
| gradient 72x88 q40 p12/13 | 1634 | 1629 | 1591 |
| gradient 128x128 q40 p6 | 3326 | 3339 | 3308 |
| gradient 128x128 q40 p7 | 3326 | 3295 | 3326 |
| gradient 64x64 q40 p6 | 961 | 971 | 947 |
| screenrep 128x128 q35 p7 | 7419 | 7422 | 7428 |

Presets 0..=5 do not move anywhere — the arms agree on all three ladders there,
which is the fork's own prediction and a useful negative control.

**It is not uniformly closer, and that is expected.** p9/p10 collapse from 2.58%
/ 1.94% off to 0.13% / 0.75%; p7/p8 and p11..13 get further. Only 3 of the
~30 picture-level ladders `sig_deriv_mode_decision_config_*` assigns are on the
video arm now — `txt_level`, `nic_level`, `txs_level`, `intra_level`,
`chroma_level`, `cfl_level`, `spatial_sse_full_loop_level`, `pic_bypass_encdec`,
`pic_disallow_4x4`, `pd0_cost_bias_weight`, `mds0_level`, `tx_shortcut_level`,
`pic_depth_removal_level`, `pic_block_based_depth_refinement_level`,
`lambda_weight` and `pic_pd0_lvl` all still take the allintra arm — so a
video-mode frame is a HYBRID and its byte count wanders until they are wired
too. The scoreboard to read is the first-diverging frame-header field, not the
size.

**One existing spot-check cell went vacuous and was replaced, not re-limited.**
`video-key-nsq-arm-p7-72x88` (`gradient 72x88 q40 p7`, limit 2.0%) guarded the
partition-arm wiring. With this chunk landed the port emits **1499 B both with
the partition arms wired and with them forced back to Allintra** — the same
stream, so no limit can make that cell witness its fix. Per the anti-vacuity
rule it is replaced by `screenrep 72x88 q40 p7`, which separates 2414 B
(1.089% off C's 2388) from 2386 B (0.084%), at a TIGHTER limit of 0.5%. The
gradient p4 / p5 cells still separate (1492/1398 and 1499/1484) and are
untouched.

## Not wired by this chunk

- The other ~16 picture-level ladders listed above. Each has a `_default` twin
  in `enc_mode_config.c` and most are already tier-1 ported in
  `port_enc_mode_config::md_config`; they are wiring, not porting.
- `set_rate_est_ctrls`' `lpd0_qp_offset` and `pd0_fast_coeff_est_level`. They
  belong to the light-PD0/PD1 path, which neither the still nor the video-key
  envelope takes. `rate_arm::rate_est_ctrls` returns only the two members the
  leaf funnel consumes and says so.
- `real_coeff_ctx` stands for the PAIR `update_skip_ctx_dc_sign_ctx` /
  `update_skip_coeff_ctx`, which agree at every level either arm assigns
  (0 -> 0/0, 1 -> 1/1, 4 -> 0/0). They disagree at levels 2 and 3 (1/0); if a
  path that reaches those is ever wired, split the field first.
- The RTC arm (`svt_aom_get_update_cdf_level_rtc`, `:8524`) and its `> M13`
  preset clamp. Ported in `leaf`, unreachable here.
