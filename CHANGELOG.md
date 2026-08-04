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
