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
