# Session handoff: perf work state, 2026-09-19

Snapshot (dated file = point-in-time; re-derive before trusting numbers).
Written at the end of a long perf session so a fresh session can resume
without re-measuring everything.

## Where the work stands

Mission: make the safe-Rust SVT-AV1 v4.2.0 port faster, targeting ~5% faster
than C, byte-identical everywhere it claims identity.

### Landed this session (all pushed to main)

| commit | what | measured |
|---|---|---|
| `8ce1a8c3` | subpel_variance const-width specialization + convolve_2d_sr v3 | inter cell 1,952M→1,720.5M Ir cumulative |
| `abc5bcb2` | filter_intra_edge v3 (replicate-padded tap window) | 38.1M→5.3M on that path; cell 1,688.1M |
| `4b0bfa1ab` | write_coeffs_txb_1d: eob==1 fast path + cached level/sign + hoisted CDF rows | writer subtree ~28.7M→24.9M (q32) / 47.0M→44.5M (q15); total −0.1..−0.2% |

Earlier in the campaign (same effort, older commits): residual_i16 v3,
convolve_x_sr/y_sr v3 (shifted-tap-load formulation), warp_affine v3,
jnt_convolve reverted (autovectorized already), lazy conv_buf allocation,
dr_predictor z1/z2/z3 rewrite, skip-arm double-clone fix.

### Correctness state

- nextest workspace: 3928/3928
- regression_spotcheck: 145/145
- identity_full_8bit (synthetic+dims): 1100/1100 byte-identical
- real_video_inter_gate: 24/24 byte-identical
- bd10_video_gate: 24/24
- All changes byte-neutral vs HEAD on the measured cells.

### Perf position (measured, this host `lilith` WSL, taskset -c 3)

- Still (1014 imazen 512×512 q32 p6): port ~44ms vs C ~36ms → **~1.22×**
- Inter (synthetic translate 4f): ~1.38×, was ~2.3× before the campaign
- Video BD-rate (freshly measured, see below): **~parity** (−0.23% johnny p6,
  +0.01% fourpeople p8 [byte-identical], +1.21% vidyo4 p8 worst cell)

## KEY MEASUREMENT LEARNINGS — read before profiling

1. **C's callgrind numbers are inflated vs hardware.** On the 1014 cell,
   callgrind said C=672M Ir but `perf stat` says C=480M instructions. Port:
   670M callgrind ≈ 669M hardware. So earlier "port ≈ C on instructions"
   callgrind comparisons were wrong — port actually executes ~39% MORE
   instructions than C on hardware, at HIGHER IPC (3.32 vs 2.91). Suspect
   valgrind makes the C binary take longer paths somewhere. **Trust
   `taskset -c 3 perf stat` for port-vs-C instruction deltas; callgrind is
   still fine for WITHIN-binary attribution (which functions are hot).**

2. `taskset` must wrap valgrind, not sit inside it — otherwise empty
   callgrind files. Correct:
   `taskset -c 3 valgrind --tool=callgrind --callgrind-out-file=X prog ...`

3. Paired A/B on the SAME cell: `git stash` → rebuild → measure →
   `git stash pop` → rebuild. perf_encode builds in ~29s. Wall-clock
   comparisons need interleaved runs and identical input .yuv.

4. `perf_c_encode` reports ENCODE_NS excluding init (like perf_encode).
   `capture_c_trace` writes the C .obu for byte-compare AND works under
   valgrind for callgrind traces (but see #1 — treat C callgrind totals
   with suspicion).

5. `perf record` needs warmup=N (e.g. 20) on these ~30ms encodes to get
   usable sample counts.

6. Symbol attribution trap: C's writer showed 2,164 calls while the port
   showed 12,580 — but C inlines most calls; `svt_av1_txb_init_levels_avx2`
   call count (11,976) proved both sides do the same txb count. Never infer
   C call counts from one symbol; find a callee C can't inline.

