# zenav1-svt zensim HDR target loop — pre-registered wave (2026-08-27)

REGISTERED BEFORE ANY BUILD RUNS ITS CENSUS. GOAL criterion 4's zenav1-svt
line. Both prerequisites cleared 2026-08-27: svt#11 fixed+backfilled (115
cells re-encoded clean), and the HDR 27-cell instrument refs FROZEN
(zensim `benchmarks/hdr_instrument_refs_2026-08-27.tsv`: 9 scenes × 3 size
tiers × t∈{70,80,88}).

## Loop-location RULING (recorded)
The loop lives in a NEW workspace member `rust/crates/svtav1-target` —
codec-owned per the per-codec-loop-ownership directive, but SEPARATE from
the byte-gated parity crates (the C-parity contract of
svtav1-encoder/dsp/entropy stays pure; the target crate is a consumer).
zensim enters ONLY there (git-pinned like zenavif's). The fleet's proven
entry (`EncodePipeline` at CQP qp, 10-bit BT.2020nc 4:2:0) is the encode
cell; decode-back via the in-repo decoder route the parity gates already
use; judge = the BHdr route on decoded nits (never the loop's own score).

## The loop (phase A form)
Bracketed qp search (the zenjpeg `search_target` SHAPE, re-implemented
in-crate per ownership — a generic closure-driven bracketed search,
`max_encodes` k, tolerance band): trial(qp) = encode→decode→PU-judge.
Seed = a fixed qp staircase by target (the content-blind control — heads
come later as phase C, same family bars).

## Census (phase B — the criterion evidence)
The frozen HDR instrument: 27 cells × k∈{2,3}, judge = `zenmetrics score
--hdr` route semantics (PU-rescale, measured peak) with the BHdr bake;
report median |err| overall/per-tier/per-t + ±2 hits + bytes + passes. NO
comparison bar in phase B (it IS the baseline measurement, like zenwebp's
phase A); seed/steering arms register their own bars later.

## Endgame
Crate + harness example + census TSVs committed here; zensim plan +
scorecard updated; any default/ship wiring is USER-GATED as always.

## CHUNK-2 DESIGN RESOLUTION (2026-08-27, before implementation)
- **Recon source**: `EncodePipeline::last_recon10_final` — the post-filter
  10-bit reconstruction, "what a conforming decoder outputs, bit-exact"
  (issue #13's surface, landed by the parity lane). Requires
  `with_recon_output(true)` + a complete bd10 recon; ALL NINE frozen
  instrument renditions are 64-aligned (verified: 1536x2048, 1024x768,
  768x1024, 384x512, 512x384 — every dim % 64 == 0), satisfying the bd10
  consumer envelope, so `None` recon = a loud error, never a fallback.
- **Judge domain**: PQ CODE VALUES end to end — the corpus refs are 16-bit
  PQ PNGs and the recon is 10-bit PQ codes; zensim's `foldapphdrpq`-class
  HDR entry ingests PQ code values directly. The only conversion in the
  trial cell is BT.2020nc LIMITED-range YUV420→RGB in code-value domain
  (the standard matrix; unit-tested against reference vectors + a
  round-trip through the fleet's to_yuv420_bd10 on a synthetic ramp).
  No PU/nits math is re-implemented in this crate — the judge owns it.
- Chunk 2 deliverable: `trial.rs` (encode+recon+convert+judge closure
  builder) + the ramp round-trip test + ONE real-cell smoke (smallest
  tier ref, t80, k1) before phase B census.

## CHUNK-2 OPEN FORK (recorded 2026-08-27; decided before implementation, not hacked)
The in-loop score model: zensim's HDR entry points are FEATURE extractors
(`compute_folded720_append2_features_hdr` → 944 slots) + a bake forward
(`score_features_with_profile`); but the SHIPPED HDR bake (BHdr family) is
372-class over the OLDER pu-linear front-end, and **no 944-HDR bake exists
yet** — hdr_v3mix@944 (orientation-gated today) is exactly its training
table, but training it is MODELS-lane work, not this crate's. Routes:
(a) DEFAULT: in-loop score via the pu-linear-372 front + shipped BHdr
    (matches the fleet's proven `--hdr` scoring semantics);
(b) a 944-HDR bake once the MODELS lane trains it on hdr_v3mix@944 —
    then the loop and the 944 extractor share one pass (preferred end
    state; swap is a registered follow-up, never silent);
(c) rejected: shelling the zenmetrics CLI from the loop (ownership
    inversion + process-per-trial cost).
Census judging stays INDEPENDENT of the in-loop score either way (the
registered `--hdr`-route judge on decoded output).

## CHUNK-2 FINAL SHAPE (2026-08-27): judge-as-closure
The route-(a) judge chain (PQ→nits→PU→372→BHdr) lives in fleet/example
code, not a clean zensim public entry — re-implementing it in-crate risks
judge drift, the exact hazard the census avoids. Resolution: the crate's
loop is **judge-agnostic** — `encode_to_target(source, target, opts,
judge)` owns EncodePipeline+recon+bracketed-search and takes
`judge: FnMut(&Recon10) -> Result<f64, E>`; the census HARNESS wires the
fleet-proven judge (and can also wire route (b) later with zero crate
change). The crate keeps ZERO metric dependencies; the BT.2020nc matrix
note from the earlier design moves to the harness with the judge.

## PHASE-B CENSUS RESULT (2026-08-27) — the baseline, CLOSED
Harness `rust/crates/svtav1-target/examples/zensim_census.rs`: judge =
shelled `zenmetrics score --metric zensim --hdr` per trial (drift-free);
in-harness math = the mirrored `to_yuv420_bd10` + its inverse
(round-trip-gated in-binary; the gate caught an out-of-gamut TEST pattern
on first run — physical-pattern fix, matrix unchanged) + a cICP splice
([1,16,0,1], mirroring the corpus refs) the judge's PQ gate requires.

| k | median \|err\| | ±2 hits | t70 | t80 | t88 | large/mid/small |
|---|---|---|---|---|---|---|
| 2 | 17.638 | 1/27 | 8.68 | 18.68 | 26.68 | 23.5 / 19.6 / 16.8 |
| 3 | **7.431** | 9/27 | 7.64 | **1.20** | 8.76 | 7.4 / 7.6 / 6.3 |

Blind-midpoint seeding over qp∈[1,63] dominates the error (t88 furthest
from the midpoint, worst at k2; one extra bisection halves the median) —
the family's seed-staircase/head levers have their HDR value proposition
QUANTIFIED here; those arms register separately with the family bars.
t88's k3 residual includes a reachability question (some scenes may not
reach 88 on this judge at qp=1) — held in the per-scene TSVs for the next
arm's registration. Cells + logs:
`/mnt/v/output/zenav1-svt/instrument-census-2026-08-27/`.

## SEED ARM S1/S2 — REGISTERED 2026-08-27 ~12:4xZ (FROZEN pre-fit)

The census quantified the blind-midpoint seed as the dominant error source
(k2 17.638 / k3 7.431). This arm builds the family's one-shot seed in the
sanctioned consts form (feedback_no_zenpredict_in_codecs).

**Fit rule (frozen before computing any constant):**
- Data: hdrgrid zenav1-svt cells, zensim from the ERA-B slice ONLY
  (`zensim_scores_by_judge_era.parquet`, judge era B-9dffa5ca), q→qp via the
  fleet's `svt_q_to_qp` (qp = round(63−q·63/100)); the 9 census instrument
  SCENES are EXCLUDED from the fit (all their renditions) — census honesty.
- Oracle per (rendition, t∈{70,80,88}): qp* = qp of the cell whose era-B
  zensim is nearest t (unreachable→qp*=1).
- **S1** = median qp* per t (3 consts). **S2** = median qp* per
  (t, pixel-count tercile) (9 consts). Seeds clamp to [1,63].
- Census phase-C: the SAME frozen 27-cell instrument + harness, seed table
  injected via `TargetOptions.qp_start`; k2 + k3; judge unchanged.

**Gates (frozen):** an arm PASSES if k2 median |err| ≤ 13.23 (≥25%
improvement over the censused 17.638) AND k3 median |err| ≤ 7.93 (baseline
7.431 + 0.5 tolerance — the seed must not hurt the 3-encode budget). ±2
hits + per-t reported. Ties between S1/S2 break toward FEWER consts (S1).
FAIL both ⇒ the anchor form is insufficient on this corpus; a learned head
registers separately (never ships inside the codec per the standing rule).

### Fit + census results — S1 PASSES BOTH GATES (4-5× margins); S2 loses the tie

Fit (25,171 era-B cells, 842 non-census renditions; `scripts/fit_zq_seed_hdr.py`):
**S1 = {t70→qp22, t80→qp13, t88→qp5}** (IQRs 19-22 / 11-14 / 1-6 — tight);
S2's terciles barely move the anchors (only t88-small 5→3, which HURTS below).

| arm | k | median \|err\| | ±2 hits | t70 | t80 | t88 |
|---|---|---|---|---|---|---|
| blind (censused) | 2 | 17.638 | 1/27 | 8.68 | 18.68 | 26.68 |
| **S1** | 2 | **3.306** | **11/27** | 3.43 | 3.33 | **1.04** |
| blind (censused) | 3 | 7.431 | 9/27 | 7.64 | 1.20 | 8.76 |
| **S1** | 3 | **1.513** | **20/27** | 1.77 | 1.95 | 0.95 |
| S2 | 2 | 3.427 | 8/27 | 3.43 | 3.33 | 3.70 |
| S2 | 3 | 1.771 | 17/27 | 1.77 | 1.95 | 1.65 |

**GATES: S1 PASSES both** (k2 3.306 ≤ 13.23; k3 1.513 ≤ 7.93) — an 81%
k2 improvement; the seeded 2-encode budget beats the blind 3-encode budget
2.2×; t88 (the blind seed's disaster, 26.68) collapses to ~1. S2 is worse
than S1 at every cell it differs (the t88-small tercile anchor overshoots) —
the frozen fewer-consts tie-break and the numbers agree: **S1 is the arm.**
Family reading: svt now matches the family pattern exactly (fitted seed pays
hugely where the baseline seed is weak — the weakest baseline in the family
got the largest win). Seeds: `benchmarks/zq_seed_s1_2026-08-27.tsv` (the
3-const one-shot, sanctioned consts form); cells committed
(`census_k{2,3}_s1.tsv`); harness seed injection via `TargetOptions.qp_start`
(7th arg). Wiring qp_start into any production default = USER-GATED. ⇒ **APPROVED + WIRED 2026-08-28** (explicit user yes, AskUserQuestion): `svtav1-target/src/seed.rs` — S1 anchors + linear interpolation clamped to [70,88], `TargetOptions::seeded(target)` = the canonical constructor (plain Default stays midpoint); anchor/interp/clamp/NaN tests.

