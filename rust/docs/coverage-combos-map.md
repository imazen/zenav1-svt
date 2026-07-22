# Feature-combination coverage map (SB128×tiles, bd10×tiles, real×tiles)

Measured 2026-07-22 on branch `coverage/combos` (off CI-green `ef14c4a3e`).
Reference: mainline C at `/root/svtav1/Bin/Release` (`SVT_CREF_LIB_DIR`).
Gate: `tools/coverage_combos_gate.sh`. Scoreboard:
`benchmarks/coverage_combos_latest.tsv`.

## Why

Every pre-existing gate tests ONE feature in isolation — `tile_gate` (bd8,
SB64, tiles), `sb128_gate` (bd8, single-tile, SB128), `bd10_*` (bd10,
single-tile). Their INTERSECTIONS were documented as unmeasured
(`tile_gate.sh:208-217`, `docs/finishing-survey.md`, `docs/sb128-port-map.md`).
This gate measures the three the notes called out and turns the result into a
map: byte-MATCH cells become asserted byte-identity cells; DIFF cells become
pinned self-promoting targets with the first-divergence localized here.

## How the gate classifies a cell

Per cell it produces four encodes and compares:

| encode | what |
|---|---|
| `port_tiled` | the port with the tile request |
| `C_tiled` | C with the tile request |
| `C_single` | C at `rows=cols=0` (single tile) |
| `port_single` | the port at `rows=cols=0` — **the CONTROL** |

* **anti-vacuity** (hard): `C_tiled != C_single` — the tile request genuinely
  changed C's encode (axis 1 also asserts `C_tiled` really is SB128 via
  `sb128_seqhdr.py`).
* **control** (classify): `port_single` vs `C_single`. A cell whose CONTROL does
  not match has a pre-existing single-tile divergence (bd10 low-preset content,
  screen-content tools, a content near-tie) that is **not about tiles**, so its
  tiled result proves nothing about the intersection. **Every DIFF cell below
  has a MATCHING control** (verified) — so each is a genuine tile-intersection
  finding, not a pre-existing content divergence.
* **decodability** (hard, every cell): `aomdec` accepts `port_tiled`.
* **byte-exact / pin**: byte-exact cells are asserted; diverging cells are
  pinned self-promoting (a pin that starts matching FAILS the gate → promote).

## The map

Gate result: **40 / 40 green** — SB128×tiles 16 byte-exact / 0 diverging;
bd10×tiles 4 byte-exact / 8 pinned-diverging; real×tiles 8 byte-exact / 4
pinned-diverging. **Every cell's single-tile CONTROL matches** (no
content-diverges), so all 12 DIFF cells are genuine tile-intersection findings.

### Axis 1 — SB128 × tiles: 16 / 16 BYTE-EXACT ✅ (clean intersection)

Frames large enough that C picks SB128 (aligned luma area ≥ 165,120 AND
preset ≤ 1) with a tile grid: `512x384` (4×3 SB128), `512x512` (4×4),
`640x512` (5×4), content gradient/uniform/diag, grids r0c1…r2c2, qp 32/55,
presets 0/1. Every cell:
* C really codes SB128 (`use_128x128_superblock=1`, asserted),
* the tile request really changed C's bytes (anti-vacuity, asserted),
* **byte-identical to C**.

So the SB128 partition/coding walk composes correctly with the per-tile MD
funnel, **including the SB128 tile limits** (`TileGrid::resolve` shifts
`max_tile_width_sb` by the SB128 pixel-log2 — the code the tile_gate note said
"nothing exercises"). These 16 are now asserted in the gate.

### Axis 2 — bd10 × tiles: 4 / 12 byte-exact, 8 / 12 DIVERGE (localized)

* **uniform → byte-exact (4/4).** Bit-depth-independent skip content: the coded
  tile is identical to bd8 apart from the SH `high_bitdepth` bit, tiles or not.
* **gradient / diag → diverge (8/8)** at presets 6/10/13, e.g. `gradient
  256x256 q40 p10 r1c1` port 2240B vs C 2250B; `diag 256x256 q40 p10 r1c1` port
  3006B vs C 2982B. **Controls all match** (bd10 single-tile gradient/diag are
  byte-exact — the existing bd10 envelope).

**Root — localized, NOT the whole-frame re-encode.** The `docs/finishing-survey`
and the `bd10_reencode_node` PORT-NOTE blamed the whole-frame `TileMi` in the
bd10 re-encode (it runs post-merge, treating the frame as one tile). **That is
wrong, and now measured wrong:** threading correct per-tile `TileMi` into the
bd10 luma+chroma re-encode was verified **byte-inert** on every diverging cell
(preserved in git stash "cov-combos: byte-inert bd10 re-encode tile threading";
the `bd10_reencode_node` PORT-NOTE was corrected in place).

