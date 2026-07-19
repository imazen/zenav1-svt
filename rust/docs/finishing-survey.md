# Finishing survey — ranked fix list toward full still-frame byte-identity

Read-only source-to-source survey, 2026-07-19. C reference `/root/svtav1/Source/`
(READ-ONLY), Rust port `/root/svtav1/rust/crates/`. Scope of this pass: **arbitrarily-
sized still frames** first, then a stub/hardcode/stale-marker sweep, then the named
open residuals. Nothing was built or edited to produce this (a concurrent agent holds
the build slot); every claim is a source read with a `file:line`, and agent-reported
claims were spot-verified (one was a false positive — see §D3).

---

## TL;DR

**Is "arbitrarily-sized still frame" one fix away or many?**

Split the question by envelope, because the answer is very different:

- **bd8 · 4:2:0 · presets 6–13 · SB64** — arbitrary dims (incl. odd + partial SB) is
  **essentially DONE**: `partial_sb_gate.sh` is 101/101. Two tiny residuals remain
  (a recon-invisible corner coeff near-tie at p6, a high-qp straddle near-tie at p7/p8),
  and both are **genuine MD near-ties, NOT shotgun fixes** (§C1, §C2).
- **The FULL still envelope** (all presets 0–13, bd8+bd10, SB64+SB128, per
  `docs/ACCEPTANCE-CRITERIA.md`) is **MANY fixes**. Three multi-pass ports gate it, each
  of which the arbitrary-size axis crosses:
  1. **presets 0–5 partial-SB** — currently **PANICS** at preset 0 (the 65×65 case) and
     is byte-unported even where it doesn't panic (§A1). This is the top item.
  2. **bd10 partial-SB** — silently **falls back to the u8 output** (§A3); no partial-SB
     bd10 cell is byte-exact.
  3. **SB128** — the encode path **LANDED 2026-07-19** (12/14 cells, §A4); this is a
     correction to the task/STATUS premise. Only 2 leaf-cost near-ties + inter remain.

So: *for the common preset band it's done; for the full envelope the arbitrary-size axis
is blocked behind the same three multi-pass ports the rest of the project is.*

**The 65×65 panic root (§A1):** a **partial** SB at preset 0–5 has `refined = false`
(that flag requires `full_sb`, pipeline.rs:5714), so it drops to the M6 fixed-tree
fallback **but carrying `FunnelCfg::for_preset(0..=5)`**, whose M0–M2 CfL is *always-on*
(`cplx_th == 0` bypasses the chroma-complexity detector, leaf_funnel.rs:525). A
straddling edge leaf (e.g. a 32×32 SPLIT child at the right edge of an aligned-72 frame,
x∈[64,96)) then arms CfL and drives a chroma prediction/TX whose coords exceed the
aligned chroma stride (`cwid = w/2`, pipeline.rs:5086) → out-of-bounds access. The
preset-6–13 partial path forces the edge *shape* (which is never CfL-eligible) and never
arms always-on CfL, which is why the 101/101 gate is green while preset 0 crashes. The
cited `leaf_funnel.rs:5417` is the **bd10** CfL-TX arm; the **bd8** twin is the u8 CfL
access ~40 lines up (leaf_funnel.rs:5090–5388) — same root, different line, which is why
"both bd8+bd10" panic.

**Top shotgun-fixable items** (safe, mechanical, high value — details below):

| # | Item | §|
|---|---|---|
| 1 | Make the 65×65 (preset 0–5 partial-SB) path **panic-free** via a runtime guard/CfL-on-edge suppression (byte-identity is separate, multi-pass) | A1 |
| 2 | Wire `frame_geom::cropped_tx_dims` into the funnel distortion (candidate close for the high-qp straddle residual; also the one written-but-DEAD helper that maps to a real C behaviour) | A2 |
| 3 | Delete/refresh **stale docs** that actively mislead: STATUS.md SB128 "NOT landed" block, STATUS.md:195 bd10 "unported" list, sb128_geom PORT-NOTEs 276/322, CLAUDE.md queue #4 | D1 |
| 4 | Route the pipeline's inline geometry through the DEAD `frame_geom` helpers (`sb_geom`, `mi_cols/rows`, `seq_size_bits`) — one definition of the frame extent | D2 |
| 5 | Discharge the `seq_size_bits` PORT-NOTE (frame_geom.rs:253) — already proven correct by the odd-dim gate; mark verified | D2 |
| 6 | Discharge/annotate the `real_coeff_ctx`/`bd10_full_rd` fragile invariant with an explicit assert (pipeline.rs:1187) | D4 |
| 7 | Annotate the pd0 `_ => 3` size-slot as SB128-safe-by-b64-decomposition (benign, but reads as the fixed EntropyCtx::bsl bug class) | D3 |
| 8 | Refresh the stale `hbd.rs` header + pipeline.rs:4322 "intentionally panics on directional/filter-intra" comment (both now ported) | D1 |

