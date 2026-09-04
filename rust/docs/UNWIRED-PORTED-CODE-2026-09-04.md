# Unwired-but-ported code — cross-crate reachability sweep, 2026-09-04

Produced by a dedicated dead-code-detection lane (per `docs/WORKING-ON-THIS.md`
and the `ported-but-unwired-is-the-default-defect` finding). **Report only —
nothing here was wired.** All analysis edits were made in a throwaway jj
workspace and reverted before this doc was written; `git show --stat` on this
commit shows only this file.

## Method: whole-crate `pub` -> `pub(crate)` demotion + `cargo build --lib`

`svtav1-encoder`'s `lib.rs` declares every module `pub mod`, and nearly every
item in it is `pub fn`/`pub struct`/etc. — the whole crate is exported, so
`rustc`'s `dead_code` lint (which only fires on items unreachable *within* the
compilation unit, and treats every `pub` item as an unknowable, always-live
root) finds nothing. That is method 1 from the brief and it was tried first;
it produced zero warnings on a plain `cargo build`, confirming the brief's
prediction.

**What worked:** demote every top-level `pub fn|struct|enum|const|static|
type|trait|use` in `crates/svtav1-encoder/src/` to `pub(crate)` (sed, whole
crate, ~186 files), *except* `pipeline.rs`, `rate_control.rs`,
`entropy/obu.rs`, and `lib.rs` — the four files the real production
entry points live in (`EncodePipeline::{new,with_*,try_encode_frame_420,
encode_frame}`, `RcConfig`/`RcMode`, `ColorDescription`, the crate's `pub
mod` list). Those four stay untouched, so every item they call — however
deep — is reachable from a genuine `pub` root exactly the way
`svtav1::avif::AvifEncoder` and the `svtav1-target` crate call into this
crate in production. Every OTHER item in the crate becomes `pub(crate)`,
so `cargo build --lib -p zenav1-svt-encoder` (which excludes `tests/` and
`#[cfg(test)]` entirely — this is the load-bearing property, since
integration tests are exactly where "ported but unwired" code gets a false
sense of coverage) now gives real whole-crate reachability from the
production entry points, and `dead_code` fires precisely on what
`pipeline.rs` never reaches, transitively.

Gotcha hit and fixed: BSD `sed` (macOS, this box) does not support `\b` word
boundaries — `s/\bpub fn /.../ ` silently matched nothing and exited 0 (see
memory `bsd-tools-fail-silently`). Fixed by dropping `\b` and matching the
literal `"pub fn "` (safe here because `"pub(crate) fn "` never contains that
substring). Re-verified the substitution actually landed before trusting any
downstream result.

Command sequence (from `rust/`):
```
find crates/svtav1-encoder/src -name '*.rs' \
  ! -name lib.rs ! -path '*/pipeline.rs' ! -path '*/rate_control.rs' \
  ! -path '*/entropy/obu.rs' > demote_files.txt
# per file: sed -i '' -e 's/pub fn /pub(crate) fn /g' -e 's/pub struct /pub(crate) struct /g' \
#   -e 's/pub enum /pub(crate) enum /g' -e 's/pub const /pub(crate) const /g' \
#   -e 's/pub static /pub(crate) static /g' -e 's/pub type /pub(crate) type /g' \
#   -e 's/pub trait /pub(crate) trait /g' -e 's/pub use /pub(crate) use /g' "$f"
cargo build --lib -p zenav1-svt-encoder 2>&1 | tee build.log
```
Result: builds clean (after also demoting a handful of `pub use` re-exports
that E0364/E0365'd), **1650 `dead_code`-family warnings**.

### Validation against the brief's known cases

| known item (from `ported-but-unwired-is-the-default-defect`) | result |
|---|---|
| `nic_prune.rs` (three NIC prunes) | **still dead** — flagged (all 6 fns + `run_md_stages`, its only caller) |
| `copy_frame_mvs` / `motion_field_projection` / `setup_motion_field` | **now wired** — absent from warnings, confirmed live callers in `pipeline.rs` |
| `mfmv_controls` | **now wired** — absent from warnings, live callers in `pipeline.rs` + `inter_hdr_arm.rs` |
| `pd0_detector` | **partially wired** — the core `pd0_detector` fn is live; `pd0_detector_allintra` + its variance-normalisation support (`SbVariance`, `NormalisedVariance`, `accumulate`, `accumulate_fork`, `QpThScaling`) are dead |
| `port_sgr_search.rs` | **now wired** — zero warnings in the whole file |
| `md_nsq_motion_search` | **now wired** — live, called from `port_md/md_search.rs` |
| `write_sequence_header_obu` | out of scope (aom port, sibling repo) |

The method reproduces every case that is *still* true and correctly reports
every case that has since been fixed (no false positives on the known-good
set) — that is the validation bar the brief set, and it passes. It also
turned up substantially more than the brief's seed list; see below.

**Coverage note:** this pass covers `svtav1-encoder` only (178k of the
workspace's ~250k LOC, and where every item on the brief's seed list lives).
`svtav1-dsp` (46k LOC), `svtav1-types`, and `svtav1-cref` were not swept —
recommended as a fast follow using the same method (its module structure is
flatter, so the "keep pipeline.rs untouched" step probably isn't even
needed).

## Ranked table — top 10

"C calls it?" states whether C's call chain to this function is on a path
the port's *currently active test envelope* reaches (CQP/CRF still-image +
the in-progress inter/video-mode campaign), not merely "C has this code
somewhere."

