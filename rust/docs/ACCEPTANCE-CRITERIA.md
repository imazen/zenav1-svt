> **Two layers.** The section directly under this header is the **live
> workstream queue** — the scoped criteria for the work in flight and the
> order the deferred work lands after it. Everything below the divider is the
> **historical still-image baseline**: its "mono out of scope", still-only and
> priority-order statements were superseded by later user instructions, and
> the ~1.2× C target is an aspiration, not measured performance. The complete
> current objective is [ENCODER-POLICY-GOAL.md](../../ENCODER-POLICY-GOAL.md);
> implementation status is [CONTEXT-HANDOFF.md](../../CONTEXT-HANDOFF.md).

# Acceptance criteria — what "done" means for zenav1-svt

## Current workstream — masked compound + inter-intra on the inter MD path

Make the already-ported masked-compound and inter-intra DSP primitives
**reachable**: searched at injection, predicted at `predict_and_price`,
priced, carried through arbitration and commit, packed into the bitstream,
and rebuilt identically at IFS/MDS3 — with C-faithful search semantics and
decoder-verified output on every shipped configuration.

Verification follows the project split: video is **decoder-verified**
(`video_selfcheck`/`bd10_video_selfcheck` — encoder recon == `aomdec`), not
byte-compared to C; stills stay **byte-identical** and this work must not
perturb them.

### Pinned C contract (what the implementation must match)

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

### Criteria

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

### Implementation map — where the continuation session picks up

Written 2026-09-22 for the session continuing this work on `i265` (mem sizing
`--mem 16G` there per AGENTS.md; pin `perf stat` to a P-core, `taskset -c 2`,
per [WORKING-ON-THIS.md](WORKING-ON-THIS.md)). Everything below is verified
against the tree at `db0e7bf9f`; re-verify before editing — line numbers drift.

**Already done, do not redo:** all DSP primitives are ported and unit-tested —
`svtav1-dsp/src/{port_masked_compound,port_wedge_search,port_compound_prep,port_interintra,port_model_rd,port_masked_blend}.rs`,
plus the masked arm inside `enc_make_inter_predictor` in
`port_enc_make_pred.rs` (per-ref CONV_BUF, `seg_mask` built on luma and
reused for chroma, warp-leaf support). The injector hook surface
(`InjectHooks` in `port_md/inject.rs`) already calls
`inter_intra_search`/`calc_pred_masked_compound`/`search_compound_diff_wedge`
at the C-correct sites. `inter_intra_level` already reaches
`InterMdFrame` via `md_config_signals`. The rate layer already prices
`comp_group_idx`/`compound_idx`/`mask_type`.

**The actual work, in dependency order:**

1. `InterCandidate` (`port_md/inject.rs`) — add `interinter_wedge_index` and
   `interinter_wedge_sign` (C `interinter_comp.wedge_index`/`.wedge_sign`).
   `InjectCtx` still builds `inter_intra_comp_ctrls` as `Default::default()` —
   wire the real `set_inter_intra_ctrls` output.
2. `WarpHooks` in `inter_md_arm.rs` — three stubs to implement:
   `calc_pred_masked_compound` (returns `false`), `search_compound_diff_wedge`
   (no-op), `inter_intra_search` (no-op). `calc_pred_masked_compound` needs a
   `cmp_store` — MV-keyed, 4-deep per ref list, luma-only, `interp_filters=0`,
   DUAL→8-bit; reset per ref-pair in the MVP-II path, per `inj_comp_modes`
   call elsewhere. `inter_intra_search` needs the once-per-block intra
   precompute (DC/V/H/SMOOTH via the funnel's `predict_unit`/
   `predict_unit_hbd`, gated on `enabled && is_interintra_allowed_bsize`).
3. `predict_and_price` (`inter_md_arm.rs`) — add the masked-compound and II
   prediction arms for both u8 and u16, including chroma and the sub-8 stitch
   via `predict_inter_chroma_sub8` (compound is unreachable on sub-8:
   `allow_bipred` requires both dims > 4).
4. Carrier propagation — `InterCandOut` → `InterCand`
   (`leaf_funnel/types.rs`) → `InterDecision` (construction sites in
   `partition.rs`, `pipeline.rs`, `bd10_reencode.rs`) → `InterModeInfo` at the
   pack site. Kill the hard-coded `interinter_wedge_index: 0` in the
   `fast_cost` path.