Everything below the line "**NOT a safe shotgun fix**" (§C) is a genuine multi-pass MD
near-tie or a whole-path port — do **not** attempt those blind; each needs its own
instrumented-C per-candidate RD dump.

---

## A. Arbitrary-sized still frames (Priority 1) — top rank

### A1. 65×65 (preset 0–5 partial-SB) OOB panic — ROOT PINNED

- **What:** encoding a partial-SB frame (aligned dims not a multiple of 64) at preset
  0–5 via `encode_frame_420` reaches an out-of-bounds chroma access and panics
  (65×65 preset 0, both bd8+bd10). It is *also* byte-unported even without the panic.
- **Where (Rust):**
  - Dispatch: `crates/svtav1-encoder/src/pipeline.rs:5714` —
    `let refined = matches!(preset, 0..=5) && use_funnel && full_sb;` A partial SB has
    `full_sb == false` (pipeline.rs:5596), so `refined` is false and the frame drops to
    the M6 fixed-tree fallback but with `FunnelCfg::for_preset(0..=5)`.
  - CfL always-on at M0–M2: `crates/svtav1-encoder/src/leaf_funnel.rs:525` (`cplx_th 0`
    "BYPASSES the detector — CfL is always evaluated"); armed at leaf_funnel.rs:5089–5432
    (u8) and 5395–5421 (bd10 arm, the cited :5417).
  - Aligned chroma stride: `pipeline.rs:5086` `let cwid = w / 2;` (aligned, not
    sb-extent), used as `FunnelCtx::c_stride` at pipeline.rs:5646/5790.
  - No guard: `encode_frame_420` (pipeline.rs:380) only `debug_assert!`s 8-alignment
    (pipeline.rs:394) — the mono `encode_frame` at least `debug_assert!`s `preset >= 6`
    for partial SBs (pipeline.rs:361), but that is (a) debug-only and (b) absent from the
    420 path entirely.
- **Where (C):** there is no single C line to "match" — the point is that C's preset-0–5
  partial-SB path is a *different search* (PD0_LVL_0 / edge-aware depth-refine + NSQ),
  not the M6 fixed tree. The edge-shape restriction C applies is `set_blocks_to_test`
  (`Source/Lib/Codec/enc_dec_process.c:1394-1438`); CfL eligibility is
  `is_cfl_allowed`-class logic; the port's preset-6+ path already reproduces the edge
  restriction (arbitrary-dims-port-map.md §"Partial SBs"), just not for the preset-0–5
  funnel config.
- **Fix — TWO distinct deliverables, keep them separate:**
  - **Panic-free (SHOTGUN, ~1 change):** in the funnel, suppress the CfL arm on any leaf
    that straddles or is an edge node — gate CfL on
    `abs_x + cw*2 <= aligned_w && abs_y + chh*2 <= aligned_h` (mirrors the existing
    `commit_leaf` straddle clip at leaf_funnel.rs:6153–6183, but on the CfL read side).
    OR add a real runtime guard in `encode_frame_420` that rejects/pads-and-falls-back
    partial SBs at preset < 6 (promote the mono `debug_assert` at pipeline.rs:361 to a
    hard path). The guard is the smallest and touches nothing on the byte-covered
    preset-6+ path. **This does NOT give byte-identity — it only stops the crash.**
  - **Byte-identity (MULTI-PASS, NOT shotgun):** make `depth_refine.rs` edge-aware (its
    own comment flags it: pipeline.rs:5710 "depth_refine.rs is not yet edge-aware") and
    port C's preset-0–5 partial-SB decision (PD0_LVL_0 + NSQ at the boundary + CfL edge
    rules). This is the same class as the *un-gated preset-0–3 full-SB* parity gap
    (below) — presets 0–3 gradient already diverge full-SB (STATUS.md:141 "Remaining 24
    = gradient at M0-M3"), so partial-SB 0–5 cannot be byte-exact until full-SB 0–5 is.
- **Sibling panic site (same family):** `crates/svtav1-encoder/src/pd0.rs:655`
  `_ => unreachable!("PD0 tx {}x{}")` in `tx_quant_core` — the PD0 transform dispatch. Its
  shape list has **already** had to be extended for #95 tall-rect shapes (Tx32x64/16x32/
  8x16, arbitrary-dims-port-map.md:100-102), so a preset-0–5 partial-SB/straddle leaf can
  panic HERE too, not only in the CfL path. Any partial-SB robustness fix must confirm the
  PD0 tx dispatch covers every shape the preset-0–5 boundary search can emit (this is a
  *confirm-and-extend* site, not a *confirm-only* one).
