# Historical GitHub issue #19: Quality ladder: one confirmed non-monotonic step (1/1599), plus 36% setting saturation on the quality axis

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/19. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

## Summary

Low-signal tracking issue, filed for the record rather than as a priority: a quality-ladder
sweep of the AVIF still-image path with the `svt-rs` backend finds **one** adjacent quality
step where raising `quality` produces a worse image by two independent perceptual metrics that
agree.

**1 confirmed step out of 1,599 examined (0.06 %).** For contrast, the rav1e backend on the
identical grid confirmed 20 of 2,457, and jxl and webp confirmed zero. So this backend's
ladder ordering is close to clean, and the one step below is recorded so it is not
rediscovered rather than because it is urgent.

| image | step | SSIMULACRA2 | butteraugli-3norm | butteraugli-max | encoded bytes |
|---|---|---|---|---|---|
| `090d19695a8b43c2_512sq` | q15 → q16 | 22.71 → **22.10** (−0.61) | 3.882 → **4.202** | +1.710 | 621 → 607 (**−2.3 %**) |

Note the file gets *smaller*, so this is a legitimate rate/distortion point that is merely
mis-ordered on the quality axis — the weakest of the shapes this sweep looks for, and much
less serious than a step that costs bytes and quality together.

## Rule used

A step is counted only when **both** reference metrics call the higher setting worse by a
material margin: SSIMULACRA2 drops by ≥ 0.5 points (our dial gate's own materiality) **and**
butteraugli 3-norm distance rises by ≥ 0.05. The butteraugli margin is the 85th percentile of
the butteraugli move on *forward* steps whose ssim2 move is exactly material, rounded up.
Requiring agreement matters: of 105 steps where ssim2 alone reads a material inversion on this
instrument, butteraugli corroborates only 47.

## Separate observation — setting saturation, not an inversion

Not a defect claim, and deliberately kept out of the count above, but it is the more
consequential property of this backend on a quality axis and is worth having written down:

| backend | cells | byte-identical duplicates | distinct settings |
|---|--:|--:|--:|
| `svt-rs` | 2,574 | **936 (36.4 %)** | 1,638 |
| rav1e | 2,574 | 78 (3.0 %) | 2,496 |

Because `quality` 0–100 maps onto QP 0–63, adjacent quality values collide in irregular runs
of one or two (measured: q0=q1, q2=q3, q5=q6, …), so **36 % of the quality axis emits a
bitstream that already exists at a neighbouring setting**. Anything that walks `quality`
expecting each step to do something spends about a third of its iterations re-encoding an
identical file. A caller-visible way to enumerate the distinct settings — or a documented
statement that `quality` is a coarse view of a 64-step axis — would let callers skip them.

## Scope and reproducing

* 39 reference images × a floor-dense quality grid (q 0–30 step 1, then coarser upward),
  2,574 cells, 1,638 distinct settings, 39 ladders, 1,599 adjacent distinct pairs.
* Deterministic: a from-scratch re-run of a sibling leg reproduced 2,574/2,574 cells with
  `max |Δ| = 0` on bytes and every metric.
* Encoder pin: this sweep ran on 2026-09-05 against `2d75a105fe0b310bf586110951315f014e274fff`,
  consumed by `zenavif` at `2ebca1b4`.
* instrument: `dial_grid_372col_ladder.parquet`, sha256
  `4c3874a78c469e15c664a63e10216760317bd9501b9fe9365b6b93845cb5f980`
* how it was built: zensim `benchmarks/ladder_instrument_2026-09-05.md`
* the attribution rule: zensim `benchmarks/inversion_truth_2026-09-05.md`, implemented in
  `zensim-validate/src/dial_addressability.rs` (`encoder_inversion`)

