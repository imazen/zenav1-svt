# Plan — bit-match mainline and Ghost Robot, then factor and speed up

**Live plan.** Tick items in the same change that lands them, with the
commit hash. Findings and evidence live in
[CODE-REVIEW-2026-09-25.md](CODE-REVIEW-2026-09-25.md) (items S1–S9,
T1–T7). This file only orders the work and says what "done" means for each
phase.

## Goal

1. One switch selects the C oracle: pristine **mainline** or the
   svt-av1-hdr **Ghost Robot** fork ([ORACLES.md](ORACLES.md)).
2. The port can bit-match either one, and each claim is backed by a
   standing gate that names its oracle.
3. The codebase is factored, deduplicated and fast while every step stays
   pinned. No refactor lands unless the output pins are unchanged, or the
   change is argued as a product change on RD.

## Rules that hold in every phase

- **Pin first, refactor second.** `svtav1/tests/output_pins.rs` hashes
  port output over a fixed matrix: stills and video, 8 and 10 bit, every
  reference, tunes, enhancements, tiles and threads.
  - A refactor must leave it byte-unchanged.
  - A behaviour change updates the manifest in the same commit, and the
    message says which cells moved and why.
  - Regenerating needs `UPDATE_OUTPUT_PINS=1`, set visibly by the caller,
    never inside the test.
- **Behaviour that differs by oracle is gated by `SvtReference`**, never
  by a new environment variable or a build flag. A `GhostRobot` arm and a
  `Mainline420` arm sit side by side, each citing its C commit.
- **One heavy job at a time**, under `run-heavy` with `--mem` sized to the
  host.

## Phase 0 — foundation

- [x] 0.1 (`5d949d34`) Oracle registry `rust/oracles/oracles.tsv` + `tools/oracle`
  (`list` / `build` / `libdir` / `srcdir`).
- [x] 0.2 (`5d949d34`) `SVT_ORACLE` honoured by `capture_c_trace` (build and run);
  `SVT_HDR_MODE` kept as an alias.
- [x] 0.3 (`50c683a4`) `SVT_ORACLE` honoured by `identity_run`: it selects
  `SvtReference` and the HDR mode from the registry row.
- [x] 0.4 (`c48488ae`) Ghost Robot submodule `reference/svt-av1-hdr` pinned at `9dabe3ca`.
- [x] 0.5 (`1267af21`; 227 cells, 2.6 s) Output-pin test (`output_pins`) and its committed manifest, in CI.
- [x] 0.6 (`50c683a4`) Tune numbering follows C: `TUNE_VMAF = 5`,
  `TUNE_FILM_GRAIN = 6`. Today film grain is 5 in Rust.
- [x] 0.7 (`ba07b218`; 188 targets -> 27, 4029 runs -> 2780) Each test file in exactly one test target (T1), plus a check
  that keeps it so.

Phase 0 done 2026-09-25 on `i265`: all four oracles build and encode,
`regression_spotcheck` 147/147, and the workspace suite passes (2780).
It was done when:
- `tools/oracle build` succeeds for all four oracles on `i265`;
- a still cell encodes through `capture_c_trace` under each of them;
- `output_pins` is green in CI.

## Phase 1 — API for both targets

- [x] 1.1 (`50c683a4`) `SvtReference::GhostRobot` (and `#[non_exhaustive]`), with its
  own `validate_hdr_config` envelope.
