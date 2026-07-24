# CRF ≡ CQP for a single still frame — empirical verification (2026-07-24)

**Question.** SVT-AV1's default / user-guide-recommended still-image rate control is
**CRF** (`--rc 0 --aq-mode 2`, `--crf 35`; `set_default_configuration` sets `aq_mode = 2`,
Parameters.md `--aq-mode` default 2, `--crf` default 35). The pure-Rust port implements
only **CQP** (`--rc 0 --aq-mode 0`). Is that a rate-control gap for stills?

**Answer: NO.** For a single still frame, CRF and CQP produce **byte-identical** output.

## Why (source)

The aq-mode-2 per-SB deltaq is `svt_aom_sb_qp_derivation_tpl_la` (`rc_aq.c:787`), called at
`rc_aq.c:899` **only** under:

```c
if (scs->static_config.aq_mode == 2 && ppcs->tpl_ctrls.enable && ppcs->r0 != 0)
```

`r0` is the TPL (temporal-prediction-lookahead) rate-distortion ratio. It is initialised to
`0` (`pcs.c:1299`) and only raised by TPL analysis over **future** frames. A single still
frame (`--avif 1 -n 1`, `intra_period = 1`) has no future frames, so `r0` stays `0`, the
gate is false, and aq-mode 2 applies **no deltaq**. The only still-frame deltaq that can fire
is the fork variance-boost (`enable_variance_boost` / tune-IQ), a separate path.

## Empirical proof (built C encoder `./Bin/Release/SvtAv1EncApp`, SVT-AV1 v4.2.0)

Input: a 128×128 content-varying still (`still.y4m` — smooth ramp + a high-frequency
textured quadrant, so per-SB variance differs widely; aq WOULD change per-SB QP if it fired).

| test | result |
|---|---|
| `--aq-mode 2` vs `--aq-mode 0`, preset 0, qp 20 | IDENTICAL (1874 B) |
| `--aq-mode 2` vs `--aq-mode 0`, preset 0, qp 55 | IDENTICAL (314 B) |
| `--aq-mode 2` vs `--aq-mode 0`, preset 8, qp 20 | IDENTICAL (2034 B) |
| `--aq-mode 2` vs `--aq-mode 0`, preset 8, qp 40 | IDENTICAL (978 B) |
| `--aq-mode 2` vs `--aq-mode 0`, preset 8, qp 55 | IDENTICAL (340 B) |
| `--crf 30` vs `--cqp 30`, preset 8 | IDENTICAL (1529 B) |
| `--crf 45` vs `--cqp 45`, preset 8 | IDENTICAL (727 B) |
| `--qp 40` vs `--cqp 40` vs `--crf 40`, preset 8 | IDENTICAL (978 B, md5 `1e42a1e01f8e…`) |

Command form: `SvtAv1EncApp -i still.y4m -b out.obu <rc-tokens> --preset P --avif 1 --lp 1 -n 1`.

## Consequence

The port's `RcConfig { mode: Crf|Cqp, qp: N }` at `qp = N` already emits SVT-AV1's
default-CRF bytes for a still. `RcMode::Crf` being identical to `RcMode::Cqp` is
**correct-by-design for one frame, not a stub**. Rate control is therefore **not** a
still-image parity gap. (Domain note: `--crf`/`--cqp` accept 1–70 in 0.25 steps; the port's
`qp` is the integer `--qp` 0–63 domain, so the extreme-low-quality tail crf 64–70 and
fractional CRF are the only unreachable values — outside the practical AVIF range.)

Tracker: imazen/zenav1-svt#7.