- **Risk:** the panic-free guard is boundary-only (partial-SB + preset<6) and cannot
  touch the byte-covered preset-6+ path or any full-SB cell. The CfL-suppression variant
  touches shared funnel code — re-verify `identity_matrix` (6/10/13), `partial_sb_gate`
  (101), and any preset-0–5 full-SB cell that arms CfL (M0 gradient) stays unchanged.
- **Blast:** stops the public-API crash on every preset-0–5 partial-SB frame. Byte-cells:
  none until the multi-pass port lands (there is no preset-0–5 partial-SB gate today).

### A2. High-qp straddle / multi-SB residual — wire cropped-RDO distortion

- **What:** partial-SB "straddle" cells at p7/p8 diverge at high qp (`200x120 q40/55`,
  `80x88/104x88/72x88/120x120 q55`) — the port codes a different byte count. C crops the
  RDO **distortion** metric to the aligned extent for a tx that reaches past it; the port
  sums the full (padded) region → different RD → different partition/mode pick.
- **Where (Rust):** the funnel distortion is computed over the **nominal** tx dims —
  `tx_unit` / `TxRdArgs` (`crates/svtav1-encoder/src/leaf_funnel.rs:1586` freq,
  1841/2096 spatial). `frame_geom::cropped_tx_dims` (`frame_geom.rs:221`) exists, is
  correct, and is **DEAD** (no caller — verified by grep).
- **Where (C):** `Source/Lib/Codec/full_loop.c:2228-2230` (`cropped_tx_width_uv` /
  `cropped_tx_height_uv = MIN(tx_dim, aligned - origin)`) and
  `product_coding_loop.c:4664` (luma cropped distortion). Confirmed the C consumers pass
  the cropped dims to the distortion kernels (full_loop.c:2356-2384).
- **Fix:** thread the aligned dims into the funnel and, at each distortion call, pass
  `cropped_tx_dims(dims, abs_x+tx_x, abs_y+tx_y, txw, txh)` to the distortion kernel
  (recon-vs-source SSE and the freq-domain path) while the coded residual/TX stays full.
  This is exactly what `cropped_tx_dims` was written for.
- **Risk:** **shared distortion path** — touches every leaf's RD. It is byte-neutral by
  construction where `cropped == nominal` (all full-SB frames and all non-straddling
  partial blocks), but re-verify `identity_matrix` (6/10/13) + `partial_sb_gate` (101) +
  `bd10_*` are byte-unchanged.
- **Blast:** candidate close for the 5–6 high-qp straddle cells. **CAVEAT (honest):** the
  arbitrary-dims-port-map (line 261–278) concluded the *low-mid-qp* straddle cells were
  NOT an uncropped-distortion problem (they shared the boundary-COST root, since fixed)
  and flags the high-qp remainder as "a separate genuine RD near-tie". So wiring
  `cropped_tx_dims` is the **first thing to try** but must be validated with a
  differential — if it does not close them, the remainder is a §C-class near-tie, not a
  shotgun fix.

### A3. bd10 partial-SB falls back to u8 (not byte-exact)

- **What:** any bd10 frame whose dims are not a multiple of 64 skips both the bd10
  full-RD funnel and the bd10 post-pass and emits the **u8-quantized** levels under a
  bd10 header — correct only for flat content, wrong (non-byte-exact) for real content.
- **Where (Rust):** `pipeline.rs:1150` `bd10_frame_aligned = w % 64 == 0 && h % 64 == 0`
  gates the post-pass; `bd10_full_rd_supported` (pipeline.rs:4962) also requires
  `w%64==0 && h%64==0`. Both false ⇒ u8 fallback (pipeline.rs:1143-1149 documents it).
