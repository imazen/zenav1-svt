# Changelog

All notable changes to `zenav1-svt` (the pure-Rust SVT-AV1 still-image encoder
port). The project's unit of progress is **byte-identity with the C reference**,
so entries state what became byte-identical and under which gate, not just what
code was added.

Crates are not published to crates.io yet — depend by git.

## [Unreleased]

### QUEUED BREAKING CHANGES

<!-- Batch API breaks here; ship them in one version bump, never piecemeal. -->
- None queued. `EncodePipeline`'s new surface (`try_encode_frame_420_hbd`,
  `try_encode_frame_hbd`, `with_superres`) is additive; the `SeqTools` and
  `ScSignal` structs gained fields (`enable_superres`, `superres`), which is a
  break only for out-of-crate struct literals — there are none.

### Added

- **10-bit encoding at NON-64-ALIGNED dimensions — the product case for 10-bit
  AVIF** (`bd10_partial_sb_gate.sh`, **157/157 byte-identical to the C
  reference**; every one of those cells was a refusal before). Both bd10 level
  producers now handle partial superblocks: the full-RD funnel (preset ≤ 8),
  which needed only the gate lifted because it rides the same partition search
  and leaf funnel as the already-partial-SB-correct 8-bit path; and the
  level-only re-encode post-pass (preset ≥ 9), which needed SB-extent-sized
  recon buffers, straddle-clipped recon writes, SB-extent-padded 10-bit
  sources, and the pack's skip-off-frame-quadrant child walk in place of a
  fixed `(partition_type, children.len())` offset table that a pruned
  partial-SB child list makes both `panic!`-prone and positionally wrong.
  `bit_depth_config_error` no longer refuses ANY 10-bit configuration on
  dimension grounds; `docs/REFUSED-CONFIGS.md` drops 12 → 10 CAPABILITY
  refusals, and `arbitrary_size_robustness.sh` goes from 80/80 with **48
  refused** to **128/128 with 0 refused** — those 48 are exactly these cells,
  and every one now decodes under the AV1 reference decoder.
  Data: `benchmarks/bd10_partial_sb_2026-08-04.tsv`; full record in
  `docs/bd10-port-map.md`. Residual (NOT closed, pinned self-promotingly in the
  gate): a set of non-flat cells, measured to be the known bd10 non-flat gap
  (21.5% of non-flat cells at 64-aligned dims vs 26.3% at partial-SB dims;
  `uniform` is 100% everywhere) rather than a partial-SB gap.

### Fixed

- **`sse_i32` subtracted coefficients in i32 where C subtracts in `int64_t`,
  and panicked in debug where C's `uint64_t` wraps** (`svtav1-dsp`
  `residual.rs`; C `svt_full_distortion_kernel32_bits_c`, `pic_operators.c:86`).
  Three widths were Rust's rather than C's — the subtraction (`(x - y) as i64`),
  the square, and the accumulator — and the accumulator is what left
  `residual_recon_distortion_all_tiers_match_core` RED on `main`. All three now
  match C in every build. The NEON arm cannot widen first (no i64xi64 multiply
  exists to square an `int64x2_t`), so it keeps `vsubq_s32` and DETECTS a wrap
  by comparing against `vqsubq_s32`, falling back to the exact scalar core;
  fast path exact, slow path exact. New gate
  `sse_i32_matches_c_widths_at_i32_extremes` checks every tier against an i128
  oracle and asserts its own case set discriminates the two widths. **Byte-inert
  on every grid** (byteid 168/168 with 0 cells moved, unaligned scan 648 cells
  with 0 changed, partial_sb 146/146, decode grid 120/120, recon parity
  432/432). Measured: the wrap is unreachable on a real encode — 0 in 59,088,480
  elements, max |difference| 788 against an i32 ceiling of 2,147,483,647
  (`benchmarks/sse_i32_width_2026-08-11.meta`), so this does NOT explain issue
  #15, which stays open.

