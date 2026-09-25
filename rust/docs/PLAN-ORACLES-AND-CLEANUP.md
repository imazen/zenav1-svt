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

- [ ] 0.1 Oracle registry `rust/oracles/oracles.tsv` + `tools/oracle`
  (`list` / `build` / `libdir` / `srcdir`).
- [ ] 0.2 `SVT_ORACLE` honoured by `capture_c_trace` (build and run);
  `SVT_HDR_MODE` kept as an alias.
- [ ] 0.3 `SVT_ORACLE` honoured by `identity_run`: it selects
  `SvtReference` and the HDR mode from the registry row.
- [ ] 0.4 Ghost Robot submodule `reference/svt-av1-hdr` pinned at `9dabe3ca`.
- [ ] 0.5 Output-pin test (`output_pins`) and its committed manifest, in CI.
- [ ] 0.6 Tune numbering follows C: `TUNE_VMAF = 5`,
  `TUNE_FILM_GRAIN = 6`. Today film grain is 5 in Rust.
- [ ] 0.7 Each test file in exactly one test target (T1), plus a check
  that keeps it so.

Done when:
- `tools/oracle build` succeeds for all four oracles on `i265`;
- a still cell encodes through `capture_c_trace` under each of them;
- `output_pins` is green in CI.

## Phase 1 — API for both targets

- [ ] 1.1 `SvtReference::GhostRobot` (and `#[non_exhaustive]`), with its
  own `validate_hdr_config` envelope.
- [ ] 1.2 Ghost Robot's configuration surface in `HdrForkConfig`:
  `enable_qmpsnr`, `luminance_qp_bias`, `hbd_mds`. Plus a typed facade
  builder (`AvifEncoder::with_fork(ForkConfig)`) exposing the fork knobs by
  their C names, instead of the raw `EncodePipeline.hdr` field.
- [ ] 1.3 `EncodingPolicy::SvtParity(GhostRobot)` resolves fork defaults
  exactly as Ghost Robot's `svt_av1_set_default_params` does.
- [ ] 1.4 Narrow the facade (S9):
  - re-export only the types its own signatures use;
  - put the rest behind the existing `__expert` feature;
  - setters stop clamping silently;
  - record each break in CHANGELOG's QUEUED BREAKING CHANGES.

Done when every Ghost Robot `EbSvtAv1EncConfiguration` field has either a
typed facade setter or an explicit refusal that names it.

## Phase 2 — oracle hygiene (before any parity work on a new pin)

- [ ] 2.1 Shim citation check: every C body copied into
  `svtav1-cref/shims` records a hash of its cited source range; the check
  fails when the pinned oracle's text differs. Where a copy exists only to
  reach a `static`, replace it with `#include` of the owning `.c` file.
- [ ] 2.2 `svtav1-cref` builds against any registry oracle
  (`SVT_ORACLE`), so `c_parity_*` runs per target.
- [ ] 2.3 Wrap-fired check in `capture_c_trace`: report every `--wrap`
  interposer that never fired on a cell known to reach it.
- [ ] 2.4 Citation remap tool: rewrite `file.c:NNN` in Rust source
  between two pins, using a `git diff -U0` line map.
- [ ] 2.5 Mirror the pinned Ghost Robot commit into `imazen/zenav1-svt-c`.
  This needs the repo owner, because it writes outside this repo.

## Phase 3 — bit-match Ghost Robot

Ghost Robot = mainline v4.2.0, plus 144 unreleased mainline-master
commits, plus 73 fork-only commits (analysis in the review). Port by the
review's table. Each item lands behind `SvtReference::GhostRobot`, with a
C-parity witness under `SVT_ORACLE=ghost-robot`.

- [ ] 3.1 Stand up the gates under `ghost-robot` and record the baseline
  divergence per cell: 8-bit stills, 10-bit photo, non-flat, video.
- [ ] 3.2 Output-changing mainline-master commits:
  - `1e3da1d7` HBD Hadamard in all-intra MDS0
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
- S5 shared vocabulary and math helpers in `svtav1-types`;
- S4 a `Pixel` sample trait plus a `BitDepth` enum;
- S3 derive signals once through the ported orchestrators;
- S2 split `pipeline.rs` into stages;
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
5. the discarded per-frame work in `pipeline.rs` (the review lists four
   items);
6. per-call allocations (S6).

## Phase 6 — test structure

T2 (in-crate differentials, shrinking the public API), T3 (one cell
harness instead of 39 bash copies), T4 (stage-boundary differentials),
T5 (never-panics sweep), T6 (no_std, tier and dead-code gates), T7 (inline
tests out of product files).
