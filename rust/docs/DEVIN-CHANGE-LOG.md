# Devin change log — audit ledger

Maintained list of changes Devin (Cognition) has landed in this repository,
written for Astra (or any auditor) to review without re-deriving intent from
diffs. This file is a **living ledger, not a snapshot** — append one entry per
landed change, newest at the top. Do not back-edit entries except to fix a
factual error; a superseded decision gets a new entry, not a rewrite.

Scope: changes to encoder behavior, tooling, gates and docs made by Devin.
Changes by other authors are not listed here.

## Entry format

```
## <date> — <short title> — <commit sha>
- What: files/symbols touched, in one or two lines.
- Why: the decision or measurement that motivated it.
- Verified by: gates/tests run and their counts (the ledger, not prose).
- Audit surface: what a reviewer should look at first — parity-sensitive
  regions, refusal strings, new refusals, extension vs parity claims,
  dead-looking code kept on purpose, generated files that must stay in sync.
```

---

## 2026-09-21 — 4:4:4 inter frames on the decoder-verified envelope — `d61238517`

- What: `RefFrameCtx` gains `uv_padded` + `ss_x`/`ss_y`;
  `predict_inter_chroma_*` parameterized by subsampling; new
  `partition::encode_chroma_block_pred` residual-codes chroma against the
  block's own motion-compensated prediction (no `uv_mode`); the
  `encode_frame_impl` envelope gate drops `!is_key`;
  `EntropyCtx::inter_mvp_fields` now derives `overlappable_neighbors` and
  `num_proj_ref` at write time; the recon-only walk keeps chroma TXB eobs;
  the non-funnel inter leaf drops OBMC blending and stamps `DC_PRED` as
  its committed intra_mode. New tooling: `examples/probe_444_dup.rs`,
  `examples/probe_444_video.rs`, `tools/chroma_444_inter_gate.sh`.
- Why: inter is the next cell of the 4:4:4 envelope — C refuses non-4:2:0
  at `enc_settings.c:470`, so the oracle stays decoder reconstruction
  equality. Two defects were found by decoding, not by reading code: the
  decoder recomputes `overlappable_neighbors`/`num_proj_ref` per block
  (decodemv.c `av1_findSamples` + `av1_count_overlappable_neighbors`) —
  hardcoded 0 omitted a `WARPED_CAUSAL`-allowed `motion_mode` symbol on
  every neighboured block and aomdec rejected frame-1 tiles; and the
  recon walk's dropped chroma eobs recorded `skip=1` where the bit walk
  wrote `skip=0`, desyncing the CDEF dlist (localized ±1-3 luma diffs,
  U/V exact). The write-time derivation feeds BOTH funnel and non-funnel
  paths — it is a shared correctness fix, not a 444 special case.
- Verified by: `chroma_444_inter_gate.sh` 47/47 (dup/rand/shift x
  64x64/128x64/64x128/128x128/256x128 x p{0,6,13} + 4-frame moving-content
  + 6-frame inter-on-inter chain — every frame's Y/U/V byte-identical to
  `aomdec`); `chroma_444_gate.sh` 13/13 stills; `cargo nextest` 3964/3964;
  `video_selfcheck_gate.sh` 270/270 4:2:0 inter cells (funnel-path
  regression check for the shared derivation); `refusal_inventory.sh
  --check` + `portnote_index.sh --check` current.
- Audit surface: the 444 envelope gate in `pipeline.rs` (8-bit, SB64, no
  superres — now key AND inter; 10-bit/SB128/superres/IntraBC/film-grain
  still refuse); `inter_mvp_fields`'s write-time derivations;
  `encode_chroma_block_pred`'s lossless (FWHT) arm; chroma LF/CDEF-uv/LR
  still signaled off; `REFUSED-CONFIGS.md` regenerated ("8-bit frames",
  no longer "still/key frames"). No C-byte-parity claim exists or is made
  for 4:4:4.