| # | item | crate/file | C counterpart | reachable from entry point? | C calls it on tested path? | what wiring takes | impact |
|---|---|---|---|---|---|---|---|
| 1 ~~ranked 1~~ **FALSIFIED 2026-09-04 — see the correction below the table** | `compute_qdelta_by_rate`, `find_qindex_by_rate` | `svtav1-encoder/src/port_rc_process.rs:209,166` | `svt_av1_compute_qdelta_by_rate` (`rc_process.c:290`), inlined into `svt_av1_frame_type_qdelta` (`rc_crf_cqp.c:157`) | **No** — zero callers outside its own tier-1 parity test | **Yes** — `crf_qindex_calc` (`rc_crf_cqp.c:193`) → `adjust_active_best_and_worst_quality` (`:168`, called at `:355`) → `svt_av1_frame_type_qdelta` (`:177`), gated only by `if (!frame_is_intra_only(ppcs))` (`:175`) — fires on **every inter frame** in CRF/CQP, the port's only supported RC mode (`WORKING-ON-THIS.md` guard #2) | Call `compute_qdelta_by_rate` from wherever the port derives `active_worst_quality`/`base_q_idx` for a non-key frame (grep `rate_control.rs` / `pipeline.rs` for where `base_q_idx` is set per-frame); pass `best_quality`/`worst_quality` from the existing qindex bounds | Every inter frame currently gets **no** rate-factor qindex adjustment — base_q_idx is wrong on every tested inter frame the moment the campaign checks bytes past frame 0 |
| 2 | Global motion: `port_ransac.rs` (RANSAC fit), `port_global_motion.rs` (`refine_integerized_param`, `convert_model_to_params`, `warp_error`), `port_enc_mode_config/leaf.rs::derive_gm_level`, `port_enc_mode_config/ctrls.rs::set_gm_controls`, `port_entropy_inter/gm.rs` (`write_global_motion`, `write_global_motion_params`) | 6 files, see above | `global_motion.c` (`determine_gm_params`), `ransac.c` (`svt_aom_ransac`), `enc_mode_config.c:180` (`svt_aom_get_gm_core_level`), `entropy_coding.c:3001,3069` | **No** — none reachable from `pipeline.rs`; `port_global_motion.rs`'s own module doc: *"global motion affects presets 0..4, inter frames only… at presets >= 5 the frame header just writes `is_global = 0`"* | **Yes at presets 0-4** — squarely in the inter campaign's tested preset band | Wire `derive_gm_level`/`set_gm_controls` into the per-frame sig-deriv pass; call the RANSAC→refine chain from wherever ME candidates are gathered for inter frames; call `write_global_motion{,_params}` from the frame-header writer instead of always signalling identity | On presets 0-4, inter frames with real camera/background motion get no global-motion model at all — wrong `is_global` bit and wrong warp params vs C whenever GM would win |
| 3 | `port_entropy_inter/gm.rs::write_sgrproj_filter` (line 297) | same file | `write_sgrproj_filter` (`entropy_coding.c:4069`) | **No** — its only caller is its own trace test | N/A — **duplicate**, see below | Delete; the live path already exists | Doc hazard, not a byte gap (see duplicates) |
| 4 | `port_md/motion_mode.rs` (12 of 13 fns) + `inter_me/obmc_search.rs` OBMC kernels (`obmc_sad`, `obmc_variance`, `get_obmc_mvpred_var`, `obmc_refining_search_sad`, `obmc_full_pixel_search`) | 2 files | `product_coding_loop.c:6741-6825` (`warp_refine_stage`/`obmc_refine_stage`/`opt_non_translation_motion_mode`), `:1068-1173` (`obmc_trans_face_off`), `entropy_coding.c:1159-1195` (`motion_mode_allowed`), `mode_decision.c:297-492` (`inter_intra_search`/`pick_interintra_wedge`), `av1me.c` OBMC search | **No** — only `warp_cand` (a small helper) has any non-test caller; everything else, including the whole OBMC motion-search kernel set, is dead | **Needs a preset/block-size check** — `motion_mode_allowed` gates SIMPLE/OBMC/WARPED per block in C's inter candidate generation; likely reached whenever the inter campaign exercises non-trivial motion. Not yet measured which presets in the current test matrix trip it | Call `motion_mode_allowed` + the refine-stage functions from wherever inter MD candidates are generated (leaf_funnel or port_md/md_search.rs); wire the OBMC search kernels behind it | MOTION_MODE syntax + inter-intra compound are real AV1 features the candidate set is currently missing entirely on inter frames |
| 5 | `port_md/coding_loop.rs` (11 of 12 fns): `determine_best_references`, `perform_md_reference_pruning`, `compute_lpd0_cost_from_variance`/`lpd0_inter_best_variance`/`compute_lpd0_cost_inter`, `is_intra_bordered`, `get_enable_use_best_me`, `derive_me_offsets`, `check_spatial_mv_size`, `check_temporal_mv_size`, `eliminate_candidate_based_on_pme_me_results` | `svtav1-encoder/src/port_md/coding_loop.rs` | `product_coding_loop.c:65-116` (`determine_best_references`), `:3004-3092` (`perform_md_reference_pruning`), `:8247-8341` (LPD0 inter cost), `:8119-8136`/`:9310-9341`, `mode_decision.c:3407-3416` | **No** — only `clip_mv_on_pic_boundary` is live | **Needs verification** — `perform_md_reference_pruning` decides which of the (up to 7) reference frames MD actually searches per block; if this doesn't run, need to find what the port uses *instead* (check `leaf_funnel` for a substitute reference-pruning gate before assuming a total gap) | Confirm whether `leaf_funnel` has its own reference-pruning logic (parallel to the `nic_prune`/`leaf_funnel::nic` pattern in item below) before wiring — may be a duplicate, not a gap | Reference-search candidate set may not match C's pruned set on inter frames — RD-neutral at best, wrong search cost at worst |
| 6 ~~suspected active bug~~ **MEASURED 2026-09-04 — one real transcription defect, latent on the grid; fixed, pinned, landed (see `docs/INTER-ENCODE-PLAN.md` §1z³⁵)** | `depth_refine.rs::skip_by_recon_dist` modulated `max_part0_to_part1_dev` by `Cand::mode` (the intra y_mode, 0 on every inter winner) where C switches on the unified `block_mi.mode` (`product_coding_loop.c:9867-9895`); the other three gates were not shown to differ | `svtav1-encoder/src/depth_refine.rs` (live) vs `port_md/nsq_skip.rs` (reference, still un-called) | `update_skip_nsq_based_on_sq_recon_dist` (`:9847`) | live copy: **yes** | **Yes** — but entered on an inter frame exactly ONCE across the 96-cell grid (`gradient 16x16 q20 p6`, parent NEWMV: threshold 73 -> 54 where the old arm gave 146), and the split-rate gate ahead of it kills the other 302 shapes | Done: `LeafEval::block_mi_mode()`, C's full 25-mode table, a 25x101 pin to `port_md::nsq_skip::modulate_by_parent_mode`, `tools/nsq_inter_reach_census.sh` | No grid cell moved (94/96 before and after); the arm is live for any inter parent that survives gate 1 |
| 7 | `port_picstruct.rs` scene-change + adaptive GOP structuring (~85 of 119 warned items: `perform_scene_change_detection`, `scene_transition_detector`, `set_mini_gop_structure`, `set_tpl_group`/`set_tpl_params`, `mctf_frame_decision`, `get_pred_struct_for_frame`, `update_pred_struct_and_pic_type`, and ~70 more) | `svtav1-encoder/src/port_picstruct.rs` | `Codec/pd_process.c` (picture-decision process) | **No** for the adaptive layer. The core RPS/DPB management in the same file (`is_pic_used_as_ref`, `update_dpb`, `set_ref_list_counts`, `generate_rps_info`, `prune_refs`, `setup_skip_mode_allowed`, …) **is** live | C runs scene-change-driven mini-GOP splitting + TPL-lookahead group sizing on every multi-frame encode | **Large, multi-file feature, not a single call site** — this is the single biggest concentration of dead code by warning count (119) but represents an entire adaptive-GOP subsystem, not a quick wire. Flagged for visibility, ranked below quicker wins | Port likely uses a fixed/simple GOP structure today rather than C's scene-cut-adaptive one — explains any GOP-shape divergence on content with real scene cuts |
| 8 | `port_temporal_filtering.rs` (78 of 80 items) | same file | `temporal_filtering.c` | No | **Self-documented as correctly inert today.** The module's own header: TF is bit-affecting on the VIDEO-MODE KEY FRAME in RANDOM_ACCESS and inert in LOW_DELAY (measured 2026-08-31); the port's current inter-campaign envelope is LOW_DELAY | Wire the moment the campaign moves off LOW_DELAY | Not a bug today — listed for completeness, explicitly NOT the payload per the brief's definition (C doesn't call it on the currently-tested path) |
| 9 | `port_pass2_gop.rs` (57 of 60 items) | same file | `Codec/pass2_strategy.c` (GOP bit allocation, 2-pass) | No | **Self-documented as out-of-envelope.** VBR/CBR 2-pass is out of scope per `WORKING-ON-THIS.md` guard #2 (stills are CQP/CRF-only); module doc explicitly says the harness to drive `STATS_BUFFER_CTX` doesn't exist yet | Build the harness first; not a missing call site | Not the payload — acknowledged gap, not a forgot-to-wire bug |
| 10 | `port_enc_mode_config/encdec.rs` (24 of 29 warned items — the ones NOT already covered by items above, e.g. unused inter-candidate-reduction control variants) | same file | `enc_mode_config.c` sig-deriv | Partial — `md_nsq_motion_search_controls`, `md_subpel_me_controls`, `md_subpel_pme_controls`, `set_cand_reduction_ctrls`, `set_spatial_sse_full_loop_level` are live | Needs per-item check | Lower priority — the live/dead split here tracks items 4-6 above (motion-mode and NSQ controls that feed the dead consumers); likely resolves itself once items 4-6 are wired | Grouped here rather than re-investigated separately given overlap with higher-ranked items |

