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

## Follow-up (2026-08-03): the mainline arm was still missing the NORMALIZER

The 2026-07-25 fix above ported the mainline boost KERNEL, but not the step C
runs immediately after it. `generate_sb_qindex` (rc_process.c:734-748) is:

```c
svt_av1_rc_init_sb_qindex(pcs, scs);
if (ppcs->frm_hdr.delta_q_params.delta_q_present && ppcs->frm_hdr.delta_q_params.delta_q_res != 1) {
    svt_av1_normalize_sb_delta_q(pcs);
}
```

Both lines are MAINLINE — outside every `#if SVT_HDR_MODE` block. And unlike
`svt_av1_variance_adjust_qp` / `av1_get_deltaq_sb_variance_boost`,
`svt_av1_normalize_sb_delta_q` has exactly ONE definition in the tree
(rc_aq.c:827-868, confirmed with `grep -n` + the enclosing `#if` map), so the
same function serves both builds.

It snaps every SB qindex onto the residue class of the FRAME base mod
`delta_q_res`. That is what makes the pack's TRUNCATING divide exact: the writer
emits `(cur - prev) / delta_q_res` and stores `prev = cur`
(entropy_coding.c:4996-5015), while a conforming decoder stores
`prev = prev + reduced * delta_q_res` (spec 5.11.41). Off the residue class the
remainder is dropped by the divide and never restored, and because `prev`
carries forward, **the error COMPOUNDS across the SB raster** — encoder and
decoder dequantize different SBs with different qindexes. Corruption class, not
a rate inefficiency.

**Which base the normalizer keys on differs by mode, and copying the fork arm
verbatim is a second, subtler bug.** The fork resignals the recentered base
(rc_aq.c:299-306, `if (readjust_base_q_idx) ppcs->frm_hdr.quantization_params
.base_q_idx = normalized_base_q_idx;`), so when the normalizer reads
`base_q_idx` it sees the recentered value. Mainline never writes it back
(rc_aq.c:455 is `(void)readjust_base_q_idx`), so the normalizer sees the
ORIGINAL frame base — which is also what the frame header signals and what the
pack's `prev` is initialised to. Keying mainline on the recentered value puts
every SB in the wrong class and reintroduces the same drift.

Landed as `sb_qindex::normalize_sb_delta_q(base_q_idx, delta_q_res, &mut sbq)`
— one Rust definition mirroring C's one definition — called from
`variance_adjust_qp_mainline` with the ORIGINAL base and from the fork
`variance_adjust_qp` with `normalized_base` (the fork arm's inline copy was
replaced by the call; byte-identical).

Reachability: `HdrForkConfig::apply_tune_overrides` sets
`enable_variance_boost = true` for TUNE_IQ / TUNE_MS_SSIM regardless of mode
(hdr_mode.rs:330-346) and the pipeline calls it unconditionally, so plain
MAINLINE + `hdr.tune = 3` at CLI qp >= 20 (qindex >= 80 => `delta_q_res >= 2`)
hits it.

MEASURED (2026-08-03):

| evidence | before (normalizer skipped) | after |
|---|---|---|
| `tools/variance_boost_recon.sh` (60 cells: 2 contents x 3 sizes x qp{20,30,40,55,63} x preset{6,10}) | **0 passed, 60 failed** — up to 11.9k luma px + 5.8k chroma px per cell diverge between encoder recon and aomdec; 256/440 planned SB qindexes outside the base residue class | **60 passed, 0 failed**; 0 residue violations |
| `c_parity_sb_qindex::normalize_sb_delta_q_matches_c_exhaustive` (base 1..=255 x res{2,4,8} x qindex 1..=255 vs the real exported C via `ref_normalize_sb_delta_q`) | n/a (new) | pass |
| `c_parity_sb_qindex::mainline_plan_survives_c_pack_decoder_roundtrip` | FAIL: "qp 20 res 2 sb 8: decoder reconstructs 72, encoder used 71" | pass |
| `tools/identity_matrix.sh` default 54 cells (tune PSNR => boost off) | 54/54 identical | 54/54 identical (change is inert with the boost off) |
| `tools/recon_parity.sh` | 432/432 | 432/432 |

Example failing plan at qp 40 (base 160, res 8): `[99, 160, 105, 99]` — 99 % 8
== 3 and 105 % 8 == 1, neither congruent to 160 % 8 == 0. After the fix every
entry is congruent.

The C oracle is a new `ref_normalize_sb_delta_q` shim that callocs
pcs/ppcs/scs + a `SuperBlock` array and calls the real exported symbol (the
same shell pattern the IntraBC shims use) — strongest evidence tier, not
hand-derived vectors.

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

## How to continue — the method that got this far

The two things that turned "the bytes differ" into a coordinate, both worth
reusing on the remaining symbol:

1. **Pick the emptiest cell that still reproduces.** `gradient 64x64 q55 p8` is
   a 65-byte stream, so its whole symbol trace fits on a screen and every
   divergence is readable by eye (`/tmp/<out>/{c,rs}.trace`, written by
   `tools/identity_diff.sh`). Debugging the same bug at q32 (519 bytes) would
   have been guesswork.
2. **Bisect by CONFIGURATION, not by code.** Each tune-IQ override can be set
   independently on both sides, so a divergence can be attributed to one
   setting before reading any implementation. `SVTAV1_VB_DUMP=<path>` prints the
   per-SB qindex plan to a FILE (never stderr — the harness parses this
   process's stderr as its symbol trace; that is why the dump is not an
   `eprintln!`).

What NOT to repeat: I twice diagnosed from the fork's C code without checking
whether a second, mainline definition existed (see `rust/CLAUDE.md` guard #7).
Both times the fork version was the one that came up first in a grep, and both
times it was the wrong one. Check for dual definitions FIRST.

## Invariants

Default tune (1 = PSNR) is byte-unchanged — identity_matrix 54/54, workspace
933/933 — which is what made it safe to land this before the gate closes. When
the delta-q question is settled, add a `tools/tune_iq_gate.sh` in the shape of
`superres_gate.sh` (byte-parity vs `SVT_TUNE=3` × preset × qp × content, with
an anti-vacuity check against the tune-1 stream) and wire it into CI.
