# Unwired-but-ported code — svtav1-dsp / svtav1-types / svtav1-cref sweep, 2026-09-04

Fast-follow to `docs/UNWIRED-PORTED-CODE-2026-09-04.md` (the `svtav1-encoder`
sweep), which flagged `svtav1-dsp` (46k LOC), `svtav1-types`, and `svtav1-cref`
as not yet covered. Same method, same guarantee: **report only — nothing here
was wired.** Written as a sibling file rather than a new section in the
original doc because another agent (`wire1` workspace) was actively editing
that file's live sections while this sweep ran; appending here avoids a
collision. All analysis edits were made in a throwaway `jj` workspace
(`../zenav1-svt--dead2`) and never committed — `git show --stat` on this
commit shows only this file.

## Method, and how it differs from the encoder pass

`svtav1-encoder` is the workspace's single production consumer crate, so the
original pass could name four files (`pipeline.rs`, `rate_control.rs`,
`entropy/obu.rs`, `lib.rs`) as the "stay `pub`" root and demote everything
else. `svtav1-dsp`, `svtav1-types`, and `svtav1-cref` are **leaf libraries**
consumed *by* other crates in the workspace — demoting their whole `src/` and
building `--lib` in isolation only tells you what each crate calls
**internally**, not what its downstream consumers actually import. So for
each of these three crates the root set was built as **every item any
production consumer imports**, established by parsing (not eyeballing) `use
<crate>::...` statements — including `module::{a, b, c}` blocks, `module as
alias` imports with the alias resolved back to real item names per file, and
inline fully-qualified `<crate>::module::item(...)` calls — across the real
consumer set for each crate:

| crate under test | production consumers scanned | consumer file count |
|---|---|---|
| `svtav1-dsp` | `svtav1-encoder/src` (the only crate that imports `svtav1_dsp` outside its own tests — confirmed by grep, `svtav1-target` and the top `svtav1` crate do not) | ~186 files |
| `svtav1-types` | `svtav1-encoder/src`, `svtav1-dsp/src`, `svtav1/src` (`svtav1` re-exports 5 types items directly) | ~230 files |
| `svtav1-cref` | **every `tests/*.rs` file that imports it** in `svtav1-dsp/tests` and `svtav1-encoder/src` — this crate is `dev-dependency`-only (`publish = false`, its own doc says "test-only FFI harness"), so its "entry points" are differential-parity test call sites, not production code | 141 files |

