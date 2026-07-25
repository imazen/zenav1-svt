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

**Strong lead:** `sb_qindex::variance_adjust_qp` recenters the frame base after
the per-SB pass (`readjust_base_q_idx`: `normalized_base = min_q + range/2`).
On a ONE-superblock frame `min_q == max_q`, so `range == 0` and every offset
collapses to zero — which is exactly the flat plan observed. C keeps
`base_q_idx = 208` and emits the delta instead. Compare the port's recentering
against C's for the single-SB case first; a synthetic probe of
`deltaq_sb_variance_boost` on a steeper gradient DOES produce a large boost
(base 128 -> 93 at curve 2 / strength 3), so the kernel itself is live.

## Invariants

Default tune (1 = PSNR) is byte-unchanged — identity_matrix 54/54, workspace
933/933 — which is what made it safe to land this before the gate closes. When
the delta-q question is settled, add a `tools/tune_iq_gate.sh` in the shape of
`superres_gate.sh` (byte-parity vs `SVT_TUNE=3` × preset × qp × content, with
an anti-vacuity check against the tune-1 stream) and wire it into CI.
