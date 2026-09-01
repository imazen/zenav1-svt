# Partition-search ladders: `max_block_size` + NSQ geometry / search

Which arm of C's `scs->allintra` fork the port takes for the three ladders that
shape the PARTITION SEARCH, what each arm does, and what is still unwired.

Written 2026-08-31 with the inter campaign's chunk 2 (`docs/INTER-ENCODE-PLAN.md`).
Every number below is measured on this host unless it says otherwise.

## 1. The three ladders

`enc_mode_config.c` dispatches each on `scs->allintra`
(`:7127` for the first, `md_config_process.c:924-930` for the other two):

| signal | allintra | rtc | video (`_default`) |
|---|---|---|---|
| `ctx->max_block_size` | `get_max_block_size_allintra` `:7042` | `_rtc` `:6995` | `_default` `:6991` |
| `pcs->nsq_geom_level` | `svt_aom_get_nsq_geom_level_allintra` `:8240` | `_rtc` `:8236` | `_default` `:8216` |
| `pcs->nsq_search_level` | `svt_aom_get_nsq_search_level_allintra` `:8363` | `_rtc` `:8331` | `_default` `:8254` |

All six are ported in `port_enc_mode_config/{common,leaf}.rs` and gated at
**evidence tier 1** — a differential against the real exported symbols out of
`libSvtAv1Enc.a` — by `tests/c_parity_sig_deriv_{leaf,common}.rs`. The rtc arm
is translated but unreachable: the port has no rtc envelope.

### What the arms actually disagree about

**`max_block_size`.** The allintra arm caps the SB to half its size when the
64x64 pixel variance exceeds a qp-scaled threshold, but `base_var_th_cap` is
`(uint16_t)~0` through M7 — no `u16` variance can exceed it — so the cap is
live only at **M8+**. Incomplete edge SBs bail to the uncapped size on both
arms. The video arm has no cap at all, at any preset:
`ctx->max_block_size = scs->super_block_size`, full stop.

**NSQ geometry.** Allintra: MR 1, M0..M3 2, M4..M6 3, **M7+ 0 (off)**. Video:
M0 `coeff_lvl == HIGH ? 2 : 1`, M1..M5 `HIGH ? 3 : 2`, M6+ 3 — **never 0**. So
above M6 the two arms disagree about whether non-square shapes exist at all,
which is what a ONE-FALSE boundary node consults: with geometry on it keeps its
single injected edge shape, with geometry off it force-splits.

**NSQ search.** Allintra: M0 3, M1 10, M2 14, M3 16, **M4+ 0 (off)**. Video:
M0 `is_base ? 2 : 3`, M1..M2 7, M3 9, M4 12, M5..M6 15, M7 18, M8+ 19 — then
the r0 modulation, the `coeff_lvl` offset and the `seq_qp_mod` offset, each of
which can saturate the level back to 0.

## 2. What was wired (2026-08-31)

`pipeline.rs` carried the allintra arm FLATTENED into three inline predicates
and ran them on every frame, video-mode key frames included:

| flattened | stood for |
|---|---|
| `speed_config.preset >= 8 && x0 + 64 <= w && y0 + 64 <= h` | `get_max_block_size_allintra` |
| `speed_config.preset <= 6` (two sites) | `..._nsq_geom_level_allintra() != 0` |
| `NsqCfg::for_preset_qp`'s base table + offsets | `..._nsq_search_level_allintra` |

`src/part_arm.rs` replaces all three with calls into the tier-1-gated ladders,
selected by the `ScArm` the frame already resolves (`sc_detect.rs`, C
`scs->allintra`, threaded into `encode_tile_rows` by the intra-BC chunk). The
still path is byte-neutral **by construction** — its arm evaluates the ladder
the flattening was transcribed from — and
`part_arm::tests::allintra_flattening_matches_the_ladder` pins that
entry-for-entry over presets 0..=13 x qp 0..=63, with the old inline
predicates kept verbatim as the regression oracle.

`NsqCfg` gained `for_arm` / `for_levels`: the level -> controls row is now
reached from either arm's ladder, `set_nsq_geom_ctrls` (`:8180`) supplies the
`(allow_HV4, min_nsq_block_size)` pair instead of the hardcoded `(true, 0)`,
and the `set_nsq_search_ctrls` tail's qp-based scaling (`:7110-7121`) is live.
That tail was previously assumed to be 1/1 — correct on the still path, where
`nsq_qp_based_th_scaling` is 0 through M3 and the search ladder is 0 from M4,
and **wrong** on the video arm, where the flag is 1 at every reachable preset.

### Two premises, both pinned rather than asserted