The parser (Python, not `sed`) resolves `use crate::mod as alias;` +
`alias::item` pairs per file, `use crate::mod::{a, b, c};` blocks (including
multi-line), and bare `crate::mod::item` qualified paths, then unions the
item *names* (not full paths) into an allow-list. Every top-level `pub
fn|struct|enum|const|static|type|trait` in the target crate's `src/` is
demoted to `pub(crate)` **unless its name is in that allow-list**; `lib.rs`
(pure `pub mod` declarations) is left untouched in all three crates. This is
name-based, not path-based, so a same-named item in an unrelated module is
conservatively kept public too (fewer false "dead" positives, never false
wiring claims). Confirmed via three counter-examples below that the
allow-list is neither too loose (things it should flag as dead, it does) nor
so tight the crate fails to build for the wrong reason (E0446 "private type
in public interface" from a kept-public function returning a demoted type —
hit once, in `svtav1-dsp/src/port_sgr/mod.rs`'s `pub use tables::{ONE_BY_X,
SGR_PARAMS, X_BY_XPLUS1};` re-export, fixed by splitting it into a
`pub(crate) use` for the two non-entry constants and a `pub use` for
`SGR_PARAMS` alone).

Gotcha from the encoder pass (BSD `sed`'s `\b` silently matching nothing) does
not recur here — this pass never shells out to `sed`; the demotion is a
Python regex over line-anchored `pub <kind> <name>` prefixes, verified by
inspecting the diff counts the script itself reports before trusting any
build.

Command sequence (from `rust/`, one crate shown — `dsp`; `types`/`cref` are
the same shape with a different consumer-file set and a different
`-p <package>`):

```
# 1. Parse every `use svtav1_dsp::...` / qualified-path site in
#    crates/svtav1-encoder/src into a flat item-name allow-list (Python).
# 2. Demote every top-level `pub <kind> <name>` in crates/svtav1-dsp/src/*.rs
#    (excluding lib.rs) to `pub(crate)` unless <name> is in the allow-list.
cargo build --lib -j 4 -p zenav1-svt-dsp 2>&1 | tee build.log
```

All three built **clean (0 errors)** after the one `pub use` fix above.

## Per-crate results

| crate | dead_code-family warnings | entry-point items (allow-list size) | files touched by demotion |
|---|---|---|---|
| `svtav1-dsp` | **514** | 237 (from `svtav1-encoder/src` imports) | 56 of 66 |
| `svtav1-types` | **217** | 128 (from `svtav1-encoder` + `svtav1-dsp` + `svtav1` imports) | 19 of 17 modules (incl. `tables/`) |
| `svtav1-cref` | **418** | 503 (from 141 `tests/*.rs` files across `svtav1-dsp` and `svtav1-encoder`) | 20 of 24 modules |

`svtav1-cref`'s numbers mean something different from the other two: it is a
dev-dependency-only FFI shim over the C static library, so "dead" there means
"no differential test anywhere in the workspace exercises this C-struct
field or wrapper function" — a **test-coverage gap in the oracle**, not a
production wiring gap. Read its section below with that distinction in mind.

---

## `svtav1-dsp` — ranked table, top 10

Same "C calls it?" convention as the encoder doc: whether C's call chain to
the function is on a path the port's currently active test envelope (still
CQP/CRF + the in-progress inter/video-mode campaign) reaches, not merely
"C has this code somewhere."

| # | item | file | C counterpart | reachable from entry point? | C calls it on tested path? | what wiring takes | impact |
|---|---|---|---|---|---|---|---|
| 1 ~~ranked 1~~ **WIRED 2026-09-04 — `leaf_funnel::ifs::ifs_at_mds3`; the RD claim in the last column was falsified by measurement (see `docs/INTER-ENCODE-PLAN.md` §1z³⁶)** | `interpolation_filter_search` (+ `filter_sets`; `svt_aom_simple_luma_unipred` is the TF wrapper and stays un-called) | `port_ifs.rs` — now called from `svtav1-encoder/src/leaf_funnel/ifs.rs`; the C function is `static` so there is still no `c_parity_ifs.rs`, and the evidence is the per-candidate join against the exported caller (`SVT_IFS_OUT` + `tools/ifs_join_gate.sh`: 96/96 cells, 330 joined, 0 mismatches) | `interpolation_filter_search` (`enc_inter_prediction.c:2058`) <- `svt_aom_inter_pu_prediction_av1` (`:3803`, via `product_prediction_fun_table`, product_coding_loop.c:57) <- `full_loop_core` (`:6848-6853`) at the stage `ifs_ctrls.level` names — `IFS_MDS3` for every video-arm preset the port accepts (enc_mode_config.c:9083-9098) | **Yes** — once per MDS3 inter candidate | **Yes** — 367 MDS3 candidates on the 96-cell grid's frame 1 | Done (`SearchFrameCfg::ifs_level`, `InterMdFrame::ifs`, `tune::sharpness_ifs`, the MDS3 hook, the interposer, the join gate) | MEASURED: every one of the 367 grid candidates has a FULL-PEL MV and C keeps `EIGHTTAP_REGULAR` on all of them, so the hardcoded 0 was byte-correct on the whole grid — what was missing was the switchable RATE C adds to `fast_luma_rate` at MDS3 (20-109 units per candidate), now paid. The sub-pel arm is transcribed but unreached on this envelope. |
| 2 | `svt_aom_inter_prediction` / `svt_aom_inter_pu_prediction_av1` / `inter_intra_prediction` / `inter_chroma_4xn_pred` | `port_full_pd1_pred.rs` (16/16 items dead) | same file as #1 | **No** | **Yes** — `mode_decision.c:364,1943`, `coding_loop.c:1167` | The port's live inter reconstruction path is `port_pd_pred.rs`'s `av1_inter_prediction_light_pd1`/`av1_inter_prediction_pd0` — a **named "light"/PD0 predictor**, not this full transcription. Confirm whether `light_pd1` is C's own `is_16bit`-fast-path variant (in which case this is correctly dormant, matching a real C fast-path split) or a port-invented simplification before wiring anything — the two modules' doc comments do not cross-reference each other. | If `light_pd1` skips a real C code path (masked-compound blend, OBMC overlap, inter-intra combine — see items 4-5), inter blocks that would take that path reconstruct differently than C. |
| 3 | `svt_aom_enc_make_inter_predictor` / `av1_make_masked_scaled_inter_predictor` | `port_enc_make_pred.rs` (13/6 warned — private helpers cascade too) | `enc_inter_prediction.c:2515,77` | **No** | **Yes** — `src_ops_process.c:808,896,998` (the SRC_OPS process, i.e. real per-frame source operations, not TPL-only) | Own doc says explicitly: *"ports this function's EXECUTABLE body; [`port_make_pred`] ports the DECISIONS"* — i.e. this is the wiring target once `port_make_pred.rs`'s dispatch (item below) is live. | Masked/scaled inter prediction (compound blending with scaling) never runs; `light_pd1`/`pd0` must be taking an unscaled, unmasked shortcut for every inter block. |
| 4 | `port_make_pred.rs` (dispatch over `(is_wm, is_masked_compound, is16bit)`) | same cluster | `enc_inter_prediction.c` (dispatch shell around #3) | **No** — 9/9 dead | **Yes** | Wire alongside #3 | Confirms #2/#3: the full `(is_wm, is_masked_compound, is16bit)` dispatch tree the DECISIONS are made from is entirely bypassed. |
| 5 | `port_masked_compound.rs` (`svt_aom_calc_pred_masked_compound`, `svt_av1_search_compound_diff_wedge`), `port_wedge_masks.rs` (wedge mask tables + `svt_av1_init_wedge_masks`), `port_interintra.rs` (`svt_aom_combine_interintra{,_highbd}`), `port_obmc_pred.rs`/`port_obmc_build.rs`/`port_obmc_data.rs`/`port_obmc_nb_pred.rs`/`port_obmc_single_pred.rs`, `port_model_rd.rs` (inter-candidate RD ranking) | 8 files, 24+29+18+24+14+15+7+5+23 = **159 dead items total** | `inter_prediction.c` / `enc_inter_prediction.c` masked-compound, wedge, inter-intra, OBMC families | **No** — every file is entirely or nearly-entirely dead | Predicated on `svtav1-encoder`'s already-known `port_md/motion_mode.rs` (12/13 dead) + `inter_me/obmc_search.rs` gap (item 4 of the encoder report) | **Downstream consequence, not an independent gap.** These are the DSP-side kernels for exactly the encoder-side motion-mode/OBMC/compound candidate set the encoder sweep already flagged as never injected. Wiring the encoder's candidate injection (motion_mode_allowed → SIMPLE/OBMC/WARPED/compound types) is the actual fix; these kernels are just waiting for candidates to reach them. Grouped as one item rather than nine because they share one root cause. |
| 6 | `sad_8x8`, `sad_16x16`, `sad_32x32`, `sad_64x64` | `sad.rs` | `svt_aom_sad{8x8,16x16,32x32,64x64}_c` with real dedicated SIMD (`aom_dsp_rtcd.c:288,302,304,312` — AVX2 **and** AVX512 for 64x64; `:670,684,686,694` NEON) | **No** — `sad::sad()` (generic, size-parameterized, entry-list-confirmed live) is what ME actually calls | **Yes**, but this is a *performance* fast-path, not a correctness gap — C's generic `_c` fallback and its dedicated per-size kernels compute the identical SAD value | Nothing to do for correctness. If perf work resumes (project priority order puts this LAST, per `CLAUDE.md` "4. Performance (#93): LAST"), these are pre-transcribed dedicated kernels for the four hottest ME block sizes, currently bypassed in favor of the generic path. | Zero correctness impact. Purely a `#93` perf item, deliberately deprioritized per the project's own binding order — noted for completeness, not urgency. |
| 7 | `fwd_txfm2d_{4x8,8x4,8x16,16x8,16x32,32x16,32x64,64x32,4x16,16x4,8x32,32x8,16x64,64x16,64x64}_dct_dct` (15 fns) + the mirror-image `inv_txfm2d_*_dct_dct` (15 fns) | `fwd_txfm.rs` / `inv_txfm.rs` | `transforms.c` per-size DCT_DCT specializations | **No** — the *square* small/medium sizes (4x4/8x8/16x16/32x32 DCT_DCT) ARE live, called directly from `svtav1-encoder/src/encode_loop.rs:125-131` (the presets ≤3 / partial-SB fallback partition path, `partition.rs` → `encode_loop.rs`, confirmed reachable from `pipeline.rs`) | Rectangular and 64x64 sizes: yes, wherever C's transform-type search picks a non-square DCT block | The GENERIC dispatch (`txfm_dispatch::fwd_txfm2d_dispatch`/`inv_txfm2d_dispatch`, both live, both entry-list items) calls a size-and-type-parameterized `fwd_txfm2d_c_exact`/`inv_txfm2d_c_exact` — **not** these 30 named functions. So every rectangular/64x64 forward+inverse transform the port performs already goes through the generic path; these 30 are a **duplicate transcription cluster**, complete but never called from anywhere except `encode_loop.rs`'s 4 square DCT_DCT cases (which is itself why those 4 ARE live) | None if the generic path is correct (it is C-parity-tested per `c_parity_txfm.rs`, all tiers). Delete-or-keep-documented per the "dead-looking C stays translated" rule — do not wire, they duplicate work the dispatch already does. |
| 8 | `port_convolve_hbd.rs` (`highbd_convolve_{x,y,2d}_sr`, `highbd_jnt_convolve_*`, `HbdKernel` dispatch) — 14/14 dead | `enc_inter_prediction.c` highbd reconstruction convolve family | **No** | Only on a 10/12-bit + inter combination | The **8-bit** twin, `port_convolve.rs` (`svt_av1_convolve_2d_sr_c` etc.), is **fully live** (0 dead items) — called from `port_pd_pred.rs`'s light-PD1/PD0 predictors. This CORRECTS a stale claim in `CLAUDE.md`'s own Inter/Motion DSP Audit note ("the reconstruction convolve `svt_av1_convolve_2d_sr_c`... is unported") — it is ported and wired now, just not its highbd twin. | 10/12-bit inter reconstruction has no wired motion-compensation convolve at all right now — a real bd10+inter gap, distinct from the encoder-side bd10 items already tracked. |
| 9 | `inter_pred.rs` (`convolve_horiz`, `convolve_vert`, `convolve_2d`, `convolve_copy`) — 13/13 dead | same file, doc explicitly distinguishes itself from #8/`port_convolve.rs`: *"the single-pass kernels that `svt_aom_upsampled_pred_c` uses for motion-estimation sub-pel refinement"* | `svt_aom_upsampled_pred_c`, called from ME subpel search | **No** | Yes, on the ME subpel path | Confirms and narrows `CLAUDE.md`'s existing "motion_est.rs — HOMEGROWN... BILINEAR subpel" note: the real C-exact subpel-refinement convolve is separately ported (this file, audited-and-verified per the same doc's own 2026-07-14 note) and sitting unused while ME uses a bilinear approximation instead. | ME subpel refinement quality only — not reconstruction correctness (reconstruction goes through the separately-wired `port_convolve.rs`, item 8's live twin). |
| 10 | `svtav1-dsp/src/cdef.rs`: `cdef_filter_block_8bit`, `cdef_find_dir_8bit`, `compute_cdef_dist_8bit` | `cdef.rs` (3 of 6 dead items; the 3 consts `CDEF_STRENGTH_BITS`/`CDEF_PRI_STRENGTHS`/`CDEF_SEC_STRENGTHS` are the rest, cosmetic) | `svt_cdef_filter_block_8bit`/`svt_aom_cdef_find_dir_8bit` (`SET_ONLY_C`, `aom_dsp_rtcd.c:839,841` — C itself has no SIMD arm for these, `_c` only) + `svt_compute_cdef_dist_8bit` (`SET_ONLY_C`, `:1007`), called from `cdef.c:532,634` and `cdef_process.c:257` — the CDEF **search/RD** phase, not the apply phase | **No** | Yes, on presets ≤6 (per `CLAUDE.md` guard 2a, the full live-block CDEF search band) | The port's CDEF search (already independently C-exact-verified per that guard) evidently reimplements search-phase filtering/distortion through the *generic* bd-parameterized `cdef_filter_block`/`cdef_find_dir` (both live, entry-list-confirmed) plus its own distortion accumulator, rather than calling these C-designated 8-bit-only search fast paths | None measured — CDEF search is independently verified byte-exact — but these three are a genuine duplicate-transcription triple worth naming since C itself treats them as a distinct RTCD entry, not a convenience wrapper. |

## Duplicate transcriptions found in `svtav1-dsp`

Beyond the nine already known and the encoder-side ones from the first pass:

1. **Rectangular/64x64 `fwd_txfm2d_*_dct_dct` / `inv_txfm2d_*_dct_dct`** (item 7 above) — 30 functions, one clean cluster, all dead in favor of the generic `fwd_txfm2d_c_exact`/`inv_txfm2d_c_exact` dispatch path.
2. **`sad_8x8`/`sad_16x16`/`sad_32x32`/`sad_64x64`** (item 6) vs the live generic `sad::sad()` — a real C RTCD split (C dedicates SIMD kernels to these four sizes), reduced to one generic path in the port. Perf-only, not correctness.
3. **`cdef_filter_block_8bit`/`cdef_find_dir_8bit`/`compute_cdef_dist_8bit`** (item 10) vs the live generic `cdef_filter_block`/`cdef_find_dir` + the port's own distortion accumulation.
4. **`variance()` / `variance_impl_scalar`** (`variance.rs`) dead, alongside the live `variance::sse`/`variance::variance_diff` — matches `CLAUDE.md`'s own pre-existing caveat ("`variance()` is an N²-scaled single-block helper, NOT the two-block `svt_aom_variance*`") — confirms it, not new.
5. **`highbd_iwht4x4_1_add`/`highbd_iwht4x4_add`** (`inv_txfm.rs`) dead vs the live `highbd_iwht4x4_16_add` — also matches an existing documented design choice (C forces `eob = max` unconditionally, so the eob-1 and generic-eob variants are correctly unreached), not new.
6. **`restoration::alloc_stripe_boundaries`** (dead) vs the live `alloc_stripe_boundaries_t` — flagged but **not confirmed** as a true duplicate (the `_t` suffix may denote a distinct tiled/wrapped variant rather than a second transcription of the same C function); needs a one-file read before acting on it.
7. **`intra_pred::predict_palette`** (dead) — no `mv_err_cost`-style quadruple here, but worth noting: palette PIXEL reconstruction exists as a standalone DSP kernel and is unreachable, meaning `svtav1-encoder`'s live palette path (search/RD/pack, all previously verified wired) must reconstruct palette blocks by some other, in-crate route rather than calling this. Not investigated further — flagged for whoever next touches palette.

No new instance of the `mv_err_cost`-style *N-independent-transcriptions-of-one-signature* pattern was found in `svtav1-dsp` — the closest candidates (the `fwd_txfm2d`/`inv_txfm2d` per-size cluster, the `sad_NxN` cluster) are better described as "one dispatch path, N unreachable named specializations" than "N competing hand transcriptions," since only one of each cluster is ever called and the rest were plainly written as C-mirroring completeness rather than independent reinvention.

### Resolution log — the dedup chunk (2026-09-04)

Gated per commit on the whole grid (the set is listed in the encoder
report's resolution log) plus the cross-ISA set on r7900x.

1. **`fwd_txfm2d_*_dct_dct` / `inv_txfm2d_*_dct_dct` (30) — kept as forwards;
   the one real second body in the cluster folded, `e0275930`, byte-inert on
   both ISAs (aarch64 full grid; r7900x x86_64: nextest 2536/2536,
   spotcheck 102/102, inter_byte_gate 96/0/1, identity_full_8bit
   1100/1100).** Measured by the demotion method: all 38 per-size DCT_DCT
   wrappers (the 30 plus the 4+4 square ones) are `never used` inside
   `svtav1-dsp`, and the 30 have no production caller anywhere (the 4+4
   square ones are `encode_loop.rs`'s). But they are not transcriptions —
   each is a one-line forward to the same `*_c_exact` generic the dispatch
   calls, i.e. the port's spelling of C's exported per-size entry points —
   and five tier-1 tests in `tests/c_parity_txfm.rs` name them
   (`fwd_named_square_wrappers_match_c`, `fwd_named_rect_wrappers_match_c`,
   `inv_named_rect_wrappers_recon_match_c`, `inv_txfm2d_recon_matches_c`,
   `inv_named_square_wrappers_flat_dc_match_c`). Per `WORKING-ON-THIS.md`
   §7 they stay and the tests stay where they are. What the cluster hid:
   the `mod_input` construction of C's `svt_av1_inv_txfm2d_add_64x*_c`
   (inv_transforms.c:2614-2733) was transcribed twice — `inv_txfm::
   mod_input_64` (packed input, the five named 64-dim inverse wrappers) and
   an inline copy in the LIVE `txfm_dispatch::inv_txfm2d_dispatch_bd`
   (full-stride input). `mod_input_64` now takes the input stride and both
   call it. Recorded, not changed: the 4+4 square `incant!` dispatchers
   whose three arms are the same one-liner into `_c_exact` (a no-op
   dispatch, not a second body; `benchmarks/neon_tier_audit_2026-08-07.md`).

2. **`sad_8x8` / `sad_16x16` / `sad_32x32` / `sad_64x64` — the four dead
   forwards removed, `a2d8ac46`, byte-inert on both ISAs (r7900x x86_64:
   nextest 2536/2536, spotcheck 102/102, inter_byte_gate 96/0/1,
   identity_full_8bit 1100/1100).** On arrival the "generic's redundant
   arm" was already gone — `sad::sad` had been reduced to a forward of
   `me_sad::block_sad` (the one SAD body) by an earlier pass — and the four
   named functions were one-line forwards of `sad::sad` with fixed dims,
   with no production caller and no C-parity test naming them
   (`tests/c_parity_sad.rs` / `tests/sad_neon_parity.rs` drive `sad::sad`).
   Their bench pairs and `sad.rs`'s own test call `sad::sad(.., 8, 8)`
   etc. under the same names. zenbench (r7900x, `kernel_tiers`, v3(avx2)
   vs forced scalar; `benchmarks/kernel_tiers_sad_dedup_2026-09-04*`):
   BEFORE 8x8 27.5/16.0 ns, 16x16 43.1/38.8, 32x32 83.4/114.5, 64x64
   189/311; AFTER 23.9/16.4, 40.5/41.1, 79.7/118.0, 195/309 — the same
   code either side, as expected of an inlined forward. Finding for a perf
   pass: on x86_64 the AVX2 arm of `block_sad` loses to the autovectorized
   scalar arm at 8 and 16 wide and only wins from 32 wide up.


### Final duplicate-fold summary (2026-09-04)

The chunk closed five clusters across both reports — this file's items 1
(`e0275930`) and 2 (`a2d8ac46`) plus the encoder report's items 3
(`2b1a74ed`), 4 (`24b7027e`) and a fifth found on the way, the per-pixel
variance about 128 (`448290c9`; three loop bodies in `port_src_ops`,
`sc_detect` and `tune` to one). **No fold moved a byte on either ISA** —
the full table, the per-cluster verdicts (item 4's three predicates were
the same C function in every copy; item 4 of this file's zenbench numbers
are repeated there), and the two duplicates deliberately left in place
(`port_md/nic_prune.rs`, the second `TransformationType` enum) are recorded
once, in `docs/UNWIRED-PORTED-CODE-2026-09-04.md` "Final duplicate-fold
summary". Of this file's other listed duplicates, 3-7 (the `_8bit` CDEF
trio, `variance()`, the `iwht` variants, `alloc_stripe_boundaries`,
`predict_palette`) were not touched: 4 and 5 are documented design
choices, not second bodies, and 3, 6 and 7 still need the one-file read
their entries ask for before anyone acts on them.

## On "unregistered SIMD arms" specifically

The brief asks for kernels whose ported SIMD arm the dispatch never selects.
This port does not have that failure mode in the form C has it: C's RTCD
tables (`common_dsp_rtcd.c`/`aom_dsp_rtcd.c`) select between **independently
hand-written** SSE4.1/AVX2/AVX512/NEON functions per kernel, so a real C-side
bug class is "the table points at `_c` when a `_avx2` exists." This port
instead generates every SIMD tier from **one** `archmage`/`magetypes`-
annotated generic body (`#[magetypes(_v4x, v4, v3, neon, wasm128)]`), so a
single call site dispatches all compiled tiers together — there is no
per-tier "forgot to register the fast arm" gap to find at the tier level.
What this sweep found instead is the tier-analogue one layer up: **whole
kernel FUNCTIONS** (not tiers within one function) that were transcribed
complete, multi-tier, and C-parity-tested, then never called from anywhere
production reaches — items 1-9 above are all this shape. Read the ranked
table's "what wiring takes" column as the SIMD-arm-equivalent finding.

---

## `svtav1-types` — summary (no top-10 table; see rationale)

217 dead items across 16 of 17 modules (`tables.rs` clean). No item-level
top-10 table is offered here because the pattern is uniform and a table would
just repeat one finding sixteen times: **the crate faithfully mirrors C's
header-level constants and enums for features whose consumers are already
known-dead from the encoder pass**, rather than naming any new gap.

| file | dead items | what they are |
|---|---|---|
| `reference.rs` | 34 | Reference-frame CDF context-count constants (`SKIP_CONTEXTS`, `REF_CONTEXTS`, `COMP_REF_TYPE_CONTEXTS`, ...), `CompReferenceType`/`RefList`/`PredDirection` enums |
| `prediction.rs` | 24 | `MotionMode`, `InterIntraMode`, wedge (`MAX_WEDGE_TYPES` etc.), CFL, palette-size constants |
| `frame.rs` | 24 | (not itemized individually — frame-header field types for features not yet emitted) |
| `constants.rs` | 19 | (general AV1 spec constants without a current consumer) |
| `bitstream.rs` | 19 | (bit-layout struct fields for unemitted syntax) |
| `restoration.rs` / `quantization.rs` | 14 / 14 | LR unit + quant-table types beyond what's wired |
| `transform.rs` | 13 | `TxMode`/`TxSetType` enums, `TxfmParam`, `EXT_TX_SIZES` family |
| `motion.rs` | 12 | **`GM_TRANS_PREC_BITS`/`GM_ABS_TRANS_BITS`/`GM_ALPHA_PREC_BITS`/`GM_ABS_ALPHA_BITS`/`GM_TRANS_PREC_DIFF`/`GM_ALPHA_PREC_DIFF`** — the global-motion precision constants |
| `segmentation.rs` | 11 | segmentation-map types beyond what's wired |
| `tables/{scan,transform,block,interp}.rs` | 8+5+5+3 | scan-order / transform-size / block-size / interp-filter lookup tables without a current caller |
| `block_mode.rs` | 3 | (small residue) |

**Every one of these traces back to an encoder-side gap the first pass
already named.** `motion.rs`'s `GM_*` constants are global motion (encoder
item 2). `prediction.rs`'s `MotionMode`/`InterIntraMode`/wedge constants are
motion-mode/OBMC/compound (encoder item 4, and this pass's item 5 above).
`reference.rs`'s CDF context-count constants belong to inter mode-context
derivation the encoder likely still computes from inline literals rather than
importing these named constants (not independently verified — a
maintenance/duplication observation, not a functional gap, since the
constant VALUES are what matter and inline literals can still be correct).
**No new independent gap was found in `svtav1-types`** — it is downstream of
what the encoder sweep already reported, one layer further from production.

---

## `svtav1-cref` — summary (test-coverage gap, not wiring gap)

418 dead items, heavily concentrated:

| file | dead items |
|---|---|
| `sig_deriv.rs` | 324 |
| `dlf.rs` | 48 |
| `cdef_search.rs` | 31 |
| `interpred_gap.rs` | 6 |
| `frame_cdf.rs` | 6 |
| `pic_operators.rs` | 2 |
| `md_subpel.rs` | 1 |

`sig_deriv.rs` alone is 78% of the total, and its dead items are almost
entirely a large enumerated table of C struct-field-offset constants (e.g.
`HBD_MD`, `R0_GEN`, `R0_MILLI`, `PCS_TEMPORAL_LAYER`, `TUNE`, `PICTURE_QP`,
`EXT_CRF_OFFSET`, `RDOQ`, `RATE_EST`, `CDF_MV`, `CDF_SE`, `CDF_COEF`,
`CDF_EN`, ...) — this crate transcribes far more of C's `sig_deriv`
configuration surface than any of the 141 differential test files currently
reads back out. **This is a test-coverage gap in the oracle harness, not a
production wiring gap** — `svtav1-cref` is `publish = false`, dev-dependency
only, and its own module doc says so explicitly. Read as: if a future session
wires one of the DSP/encoder items above and wants tier-1 (`c_parity_*`)
coverage for it, the FFI surface to read the corresponding C struct field
back out already exists here, untested but ready.

Two mid-size clusters worth a specific pointer for whoever picks up item 1
above (`port_ifs.rs`): **`dlf.rs`'s 48 dead items and `cdef_search.rs`'s 31**
are both larger than a "handful," and neither corresponds to anything on this
pass's DSP top-10 — they were not chased further (out of this pass's time
budget) but are flagged as the next two clusters worth a `grep`-first look
if a future dead-code pass extends into `svtav1-cref`'s own test-coverage
gaps specifically.

---

## Corrections to prior documentation surfaced by this sweep

1. **`CLAUDE.md`'s "Inter/Motion DSP Audit vs v4.2 C (2026-07-14)" section is
   stale** where it says *"the reconstruction convolve `svt_av1_convolve_2d_sr_c`
   ... is unported."* It is ported (`svtav1-dsp/src/port_convolve.rs`, 0 dead
   items) and wired (`port_pd_pred.rs`'s light-PD1/PD0 predictors call it).
   The **highbd twin**, `port_convolve_hbd.rs`, genuinely is unwired (100%
   dead) — the stale sentence should be corrected to name that instead.
2. ~~**`svtav1-encoder/src/inter_md_arm.rs:708`'s comment**~~ **Done 2026-09-04**:
   the search is wired (`leaf_funnel::ifs`) and the comment now says where the
   filter is decided. A `c_parity_ifs.rs` at tier 1 is not possible — the C
   function is `static` and takes the whole MD context — so the evidence is
   the per-candidate join on its exported caller (`tools/ifs_join_gate.sh`),
   with the two pinned inputs (`svt_aom_get_switchable_rate`,
   `model_rd_from_sse`) covered by their existing `c_parity_*` files.

## Validation against the encoder pass's known-good/known-still-dead set

Not directly applicable — the encoder pass's seed list (motion field trio,
`mfmv_controls`, `pd0_detector`, `port_sgr_search.rs`, `md_nsq_motion_search`,
`nic_prune.rs`) is entirely `svtav1-encoder`-scoped and none of it lives in
`svtav1-dsp`/`svtav1-types`/`svtav1-cref`. This pass's own internal check —
whether the method reproduces facts already known from `CLAUDE.md`'s DSP
audits — is the "corrections" section above: `port_convolve.rs` (live, was
suspected unported) and `variance()`/`highbd_iwht4x4_{1,}_add` (dead,
matching pre-existing documented design choices) both landed exactly where
prior, independently-derived documentation said they should.
