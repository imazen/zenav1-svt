# Historical GitHub issue #18: bd10: 10-bit encode produces wrong pixels above ~8-12 MP (8-bit and zenrav1e are fine at the same size)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/18. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

## Summary

The 10-bit (`bd10`) encode path produces **structurally wrong pixels above ~8–12 MP**. 8-bit at the same size and content is fine, and a different 10-bit AVIF encoder (zenrav1e) on the same sources is fine, so this is specific to this port's 10-bit path.

Found while running the registered AVIF high-bit-depth arm (`zenmetrics/benchmarks/avif_hdr_arm_plan_2026-09-02.md` §10.4d).

## Measurement

Per-image mean SSIMULACRA2 on the native DOE corpus, `bd10` vs its 8-bit control:

| size class | images | mean ssim2 |
|---|--:|---|
| ≤ 8.01 MP | 24 | **+43.5 … +91.1** (healthy) |
| ≥ 12.00 MP | 8 | **−59.8 … +7.9** (broken) |

Worst case, `1008.scale3000x4000.png` at **q=90**:

* 8-bit control: **ssim2 86.57**
* `bd10`: **ssim2 −57.05**

An ssim2 of −57 at q90 is destroyed output, not a rate/quality trade.

## Controls (all measured, none assumed)

* **Not the metric or the harness** — the blob was pulled back out of storage and rescored locally: `-57.049692268057996`, reproducing the fleet value exactly.
* **Not geometry or depth** — the file is a well-formed 3000×4000 **10-bit** AVIF; av1C box, AV1 sequence header and decoder `ImageInfo` all agree on depth 10.
* **Not 8-bit** — the 8-bit arm at the same image and q scores 86.57.
* **Not content** — the same scan family spans the threshold: 7.91 MP scans score **+89.4 / +91.1**, 16 MP scans score **−19.4 / −36.5**.
* **Not 64-alignment** (falsified) — `2320×3408` is equally unaligned (`w % 64 == 16`) and scores **+89.4**.
* **Not 10-bit AVIF generally** — the same 12.2 MP sources encoded 10-bit through zenrav1e score **+71.0 … +71.5**.

## Threshold

**Bracketed, not bisected: healthy at 8.01 MP, broken at 12.00 MP.** Narrowing it needs encodes at intermediate sizes, which I did not run.

## Note on STATUS.md

`rust/STATUS.md:55-58` records a `bd10` **byte-parity** gap at low presets on 64-aligned sizes. That is a different claim from a >8 MP correctness cliff, so this looks new rather than a known-issue instance.

## Impact

Two waves hit it. A high-bit-depth HDR encode wave was **stopped at 120/3,248 cells** once this was measured (24.5 MP HDR cells scoring −64 … −68), so ~96 % of that compute was not spent. Any `bd10` result on sources above ~8 MP should be treated as invalid pending a fix.


## Historical comment — 2026-09-02T20:49:37Z

## Root cause found and fixed: bd10 intra prediction crossed TILE boundaries

**It is not a size cliff.** The size threshold is a proxy. The driver is AV1's
**forced tile grid**, and the same defect reproduces at **0.27 MP**.

### What is actually wrong

AV1 forces a multi-tile grid once a frame exceeds either limit in
`svt_av1_get_tile_limits` (ported at `TileGrid::resolve`, `entropy/obu.rs`):

| limit | fires when | in pixels |
|---|---|---|
| `MAX_TILE_WIDTH` | `sb_cols > max_tile_width_sb` | **width > 4096** |
| `MAX_TILE_AREA` | `sb_rows * sb_cols > max_tile_area_sb` | **SB-aligned area > 4096·2304 = 9,437,184** |

A request of `(TileRowsLog2, TileColsLog2) = (0, 0)` is **clamped up** to that
minimum, never honoured — and `AvifEncoder` never requests a tile at all. So
every AVIF encode past ~9.44 MP is multi-tile whether or not anyone planned
for it. (Both limits are identical at SB64 and SB128: the limit and the SB
shift scale together.)