- **Loop restoration walked a different unit grid than the one the search
  sized — an out-of-bounds panic on the public encode API** (issue #11,
  `restoration.rs:985`, `index out of bounds: the len is 2 but the index is 2`).
  C derives the restoration-unit count (`svt_av1_alloc_restoration_struct`) and
  every unit walk (`svt_av1_loop_restoration_filter_frame`,
  `svt_av1_loop_restoration_save_boundary_lines`) from ONE
  `whole_frame_rect(&cm->frm_size, ..)`, and `cm->frm_size` is the pre-8-alignment
  coded size (`pcs.c:1337`, `picture_width - non_m8_pad_w`), CEILING-subsampled
  for chroma. The port's SEARCH used the true extent (task #95 goal 1) but
  `apply_restoration_frame` / `save_lr_boundaries` were still handed the ALIGNED
  `w`/`h`, so wherever the 8-alignment crossed a `count_units_in_tile(256, ..)`
  boundary the walk visited more units than the grid holds: true 383 counts one
  horizontal unit, aligned 384 walks two. Both now take the true extent plus the
  aligned canvas STRIDE, and chroma rounds up like C rather than down. Reported
  on 5 real renditions (115 of 34,200 HDR-grid cells); reproduced synthetically
  at `383x512` / `766x128` / `258x128` / `385x257` at bd8 AND bd10. The
  bitstream was never affected — the panic came after the tile was written — and
  the previously-panicking cells are now byte-identical to the C encoder
  (`regression_spotcheck.sh` cells `lr-align-cross-*`). A 2,280-cell A/B of the
  pre- and post-fix encoders over 19 dimensions × 5 presets × 4 qps × 2 depths ×
  3 contents shows every previously-working cell byte-unchanged.
- **The bd10 per-tile recon canvases were MERGED at the wrong stride.**
  `commit_leaf` writes them at the ALIGNED stride (the SB-extent product exists
  only so a right-straddle write wraps into slack rather than out of bounds),
  but the frame merge read them at the SB-EXTENT stride. Byte-inert while every
  gated bd10 cell had `ext_w == w`; it scrambled the 10-bit recon that the bd10
  deblock / CDEF / Wiener searches read the moment a frame had a partial SB.
- **The native-u16 source had no SB-extent twin.** `HbdSource` is padded
  TRUE→ALIGNED only while `blk_y_src10` gathers by absolute coordinates, so a
  straddling block would read past the plane or wrap into the next row. Added
  the `sb_input` / `sb_chroma_owned` equivalents and threaded `in_stride` into
  `FunnelSrc10`; the `debug_assert_eq!(in_stride, w, "bd10 hbd source assumes a
  64-aligned frame")` that stood in for this is gone.
- **Two out-of-bounds panics on the public encode API**
  (`crates/svtav1-encoder/src/intrabc_hash.rs`). C computes
  `x_end = pic_width - block_size + 1` as a SIGNED int
  (`hash_motion.c:195-196`, `:222-223`), so a picture smaller than the hash
  block just yields an empty loop; the port used `usize`, underflowed to ~2^64
  and indexed off the end. A 32x32 screen frame at preset 0 panicked twice
  (`len is 1024 but the index is 1024`, and `index 2048`). Found by the new
  8-bit gate's dims tier — no earlier gate encoded anything below 60x60 with
  the screen-content tools armed.

### Changed

- **CI gates four more 8-bit surfaces**: partial-SB / odd dimensions (104
  cells), tiles across rows AND columns (29), SB128 (22), and panic-freedom on
  gradient AND screen (80). All four already failed loudly — they were simply
  never in the workflow.
- **`identity_run` reports a REFUSAL distinctly from a crash** (exit 3). It
  called the infallible `encode_frame*` wrappers, whose `.expect()` turned every
  deliberate out-of-envelope refusal into a panic; `arbitrary_size_robustness.sh`
  therefore reported 48 correct bd10 refusals as PANIC, unable to tell the
  port's best behaviour from its worst. That gate now reads 80/80 + 48 refused
  where it read 80/128, on identical encoder behaviour.
- **`tools/arbitrary_size_robustness.sh` now sweeps `screen` content as well as
  `gradient`, and adds sub-64 cells.** It previously ran gradient only, which
  never arms the screen-content detector — so palette and IntraBC were off in
  every cell and the gate could not reach the code paths they use. It ran
  straight past the `intrabc_hash` panics above. A panic-freedom gate that
  cannot arm half the encoder's tools is not a panic-freedom gate.

### Added

- **A comprehensive 8-bit byte-parity gate, and CI coverage for it**
  (`tools/identity_full_8bit.sh`). Until now there was **no 8-bit
  byte-vs-C identity gate in CI at any preset**: `identity_matrix.sh` is a
  scoreboard whose own header says "Exit 0 always", and it was not in the
  workflow either — so every 8-bit byte-identity claim, on the port's primary
  product surface, rested on hand-run measurements that nothing re-checked.
  The new gate exits nonzero, sweeps **every preset 0..13** (C clamps all-intra
  above M9 to M9 but the port does not, so 10..13 are distinct configurations
  here), carries low-q density where structural problems hide, covers
  partial-SB / odd / tiny / large geometry and four content classes including
  screen, pins divergences **self-promotingly** (a pinned cell that starts
  matching fails until promoted), and fails on harness errors so a cell that
  could not run can never look like a pass. `identity_matrix.sh` keeps its
  scoreboard role and gains `IM_STRICT=1` for gate use.