7. LLVM already elides most slice bounds checks in hot loops —
   const-generic plumbing to "help" elide them measured neutral (reverted).

## Current profile shape (1014 still cell, port, callgrind Ir)

- `optimize_b::{closure#0}` 111.7M self / 122M incl, 28,047 calls (~4.3K/call)
  — the RDOQ trellis; helpers (update_coeff_simple/general/eob,
  coeff_cost_general, lower_levels_ctx_tc, br_ctx_tc, dqv_qm, rdcost) are all
  tight C-faithful translations already const-generic + inline. Excess vs C is
  diffuse ~20-30% codegen overhead, no single fixable site found.
- `tx_unit_inner` 56M self / 381M incl — driver; already grown-buffer
  optimized, eob==0 recon copy (9.7M memcpy) is C-parity work.
- `cost_coeffs_txb` 49.9M — already has eob≤1 levels_skip_init and eob==1
  shortcut; C runs get_nz_map_contexts unconditionally too (rd_cost.c:412).
- `write_coeffs_txb_1d` ~22M — just optimized to C parity structure.
- `residual_i16` 19.3M — v3-specialized by width (u8x16 packing).
- scalar `get_nz_map_contexts` ~13M over 39K calls — C-parity cadence.
- memcpy 14.5M + memset 12.7M — diffuse; eob==0 pred→recon row copy
  dominates and is required semantics.
- `build_coeff_cost_tables_from_fc` 7.4M, 5,330 calls — per-SB rebuild,
  matches C's per-SB `av1_estimate_coefficients_rate` cadence.

## Remaining targets (ranked by est. win)

1. `optimize_b` trellis (~122M) — biggest single item. ~20-30% per-call vs C.
   Ideas not yet tried: reduce arg-count pressure in update_coeff_* (pack
   into a ctx struct passed by ref?), check whether `qc_dqc_low`/`rdcost`
   (i64 math) can narrow, see if `nz_map_ctx_tc`'s per-tap reads vectorize.
   Deep sequential DP — expect grind, not a kernel rewrite.
2. `tx_unit_inner` self-cost 56M — orchestration overhead spread across
   residual/transform/quant/inv/satd plumbing.
3. `cost_coeffs_txb` 50M — same diffuse-overhead story as the trellis.
4. `eval_candidate` 184K small memcpy calls (~13 Ir each) — struct moves,
   mostly unavoidable without layout changes.
5. Anything claiming big single-function wins has been picked — what remains
   is many 1-5M items.

## Video BD-rate recipe (just used; reproduce/widen as needed)

```
ASSETS=/home/lilith/work/zen/video/pd-derf-720p
# port (dumps own recon via SVTAV1_FINAL_RECON):
env -u SVTAV1_FRAME_SHIFT SVTAV1_FRAMES=8 SVTAV1_INTRA_PERIOD=64 \
  SVTAV1_HIER_LEVELS=0 SVTAV1_FINAL_RECON=$out/rs_rec \
  ./target/release/examples/identity_run rawseq:$ASSETS/${clip}_256x256_8f.i420 \
  256 256 $qp 6 $out/rs_q$qp
# C + decode:
SVT_FRAMES=8 SVT_INTRA_PERIOD=64 SVT_HIER_LEVELS=0 \
  ./tools/capture_c_trace/capture_c_trace.bin 256 256 $qp 6 \
  $ASSETS/${clip}_256x256_8f.i420 $out/c_q$qp.obu 8
aomdec --rawvideo -o $out/c_q${qp}_rec.yuv $out/c_q$qp.obu
# then per-frame PSNR-Y vs source .i420, Bjøntegaard-integrate log(bits) vs PSNR.
```
Clips: fourpeople kristenandsara johnny vidyo1 vidyo3 vidyo4 at 128/256,
8 frames each. NOTE: port recon dumps are per-frame `rs_rec.fN`; C recon is
one concatenated .yuv. `rs_q*.obu.fN` files give per-frame sizes.

