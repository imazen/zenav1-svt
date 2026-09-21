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

## 2026-09-21 — ZenEnhancement::DeepSearch ("deep-search-v1") — work in progress

- What: `enhancements.rs` gains `DeepSearch`. When armed on all-intra
  4:2:0, a single `md_preset = -1` binding inside `encode_tile_rows`
  re-points every SEARCH-EFFORT derivation at the research tier:
  `FunnelCfg::for_preset` (incl. `bypass_encdec`), `intra_arm`/`txs_arm`/
  `funnel_arm`/`nic_arm`/`encdec_arm`/`mds0_arm` applies,
  `depth_refine::DrCtrls::for_arm`, `rate_est_ctrls`, `update_cdf_level`,
  `NsqCfg::for_arm_with_coeff`, IntraBC level + `disallow_4x4`, and the
  all-intra `rdoq_level` floor (gated on `filter_chroma`). The
  sequence-header bits that gate funnel-emitted symbols follow it:
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
- Verified by: `deep_search_gate.sh` 8/8 (fires on photo AND screen at
  p8, both decode under `aomdec`, `SVTAV1_FINAL_RECON` == `aomdec` recon
  byte-exact, byte-inert at p-1, video and mono refuse). Unit:
  `enhancements::tests::deep_search_is_allintra_420_scoped_and_inert_at_
  native_minus1`; pipeline envelope: `real_encode::
  deep_search_refuses_everything_outside_still_420` (still-420 accepts,
  video/mono/444 refuse naming the arm). Frontier sweep cells pending.
- Audit surface: THE desync fix — the two seq bits in `seq_tools` (a p8
  stream that signals `enable_filter_intra=0` while the funnel emits
  `use_filter_intra` symbols desyncs the decoder; found by decode, fixed
  before any claim). `md_preset` is bound ONCE in `encode_tile_rows`;
  grep `deep_search` in pipeline.rs shows all three sites. `validate`
  now takes `chroma_420 && chroma.is_some()` — the configured format
  AND the frame's planes (refuses 444 where `is_some()` alone passed,
  and mono on a chroma-configured pipeline). `identity_run` env block
  extracted to `apply_enhancement_env` shared by still + multi-frame
  pipelines — previously enhancement flags were silently DROPPED on the
  `SVTAV1_FRAMES>1` path (a real tool defect: the video refusal could
  never fire). Same `enhancements.rs` inventory gap as the Aom* arms.

## 2026-09-21 — ZenEnhancement::AomScreenTools ("aom-screen-tools-v1") — work in progress

- What: `enhancements.rs` gains `AomScreenTools`; `pipeline.rs` runs the
  screen-content detector at every preset on the allintra arm (same
  `min(7)` clamp as an SCM-3 force) and, when `sc_class5` fires, fills in
  palette/IntraBC levels ONLY where the allintra ladder left 0 — from the
  VIDEO arm's own ladder for the same preset index, clamped at its last
  nonzero row (palette holds M10's, IntraBC M9's). A level the allintra
  arm already set is never lowered (M4 keeps IntraBC 7, not video's 3).
  `identity_run` env hook `SVTAV1_SCREEN_TOOLS=1`.
  New gate `tools/screen_tools_gate.sh`, sweep `tools/frontier_sweep.sh`,
  probe `examples/probe_sc.rs`.
- Why: measured — on detected screen content at p8 (where C's ladders
  switch BOTH tools off and never even run the detector) the arm-on
  stream is 21–43% smaller at equal-or-better SSIMULACRA2
  (terminal q20: 18263B vs 32044B AND +1.2 ssim2). It is byte-inert on
  photos and on screen images where `sc_class5` does not fire —
  verified identical md5. C-faithful machinery, libaom-style
  availability: C's own video ladders supply the levels.
- Verified by: `screen_tools_gate.sh` 8/8 (fires+decodes on imessage/
  terminal/gui at p8; byte-identical on baby-lossless photo and
  codec_wiki sc5-miss); qp-curve hand check q10–q60 on 4 screen images;
  imazen-26 42-image frontier sweep running at
  `/tmp/frontier_sweep/frontier.tsv` when this entry was written.
- Audit surface: the substitution only runs when `sc_arm == Allintra`
  AND `classes.sc_class5` — read `pipeline.rs` near `derive_sc`. The
  4:4:4 `allow_intrabc=false` override still wins after it. NOTE:
  forcing detection on also lets `sc_class0..3` derivations engage —
  identical semantics to what `screen_content_mode=3` (tune IQ) does;
  that is intended. `tools/refusal_inventory.sh` does NOT scan
  `enhancements.rs`, so this arm's `validate` refusal (like all prior
  `Aom*` ones) is absent from `docs/REFUSED-CONFIGS.md` — a
  pre-existing inventory gap, not a new inconsistency. qp60 cells can
  land slightly worse (deep-low-quality territory); q10–q50 are the
  measured win zone. SWEEP CAVEAT: the first imazen-26 frontier run's
  e-arm cells at p4/p6 on sc5-images used an earlier substitute-always
  build — re-run those rows (delete from frontier.tsv; the script skips
  existing cells) before quoting them.

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