Intra prediction is tile-scoped in AV1 — a block on a tile's own top row / left
column has **no** above / left neighbour, because a conforming decoder
reconstructs each tile independently and has no such pixels either. The 8-bit
path honours that (`partition::extract_neighbors_tiled` takes
`tile_top`/`tile_left`). **Two 10-bit sites did not**, one per preset band:

| site | preset band | was |
|---|---|---|
| `predict_unit_hbd` non-directional arm → `partition::extract_neighbors_hbd` | ≤ 8 (full-RD funnel) | availability `abs_y > 0` / `abs_x > 0` — frame-absolute |
| `bd10_reencode_{luma,chroma}_node` | ≥ 9 (level re-encode post-pass) | hardcoded `TileMi::whole_frame` |

`predict_unit_hbd`'s **directional** arm was already correct (it passed
`tile: geom.tile` into `dr_predict_hbd`), so the gap was exactly DC / V / H /
smooth\* / paeth / filter-intra.

The encoder therefore read real pixels across the tile edge while the decoder
used the unavailable-edge fills, and everything from the boundary onward
drifted. `extract_neighbors_hbd`'s own doc comment stated the false premise:
*"`tile_top == 0` (single tile row) matches the funnel's current scope."* The
funnel's scope is not the caller's choice.

### Localization (the bisect that matters is not on size)

Oracle throughout: the encoder's published final 10-bit recon
(`EncodePipeline::last_recon10_final`) vs `aomdec` output, sample for sample —
no tolerance, no C reference needed. Content `gradient`, q20, 4:2:0.

**Dimension experiment — the flip is not driven by width, height, or area:**

| cell | MP | SB grid | tiles (requested 0,0) | bd8 | bd10 |
|---|--:|---|---|---|---|
| 4096×64 | 0.26 | 64×1 = 64 | **1** | ok | **ok** |
| 4160×64 | 0.27 | 65×1 = 65 | **2 cols** | ok | **65,054 / 399,360 differ** |
| 4096×128 | 0.52 | 64×2 = 128 | **1** | ok | **ok** |
| 4160×128 | 0.53 | 65×2 = 130 | **2 cols** | ok | **196,291 / 798,720 differ** |
| 256×256 | 0.07 | 4×4 = 16 | 1 | ok | ok |
| 256×256 `r1c0` | 0.07 | 4×4 | 2 rows | **ok** | **24,169 / 98,304 differ** |
| 256×256 `r0c1` | 0.07 | 4×4 | 2 cols | **ok** | **24,977 / 98,304 differ** |
| 256×256 `r1c1` | 0.07 | 4×4 | 2×2 | **ok** | **30,609 / 98,304 differ** |

Two 0.27 MP frames one superblock column apart, one clean and one destroyed,
settles it: **not area, not an overflowing accumulator — tiles.** bd8 is
identical at every tiling, which is why the 8-bit control looked fine.

The corruption is spatially exact. On 4160×128 the tile-column boundary is at
`33 SB · 64 = 2112`; **196,263 of the 196,291 differing luma samples are at
x ≥ 2112**, and the 28 that are not sit at x = 2110–2111 (deblock filtering
across the tile edge, reading the already-wrong tile-1 pixels). Against the
source: mean |err| left of the boundary 8.19 both sides, right of it **encoder
8.19 / decoder 13.45, max 292 vs 78** — i.e. the encoder's own reconstruction
is fine everywhere and only the decoded picture is wrong, which is the
signature of an encoder/decoder prediction mismatch rather than a bad encode.

**Prediction test.** Predicted flip: `ceil(w/64)·ceil(h/64) > 2304`. Tested on
a pair straddling it by one superblock row (a 0.19 MP bracket, vs the issue's
8.01–12.00 MP), pre-fix binary:

