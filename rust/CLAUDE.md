# SVT-AV1 Rust Port Rules

## CONFORMANCE MANDATE

**NEVER stop working while ANY conformance or parity issue remains.** If the bitstream does not decode with rav1d-safe at ALL tested sizes, the work is NOT DONE. If any differential test shows a decode failure, investigate the root cause and fix it before committing documentation, before updating handoffs, before doing anything else. Conformance failures are the #1 priority — above new features, above performance, above code cleanup. Do not describe a conformance failure as "expected" or "known" — describe it as "BLOCKING" and fix it in the same session.

This applies to:
- Bitstream decode failures at any image size
- Parity mismatches between C golden output and Rust implementation
- CDF/entropy coding mismatches that produce non-decodable output
- OBU structure errors that cause decoder rejection

**The definition of "done" for any encoding feature is: rav1d-safe decodes the output correctly at all tested sizes.**

## FIXED 2026-07-18 — palette blocks coded an EXTRA filter_intra flag (4:2:0 decode-conformance desync)

**ROOT + FIX (one line):** `encode_block_syntax` (pipeline.rs:2680) coded the
`use_filter_intra` flag for DC-mode ≤32×32 blocks but was **missing C's
`palette_size == 0` gate** (`svt_aom_filter_intra_allowed`, mode_decision.c:107).
A winning palette block (DC, ≤32) therefore emitted an EXTRA symbol the decoder
never reads → whole-tile desync. Latent while palette was never picked
(allow_screen_content_tools=0 historically); it fired the moment screen-content
frames started winning palette blocks. Fix: add `&& decision.palette.is_none()`.
The RATE side was already correct (4543a3651 prices palette candidates 0
filter-intra bits) — only the PACK emitted the stray flag, which is why the RD
was fine but the coded bytes desynced. **Result: 4:2:0 decode-conformance
1260/1260 (was 99 failures), mono 945/945, non-sc matrix 54/54 unchanged
(byte-neutral — non-palette blocks keep `palette.is_none()==true`), full
workspace 813/0.** CI-covered by the existing decode_conformance 420 gate (which
includes the 48/80/96 palette-triggering content). Harness gained
`identity_run` `raw:<yuv>` + `SVTAV1_MONO` modes (used to localize this).
The full localization trail (kept for method reference):

### (historical) sub-8 chroma-ref pairing desync investigation

**CI has been RED since 2026-07-17T00:31 (commit 5eb8e5d97, the PRIOR session) — ~20h before it was noticed.** The `svtav1-rs-gates` job's "Decode conformance — 4:2:0" step fails. It was MASKED in the job log by a second failure (the mono `encode_frame` 64-multiple guard, c17fb1b53, broke the earlier "Workspace tests" step; GitHub Actions stops the job at the first failing step). That second failure is FIXED (d41704495); this pre-existing one remains the red.

- **Symptom:** 99/1260 4:2:0 decode-conformance streams fail aomdec ("Corrupt frame detected: Failed to decode tile data"). Exactly the TEXTURED content (`gradient`, `color` — NOT `uniform`) at the sizes that pad from {48, 80, 96} (→ 64/128). The MONO equivalent of every failing stream DECODES — so it is chroma-specific.
- **PRE-EXISTING, not this session's regression (proven):** a worktree build at the pre-session base `dcd725a64` fails the identical streams; my session never touched the 420 encode path in a way affecting 128-aligned content (edge coding is byte-neutral there, content-independently — every node has both edge flags true). My session ADDED a separate failure (mono guard) and FIXED it.
- **It IS a port bug (proven):** C (`capture_c_trace`) encodes the exact same content into a decodable stream (gradient_48x48→64×64: C 511B decodes, port 561B desyncs).
- **LOCALIZED:** first diverging op is op 1 — at a 32×32 node C codes PARTITION_SPLIT(3), the port codes PARTITION_VERT_4(9). The port legitimately picks a 4:1 partition (RD divergence — decode-conformance is not a byte-match gate), which at preset 0 (min_block_dim=4) produces sub-8 blocks (16×4, 4×16, 8×32). The design assumed min-8×8 ("sub-8x8 chroma-ref rules deferred", pipeline.rs:65/2429); the 4:1/AB partitions violate that. **ROOT CONFIRMED = PALETTE coding on 4:2:0 (bisect, 2026-07-18).** Forcing `funnel_cfg.palette_level = 0` (pipeline.rs:3445) and re-encoding gradient_48x48 via `raw:` → the stream DECODES CLEANLY (559B, exit 0). So the desync is in the PALETTE path on 420, NOT general sub-8 chroma. RULED OUT en route: (1) search/pack `has_uv` / `blk_has_uv` (leaf_funnel.rs:2292 / pipeline.rs:2440) implement `is_chroma_reference` identically and agree; (2) chroma PAIR geometry identical funnel(:2323)/pack(:2447), hand-traces correct; (3) 4:1 (HV4) partitions — bisect `allow_hv4=false` STILL desyncs, so the op-1 VERT_4-vs-SPLIT was a red herring; (4) the UV-palette-flag `is_chroma_ref` (pipeline.rs:2661 `chroma_blocks.is_some()`) is consistent with `blk_has_uv`. **Mono passes** with the same palette LUMA (colors+map+flags all correct), so the desync is specifically the palette block's interaction with CHROMA coding on 420 — the UV-palette-flag / palette-map-token / coding-ORDER relative to the chroma txbs. **Pre-existing, NOT this session's palette work** (#71 REDUCED over-picking 712→696 but the pre-session encoder over-picks AND desyncs identically). aomdec/aomdec-debug do not report the desync offset. Note this over-picking connection: the port codes MANY palette blocks on this content (the #71 over-picking) where C codes few; each extra palette block on 420 risks the desync, so #71 over-picking AMPLIFIES exposure even though it is not the coding-bug root.
- **REPRO (committed harness):** `identity_run` now takes `raw:<i420.yuv>` content. Generate the exact YUV (see the g48 generator in the session log — 48×48 gradient replicated to 64×64 + flat-128 chroma), then `tools/identity_diff.sh 64 64 20 0 raw:<yuv>` shows op-1 divergence and produces the port's undecodable `rs.obu`. `SVTAV1_PACKTREE=<f>` dumps the port's partition tree (shows the 4:1 → sub-8 blocks).
- **NEXT (the fix pass) — root is PALETTE-on-420, exact symbol mismatch still to pinpoint.** Exhaustively RULED OUT by reading C-vs-port: `is_chroma_reference`/`has_uv`, chroma pair geometry, 4:1/HV4 (bisect), `allow_palette` (both `bsize>=BLOCK_8X8` enum-order incl. 4X16/16X4/8X32; block_size_index maps correct), the UV-palette-flag `is_chroma_ref` gate, and the `[Y-flag,colors,UV-flag]`-then-map ORDER (C write_palette_mode_info entropy_coding.c:4355 matches). Since MONO passes (all palette LUMA syntax — flag/size/colors/cache-flags/map — correct) and the 420-only delta is {UV-flag (ruled out), chroma-coeffs}, the remaining suspects are: (a) the palette candidate's CHROMA decision in the funnel (`decision.chroma_dec` for a palette winner) being inconsistent with what the pack codes — check whether the palette candidate reconstructs/decides chroma the same as a regular UV_DC candidate; (b) the palette block's chroma tx-size/type. TO PINPOINT: build a position-reporting decode of the port's `rs.obu` (aomdec/dav1d report only "tile data" with no offset), OR finer-bisect (zero the chroma coeffs of palette blocks only; or reduce to a SINGLE palette block). Do NOT band-aid by disabling palette on 420 (C codes this into a decodable stream — the port's palette-420 chroma must be fixed to match). #71 over-picking AMPLIFIES exposure (more palette blocks = more desync surface) but is not the coding-bug root. This is the #1 gate per the mandate above.

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
under the AV1 reference decoder as of 2026-07-13; C baseline retargeted to
the final v4.2.0 tag 2026-07-16 — all-intra output byte-identical to the old
v4.2.0-rc pin, so all gates carried over unchanged)

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
   2a. **CDEF strength policy matches C per preset; the full live-block
       search is ported AND VERIFIED C-exact (2026-07-15 M6 chunk 2)**:
       C's allintra policy is preset-split (enc_mode_config.c:3543-3600)
       — presets >= M7 use the use_qp_strength fast path
       (pick_cdef_params_key_frame ports svt_pick_cdef_from_qp
       enc_cdef.c:849 intra branch bit-exactly, tests/c_parity_cdef_pick.rs;
       damping 3+(qindex>>6)) and presets <= M6 run the full
       svt_av1_cdef_search (cdef.rs cdef_search_still + finish_cdef_rd +
       joint_strength_search_dual + the default_mse_uv*64 sentinel + the
       M6=level-7 set_cdef_search_controls candidate set fs=[0,60,2,62]).
       Scratch-C instrumentation of cdef_seg_search/finish_cdef_search on
       real content (1001682 q40 p6) proves it: every 64x64 filter block
       whose post-deblock recon matches C produces BYTE-IDENTICAL per-fb
       luma+UV mse rows and the RD pick logic matches. So the search is
       NOT the real-content gap. The `FH | cdef_uv_pri_strength[0] C=0
       Rust=15` divergence is a *downstream symptom*: the post-deblock
       recon feeding the search still diverges (avg_cdf fix 9563ac471
       fixed SB rows 0-2, rows 3+ still cascade). See
       docs/IDENTITY-STATUS.md "2026-07-15 ... M6 chunk 2" for the full
       ruled-out list and the leaf-level next step. Inter frames signal
       zero strengths (no CDEF), like inter deblock levels.
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