- **`pcs->coeff_lvl` is `INVALID_LVL` on a video-mode I-slice.**
  `md_config_process.c:898-902` runs `derive_intra_coeff_level` only under
  `scs->allintra` and `derive_inter_coeff_level` only when
  `!rtc && slice_type != I_SLICE`; a video KEY frame matches neither. Both
  `_default` ladders compare `coeff_lvl` only by equality, so `INVALID_LVL`
  behaves as `NORMAL_LVL` — pinned against the real symbols by
  `nsq_levels_treat_invalid_coeff_lvl_as_normal`, with
  `invalid_coeff_lvl_probe_has_a_positive_control` proving the probe can fire.
- **`r0_gen` is 0 for every configuration this port encodes.** It comes from
  `pcs->tpl_ctrls.enable` (`initial_rc_process.c:734-744`), and `get_tpl`
  (`enc_handle.c:3665`) returns 0 whenever `pred_structure == LOW_DELAY` —
  which is the only multi-frame GOP shape the port and the inter harness use.
  So the video search ladder's r0 modulation is structurally unreachable here.
  **Revisit this first** if a RANDOM_ACCESS envelope is ever wired.

## 3. Measured outcome

Still path, after the wiring: **1100 / 1100** on `identity_full_8bit.sh`, and
all six reference cells byte-identical at their pinned sizes — gradient 64x64
q40 p6 290 B, q20 p3 839 B, q55 p0 63 B, 128x128 q55 p8 171 B, 64x64 q30 p13
580 B, screenrep 64x64 q35 p4 693 B. `regression_spotcheck.sh` 44 / 44.

Video-mode KEY frame (`identity_diff_inter.sh`, frames=2, frame 0), before and
after, on the SAME build of the C oracle:

`gradient 72x88 q40` — the partial-SB cell, where NSQ geometry is observable:

| preset | C | port before | port after |
|--:|--:|--:|--:|
| 4 | 1403 | 1492 | **1398** |
| 5 | 1485 | 1499 | **1484** |
| 6 | 1523 | 1509 | 1509 |
| 7 | 1539 | 1502 | **1511** |
| 8 | 1554 | 1541 | 1541 |
| 9 | 1589 | 1630 | 1630 |

`gradient 64x64 q40` — 64-aligned, so no boundary node exists and the geometry
ladder cannot be witnessed at all:

| preset | C | port before | port after | first diverging FH field after |
|--:|--:|--:|--:|---|
| 0 | 976 | 964 | 964 | `loop_filter_level[0]` C=0 port=2 |
| 1 | 969 | 959 | 959 | none |
| 2 | 974 | 953 | **959** | `cdef_y_sec_strength[0]` C=1 port=2 |
| 3 | 975 | 948 | **966** | `lr_type[0]` C=3 port=0 |
| 4 | 951 | 948 | **967** | `cdef_y_sec_strength[0]` C=2 port=0 |
| 5 | 951 | 948 | 948 | none |
| 6 | 961 | 971 | 971 | `cdef_uv_pri_strength[0]` C=7 port=0 |
| 7 | 961 | 971 | 971 | `cdef_uv_pri_strength[0]` C=7 port=0 |
| 8 | 971 | 977 | 977 | `lr_type[0]` C=2 port=0 |

Read that table honestly: at p4 on the 64-aligned cell the port moved 16 bytes
FURTHER from C in size while taking the arm C takes. Size is not the metric —
the ladder is — and the frame-header fields on these cells are all downstream
of a recon that is still diverging for other reasons (the CDEF and LR searches
run on it). `screenrep 128x128 q20` presets 0..8 is byte-identical before and
after, which is consistent: screen content at that size routes around the
`refined` funnel path these predicates feed.

## 4. What is NOT wired

Named, so nobody has to rediscover them:

- **NSQ geometry level 1's `allow_HVA_HVB`.** Level 1 is the only level that
  sets it, and it is reachable only on the video arm at preset 0 (`_default`
  with a non-HIGH `coeff_lvl`). The funnel has no HorzA/HorzB/VertA/VertB
  candidate — `shapes_for_size` never emits them and `shape_children` is
  `unreachable!` on them — so that one cell searches level 2's shape set.
  Everything else about level 1 (`allow_HV4 = 1`, `min_nsq_block_size = 0`) is
  identical to level 2, so this is the whole of the gap.
- **`pcs->mimic_only_tx_4x4`.** `md_config_process.c:1040` forces
  `nsq_search_level = 0` (via `set_nsq_search_ctrls`'s first branch) on a
  coded-lossless frame. `NsqCfg` does not consult it on either arm; this
  predates the arm split and is unchanged by it.
- **`nsq_search_ctrls.sub_depth_block_lvl`.** Present in every C level row,
  absent from `NsqCfg`. Not reached by the funnel's walk today.
- **`ctx->pd_pass == PD_PASS_0`'s override** of the whole control row
  (`:7123-7133`). The port's `NsqCfg` serves the PD1 refined walk only.
- The **rtc arm** of all three ladders: translated, never selected.