| cell | MP | SBs | vs 2304 | pre-fix | post-fix |
|---|--:|--:|---|---|---|
| 2944×3200 | 9.42 | 2300 | ≤ | **ok** | ok |
| 2944×3264 | 9.61 | 2346 | > | **3,448,059 / 14,413,824 differ** | **ok** |

Confirmed exactly.

### Every reported data point, re-derived

| report | SB grid | forced tiles? | reported |
|---|---|---|---|
| ≤ 8.01 MP (24 images) | ≲ 2000 SB | no | healthy ✓ |
| 2320×3408 (7.91 MP, the 64-alignment control) | 37×54 = 1998 | no | +89.4 ✓ |
| 3000×4000 (12.0 MP, worst case) | 47×63 = **2961** | **yes** | **−57.05** ✓ |
| 16 MP scans | ≈ 3900 SB | **yes** | −19.4 / −36.5 ✓ |
| ≥ 12.00 MP (8 images) | ≳ 2930 SB | **yes** | broken ✓ |
| zenrav1e at 12.2 MP | — | different encoder | +71 ✓ |

No exceptions. The 8.01–12.00 MP bracket contains 9.44 MP.

**One exposure the bracket missed:** the width limit is independent of area, so
a frame **wider than 4096 px** was broken at *any* size — a 6000×1000
panorama (6 MP) was affected while the bracket said "healthy below 8 MP".

### Fix

Three files, both sites made tile-scoped:

* `partition::extract_neighbors_hbd` takes `tile_top` / `tile_left` and derives
  availability from them, exactly like the u8 twin; `predict_unit_hbd` passes
  `geom.tile.top_px(geom.ss)` / `.left_px(geom.ss)`, exactly like `predict_unit`.
* `bd10_reencode_{luma,chroma}` take the resolved `TileGrid` and hand each
  superblock its own `TileMi`. New `TileGrid::tile_mi_for_sb` is the single
  owner of "which tile is this SB in" (raster-order walking is still fine for
  the recon *reads* — an in-tile above/left SB is always already written; only
  the *availability* needed scoping).

All 16 cells of {preset 6, 9, 10, 13} × {1×1, 2×1, 1×2, 2×2} now read
encoder-recon == decoder at bd10.

**Byte-inertness, measured A/B on 28 cells:** 26 emit **identical OBUs** —
every single-tile cell at both depths (presets 0/2/3/5/6/9/10/13, including the
partial-SB 200×136, 96×80, 65×257, 383×512 cells) and **every bd8 multi-tile
cell**. The only two that moved are bd10 × multi-tile, i.e. the broken
configuration.

### Gates

* `rust/svtav1/tests/issue18_repro.rs` — 4 cells + a single-tile control, on
  the decoder oracle. Pre-fix: forced-columns and tile-rows **FAIL**, control
  **passes** (so the oracle is not vacuous). Anti-vacuity: each cell asserts
  via `TileGrid::resolve` that the grid it is named for actually resolved.
* `rust/tools/regression_spotcheck.sh` — new `bd10ReconEq` helper + 4 cells.

### Note for whoever reads `docs/coverage-combos-map.md`

That doc's Axis 2 says threading per-tile `TileMi` into the bd10 re-encode was
verified **byte-inert**, and concludes `whole_frame` "is NOT the root". That
measurement is correct and it hid this bug for six weeks: it was taken on the
**C byte-parity** oracle, which cannot see an encoder/decoder mismatch at all
(both encoders wrong the same way stays green). On the **decoder** oracle
`whole_frame` was a root — of wrong pixels, not of wrong bytes. Corrected in
place; the eff-M9 partition near-tie remains the open *byte* root and is
unchanged by this fix.

### Against C