### Inter/Analysis Encoder-Module Audit vs v4.2 C (2026-07-14, wave2/entropy-c-parity)
Audited the 5 pre-v4.2-bump modules with inline tests but ZERO cref coverage
(motion_est, film_grain, temporal_filter, multipass, mv_coding). Change-status
from `mainline_v4.2.bit-affecting.diff`. cref oracles + new c_parity suites.

- **mv_coding.rs (NORMATIVE) — was BROKEN, now FIXED bit-exact.** The old port
  wrote raw literals for joint/class/sign/bits (and coded X before Y) instead of
  CDF symbols — non-decodable. Dormant on the still gates (write_mv only under
  `decision.is_inter`; pipeline.rs:1718 debug_asserts the 420/still path is
  intra-only), so the conformance matrix never caught it. Rewrote as a bit-exact
  port of svt_av1_encode_mv + encode_mv_component + svt_av1_get_mv_class + the
  default_nmv_context CDFs. The MV-encode path itself is UNCHANGED 4.1->4.2
  (pre-existing wrong port, not drift). cref: ref_encode_mv_seq + ref_get_mv_class
  + FcTable::Nmvc; tests/c_parity_mv.rs (exhaustive class, 143-entry nmvc drift,
  byte-exact seqs w/ CDF adaptation across all 3 precisions). Follow-up: the
  inter path still needs a persisted per-frame nmvc + real ref_mv subtraction
  (write_mv uses fresh default CDFs per call — documented).
- **temporal_filter.rs — estimate_noise_fp16 PORTED bit-exact** vs
  svt_estimate_noise_fp16_c (temporal_filtering.c; body unchanged 4.1->4.2 —
  nearby bit-affecting hunk is VMAF RTCD decls only). cref ref_estimate_noise_fp16
  + tests/c_parity_temporal.rs. The temporal_filter() planewise blend + old f64
  estimate_noise are HOMEGROWN heuristics (NOT svt_av1_apply_temporal_filter_
  planewise_medium), non-normative, inter-only/dormant.
- **motion_est.rs — HOMEGROWN, not a port.** Full-pel raster SAD + BILINEAR
  subpel; C svt_aom_motion_estimation_b64 / mcomp subpel / av1me are all
  bit-affecting-changed 4.1->4.2 and untracked. Inline SAD IS C-equivalent:
  tests/c_parity_motion_est.rs pins full_pel_search distortion == svt_aom_sad at
  the chosen MV. Non-normative, inter-only/dormant.
- **film_grain.rs — HOMEGROWN + INERT.** estimate_film_grain output is discarded
  (pipeline `_grain_params`) and obu.rs always emits film_grain_params_present=0.
  Not a port of noise_model.c/grainSynthesis.c (bit-affecting-changed). No FH
  grain write path exists; no single-fn oracle. Documented.
- **multipass.rs — HOMEGROWN + UNWIRED (zero callers).** Not a port of
  firstpass.c (unchanged) / initial_rc_process.c / pass2_strategy.c (changed).
  Non-normative. Documented.
Gates after: workspace tests 662/0. Still-image gates provably unaffected
(intra-only still path). C tree pristine.

### Performance
Release-mode benchmarks (x86_64 AVX2):
- SAD 16x16: ~18 Gpix/s (archmage auto-vectorization)
- fwd_txfm 4x4: ~170 Mpix/s
- fwd_txfm 8x8: ~215 Mpix/s
These numbers are MEASURED, not estimated.

## PRODUCTION PRIORITIES (user directive 2026-07-17 — BINDING ORDER)

1. **Ship-blocking correctness + coverage**: 10-BIT SUPPORT and ARBITRARY
   DIMENSIONS are required for production. In-flight structural verticals
   (palette/sc, tiles, SB128) continue — they are production surface.
2. **Long-term code quality & human maintainability**: clear module
   boundaries, documented invariants, honest PORT-NOTE index, rustdoc on
   public surfaces, no dead code left behind, no mega-file growth
   (leaf_funnel.rs is slated for a module split AFTER the EPICA
   calibration lands — do not thrash hot files mid-drill).
