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
| 2 | ~~Wire `frame_geom::cropped_tx_dims` into the funnel distortion~~ **DONE 2026-08-03** — closed the p6 q55 straddle-win trio (gate 101 → 104); the p7/p8 high-qp remainder is a different root | A2 |
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

### A2. High-qp straddle / multi-SB residual — wire cropped-RDO distortion — **LANDED 2026-08-03 (PARTIAL close)**

- **What:** partial-SB "straddle" cells at p7/p8 diverge at high qp (`200x120 q40/55`,
  `80x88/104x88/72x88/120x120 q55`) — the port codes a different byte count. C crops the
  RDO **distortion** metric to the aligned extent for a tx that reaches past it; the port
  summed the full (padded) region → different RD → different partition/mode pick.
- **Where (Rust) — NOW WIRED:** `frame_geom::cropped_tx_dims` + the new
  `cropped_tx_dims_uv` feed `leaf_funnel::tx_unit`'s new `crop` parameter,
  `TxRdArgs::crop` (the bd10 twin) and `txt_search`'s new `crop` argument.
  `FunnelFrame` gained `frame_w_px` (the aligned width; `frame_h_px` already existed)
  so the leaf can build the `FrameDims` the bound is taken against.
- **Where (C):** `Source/Lib/Codec/product_coding_loop.c:4664-4665` (`tx_type_search`)
  and `:5752-5754` (`perform_dct_dct_tx`, the same expression with inert `(uint8_t)`
  casts + `tx_height >> mds_subres_step`) for LUMA; `full_loop.c:2228-2232`
  (`cropped_tx_width_uv`/`_height_uv`, computed in the CHROMA domain from the ROUND_UV
  origin) for chroma. All three feed ONLY the spatial arm
  (`svt_spatial_full_distortion_kernel_facade` + `get_svt_psy_full_dist` + the tune-SSIM
  kernel); the coefficient-domain arm takes the FULL tx dims and cannot crop.
- **MEASURED (2026-08-03, crop OFF → ON on the same build, 48 partial-SB cells):**
  8 cells changed bytes, **3 went DIFF → MATCH** (`80x88 q55 p6`, `104x88 q55 p6`,
  `72x88 q55 p6` — the documented straddle-win trio), **0 regressed**. Those three are
  now gated in `tools/partial_sb_gate.sh` (101 → 104 cells).
  Byte-neutral elsewhere: `identity_matrix` 54/54, `bd10_matrix` 36/36, workspace 945/945.
- **STILL OPEN (a different root):** `200x120 q40/55` at p7/p8 and `80x88/104x88/72x88/
  120x120 q55` at p7/p8, plus `120x120 q55 p6`. Their bytes DID move with the crop (so
  the crop is live there too) but they still diverge — consistent with the port-map's
  read that the high-qp p7/p8 remainder is a separate root. **The candidate named
  here is now RULED OUT (measured 2026-08-04).** This entry used to say the
  suspect was `end_tx_depth` (`product_coding_loop.c:6712-6717`) "which is NOT
  ported". It IS ported (leaf_funnel.rs, landed 2026-08-03) and every one of the
  ten cells still diverges with it live — verified against a positive control in
  the same run, so the zero is trustworthy. The root is UNIDENTIFIED.

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
| HDR fork × bd10 | 54/64 byte-match; **10 diverge** | §C5 |
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
- `cropped_tx_dims` IS now wired (§A2, landed 2026-08-03). It closed the p6 q55 trio
  (80x88 / 104x88 / 72x88) and MOVED the p7/p8 cells' bytes, but did NOT close them —
  so the remainder is confirmed a separate root, as the map suspected
  (arbitrary-dims-port-map.md:277). The next candidate named here — the
  `end_tx_depth` frame-boundary force-to-0 (product_coding_loop.c:6712-6717) —
  is **RULED OUT**: it landed 2026-08-03 and all ten cells still diverge with it
  live (measured 2026-08-04, positive control green). No candidate stands.

### C2a. The p4/p5 straddle remainder is in the REFINED walk, not shared with p7/p8

MEASURED 2026-08-04, after the edge-aware PD1 walk landed. Sweeping the three
geometries where p5 still diverges across presets 4..7 (`~/tmp/straddle.sh`,
positive control `gradient 64x64 q20 p6` IDENTICAL in the same run):

