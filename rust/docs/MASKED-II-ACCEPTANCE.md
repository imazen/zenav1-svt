> **Live acceptance criteria for one workstream**: wiring masked compound
> (WEDGE + DIFFWTD) and inter-intra into executable inter mode decision.
> Scoped — it sits under [ACCEPTANCE-CRITERIA.md](ACCEPTANCE-CRITERIA.md) and
> [ENCODER-POLICY-GOAL.md](../../ENCODER-POLICY-GOAL.md) and defines "done" for
> this task only. Each criterion names its evidence; a criterion without its
> evidence is open, not met.

# Goal: masked compound + inter-intra on the inter MD path

Make the already-ported masked-compound and inter-intra DSP primitives
**reachable**: searched at injection, predicted at `predict_and_price`,
priced, carried through arbitration and commit, packed into the bitstream,
and rebuilt identically at IFS/MDS3 — with C-faithful search semantics and
decoder-verified output on every shipped configuration.

Verification model follows the project split: video is **decoder-verified**
(`video_selfcheck`/`bd10_video_selfcheck` — encoder recon == `aomdec`), not
byte-compared to C; stills stay **byte-identical** and this work must not
perturb them.

## Pinned C contract (what the implementation must match)

- **`hbd_md` raw-vs-mapped split.** `calc_pred_masked_compound`,
  `pick_wedge`, `pick_interinter_seg` map `EB_DUAL_BIT_MD` → 8-bit locally;
  `inter_intra_search`, `pick_wedge_fixed_sign`, and `model_rd_with_curvfit`'s
  dequant pick read `ctx->hbd_md` raw. Port mapping: `!bd.active` → 0;
  `full_rd10 && preset <= -1` → 1; `full_rd10 && preset >= 0` → 2.
- **`cmp_store`.** MV-keyed per-ref luma unipred at `interp_filters=0`,
  4-deep per list, DUAL→8-bit depth; reset per ref-pair in
  `inject_mvp_candidates_ii`, per `inj_comp_modes` call everywhere else.
- **`seg_mask` lifetime.** DIFFWTD mask built on luma during the second
  reference's prediction, reused for chroma — never rebuilt per plane.
- **II precompute.** `enabled && is_interintra_allowed_bsize(bsize)`, once
  per block, 4 modes (DC/V/H/SMOOTH) at raw `hbd_md` depth; smooth-mode
  mapping via `interintra_to_intra_mode`. MD-time eval writes the residual
  into a same-sized src-shaped buffer, not the source stride.
- **Chroma on committed recon.** `inter_intra_prediction` re-runs intra
  pred per plane (same mapped mode); luma may reuse the precomputed buffer;
  `use_precomputed_ii` is suppressed at the MDS3 hbd bump.
- **Compound warp.** Each reference is evaluated against its own global-
  motion model — never a single shared warp.

## Criteria

| # | Criterion | Evidence |
|---|---|---|
| M1 | **Reachability** — masked-compound and II candidates are injected at exactly the preset/bit-depth/blocksize/class gates C's `set_inter_comp_controls`/`set_inter_intra_ctrls` produce, and are *selected* in emitted streams on content where C selects them | Instrumented per-mode selection counts on the video corpus; nonzero where enabled (anti-vacuous — a wired path that never fires is not evidence) |
| M2 | **Search faithfulness** — cmp_store semantics, DUAL→8-bit local maps, strict `<` tie-breaks, residual order (`residual1 = src − pred1`, `diff10 = pred1 − pred0`), wedge-index/sign derivation, and `pick_wedge_fixed_sign` all match the named C functions | Unit + `c_parity_*` tests that fail if the wiring is removed; C `file:line` citations in code |
| M3 | **Prediction parity** — masked compound uses the per-ref CONV_BUF path with the mask arm inside `enc_make_inter_predictor`; II combines inter pred with the precomputed intra buffer per C's blend; sub-8 chroma uses the existing `predict_inter_chroma_sub8` stitch | Decoder recon == encoder recon on cells that select these modes |
| M4 | **Carrier integrity** — `interinter_wedge_index`/`wedge_sign`/`mask_type`/`interinter_comp_type`/`comp_group_idx`/`compound_idx` and `is_interintra_used`/`interintra_mode`/`use_wedge_interintra`/`interintra_wedge_index` propagate injection → `predict_and_price` → `InterCandOut` → `InterCand` → `InterDecision` → commit → `InterModeInfo` pack → MDS3/IFS rebuild with no hard-coded defaults left in the rate path | The real wedge index reaches `fast_cost` (today's `interinter_wedge_index: 0` is removed); pack writes what the winner selected |
| M5 | **No silent drops** — `build_inter_candidates`'s refusal assertion stays armed for every class not yet predictable; a reachable-but-unsupported candidate aborts loudly rather than being filtered | Refusal strings remain truthful; `REFUSED-CONFIGS.md` regenerates only if a refusal actually moved |
| M6 | **Gates green** — `cargo nextest run --workspace --locked` + `tools/regression_spotcheck.sh` + every gate covering touched paths: `video_selfcheck_gate.sh` 270/270, `bd10_video_selfcheck_gate.sh` 396/396, `bd10_video_gate.sh` 24/24 byte-identical, stills identity unchanged (1100/1100, 191/191, 309/309) | Gate output in the landing commit's message or meta; numbers re-measured, not carried |
| M7 | **Decoder proof on firing cells** — cells where instrumentation shows masked/II selection must pass `aomdec` recon equality; a mode that only encodes but never decodes correctly is a defect | Selfcheck cells chosen to cover the firing content classes |
| M8 | **RD sanity** — video BD-rate measured before/after on the shipped envelope; masked/II is expected to help or hold even, and a regression is a defect to explain, not absorb | `benchmarks/` meta record with host, corpus, and commit pair |
| M9 | **Constraints** — `#![forbid(unsafe_code)]` preserved; allocations through `crate::vecpool`; MSRV 1.98; no loosened thresholds, hidden skips, or relaxed assertions | `cargo check` MSRV leg + review |

## Explicit non-goals

- **OBMC causal** stays where it is — a separate, still-open reachability gap
  (presets −1/0/1). This task must not weaken its refusal.
- **Selection-count parity with C** is not the gate. Video is decoder-verified
  by policy; what must match C is the *search* (M2), not a byte-for-byte
  candidate sequence on multi-frame input.
- **Hierarchical/RA GOPs, temporal filtering, VBR/CBR** — untouched; their
  refusals and matrix rows stay.
- **Stills paths** must be bit-for-bit unchanged — the tools are inter-only
  and stills identity is the anti-regression witness.

## Landing checklist

- [ ] All M1–M9 criteria met with evidence attached
- [ ] `VIDEO-PARITY-MATRIX.md` masked-compound / inter-intra rows moved to
      measured state
- [ ] `DEVIN-CHANGE-LOG.md` entry written
- [ ] `tools/portnote_index.sh --check` clean; no new unverified markers left
      unexplained
- [ ] Commit pushed and confirmed on `origin/main` with
      `git merge-base --is-ancestor`