- [ ] 1.2 Ghost Robot's configuration surface in `HdrForkConfig`:
  `enable_qmpsnr`, `luminance_qp_bias`, `hbd_mds`. Plus a typed facade
  builder (`AvifEncoder::with_fork(ForkConfig)`) exposing the fork knobs by
  their C names, instead of the raw `EncodePipeline.hdr` field.
  - Done (`7df9a27f`):
    - `HdrForkConfig` carries `luminance_qp_bias`, `hbd_mds`,
      `enable_qmpsnr` and `max_hierarchical_levels` at C's defaults.
      `validate_hdr_config` refuses every value the port does not implement,
      and a unit test pins that.
    - `svtav1/tests/config_surface.rs` gives each of Ghost Robot's 139
      `EbSvtAv1EncConfiguration` fields one disposition (Api / Hdr /
      Refused / Derived / DefaultOnly / NotApplicable). It parses the C
      header, so a new upstream field fails the test until it is
      classified.
    - The refusal ledger now also scans the `validate*` `Err(...)` refusals
      and the animation facade, which it had missed: 83 -> 112 rows.
  - Open: the typed facade builder. Plan 1.4 narrows the facade, which
    needs a decision on what `svtav1_encoder` itself makes public: zenavif
    (pinned by git rev) uses `EncodePipeline`, `RcConfig`, `HdrForkConfig`,
    `FilmGrainConfig`, `entropy::obu` and `types::EncodeError` directly,
    and many integration tests reach internals (T2).
- [x] 1.3 (`893f6823`) `EncodingPolicy::SvtParity(GhostRobot)` resolves fork defaults
  exactly as Ghost Robot's `svt_av1_set_default_params` does.
- [ ] 1.4 Narrow the facade (S9):
  - re-export only the types its own signatures use;
  - put the rest behind the existing `__expert` feature;
  - setters stop clamping silently;
  - record each break in CHANGELOG's QUEUED BREAKING CHANGES.

Done when every Ghost Robot `EbSvtAv1EncConfiguration` field has either a
typed facade setter or an explicit refusal that names it.

## Phase 2 — oracle hygiene (before any parity work on a new pin)

- [x] 2.1 (`2169f16f`) Shim and source citation check: `tools/citations.py check
  --to <oracle>` fingerprints every cited C span in the base oracle and
  classifies it in the target: same, moved, ambig, changed, unresolved.
  Measured hybrid-3115 -> ghost-robot: 7,386 citations; 903 changed (12%),
  42 of them in `svtav1-cref/shims`, which are the copies to refresh before
  `c_parity` means anything for that oracle; 4,432 moved. Against
  mainline-4.2.0: 162 changed. Still open: replace shim copies that exist only
  to reach a `static` with `#include` of the owning `.c` file.
- [ ] 2.2 `svtav1-cref` builds against any registry oracle
  (`SVT_ORACLE`), so `c_parity_*` runs per target.
- [ ] 2.3 Wrap-fired check in `capture_c_trace`: report every `--wrap`
  interposer that never fired on a cell known to reach it.
- [x] 2.4 (`2169f16f`) Citation remap: `tools/citations.py remap --to <oracle>`
  rewrites `moved` citations in place. Run it in the same change that bumps
  or retires a pin, not before.
- [ ] 2.5 Mirror the pinned Ghost Robot commit into `imazen/zenav1-svt-c`.
  This needs the repo owner, because it writes outside this repo.

## Phase 3 — bit-match Ghost Robot

Ghost Robot = mainline v4.2.0, plus 144 unreleased mainline-master
commits, plus 73 fork-only commits (analysis in the review). Port by the
review's table. Each item lands behind `SvtReference::GhostRobot`, with a
C-parity witness under `SVT_ORACLE=ghost-robot`.

- [x] 3.1 (`2d96724a`) Still-grid baseline under `ghost-robot`:
  `tools/oracle_still_grid.sh` over {gradient, photo} x {64,128,256} x
  {8,10}-bit x qp {20,32,45,55} x presets {2,4,6,8,10,13} = 288 cells.
  32/288 identical (bd8 28/144, bd10 4/144); control under
  `mainline-4.2.0` is 288/288. Recorded in
  `benchmarks/ghost_robot_baseline_2026-09-25.meta` with the per-axis and
  stage histogram. The non-flat/video legs of the wider baseline remain
  open.
