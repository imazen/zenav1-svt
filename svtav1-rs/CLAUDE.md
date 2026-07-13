# SVT-AV1 Rust Port Rules

## CONFORMANCE MANDATE

**NEVER stop working while ANY conformance or parity issue remains.** If the bitstream does not decode with rav1d-safe at ALL tested sizes, the work is NOT DONE. If any differential test shows a decode failure, investigate the root cause and fix it before committing documentation, before updating handoffs, before doing anything else. Conformance failures are the #1 priority — above new features, above performance, above code cleanup. Do not describe a conformance failure as "expected" or "known" — describe it as "BLOCKING" and fix it in the same session.

This applies to:
- Bitstream decode failures at any image size
- Parity mismatches between C golden output and Rust implementation
- CDF/entropy coding mismatches that produce non-decodable output
- OBU structure errors that cause decoder rejection

**The definition of "done" for any encoding feature is: rav1d-safe decodes the output correctly at all tested sizes.**

## Commit Discipline
- **Commit after EVERY meaningful change.** After porting a function, commit. After adding a test, commit. After fixing a test, commit. Never batch more than ~30 minutes of work into one commit.
- **Push after every commit.** CI runs on remote.
- **Commit message format:** `<type>(<crate>): <description>` — e.g., `feat(svtav1-dsp): port forward DCT 4x4 with AVX2`

## Safety Rules
- `#![forbid(unsafe_code)]` on every crate by default.
- Archmage `#[arcane]`/`#[rite]` do NOT require `#[allow(unsafe_code)]` — they generate safe code via tokens.
- NEVER add `unsafe` without: (1) profiling evidence, (2) `cargo asm` evidence, (3) feature-flag gating, (4) parity test, (5) comment citing benchmark commit.
- NEVER use `core::arch::*` directly — only `archmage::prelude::*`.
- NEVER `#[inline(always)]` on `#[arcane]`/`#[rite]` functions.

## TDD Rules
- Write the test BEFORE the implementation for every ported function.
- Every function must have a parity test against C golden output.
- Every SIMD function must pass `for_each_token_permutation`.
- Floating-point: exact match required. Don't accept "small differences" — find the divergence.
- Run `cargo test` before every commit.

## Archmage Rules
- `#[arcane]` = entry points only (after token summon). One per hot path.
- `#[rite]` = inner helpers (no dispatch overhead). Default for all SIMD helpers.
- `incant!` for multi-platform dispatch with `[v3, neon, scalar]` tiers.
- Token summon once at entry, pass through call chain.
- `Desktop64` for x86 AVX2+FMA, `NeonToken` for ARM, `ScalarToken` always.

## Porting Rules
- Read the ENTIRE C function before porting. Don't port line-by-line — understand the algorithm.
- Port ALL constants/tables/helpers. No stubs, no TODOs.
- Reference `specs/` files for algorithm documentation.
- Exact-match parity test before moving to next function.
- Use `specs/18-testing.md` for test patterns and coverage requirements.
- Floating-point determinism: test pipeline stages independently.

## Performance Rules
- Correctness first, performance second. Get parity, THEN optimize.
- Profile with `cargo flamegraph` and `cargo asm` before optimizing.
- SIMD via archmage only — never hand-write `asm!` blocks.
- Benchmark with criterion/divan. Commit results to `benchmarks/`.
- Thread-local scratch buffers for ME/transform intermediates (rav1d-safe pattern).

## Spec References
- `specs/00-architecture.md` through `specs/18-testing.md` are the algorithm bible.
- When in doubt, read the spec, then read the C source, then implement.
- Update specs if you discover they're wrong.

## Reference Code
- rav1d-safe at `/home/lilith/work/zen/rav1d-safe/` can be referenced for patterns (DisjointMut, archmage usage, etc.)
- Document any borrowing from rav1d-safe in commit messages and here under "Borrowed Patterns"

## Borrowed Patterns
- **DisjointMut concept**: The `svtav1-disjoint-mut` crate's region-based borrow tracking
  pattern is adapted from `rav1d-disjoint-mut` at `/home/lilith/work/zen/rav1d-safe/crates/rav1d-disjoint-mut/`.
  Our implementation is simplified (no UnsafeCell, fully safe), but the API shape
  (Region, BorrowTracker, overlap detection) follows rav1d's design.

