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

**ROOT CAUSE FOUND (2026-07-25) — it is NOT the recentering.** An earlier note
here blamed `variance_adjust_qp`'s base recentering; that was wrong and is
corrected: the port's recentering matches C's mainline
`svt_av1_variance_adjust_qp` (rc_aq.c:454) exactly — including the fact that
mainline NEVER writes `normalized_base_q_idx` back to the frame header (the
`readjust_base_q_idx` argument is `(void)`-ignored there, rc_aq.c:455; only the
fork build at rc_aq.c:226 honours it). On a one-superblock frame that yields
`sb_qindex = base - boost` with the frame base untouched — which is exactly
what C emits.

The real cause is one level down. **C has TWO boost functions and the port
implements the wrong one for mainline:**

| build | signature | rc_aq.c |
|---|---|---|
| fork (`SVT_HDR_MODE`) | `av1_get_deltaq_sb_variance_boost(base_q_idx, uint64_t mean, double* variances, strength, bd, octile, curve)` | :87 |
| **mainline** | `av1_get_deltaq_sb_variance_boost(base_q_idx, uint16_t* variances, strength, bd, octile, curve)` | :350 |

The mainline one takes **u16** variances and has **no `mean` argument** — it
reads `ppcs->variance[sb_addr]`, the integer per-b64 array that picture analysis
builds (the same `compute_b64_variance` output the PD0 path already uses,
ported C-exactly in `pd0.rs`). The port's `var_boost::deltaq_sb_variance_boost`
is the FORK variant (f64 variances scaled `/65536`, plus a mean-based dark-bias
term), fed from `sb_qindex::compute_sb_variances` — a SECOND, differently-scaled
variance implementation. Wrong input domain, so the boost comes back 0.

**The fix**: port the mainline `av1_get_deltaq_sb_variance_boost` (rc_aq.c:350)
against the u16 `pd0::compute_b64_variance` output, and select it whenever the
encode is not in fork mode; keep the existing f64 variant for the fork. Note
`c_parity_var_boost.rs` does NOT currently cover either boost function (its
tests are qindex<->q conversions), so the new one needs its own differential
test — `av1_get_deltaq_sb_variance_boost` is `static` in C, so it needs the
`ref_shims.c` wrapper treatment the palette statics got.

## Invariants

Default tune (1 = PSNR) is byte-unchanged — identity_matrix 54/54, workspace
933/933 — which is what made it safe to land this before the gate closes. When
the delta-q question is settled, add a `tools/tune_iq_gate.sh` in the shape of
`superres_gate.sh` (byte-parity vs `SVT_TUNE=3` × preset × qp × content, with
an anti-vacuity check against the tune-1 stream) and wire it into CI.