5. IFS rebuild (`leaf_funnel/ifs.rs`) and MDS3 hbd arms must re-predict these
   candidates identically — `use_precomputed_ii` is suppressed at the MDS3
   hbd bump, matching C.
6. Keep the `build_inter_candidates` refusal assert armed for everything not
   yet predictable (OBMC included — it is Q1, not this task).

**Host note for i265:** it is a hybrid Core Ultra 7 265K — unpinned wall
clock moves several percent on core placement; M8's BD-rate/time record must
name the host and pin measurement threads.

### Non-goals for this workstream (queued below)

- **OBMC causal** — a different predictor surface (`obmc_pred_arm` + motion
  refinement), queued as Q1.
- **Selection-count parity with C** — video is decoder-verified by policy;
  what must match C is the search (M2), not a byte-for-byte candidate
  sequence. Not queued; remains governed by the baseline criterion.
- **Hierarchical/RA GOPs, temporal filtering, VBR/CBR** — orthogonal axes,
  queued as Q2–Q4.
- **Stills paths** must be bit-for-bit unchanged — the tools are inter-only,
  so any drift is collateral damage, not the feature. A tripwire, not a goal.

### Landing checklist

- [ ] All M1–M9 criteria met with evidence attached
- [ ] `VIDEO-PARITY-MATRIX.md` masked-compound / inter-intra rows moved to
      measured state
- [ ] `DEVIN-CHANGE-LOG.md` entry written
- [ ] `tools/portnote_index.sh --check` clean; no new unverified markers left
      unexplained
- [ ] Commit pushed and confirmed on `origin/main` with
      `git merge-base --is-ancestor`

## Follow-on queue — the deferred workstreams, in landing order

Once M1–M9 are met, the deferred work lands in this order. The ordering is
dependency-driven: each item reuses or depends on the surface the one before
it built.

| # | Workstream | Why this position | Acceptance shape |
|---|---|---|---|
| Q1 | **OBMC causal** (presets −1/0/1) | Same plumbing this workstream builds — `WarpHooks::obmc_motion_refinement`, the `obmc_pred_arm` driver, carrier fields, MDS3 rebuild. Cheapest reachability item; highest reuse while the plumbing is fresh | Same M-shape: injection gated to C's ctrls, decoder-verified on firing cells, refusal lifted only where the predictor is wired |
| Q2 | **Temporal filtering** | Pipeline stage ahead of MD — zero coupling to the inter toolbox, so it neither blocks on nor destabilizes Q1. Largest remaining video-quality win per unit of risk | Decoder-verified on the shipped envelope; enable gating matches C's preset derivation; measured BD-rate record |
| Q3 | **Hierarchical/RA GOPs + mfmv + TPL/AQ** | Structural change to picture control; benefits from a complete per-frame inter toolbox (bipred/masked-compound/OBMC are exercised far more under layered references) | Decoder-verified across presets that enable layered structures; per-layer reference correctness |
| Q4 | **VBR/CBR rate control** | Rate allocation distributes across temporal layers — landing before Q3 would mean redoing allocation once the GOP structure exists | Rate adherence vs C tolerances + decoder-verified streams |
| Q5 | **Long-run sequence validation** | Deferred gate: extended-length sequences exercising everything above; meaningless until the structural work is in | New gate script + recorded run |

Residual defects are not queued — they are fixed in-scope as found, per the
working rules. The known one at this writing is the u8 `chroma_pass` /
`sb_chroma_owned` right-straddle wrap.

---

## Historical baseline — the still-image acceptance document

The sections below are the original 2026-07 still-image criteria, preserved
for the global definition of "done" they carry (byte-identity, dual-witness
fork rule, evidence ranking, performance and reliability posture). Superseded
parts are flagged in the header above.

## The criterion

`zenav1-svt` is done when it is a drop-in replacement for SVT-AV1 v4.2.0 that emits
**byte-identical bitstreams** — not visually equivalent, not PSNR-matched, not "close
enough at high quality," but the same bytes — for every configuration a still-image
encoder can be asked for: every preset M0–M13, every qp across the full 0–63 range,
8-bit and 10-bit, 4:2:0 (see the chroma note below), arbitrary frame dimensions including odd
sizes and partial superblocks, every tile configuration, SB64 and SB128, and every
content class (uniform, gradient, photographic, screen/synthetic) — with the HDR/PSY
fork's features available as an explicitly gated mode that is byte-identical to the
*rebased* fork when on and byte-identical to *mainline* when off; all of it in safe
Rust (`#![forbid(unsafe_code)]`), deterministic across runs and thread counts, panic-
free on adversarial input, and within ~1.2× of C's wall clock. Anything short of that
is a defect with a `file:line`, not an accepted limitation.