3. **Lossless (q0)**: LESS important — do not prioritize over the above.
4. **Performance (#93)**: LAST. Algorithmic/allocation work before SIMD
   when it does happen.


## STANDING DIRECTIVE — KEEP GOING ON ALL REMAINING WORK (2026-07-17)

Every session (including post-compaction resumes): pick up the queue below
and CONTINUE until all gates pass. Do not wait for prompting; do not
re-litigate priorities (they are in "PRODUCTION PRIORITIES" above).

### Do-not-clobber map (concurrent HDR-fork session — read docs/HDR-ON-4.2.md first)

The hdr-hybrid workstream actively lands on master. NEVER touch its
surface: `hdr_mode.rs`, `var_boost.rs`, `chroma_q.rs`, `noise_gen.rs`,
`noise_norm.rs`, `qm.rs`, `qm_tables.rs`, `tx_bias.rs`, `tune.rs`,
`ssim_md.rs`, `tests/hdr_fork_e2e.rs`, the gate examples
(`hdr_fork_smoke.rs`, `noise_gate.rs`, `tune_gate.rs`),
`xtask/transcribe_qm.py`, and the `#if SVT_HDR_MODE` C paths. In SHARED
files preserve their fork arms exactly: `leaf_funnel.rs` (`mds0_ssd`
SSD-vs-SATD<<4 MDS0 arm, the noise-norm call in `tx_unit`), `pd0.rs`
(`kf_full_lambda_8bit_ex` qdiff factors), `quant.rs` (light-RDOQ,
rdoq_rdmult_sharp, QM routing), `pipeline.rs` (per-SB delta-q + chroma
qindex threading), `deblock.rs` (`sharpness` param). Their invariant =
ours: MODE0/Mainline output byte-identical to stock v4.2.0-final. Rebase
over their commits (fetch+rebase+push retry loop); on conflict in a
shared file, THEIR fork arms win, OUR mainline arms win.

### The remaining-work queue (work top to bottom; per-item state lives in the task tracker + docs/ maps)

1. **#71 palette calibration**: PARTIAL — the per-class NIC lane +
   per-class MDS0 dev-threshold prune landed (ba58a3ec2): C's
   `post_mds0_nic_pruning` (product_coding_loop.c:7840) prunes PER
   candidate class against that class's own best fast cost; the port had
   run it over the sorted union vs the global best, so palette (far lower
   fast cost on sc) pruned out every regular mode and won by default.
   EPICA p6 q32 palette blocks **2064 -> 516** (C: 178), bytes 14621 ->
   14409; non-sc matrix 36/36 unchanged (fast path byte-identical by
   construction). RESIDUAL (516 vs 178) is a genuine full-RD WIN, not
   survivor starvation: at a port-only block (mi 18,60) all regular modes
   now reach MDS3 but palette n=5 full=18.8M beats best regular ~26.4M —
   uniform neighbours make every intra mode predict the same flat value
   (satd 26304, dist 47863) while palette reconstructs the text interior
   (dist 19312). C picks H_PRED there, so C's palette must cost more OR
   its regular cost less.
   **ROOT CAUSE CORRECTED 2026-07-17 (measured via real-C leaf-fn drill,
   OBU byte-verified — the rate premise was FALSE):** C's palette luma
   rate at the first divergence mi(20,6) is bit-EXACT to the port —
   map-token cost 26693 == 26693, color cost 22528 == 22528, ysize /
   uniform / ymode all exact. `palette.rs` (`color_map_wavefront` /
   `palette_color_index_context` / `delta_encode_bits`) is VERIFIED
   BIT-EXACT — **do NOT touch it.** The real divergence is at **MDS3, on
   the DC candidate**: measured C costs at (24,80) are MDS1 DC 18.9M /
   palette-6 10.78M (palette wins MDS1 in C too, both survive), but
   **MDS3 DC = 8,538,560** (ycb 35017, ydist 41507→**10128** via residual
   coeff coding) vs palette-6 10,776,028 → **C's DC wins at MDS3.** The
   port's palette MDS3 (~10.9M) ≈ C's (faithful). So the port is NOT
   dropping its DC candidate's MDS3 cost to ~8.5M — either its MDS3
   residual/coeff/dist path or DC not surviving MDS1→MDS3 while palette
   does — it was the SURVIVAL: **FIXED 2026-07-17 (765d60a7e).**
   **DC-MDS3 ROOT = the MDS1→MDS3 per-class prune (the MDS0 fix's
   sibling).** C's `post_mds1_nic_pruning` (:7885) + `post_mds2` (:7961)
   loop PER CLASS against each class's own best full_cost; the port ran
   them over the sorted UNION vs the global (palette) best, so DC (full
   18.9M, +73% from palette 10.9M) was pruned before MDS3 and never
   reached the stage where its residual-coded MDS3 cost (8.5M) beats
   palette. Fix: per-lane prune (incl. C's per-class rank staging +3/+2),
   union+sort for MDS3; single-class path byte-identical. RESULT: mi(20,6)
   now codes DC like C, the first EPICA divergence advanced mi(20,6)→
   **mi(22,6)**, bytes **14409→13189** (C 13097, 0.7% off), matrix 54/54,
   suite green. The fi_flag over-price is also FIXED (4543a3651). **NEW
   first divergence mi(22,6) — ROOT FOUND (agent, real-C OBU-verified):
   the neighbor palette COLOR CACHE is empty in the port's palette
   search.** Not a near-tie: C's n=8 (25.28M) beats n=5 (28.60M) by 3.32M
   and n=5 never even reaches MDS3 in C. The block has `n_cache=4` (above
   +left neighbors carry palettes) where mi(20,6) had n_cache=0 (why it
   was bit-exact). C's k-means branch (palette.c:513) runs
   `palette_rd_y(opt_colors=TRUE, color_cache, n_cache)` →
   `optimize_palette_colors` snaps centroids toward the neighbor cache;
   the port passes an EMPTY cache, so its n=8/n=5 COLORS diverge (measured:
   port n=8 total_rate ~115556 vs C 110356; port n=5 fits the block better
   than C's, coeff 22595 vs 38458 — different palettes both sizes). coeff
   738==738 and the map-cost machinery are bit-exact, so it is NOT a
   residual or map-cost bug. **THE MD-CACHE FIX IS IMPLEMENTED 2026-07-18
   (this session) — faithful to C, but it does NOT close the cascade; the
   prior "closes mi(22,6) + the whole cascade" prediction was MEASURED
   WRONG.** The 4 parts landed (all verified line-by-line vs palette.c +
   rd_cost.c): (1) `commit_leaf` (leaf_funnel.rs) stamps
   `fx.ectx.record_palette(winner)` per committed block in coding order —
   the MD-time neighbour state, mirroring the pack walk's :2935 stamp +
   `record_block`; (2) `evaluate_leaf` reads `palette_cache(&*fx.ectx,
   abs_x, abs_y)` (the ported `svt_get_palette_cache_y`) into
   `search_palette_luma` (was `&[]`) — feeds the k-means centroid snap
   (`optimize_palette_colors`, opt_colors=TRUE) + the cost; (3) cache-aware
   colour cost `(n_cache + delta_encode_bits(index_color_cache(cache,
   colors).new)) << 9` (C `svt_av1_palette_color_cost_y`, palette.c:143);
   (4) the palette-Y mode flag rate is now `palette_ymode_fac_bits[bctx]
   [mode_ctx]` — the rate table (`MdRates::palette_y_no/yes`) gained the
   `mode_ctx` dim, indexed by the ALREADY-EXISTING
   `EntropyCtx::palette_neighbor_ctx` (C `svt_aom_get_palette_mode_ctx`),
   for BOTH the DC no-flag and the palette yes-flag. All 4 are
   byte-identical at n_cache==0 + mode_ctx==0 → **non-sc matrix 54/54 +
   244/244 unit tests green (safe by construction — no palette winner ⇒
   empty neighbour state ⇒ every non-screen leaf unchanged).**
   **MEASURED on EPICA p6 q32 (clean stash before/after): bytes 13192 →
   13153 (toward C 13097), BUT palette blocks 556 → 712 (AWAY from C's
   ~178), first divergence op 48 → 43 (EARLIER).** The fix is correct yet
   AMPLIFIES the over-picking via a FEEDBACK LOOP: the port's pre-existing
   over-picking (556 vs 178, from before this fix) gives it MORE palette
   neighbours than C ⇒ bigger caches than C at the same block ⇒ palette
   even cheaper ⇒ even more palette. So the cache fix can only be
   byte-VALIDATED once the over-picking root is fixed (then port + C share
   the same neighbour state). **THE REAL EPICA ROOT is the over-picking
   FULL-RD divergence (see the para above, mi 18,60): the port's palette
   full-RD beats the regular modes where C picks a regular mode — either C
   costs palette MORE or regular LESS at those blocks. The cache is NOT
   that root.** NEXT: real-C leaf-fn drill comparing the port's vs C's
   FULL cost for {palette n5/n8, best regular} at a stable over-picked
   block, to localise which side (palette or regular full-RD) diverges —
   the MDS0/MDS1/MDS3 per-class dev-prunes were correct+necessary but did
   not fully converge the survivor→winner set. (pipeline.rs:1531 stale
   PORT-NOTE updated this session.) Gate: EPICA p6/p7 q32 byte-match
   (13097B / 14736B) — still OPEN (RD/over-picking gap only).
   **NOTE 2026-07-18: EPICA now DECODES cleanly (was undecodable).** The
   palette filter_intra conformance fix (a0b505b4f) applied to EPICA too —
   its palette-heavy stream is now valid (13173B vs C 13097B, decodes under
   aomdec). So #71 is now purely an RD-efficiency gap (byte-match), NOT a
   correctness/desync issue. The over-picking drill above is the remaining
   work; it no longer blocks decodability.
2. **#71 IBC wiring**: wire `intrabc.rs` into the funnel injection
   (palette_hint coupling) + FH allow_intrabc for M2-M4 sc + the already
   -dormant obu.rs LF/CDEF/LR skips. Gate: EPICA p2-p5 cells.
3. **#86 tiles payload gaps**: per-tile LR handling + leaf_funnel
   neighbor availability via tile_top_px (same pattern as partition.rs).
   Gate: the 3 recorded 2-tile-row cells IDENTICAL.
4. **#95 arbitrary dims integration** (P0): **CHUNK 1 LANDED** — the
   pipeline now carries TWO dim systems (`true_width/height` vs the
   ALIGNED `width/height` = round-true-up-to-8): `new()` computes aligned
   via `frame_geom::FrameDims`, `encode_frame_420` edge-replicates the
   input planes TRUE->ALIGNED (C `pad_input_picture`), the seq header
   carries TRUE dims (`max_frame_width_minus_1`), and the small-frame
   restoration disable (enc_settings.c:214-232: true w|h < 64 clears
   `enable_restoration`) is replicated. SCOPE: aligned dims a multiple of
   64 (full SBs) — dims in {57..64} e.g. 60x60 -> 64x64. VERIFIED: 60x60
   uniform+gradient byte-identical vs SvtAv1EncApp across presets 13/10/6
   × q20/40/55 (18/18); `60` added to the default identity_matrix (now
   54/54); 64-aligned regression 36/36; 196 encoder tests green; mono
   path preserved.
   **CHUNK 2 IN PROGRESS (partial SBs — aligned NOT a mult of 64).**
   Landed so far: (a) `ebd770d1b` the ENTROPY + PACK edge coding — C
   `encode_partition_av1` (entropy_coding.c:932-981) + both CDF gathers
   (cabac_context_model.h:378-406) as `write_partition_edge` /
   `partition_gather_{horz,vert}_alike` / `cdf_element_prob`, and
   `pipeline::partition_edge_flags` (derives the ALIGNED extent from the
   `DeblockGeom` already threaded through the walk, so partition edges and
   the deblock walk can never disagree). KEY SEMANTIC: the binary arm must
   NOT adapt the frame CDF — C gathers onto the STACK and lets
   `aom_write_symbol` adapt that throwaway copy (decoder does the same), so
   the port uses `write_cdf` (encode, no update). (b) `c17fb1b53` closed a
   SILENT-CORRUPTION hole: mono `encode_frame` only asserted 8-alignment, so
   partial-SB dims reached the search with a CLAMPED root and emitted an
   undecodable stream (96x80 codes a 32-node NONE where the decoder expects
   a 64-node binary SPLIT-vs-VERT); some sizes coincide (16x16 codes no
   symbols at the two forced-split levels), which is what made it silent.
   Both landings are byte-neutral: matrix 54/54, entropy 119/119, encoder
   suite green.
   **REMAINING (the SEARCH restructure — the real work).** The port's search
   threads a CLAMPED extent (`cur_w = hw.min(width - x0)`, partition.rs:1319,
   and pipeline.rs passes the clamped SB extent as the ROOT) — i.e. it models
   a partial SB as a SMALLER BLOCK. That is the wrong model: AV1 keeps the
   node's TRUE size for partition decisions and guarantees coded LEAVES land
   fully inside via edge-driven splits. Required shape, per C
   `set_blocks_to_test` (enc_dec_process.c:1394-1438):
   - root stays 64x64; `has_rows/has_cols` (hbs = node_w/2 vs ALIGNED dims)
     drive the decision, NOT a clamped size;
   - both flags false -> `tot_shapes = 0` -> FORCED SPLIT (no NONE, no rect);
   - exactly one false -> `inj_hv_incomp` injects EXACTLY ONE shape (H at the
     bottom edge, V at the right edge) and **excludes PARTITION_NONE**; SPLIT
     is still evaluated;
   - NSQ disallowed at that size -> also forced SPLIT;
   - children whose ORIGIN is outside the aligned frame are SKIPPED entirely
     (search, tree, and pack);
   - an edge node must NEVER call `encode_with_neighbors` (it would predict/
     reconstruct outside the frame buffer) — only recurse or take the single
     legal rect, whose in-frame child is fully inside. Note 8x8 nodes can
     never be edge nodes while aligned dims are a multiple of 8 (hbs=4 keeps
     both flags true), so the `width <= MIN_BLOCK_SIZE` base case is safe.
   Then: cropped-RDO distortion (`frame_geom::cropped_tx_dims`, already
   written + dead), `pd0::compute_b64_variance` needs clamped reads before
   the `use_pd0` gate (pipeline.rs ~:3592) can be relaxed, and the two soft
   64-gates (`c_quant` at ~:451, `use_pd0`) plus the two 64-multiple asserts
   come off LAST. NOTE `frame_geom.rs` already contains `sb_geom`,
   `edge_has_rows_cols`, `cropped_tx_dims`, `pad_input_plane`,
   `mi_cols/mi_rows/sb_cols/sb_rows` — all correct, all DEAD except
   `FrameDims::new`; route through them rather than re-deriving inline.
   Filters need NO work: CDEF already clamps partial fbs explicitly, DLF/LR
   are mi/frame-driven (verified). Gate: 96x80 (exercises all three edge
   cases: right at SB(0,1), bottom at SB(1,0), both at SB(1,1)) then 65x65.
   **SCOPE-CHANGING FINDING 2026-07-18 (verified) — partial SBs take the WRONG
   partition PATH, not just wrong edge coding.** `use_pd0` (pipeline.rs:3653)
   requires `cur_w == sb_size && cur_h == sb_size`, so a partial SB (cur_w or
   cur_h < 64) DROPS the PD0/funnel path and falls to the homegrown recursive
   `partition_search_with_config` (pipeline.rs:4079) — a DIFFERENT, non-C-
   faithful search. C always uses its PD0-equivalent partition search with the
   sb_geom-clamped extent. So even with perfect edge coding (landed), a partial
   SB cannot byte-match C while it runs the homegrown search. The REAL chunk-2
   core is therefore: make the PD0/funnel path (`pd0::pd0_pick_sb_partition` +
   `decide_leaf`) handle a clamped sb_geom extent with edge-forced partition
   decisions + cropped-RDO distortion + C-faithful padded b64 variance — i.e.
   `use_pd0` must become true for partial SBs and that path must apply the
   5.11.4 edge rules in its DECISION (not only in the pack). There is NO byte-
   matching sub-chunk smaller than this whole path: the partition decision, the
   edge coding, the cropped distortion, and the padded variance must ALL align
   with C before a single partial-SB cell matches. Every byte-NEUTRAL piece is
   already landed (edge pack coding ebd770d1b, mono guard c17fb1b53, frame_geom
   wiring fc358cfdc, gate design aefbbfb5e, variance finding 99da1b318). This
   piece is a dedicated careful pass — high risk to the 54/54 gate if rushed —
   so it is the next #95 vertical, sequenced against continued #94 FFI-kernel
   verification (independently landable, also P0).
5. **#94 bd10 integration** (P0): u16 intake + harness axis, hbd module
   consumption, lambda *16/*4, filters at true depth per
   docs/bd10-port-map.md. Gate: uniform 64x64 bd10 <=M3 cell vs C.
6. **#91 SB128 integration**: 11-chunk order in docs/sb128-port-map.md
   (CDEF three-phase as its own reviewed chunk + unit test; flip the
   Globals/enc_handle.c:4071 gate LAST). Gate: the 12 real p0/p1 cells.
7. **Verification debt**: paeth tie-break FFI test (flagged, wired u8
   code — test BEFORE changing); PORT-NOTE(unverified) audit — every
   marker verified or carried consciously; qlookup spot-check vs C.
8. **#96 quality pass**: leaf_funnel module split (AFTER #71 lands),
   rustdoc invariants, dead-code sweep, tools/tests README.
9. **Broad gates**: #44/#73 full-matrix + real-content re-sweeps after
   each vertical; then determinism/--lp; **#93 perf LAST**
   (algorithmic/alloc before SIMD).

Rules of engagement per item: read the relevant docs/ map first; land in
byte-identical-current-output chunks; every landing = commit + rebase +
push + `git merge-base --is-ancestor` verify; batteries via sonnet
agents (foreground commands, no idle waits); record drill state in task
metadata BEFORE deep edits (compaction insurance).

## BULK-PORT MODE (user directive 2026-07-16)

Port ALL remaining C machinery with detailed, careful source-to-source
transcription FIRST; full per-cell verification batteries are REVISITED
after the code is in. Rules:

- Every spot whose bit-exactness is NOT yet verified against C carries a
  marker comment: `// PORT-NOTE(unverified): <what + C file:line + how to
  verify>`. Grep `PORT-NOTE(unverified)` = the complete debt list.
- When a piece BECOMES verified (FFI parity test, identity cell, or
  differential), delete the marker in the same commit as the evidence.
- The index below tracks AREAS with outstanding markers — update it when
  adding/clearing markers in a module.
- Development happens ON MASTER now (wave2/entropy-c-parity is merged and
  frozen; push origin HEAD:master). The HDR fork mode lands via PR #2
  (hdr-hybrid branch) — do not touch SVT_HDR_MODE code paths here.

### PORT-NOTE(unverified) index

- **Bulk-translation modules — WIRED 2026-07-17 (all in lib.rs; 798/798
  workspace tests + 36/36 matrix green post-wiring; bd10 qlookup tables
  generated and included). Remaining per-module work = the markers
  below (verification/integration, not translation):**
  - `crates/svtav1-encoder/src/frame_geom.rs` (#95 chunk 1): FrameDims
    true/aligned model, pad_input_plane, edge_has_rows_cols,
    cropped_tx_dims, DLF floor-chroma vs ceiling split, LR unit
    collapse, <64 restoration disable. Markers: edge predicates
    (96x80 milestone), pad byte-compare, odd-width chroma differential.
  - `crates/svtav1-encoder/src/sb128_geom.rs` (#91 chunk 1):
    NS_BLK_OFFSET(_128)_MD tables, partition_cdf_length, shape
    legality, sb128 variance-avg + me-cost-var-MAX bridges, CDEF
    stale-quadrant predicate, sb_header_params. Marker: CDEF phase-2
    fan-out + write_cdef quadrants must land WITH consumers + unit test.
  - `crates/svtav1-encoder/src/bd10.rs` (#94 chunk 1): clip_pixel_highbd,
    msb_truncate_plane, lambda *16/*4 consts, qzbin ladder, inv-txfm
    range, allintra_hbd_md. Markers: qlookup tables NOT transcribed
    (run xtask/transcribe_bd10_qlookup.py -> include the generated
    file, replace the unimplemented!() placeholders); lambda two-stage
    scaling needs a C dump check. **Cross-check finding (2026-07-17,
    from the sibling svtav1-dsp/hbd.rs translation below): the `else`
    arm of `qzbin_factor` returns 64, but the real C
    `svt_aom_get_qzbin_factor` (inv_transforms.c:3492-3505) returns 80
    there (`quant < th ? 84 : 80`, not `84 : 64`), and C also
    unconditionally special-cases `q == 0 -> 64` before consulting
    `quant` at all — this fn's signature has no `q` param so it cannot
    reproduce that case. Looks like a real bug; NOT fixed (this module
    is itself unwired/unverified) — fix when bd10.rs gets its
    verification pass.**
  - `crates/svtav1-dsp/src/hbd.rs` (#94 chunk 2, DSP-layer counterpart
    to bd10.rs): highbd intra predictors (DC/V/H/Paeth/smooth family,
    directional z1/z2/z3 with edge upsample, filter-intra, CfL 420 +
    predict), `highbd_clip_pixel_add`/`check_range` recon-add-clip,
    distortion (`full_distortion_kernel16_bits`, `highbd_variance`,
    `highbd_sad_kernel`, all generic W×H), deblock
    `lpf_{horizontal,vertical}_{4,6,8,14}_hbd`, `cdef_filter_block_hbd`
    (a pure `u16`-store variant of `cdef::cdef_filter_block` — zero new
    CDEF arithmetic, cited in-module), `dc_quant_qtx`/`ac_quant_qtx`
    switch-shape dispatch (bd10/bd12 table bodies still
    `unimplemented!()`, same as bd10.rs). Compile-checked clean (0
    warnings) via the temporary-`pub mod hbd;` dance; lib.rs left
    untouched. Markers: every fn (full FFI-parity + bd10 uniform-64
    verification pass, not run by this translation). **Two correctness
    findings surfaced while translating (documented with full detail in
    the module's doc comment, NOT fixed — out of this chunk's scope):**
    (1) the qzbin_factor cross-check above; (2) `intra_pred::
    predict_paeth_core`'s tie-break order (`p_top` checked first) does
    NOT match the real C `paeth_predictor_single`
    (intra_prediction.c:1226-1234, shared by C's lbd AND hbd paeth) —
    C checks `p_left` first. The two orders disagree exactly when
    `p_top == p_left` (both the minimum), which is a real, if
    infrequent, byte-exactness bug in the ALREADY-WIRED u8 intra
    predictor (not itself unwired/unverified code) — worth a priority
    look given this project's zero-conformance-regression mandate.
    `hbd.rs`'s own `predict_paeth_hbd` is translated directly from the
    real C order and does not reproduce the bug.
  - `crates/svtav1-encoder/src/intrabc.rs` (IntraBC/DV encoder vertical,
    allintra KEY screen-content path): `IbcCtrls::for_level` (full
    `set_intrabc_level` table, levels 0-7) + QP-mesh-scaling; DV validity
    (`is_dv_valid`, spec 5.11.35 — tile containment, sub-8x8 chroma
    margin, 256px wavefront delay, SB64/128 already-coded + SW-wavefront
    constraints) + `is_chroma_reference`; ref-DV composition
    (`resolve_dv_ref`/`find_ref_dv`); the FULL diamond+exhaustive-mesh
    pixel search stack (`init_search_sites`/`diamond_search_sad`/
    `refining_search_sad`/`exhaustive_mesh_search`/`full_pixel_diamond`/
    `intrabc_full_pixel_exhaustive`/`full_pixel_search`, all parameterized
    over caller-supplied `pic`/`stride`/`block_origin` — absolute-picture-
    coordinate addressing, not raw-pointer relative offsets, see the
    module's §4 header note); the hash-bucket SELECTION algorithm
    (`hash_search_best_in_bucket`, hash TABLE/CRC construction NOT
    translated — documented-only, a frame-wide precompute out of scope
    for a per-block pure-fn skeleton); DV rate-cost tables
    (`build_nmv_cost_table`/`mv_table_cost`/`mv_err_cost{,_light}`/
    `mv_bit_cost{,_light}`, reusing `svtav1_entropy::mv_coding`'s already-
    verified `NmvContext` — C seeds `ndvc` from the SAME `default_nmv_
    context` table as `nmvc`, so no separate DV-context transcription was
    needed); injection gating (`do_intra_bc_gate`/`eval_intrabc_after_
    palette`/`parent_gate_allows_intrabc`) + `IbcCandidate` builder;
    `write_intrabc_info` (thin wrapper over `svtav1_entropy::mv_coding::
    encode_mv_diff` with `MvSubpelPrecision::None`) + `INTRABC_DEFAULT_
    CDF`. RD integration (fast/full cost assembly, the recon-domain block-
    copy compensation path, tx-path reuse) is DOCUMENTED ONLY (prose
    section at the file's end, C file:line cited, not transcribed — out
    of scope per the task that produced this file). Compile-checked clean
    (0 warnings after one fix) via the temporary-`pub mod intrabc;` dance;
    lib.rs left untouched. 8 `PORT-NOTE(unverified)` markers: (1)
    `IbcCtrls::for_level`'s levels-6/7 unassigned-mesh-fields ambiguity
    (pooled-`PictureParentControlSet`-reuse question, §1); (2) `no_std`
    `exp()` not wired for `qp_based_th_scaling_factors` (§1); (3)
    `MvComponentCost::cost`'s clamp is 1 ULP narrower than C's literal
    `CLIP3(MV_LOW,MV_UPP,..)` bound — self-consistently safe, unverified
    that the 1-ULP gap is truly unreachable (§4); (4) `mvsad_err_cost`
    and 3 sibling `static`-in-C functions have no exported symbol,
    weakest evidence tier (§4); (5) the `window()` helper's debug_assert
    that every search position resolves to a non-negative absolute
    picture coordinate — relies on `direction_mv_limits`/`frame_mv_
    limits` being correctly tile/frame-bounded, unverified end-to-end
    (§4); (6) `exhaustive_mesh_search`'s tail-loop off-by-one, reproduced
    bug-for-bug from C (`end_col - c` not `+1`), unverified as truly
    unreachable on the real `mesh_patterns` grids (§4); (7)-(8) `default_
    cdfs.rs` migration note for `INTRABC_DEFAULT_CDF` + a `FrameContext`
    `intrabc_cdf`/`ndvc` field pair (§7). Upgrade path for the `static`-
    only functions: same `ref_shims.c` pattern as `palette.rs`'s six.
    Wiring TODO (documented in the module's top doc comment): add `pub
    mod intrabc;`, thread candidate injection into `mode_decision.rs`
    (mirroring `palette.rs`'s eventual wiring), flip `sc_detect.rs`'s
    hardcoded `allow_intrabc = false` to the real derivation, feed
    `write_intrabc_info` into the PACK block-mode-info writer ahead of
    the y-mode symbol.


- leaf_funnel.rs: fork complex-hvs MDS0 SSD fast cost (1 marker) — needs a
  C-side fast_loop_core dump once the C hybrid carries the fork's
  set_mds0_controls case 3 (the hybrid assert(0)s on mds0_level 3 today).
- leaf_funnel.rs: fork alt-ssim full_cost_ssim (1 marker) — the kernel is
  240-cell parity-tested (c_parity_ssim_md.rs); the marker covers the
  cost ASSEMBLY (whole-block vs C per-txb DIST_SSIM accumulation) — needs
  a C-side MD dump with alt_ssim_tuning=1.
- pipeline.rs: tune-SSIM rdmult per-SB lambda scaling (1 marker) — C
  scales per BLOCK (set_ssim_rdmult); the port applies the geometric-mean
  scale at SB granularity. Refine with a C-side per-block lambda dump.
- **`crates/svtav1-encoder/src/palette.rs`** (task #71, chunks 1-2 of
  docs/palette-port-map.md). FFI-verified against the real C (parity test
  `crates/svtav1-encoder/tests/c_parity_palette.rs`, all green, no markers
  needed): `count_colors`, `index_color_cache`, `k_means_dim1`,
  `calc_indices_dim1` (+ the internal `calc_centroids_dim1`/`lcg_rand16`
  reseed path, exercised via a dedicated forced-empty-cluster scenario).
  Carrying `PORT-NOTE(unverified)` markers (all reachable only through
  `static` C functions with no exported symbol — validated instead by
  hand-derived vectors traced against the C source, this project's
  WEAKEST evidence tier):
  - `delta_encode_steps` / `delta_encode_bits` — C `delta_encode_cost`,
    `static` in palette.c:80-109.
  - `remove_duplicates` — C `av1_remove_duplicates`, `static` in
    palette.c:66-78.
  - `optimize_palette_colors` — `static AOM_INLINE` in palette.c:250-270.
  - `extend_palette_color_map` — `static AOM_INLINE` in palette.c:275-294.
  - `palette_color_index_context` — C `av1_fast_palette_color_index_
    context` + `_on_edge`, both `static inline` in palette.c:612-743.
  - `color_map_wavefront` — its traversal loop is transcribed from inside
    the `static` `cost_and_tokenize_map` (palette.c:748-782).
  Upgrade path for all six: add a `ref_shims.c` wrapper exposing the
  otherwise-static C function (the sanctioned mechanism this crate already
  uses for other hard-to-reach C internals), or validate indirectly once
  chunk 3+ (`search_palette_luma`/RD/PACK) lands and can be differentially
  tested end-to-end.
- **`crates/svtav1-entropy/src/context.rs`** (task #71 chunk 5, PACK
  writers: `write_palette_mode_info`, `write_uniform`,
  `write_palette_map_tokens` + private helpers). Replaces the old
  `write_no_palette_flags`; its `None` arm is bit-for-bit that function's
  old behavior — re-verified after landing via `IM_PRESETS='2 6'
  tools/identity_matrix.sh` (24/24) and `tools/real_image_matrix.sh`
  (byte-identical), so the `None` path needs NO marker. Home for the
  delta-encode writer chosen here (entropy crate) since svtav1-entropy
  cannot depend on svtav1-encoder; the cache SPLIT
  (`svt_av1_index_color_cache`) stays in `svtav1-encoder::palette`
  (already FFI-verified there) and is threaded in pre-split from the
  pipeline.rs caller. Carrying `PORT-NOTE(unverified)` markers (the `Some`
  arm — no `BlockDecision` carries a palette winner yet, so none of this
  runs end-to-end; covered only by same-crate unit tests):
  - `write_palette_mode_info`'s `Some` arm (size symbol + colors) — smoke
    test `write_palette_mode_info_some_vs_none_arm` only.
  - `write_palette_colors_y`, `write_delta_encoded_colors` — C
    `write_palette_colors_y` / `delta_encode_palette_colors`
    (entropy_coding.c:4256-4341); self-consistency unit tests only.
  - `palette_map_pixel_ctx` — DUPLICATE of `svtav1_encoder::palette::
    palette_color_index_context` (re-transcribed, cross-crate dependency
    direction); cross-checked against the SAME hand-derived vectors as
    that fn's own tests (`palette_map_pixel_ctx_*_hand_vectors`), the
    same weakest-evidence tier as its palette.rs twin.
  - `write_palette_map_tokens` — two gaps: (1) the `Some`-arm-only
    reachability above, and (2) the within-bounds `rows`/`cols` clip is
    untested (no edge-clipped blocks on this port's 64-aligned frames).
  Upgrade path: EPICA/identity cells once #71 chunk 3/4 (`search_palette_
  luma`/RD integration) wires a winning candidate into `BlockDecision`.
- **`crates/svtav1-encoder/src/pipeline.rs`** — `EntropyCtx` gained
  `above_palette`/`left_palette`/`above_palette_colors`/
  `left_palette_colors` + `record_palette`/`palette_neighbor_ctx` methods
  and the free fn `palette_cache` (C `svt_get_palette_cache_y`,
  palette.c:164-210). `record_palette`'s `None`-colors path and
  `palette_neighbor_ctx` run on EVERY block today (re-verified via the
  same identity/real-image gates above) but always see an all-zero
  grid — carrying a `PORT-NOTE(unverified)` marker: `palette_cache`'s
  above/left MERGE loop (the SB-row-drop + sorted-merge-with-dedup logic)
  is only exercised on the trivial empty-cache early return until a
  palette winner exists on an adjacent block.
- **Tile rows (task #86 phase 1, allintra KEY path)** —
  `crates/svtav1-entropy/src/obu.rs` (`resolve_tile_rows_log2`,
  `tile_row_log2_limits`, `tile_log2_blk`, `write_tile_info`,
  `tile_size_bytes_minus_1_for`, `build_tile_group_multi`) +
  `crates/svtav1-encoder/src/pipeline.rs` (`EncodePipeline::tile_rows_log2`
  / `with_tile_rows_log2`, the per-tile loop inside `run_entropy_walk`) +
  `crates/svtav1-encoder/src/partition.rs`
  (`PartitionSearchConfig::tile_top_px`, `extract_neighbors_tiled`).
  Tile COLUMNS are out of scope (always 1 column); default
  `tile_rows_log2 = 0` is byte-identical to pre-#86 (36/36
  `identity_matrix.sh`, unaffected).
  - **FH/tile_info bits: VERIFIED byte-identical to real aomenc/SVT-AV1 C**
    at `tile_rows_log2 = 1` (2 tile rows), both the 128x128 (hits
    `maxLog2TileRows` exactly — no trailing stop bit) and 512x512 (doesn't
    hit the max — has one) shapes, via `tools/identity_diff.py`'s
    field-level FH walk: "FRAME field walk... all decoded fields
    identical" including the `increment_tile_rows_log2`/
    `context_update_tile_id`/`tile_size_bytes_minus_1` trailer. Verified
    with unit tests too (`tile_info_128x128_two_tile_rows`,
    `tile_info_512x512_two_tile_rows`, `resolve_tile_rows_log2_clamps_
    like_c`, `tile_size_bytes_minus_1_for_thresholds`,
    `build_tile_group_multi_variable_width_prefix`).
  - **Real bug found+fixed while landing this (pre-existing, unreachable
    before #86 since `tile_rows` was always 1)**: `encode_tile_rows`'s
    `chain_snaps` per-SB CDF-chain accumulator (pipeline.rs, the
    `funnel_chain`/M4-M6 rate-estimate path) was indexed by the ABSOLUTE
    frame-wide `sb_index`, but is a vector that starts EMPTY at every
    `tile_idx` — tile_idx >= 1 panicked with "index out of bounds"
    (`chain_snaps[sb_index - 1]` when `chain_snaps.len() == 0`). Fixed by
    indexing with a TILE-LOCAL `local_sb_index = (sb_row -
    tile_sb_row_start) * sb_cols + sb_col` and gating `topright_avail` on
    `sb_row > tile_sb_row_start` (was `sb_row > 0`) instead.
  - **Real bug found+fixed (intra-prediction availability at a tile's own
    top row)**: `partition.rs::extract_neighbors` computed `has_above =
    abs_y > 0` (frame-absolute) instead of tile-relative — a block at a
    TILE's own top row (not the frame's) would read stale/default-128
    pixel data across the tile boundary as if it were a real "above"
    neighbor, instead of falling back to the C decoder's real
    unavailable-neighbor rule (`left_ref[0]` else 127) — a genuine
    bitstream-CORRECTNESS bug (encoder/decoder CDF-context and
    prediction disagreement), not merely a byte-identity nicety, for any
    block whose tile-relative row is 0 but frame-absolute row is not.
    Fixed via a new `tile_top: usize` param
    (`extract_neighbors_tiled` — the original `extract_neighbors(...)`
    6-arg signature is kept AS A THIN WRAPPER at `tile_top=0` because
    `leaf_funnel.rs` — a separate, off-limits workstream file, task #86
    scope — calls it directly) threaded via
    `PartitionSearchConfig::tile_top_px` (MD search,
    `encode_with_neighbors`) and `EntropyCtx::tile_top_px` (pack side,
    `encode_chroma_block_dc`'s `tile_top` param — chroma-plane units,
    `ectx.tile_top_px / 2`; also fixed `tx_size_ctx`'s equivalent
    `y > 0` check, empirically inert for that ONE call site — a
    freshly-reset array reads 0 either way — but wrong by the same
    reasoning). Verified via 3 new targeted unit tests
    (`extract_neighbors_tiled_top_row_has_no_above`,
    `_falls_back_to_left`, `_interior_row_has_above`) proving the exact
    before/after behavior at a tile boundary.
  - **PORT-NOTE(unverified) — `leaf_funnel.rs` has the SAME
    intra-availability gap, unfixed (off-limits file).** Its own
    `extract_neighbors` call (leaf_funnel.rs:978) uses the untiled
    wrapper (`tile_top=0` always). Because `use_funnel` (pipeline.rs) is
    true whenever `chroma_420 && chroma_src.is_some() &&
    ref_frame_data.is_none() && c_quant.is_some()` — which is EVERY
    still/4:2:0/KEY/64-aligned encode, i.e. every config the identity
    harness can produce — 100% of the task #86 acceptance cells route
    through `leaf_funnel.rs`, so the `PartitionSearchConfig::tile_top_px`
    fix above is real and unit-tested but DORMANT for those specific
    cells; the dominant residual divergence they show is this gap (plus
    the LR one below), not the fixed one. Marker at
    `partition.rs::extract_neighbors`'s doc comment. Verify by adding
    `tile_top` threading inside `leaf_funnel.rs` (coordinate with its
    owning workstream) and re-running the task #86 identity cells.
  - **PORT-NOTE(unverified) — loop-restoration RU grid is frame-wide, not
    per-tile.** Marker at the `search_restoration_still` call site in
    `pipeline.rs` (~line 1114, `Step 6a''`): the RU grid
    (`svtav1_dsp::restoration::count_units_in_tile`/
    `foreach_rest_unit_in_tile`, genuinely tile-parameterized in C per
    restoration.c) is invoked ONCE post-tile-merge over the WHOLE frame,
    matching C only at NumTiles==1. Empirically the dominant divergence
    class in the task #86 acceptance cells that reach far enough to
    signal wiener (both preset-6 cells: gradient 128x128 q32 and
    1147124.png 512x512, both classified "op-class: lr-taps
    (wiener_restore + literal run)" with FH byte-identical and hundreds
    of tile-payload bytes matching before the LR-tap divergence).
  - **Acceptance-gate results (task #86 phase 1, all at `tile_rows_log2 =
    1` / `SVT_TILE_ROWS = 1` on the C side — see the units note below):**
    `identity_matrix.sh` 36/36 (regression, log2=0 default, unaffected).
    `gradient 128x128 q32 p6`: FH byte-identical (77 bits/34 fields);
    tile payload DIFFERS at byte +958, "op-class: lr-taps" (the LR gap
    above). `1147124.png 512x512 q20 p2`: diverges EARLIER, at a
    `loop_filter_level[0]` VALUE mismatch (C=8, Rust=9) — a genuine
    per-tile-independent-MD-search RD cascade (this preset does its own
    full deblock-level SSE search over the recon, which differs once
    upstream per-tile decisions differ — same root-cause CLASS as the
    intra-availability bug, but inside `leaf_funnel.rs`, not proven to be
    the exact same instance; not further isolated this session).
    `1147124.png 512x512 q20 p6`: FH byte-identical; tile payload DIFFERS
    at the LR layer (same "lr-taps" class as the 128x128 p6 cell).
  - **Units note**: the C driver's `SVT_TILE_ROWS` env var (task #86
    harness spec) is a DIRECT passthrough to `cfg.tile_rows`, which the
    public API documents as the LOG2 value (`EbSvtAv1Enc.h:607-611`,
    "0 means no tiling, 1 means split into 2") — i.e. `SVT_TILE_ROWS=1`
    means the SAME "2 tile rows" as `SVTAV1_TILE_ROWS_LOG2=1`, NOT
    `SVT_TILE_ROWS=2`. Empirically confirmed:
    `SVT_TILE_ROWS=2` on a 512x512 cell (8 SB rows, so no coincidental
    clamp) makes the C side pick TileRowsLog2=2 (4 actual tile rows)
    while Rust stays at log2=1 (2 tile rows) — the differ catches it
    precisely at the FH bit level (`STAGE: FH |
    increment_tile_rows_log2[1] C=1 Rust=0`). All acceptance-cell numbers
    above use the MATCHED config (`SVT_TILE_ROWS=1`).
  - **Not yet ported (tile columns, non-uniform tile spacing, the C
    "fewer actual tiles than `1 << TileRowsLog2` requested" edge case
    when a request exceeds SB-row count at a non-power-of-2 split)** —
    out of task #86's scope; `resolve_tile_rows_log2`'s min/max clamp IS
    fully ported (mirrors `svt_av1_get_tile_limits` +
    `svt_aom_set_tile_info`'s row half), only the ACTUAL SB-row split
    (`rows_per_tile = sb_rows.div_ceil(tile_rows)`, pre-existing in
    `encode_tile_rows`) keeps the simpler "exactly `1 << log2` tiles,
    some possibly empty" shape instead of C's early-stopping loop —
    unreachable by any in-scope test cell (both use clean, power-of-2
    splits).

## Zen codec cross-cutting compliance (encode backend) — SPEC (2026-07-20)

This crate (`svtav1` lib / pkg `zenav1-svt`, encoder `zenav1-svt-encoder`) is an
**encode backend** consumed by zenavif (feature `encode-svt-rs`,
`src/encoder_svt_rs.rs` → `EncodePipeline::encode_frame_420`). Its input is
**trusted in-memory pixels the caller already holds**, not an adversarial
bitstream — so the resource-limit / panic-on-untrusted-input bar is genuinely
LOWER than a decoder's. But three gaps are NOT excused by trusted input and are
the priority here: (a) entry points that **panic** on a caller contract
violation instead of returning `Err`; (b) **silently emitting a corrupt
bitstream** for un-gated configs (the seam can't tell garbage from valid); (c)
**no cancellation hook** for a long encode. Reference codec is zenavif; contract
types live in `zencodec` 0.1.26, `whereat`, `enough`/`almost-enough`.

**Design rule: stay codec-only.** Do NOT take a hard `zencodec` dependency. The
integration crate (zenavif) owns the `CategorizedError`/limits/estimate trait
impls (zenavif `main` already implements `zencodec::CategorizedError for Error`).
This crate's job: return a **structured `Result`** (never panic on a caller
mistake), carry enough error granularity for the seam to categorize, accept a
**stop token** and a **fallible-alloc setting**, and **never return a non-empty
`Vec<u8>` that isn't a valid bitstream**.

### 1. Limits enforcement — LOW priority (trusted input)
- **State:** only zero/alignment/buffer-size validation
  (`avif.rs:451`, `encoder_svt_rs.rs:164`). No max-dimension / max-memory cap;
  `EncodePipeline::new` accepts arbitrary `u32×u32` and `encode_y8:329` does
  `vec![128u8; padded_w*padded_h]` unbounded.
- **Bar (relaxed):** because dims come from a caller-held buffer, an over-large
  request is a caller bug, not an attack. Still, replace the *abort* with a
  typed `Err`: add a sanity ceiling (e.g. reject `width*height` beyond a
  configurable `max_pixels`, default generous ~1 Gpx) returning
  `EncodeError::InvalidDimensions`/a new `TooLarge` variant, so a bad dimension
  is a graceful error not an OOM-abort. This is §5's "no panic on caller
  contract violation", not decoder-grade DoS hardening.

### 2. Resource estimation — LOW priority
- **State:** none.
- **Bar:** a lightweight `estimate_encode(width, height, preset, qp) ->
  EncodeEstimate { peak_memory_bytes, time_ms, output_bytes }` keyed on pixels
  × preset. zenavif has its own calibrated `heuristics::estimate_encode`, so a
  minimal honest peak-memory bound (buffers are `~padded_w*padded_h*k`) is
  enough to let a caller pre-flight. Optional until §3/§5/§6 land.

### 3. Structured `Result` errors + whereat — MEDIUM (a real gap)
- **State:** `EncodeError` exists (`avif.rs:44`) with 3 variants
  (`InvalidDimensions`, `InvalidQuality`, `EncodeFailed(String)` — the last
  **never constructed**). Critically, **the pipeline returns bare `Vec<u8>`**:
  `encode_frame`/`encode_frame_420`/`encode_frame_impl` (`pipeline.rs:347,380,
  423`) cannot report a runtime failure — they **panic** instead (§5). So
  `avif.rs::encode_y8` returns `Ok(...)` unconditionally and the zenavif seam's
  `is_empty()` check (`encoder_svt_rs.rs:345`) can never fire. No `whereat`.
- **Bar:** `EncodePipeline::encode_frame*` must return
  `Result<Vec<u8>, EncodeError>` so a runtime failure (unsupported partition
  shape, un-gated config, worker panic caught) surfaces as `Err`, not a
  process panic and not silent garbage. `EncodeError` gains the granularity in
  §4. Behind a default-off `whereat` feature, `define_at_crate_info!()` +
  `At<EncodeError>`; without it, the bare enum still carries category+message.

### 4. Category granularity (feeds zencodec `CategorizedError`) — MEDIUM
- **State:** 3 flat variants, no categorization, no zencodec.
- **Bar:** `EncodeError` variants that let the zenavif seam map to the right
  `zencodec::ErrorCategory` arm instead of a blanket `Error::Encode` →
  `Internal`. Suggested set:
  - `InvalidDimensions` / `InvalidQuality` / `TooLarge {..}` → caller-request
    faults → zenavif maps to **Request::Invalid(Parameters/Buffer)** /
    **Resource::Limits**.
  - `UnsupportedConfig(what)` (a preset/qp/dims combination the port can't yet
    encode correctly — see §5 corruption) → **Request::Unsupported** /
    zenavif `Error::Unsupported`. This is the honest home for "un-gated config"
    (below) instead of emitting garbage.
  - `Internal(reason)` (a genuine port bug: unexpected partition shape, worker
    panic) → **Internal::Bug**.
  - `Cancelled(StopReason)` (§6) → **Stopped**.

### 5. Panic-freedom, no-silent-corruption, configurable fallible alloc — HIGH
- **Bar (no panic on a caller mistake):** the encode entry points currently
  `assert!` on contract violations — `encode_frame:348` (dims 8-aligned),
  `encode_frame_420:381` (chroma_420 enabled), `:402` (plane lengths) — plus
  `panic!` at `pipeline.rs:3916,4297,4571,4877` and worker `join().unwrap()`
  at `:6119`. Trusted input does NOT excuse crashing the caller's process on a
  legal-but-unaligned request or an internal partition case: convert these to
  `Err(EncodeError::…)` at the entry-point boundary (validate → `Err`), and
  catch worker panics into `EncodeError::Internal` rather than propagating.
- **Bar (NO SILENT CORRUPTION — the load-bearing one):** per STATUS.md, many
  preset/qp/dimension combinations outside the gated matrix produce
  **decodable-but-wrong or non-decodable** bitstreams (STATUS.md:68), returned
  as a **non-empty `Vec<u8>` of garbage** that the zenavif `is_empty()` seam
  happily muxes. This violates the global "ZERO TOLERANCE for image corruption"
  rule at the integration boundary. Required: the encoder must **know its own
  verified envelope** and, for a config outside it, return
  `EncodeError::UnsupportedConfig` (§4) — refuse, don't emit garbage. A
  wrong-pixels output that is indistinguishable from a correct one at the seam
  is a shipping bug, not a "known limitation". (This is the encoder analogue of
  the decoder's panic-freedom: the failure mode differs — corruption vs panic —
  but both must become a typed `Err`.)
- **Bar (fallible alloc is a SETTING — per user directive):** the
  fallible-vs-infallible allocation trade must be a **configurable knob**, not
  hardcoded. The pipeline allocates large buffers infallibly off dims
  (`pipeline.rs:516,1009,4403,4646,5227,5255,6081` — `vec![v; w*h]`, abort on
  OOM). Add an `AllocMode { Fallible, Infallible }` on the pipeline/encoder
  config; route the dim-sized buffers through a helper honoring it
  (`try_reserve_exact` → `EncodeError::AllocFailed` on the fallible path). For
  a *trusted-input encoder* the sensible default is **Infallible** (single
  `calloc`, faster — the caller controls the size), with Fallible available for
  memory-bounded/server contexts. This is the inverse default from the
  untrusted decoder, and that asymmetry is the point of making it a setting.
- **Bar (fuzz):** add a `fuzz/` target over `AvifEncoder::encode_y8`/`_yuv420`
  with arbitrary dims/quality/preset + small random pixels; any panic/abort is
  a bug. None exist today. This mechanically enforces the "no panic on caller
  mistake" bar.
- **Keep:** `#![forbid(unsafe_code)]` (already workspace-forbidden).

### 6. Stop-token cancellation — HIGH (a long encode is uninterruptible)
- **State:** the pipeline takes no token; `encode_frame_420` runs to completion
  (`pipeline.rs:380`). zenavif checks `stop` only at coarse phase boundaries
  (`encoder_svt_rs.rs:275,316`) and the single `encode_frame_420` call at
  `:344` is uninterruptible — a slow preset-0 encode blocks past the last check.
- **Bar:** thread an `&impl enough::Stop` (default `enough::Unstoppable`) into
  `encode_frame`/`encode_frame_420` and poll `stop.check()?` inside the
  superblock loop / at tile-worker boundaries (`encode_frame_impl` body around
  `pipeline.rs:5227-6119`), returning `EncodeError::Cancelled(StopReason)`.
  The module doc already flags the missing hook
  (`encoder_svt_rs.rs:275`: "no per-superblock stop hook yet") — this is where
  it lands. Cadence: at least once per superblock-row so cancellation is
  observed within bounded work; `enough` is zero-dep `no_std`.

### Priority order for this backend
1. §5 no-silent-corruption + no-panic-on-caller-mistake (make entry points
   return `Err` for out-of-envelope configs and contract violations) — the
   corruption-at-the-seam risk is the top issue and is NOT excused by trusted
   input.
2. §3 pipeline returns `Result` + §4 category-bearing `EncodeError`.
3. §6 stop hook in the superblock loop.
4. §5 configurable `AllocMode` (default Infallible for this trusted encoder),
   §1 sanity ceiling, §2 estimate.

When any item lands, update the zenavif seam (`src/encoder_svt_rs.rs`) in the
SAME change to consume it (thread the token, map the new error variants, drop
the `is_empty()`-as-failure heuristic once `Result` is real).