The fix also moved **7 of the 8** previously pinned-diverging `bd10 × tiles`
cells in `coverage_combos_gate.sh` to **byte-exact with the C reference** — the
gate's own PIN-BROKEN check fired on all seven and they are promoted. Axis 2
goes **4/12 → 11/12 byte-exact**. The one that did not move
(`gradient 256x256 q40 p10 r1c1`, C=2250B port=2240B) is the eff-M9 partition
near-tie the doc already analyses, which is a byte root and not a correctness
one.

Verified green after the fix: workspace tests **2,458 / 2,458** (189 binaries),
regression spotcheck **71 / 71**, coverage combos **40 / 40**, bd10 recon
parity vs C **13 / 13**, alignment gate **74 / 74**, bd10 matrix **36 / 36**,
bd10 non-flat **309 / 309**, bd10 partial-SB **159 / 159**, bd10 hbd-src
**26 / 26**, tile gate **29 / 29**.

Landed on `main` as `3121b6a8`. Envelope guard 8 added to `rust/CLAUDE.md`
("tiles are not opt-in"), since the premise that broke this — *"single tile
matches the funnel's current scope"* — is about a knob the caller does not own.

### Impact on the stopped wave

Any `bd10` result on a source whose SB-aligned area exceeds 9,437,184 px, **or
whose width exceeds 4096 px**, is invalid and needs re-encoding. Results below
both limits were single-tile and are unaffected.


## Historical comment — 2026-09-02T20:57:26Z

Closing the loop on the exact reported geometry, since everything above was measured on synthesised cells.