## Scope, stated honestly

The envelope above is the **still-image / CQP KEY-frame** envelope — that is what the
port targets and what the harnesses measure. Inter-frame and multi-frame sequence
parity is a later, separately-scoped phase; it is not silently folded into "done."
Lossless is in scope but low priority. The standing priority order is: 10-bit and
arbitrary dimensions first, maintainability continuously, lossless later, performance
last.

### Chroma: 4:2:0 only — 4:4:4 / 4:2:2 / monochrome are OUT OF SCOPE (decided 2026-07-19)

Mainline SVT-AV1 v4.2.0 **cannot encode anything but 4:2:0**, so there is no C oracle to
be byte-identical *to*. `svt_av1_verify_settings` (`Source/Lib/Globals/enc_settings.c:470-473`)
rejects every other format unconditionally at encoder init:

```c
if (config->encoder_color_format != EB_YUV420) {
    SVT_ERROR("Only support 420 now \n");
    return_error = EB_ErrorBadParameter;   // init fails
}
```

That reject also covers `EB_YUV400` (monochrome). The profile-1 → 4:4:4 and profile-2 →
4:2:2 checks immediately below it (`:480`, `:485`), and the `EB_YUV444`/`EB_YUV422`
`ss_x`/`ss_y` handling in `pic_buffer_desc.c`, are **dead scaffolding** — unreachable past
the reject. Upstream has started the wiring but it is gated off.

Consequence: 4:4:4 / 4:2:2 / monochrome are **not port work** — they are blocked on upstream
enabling them. If upstream lands real non-420 support, re-scope this line and gate them
then. Monochrome output may still be *decode*-validated (aomdec) as a non-byte-parity
sanity check, but it is not part of "done".

## Two products, one baseline

Mainline parity is the baseline; the HDR/PSY fork is a **gated delta on top of it**.
This is not a stylistic choice — the fork is v4.1-based and is *not additive*. It makes
unconditional changes (the loop-filter guard, the uint16→double variance path) that
measured 0/36 parity against mainline (a 2026-07 measurement with no committed
artifact — the *conclusion* is what this section carries). So fork features land behind `SVT_HDR_MODE`,
rebased onto the v4.2 baseline, and each one needs **two witnesses**:

1. **Mode off** → byte-identical to mainline C. This is the anti-regression witness.
2. **Mode on** → byte-identical to the rebased fork-on-4.2 C build.

A feature that satisfies only one of those is not ported — it is a fork of a fork.

## How parity is validated

Every claim is differential against **real C**, never against our own transcription.
Evidence ranks, highest first: a real exported C function > a synthetic facade over a
real C function > verbatim transcription (which can carry shared bugs — a transcribed
oracle agreeing with transcribed code proves only that they were transcribed the same
way).

- **Kernel level** — `c_parity_*.rs` call the actual exported C symbol and compare over
  randomized and edge-case inputs.
- **Stream level** — `identity_diff.sh` feeds one `.yuv` to both encoders and
  byte-compares the OBUs. On mismatch, the od_ec op traces localize the first diverging
  arithmetic symbol and classify the stage (SH / FH / tile-op).
- **Sweep level** — `identity_matrix.sh` is the pass/fail gate over synthetic content.
  `tile_gate.sh` covers the tile-configuration axis (both `TileRowsLog2` and
  `TileColsLog2`, on divisible and ragged SB grids), backed by the 162-cell
  `tile_map.sh` sweep; it additionally asserts, per cell, that the C oracle's
  bytes actually CHANGED under the tile request (anti-vacuity — both log2s are
  clamped to the geometry, so an unhonourable request silently yields a
  single-tile encode) and that aomdec accepts the port's stream whether or not
  it byte-matches. That decodability assert is not optional: a byte gate is
  structurally blind to corruption among expected-DIFF cells, and this axis had
  shipped exactly that — an out-of-range `context_update_tile_id` that every
  conforming decoder rejected, invisible because no gate had ever decoded a
  multi-tile stream.
  `real_image_matrix.sh` is the ratchet over real photographic and screen content,
  where divergences are findings rather than failures **until the end state, at which
  point it becomes a pass/fail gate too**.
