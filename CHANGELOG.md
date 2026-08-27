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

- **The 10-bit reconstruction never received the loop restoration it
  signalled — issue #13.** `recon10` fed the Wiener SEARCH (taps picked on
  10-bit data, signalled in the frame header) but only the u8 chain was handed
  to `apply_restoration_frame`, so no 10-bit plane in the port ever carried the
  filter a conforming decoder applies — and nothing could observe it, because
  no post-filter 10-bit recon was published. Now: the DSP stripe-boundary
  machinery (`StripeBoundariesT<T>`, `save_tile_row_boundary_lines`,
  setup/restore) is generic over the pixel type with the u8 names unchanged,
  `loop_restoration_filter_unit_hbd` is the highbd apply arm WITH boundaries
  (C `svt_av1_loop_restoration_filter_unit` at `highbd = 1`, pinned by the new
  `highbd_filter_unit_with_boundaries_matches_c` differential — 200 random
  cells, both `need_boundaries` arms, `data` restored exactly), the encoder's
  `save_lr_boundaries_bd` / `apply_restoration_frame_bd` are the generic
  bodies (u8 delegates, byte-neutral by construction), and the pipeline
  applies LR to the 10-bit canvas with boundary lines from the 10-bit
  post-deblock / post-CDEF planes. Published as the additive
  `EncodePipeline::last_recon10_final` (deblock -> CDEF -> LR on the 10-bit
  canvas; the 10-bit twin of `last_recon`, `with_recon_output` gated).
  Witness `svtav1/tests/issue13_repro.rs`: 383x512 bd10 p6 q40 (luma Wiener
  fires) — `last_recon10_final` == `aomdec` sample for sample; with the apply
  disabled 175,734 samples differ. `SVTAV1_FINAL_RECON` dumps the 10-bit final
  recon (u16 LE) at bd10, and `alignment_gate.sh`'s RECON leg now runs at
  BOTH bit depths (it was bd8-only because nothing 10-bit was comparable).
- **The MDS3 independent-chroma search ran on blocks where C skips it —
  issue #15 closed at 648/648** (`leaf_funnel.rs`). C gates
  `search_best_mds3_uv_mode` on `perform_ind_uv_search_last_mds`
  (product_coding_loop.c:1472-1504); the port implemented only its first arm
  and had nothing for the `inter_vs_intra_cost_th` arm (:1498-1501), which
  zeroes the intra count when `best_inter_cost * 100 < best_intra_cost * 100`.
  `is_inter` there is `is_inter_mode(mode) || use_intrabc`, so on SCREEN
  CONTENT a winning IntraBC candidate makes C skip the search entirely, keep
  `ind_uv_avail = 0`, and code each MDS3 candidate's injected uv-follows-luma
  chroma — where the port's uv table substituted `UV_DC_PRED`. Measured on
  `terminal` 188x256: p2 q55 C MDS1 best intra 97,762,561 vs best IntraBC
  84,376,537 (C codes uv=D113/-1), p4 q12 163,691 vs 148,994 (C codes
  `UV_CFL_PRED`); `ind_uv_avail = 0` read directly off C via the new
  `svt_aom_get_intra_uv_fast_rate` interposer. This was the last of #15's 67
  cells: `unaligned_identity_scan.sh` **646 → 648 / 648, 2 fixed, 0 broken**.
  Byte-neutral wherever no IntraBC candidate exists — the arm is genuinely
  inert there (`byteid_fingerprint` 168/168, **0 rows moved**). Regression cell
  `ind-uv-ibc-cost-gate-188x256` (spot-check 27 → 28). Data:
  `benchmarks/unaligned_real_identity_2026-08-14-induv.{tsv,meta}`.
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