- **Where (C):** the bd10 tx path is `svt_av1_inv_txfm2d_add_*_c` +
  `highbd_quantize_fp_helper_c` (full_loop.c:367-395) at the true depth; the partial-SB
  geometry crosses the straddle/edge handling that the u16 tx unit
  (`leaf_funnel::tx_unit_hbd`) is "not yet partial-SB-aware" about.
- **Fix — MULTI-PASS, NOT shotgun:** make the bd10 u16 re-encode partial-SB-aware (the
  edge/straddle tx footprint the highbd tx unit can't map). Follow-up already logged in
  `docs/bd10-port-map.md`.
- **Risk:** bd10-only; full-SB bd10 cells (bd10_matrix 36/36, nonflat, photo) must stay
  byte-unchanged.
- **Blast:** every partial-SB bd10 cell (none gated today).

### A4. SB128 encode — LANDED (correction to the task/STATUS premise)

- **What:** the task brief and `STATUS.md:118-141` say `sb128_encode_supported()` is
  `false` and SB128 cells fall back to SB64. **This is stale.** A chunk-3 landing
  (2026-07-19) flipped it on.
- **Where (Rust):** `pipeline.rs:272-281` `sb128_encode_supported()` now returns `true`
  unconditionally; pipeline test asserts `!p.sb128_fallback` (pipeline.rs:6267). Walk:
  `sb128_geom::sb_coding_units` (sb128_geom.rs:164) + `pipeline::merge_sb_units`
  (pipeline.rs:3999). 12/14 `sb128_gate.sh` cells byte-match (gate 18/18).
- **Why it was small:** on an I_SLICE C clamps the MD scan's max square to 64×64
  regardless of SB size (`Source/Lib/Codec/enc_dec_process.c:1483-1499`), so the 128 root
  is **structurally always PARTITION_SPLIT** — no 128-level NONE/HORZ/VERT search on KEY.
- **Relevance to arbitrary size:** IN scope and DEFAULT for **preset 0/1** above the
  165,120-px area threshold (`INPUT_SIZE_240p_TH`, definitions.h:1834) — i.e. essentially
  every realistically-sized preset-0/1 photo. Now works (12/14).
- **Remaining (all inert/out-of-envelope on KEY, sb128-port-map.md:400-429):**
  `av1_intra_luma_prediction` multipler (unmodelled, empirically inert — clamp bites
  *less* at SB128, the safe direction, product_coding_loop.c:4027); `tx_reset_neighbor_
  arrays` (only at tx_depth>0); **bd10 × SB128** (untested); **INTER** (needs a real
  128-level RD search, `debug_assert`ed). The 2 pinned cells are NOT SB128 — see §C3.

### A5. Presets 0–3 are byte-unported even full-SB (context for A1)

- **What:** the default `identity_matrix.sh` gates only presets **6/10/13**
  (`IM_PRESETS:-13 10 6`, tools/identity_matrix.sh:28), with a comment that unported
  preset paths "may hang" (:29). Presets 0–3 gradient diverge (STATUS.md:141); presets
  4–5 full-SB were closed via `depth_refine` but are not in the default gate. So preset
  0–5 is the general parity frontier, not just an arbitrary-size corner.
- **Fix:** MULTI-PASS — the PD0_LVL_0 (M0/M1) + NSQ + CfL-at-M0 decision port. This is
  the same body of work A1's byte-identity half needs.
- **Blast:** unlocks preset 0–5 both full- and partial-SB; large scope.

---

## B. Coverage matrix — which (dim × preset × bd) is byte-covered

| axis | SB64, bd8, 4:2:0 | notes |
|---|---|---|
| full-SB, presets 6/10/13 | **byte-gated** (identity_matrix default) | 54/54 |
| full-SB, presets 4/5 | closed (depth_refine) but **not in default gate** | run via `IM_PRESETS` |
| full-SB, presets 0–3 | **NOT byte-exact** (gradient diverges, STATUS.md:141) | multi-pass |
| partial-SB, presets 6/7/8/9/10/13 | **byte-gated** | partial_sb_gate 101/101 |
| partial-SB, presets 0–5 | **PANIC (p0) / unported** | §A1 |
| partial-SB, presets 6–13, 2 residuals | recon-invisible p6, high-qp straddle p7/p8 | §C1, §C2/§A2 |
| bd10, full-SB, preset ≤ 8 | byte-gated (full-RD funnel) | bd10 matrix/nonflat/photo |
| bd10, full-SB, preset ≥ 9 | byte-gated (level-only post-pass) | real_coeff_ctx=false band |
| bd10, partial-SB | **u8 fallback, not byte-exact** | §A3 |
| SB128, preset 0/1, ≥165k px | **12/14 byte-match** (LANDED) | §A4; 2 near-tie pins |
| multi-tile, preset 6 | **25 cells diverge** | §C4 |
| HDR fork × bd10 | 46/64 byte-match; **18 diverge** | §C5 |
| 4:4:4 / 4:2:2 / mono | **OUT OF SCOPE** (no C oracle) | ACCEPTANCE-CRITERIA.md:27 |

---

## C. NOT a safe shotgun fix — genuine near-ties / whole-path ports (flag, don't blind-fix)

Each of these needs an instrumented-C per-candidate RD dump to close; none is
pointable from source alone.

### C1. 65×65 q32 / 65×96 q20 recon-invisible coeff near-tie (p6)
- Decoded pixels are **byte-identical**; the streams differ only in the padding-dominated
  corner block's coefficient/sign choice (first divergence op 5626, a bypass bit in the
  both-partial corner SB). arbitrary-dims-port-map.md:261-274. A coding near-tie in the
  cropped padding region; qp-specific. **Multi-pass** (`SVT_CCOEF_XY` dump vs port coeff).