| geometry | p4 q20 | p4 q48 | p5 q20 | p5 q48 | p6 q20 | p6 q48 | p7 q20 | p7 q48 |
|---|---|---|---|---|---|---|---|---|
| 72x88  | OK | **NOT** | **NOT** | **NOT** | OK | OK | OK | OK |
| 80x88  | OK | **NOT** | **NOT** | OK | OK | OK | OK | OK |
| 104x72 | OK | OK | **NOT** | **NOT** | OK | OK | OK | OK |

**p6 and p7 are byte-identical on every one of these cells.** So this is NOT a
shared "straddle geometry" root with the p7/p8 high-qp remainder (C2 above), and
the two should not be chased together — the earlier reading that they were the
same family is refuted. p6/p7 take the fixed-tree PD0 path; p4/p5 take the
refined PD1 walk, so the remainder is specific to **the refined walk on a
partial SB**.

**It is NOT about straddling leaves — that guess was measured and killed.** The
first version of this entry said the failures lived in the walk's handling of a
STRADDLING node (a block reaching past the aligned extent). Counting straddling
leaves in `SVTAV1_PACKTREE` for the five p5-failing cells says otherwise:

| cell | leaves | straddling |
|---|---|---|
| 72x88 q20   | 126 | **0** |
| 80x88 q20   | 152 | **0** |
| 104x72 q20  |  38 | **0** |
| 96x80 q48   |  14 | **0** |
| 65x65 q48   |  21 | 18 |

Four of the five code NO straddling leaf at all. The probe is trustworthy: the
positive control (`80x88 q55 p6`, the straddle case this repo already documents)
reports the expected `(0,64) 64x32 -> (64,96)` leaf, so the zeros are real
absences and not a silent harness. (The first attempt at this count WAS a silent
harness — it filtered on `x=`/`y=`/`w=`/`h=` keys the dump does not emit, and
returned 0 for everything including the control.)

So the open question is narrower and differently shaped than "straddle": why
does the refined walk diverge on a partial SB whose coded leaves are all fully
inside the frame? That points at the BOUNDARY-node handling (rates, the injected
shape, the forced-split path) or at the refinement gates' thresholds on the
smaller edge geometry — not at straddling blocks.

**Where in the stream it happens.** All three probed cells have BYTE-IDENTICAL
frame headers — the divergence is entirely tile payload, so the deblock/CDEF/LR
searches (which run on the recon) already agree:

| cell | C / port payload | first differing byte |
|---|---|---|
| 104x72 q20 p5 | 1562 / 1564 | 11 — but only because the payload LENGTHS differ, so the leb128 size field moves. Not a header bug. |
| 96x80 q48 p5  |  167 /  165 | 11, same reason |
| **72x88 q20 p5** | **1244 / 1244** | **1056 of 1265 (~83 % through)** |

**Start with `72x88 q20 p5`.** Equal payload lengths means no size-field noise,
and 83 % through a 2x2-SB frame puts the first divergence in the LAST superblocks
— the right/bottom/corner partial ones, which is exactly the new edge code. The
corner SB there is the interesting one: its 64x64 root at (64,64) has both flags
false (forced split), its only in-frame child is a 32x32 that is one-false
(injected VERT), and the frame is just 8 px wide at that column.

**The legality is VERIFIED IDENTICAL — so it is a COST question, not a rules
question.** Traced `set_blocks_to_test` (enc_dec_process.c:1394-1437) against
that exact node (16x16 at (16,80), `has_cols` true, `has_rows` false, allintra
M5 → `nsq_geom_level` 3 → `enabled = 1`, `min_nsq_block_size = 8`):

- the force-split early-out needs `sq_size <= MAX(min_nsq = 4,
  min_nsq_block_size = 8)` = 8; the node is 16, so C does NOT force-split;
- `max_part` = `PART_V` (via `inj_hv_incomp`), and the in-loop filter
  `if ((has_cols && part != PART_H) || (has_rows && part != PART_V)) continue;`
  drops PART_N and PART_V and keeps **PART_H only** — `tot_shapes = 1`.

That is exactly what the port injects. C is allowed to keep a 16x8 here and
chooses not to, so the port's edge shape is too CHEAP or its split too
EXPENSIVE. Rates are not the suspect either: `edge_shape_bits` /
`edge_split_bits` were checked against `svt_aom_partition_rate_cost`
(rd_cost.c:1834-1866), including the easily-inverted detail that `!has_rows`
selects the **vert**_alike table and `!has_cols` the **horz**_alike one, and the
`[p == PARTITION_SPLIT]` bool index. Remaining suspects: the `PARENT_COST_BIAS`
(995) compare at a boundary node, and whether the refinement scan should admit
depth 16 there at all.

