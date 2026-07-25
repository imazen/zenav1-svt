# Tune IQ (`--tune 3`) — port map

**Status: WIRED END-TO-END, one variable left.** Everything C's tune IQ changes
is ported and byte-comparable; the remaining divergence is the variance-boost
MAGNITUDE, localized to a single function.

## What tune IQ is

`--tune 3` is documented "still image only" (`Docs/Parameters.md:77`). It is not
one RD knob: `svt_av1_enc_set_parameter` rewrites seven settings
(`enc_handle.c:4889-4915`), so honouring `tune` alone does NOT honour `--tune 3`:

| setting | tune-IQ value | shared with TUNE_MS_SSIM |
|---|---|---|
| `enable_qm` | 1 | yes |
| `min_qm_level` / `max_qm_level` | 4 / 10 | yes |
| `min_chroma_qm_level` / `max_chroma_qm_level` | 4 / 10 | yes |
| `sharpness` | 7 | yes |
| `enable_variance_boost` | 1 | yes |
| `variance_boost_strength` / `variance_boost_curve` | 3 / 2 | yes |
| `max_tx_size` | `qp <= 45 ? 32 : 64` | **no — IQ only** |
| `screen_content_mode` | 3 | **no — IQ only** |

## Ported

- `HdrForkConfig::apply_tune_overrides(qp)` — the block above, verbatim, applied
  once per frame at the top of `encode_frame_impl`.
- The four knobs are no longer `is_fork()`-gated. They are MAINLINE v4.2.0
  features; gating them made SVT-AV1's own still-image advice unreachable.
- `max_tx_size` threaded into all four `pd0_pick_sb_partition*` entries
  (`max_sq_size = MIN(max_sq_size, 32)`, `enc_dec_process.c:1494-1495`, plus the
  depth-refinement cap at `:1815`).
- `screen_content_mode = Some(3)` forces the detector on regardless of preset.
- Harness: `SVTAV1_TUNE` (port) / `SVT_TUNE` (C driver) — one env vector drives
  both encoders through `tools/identity_diff.sh`.
- `SVTAV1_VB_DUMP=<path>` dumps the per-SB qindex plan to a file.

## The one open variable — with the evidence

MEASURED on `gradient 64x64 q55 p8` (a nearly empty cell, so the trace is short
enough to read symbol by symbol):

- the frame header matches C **field for field**;
- the partition symbol matches (`CDF10 s=0`), the skip bool matches;
- the FIRST and ONLY divergence is the per-SB delta-q: **C codes
  `delta_q_abs = 1`, the port codes `0`**, because its plan comes back flat —
  `SVTAV1_VB_DUMP` prints `base=208 res=8 plan=[208]`.

At `q32 p8` the same signature appears one symbol later, after C additionally
splits 64->32 (which the port now matches through `max_tx_size`).

So QM signalling, sharpness, the partition cap, the scm force and the header are
all already C-exact under tune IQ. What differs is `variance_adjust_qp`'s
magnitude at strength 3 / curve 2 / octile 5.

## Progress: the delta-q divergence is FIXED; one coefficient symbol remains

**Fixed (2026-07-25):** C defines `av1_get_deltaq_sb_variance_boost` TWICE and
the port implemented only the fork one.

| build | signature | rc_aq.c |
|---|---|---|
| fork (`SVT_HDR_MODE`) | `(base_q_idx, uint64_t mean, double* variances, strength, bd, octile, curve)` | :87 |
| **mainline** | `(base_q_idx, uint16_t* variances, strength, bd, octile, curve)` — no mean | :350 |

The mainline kernel reads the INTEGER per-b64 map picture analysis builds
(`ppcs->variance[sb_addr]`, i.e. `pd0::compute_b64_variance`'s output), not the
fork's f64 maps. Feeding the fork kernel on a mainline encode computed the boost
in the wrong input domain and returned 0. Now ported as
`var_boost::deltaq_sb_variance_boost_mainline` +
`sb_qindex::variance_adjust_qp_mainline`, selected whenever the encode is not in
fork mode — and mainline correctly leaves the frame base alone
(`readjust_base_q_idx` is `(void)`-ignored at rc_aq.c:455), where the fork
resignals it.

Also landed: `qm::still_get_qmlevel` had a call site but no definition on the
mainline path — the degree-7 still-image polynomial (md_config_process.c:185)
that `TUNE_IQ`/`TUNE_MS_SSIM` use instead of the linear `aom_get_qmlevel`.

MEASURED effect on `gradient 64x64 q55 p8`: the encoded SIZE now matches C
exactly (65B == 65B, was 64B vs 65B) and the first divergence moved from
**tile-op 3 to tile-op 197** — i.e. the frame header, the partition, the skip
flag, the delta-q value AND its sign, the y-mode, and ~190 coefficient symbols
now all match C.

## The one remaining symbol

At op 197 both encoders are deep in a run of 4-symbol coefficient CDFs; C codes
`s=0` where the port codes `s=1` (icdf0 = 8946) — one coefficient's level class,
one step apart. At q32 the streams are 519B (C) vs 517B (port), same class.

RULED OUT: the QM LEVEL selection — the frame header matches field for field,
and the qm levels are frame-header fields, so both encoders picked the same
matrices.

REMAINING CANDIDATES, in order:
1. **`sharpness = 7`'s effect on RDOQ.** Tune IQ sets sharpness 7, and this
   port's RDOQ rshift formula is documented to depart from mainline only at
   `>= 3` (see `rust/CLAUDE.md`) — so 7 exercises a path that has never been
   byte-verified against C. Compare `quant.rs`'s sharpened rshift against C's
   for sharpness 7 first.
2. **QM APPLICATION** (not selection) — the matrices are chosen correctly, but
   check the quantizer applies them exactly as C does at these levels.

Neither `deltaq_sb_variance_boost` variant has a differential test yet
(`c_parity_var_boost.rs` covers only the qindex<->q conversions), and the C
symbols are `static`, so closing this properly also means giving them the
`ref_shims.c` wrapper treatment the palette statics got.

## Invariants

Default tune (1 = PSNR) is byte-unchanged — identity_matrix 54/54, workspace
933/933 — which is what made it safe to land this before the gate closes. When
the delta-q question is settled, add a `tools/tune_iq_gate.sh` in the shape of
`superres_gate.sh` (byte-parity vs `SVT_TUNE=3` × preset × qp × content, with
an anti-vacuity check against the tune-1 stream) and wire it into CI.