**`3000×4000`, 10-bit, preset 7** (`AvifEncoder`'s default speed 6 maps to preset 7), requested tiles `(0,0)` — the image and depth this issue was filed on. SB grid `47×63 = 2961 > 2304`, so AV1 forces 2 tile rows.

| | encoder final 10-bit recon vs `aomdec` |
|---|---|
| parent commit | **3,412,579 of 18,000,000 samples differ**, first at Y r2040 c80 |
| `3121b6a8` | **OK — identical** |

The first bad row is 8 above the tile-row boundary at `32 SB × 64 = 2048`, which is the deblock filter reaching back across the tile edge into the already-wrong tile — the same signature as every smaller cell.

Also worth stating plainly for anyone re-running the wave: the **width** limit is independent of area. A frame wider than 4096 px was broken at *any* size, so "under 8 MP" was never a safe rule on its own. The two conditions to check are `ceil(w/64)·ceil(h/64) > 2304` **or** `w > 4096`.

One gate this lane did not leave green, stated so the run above is not over-read: `tools/bd10_hbd_pq_gate.sh` reads **48/60** on my box, all 12 failures at preset 6 — the cells `docs/SUSPECTED-C-BUGS.md` #9 pins as aarch64-scoped, failing here on x86-64 against a **locally built** C reference rather than CI's. Ruled out as this change's doing by direct A/B: the port's OBU is byte-identical pre-fix vs post-fix on **all 60** of those cells, so the verdict cannot have moved. Left as found rather than annotated — that entry's owner should decide whether it is a second host-divergence datapoint or a real x86-64 gap.

## Historical comment — 2026-09-02T21:14:19Z

## The fix repairs the tile-COLUMN case; the tile-ROW (portrait) case is still broken

Re-validated on real corpus cells after `3121b6a8`, composed via `[patch]` against a clean `90fca425a` (which has `3121b6a8` as an ancestor). **The 4 `issue18_repro` tests pass here**, so this is not a "didn't get the fix" report — it is a residual the synthetic tests do not catch.

`bd10`, q=90, speed 4, through the zenavif `encode-svt-rs` seam, SSIMULACRA2 vs the source. Same encoder, same run:

| dims | orientation | sb-area | pre-fix | **post-fix** |
|---|---|--:|--:|--:|
| **4000×3000** | landscape | 12,128,256 | broken | **+88.96 ✅** |
| **3000×4000** | **portrait** | 12,128,256 | −57.05 | **−10.61 ❌** |
| 3302×4844 | portrait | 16,187,392 | broken | **+1.56 ❌** |
| 3286×4868 | portrait | 16,400,384 | broken | **−15.45 ❌** |
| 2479×3230 | portrait | under limit | fine | +88.21 ✅ |
| 3000×2235 | landscape | under limit | fine | +90.10 ✅ |

**4000×3000 and 3000×4000 have identical area and identical sb-aligned area.** The only difference is which axis is long — and one is repaired while the other is not. That points at the forced tile grid being split along **rows** in the portrait case, with the row-boundary neighbour availability still reading across a tile edge.

The 8-bit control on the same 3000×4000 cell scores **86.57**, so ~88–90 is the target; −10.61 is still destroyed output, just less destroyed than the −57.05 it started at.

`issue18_bd10_tile_rows_recon_matches_aomdec` passes, so whatever the real portrait path does differs from that test's construction — plausibly the tile-row count actually chosen at these dimensions, or a partial-SB interaction on the bottom row that the synthetic case does not reach.

### Impact

This is blocking a high-bit-depth wave: 13 of its 16 HDR references are portrait and over the area limit, so re-encoding on the current fix would still produce wrong pixels on ~81% of the wave. Holding the restart until the row case lands.

Reproduce with any portrait source over `4096·2304` sb-aligned area at `bd10`; the 4000×3000 / 3000×4000 pair above is the tightest control, since only orientation varies.


## Historical comment — 2026-09-02T21:38:35Z

## Round 2: the residual is `dr_predict_hbd` — it took a tile and ignored it. Fixed; the reported cell is clean.

Confirmed the re-verification, found the residual, and fixed it. Thank you for the 4000×3000 / 3000×4000 pair — it is what made this findable.

### One correction to my first comment, because it is the whole mistake

I wrote: *"`predict_unit_hbd`'s **directional** arm was already correct — it passed `tile: geom.tile` into `dr_predict_hbd` — so the gap was exactly DC / V / H / smooth\* / paeth / filter-intra."*

**Passing a tile is not using one.** `intra_edge::dr_predict_hbd` received the correct `DrGeom.tile` and then derived every availability predicate from the FRAME:

```rust
// dr_predict_hbd — BEFORE                    // dr_predict (u8 twin) — always correct
have_top  = g.row_off > 0 || g.mi_row > 0;    g.mi_row > g.tile.mi_row_start
have_left = g.col_off > 0 || g.mi_col > 0;    g.mi_col > g.tile.mi_col_start
right_available  = ... < mi_cols;             ... < g.tile.mi_col_end
bottom_available = ... < mi_rows;             ... < g.tile.mi_row_end
```

All four `g.tile` fields, threaded in and discarded. That is why bd8 was always fine (its directional path was tile-scoped from the start) and why the directional arm was the *larger* half of the defect — it is the one real photographs reach.

### Why my tests were blind — measured, and it is NOT the tile axis

The orientation reading turned out to be a coincidence of which preset each arm ran, not an axis asymmetry. `4000×3000` and `3000×4000` both resolve to **1 tile column × 2 tile rows** — same grid — so a rows-vs-columns explanation could not have been right. Sweeping my own test geometry (256×256, 2 tile rows, bd10) instead:

| | q6 | q12 | q20 | q40 |
|---|---|---|---|---|
| **p0 / p2 / p3 / p4 / p5** | FAIL | FAIL | FAIL | FAIL |
| p6 / p7 / p8 / p9 | ok | ok | ok | ok |
| `uniform`, any preset | ok | ok | ok | ok |

(`gradient` **and** `diag`; 12,480–24,901 of 98,304 samples.)

**It is the PRESET axis, and my grid was 6/7/9/10/13 — the passing side.** `dr_predict_hbd` is reached only when a directional leaf wins, and that is the band where the intra candidate set still offers one. `uniform` is flat, so it never gets there — which is also why the multi-tile `uniform` cells in `coverage_combos_gate.sh` have always been green. Not content, not qp, not the tile axis, not orientation: every one of those was varied and none discriminates.

`AvifEncoder`'s **speed 4 → preset 4** and **quality 90 → qp 6** sit dead centre of the failing band. That is the gap between "synthetic gates green" and "product broken", and it is the real lesson here.

### Fix

Four lines: `dr_predict_hbd`'s availability block is now character-for-character its u8 twin's. Byte-identical for a whole-frame tile by construction.

### Measurements

Encoder final 10-bit recon vs `aomdec`, before → after:

| cell | before | after |
|---|--:|---|
| **`1008…_3000x4000` real photo, qp 6 / preset 4** (your cell) | **6,468,452 / 18,000,000**, first Y r2048 | **0** |
| same, preset 0 | broken | **0** |
| `gradient 2920×3270` — portrait, forced by AREA (46×52 = 2392 SB), partial SB both axes | **4,185,160 / 14,322,600**, first Y r1664 | **0** |
| 60-cell sweep {gradient,diag,uniform} × p{0,2,4,6,9} × qp{6,12,20,40}, 2 tile rows | 24 fail | **0** |
| same sweep at 2×2 tiles | — | **0** |
| same sweep at **bd8** | 0 | **0** |

The first differing row is the tile-row boundary in both large cells — r2048 = 32 SB × 64, r1664 = 26 SB × 64 — a predicted boundary, confirmed twice.

**Byte-inertness A/B, 32 cells: 30 identical.** Every single-tile cell at both depths across presets 0/2/3/4/5/6/9/10/13 (including partial-SB 200×136, 96×80, 65×257, 383×512 and `screen`), and every bd8 multi-tile cell. Only the two bd10 multi-tile cells moved.

**C-parity: `coverage_combos_gate.sh` 40/40, bd10 × tiles unchanged at 11/12, no `PROMOTE` fired.** Worth stating plainly: every cell on that axis is preset 6, 10 or 13, so it read *identically* before and after — **a green axis said nothing about this bug.** I've noted in that doc that extending it means extending it *down* into presets 0–5.

Also green: regression spotcheck **81/81** (5 new `bd10ReconEq` cells), bd10 recon parity vs C 13/13, alignment gate 74/74.

### Tests (written before the fix, failing at HEAD)

* `issue18_bd10_directional_tile_rows_recon_matches_aomdec_low_preset_band` — sweeps presets **0/2/4/5** × {gradient@q12, diag@q6}, and asserts its 256×256 grid resolves to the **same (1 col × 2 rows)** as `3000×4000` does on its own. Pre-fix: 27,861 / 98,304.
* `issue18_bd10_directional_forced_tile_columns_recon_matches_aomdec` — forced (not requested) columns, preset 2. Pre-fix: 178 / 399,360.
* `issue18_bd10_forced_by_area_portrait_recon_matches_aomdec` — **your shape, not a stand-in**: `2920×3270`, portrait, tiles forced by AREA, partial SB on both axes, preset 4. ~6.4 s, deliberately paid.
* `issue18_bd10_directional_single_tile_control_matches_aomdec` — the same band at a single tile; passes before and after, so "low preset is just broken" and "a fix that disables directional prediction" both fail it.

### Remaining rollout steps — neither is mine to make

1. **`zenavif` pin bump.** It pins `zenav1-svt` at rev `ef0b122bd` (`fix(inter): a PANIC was being counted as a byte divergence`), which predates both fixes — confirmed by ancestry, which is why the `[patch]` was needed. That bump is a deliberate commit in `zenavif`, **not a `[patch]` left on master**; I have not touched that repo.
2. **A new fleet image tag** carrying the bumped seam, per the one-package-many-tags rule.

Landed on `main` as **`2ca060f4`**.

t2a can re-encode from scratch once both land. Both limits still apply to what needs re-encoding: SB-aligned area > 9,437,184 px **or** width > 4096 px.