One correction to the port's own paraphrase while verifying: `pd0.rs` calls the
`sq_size <= MAX(...)` term "inert ... edge nodes are always >= 16 wide". True at
M0..M3, where `min_nsq_block_size` is 0. At **M4..M6 it is 8** (geom level 3), so
the term would fire on an 8x8 one-false node — still unreachable, because an 8x8
node on an 8-aligned frame always has `hbs = 4` and both flags true, but
unreachable for a different reason than the comment gives.

**Two findings from instrumenting that node** (`SVTAV1_NSQDBG=1
SVTAV1_DBG_MI=20,4`):

1. **FIXED** — `test_split` ran its per-quadrant early exit BEFORE the in-frame
   check, so an out-of-frame quadrant could abort the split (`NSQDBG TSX ...
   i=2 parent=7520607 split=7576441`; i=2 is at y=88 on an 88-tall frame). Both
   real children had already been evaluated. Byte-neutral on today's grid — 0
   cells changed verdict — and wrong regardless.
2. **STILL OPEN** — with that out of the way the node makes a genuine cost
   compare and the port still keeps the parent by 0.7 %:

   | | cost | components |
   |---|---|---|
   | parent (injected 16x8) | 7,520,607 | block 7,478,024 + part_rate **850** |
   | split (two 8x8)        | 7,576,441 | 3,921,260 + 3,641,104 + rate **281** |

   Note the rates come from the binary alike alphabet and already favour split
   (281 vs 850); **the block costs are what carry the decision.**

   Everything around the block costs has now been read against C's
   `test_split_partition` (product_coding_loop.c:10770-10845) and matches:
   - the split rate is `RDCOST(full_lambda, above_split_rate, 0)` with the `*2`
     bias applied only when `!use_accurate_part_ctx` — which is 1 at M2..M8, so
     no doubling at p5, as the port assumes;
   - the final compare is `parent_cost_bias * parent_rd <= split_cost * 1000`,
     identical to the port's;
   - the out-of-bounds `continue` precedes the early-exit check, which is what
     finding 1 above fixed.

   So legality, rates, bias and compare are ALL verified identical, and the
   divergence is inside the leaf block cost itself — the port's 16x8 at (16,80)
   is too cheap relative to its two 8x8s, by under 1 %. Pinning that needs a
   C-side MD leaf dump (the `-Wl,--wrap` PICKPART/MD instrumentation, i.e. a
   GNU-ld host) rather than more reading: every surrounding quantity has been
   eliminated.

**Reproduce:** `tools/identity_diff.sh 72 88 20 5 gradient`, then
`SVTAV1_PACKTREE=/tmp/t.txt` (delete it first — it APPENDS) for the port's tree.