## Known Bugs — BLOCKING

(none — decode conformance gate is green: 525/525 matrix streams decode
under the AV1 reference decoder as of 2026-07-13, C baseline v4.2.0-rc)

### Next structural gaps toward C bit-identity (not decode blockers)

> **Identity harness live (2026-07-13):** `tools/identity_diff.sh <w> <h>
> <qp> <preset> [content]` (or `just identity`) runs Rust + wrapped-C at a
> matched still/AVIF CQP config and diffs OBU bytes (SH/FH field-level) +
> canonicalized od_ec op traces. First divergence map with per-class C
> source cites + priority-ordered fix list: **docs/IDENTITY-STATUS.md**.
> Headlines: Rust-only frame-level VAQ shifts base_q (CQP ≠ CQP); C always
> signals TX_MODE_SELECT + per-block tx_depth (we signal LARGEST, no
> syntax); SH level/CICP/color_range constants differ; op-0 tile
> divergence is the 64x64 partition decision (C md RDO vs our homegrown
> partition_search costs).
0a. **[FIXED 2026-07-13] Edges-content divergence** — root cause was NOT the
   transforms (all named + dispatch wrapper paths are now pinned bit-exact
   vs C by c_parity_txfm, incl. rect + flat-DC shapes): extract_neighbors
   filled unavailable prediction edges with 128 while the decoder fills
   above-missing with left_ref[0] (else 127), left-missing with
   above_ref[0] (else 129) per libaom build_intra_predictors. Edge V_PRED
   blocks were coded against pred=128 but decoded against pred=32 — the
   observed "half residual". After the C-exact fill (5d51ef1e6): edges 64
   qindex30 s2 decodes LOSSLESSLY (205 bytes vs C's 172), gradient 64
   qindex30 s4 = 46.76 dB, gradient 128 q50 s8 = 30.39 dB, conformance
   525/0. Residual gap (directional extended-edge arrays) FIXED — see 4.
0b. **[FIXED 2026-07-13] 2D transform wrapper divergence (AC only, encoder-blind)** — evidence:
   flat-140/flat-250 decode bit-exactly (DC + golomb path perfect), but any
   AC-rich content degrades (gradient 64px qindex30: 11.7 dB; 128px q50:
   29.5 dB) identically across speeds, invisible to the encoder because our
   fwd+inv roundtrip is self-consistent while the decoder inverts per spec.
   The 1D kernels are C-bit-exact (51 golden tests) but the 2D wrappers
   (stage ranges/shifts/transpose order in fwd_txfm/inv_txfm/txfm_dispatch)
   were never differentially tested. NEXT: cref shims for
   svt_av1_fwd_txfm2d_NxN + svt_av1_inv_txfm2d_add_NxN and per-size fuzz;
   fix wrappers to match; PSNR should jump to sane values everywhere.
1. **[LANDED 2026-07-13, opt-in] Chroma (4:2:0) support** — C SVT-AV1
   cannot emit monochrome; this was the structural prerequisite for
   bit-identity. `EncodePipeline::with_chroma_420(true)` +
   `encode_frame_420(y,u,v,stride)` emits mono_chrome=0 profile-0 420
   streams (SH color_config bit-identical to C for matching CICP; per-block
   UV_DC + per-plane chroma txbs with plane_type=1 CDFs and per-plane
   neighbor contexts; chroma reconstructed inside the entropy walk so
   coding order == parse order). Gates: 700/700 chroma decode-conformance
   matrix under aomdec + per-plane pixel probes (128px q30: Y 46.03 dB,
   U 51.92, V 52.86). Remaining policy limitations toward C parity:
   1a. **min-8x8 luma partition policy under 420** (`min_block_dim = 8`):
       4x4/4x8/8x4 luma blocks never occur, so every block is a chroma
       reference with chroma dims exactly (w/2, h/2) — AV1's sub-8x8
       is_chroma_ref/last-block-carries-chroma rules are NOT implemented
       yet and must be ported before decision-layer parity vs C (C will
       pick sub-8x8 partitions).
   1b. **Chroma mode search is UV_DC-only, cost-free** — no uv_mode RDO,
       no CFL, no chroma tx-type search; C's chroma RD must be ported for
       bit-identity.
   1c. **Still/key frames only** — inter frames would need chroma in the
       DPB + chroma-aware inter FH (asserted at runtime).
   1d. `svtav1::avif::encode_yuv420` still uses the legacy
       three-mono-stream format (output-contract migration pending; TODO
       in avif.rs).
2. **Filter signaling** — **deblocking DONE 2026-07-13**: key frames signal
   the q-picked loop_filter levels (svt_av1_pick_filter_level_by_q closed
   form; sharpness 0, delta_enabled 0) and the encoder applies the
   decoder-exact edge walk to the OUTPUT recon copy only (intra prediction
   keeps reading unfiltered pixels) — recon-parity 216/0 WITH filtering
   live; kernels differential-fuzzed vs C (tests/c_parity_lpf.rs).
   **CDEF DONE 2026-07-13**: SH enable_cdef=1; key-frame FH carries spec
   5.9.19 cdef_params with cdef_bits=0 (per-64x64 cdef_idx is then ZERO
   arithmetic-coder bits — libaom read_cdef aom_read_literal(r,0) is a
   no-iteration loop, bitreader.h:161 — so no EC syntax exists for it);
   kernels are C-exact ports of svt_cdef_filter_block_c /
   svt_aom_cdef_find_dir_c (svtav1-dsp/src/cdef.rs, differential-fuzzed
   over every signalable strength x damping x dir x bsize x border
   pattern, tests/c_parity_cdef.rs); application ports libaom
   av1_cdef_frame decoder-exactly (deblock -> CDEF on the output copy;
   pre-CDEF snapshot replaces the linebuf/colbuf machinery — provably
   identical single-threaded). Recon-parity 216/0 with CDEF FIRING
   (168/216 streams, 2.34M px filtered, 882k changed).
   2a. **CDEF strength policy is C's use_qp_strength fast path, NOT the
       C-default RDO search**: pick_cdef_params_key_frame ports
       svt_pick_cdef_from_qp (enc_cdef.c:849) intra branch bit-exactly
       (f32 fits pinned vs C for all 256 qindexes,
       tests/c_parity_cdef_pick.rs) + damping = 3 + (qindex>>6)
       (enc_cdef.c:923). C presets default to svt_av1_cdef_search
       (per-fb RDO over 64 strengths + cdef_bits>0) — that port lands
       with decision-layer parity (gap 3). Inter frames signal zero
       strengths (no CDEF), like inter deblock levels.
   Still open: restoration (neither signaled nor applied — consistent),
   inter-frame deblock/CDEF (signaled 0, applied nothing), the C
   SSE-based filter-level search (we ship LPF_PICK_FROM_Q only), and the
   CDEF RDO search (2a).
3. **Decision-layer parity** — partitions/modes/qcoeffs still come from our
   own RDO; port C mode decision after pixel-path parity.
4. **[FIXED 2026-07-13] Intra edge preparation** — directional
   predictions padded extension arrays with 128 where the decoder
   replicates edges / uses real above-right/bottom-left pixels. Fixed by
   porting has_top_right/has_bottom_left (+ verbatim has_tr_*/has_bl_*
   tables) and the dr-mode edge construction of build_intra_predictors
   from libaom reconintra.c (svtav1-encoder/src/intra_edge.rs), and by
   correcting dr_prediction_z2 to av1_dr_prediction_z2_c (dx/dy were
   swapped in the above/left branches — only D135 with dx==dy was
   unaffected — and base == -1 now reads the top-left sample via the new
   top_left parameter). This was the last recon-parity blocker: the gate
   went 102/6 → 108/0 (the 6 were gradient s2 ±1 at r0 c0/c63, D-modes
   at the top frame row coded against 128-padding the decoder never
   sees). Matrix since extended to 216/0 (added speed 4 + 96px padded
   content).
5. **Non-64-aligned frame dims (partial superblocks) unsupported** —
   every caller pads to 64-aligned before EncodePipeline (see
   decode_conformance/trace_one/AvifEncoder), so this is masked in all
   gates. Encoding an unpadded 96x96 panics in the encoder itself
   ("no TxSize for 4x32", coeff_c.rs:111): the mono search
   (min_block_dim=4) 4:1-splits partial-SB areas (e.g. 16x32) into
   unsignalable leaf shapes, and the partition writer has no
   split_or_horz/split_or_vert bool syntax or forced-split /
   skip-out-of-frame-children handling for partial SBs. Needed before
   true odd-size frame support; until then the padding convention is
   load-bearing.

### Recently fixed (2026-07-13, wave2/entropy-c-parity)
- **QP domain split (C-exact)**: RcConfig.qp is CLI 0..63 like C's
  `--qp`; the pipeline converts ONCE at frame setup via the verbatim
  `quantizer_to_qindex[64]` port (rate_control.rs, from C
  md_process.c:20) and everything downstream (quant tables, FH
  base_q_idx, CDF q bucket, chroma, deblock picker) consumes qindex
  0..255. Lambda stays CLI-qp-calibrated via `qindex_to_qp` (exact
  inverse on table values) until C's lambda_rate_tables.h port. The old
  conflation capped the reachable range at qindex 63 and made "qp 70/90"
  matrix cells silent duplicates of qindex 63.
- **VERT_A/VERT_B intra edge availability**: their children now select
  the C has_tr_vert_*/has_bl_vert_* tables (partition type threaded
  through encode_single_block -> build_directional_edges). The generic
  tables coded VertA D-mode children against above-right pixels the
  decoder decodes AFTER them — recon-parity went 211/5 -> 216/0 once
  high qindexes (80/172/255) let ext partitions win. Debug aids added:
  SVTAV1_DUMP_TREE=1 leaf dump, recon_parity full diff counts +
  per-case progress, EncodePipeline.last_recon_unfiltered +
  examples/deblock_evidence.
- 525/525 decode conformance via: C-exact range coder + update_cdf
  (differential-fuzzed vs libSvtAv1Enc.a), C default CDF tables + scan
  orders (generated + drift-tested), C-exact coefficient writer
  (av1_write_coeffs_txb_1d port with real txb_skip/dc_sign neighbor
  contexts), SH/FH/tile-group bit-layout fixes, ext-partition child
  geometry (was coding all children at the parent origin), angle_delta
  gated to >=8x8 blocks, and mode syntax made unconditional (skip only
  gates residuals — this was the all-skip decode failure).
- C baseline updated to upstream v4.2.0-rc; all parity suites re-verified
  against the new library.

## Investigation Notes

### Transform Parity
All 26 1D transform kernels are bit-exact with C SVT-AV1. Verified by extracting golden data from C object files (`cbuild/Source/Lib/Codec/CMakeFiles/CODEC.dir/transforms.c.o` and `inv_transforms.c.o`). The C functions accept `(input, output, cos_bit, stage_range)` — we pass `cos_bit=12` and `stage_range=NULL` for forward (ignored), `wide_range=[31;12]` for inverse (clamping never triggers at 8-bit).

Key finding: the C `svt_av1_fadst4_new` uses i32 arithmetic while our initial port used i64, producing different rounding. Fixed by matching the C decomposition exactly. Same issue with `fadst8` output permutation — C uses `[step[1], step[6], step[3], step[4], step[5], step[2], step[7], step[0]]` without negation, while our initial port had sign flips.

### Pipeline Architecture
The pipeline processes superblocks in raster order (left-to-right, top-to-bottom) per spec 00. Each SB goes through partition_search which recursively tries all 10 partition types. At each leaf, encode_single_block evaluates 11 intra modes with mode-specific TX RDO, picking the lowest RD cost. Deblocking runs after the entropy walk on the OUTPUT recon copy only (signaled levels; prediction sources stay unfiltered — the decoder's split); CDEF/Wiener/sgrproj stay disabled+unsignaled pending their ports.

### Performance
Release-mode benchmarks (x86_64 AVX2):
- SAD 16x16: ~18 Gpix/s (archmage auto-vectorization)
- fwd_txfm 4x4: ~170 Mpix/s
- fwd_txfm 8x8: ~215 Mpix/s
These numbers are MEASURED, not estimated.