- **Encode speed: the port-vs-C per-pixel slope gap closes to 2.89x at presets
  10 and 13, 3.27x at preset 6, and — for the first time this campaign — 3.93x
  at preset 2** (from 3.06x / 3.07x / 3.39x / 4.14x). All 24 campaign cells
  byte-identical to C (`rust/benchmarks/perf_gap_2026-08-13-r1r2.meta`). Two
  byte-identical changes, and unlike everything before them these remove work
  whose result was **discarded**, not duplicated — the two top findings of
  `rust/docs/C-VS-PORT-CODE-REVIEW-2026-08-13.md`:
  - **R1: the inverse transform + reconstruction ran even where the
    reconstruction is thrown away.** C gates both on `mds_do_spatial_sse ||
    (!is_inter && tx_depth)` (product_coding_loop.c:4783-4784) and the all-intra
    derivation pins `spatial_sse_full_loop_level = 3`, so C inverts nothing at
    MDS1/MDS2; the port inverted unconditionally. A census measured the
    discarded share of inverse-transform pixel work at 40-50% (p10/p13), 36-50%
    (p8), 43-51% (p7), 28-53% (p6) and 24-44% (p2). Three call sites (MDS1
    luma, the CfL alpha search, the non-CfL chroma re-cost) now pass an explicit
    `need_recon = false`, each with an exhaustive-scan proof that the
    reconstruction is unread in its whole binding scope. 56d19efe1 — A/B 12/12
    cells 1.021-1.053x at qp40, and 28 of 28 cells below 1.0 across 6 presets x
    3 sizes x 2 qps against a control arm that split 13/15 (sign test
    p = 3.7e-9).
  - **R2: the exact coefficient rate was computed and then overwritten**
    wherever C's closed forms apply. C's rate tiers are an `if / else if /
    else` and the estimator is never reached on those arms
    (product_coding_loop.c:4914-4934, :5540-5564); the port called
    `cost_coeffs_txb` first and discarded it. Now evaluated in C's order.
    8179a7d94 — 1.038-1.060x at p10/p13 **qp20**, null at qp40/512+; the wall
    clock tracks the census share of replaced coefficient work (51-54% at qp20,
    16-38% at qp40, zero at qp55), which is what identifies the win as the
    mechanism rather than code placement.
  - the census instrument behind both, `leaf_funnel::txcensus` (cargo feature
    `__txcensus`, off by default, zero cost when off). 7dec5f24e.
- Preceding this, four byte-identical changes that took p10/p13 from 3.53x to
  3.06x, every one of them removing a duplicated COPY of something already
  computed rather than making an allocation cheaper:
  - the frame's block-decision set was materialised **four** times per frame —
    a leaf-level clone so the partition tree and a parallel `decisions` list
    could both own it, an aggregation of that list up the tree, a deep clone
    into a `per_tile_decisions` that was **written and never read**, and a deep
    clone of each superblock tree into its raster slot. Only the tree survives;
    `PartitionResult::decisions` is now populated by the legacy
    `partition_search` path alone and `num_blocks` comes from the new
    `PartitionTree::count_leaves` (29847e5d3, A/B 1.07-1.11x at p10).
  - `LeafEval::to_choice` deep-cloned seven of the winning candidate's buffers
    only because it ran *before* `commit_leaf`; both callers now commit first
    and `into_choice` moves (6ad044d00, A/B 1.02-1.03x at p10).
  - `funnel_block_decision`'s depth-0 qcoeff "unpack" was a byte-for-byte copy
    on every block without a 64-dim transform side, and
    `DecodedPictureBuffer::refresh` deep-cloned the whole picture once per set
    bit of `refresh_frame_flags` — eight full Y planes per KEY frame, into
    slots only ever read as `&ReferenceFrame` (now `Arc`-shared; the field is
    private and `store`/`get`/`refresh` keep their signatures, so no API
    change). 81a1bb111, A/B 1.01-1.02x at p10.
  - the per-SB reconstruction staging buffer (an allocation, a zero-fill and a
    second pass over every pixel of every superblock) is gone; **measured
    null**, kept only because it is strictly less work.
- **Measured negative, recorded so it is not retried**: a thread-local `Vec`
  pool for the mode-decision buffers removed a whole class of allocations from
  the profile (`drop_glue::<Cand>` 7.1% of malloc samples -> 0) and measured
  **null** at n=31 against an in-grid identity control. On macOS's xzone
  allocator the pool's machinery costs about what `malloc`/`free` costs at
  these sizes. `rust/benchmarks/alloc_bufpool_null_2026-08-13.meta` names the
  shape that is still unpriced (one construction-time arena the buffers are
  slices into, which is what the C reference does).
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