## Harness cheat sheet

```
# port perf/identity harness (builds in rust/):
cargo build --release --example perf_encode   # ~29s
./target/release/examples/perf_encode raw:<yuv> W H QP PRESET out [warmup]
  # multi-frame: SVTAV1_FRAMES=N SVTAV1_FRAME_SHIFT=k SVTAV1_INTRA_PERIOD=-1
  # SVTAV1_HIER_LEVELS=0 ; writes out.obu + out.obu.fN + out.yuv
./target/release/examples/identity_run rawseq:<clip.i420> W H QP PRESET out
  # same env vars + SVTAV1_FINAL_RECON=<prefix> dumps per-frame recon
# C side:
./tools/capture_c_trace/capture_c_trace.bin W H QP PRESET in.yuv out.obu [bd]
  # env: SVT_FRAMES=N SVT_INTRA_PERIOD SVT_HIER_LEVELS SVT_PRED_STRUCT
./tools/perf_c_encode/perf_c_encode W H QP PRESET in.yuv out.obu [warmup]
```

## Repo mechanics (the stuff that bites)

- `.workongoing` at repo root: check/write BEFORE any work, refresh every
  ~2min during, delete when done. `jj` is the git interface: commit normally
  (git commit works, lands as child of @), then
  `jj bookmark set main -r <sha> --allow-backwards && jj git push --bookmark main`;
  verify `git merge-base --is-ancestor <sha> origin/main`.
- Heavy jobs serialize under `run-heavy` + always `nice -n 19`:
  `TMPDIR="$HOME/tmp" ~/work/zen/scripts/run-heavy --mem 12G -- <cmd>`
  (12G on this 23GiB WSL box).
- `#![forbid(unsafe_code)]`; SIMD only via archmage/magetypes 0.9.29,
  `#[arcane]`/`#[rite]`/`incant!`, Desktop64 tier. No `#[inline(always)]` on
  arcane/rite. No `#[inline(always)]` needed elsewhere unless measured.
- `reference/svt-av1` is read-only (temp instrumentation must be reverted).
- Dated docs + `benchmarks/*.meta` are snapshots, not truth. Gate output is
  the ledger.
- `cargo nextest run --workspace --locked` (process isolation matters:
  SIMD token disabling is process-global).
- Disk was ~93% full at one point — clean old /tmp/cg_*.out and /tmp/*.yuv.
- Long-running probes earlier left useful scratch: /tmp/bdr/ has the BD-rate
  outputs; callgrind traces at /tmp/cg_wopt2.out (post-writer port),
  /tmp/cg_base1014.out (baseline port), /tmp/cg_c1014.out (C, inflated).

## Things tried that did NOT work / were reverted

- Vectorized jnt_convolve_2d_copy — LLVM already autovectorized; v3 was
  slower (30.8M vs 21.3M). Reverted.
- Const-generic `write_symbol`/CDF plumbing to elide bounds checks — LLVM
  already elided; identical Ir. Reverted.
- Per-pixel madd formulation of convolve_x_sr — reduce_add per pixel ≈ wash;
  the winning formulation batches 8 outputs with one shifted load per tap.
- Believing C callgrind totals for port-vs-C deltas (see above).
- External process timing for port-vs-C — C's startup dominates; use
  ENCODE_NS internals.

## Open threads

- `optimize_b`/`cost_coeffs_txb`/`tx_unit_inner` grind (above).
- Pre-existing divergence: johnny-class 8-frame P content isn't byte-
  identical to C (RD-neutral, ~±1%); e.g. 1014 4f translate cell diverges at
  HEAD too — NOT a regression, verified byte-neutral vs HEAD.
- The `packed`/`dq_full` re-lay only exists for 64-dim txs — already minimal.
- Wall-clock still gap ~1.22× (instructions +39%, IPC higher) — closing it
  means instruction reduction in the trellis/rate/tx-driver block, which is
  ~68% of the encode but written at C-parity structure.