## 2026-09-21 — ZenEnhancement::DeepSearch ("deep-search-v1") — shipped, deepened + 8-bit envelope — `4283525f1` (delta; first wiring `370ee62cf`)

- What: `enhancements.rs` gains `DeepSearch`. When armed on all-intra
  4:2:0 8-bit, a single `md_preset = -1` binding inside `encode_tile_rows`
  re-points every SEARCH-EFFORT derivation at the research tier:
  `FunnelCfg::for_preset` (incl. `bypass_encdec`), `intra_arm`/`txs_arm`/
  `funnel_arm`/`nic_arm`/`encdec_arm`/`mds0_arm` applies,
  `depth_refine::DrCtrls::for_arm` (both sites), `rate_est_ctrls`,
  `update_cdf_level`, `NsqCfg::for_arm_with_coeff` (both sites),
  IntraBC level + `disallow_4x4` (incl. its preset fallback),
  `use_pd0`, `video_pd0_params`, `p9_fixed_partition`,
  `nsq_geom_enabled` x3, the p5 directional-search gate, the lossless
  leaf threshold (`coded_lossless && preset >= 4`), the bd10
  full-RD/funnel preset gates, and the all-intra `rdoq_level` floor
  (gated on `filter_chroma`). A second grep audit of
  `speed_config.preset` inside `encode_tile_rows` caught the pd0/
  partition/nsq-geom/lossless sites after the first wiring — every
  remaining `speed_config.preset` use in the function is intentionally
  native (lambda/rate/header/post-filter). The sequence-header bits
  that gate funnel-emitted symbols follow it:
  `enable_filter_intra` = `filter_intra_level(Allintra,-1)!=0`,
  `enable_intra_edge_filter` = `intra_edge_filter(Allintra,-1)` — set
  inside `seq_tools` construction, gated on the enhancement + allintra +
  420. Rate/lambda/QP/`native_preset`, Wiener/SGR/restoration and the
  other preset-derived header bits keep the CALLER's preset. Env hook
  `SVTAV1_DEEP_SEARCH=1`; gate `tools/deep_search_gate.sh`; `s` arm in
  `frontier_sweep.sh`.
- Why: the measured frontier point — native -1 bytes at near-requested-
  preset time. At p8/q30 512x512 photo: default 5669B/39.46dB/359ms,
  deep 4605B/39.54dB/~500ms, native -1 4628B/39.89dB/~3200ms. The arm
  captures ~92% of -1's byte savings at ~5% of its time cost — the
  -1 recipe (restoration, slower rate model) buys the residual quality
  the arm deliberately leaves alone.
- Verified by: `deep_search_gate.sh` 8/8 on the final build (fires on
  photo AND screen at p8, both decode under `aomdec`,
  `SVTAV1_FINAL_RECON` == `aomdec` recon byte-exact, byte-inert at p-1,
  video and mono refuse). Unit: `enhancements::tests::
  deep_search_is_allintra_420_8bit_scoped_and_inert_at_native_minus1`;
  pipeline envelope: `real_encode::
  deep_search_refuses_everything_outside_still_420` (still-420 accepts,
  video/mono/444/10-bit refuse naming the arm). Deeper-clamp spot
  check: p10 5989→4276B (−28.6%), decodes clean.
- Measured (imazen-26 42-image subset x p4/6/8/10 x qp20-56, 672 s-arm
  cells on the FINAL deeper build, BD-rate vs default over
  SSIMULACRA2): p4 −3.98 med/−5.31 mean, p6 −12.16/−16.96,
  p8 −18.40/−23.83, p10 −22.93/−28.02 — the win grows with the preset,
  i.e. exactly where C prunes hardest. Essentially dominant: worst
  cell +0.20% (one web screenshot at p4); best −68% (violin plot p10).
  Class medians: photos −3.3..−12.7%, documents/brochures −21..−26%,
  screenshots/plots/clipart −15..−45%, patent scans −13.0 (the e-arm's
  worst class is the s-arm's win). TIME (clean paired re-time, idle
  box, 512x512 photo q30 p8): default 303ms, deep 731ms (2.4x),
  native -1 3091ms (4.2x deep) — the sweep's own ms column is
  load-polluted on BOTH sides (d cells ran beside the identity gate,
  s cells beside builds); use the paired number, not the column.
  On the same cell deep-p8 is 4276B — SMALLER than native -1's 4628B:
  the -1 search tier inside the p8 lambda/rate frame out-scores -1's
  own recipe there. Positive control still holds on the final build:
  s-p8 and s-p10 streams are byte-IDENTICAL where checked (caller-side
  rate/lambda derivations saturate at p8+, and both share md=-1
  search) — while s != native -1 bytes by design.
