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
> canonicalized od_ec op traces. Divergence map with per-class C source
> cites + priority-ordered fix list: **docs/IDENTITY-STATUS.md**.
> **FIRST BYTE-IDENTICAL STREAM 2026-07-13** (commits d72a7641..85d7e0fd):
> `identity_diff.sh 64 64 40 13 uniform` → VERDICT: streams IDENTICAL
> (22 bytes, 5/5 EC ops incl. rng). Landed: aq_mode=0 CQP gating (VAQ
> off by default), C-exact SH level auto-derivation + CICP-unspecified
> defaults, TX_MODE_SELECT + per-block tx_depth syntax, real
> entropy-cost partition rates.
> **PRESET-6 IDENTICAL 2026-07-13** (commits 084d2c13e+eab9d8860):
> matrix 18/36 — uniform is byte-identical at ALL tracked
> presets (13/10/6) x {64,128} x qp{20,40,55}. Landed: C-exact allintra
> SH tool bits (filter_intra/restoration per preset,
> seq_tools_for_preset), FH lr_params all-RESTORE_NONE syntax,
> per-block use_filter_intra flag (DC <=32x32, always 0), CDEF
> all-skip-frame search outcome (bits=0/strengths 0).
> **PRESET-5 FUNNEL 2026-07-14 (later)**: matrix 102/132 — presets
> 5-10 all 72/72 (M5 leaf funnel: PAETH mode_end + angular deltas +
> SH edge-filtered prediction + MDS3 ind-uv + txt 6/6/15/250; deblock
> chroma-TX-from-block-dims fix). Remaining 30 = gradient at M0-M4.
> See docs/IDENTITY-STATUS.md 2026-07-14 M5 chunk.
> **PRESET-4 + DEPTH REFINEMENT 2026-07-14 (latest)**: matrix
> **108/132** — presets 4-10 all 84/84 (a4ec124a0+ee5b08c65: M4 funnel
> config — intra_level 1 all-7-angle-deltas, nic case 5 zero-rank
> semantics, unfiltered prediction — PLUS the PD1 depth-refinement +
> inter-depth decision port, depth_refine.rs: dr_mode 1 ADAPTIVE at
> M0-M5, PD0-cost deviation gates, funnel evals at admitted depths,
> bias-995 compares at real partition contexts; presets 4..5 now
> decide trees through the walk — M5 re-verified 12/12 through it).
> Remaining 24 = gradient at M0-M3 (nsq search, 4x4, bypass_encdec=0,
> PD0_LVL_0 at M0/M1, cfl at M0). See docs/IDENTITY-STATUS.md
> 2026-07-14 M4 chunk.
> **MATRIX COMPLETE 36/36 — 2026-07-14** (M6 leaf funnel chunk:
> captures 265830cf7, kernels 2fc88e564, funnel 725fd3b09, per-SB CDF
> chain 661efa7bc): the C-exact M6 leaf intra funnel (MDS0 Hadamard
> fast loop -> NIC pruning -> MDS1 quantize_b full loop -> MDS3
> TXS/TXT/RDOQ/chroma full loop, filter-intra candidates, uv-follows-
> luma chroma decisions, per-SB rate-table refresh into both the
> funnel and the M6 PD0) closed ALL 6 gradient preset-6 cells. Every
> tracked identity cell is byte-identical. Residual non-cell gaps:
> CFL evaluation when the chroma detector arms (never on tracked
> content), avg_cdf_symbols for frames > 2 SBs wide, funnel scope
> preset 6/420-still only. See docs/IDENTITY-STATUS.md 2026-07-14.
> **GRADIENT M13/M10 IDENTICAL 2026-07-13** (PD0 partition port
> ffb73bcf2+60b006b85, is_dc_only_safe leaf gate b7f362af4, C-exact
> coding quantizer ee18fed11 — quant.rs: quantize_b / quantize_fp +
> svt_av1_optimize_b RDOQ trellis + coeff_lvl->rdoq_level policy):
> matrix now **30/36** — every uniform AND every gradient M13/M10 cell
> is byte-identical. Remaining: NONE — the M6 leaf funnel
> (2026-07-14) closed the final 6 cells; the matrix is 36/36.
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
   2a. **CDEF strength policy matches C per preset EXCEPT the live-block
       search**: C's allintra policy is preset-split (enc_mode_config.c:
       3543-3600) — presets >= M7 use the use_qp_strength fast path
       (pick_cdef_params_key_frame ports svt_pick_cdef_from_qp
       enc_cdef.c:849 intra branch bit-exactly, f32 fits pinned for all
       256 qindexes, tests/c_parity_cdef_pick.rs; damping 3+(qindex>>6),
       enc_cdef.c:923) and presets <= M6 run svt_av1_cdef_search. Of the
       search, ONLY the sb_count==0 outcome is ported (every filter
       block all-skip -> cdef_bits=0, strengths 0/0 — deterministic,
       enc_cdef.c:1296-1449; cdef.rs pick_cdef_params_all_skip_search):
       C-exact for flat/all-skip frames (uniform p6 identity cells prove
       it). Frames with ANY live filter block at presets <= M6 still
       take the qp fast path — self-consistent (signal == apply) but
       divergent from C's searched strengths (1 of 16 matrix stages:
       gradient64 q55 p6 FH). The per-fb mse search port (64 strengths x
       joint_strength_search_dual + lambda rate) lands with
       decision-layer parity (gap 3). Inter frames signal zero
       strengths (no CDEF), like inter deblock levels.
   **Wiener loop restoration DONE 2026-07-14** (commits
   a724ebdc0..bbf1ddb69): C-exact kernel + stripe machinery
   (svt_av1_wiener_convolve_add_src_c / svt_av1_loop_restoration_
   filter_unit incl. the deblock/CDEF boundary line buffers,
   differentially fuzzed — svtav1-dsp/src/restoration.rs,
   tests/c_parity_wiener.rs), the tap-coding chain (refsubexpfin
   exhaustive byte-parity, svtav1-entropy/src/lr.rs), the FULL search
   (restoration_seg_search + rest_finish_search at the allintra
   wn_filter controls; sgrproj NEVER searched — sg_filter_lvl=0;
   svtav1-encoder/src/restoration.rs, validated to reproduce C's taps
   and picks on C's exact dgd for all 6 instrumented cells —
   tests/lr_search_c_capture.rs pins two of them), FH lr_params +
   per-SB tile syntax (tile re-walk when wiener signals), and the
   decoder-exact frame application on the output copy. recon-parity
   216/0 with 59/216 streams signaling wiener (107 RUs incl. chroma).
   Still open: inter-frame deblock/CDEF/LR (signaled 0, applied
   nothing), the C SSE-based filter-level search (we ship
   LPF_PICK_FROM_Q only), the CDEF RDO live-block search remainder
   (2a), and sgrproj (out of scope — C never searches it at any
   representable allintra preset).