**Candidate 1 (cropped-TX distortion) is RULED OUT.** The obvious suspect was
that the refined path's leaves miss the cropped-TX spatial distortion the
fixed-tree path gets — a straddling block priced over its pad rows is exactly
the failure the crop fixed for p6 at q55, and the qp-dependence here (q48
failing where q20 passes on 72x88/80x88 at p4) has the same signature. Checked
by reading the call chain: `depth_refine.rs:1788` calls
`leaf_funnel::evaluate_leaf`, and the crop is computed INSIDE that function
(`leaf_funnel.rs:3627` `uv_crop`, `:3634` `blk_crop`, both from the shared
`FunnelFrame`'s `frame_w_px`/`frame_h_px`). Both paths therefore get it. Not
the root.

Remaining candidates, untested: the walk's `test_split` early-exit thresholds
and parent-vs-split compare at a node whose children straddle (the child RD sums
include pad-region cost that C's may not); the PD0 `Pd0Eval` costs feeding
`build_refined_scan_at` at a straddling node (PD0's `block_cost` reads a full
`sq_size` block from the padded plane — correct for C, but the refinement GATES
compare those costs against thresholds); and the NSQ shape gates at a straddling
node. The discriminator to reach for first is which SB the first divergence
lands in — `SVTAV1_PACKTREE` gives the port's tree, but the C-side tree needs
the `-Wl,--wrap` PICKPART dump, i.e. a GNU-ld host.

### C2b. The two real-corpus images at presets 0..4 (`1028637.png`, `graph.png`)

MEASURED 2026-08-04 (512x512 centre crop, presets 0..5 x qp{5,20,32,48,63},
`~/tmp/two_images.sh`; the corpus paths are `codec-corpus/CID22/CID22-512/
training/` and `codec-corpus/gb82-sc/`):

- **preset 5 is byte-identical for BOTH images at every qp** (10/10 cells). The
  divergence is confined to presets 0..4.
- `1028637.png` (CID22 photo): q63 identical at every preset; q5/q20/q32/q48
  diverge at p0..p4.
- `graph.png` (gb82-sc screen): identical at q63 for p1/p3/p4 and at every qp
  for p5; the rest diverge.
- Where the lengths match closely enough for the field walk to run, **the frame
  header is byte-identical and the divergence is entirely tile payload** (e.g.
  `graph q63 p2`: C=252B port=252B, headers identical, first differing byte 160
  of 252). So this is an RD-decision divergence, not a syntax or header bug.

**Both screen-content tools are LOAD-BEARING and neither is simply
over-picking.** Bisected with the new `SVTAV1_SC_TOOLS` knob (see below), on
`graph.png`:

| cell | default | palette off | IBC off | both off |
|---|---|---|---|---|
| q32 p0 (C=3781B) | 3792 | **4186** | 3792 | 4178 |
| q32 p2 (C=4002B) | 3990 | **4304** | 3998 | 4353 |
| q32 p4 (C=4093B) | 4087 | **4472** | 4111 | 4573 |

Turning palette off costs 8-10 % in bytes and moves the port much FURTHER from
C, so the port's palette is winning real RD rather than winning spuriously. IBC
is inert at p0 q32 (identical byte count with it off) and active from p2 up.
This refutes the convenient reading that the screen divergence is #71
over-picking at these cells; the port is close to C in SIZE and differs in
WHICH decisions it makes.

**Not localized further here, and the reason is a real constraint:** this host
is aarch64/ld64, so `capture_c_trace` has no `-Wl,--wrap` op trace and
`identity_diff.sh` degrades to byte + header-field comparison. Symbol-level
localization of a tile-payload flip needs a GNU-ld host. The next step is that
trace, not more guessing from byte counts.

### C3. The 2 SB128 pins (`gradient 512x384 / 448x384 q32 p0`)
- A single leaf-cost RD near-tie at a 32×32 node: C codes PARTITION_VERT_4, port codes
  NONE — V4 **loses by 0.207 %** in the port's NSQ dump. Proven NOT SB128 (reproduces at
  SB64 on `424x384`, below the area threshold). sb128-port-map.md:379-398. **Multi-pass.**

### C4. Multi-tile preset-6 residual (25 cells)
- Every diverging tile cell is **preset 6** (presets 10/13 are 48/48). A preset-6
  non-tile-aware MD corner + the per-tile rate-chain (`chain_snaps` PORT-NOTE at
  pipeline.rs:5433, indexed tile-locally). tools/tile_gate.sh:164-187. Orthogonal to the
  arbitrary-size goal (only fires on multi-tile requests). **Multi-pass.**

### C5. HDR fork × bd10 tail (10 cells)
- Complement of the measured 54/64 (docs/HDR-ON-4.2.md). **Class A CLOSED** — it
  was the PD0 leaf quantize being QM-blind (C's `svt_aom_quantize_inv_quantize_light`
  applies the luma matrix, the port's `pd0::tx_quant_core` did not), a QM-tipped
  PD0 partition near-tie; fixed by `pd0::quantize_b_qm` threaded through the bd10
  `pd0_pick_sb_partition_lvl0` path (also closed diag 128 q48). Remaining: Class B
  (10 cells — the q5 cells + diag 128 q12), a deeper tile-level RD divergence.
  HDR-fork-gated, orthogonal to the mainline arbitrary-size goal. **Multi-pass**
  (sibling-C RD dump).

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
- `frame_geom.rs` `sb_geom`, `mi_cols/mi_rows`, `seq_size_bits` are correct but **DEAD**
  (`FrameDims::new`, `pad_input_plane`, `edge_has_rows_cols` and — since 2026-08-03 —
  `cropped_tx_dims`/`cropped_tx_dims_uv` are wired). The pipeline re-derives the frame extent inline (e.g. `cwid = w/2`,
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