- Audit surface: THE desync fix — the two seq bits in `seq_tools` (a p8
  stream that signals `enable_filter_intra=0` while the funnel emits
  `use_filter_intra` symbols desyncs the decoder; found by decode, fixed
  before any claim). `md_preset` is bound ONCE in `encode_tile_rows`;
  grep `deep_search` in pipeline.rs shows all sites. `validate` now
  takes `(preset, allintra, chroma_420, bit_depth)` — chroma arg is
  `self.chroma_420 && chroma.is_some()`: the configured format AND the
  frame's planes (refuses 444 where `is_some()` alone passed, and mono
  on a chroma-configured pipeline). The bit_depth arg was added after
  audit found `postpass_real_ctx` (the 10-bit MD post-pass) stays on
  the caller preset — rather than half-arm 10-bit, the arm refuses it:
  "deep-search-v1 is measured for all-intra 4:2:0 8-bit only".
  `identity_run` env block extracted to `apply_enhancement_env` shared
  by still + multi-frame pipelines — previously enhancement flags were
  silently DROPPED on the `SVTAV1_FRAMES>1` path (a real tool defect:
  the video refusal could never fire). Same `enhancements.rs`
  inventory gap as the Aom* arms.

## 2026-09-21 — ZenEnhancement::AomScreenTools ("aom-screen-tools-v1") — shipped, engagement narrowed