- [ ] 3.2 Output-changing mainline-master commits:
  - `1e3da1d7` (`1bdae047`) HBD Hadamard in all-intra MDS0: ported
    `svt_aom_highbd_hadamard_{8x8(avx2),16x16,32x32}` as
    `aom_highbd_hadamard_*`, gated on `GhostRobot` in `hadamard_satd_hbd`.
    Grid 32/288 identical (unchanged) — the bd10 cells it moves (op 9→15
    on photo-256-b10-p8-q32, payloads closer on six more) still hit
    earlier divergences; kernel parity pinned in c_parity_hadamard
    (avx2 multiset + composed _c oracle). Mainline control 288/288.
  - `8b1f9a0d` restoration-enable derivation
  - `507025f6` PD1/LPD1 switch
  - `ccdfdb09` fractional CRF above 63
  - `0c1c4dec` luma bias removed
  - the rest of the 35 "behaviour?" commits, triaged by the gate.
- [ ] 3.3 Fork determinism and correctness fixes: `f0111bae`, `d6f4b170`,
  `560f7453`, `ec7e414d`, `2c66d9ea`.
- [ ] 3.4 Fork behaviour: complex-hvs (`70877799`, `d705ef50`), MDS0
  ac-bias dampening (`c65c2bfa`), chroma noise `pow(luma, 0.75)`
  (`9f54af57`), delta-q all-skip (`2f08c8e8`), lossless across tunes
  (`a74cfb9e`).
- [ ] 3.5 QM-PSNR (`dff0a9f8`).
- [ ] 3.6 High Profile 4:4:4 (`f67a0f74`, `c4e1b9ce`). This is the largest
  item, and needs the 4:4:4 chroma paths completed first (S1).
- [ ] 3.7 Retire `hybrid-3115*`: drop the registry rows, `SvtHdrMode`,
  `Hybrid3115` (a queued break), the hybrid gates, and `3115c0c1b` as the
  submodule pin.

Done when each gate reports a pinned count under both `mainline-4.2.0`
and `ghost-robot`, and README states both.

## Phase 4 — structure and deduplication (every step pinned)

In dependency order:
- S5 shared vocabulary and math helpers in `svtav1-types`.
  - Done: `svtav1_types::math` replaces 43 copies of `round_power_of_two*`,
    `divide_and_round` and `clip3` in 30 files, imported under the old local
    names, with pins unchanged (`0d544f9d`).
  - Kept: the two `wrapping_add` variants (different overflow semantics), and
    the copies in `intra_pred.rs` and `hbd.rs` (owned by the intra-x86
    delegate; fold them in after it lands).
  - Done: the 13 `rdcost` copies -> `svtav1_types::math::rd` (`fc82b896`).
  - Next: AV1 vocabulary constants (`INTRA_FRAME` ×8,
    `LAST_FRAME` ×9, `MI_SIZE` ×9), and same-name types (`SbVariance` ×3, and
    others).