3. **Decision-layer parity** — CLOSED for still/420 presets 4-10
   (2026-07-13/14: leaf_funnel.rs MDS0/MDS1/MDS3 funnel for
   intra_level 1/2/6/7/8 + depth_refine.rs PD1 depth
   refinement/inter-depth decision at M4/M5; matrix 84/84 for
   presets 4-10). Still homegrown: presets <= 3 (nsq search, 4x4,
   bypass_encdec=0, PD0_LVL_0 at M0/M1, cfl_level 1 at M0), mono
   leaf decisions (C cannot emit mono), inter frames, and CFL
   evaluation when the chroma detector arms.
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
The pipeline processes superblocks in raster order (left-to-right, top-to-bottom) per spec 00. Each SB goes through partition_search which recursively tries all 10 partition types. At each leaf, encode_single_block evaluates 11 intra modes with mode-specific TX RDO, picking the lowest RD cost. The loop-filter chain runs after the entropy walk on the OUTPUT recon copy only (prediction sources stay unfiltered — the decoder's split), in decoder order: deblock -> CDEF -> Wiener loop restoration. The LR search needs the post-CDEF recon, so when it signals wiener the entropy walk is re-run with the per-SB lr syntax (the walk is a deterministic re-runnable pass). sgrproj is never searched (C sg_filter_lvl = 0 at every representable allintra preset) and stays unported.

### Inter/Motion DSP Audit vs v4.2 C (2026-07-14, wave2/entropy-c-parity)
Differential-tested the 7 pre-v4.2-bump DSP modules that had inline tests but
ZERO cref coverage. All 7 C reference files are UNCHANGED 4.1->4.2
(v4.2_functions.md), so findings are pre-existing port state, not drift.
cref oracles + `tests/c_parity_{sad,variance,inter_pred,obmc,warp,scale,superres}.rs`.

- **sad.rs** — VERIFIED bit-exact vs `svt_aom_sad{W}x{H}_c` (all 22 sizes).
- **variance.rs** — `sse()` bit-exact vs the C variance kernel's `*sse`;
  single-block `variance()` bit-exact vs the exact C-derived numerator
  (`N*sum(x^2)-sum(x)^2`). NOTE: `variance()` is an N^2-scaled single-block
  helper, NOT the two-block `svt_aom_variance*` (which returns `sse-sum^2/N`).
- **inter_pred.rs** — `convolve_horiz/vert` bit-exact vs
  `svt_aom_convolve8_{horiz,vert}_c` (all 16 phases); `convolve_2d` matches the
  u8-intermediate 2-pass (the `svt_aom_upsampled_pred_c` ME path). It is NOT the
  reconstruction convolve `svt_av1_convolve_2d_sr_c` (16-bit intermediate) —
  that kernel is unported.
- **obmc.rs** — was STALE, now FIXED: wrong mask tables (had e.g. overlap-4
  `[53,32,11,4]` vs C `[39,50,59,64]`) AND inverted blend weighting. Now
  bit-exact vs `svt_av1_get_obmc_mask` + `svt_aom_blend_a64_{v,h}mask_c`. This
  is the only audited DSP module the encoder imports (partition.rs inter path;
  dormant for the still gates — `mv_map` is None on key frames).
- **warp.rs / scale.rs / superres.rs** — STUBS (0 in-tree callers), homegrown
  approximations, NOT ports of `svt_av1_warp_affine_c` /
  `svt_av1_convolve_2d_scale_c` / normative `upscale_normative_rect`. Oracles
  shimmed + gap-pinning tests (flip `assert_ne!`->`assert_eq!` when ported).
  A real port needs: warp -> 193-phase `svt_aom_warped_filter` + shear
  (alpha/beta/gamma/delta) + ROUND0/1 + 8x8 tiling (Rust also double-scales
  m0/m1 by 1<<16); scale -> SCALE_SUBPEL_BITS=10 EIGHTTAP + 16-bit ROUND0/1;
  superres -> 64-phase `svt_av1_resize_filter_normative` + RS_SCALE_SUBPEL_BITS=14.
Gates after the OBMC fix: workspace tests 647/0, recon_parity 378/0,
decode_conformance 840/0.

### Performance
Release-mode benchmarks (x86_64 AVX2):
- SAD 16x16: ~18 Gpix/s (archmage auto-vectorization)
- fwd_txfm 4x4: ~170 Mpix/s
- fwd_txfm 8x8: ~215 Mpix/s
These numbers are MEASURED, not estimated.