- What: `enhancements.rs` gains `AomScreenTools`; `pipeline.rs` runs the
  screen-content detector at every preset on the allintra arm (same
  `min(7)` clamp as an SCM-3 force) and, when `sc_class5` fires AND the
  allintra ladder left BOTH palette and IntraBC at 0 (the M8+
  tool-dead zone), substitutes the VIDEO arm's levels for the same
  preset index, clamped at its last nonzero row (palette M10's,
  IntraBC M9's). `identity_run` env hook `SVTAV1_SCREEN_TOOLS=1`.
  Gate `tools/screen_tools_gate.sh`, sweep `tools/frontier_sweep.sh`,
  probe `examples/probe_sc.rs`.
- Why the both-dead gate: the first fill-semantics (fill ANY level the
  allintra ladder left at 0, never lowering a live one) measured a
  regression at p6 — +1.95% mean BD over the 42-image sweep (patent
  scans +7.9% class mean, clipart +3.5%, EPA +2.2%) because adding
  IntraBC-5 on content palette-7 already handles costs more than it
  saves. At p8+ (both dead) the same fills win: −2.87% mean p8,
  −5.94% mean p10 overall; documents/screenshots −13% to −23% median.
  Narrowed so the arm is byte-INERT at p<=7 by construction (verified:
  p6 stream byte-identical to default on a detected screenshot).
  Residual honest caveat: inside the engagement zone, noisy grayscale
  patent scans regress ~+8-10% BD (detector fires, tools don't pay);
  measured, documented, arm stays opt-in.
- Measured (imazen-26 42-image subset, 4 presets x 4 qps, p8/p10 rows):
  p8 −2.87% / p10 −5.94% mean BD overall; 5000-nps-brochures −18.4/
  −37.9 med/mean at p10, 5300-noaa-docs −33.2/−41.5, 8000-screenshots
  −13.1/−26.1, 7000-plots −5.4 mean; all photo classes +0.00 (inert).
  C-faithful machinery, libaom-style availability: C's own video
  ladders supply the levels.
- Verified by: `screen_tools_gate.sh` 8/8 (fires+decodes on imessage/
  terminal/gui at p8; byte-identical on baby-lossless photo and
  codec_wiki sc5-miss); p6 byte-inertness verified at scale — all 168
  e-p6 sweep cells byte-identical to d after the narrowing; 42-image
  frontier sweep, 2016 cells total at
  `/tmp/frontier_sweep/frontier.tsv`; workspace nextest 3964/3964 and
  regression_spotcheck 145/145 on the final build.
- Audit surface: the substitution only runs when `sc_arm == Allintra`
  AND `classes.sc_class5` AND both allintra levels are 0 — read
  `pipeline.rs` near `derive_sc`. The 4:4:4 `allow_intrabc=false`
  override still wins after it. NOTE: forcing detection on also lets
  `sc_class0..3` derivations engage — identical semantics to what
  `screen_content_mode=3` (tune IQ) does; that is intended.
  `tools/refusal_inventory.sh` does NOT scan `enhancements.rs`, so
  this arm's `validate` refusal (like all prior `Aom*` ones) is absent
  from `docs/REFUSED-CONFIGS.md` — a pre-existing inventory gap, not a
  new inconsistency. TIME CAVEAT same as the deep-search entry — the
  ms column is load-inflated; bytes/ssim2 are the measured axes.

## 2026-09-21 — 4:4:4 chroma on a measured decoder-verified envelope — `f06ef09f3`

- What: `ChromaFormat` generalized to `(ss_x, ss_y)` everywhere —
  `plane_block_size` lookup (C `svt_aom_ss_size_lookup`),
  `is_chroma_reference`, multi-TXB chroma residual per plane-block in
  `encode_block_syntax`, format-derived strides/allocations in
  `encode_frame_impl` and `encode_tile_rows`. New committed tooling:
  `examples/probe_444*.rs`, `probe_mono_file.rs`,
  `tools/chroma_444_gate.sh`, `tools/i444_png.py`,
  `tools/rd_ext_sweep.sh`.
- Why: C refuses non-4:2:0 (`enc_settings.c:470`), so 444 ships as a Zen
  extension verified by decoder reconstruction, never byte parity.
  Coded-lossless needed three stacked rules: `cfl_allowed` requires the
  plane block be 4x4, TX_4X4 preempts every plane, and the lossless
  transform is Walsh-Hadamard (FWHT/transpose/quantize_b/IWHT in
  `encode_chroma_block_dc`, mirroring `lossless_mono`) — the generic
  chroma path had emitted DCT coefficients.
- Verified by: `chroma_444_gate.sh` 13/13 (aomdec AND dav1d, recon
  equality incl. lossless); nextest 3961/3961; regression_spotcheck
  145/145; `identity_full_8bit` 1100/1100 byte-identical;
  `rd_ext_sweep.sh` 396 cells — 0 failures, monotonic SSIMULACRA2,
  U/V PSNR +1.3..+15.3 dB vs own 420 at matched qp.
- Audit surface: the envelope gate in `pipeline.rs` (8-bit, key/still,
  SB64, no superres — everything else refuses); `sc_derivation
  .allow_intrabc` forced false at 444; chroma LF/CDEF-uv/LR signaled off
  until ported; `docs/REFUSED-CONFIGS.md` regenerated (53 contract +
  11 capability refusals); README/IDENTITY-STATUS rows claim
  decoder-verified only. The 422 entry point is staged through the same
  generalized code but is NOT claimed supported.

## 2026-09-21 — Devin change log created — this file

- What: this ledger.
- Why: user request — persistent audit trail of Devin-authored changes
  for maintainability review by Astra.
- Verified by: n/a (docs only).
- Audit surface: format discipline (one entry per landed change, newest
  first, no rewrites).