- **Native 10-bit input** (#6). `EncodePipeline::try_encode_frame_420_hbd` /
  `try_encode_frame_hbd` take real `u16` planes. The low 2 bits reach the mode
  decision, the coded levels, and the deblock / CDEF / Wiener searches — the
  port no longer widens an 8-bit source internally (35743ebd5, f319ec298).
  Gate: `tools/bd10_hbd_src_gate.sh`, 100/100 cells byte-identical to C.
- **Super-resolution**, opt-in via `EncodePipeline::with_superres(denom)` with
  `denom` in 9..=16, off by default exactly as in C (5c69edcb2, f4a1b7516,
  2f4d24cba, f319ec298, 174b0f184). Gate: `tools/superres_gate.sh`, 128/128
  cells checked three ways — byte-parity vs C, decodability at the upscaled
  size under the reference decoder, and anti-vacuity vs the non-superres stream.
  - `svtav1-dsp::superres` — the normative 64-phase upscale (was a 16-phase
    stub); `svtav1-dsp::resize` — the source downscale (new).
  - Sequence-header `enable_superres` + frame-header `superres_params()`.
  - C's stale full-resolution variance array, read through coded-grid indices,
    is reproduced deliberately (chunk B.4) — matching C requires it.
- `tools/bd10_hbd_src_gate.sh` and `tools/superres_gate.sh`, both wired into CI.
- `CONTEXT-HANDOFF.md` — build-from-scratch, gate, and open-work guide.

### Changed

- The test runner is `cargo nextest run` (CI and `just test`); each test gets
  its own process, which prevents archmage's process-wide dispatch-tier state
  from leaking between tests (d807fa0fe).
- Out-of-envelope configurations are REFUSED with
  `EncodeError::UnsupportedConfig` rather than silently encoding truncated or
  mis-scaled content (`hbd_source_consumed`, `superres_config_error`).

### Fixed

- **Partial-superblock RD mis-pricing: the cropped-TX distortion bound is now
  wired** (#95 chunk 2 (b)+(c)). On a frame whose aligned dims are not a
  multiple of 64, a coded TX block can straddle the frame edge; C prices only
  the part inside the ALIGNED frame (`cropped_tx_width`/`cropped_tx_height`,
  `Source/Lib/Codec/product_coding_loop.c:4664-4665` and `:5752-5754`;
  `cropped_tx_width_uv`/`_height_uv`, `full_loop.c:2228-2232`), while the port
  scored the whole block — so every boundary block was mis-priced. The
  already-written `frame_geom::cropped_tx_dims` (plus a new `cropped_tx_dims_uv`
  for C's chroma-domain expression) now feeds `leaf_funnel::tx_unit`,
  `tx_unit_hbd` and `txt_search`. The crop touches ONLY the spatial distortion
  kernels; the residual, transform, quantizer, RDOQ, recon and coefficient rate
  still run over the full TX block, exactly as in C.
  Measured crop-off → crop-on over 48 partial-SB cells: 8 changed bytes,
  **3 went divergent → byte-identical to C** (`gradient 80x88 / 104x88 / 72x88
  at q55 preset 6`, the straddle-win trio), **0 regressed**. Those three are now
  gated: `tools/partial_sb_gate.sh` 101 → **104/104**. Byte-neutral everywhere
  else (`identity_matrix` 54/54, `bd10_matrix` 36/36) — on a 64-aligned frame
  the crop is the identity. New differential test
  `leaf_funnel::tests::cropped_tx_distortion_matches_c_spatial_facade` pins the
  cropped distortion to the real exported
  `svt_spatial_full_distortion_kernel_facade` via `svtav1-cref`.
- `coeff_c_txb_init_levels_partial_zero_no_stale_reads` failed at default test
  parallelism: archmage token disabling is process-wide, so a sibling
  permutation test could move it onto the scalar arm. It now holds
  `lock_token_testing`, and 31 further dsp tests pin their tier the same way
  (d807fa0fe). No bitstream impact — every consumer reads only scan positions
  below `eob`.
- `perf_report` example declared `required-features = ["std"]`; a bare
  `cargo test -p zenav1-svt-dsp` previously failed to build it (f319ec298).

### Removed

- `svtav1_dsp::superres::{superres_upscale, superres_upscale_row}` — the
  non-normative 16-phase stub, replaced by the real kernel. No in-tree callers.

## Earlier history

This file starts at 2026-07-24. Prior progress (the 8-bit byte-identity
campaign, chroma/4:2:0, deblocking, CDEF, Wiener restoration, palette, tiles,
arbitrary dimensions, the 10-bit MD path) is recorded per-feature in
`rust/docs/*.md` and in `rust/CLAUDE.md`'s status sections, with the commit
hashes cited inline there.