- S4 a `Pixel` sample trait plus a `BitDepth` enum;
- S3 derive signals once through the ported orchestrators;
- S2 split `pipeline.rs` into stages. Target: every `.rs` file at 2-3 kloc
  (user, 2026-09-25). MET for the whole workspace on 2026-09-25 (largest file:
  2,958 lines), and kept by the `file_size_check.py` CI gate.
  - Done (`64e86276`): pure moves of the tests and the entropy walk into
    `pipeline/{tests, inter_tile_byte_gate, inter_decision_probe,
    entropy_ctx, lpd1, block_syntax, partition_walk, tile_walk}.rs`, which
    takes `pipeline.rs` from 23,560 to 13,351 lines. The tools are
    `tools/split_inline_mod.py` and `tools/move_items.py`; they only raise
    privacy to `pub(super)`, and the pins are unchanged.
  - Done (`394ebdb5`): `EncodePipeline` methods move by name
    (`tools/move_methods.py`) into
    `pipeline/{ra, tpl_stage, setup, entry, grain, config_check, superres,
    cbr}.rs`, taking `pipeline.rs` to 8,497 lines.
  - Done (`cd862116`): 14 stages extracted from `encode_frame_impl`
    (`tools/ra_extract.py` drives rust-analyzer's "Extract into function",
    which works out every stage's inputs and outputs) and moved into
    `pipeline/{frame_setup, inter_setup, restoration_stage,
    frame_output}.rs`. `pipeline.rs` is 6,508 lines; `encode_frame_impl` is
    5,625. Lesson from the attempt: a stage cannot take `&mut self` while
    `run_entropy_walk` (a closure holding a shared borrow of `self`) is live,
    and a stage that RETURNS data borrowing `self` pins all of `self`. Such
    stages take `&self` and hand their `self` writes back to the caller
    (`search_restoration`). `build_inter_md_frame` and `bd10_post_pass` are
    left inline until they can take the specific fields they borrow.
  - Done (`bd3962ec`): 12 more stages move to `pipeline/{md_setup,
    loop_filters, recon_output, diagnostics}.rs`, so the debug dumps sit in
    `diagnostics.rs` (S7). `pipeline.rs` is 5,788 lines;
    `encode_frame_impl` is 4,900. rust-analyzer failed on four ranges (the
    `run_entropy_walk` closure body, the TPL r0 block, the chroma and HBD
    source prep), each for its own reason: invalid code, a closure left
    `FnOnce`, moves out of borrowed values. They remain inline.
  - Done (`5d0a77ee`): the tangled three, `entropy_walk` (the body of the
    `run_entropy_walk` closure), `bd10_post_pass` and
    `build_inter_md_frame`, are ASSOCIATED fns that take the few `self` fields
    they touch as parameters (4, 6 and 8). Then a closure or a
    returned borrow pins only those fields, as the inline code did, instead
    of all of `self`. Moved to `pipeline/{walk_driver, bd10_post,
    inter_md_stage}.rs`. `pipeline.rs` is 4,950 lines; `encode_frame_impl` is
    4,055.
  - Done (`493ad07a`): 18 more stages, with the comments above each range
    moving along with it. The three constants local to `encode_frame_impl`
    that they share (`DLF_FAST_DECODE`, `SEQ_CDEF_LEVEL`, `CDEF_FAST_DECODE`)
    are now module-level. `pipeline.rs` is 4,509 lines; `encode_frame_impl`
    is 3,595.
  - Done (`438878ba`): the state types (`EncodePipeline` and its private
    helper structs), the plane helpers and the three constants move to
    `pipeline/state.rs`; `palette_cache` moves to its own file.
    `pipeline.rs` is 3,792 lines, nearly all of it `encode_frame_impl`.
  - Done (`7988f2a4`): `encode_frame_impl` is three phases: setup stays in
    `pipeline.rs`; `tile_phase::decide_and_encode_tiles` (mode decision,
    tiles, bd10 post-pass); `pack_phase::filter_and_pack_frame` (entropy
    walk, deblock/CDEF/LR, bitstream, recon outputs, reference). `pipeline.rs`
    is 2,286 lines; `encode_frame_impl` is 2,085.
  - Done (`cd33b8b5`): `tile_walk::encode_tile_rows` (the per-tile closure,
    then the per-coding-unit body) and `mds3::eval_candidate` (intra chroma,
    inter chroma, chroma detector, tx-depth search) are split too; neither file
    is over 1.8 kloc now. The 56 extracted single-call-site stages are
    `#[inline(always)]`, which puts their code back inline.
  - Perf cost, measured (`benchmarks/perf_s2_split_2026-09-25.meta`): output
    byte-identical; +0.10% to +0.39% instructions on small frames (callgrind)
    and neutral on large ones. The remainder is extra memcpy of values that the
    stage boundaries move or return by value. The FramePlan bundling below should
    remove it; re-measure with callgrind when it lands.
  - Next: the phase calls pass 67 and 69 parameters (rust-analyzer's
    explicit data flow). Bundle them into a `FramePlan` struct that setup
    returns, so each phase reads named fields. Then `tile_walk.rs` (3,501,
    one function) and the other files over 3 kloc.
  - Next: `encode_frame_impl` is ONE 7,646-line function, and
    `tile_walk::encode_tile_rows` is 3,500. Only stage extraction gets them
    under target; moves cannot. Then the other 21 files over 3 kloc, starting
    with their inline tests (T7).
- S1 retire or quarantine the pre-port encoder;
- S6 one scratch strategy;
- S7 diagnostics behind an observer and a `trace` feature;
- S8 uniform `incant!` plus the tier gate.

Each is a series of small commits, each green on `output_pins`.

## Phase 5 — performance (bit-exact unless argued otherwise)

Protocol: `tools/perf_ab.sh`, interleaved and pinned to one core, with
`output_pins` unchanged; record each result in `benchmarks/*.meta`.
Order, by expected size:
1. x86 tiers for the intra, 10-bit intra and CFL kernels;
2. dispatch for convolve (10-bit, scaled), diff-weighted masks,
   `enc_make_inter_predictor` and the temporal-filter kernels; NEON for
   `port_convolve`;
3. multi-offset SAD for full-pel ME;
4. 10-bit mode decision without the 8-bit twin work;
5. the discarded per-frame work in `pipeline.rs`.
   - Done: the post-encode per-SB full-pel `mv_map` fill (its only reader
     had already run) and the `tpl_sb_qp_offsets` full-frame pass (consumed
     by `let _`) are removed; the dead homegrown temporal-filter branch is
     gone from the pipeline (`092c7e27`).
   - Open: the third symbol-writing walk (`recon_only`), and the per-leaf
     `FunnelFrame` deep clones under TPL/SSIM lambdas.
   - Done (`9409e9f6`): `temporal_filter::temporal_filter`, `TfConfig`,
     `TfResult` and the f64 `estimate_noise` are deleted. The
     `fallible-alloc` test now calls `try_vec!` inside the encoder crate
     directly, which is what it was testing;
6. per-call allocations (S6).

## Maintenance backlog

- [ ] CHANGELOG `[Unreleased]` is 3,800 lines of repeated
  Added/Changed/Fixed blocks appended per campaign, with QUEUED BREAKING
  CHANGES buried at line ~280. Fold it into one set of categories, and cut a
  dated section per released version.
  - Done (`53107d71`): folded into one QUEUED BREAKING / Added / Changed /
    Removed / Fixed set, with the queued breaks first. Every entry is
    preserved verbatim (the line multiset was checked).
  - Open: cut a dated section at the next release.

## Phase 6 — test structure

- [x] T5 (`978df514`) never-panics sweep: `svtav1/tests/never_panics.rs`,
  536 tiny encodes across geometry, depth, chroma, preset, qp, every
  reference and mode, fork knobs, LD/RA video and all Zen enhancements, in
  3 s. Measured 2026-09-25: 453 Ok, 83 explicit refusals, 0 panics; it
  asserts an Ok floor of 440 against vacuity.
- [ ] T2 in-crate differentials, shrinking the public API.
- [ ] T3 one cell harness instead of 39 bash copies.
- [ ] T4 stage-boundary differentials.
- [ ] T6 no_std, tier and dead-code gates.
  - Done: no_std. `just nostd-check` (and a CI step in shard 3) checks
    `types`, `dsp` and `encoder` one at a time with `--no-default-features`
    and `-D warnings`. It must be per crate: at workspace level, feature
    unification through dev-dependencies turns `std` back on, which is how
    the encoder carried 121 no_std errors past `test-minimal`. The encoder's
    `std` feature now forwards to `types/std` and `dsp/std`. The raw
    `std::env` debug reads (DISPDBG, QTRACE, BLKDBG, CRDBG, PSYM, and the
    `SC_TOOLS` bisect knob) moved into `dbgenv`, which reads each one once
    and returns `false` or `None` without std. OBMC's thread-local buffers
    fall back to fresh buffers on every call.
  - Open: the tier gate (S8) and a dead-code gate.
- [ ] T7 inline tests out of product files.
  - Done (this change): 38 inline `#[cfg(test)]` modules in the 23 files
    over 2 kloc moved to sibling files (`<file>/tests.rs` and so on) with
    `tools/split_inline_mod.py`. The test count is unchanged (2770).
  - Open: the smaller files, and `crates/*/tests/` binaries that test
    private internals, which belong in-crate (T2).