### C2. High-qp straddle p7/p8 near-tie
- See §A2 — try wiring `cropped_tx_dims` first; if that does not close it, the map flags
  it as a genuine RD near-tie (arbitrary-dims-port-map.md:277).

### C3. The 2 SB128 pins (`gradient 512x384 / 448x384 q32 p0`)
- A single leaf-cost RD near-tie at a 32×32 node: C codes PARTITION_VERT_4, port codes
  NONE — V4 **loses by 0.207 %** in the port's NSQ dump. Proven NOT SB128 (reproduces at
  SB64 on `424x384`, below the area threshold). sb128-port-map.md:379-398. **Multi-pass.**

### C4. Multi-tile preset-6 residual (25 cells)
- Every diverging tile cell is **preset 6** (presets 10/13 are 48/48). A preset-6
  non-tile-aware MD corner + the per-tile rate-chain (`chain_snaps` PORT-NOTE at
  pipeline.rs:5433, indexed tile-locally). tools/tile_gate.sh:164-187. Orthogonal to the
  arbitrary-size goal (only fires on multi-tile requests). **Multi-pass.**

### C5. HDR fork × bd10 tail (18 cells)
- Complement of the measured 46/64 (docs/HDR-ON-4.2.md:292). Class A (3 cells, a QM
  residual beyond kernel/level wiring) + Class B (deeper q5 cells). HDR-fork-gated,
  orthogonal to the mainline arbitrary-size goal. **Multi-pass** (sibling-C RD dump).