The divergence is UPSTREAM, in the **eff-M9 partition search**. `tree_diff` on
`gradient 256x256 q40 p10 r1c1`:
```
FLIP mi=(32, 16) bsize: C=6 port=9     # y=128, the tile-ROW boundary
FLIP mi=(32, 48) bsize: C=6 port=9     # y=128, the tile-ROW boundary
```
At the y=128 tile-row-boundary SBs the port keeps a 32×32 (bsize 9) where C
splits to 16×16 (bsize 6). Disambiguation (measured): C picks bsize 6 there at
**both** bit depths, and the port matches C at **bd8 tiles** (`gradient 256x256
q40 p10 r1c1 bd8` byte-matches) and at **bd10 single-tile** (control matches) —
it only diverges at **bd10 + multi-tile**. So the port's partition decision at a
tile boundary is bit-depth-sensitive in a way C's is not: a partition near-tie
(KB-2/KB-10 family) that the bd10 path tips the wrong way exactly at the tile
edge. Fixing it belongs with the bd10 u16 MD/partition pass
(`docs/bd10-port-map.md`), not the re-encode — follow-on.

### Axis 3 — real content × tiles (bd8): 8 / 12 byte-exact, 4 / 12 DIVERGE (localized)

| image | dims | grid | preset | verdict |
|---|---|---|---|---|
| CID 1001682 | 512×512 | r1c1 | 10 | MATCH |
| CID 1001682 | 512×512 | r2c2 | 10 | **DIFF** (port 11245 vs C 11256) |
| CID 1001682 | 512×512 | r2c2 | 6 | MATCH |
| CID 2119713 | 512×512 | r2c2 | 10 | MATCH |
| CID 2119713 | 512×512 | r1c1 | 6 | MATCH |
| CID 4666751 | 512×512 | r2c2 | 10 | **DIFF** (port 6216 vs C 6211) |
| CID 2738653 | 512×512 | r1c1 | 10 | MATCH |
| CID 1484678 | 512×512 | r2c2 | 10 | MATCH |
| screen windows95 | 640×512 | r1c1 | 10 | MATCH |
| screen windows95 | 640×512 | r2c2 | 10 | MATCH |
| screen graph | 832×512 | r1c1 | 10 | **DIFF** (port 8212 vs C 8215) |
| screen graph | 832×512 | r2c2 | 10 | **DIFF** (port 8486 vs C 8494) |

All controls match (single-tile real content is byte-exact at these cells), all
deterministic. Two properties stand out:
* **tile-count dependent**: CID 1001682 matches r1c1 but DIVERGES r2c2 (same
  image, same qp/preset — the finer 2×2 grid tips it).
* **content dependent**: photographic images mostly match; `graph` (screen)
  diverges at both grids, `windows95` (screen) matches both. Not a
  screen-vs-photo split — a per-content near-tie.

**Root — same family as axis 2: eff-M9 partition near-ties at tile boundaries.**
`tree_diff`:
```
graph 832x512 r1c1:  FLIP mi=(76,84) bsize C=6 port=3   (port over-splits 16→8)
                     + 3 port-only sub-blocks
1001682 512x512 r2c2: FLIP mi=(32,16) bsize C=12 port=9  (y=128 tile boundary;
                        port splits 64→32 where C keeps 64)
                      FLIP mi=(48,96) C=9 port=6
                      FLIP mi=(56,104) C=6 port=9
```
The tile_gate is 162/162 on **synthetic gradient**, whose partition decisions
sit far from the RD/variance threshold, so its tile-boundary handling never
reacts. Real photographic/screen statistics sit ON the threshold at tile
boundaries, and the port's eff-M9 tile-boundary partition decision diverges from
C (some over-split, some under-split). Distinct from the bd8 preset-6 M6-PD0
tile-boundary fix (that closed the synthetic multi-tile preset-6 residual);
these are eff-M9 (preset 10) near-ties on real content. Follow-on — a per-cell
sibling-C RD dump at each flipped node (the KB-2/KB-3 method) is the close.

## What this gate does NOT cover (honest remaining scope)

* **bd10 × SB128** — a 4th intersection (`finishing-survey.md` A4 lists it
  untested). Every SB128 cell here is bd8; every bd10 cell here is SB64 (256²).
  Not measured.
* **SB128 × tiles at higher tile counts / larger frames** — capped at 5×4 SB128
  (640×512) to hold preset-0 encode time down; the SB128 `max_tile_area_sb`
  quartering only binds on frames far larger than any still image.
* **real × tiles at presets < 6** — the real cells are preset 6/10 (SB64). Lower
  presets add the full-RD partition search on top of tiles; unmeasured here.
* **bd10 × tiles fix** — localized to the partition, not landed (needs the bd10
  u16 partition pass). The byte-inert re-encode threading is in stash for when
  that lands.
* **real × tiles fix** — localized to eff-M9 tile-boundary partition near-ties,
  not landed (per-node RD dump is the close).