## CORRECTION 2026-09-04 — item 1 is a NULL, measured

The wiring chunk took item 1 first and it does not survive contact with a
probe. Full account: `docs/INTER-ENCODE-PLAN.md` §1z³⁴. In short:

* **"C calls it on the tested path: Yes" is wrong.** The row's citation stops
  one gate too early. `svt_av1_frame_type_qdelta` is reached only from
  `crf_qindex_calc`, and `svt_av1_rc_calc_qindex_crf_cqp` (`rc_crf_cqp.c:489`)
  calls that only `if (ppcs->tpl_ctrls.enable)`. `get_tpl`
  (`Globals/enc_handle.c:3657-3677`) returns 0 for `allintra`, for
  `aq_mode == 0` **and** for `pred_structure == LOW_DELAY` — the port's entire
  envelope. MEASURED with a new `-Wl,--wrap=svt_av1_compute_qdelta_by_rate`
  interposer (`SVT_QDELTA_OUT`): **0 calls** on `gradient 64x64 q40 p8
  frames=2 SVT_PRED_STRUCT=1` (the campaign grid's config verbatim), **1 call**
  on the `SVT_PRED_STRUCT=2 SVT_AQ_MODE=2` positive control.
* **The "impact" column is wrong even where C does call it.** The delta is
  added to `active_worst_quality` only; `crf_qindex_calc` returns
  `active_best_quality` (`:363`). The adjusted worst reaches `ppcs->top_index`
  (`:359`), read only by the recode loop, which `enc_handle.c:3744-3749` forces
  to `DISALLOW_RECODE` for CQP/CRF with `max_bit_rate == 0`. It cannot move
  `base_q_idx` in this RC mode at all.
* **The already-standing measurement said so.** 94 of the campaign's 96 cells
  are byte-identical on frame 1, and `base_q_idx` is a frame-header field.

Nothing was wired; `port_rc_process.rs` stays translated and pinned per §7 of
`WORKING-ON-THIS.md`, with its module header corrected in place.

**The method lesson, for the rest of this table:** "reachable from the entry
point?" was answered by whole-crate reachability, which is sound. "C calls it
on the tested path?" was answered by reading C's call chain, which is **not**
the same evidence tier, and item 1 is what that gap looks like. Treat every
"Yes" in that column as a hypothesis with a named probe, not a finding.

## Duplicate transcriptions spotted (beyond the seven already known)

1. **`write_sgrproj_filter`** — `entropy/lr.rs:250` (LIVE, called from `restoration.rs:1696,1722`) vs `port_entropy_inter/gm.rs:297` (DEAD, only its own test calls it). **`docs/WORKING-ON-THIS.md` guard #5c is stale**: it states *"the port's `write_sgrproj_filter` is at `svtav1_encoder::port_entropy_inter::gm`"* — that is the dead copy; the live one is `entropy::lr::write_sgrproj_filter`. Needs a doc correction in the same pass as any cleanup here.
2. **`nic_prune.rs`'s six functions vs `leaf_funnel::nic`** — already self-flagged by the file's own module doc as of 2026-09-03 ("Two implementations of one C function is a standing hazard"). Confirmed dead by this sweep; safe to delete per the file's own recommendation once `run_md_stages` (its sole, also-dead caller) is confirmed to have no other future use.
3. **`mv_err_cost` — four independent transcriptions**: `inter_mv_code.rs:464` (`(mv, ref_mv, rate: &NmvRate, error_per_bit)`), `md_subpel.rs:244`, `port_md/pme.rs:190` (`(mv, params: &MvCostParams)`), `intrabc.rs:916` (`(mv, ref_mv, tables: &MvCostTables, error_per_bit)`). Different signatures suggest these may legitimately be different C call sites (`intrabc.rs`'s own doc says it ports `svt_aom_mv_err_cost{,_light}` for the IntraBC half of `av1me.c`, a documented separate case) rather than four copies of one bug — **not asserted as duplicates, flagged for the wiring chunk to disambiguate** signature-by-signature against C before consolidating.
4. **`have_newmv_in_inter_mode` / `is_motion_variation_allowed_bsize` / `is_global_mv_block` — each transcribed 3 times**: private copies in `inter_mvp.rs` (286, 292, 298 — note: `is_global_mv_block` here is `pub` and IS the live one per the validation table above), and separate `pub(crate)` copies in `port_md/predicates.rs` (632, 644) and `port_entropy_inter/modes.rs` (111, 144). Not yet checked which of the `predicates.rs`/`modes.rs` copies are live vs dead-alongside-item-4/-6 above — flagged, not resolved, given the overlap with the already-large motion-mode investigation.

### Resolution log — the dedup chunk (2026-09-04)

Each cluster is folded to ONE body and gated on the whole grid after each
commit — `identity_full_8bit` 1100/1100, `regression_spotcheck` 102/102,
`inter_byte_gate` 96 required / 0 failed / 1 known-open, `video_key_matrix`
58/60, `fctx_gate` 96/96, `inter_decode_gate` 5/5, `inter_decode_census`
96/96, `SCAN_GATE=1 inter_completion_scan` 64/0/0, nextest, the six still
cells at 290/839/63/171/580/693 B, and the cross-ISA set on r7900x (x86_64:
nextest, spotcheck, `inter_byte_gate`, `identity_full_8bit`). "Byte-inert"
below means every one of those was unmoved by the fold.

3. `mv_err_cost` — **folded, `2b1a74ed`, byte-inert on both ISAs.**
   Disambiguated against C: `md_subpel::mv_err_cost` (mcomp.c
   `svt_mv_err_cost`, all six arms, tier 1 through the exported
   `svt_aom_fp_mv_err_cost`) is THE body. `port_md::pme::{mv_err_cost,
   fp_mv_err_cost}` were a second full body with their own `MvCostType` /
   `MvCostParams` / `MvCostTable` — now re-exports of `md_subpel`'s types
   (`MvCostTable` is an alias of `intrabc::MvCostTables`) and one-line
   forwards. `intrabc::mv_err_cost` (av1me.c `svt_aom_mv_err_cost`) is C's
   older name for the ENTROPY arm — a forward to it, still tier 1 against
   the real symbol. `inter_mv_code::mv_err_cost` was already a forward over
   `NmvRate`, not a body. The two retired copies differed from the body only
   at a per-component diff of exactly -16384 (where C itself reads one past
   the row and `is_valid_mv_diff` has already rejected the candidate) and in
   taking the difference in i32 instead of int16 (equal on every legal MV
   pair) — both unreachable, and the grid agrees (r7900x, x86_64: nextest
   2536/2536, spotcheck 102/102, inter_byte_gate 96/0/1, identity_full_8bit
   1100/1100). `svt_init_mv_cost_params`
   likewise has one transcription now (`port_md::pme::init_mv_cost_params`;
   `md_search` and `inter_search_arm` re-derived it inline).

4. `have_newmv_in_inter_mode` / `is_motion_variation_allowed_bsize` /
   `is_global_mv_block` — **folded, `4fb78544`, byte-inert on both ISAs
   (r7900x x86_64: nextest 2536/2536, spotcheck 102/102, inter_byte_gate
   96/0/1, identity_full_8bit 1100/1100).**
   Verdict at the signature level: every copy is the SAME C function, the
   copies differ only in argument spelling (typed enum vs. the raw byte /
   raw index C's `mi` grid holds) — fold, nothing to record as different.
   One body each now: `inter_mv_code::have_newmv_in_inter_mode_raw(u8)`
   (the typed function forwards; `inter_mvp` / `intrabc_mvp` `use` it),
   `port_entropy_inter::modes::is_motion_variation_allowed_bsize_idx(usize)`
   and `::is_global_mv_block_idx` (the BlockSize spellings and
   `port_md::predicates`' u8 spellings forward; `inter_mvp` `use`s them).
   Found on the way, recorded not folded: `port_entropy_inter::modes::
   TransformationType` is a second transcription of C's enum next to
   `svtav1_types::motion::TransformationType` (identical discriminants;
   seven files use the modes one) — the predicate body takes the
   `svtav1_types` one, the writers convert via `as_motion()`.

## What this changes about the brief's seed list

Of the 8 named examples in `ported-but-unwired-is-the-default-defect`, **5 are
now fixed** (motion field trio, `mfmv_controls`, `pd0_detector`'s core,
`port_sgr_search.rs`, `md_nsq_motion_search`) and **1 remains exactly as
described** (`nic_prune.rs`). This sweep's real contribution is the ~15 newly
identified modules/chains above it was not looking for — rate-control qdelta,
the entire global-motion chain, motion-mode/OBMC, and the suspected
active correctness bug in item 6.

## Next steps (for the wiring chunk, not this one)

Priority order for a wiring session, independent of the table's numbering
(which mixes "quick, high-value wire" with "large subsystem" and "needs more
investigation before any code is written"):

1. ~~Item 6 (nsq_skip vs depth_refine)~~ **Done 2026-09-04** — the differential
   found one transcription defect (the recon-dist gate's parent-mode read),
   fixed and pinned; entered once on the grid, no byte moved (§1z³⁵).
2. Item 1 (`compute_qdelta_by_rate`) — single clear call site, C's own
   comments say exactly where, tier-1 tests already exist.
3. Item 2 (global motion chain) — larger but self-contained; the port's own
   docs already state the exact preset/slice-type gate.
4. Items 4-5 (motion-mode/OBMC, coding_loop reference pruning) — largest
   code volume, needs the "what does the port do INSTEAD today" question
   answered per function before wiring (some may turn out to be duplicates
   of an already-live `leaf_funnel` path, per the `nic_prune` precedent).
5. Items 7-9 — explicitly out of the currently tested envelope; not
   near-term work.