### C6. Synthetic bd10 p0/p3 `diag` residual
- p0 = a mid-tile MODE flip (op 163, SMOOTH-vs-DC); p3 = the PARTFLIP axis (bd10 leaf-cost
  precision flips the partition depth). docs/bd10-port-map.md:1829-1834 / :527.
  **Multi-pass MD near-tie.** (Caveat: the sibling `diag q5/q12` class was a pointable
  chroma re-encode defect, already landed — so keep a "could still be pointable" flag on
  p0 until a dump confirms it's precision.)

---

## D. Stub / hardcode / stale-marker sweep (Priority 2) — mostly shotgun/maintainability

### D1. Stale docs that actively mislead (delete/refresh — SHOTGUN, high value)
- `STATUS.md:118-141` — "NOT landed: the SB128 encode path … `sb128_encode_supported()`
  is `false`". **Contradicts code** (pipeline.rs:280 returns `true`) and
  sb128-port-map.md:281. The task brief inherited this. Refresh.
- `STATUS.md:195` + `pipeline.rs:4322-4329` doc-comment — "intentionally panics on
  directional intra / filter-intra / tx_depth>0" for bd10. **Stale:** directional and
  filter-intra are now ported (`predict_unit_hbd` handles them, leaf_funnel.rs:1786-1808;
  `bd10_tree_supported` only rejects tx_depth>0 unconditionally, and directional only
  when the SH edge filter is on). Only tx_depth>0 is an unconditional fallback now.
- `hbd.rs:1-8` header — "COMPILED … but NOT YET CALLED from production". Stale: several
  kernels (`cdef_filter_block_hbd`, `full_distortion_kernel16_bits`, `cfl_*_hbd`,
  `lpf_*_hbd`) ARE wired. Only the qlookup arms remain unwired (and correctly dead, D3).
- `sb128_geom.rs:276` and `:322` PORT-NOTEs — both resolved by chunk 3 (cdef fan-out not
  needed on KEY; per-quadrant `cdef_idx` landed). The `CdefTransmit` /
  `cdef_strength_fanout_offsets` / `sb128_variance` / `sb128_bridge_*` structs in that
  file are **written-but-unconsumed** (the pipeline reimplemented the CDEF state machine
  inline as `CdefSbState`). Refresh or delete.
- `CLAUDE.md:562-645` (queue item #4) — describes #95 chunk 2 as "IN PROGRESS (the SEARCH
  restructure — the real work)". Stale: chunk 2 LANDED (arbitrary-dims-port-map.md, 101/101).

### D2. Dead `frame_geom` helpers — route inline derivations through them (SHOTGUN, maintainability)
- `frame_geom.rs` `sb_geom`, `cropped_tx_dims`, `mi_cols/mi_rows`, `seq_size_bits` are
  correct but **DEAD** (only `FrameDims::new`, `pad_input_plane`, `edge_has_rows_cols` are
  wired). The pipeline re-derives the frame extent inline (e.g. `cwid = w/2`,
  `sb_ext_w/h`, `w.div_ceil(sb)`). The module doc (frame_geom.rs:8-12) and CLAUDE.md:618
  ask for this. One definition of the extent removes a class of drift bugs.
- `seq_size_bits` PORT-NOTE (frame_geom.rs:253) says "swap to this at #95 chunk 2 and
  byte-compare the SH on a non-aligned cell". **Already discharged:** the SH writer
  derives the same value inline (`obu.rs:609-614`, `w_bits = 32 - (width-1).leading_zeros()`)
  and the odd-dim cells (65x64/65x63/…) byte-match, which proves it correct at odd TRUE
  widths. Mark the PORT-NOTE verified (or route obu.rs through the helper and delete it).

### D3. `_ => 3` size-slot catch-alls — benign but read as the fixed bsl-bug class
- `pd0.rs:1204` and `:1248` (`M6Pd0Tables` size slots 8/16/32/`_`) fold `64` into slot 3.
  This is the **same shape** as the EntropyCtx::bsl `_ => 3` bug that was fixed for SB128
  (pipeline.rs:2753). Here it is **benign**: this path only sees squares ≤ 64 (comment
  pd0.rs:1212 "is_128 = false"), and even at SB128 the b64-coding-unit decomposition
  (`sb_coding_units`) keeps every square ≤ 64. Verified benign — but add a one-line
  assert/comment so it doesn't read as a latent 128 bug to the next reader.
- **Agent false-positive verified and rejected:** `bd10::qzbin_factor` (bd10.rs:52) was
  reported as returning 64 where C returns 80. **It does not** — it matches
  `svt_aom_get_qzbin_factor` (inv_transforms.c:3492) exactly (`q==0 → 64`, else
  `dc<th ? 84 : 80`, `_ => 2368` is the correct bd12 threshold). No fix needed.

### D4. bd10 post-pass 0/0-RDOQ-context invariant — correct but fragile (annotate)
- `pipeline.rs:1187` `bd10_postpass_runs = !bd10_full_rd && bd10_frame_aligned && …`. The
  post-pass hardcodes `txb_skip_ctx/dc_sign_ctx = 0/0` (leaf_funnel.rs:1899), which is
  correct **only** because it runs solely in the preset≥9 band where `real_coeff_ctx ==
  false` (leaf_funnel.rs:855) — mapping to C's `update_skip_ctx_dc_sign_ctx` flag
  (full_loop.c:1901). The `!bd10_full_rd` term is the load-bearing guard and was a
  bitstream defect before it was added (bd10-port-map.md history). It is correct in the
  current reachable envelope but would silently re-open if `bd10_full_rd_supported` were
  widened downward without re-checking `real_coeff_ctx`. **Fix:** add a `debug_assert!`
  that the post-pass only runs where `real_coeff_ctx` would be false, so the coupling is
  enforced, not implicit. (bd8 never runs this block — it is under `if bit_depth == 10`.)

### D5. hbd.rs `unimplemented!`/`unreachable!` cluster — all DEAD (no fix needed)
- `hbd.rs:1559/1567` (dc/ac qlookup_10) — unwired; the real bd10 tables live in
  `bd10_qlookup_tables.rs:5/41` (FFI-verified by `c_parity_bd10_quant`). The hbd copies
  exist only because svtav1-dsp can't depend on svtav1-encoder (crate-dedup, not a gap).
- `hbd.rs:1575/1580` (bd12) + `:1536/:1548` (`_` bit-depth arms) — bd12 out of scope /
  unreachable bit depth. All dead. Verify-and-carry, no reachable still-frame path.

### D6. Palette-on-partial-SB PORT-NOTEs — reachable only for screen-content partial SBs
- `context.rs:1247` (`write_palette_map_tokens` within-bounds clip) and `:1134`
  (`palette_map_pixel_ctx`) — the clip only fires on a partial-SB frame carrying a
  palette (screen-content) block. Double-blocked: the EPICA screen cell doesn't byte-match
  anyway (#71 over-picking), so this is "exercised-but-not-verified". Low priority for the
  arbitrary-size (photographic) goal; verify once a non-64-aligned screen-content cell
  byte-matches. **Not shotgun** (needs an edge-clipped palette cell).

### D7. `intrabc.rs` is an ENTIRELY DEAD module (2375 lines) — decide: wire or document
- `pub mod intrabc;` compiles, but `intra_bc_search`, `build_intra_bc_candidate`, and
  `IbcCtrls` have **zero callers** anywhere in the crate (grep-verified — only
  self-references + its own `#[cfg(test)]`). All 7 of its PORT-NOTE/`unimplemented!`
  markers (intrabc.rs:181/383/708/870/929/1332/2057) are therefore unreachable regardless
  of content. The module's own doc-comment is **self-contradictory** (line 6: "NOT declared
  in lib.rs"; line 17: "permanently wired in lib.rs") — the truth is compiled-but-never-
  invoked. Screen-content IBC is a real still-frame feature (needed for the EPICA p2–p5
  cells, CLAUDE.md queue #2), so this is either (a) wire it into the funnel injection, or
  (b) fix the doc to say "unwired, prep only". Not shotgun to wire; shotgun to correct the
  doc. (The `:383` `unimplemented!` is additionally `#[cfg(not(feature="std"))]`-only and
  the encoder builds with std, so doubly dead.)

### D8. PORT-NOTE / `unreachable!` audit conclusion — no open correctness bug in the still envelope
- A full sweep of all 40 markers found **none that is simultaneously reachable in the plain
  photographic still envelope, auditable now, AND still uncertain about correctness.** The
  reachable-in-still-envelope `unreachable!`s (leaf_funnel.rs:1065/1099/2219/6316/6615,
  pd0.rs:1204/1248) are all **defensive completeness arms provably safe by caller-side
  domain closure** — confirm-and-close, not bugs. The cleanest to lock is
  `leaf_funnel.rs:6316` (`tx_size_cat`) which is a direct 1:1 of C `bsize_to_tx_size_cat`
  (`Source/Lib/Codec/inter_prediction.h:318`). Everything else is dead (intrabc, hbd
  qlookup, bd12), HDR-fork-gated (leaf_funnel.rs:3544/5943, pipeline.rs:831 — default
  `mainline()`, `is_fork()==false`), screen-content-gated (all palette markers +
  context.rs:1134/1247), or bd10-only. The one that needs *extending* rather than
  *confirming* as arbitrary-dims work lands is pd0.rs:655 (§A1 sibling).

---

## E. What was ruled out / already handled (do not re-chase)
- Tile count `1 << log2` — already fixed via `TileGrid::resolve` (pipeline.rs:786,
  documents the empty-trailing-tile + out-of-range `context_update_tile_id` bug it fixed).
- SB writer max-frame-size bits at odd dims — proven correct by the odd-dim gate (D2).
- `bd10::qzbin_factor` 64-vs-80 — verified matches C, agent false positive (D3).
- pd0 `_ => 3` as a 128 bug — benign by b64-unit decomposition (D3).
- SB128 CDEF fan-out / stale-quadrant machinery — correctly unconsumed on KEY (A4).