- **Anti-vacuous witnesses** — every fix ships with a test that *fails without it*. A
  test that passes before and after the change proves nothing and is not evidence.
- **Landing verification** — every landing is confirmed on `origin/main` with
  `git merge-base --is-ancestor`. A report that something was pushed is a claim, not
  evidence; the two have been different before.

Prohibited, without exception: `#[ignore]` on a failing test, loosened thresholds,
commented-out assertions, runtime "graceful skips," and calling a stub complete. At the
end state no `PORT-NOTE(unverified)` remains unaudited — each is either differentially
tested or consciously carried with a written reason and a named risk.

## Performance

Performance is deliberately the **last** gate: a fast encoder that emits different bytes
is worthless. Once parity holds, the target is ≤ ~1.2× C wall clock at matched preset
and quality, approached in that order — algorithmic parity, then allocation discipline,
then SIMD. Measurement rules: an interleaved paired-statistics harness rather than
back-to-back isolated runs; no `-C target-cpu=native` (runtime dispatch is what users
get); results fitted as `total = intercept + slope · pixels` across tiny / small /
medium / large so per-call fixed cost never hides inside a "ms/MP" figure. Never
extrapolate a measurement from one size to another — measure the size you claim.
Memory numbers come from heaptrack or `time -v`, never from struct arithmetic.

**MEASURED 2026-08-16** (`tools/mem_gate.sh`, record
`benchmarks/mem_2026-08-16.meta`, reported in CI). Peak RSS, gradient qp32 p6,
aarch64:

| size | port | C | port/C |
|---|---|---|---|
| 64x64 | 3.5 MiB | 6.9 MiB | **0.51** |
| 512x512 | 12.0 MiB | 15.0 MiB | 0.80 |
| 1024x1024 | 37.1 MiB | 35.8 MiB | 1.03 |
| 2048x2048 | 122 MiB | 117 MiB | 1.05 |

The port uses HALF C's fixed overhead and slightly more per pixel; they cross
around 1 MP. This measurement is also the concrete case for the
never-extrapolate rule directly above it: least squares over 64..1024 gives
33.6 MiB/MP, over 1024..2048 gives 29.2 — quoting the small-range slope at 4 MP
over-predicts by ~18 MiB. NOT measured: allocation counts, transient peaks
between samples, bd10 (u16 buffers roughly double the pixel term — a prediction,
not a number), and Linux/glibc.

## Reliability

Safe Rust throughout, fallible allocation on untrusted paths, bounded memory. Bitstream
output is deterministic across runs, thread counts, and `--lp` settings — same input,
same bytes, every time. The fuzz corpus produces no panics, no OOMs, and no hangs, and
every fixed crash keeps a minimized regression seed in-tree. CI is green on every target
the crate claims, including `windows-11-arm`, macOS Intel, and `i686-unknown-linux-gnu`
(32-bit correctness is not optional — it is what catches pointer-width bugs and keeps
WASM viable).

Maintainability is part of the deliverable, not overhead. The port's durable value is
that a future maintainer can trace any decision back to a C `file:line`; the C
citations, the `docs/*-port-map.md` files, and the Known-Bugs log are load-bearing. A
byte-identical encoder nobody can safely modify has a short shelf life.

## Summary gate table

| Gate | Criterion | Evidence |
|---|---|---|
| G1 Mainline parity | Byte-identical OBUs vs `SvtAv1EncApp` v4.2.0 across the full still-image matrix | `identity_matrix.sh` + `real_image_matrix.sh` both pass/fail green; per-axis gates (`partial_sb_gate`, `bd10_*`, `sb128_gate`, `tile_gate`) green |
| G2 HDR mode | Mode off == mainline C; mode on == rebased fork-on-4.2 C | Both witnesses per feature |
| G3 Kernel parity | Every kernel differentially tested vs the real exported C symbol | `c_parity_*.rs` |
| G4 Performance | ≤ ~1.2× C wall clock, matched preset/quality | Interleaved paired-stat harness, intercept + slope reported |
| G5 Reliability | Deterministic, panic-free, memory-bounded, CI green incl. arm64-Windows / macOS-Intel / i686 | Fuzz corpus + determinism runs + CI |
| G6 Verification debt | Zero unaudited `PORT-NOTE(unverified)`; no ignored/relaxed tests | Audit sweep at end state |
