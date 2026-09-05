# SVT-AV1 Rust Port — Status

Last updated: 2026-08-04 (bd10 partial superblocks) — C baseline **v4.2.0**

> **2026-09-05: this file is HISTORY, not the current scoreboard.** It predates
> the inter campaign entirely — it has nothing about inter frames, video-mode
> key frames, global motion, the IntraBC screen band, or the CPU/memory axes,
> and several of its tallies have since moved. The live state is:
> `../CONTEXT-HANDOFF.md` (the index), `.github/workflows/rust-gates.yml` + the
> root `README.md` tables (CI tallies on every push), `docs/INTER-ENCODE-PLAN.md`
> (the §1z chunk log), `docs/perf-status.md` (CPU + memory) and
> `docs/WORKING-ON-THIS.md` (how to work here). Read this file for how a
> still-path result was reached, never for whether it still holds.

## 10-BIT AT ARBITRARY DIMENSIONS — the refusal is gone (2026-08-04)

Until this date `bit_depth_config_error` refused **every** 10-bit encode whose
aligned dims were not a multiple of 64 — the product case for 10-bit AVIF, since
real images are not 64-aligned:

> "10-bit requires 64-aligned encode dimensions: no bd10 producer is partial-SB
> aware, so the encode would be 8-bit-quantized under a 10-bit sequence header"

No bd10 gate could have caught it either way: `bd10_matrix.sh` sweeps
`BD10_SIZES=64 128` and `bd10_nonflat_gate.sh` only 64x64/128x128, so no bd10
gate reached a partial superblock at all.

**`tools/bd10_partial_sb_gate.sh` — 157/157 byte-identical, all previously
refused.** Wired into CI. `tools/arbitrary_size_robustness.sh` went from
**80/80 panic-free + 48 refused as out-of-envelope** to **128/128 with 0
refused** — those 48 are exactly these cells, and every one now decodes under
the AV1 reference decoder.

The stated blocker was the wrong function. `tx_unit_hbd` takes explicit
`(w, h, src_stride, src_off)`; its only geometry term is `TxRdArgs::crop`, which
the post-pass never even supplies (`rd: None`). `bd10_tree_supported`, blamed
elsewhere for the same gate, takes no coordinates and no frame dims at all. The
exposure was entirely in the CALLERS:

- preset ≤ 8 (full-RD funnel) needed **only the gate lifted** — it rides the
  same partition search and leaf funnel as the 8-bit path, which is partial-SB
  correct (`partial_sb_gate.sh` 146/146);
- preset ≥ 9 (level-only re-encode post-pass) needed real work: SB-extent-sized
  `recon10` (was ALIGNED-sized, so a straddling leaf wrote past the buffer or
  wrapped a row), straddle-clipped recon writes, SB-extent-padded 10-bit
  sources, and the pack's skip-off-frame-quadrant child walk in place of a fixed
  `(partition_type, children.len())` offset table — which both `panic!`s on a
  pruned child list AND, when the count happens to fit, places a
  right-edge-pruned bottom-left child at the top-right offset.

Two bugs fixed en route that were byte-inert before and would have corrupted
after: the per-tile bd10 canvas merge read at the SB-EXTENT stride while
`commit_leaf` writes at the ALIGNED stride, and the native-u16 source had no
SB-extent twin (its `debug_assert_eq!(in_stride, w)` was asserting that the
frame had no partial SB).

### The residual, measured rather than asserted

Data: `benchmarks/bd10_partial_sb_2026-08-04.tsv`.

|  | cells | MATCH | bd10-only failures |
|---|---|---|---|
| bd8 @ partial-SB, p0..p8 | 594 | 565 | — |
| bd8 @ 64-aligned, p0..p8 | 270 | 270 | — |
| bd10 @ 64-aligned, p0..p8 | 270 | 241 | 29 = **21.5%** of non-flat cells |
| bd10 @ partial-SB, p0..p8 | 594 | 490 | 78 = **26.3%** of non-flat cells |
| bd10 @ 64-aligned, p9..p13 | 90 | 90 | 0 |
| bd10 @ partial-SB, p9..p13 | 330 | 310 | 3 configs (a 4th fails at bd8 too) |

Every failing cell on every grid is `gradient`; `uniform` is 100% everywhere. In
the p0..p8 band the residual is the known bd10 NON-FLAT gap
(`bd10_nonflat_gate.sh`, 197/309 at 64-ALIGNED dims — an arm64 measurement,
see "Measurement caveat for arm64 hosts" below; x86 CI measures 309/309 as of
`1ed7db46`) plus ~5 percentage points
from partial-SB geometry. In the eff-M9 band it is four configurations, one of
which (`gradient 48x48 q20 p9`) is localized to a bd10 MDS0 fast-cost near-tie
on the frame's FIRST 32x32 block — a block that straddles nothing and has no
neighbours, so no partial-SB machinery participates. A representative slice is
PINNED self-promotingly in the gate. None of it is claimed closed;
`docs/bd10-port-map.md` has the per-cell trail.

### Safety measurement (the failure this would otherwise hide)

A runtime decline — `bd10_tree_supported` false on any SB — at preset ≥ 9
silently drops a frame to 8-bit-quantized levels under a 10-bit sequence header,
because `bd10_levels_native` approves that band from CONFIG alone and the
`hbd_source.is_some() && !hbd_used` backstop needs a NATIVE u16 source to fire.
Probed 297 cells under `SVTAV1_BD10_POSTPASS=1`: `runs=true`,
`unsupported_sbs=0/N` on all 297, and all 297 printed the diagnostic line (the
positive control — a probe that silently never ran reports the same zero). Not
reachable on that grid; NOT proven unreachable in general, since
`FunnelCfg::for_preset`'s `9..=255` arm can express `tx_depth == 1`. Closing
that hole is a standing follow-up and it PRE-DATES this work.

## 8-BIT: the comprehensive gate, and what it measures (2026-08-03)

8-bit 4:2:0 is the port's primary product surface, and until this date **no
8-bit byte-vs-C identity gate ran in CI at any preset**. `identity_matrix.sh`
is a scoreboard that exits 0 whatever the tally and was not in the workflow
anyway. `tools/identity_full_8bit.sh` is the gate that fixes that; it is wired
into `.github/workflows/rust-gates.yml` and fails on harness errors as well as
divergences, so a cell that could not run cannot look like a pass.

**Default run: 738/738 byte-identical, +2 pinned, 0 harness errors.**

| tier | axes | cells |
|---|---|---|
| synthetic | {uniform,gradient,diag,screen} x {64,128}px x qp{5,20,32,48,63} x **presets 0..13** | 560 |
| dims | 15 geometries 32x32..512x512 (odd 65x65, straddle 80x88, both-partial 120x104) x 2 content x qp{20,48} x p{6,9,13} | 180 |

Every preset 0..13 is byte-identical on 64-aligned content at every swept qp,
q5 and q63 included. Presets 10..13 are swept as DISTINCT configurations rather
than assumed equal to M9 — C clamps all-intra above M9
(`enc_handle.c:4415-4419`), the port does not.

### What is NOT clean, measured rather than assumed

Running the dims tier with the unclaimed band (`IF_PRESETS="0 4"`, committed as
`benchmarks/identity_full_8bit_dims_2026-08-03.tsv`) gives p0 30/60 and p4
34/60 against p6/p9/p13 at 60/60. The 56 divergences split into exactly two
ALREADY-KNOWN classes and no third:

- **53 partial-SB at p0/p4** — presets 0-5 skipped the C-faithful PD1 walk on a
  non-64-aligned SB (`pipeline.rs`'s `refined` required `full_sb`), so the
  search was structurally different. **LARGELY FIXED 2026-08-04**: the PD1 walk
  is now edge-aware (forced split at a both-false node, the single injected
  shape priced from the BINARY alphabet at a one-false node, off-frame quadrants
  skipped) and `refined` no longer requires `full_sb`. Partial-SB pass rates
  went p0 7→24/36, p1 7→24, p2 10→29, p3 11→33, p4 12→28, with the 64-aligned
  columns UNCHANGED at every preset. p5 did not move (25/36) — see below. See
  `docs/arbitrary-dims-port-map.md` and `docs/finishing-survey.md` §C2a.
- **3 ALIGNED `screen` cells at 256/384/512** — the #71 screen-content RD
  class, the same one the production-corpus sweep sees on its M0 screen
  classes.

Pinned in the gate, self-promotingly: `screen 64x64 q63 p1` and
`screen 128x128 q63 p1`. These are the SMALLEST known reproducer of #71 — the
documented witnesses were 512x512 photo/EPICA cells. Isolated: p0 and p2 at the
same qp are identical, q48/q55 identical at every preset, and with palette
forced off the port drops to 60B against C's 64B (SMALLER), so C codes palette
there too and the two simply decide differently.

### Real corpus: preset 6 and above is byte-identical on EVERY image tested

450 cells, 18 images at 512x512 centre crop x qp{5,20,32,48,63} x p{0,4,6,10,13}
(`benchmarks/identity_full_8bit_real_2026-08-03.tsv`), 403/450:

|            | p0 | p4 | p6 | p10 | p13 |
|---|---|---|---|---|---|
| gb82 photo | 30/30 | 30/30 | **30/30** | **30/30** | **30/30** |
| CID22 photo | 26/30 | 26/30 | **30/30** | **30/30** | **30/30** |
| gb82 screen | 10/30 | 11/30 | **30/30** | **30/30** | **30/30** |

The entire gap is presets 0/4, in the two already-known classes: the #71
palette/IBC calibration band on sc_class5 screen images (`codec_wiki` and
`gmessages`, the non-detected controls, are 25/25), and one CID22 photo in an RD
near-tie band. Magnitudes ±0.4%–±1% in BOTH directions — near-ties, not a
systematic mis-cost.

Across synthetic + dims + real that is ~1,500 8-bit cells with **no unexplained
divergence class**.

### CI now gates, for 8-bit

every preset 0..13 x 4 content classes x the full qp range; partial-SB and odd
geometry (104 cells); tiles, rows and columns (29); SB128 (22); and
panic-freedom on gradient AND screen (80). All four of the latter already failed
loudly — they were simply never in the workflow.

LIMIT, stated because it is the same class of gap this work set out to close:
the CI identity step runs the synthetic tier at ONE size to fit the job budget,
so partial-SB byte-parity and real content remain hand-run.

### Per-preset coverage: COMPLETE (all 14 presets x all 5 axes)

`python3 tools/coverage_matrix.py` consolidates every
`benchmarks/identity_full_8bit*.tsv` and prints `--` for an axis with no cells.
As of 2026-08-03 there are none — 2,495 cells:

| preset | synth | dims-aligned | dims-partial | real-photo | real-screen |
|---|---|---|---|---|---|
| p0 | 40/40 | 23/24 | 7/36 | 56/60 | 10/30 |
| p1 | 40/40 | 22/24 | 7/36 | 26/30 | 11/15 |
| p2 | 40/40 | 22/24 | 10/36 | 26/30 | 10/15 |
| p3 | 40/40 | 21/24 | 11/36 | 26/30 | 11/15 |
| p4 | 40/40 | 22/24 | 12/36 | 56/60 | 11/30 |
| p5 | 40/40 | **24/24** | 25/36 | **30/30** | **15/15** |
| p6 | 100/100 | **24/24** | **36/36** | **60/60** | **30/30** |
| p7 | 100/100 | **24/24** | 34/36 | **30/30** | **15/15** |
| p8 | 40/40 | **24/24** | **36/36** | **30/30** | **15/15** |
| p9 | 100/100 | **24/24** | **36/36** | **30/30** | **15/15** |
| p10 | 40/40 | **24/24** | **36/36** | **60/60** | **30/30** |
| p11 | 40/40 | **24/24** | **36/36** | **30/30** | **15/15** |
| p12 | 40/40 | **24/24** | **36/36** | **30/30** | **15/15** |
| p13 | 100/100 | **24/24** | **36/36** | **60/60** | **30/30** |

- **Every preset >= 5 is byte-identical to C on real content — photo AND
  screen.** Nothing in that band diverges on any real image tested.
- **Every preset >= 6 is 24/24 aligned and 34-36/36 partial**; p8/p10/p11/p12
  are 36/36. The dims gate default is now ALL of 6..13, measured per preset
  rather than assumed to follow a neighbour.
- **The remaining gap is presets 0..5**, in two classes: residual partial-SB
  geometry and RD divergence on two specific images.
- **Partial-SB is RESOLVED: 36/36 at p0/p1/p2/p3/p5 and 34/36 at p4** (was
  7-12/36 at p0-p4, 25/36 at p5). Two roots, both in the PD1 refinement path —
  the walk never ran on a partial SB, and a boundary PD0 leaf must never be
  refined because C's `tested_blk[PART_N][0]` is false there.
  `docs/finishing-survey.md` §C2a.
- **The two images: one cell of graph.png closed, the rest still open** (§C2b).
  `graph.png 512x512 q63 p2` (C=252B/port=252B, same length, differing from
  offset 160) is byte-identical after the IntraBC out-of-set tx-type fix. The
  other 29 graph cells and the photo's p0..p4 cells still diverge. Preset 5 is
  byte-identical for both images at every qp; headers match and the divergence
  is entirely tile payload. Both screen tools are load-bearing — disabling
  palette costs 8-10 % and moves the port FURTHER from C — so the convenient
  "#71 over-picking" reading is refuted for these cells.

Default gate: **1100/1100 byte-identical, 0 pinned, 0 harness errors**
(2026-08-16). The KNOWN_DIFF list is now EMPTY — every cell in the default
8-bit gate matches C.

Two rounds of promotion got it there, both driven by the self-promoting pin
rather than by anyone noticing: `screen 64x64` and `128x128 q63 p1` on
2026-08-04 (the luma-palette uv-flag row), then `screen 72x88` and `80x88 q48 p7`
on 2026-08-16 (C=187/187 and C=195/195), which were the last two partial-SB
misses at p7. The second pair was promoted UNCONDITIONALLY rather than
`uname -m`-scoped, and that was measured: both match on aarch64 AND on emulated
x86-64 with the C oracle rebuilt there. Six other cells in this repo match on one
architecture only (`docs/SUSPECTED-C-BUGS.md` #9), so checking first is the
difference between a promotion and a red CI cycle.

### How the coverage hole was found (the p1/p2/p3/p7 audit)

The first sweeps covered presets 0..13 on synthetic content but only
{0,4,6,9|10,13} on real content and geometry — so p1, p2, p3 and p7 lived in one
tier. That matters because of WHICH feature is unique to each.

`intrabc_level` is p0->3, p1->4, p2->5, p3->6, p4->7, p5+ ->0, so p1/p2/p3 are
the only presets carrying levels 4/5/6. MEASURED IntraBC blocks coded (q20):

| content | p0 | p1 | p2 | p3 | p4 | p7 |
|---|---|---|---|---|---|---|
| synthetic `screen` 128² | 0 | 0 | 0 | 0 | 0 | 0 |
| real screen 512² | 674 | 588 | 792 | 890 | 558 | 0 |

**IntraBC never wins a block on synthetic content at any preset** — palette does,
so the synthetic tier covered the palette half of the screen vertical and none of
the IntraBC half. Levels 4/5/6 therefore had no byte-parity coverage at all.
(p7's zero is correct: level 0 above p4. Its unique combination is palette_level
7 + the CDEF qp-strength arm + live screen detection, which
`screen_palette_bd_gate.sh` covers with 12 p7 cells.)

Coverage added — `identity_full_8bit_{real,dims}_p1237_2026-08-03.tsv`:

| | p1 | p2 | p3 | p7 |
|---|---|---|---|---|
| real, CID22 photo | 11/15 | 11/15 | 11/15 | **15/15** |
| real, gb82 photo | **15/15** | **15/15** | **15/15** | **15/15** |
| real, gb82 screen | 11/15 | 10/15 | 11/15 | **15/15** |
| dims, aligned | 22/24 | 22/24 | 21/24 | **24/24** |
| dims, partial | 7/36 | 10/36 | 11/36 | **34/36** |

**p7 is essentially production-clean** and is now in the dims gate (its two
misses, `screen` q48 at 72x88 / 80x88, are pinned). p1/p2/p3 land in exactly the
classes p0/p4 already showed — the same two images (`1028637.png`,
`graph.png`), `screen` q48 on large aligned sizes, and the documented
presets-0..5 partial-SB structural gap. No new class, no new image.

MEASUREMENT CAUTION: `SVTAV1_PACKTREE` APPENDS. A first pass at the table above
did not remove the file per run and reported cumulative counts (p7 appeared to
code 3502 IntraBC blocks). Remove it per run.

### Two panics the sweep found on the PUBLIC API

`intrabc_hash.rs` computed `x_end = pic_width - block_size + 1` in `usize`. C
computes it SIGNED (`hash_motion.c:195-196`, `:222-223`) so a picture smaller
than the hash block yields an empty loop; in `usize` it underflows to ~2^64 and
indexes off the end. A 32x32 screen frame at preset 0 panicked twice
(`len 1024, index 1024` and `index 2048`). No earlier gate encoded anything
below 60x60 with the screen-content tools armed — which is precisely why it
survived. Fixed with `checked_sub` + early return, regression-tested.

## Audit-driven port wave (2026-08-03)

A full-file C-vs-Rust audit (18 domains + 4 cross-cutting verticals, each
adversarially verified) produced a ranked backlog; this wave landed its top
items. Reports: `/Users/lilith/tmp/svt-port-audit-2026-08-03/` (not in-tree —
regenerate rather than trust; they are AI output and several claims were
overturned by measurement below).

| item | what it was | evidence |
|---|---|---|
| bd10 palette | funnel refused palette candidates at 10 bits | screen 128x128 q32: port 664B vs C 327B (p0) → byte-identical; new gate 58/58 |
| bd10 IntraBC | funnel refused IBC candidates at 10 bits | gb82-sc corpus mean size delta **+23.58% → +0.42%**; `terminal` p2 +75.2% → −1.2% |
| CDEF screen arm | `svt_pick_cdef_from_qp`'s screen branch unported | 10/12 preset-7 screen cells now byte-match; 512-cell C differential |
| variance-boost normalizer | `svt_av1_normalize_sb_delta_q` missing on the MAINLINE path | recon-vs-decoder **0/60 → 60/60**; new CI gate |
| cropped-TX distortion | `frame_geom::cropped_tx_dims` written but never called | `partial_sb_gate` **101 → 104/104** (3 straddle cells closed) |
| WHT kernels | AV1 lossless transform absent in both directions | 12 differentials vs the real exported C symbols |
| inter frames | emitted an UNDECODABLE stream through the public API | aomdec "Corrupt frame detected" / dav1d "No data decoded" → now refused |
| 10/12-bit configs | emitted 8-bit-quantized levels under a 10-bit header | refused at the `encode_frame_impl` choke point |
| `max_tx_size` cap | depth refinement hardcoded 64 | unit witness; byte-neutral at default |
| SB qindex in PD0 | partition search used the FRAME base qindex | 18/18 tune×qp recon parity |

Three claims were MEASURED WRONG in the course of this wave and are recorded as
such, because a status doc that only lists wins trains the next session to trust
the audit text over measurement — which is how several of these bugs shipped:
- the bd10 chroma recon proxy overwrite (a real dead-code defect) is
  byte-INERT on every cell reachable here, not the wide-blast-radius corruption
  the audit predicted;
- the CDEF screen-arm port initially shipped with a VACUOUS gate — deleting the
  wiring left the whole suite green — caught by the adversarial verifier, not by
  the port's own tests;
- **my own** claim that C's `end_tx_depth` frame-boundary rule was "measured
  unreachable". It is LIVE at preset 7. The probe behind the claim was an inline
  shell loop that silently never ran, and `grep -c` on empty output returns 0.
  Both the revert and the doc entry had to be undone (`4eca22119`, `d05decedf`).

That last one produced two standing rules, now in `rust/CLAUDE.md`:
**dead-looking C stays translated and documented, never reverted** (the analysis
calling it dead is often wrong, and upstream can re-enable a path with one
commit); and **a negative result needs an observed positive control** before it
is trusted, since a silent harness and a genuine absence are indistinguishable
from an exit code.

Standing gap this wave did not close: there is still **no 8-bit byte-vs-C
identity gate in CI at any preset** (`identity_matrix.sh` always exits 0 by
design and is not in the workflow), so every "byte-identical" claim outside the
CI gate list is a hand-run measurement.

## bd10 PALETTE — the M6 bd10 screen-content gap CLOSED (2026-08-03)

The bd10 mode-decision funnel refused to inject palette candidates
(`!bd10_funnel`, leaf_funnel.rs), and `bd10_funnel` is true for EVERY
64-aligned bd10 4:2:0 frame at every preset — so where C codes hundreds of
palette blocks the port coded none. The gate had been added to convert a
`tx_unit_hbd` panic into graceful output; it did that, but its parity cost was
never measured. MEASURED (`screen 128x128 q32`, port vs real C):

| preset | C | port (before) | ratio |
|---|---|---|---|
| 0 | 327 B | 664 B | 2.03x |
| 6 | 453 B | 1110 B | 2.45x |

This is the whole of the `imazen26_sweep_2026-07-24` preset-6 bd10 anomaly
(380/515 byte-identical vs **515/515 at bd8**, with all 135 failures on the
eight screen-detecting content classes). The sweep's own `.meta` attributed
those cells to the SAMEPART-DIFF MD residual, which had closed on 2026-07-23 —
that attribution was wrong. At M6, IBC is already off (`intrabc_level = 0`
above preset 4), so M6 bd10 was a PURE palette divergence; M0 adds IBC on top.

Ported: `count_colors_highbd` (C pic_analysis_process.c:869), a depth-generic
`search_palette_core` shared by `search_palette_luma{,_hbd}` (C has ONE search
parameterized by `is16bit`, palette.c:391-399), `clip_pixel_highbd` centroid
clipping, the `<< (bit_depth - 8)` cache-snap threshold, and a 10-bit
substitution prediction on the candidate. Two LATENT DESYNCS fixed alongside:
the palette COLOUR literals are `encoder_bit_depth` wide and BOTH the writer
(entropy_coding.c:4369) and the RD cost (rd_cost.c:600) hardcoded 8.

**Gate: `tools/screen_palette_bd_gate.sh` — 48/48 byte-identical**
({bd8,bd10} × {64,128} × q{20,32,55} × p{0,2,4,6}), wired into CI. It needed a
new synthetic content type (`screen`) because every pre-existing one is
photographic, the screen-content detector never armed, and therefore **no gate
cell in the repo could reach the palette path at all** — which is how this
shipped. The gate self-asserts anti-vacuity per cell (fails a cell that codes
zero palette leaves), so it cannot pass with the palette path deleted.

### Open, found while closing the above (NOT palette)

`screenrep` (high-entropy, exactly-repeated regions — opt-in via
`SP_CONTENTS`) is byte-identical at bd8 at every swept cell but diverges at
bd10 on p2/q20 and p6/{q20,q32,q55}, plus the pinned p4 cells. That is the
same class as `bd10_nonflat_gate`'s open cells (diag/gradient at bd10), not a
palette issue — but it is now reproducible from synthetic content, where it
previously needed a photo corpus.

**IBC at bd10 — CLOSED 2026-08-03.** The witness that synthetic content could
not provide came from the real screen corpus, which IS on this box at
`~/work/zen/codec-corpus/gb82-sc` (the gates' `SCREEN_DIR` default points at a
Linux path). It could not be run before because ten gate scripts hard-coded
`nice -n 19 ionice -c3`, and `ionice` is util-linux — `nice` execs its argument
vector, so on macOS the missing binary killed the command outright. Fixed via
`tools/lib_nice.sh` (probes for the tool). See the port-wave table above.

### Measurement caveat for arm64 hosts

`bd10_nonflat_gate` measures **197/309 on this arm64 box** where this file
previously claimed 309/309. A worktree at the pre-NEON commit `bf56f8177`
measures the SAME 197/309 with the SAME 112 cells, so the aarch64 NEON wave did
not cause it. Every documented run was on x86 (`dev-32gb`), and the failures are
bd10 + non-flat ONLY (bd8 54/54 and bd10-uniform 36/36 pass here) — exactly
where residuals leave the 8-bit range and PORTING.md warns the RTCD binds
kernels to arch-specific implementations not equivalent to their `_c` twins.
Untested hypothesis: an x86-vs-arm64 **C-oracle** difference rather than a port
bug. Do not read a bd10 non-flat number off an arm64 box until that is settled.

## Photo preset-0 bd8 — the universal "FH loop_filter_level" class CLOSED (2026-07-23)

The dominant real-content residual (the wider-corpus sweep's group 1: ~85% of
photo p0 bd8 cells diverging, first byte always the FH `loop_filter_level`,
pre-deblock recon SSE off by only ~0.08%) is closed by TWO roots in the
M0/M1 independent-uv chroma search, branch `parity/photo-p0-deblock`:

1. **`79cc43d3c` — the bd8 ind-uv fast-candidate sort.** C sorts the
   SAD-ranked (uv_mode, angle_delta) candidates with
   `sort_fast_cost_based_candidates` (product_coding_loop.c:1415, ind-uv call
   :7680) — a swap-on-`<` selection sort whose TIE order differs from
   injection order. The port's bd8 arm used a stable `sort_by_key` (bd10
   already replicated C) under a "byte-inert" claim. On real photos,
   flat-chroma SAD tie groups straddle the nfl=32 full-loop cut on nearly
   every frame -> a different full-loop set -> a chroma angle-delta / uv-mode
   winner flips mid-frame -> the winner's chroma recon shifts every later
   chroma DC prediction (localized with the new `tools/uvdc_join.py`:
   1604/3488 blocks with drifted DC inputs on the drill cell) -> MD cascades
   -> the recon-driven deblock/CDEF/LR searches re-price -> the FH symptom.
   **This ONE fix: photo p0 bd8 probe 61/135 -> 134/135** (135 cells = 27
   CID22+clic images x qp {5,20,32,48,63}, 512x512; 73 fixed, 0 regressed).
2. **`78bb5d361` — the bd8 ind-uv CfL arbitration tie-break.** C's
   `check_best_indepedant_cfl` (:3927) reverts CfL only when
   `best_uv_cost < cfl_uv_cost` — CfL WINS exact RD ties; the port's bd8 arm
   had `cfl < best` (non-CfL wins ties; the bd10 arm was already correct).
   Latent-documented since 2026-07-15 with no witness; the witness cell is
   CID22 5739122 q5 p0 at mi(31,80) 8x4 DC+filter-intra — both sides' terms
   byte-identical, RD collides at exactly 130518==130518, C codes CfL.
   **Takes the probe to 135/135.**

Method (the reusable chain, ~1 drill each): `tools/drill_cell.sh` ->
`decode-diff --first-block-diff` (NOTE: decoder block records ignore angle
deltas — cross-check `tree_diff` aduv/ady flips for the true first coded
flip) -> the new `NSQDBG CFLARB` per-candidate arbitration dump (leaf_funnel)
vs C `SVT_UVLOOP_OUT`/`SVT_UVLOOP_XY` -> `tools/uvdc_join.py` for
coding-invisible chroma-neighbour drift. Records:
`benchmarks/photo_p0_bd8_sortfix_2026-07-23_{before,after}.tsv` + `.meta`.

Residual scope after this landing: the deferred union-sort question for
multi-lane (palette/IBC) MDS1/MDS3 ordering — C concatenates per-class
sorted lists (`construct_best_sorted_arrays_md_stage_3`, :1454) where the
port re-sorts the union; photo has a single lane so it is inert here, but
screen-content cells with palette/IBC lanes should be re-checked against it.

## QP domain (C-exact since 2026-07-13)

`RcConfig.qp` is CLI-domain 0..63 exactly like C's `--qp`; the pipeline
maps it through the verbatim `quantizer_to_qindex[64]` port ONCE at frame
setup and every downstream consumer (quantizer tables, FH base_q_idx,
CDF q bucket, chroma quantization, deblock level picker) operates on the
qindex 0..255. Before the split the 0..63 value was consumed as qindex
directly, capping the reachable quantizer range at qindex 63 (top-quality
quarter) and keeping deblock levels <= 3. All matrices below use
CLI-domain qps.

## Arbitrary dimensions — chunk 1 (task #95, 2026-07-17)

Full-SB arbitrary dimensions land: the pipeline carries TWO dim systems —
TRUE (caller-passed, header/crop) and ALIGNED (round-up-to-8, the encode
grid). `encode_frame_420` edge-replicates the input planes TRUE->ALIGNED
(C `pad_input_picture`), the seq header carries TRUE
`max_frame_width/height_minus_1`, and the small-frame restoration disable
(`enc_settings.c:214-232`, true w|h < 64) is replicated. Scope: aligned
dims a multiple of 64 (dims {57..64} -> a single 64x64 SB, e.g. 60x60).

| Gate | Result |
|---|---|
| 60x60 uniform+gradient vs SvtAv1EncApp, presets 13/10/6 × q20/40/55 | **18/18 byte-identical** |
| default identity_matrix (64/128 full-SB + 60 arb-dims) | **54/54** |

## Arbitrary dimensions — chunk 2: PARTIAL SBs byte-match (task #95, 2026-07-18)

Partial superblocks (aligned NOT a mult of 64) AND ODD dimensions now byte-match
real C. `tools/partial_sb_gate.sh` = **101/101** (presets **6/7/8/9/10/13**, bd8
4:2:0; includes both-partial + straddle + odd dims): the 96x80
milestone (cmp-verified 878B) + full/straddle cells + **11 odd-dim cells** (65x64,
64x65, ...) + 6 bottom-edge/8-aligned-partial + 5 straddle-win. Full-SB identity
matrix stays **54/54**; bd10 36/36 + bd10-nonflat 8/8 untouched. Verified
PANIC-FREE incl. odd dims (484 cells dims×qp, all decodable). Landed pieces:
- **ODD dims** — harness ceiling chroma `(w+1)/2` both sides; LR true-dim search
  (`search_restoration_still`/`write_lr_for_sb` on TRUE luma / CEILING chroma,
  fixing the odd-height FH `lr_type` WIENER-vs-NONE divergence).
- **PD0 boundary-node cost fix** (the high-leverage root, `pd0.rs` +
  `context::partition_alike_split_cost`) — TWO real bugs pinned by a new
  `SVT_PD0COST` C `--wrap` interposer (harness, env-gated, C tree pristine):
  (1) rectangular tx-type rate returned 0 for non-square edge shapes (748 bits
  too cheap) — fixed via `TXSIZE_SQR_MAP`; (2) boundary split used the
  full-alphabet rate instead of C's binary `partition_{vert,horz}_alike` (cross-
  named). Unlocked all single-edge partial + the straddle-win cells at q≤32.

- **SB-extent padded variance** — `encode_input` padded TRUE->sb_ext
  (`frame_geom::pad_input_plane`, edge replication) at the sb_ext stride, so
  `compute_b64_variance`'s unclamped 64x64 walk reads C's replicated border.
- **Partition edge SEARCH** — a partial node is a DETERMINISTIC edge-shape
  decision (`set_blocks_to_test`: one shape injected, `md_disallow_nsq_search`),
  priced on the NON-SQUARE in-frame block (`pd0::lvl1_block_cost_rect`,
  `leaf_funnel::decide_leaf_rect` + tall-rect TX Tx32x64/16x32/8x16), NOT the
  square PART_N cropped nor forced-split. Off-frame quadrants = `Pd0Tree::Off`.
- **Partition edge CODING** — `encode_partition_av1` binary SPLIT-vs-{H,V} with
  the CROSS-named `partition_gather_{horz,vert}_alike` (see arb-dims-port-map),
  no-symbol forced split when both-false, single-child H/V pack arms.
- **Straddle boundary blocks** — C codes blocks that reach PAST aligned (the
  "leaves inside ALIGNED" assumption was false — even both-true nodes straddle,
  e.g. 48x56's 64-root); recon+chroma working buffers are sized to the sb_ext
  PRODUCT so straddling reads/writes never OOB. Verified PANIC-FREE: 240
  partial-SB cells (dims x qp) all decodable, 0 panics.

CROPPED-TX RD DISTORTION LANDED 2026-08-03 (task #95 (b)+(c)): C prices a
boundary TX block's SPATIAL distortion only over the part inside the ALIGNED
frame (`cropped_tx_width`/`_height`, product_coding_loop.c:4664-4665 and
:5752-5754; `cropped_tx_width_uv`/`_height_uv`, full_loop.c:2228-2232). The
already-written `frame_geom::cropped_tx_dims` (+ the new `_uv` sibling) is now
wired through `leaf_funnel::tx_unit` / `tx_unit_hbd` / `txt_search`. MEASURED
crop OFF->ON over 48 partial-SB cells: 8 changed bytes, 3 went DIFF->MATCH
(80x88 / 104x88 / 72x88 at q55 p6 — the straddle-WIN trio), 0 regressed;
partial_sb_gate 101 -> 104, identity_matrix 54/54, bd10_matrix 36/36 unchanged.

REMAINING (decodable-DIFF, documented in docs/arbitrary-dims-port-map.md, NOT
gated): the high-qp p7/p8 straddle/multi-SB cells (200x120 q40/55, 80x88 /
104x88 / 72x88 / 120x120 q55) — their bytes MOVED with the crop but still
diverge, so a separate root (candidate: the `end_tx_depth` frame-boundary
force-to-0, product_coding_loop.c:6712-6717, unported); a true sb_ext chroma
STRIDE (not just product slack); 65x65 odd-width (harness even-dim + DLF
floor-vs-ceiling chroma); the M9+ boundary edge-shape cost (wired on LVL_1
only). See CLAUDE.md #95.

## SB128 (128x128 superblocks) — selection rule + plumbing (task #91, 2026-07-19)

**The port now knows when C uses 128px superblocks; it cannot yet CODE one.**

C has no `super_block_size` config field — it DERIVES the value
(`Globals/enc_handle.c:4071-4111`). `sb128_geom::derive_super_block_size`
replays that rule branch for branch, so both encoders agree with no harness
flag. Unit-tested against the real encoder's emitted SH bit, read back with
`tools/sb128_seqhdr.py` — MEASURED, not transcribed:

| request | aligned px | preset | C `use_128x128_superblock` |
|---|---|---|---|
| 512x384 | 196,608 | 0 / 1 | **1** |
| 512x384 | 196,608 | 2 / 3 | 0 |
| 512x320 | 163,840 | 0 | 0 |
| 512x336 | 172,032 | 0 | **1** |
| 256x256 | 65,536 | 0 | 0 |

**Two clauses decide it, and both invalidate the obvious gate design:**
1. `input_resolution == INPUT_SIZE_240p_RANGE` forces 64 UNCONDITIONALLY —
   that bucket is aligned luma area `< 165,120` (`INPUT_SIZE_240p_TH`). So a
   128x128 / 192x192 / 256x256 cell can NEVER exercise SB128.
2. In allintra only `enc_mode <= ENC_M1` picks 128 — presets 2..13 are SB64
   at every size.

Every existing gate cell is under the area threshold, which is the only
reason SB64 has been correct so far. **SB128 is the DEFAULT for allintra
M0/M1 at any real image size.**

Landed (all byte-neutral at SB64 — every gate re-verified, see below):
- `sb128_geom::derive_super_block_size` + `SbSizeInputs` (the force-64 knobs:
  variance-boost — which the HDR fork defaults ON — resize, rtc, sframe,
  fast-decode).
- `EncodePipeline::{sb_size, derived_sb_size, sb_size_override,
  sb128_fallback}`; `SVTAV1_SB=64|128` pins it in `identity_run`.
- `SeqTools::use_128x128_superblock` -> the SH bit; SB-derived tile limits
  (`resolve_tile_rows_log2_sb`, `write_key_frame_header_full_lr_sb`) with the
  64px entry points kept as compat shims.
- **`EntropyCtx::bsl` 128 fix (a real latent bug):** a `_ => 3` catch-all
  folded 128 into the 64 level, capping `partition_ctx` at ctx 15 and making
  ctx 16..19 — the only rows carrying the 8-symbol 128 alphabet — dead code.
  A 128-wide node would have coded against the 64x64 CDF row with a
  10-symbol alphabet: wrong probabilities AND wrong alphabet length.

LANDED (2026-07-19): the SB128 encode path. `sb128_encode_supported()` now
returns `true`; a preset-0/1 cell above the 165,120-px area threshold encodes
as a real 128px SB (walk: `sb128_geom::sb_coding_units` + `merge_sb_units`).
**12 of 14** `sb128_gate.sh` cells byte-match real aomenc (gate 18/18 incl. the
4 SB64 controls). Why it was small: on an I_SLICE C clamps the MD scan's max
square to 64x64 regardless of SB size (`enc_dec_process.c:1483-1499`), so the
128 root is STRUCTURALLY always PARTITION_SPLIT — there is no 128-level
NONE/HORZ/VERT search on KEY. So coding the SH `use_128x128_superblock` bit +
the per-SB forced-SPLIT root over the (already-identical) 64-block groups is the
whole delta — exactly the pre-landing first-divergence analysis
(`identity_diff.sh 512 384 32 0 gradient`: `SH use_128x128_superblock C=1
Rust=0`, then a 128 root coded against the 8-symbol alphabet, CDF row 16).

Gate: `tools/sb128_gate.sh` — 14 sb128 cells (all >= 165,120 px, preset <= 1)
+ 4 SB64 controls; each asserts the ORACLE really emitted SB128 (anti-vacuity,
`sb128_seqhdr.py`) so a mis-sized cell fails loudly. The 2 remaining diverging
cells are pinned SELF-PROMOTING (a cell that starts matching FAILS the gate
until moved into `SB128_BYTE_EXACT`); both are leaf-cost RD near-ties that
reproduce at SB64 — NOT a 128-structural gap (`docs/sb128-port-map.md`). INTER
at SB128 is unported (`debug_assert`ed).

## Decode conformance (AV1 reference decoder)

`tools/decode_conformance.sh` — 525-stream mono matrix (gradient/uniform/
edges x 32..128 px x CLI qp {20,32,43,55,63} = qindex {80,128,172,220,
255} x speeds 2..10) plus a 700-stream 4:2:0 matrix
(`tools/decode_conformance.sh <dir> chroma`: same grid + a `color`
content whose chroma planes carry real patterns), every stream must
decode under **aomdec**:

| Gate | Result |
|---|---|
| 525/525 mono streams decode | **PASS** (was 0/525 before this wave) |
| 700/700 chroma-420 streams decode | **PASS** (new 2026-07-13; opt-in `with_chroma_420`) |

The old rav1d-based "decode PASS" claims were leniency artifacts; aomdec is
the gate now. **2026-07-18: the 4:2:0 gate gained palette-forcing `stripes`
content (1575/1575) after fixing a palette `filter_intra` desync (a0b505b4f)
that had held CI red — see CLAUDE.md.**

## 10-bit (bd10) encode — uniform, ALL presets (task #94, 2026-07-18)

`tools/bd10_matrix.sh` (also a CI gate): uniform {64,128} x qp{20,40,55} x
preset{0,2,3,6,10,13} encodes byte-identical to real aomenc at bit depth 10
(**36/36**) and decode under aomdec. Harness: `capture_c_trace <..> 10` (packed
u16 LE) + `identity_run SVTAV1_BD=10` + the pipeline's `with_bit_depth`. Three
frame-header chunks landed: the first cell (uniform, aa89a83be — the port stays
u8 because flat->skip makes the tile bit-depth-independent), the M6+
LF-level-from-Q bd10 derivation (be1ea0770), and the qp-fast-path CDEF
strength-from-Q bd10 derivation (885ece6da: `q = AC_QLOOKUP_10[qindex] >> 2`,
same f32 fit — proven C-exact for all 256 qindexes by the `c_parity_cdef_pick`
bd10 differential, and end-to-end by the gradient bd10 op-trace's first
divergence moving off the FH cdef line into the tile). The 5 bd10 DSP kernel
families are FFI-verified (see the differential-suites table).

## bd10 REAL-PHOTO p0–p3 byte-identity — CLOSED (task #94, 2026-07-23)

The last open photographic band at bd10. Root: C's candidate sorts
(`sort_fast_cost_based_candidates` :1415 / `sort_full_cost_based_candidates`
:1438) are UNSTABLE swap-on-`<` exchange sorts; the port's stable
`sort_by_key` diverged from them on EXACT cost ties (real-content flat
regions at q5 — two candidates predicting identically, or (rate,dist) pairs
colliding after the lambda fold). Two sites fixed in `leaf_funnel.rs`, both
bd10-gated (`bd10_rd.is_some()`; bd8 verbatim-unchanged): the post-MDS1
`order1` sort (WIP `70b26b6c6`, verified + A/B-re-proven this session —
closed `7062227 q5 p1/p2` + CLIC `02809272… q5 p2`; the prior session's
540-cell p0-3 sweep 537/540 → 540/540) and the MDS0 per-class `sort_lane`
(this session — closed `2119713 q5 p1`, a decode-IDENTICAL angle_delta tie
found OUTSIDE the swept images; op-trace localized to op 66843). The ind-uv
site had it already. Gates: `bd10_photo_gate.sh` +33 group-G cells (4 CID22
images × q{5,32} × p0-3 + a CLIC crop spec cell) = **187/187** (as of that
commit; 191 cells today); battery
green: identity 54/54, bd10_matrix 36/36, nonflat 309/309, partial_sb
101/101, sb128 20/20, tile 29/29, arbitrary 57/57, combos 40/40, panic
60/60, palette 50/50, ibc-fh PASS, ibc rc=0, nextest 915/915. Remaining
bd10 low-preset scope: p4 (13/15 probe; `2119713 q32`, `7062227 q5`) — see
docs/bd10-port-map.md REMAINING.

## bd10 NON-FLAT — first cells with a coded residual byte-match (task #94, 2026-07-18)

`tools/bd10_nonflat_gate.sh` (CI gate): `gradient 64x64 q40` at preset **10 and
13** byte-match real aomenc at bd10 (**2/2**) — the first non-flat bd10 cells.
Root cause of the prior tile divergence: the port quantized the residual with
the bd8 Q8 tables while C uses bd10 Q10 (~4xQ8 but NOT exactly) → different coded
levels. Fixed via an ADDITIVE, bd10-gated u16 re-encode (the "M4+ bypass_encdec
re-predict" shape — the u8 partition/mode/tx decisions are RD-scale-invariant for
`sample<<2` content, so only the bit-depth-sensitive coded luma LEVELS + true
10-bit recon are recomputed; NOT a full u8->u16 refactor). Pieces:
`quant::build_quant_table_bd` (Q10 + qzbin), `quant::quantize_fp_hbd` (**THE FIX**:
the INT16 clamp in `quantize_fp` is bd8-only — C dispatches bd>8 to
`highbd_quantize_fp_helper_c`, full_loop.c:367-395), `leaf_funnel::{predict_unit_hbd,
tx_unit_hbd}`, `pd0::kf_full_lambda_bd10` (EXACT C full_lambda_md[1], not ×16 of
bd8), a bd-aware inverse transform, and `pipeline::bd10_reencode_luma`. The u8 path
is byte-UNCHANGED (bd8 identity 54/54, bd10 uniform 36/36).

UPDATE (2026-07-19): the envelope below is SUPERSEDED — the "FOLLOW-UPS" listed
as unported are DONE. `dr_predict_hbd` (directional) and `predict_filter_intra_hbd`
(filter-intra) are ported and wired into the bd10 full-RD funnel, which now also
decides the chroma uv mode + CfL at 10 bits and runs the deblock-level full search
at 10 bits. `bd10_tree_supported` now falls back to u8 ONLY for `tx_depth > 0`
unconditionally (directional additionally when the SH edge filter is on). Current
byte-identity coverage: `bd10_matrix` 36/36, `bd10_nonflat_gate` (diag+gradient,
presets 0–13) 288, `bd10_photo_gate` (photographic, incl. preset 5) 154 (as of
2026-07-19; the gate is 191 cells today — groups G/H were added 2026-07-23),
`bd10_recon_parity_gate` 13. The remaining bd10 residuals are the p0–p3 luma
partition RD near-tie + `search_best_independent_uv_mode` (M0/M1) — see
docs/bd10-port-map.md. The 2026-07-18 note below is kept as the historical record.

ENVELOPE (narrow, honest): only the **DC-family / tx_depth-0 / rdoq-fp** subset is
ported. Out-of-envelope bd10 frames (directional or filter-intra intra, tx_depth>0,
rdoq level 0, non-uniform chroma) FALL BACK to the non-panicking u8 output via the
`bd10_tree_supported` gate — the encoder stays panic-free on the public
`encode_frame_420` API; the u16 predict/tx path panics loudly only where a
future-ported case would land, and the gate never lets it. FOLLOW-UPS (#94):
`dr_predict_hbd` (directional), `predict_filter_intra_hbd`, `quantize_b_hbd`
(rdoq-0, same INT16-clamp class), tx_depth>0 re-encode, the u16 chroma path, and
native (non-`<<2`) u16 ingestion. See docs/bd10-port-map.md.

NOTE (2026-07-18): the prior session's bd10 + palette-conformance work (10
commits, 58bd3b4c9..885ece6da) was committed+verified-green locally but **never
pushed to origin** — origin CI had been red since 2026-07-16 without the palette
`filter_intra` conformance fix. Recovered this session: pushed + origin-verified
(`merge-base --is-ancestor HEAD origin`), all gates green locally (workspace
tests, bd8 54/54, bd10 uniform 36/36, mono conformance 1260/1260, chroma
1575/1575).

## Bit-exact-vs-C differential suites (svtav1-cref harness)

All verified against the linked `libSvtAv1Enc.a` (v4.2.0-rc) on every test
run:

| Module | Verification |
|---|---|
| Range coder (`OdEcEnc`) | byte-identical streams: 30k update_cdf cases, 300 random static/adaptive streams, carry torture, tiny streams |
| `update_cdf` | bit-identical, all alphabet sizes 2–16 |
| Default CDF tables (13 coef families x 4 q-buckets + 12 mode families) | drift test re-extracts from C every run |
| Scan orders (19 x 3) | drift test vs `eb_av1_scan_orders` |
| Quantizer step tables | generated from `svt_aom_dc/ac_quant_qtx` |
| Coefficient writer helpers (level maps, nz/br ctx, eob tokens, txb dims) | fuzzed vs exported C impls |
| Deblocking kernels (`svt_aom_lpf_{h,v}_{4,6,8,14}_c`) + sharpness limits | bit-exact over all (level, sharpness) x content classes (c_parity_lpf) |
| CDEF kernels (`svt_cdef_filter_block_c` dst8, `svt_cdef_filter_block_8bit_c`, `svt_aom_cdef_find_dir{,_8bit}_c`) | bit-exact over all 64 signalable strengths x damping 2..=6 x dirs x 8x8/4x4 x frame-border sentinel patterns + randomized wide/torture (c_parity_cdef) |
| CDEF qp-strength picker (`svt_pick_cdef_from_qp` intra branch) | bit-exact for all 256 qindexes vs C float semantics (c_parity_cdef_pick) |
| **bd10** quant step tables (`svt_aom_dc/ac_quant_qtx` at `EB_TEN_BIT`) | all 256 qindexes DC+AC vs real C (c_parity_bd10_quant) — #94 |
| **bd10** loop filters (`svt_aom_highbd_lpf_{h,v}_{4,6,8,14}_c`) | bit-exact at bd10+bd12 over all (level, sharpness) x content (c_parity_lpf_hbd) — #94 |
| **bd10** distortion/variance/SAD (`svt_full_distortion_kernel16_bits_c`, `svt_aom_variance_highbd_c`, `svt_aom_sad_16b_kernel_c`) | bit-exact at bd10+bd12 over 14 block shapes, strided (c_parity_hbd_distortion) — #94 |
| **bd10** intra predictors (sized `svt_aom_highbd_*_predictor_WxH_c`) | bit-exact at bd10+bd12: 10 modes (DC×4 / V / H / Paeth / Smooth×3) × 19 sizes, 7600 preds (c_parity_intra_pred_hbd) — #94 |

v4.2.0-rc note: upstream refactored the coder internals (borrowed buffer,
ptr walk) — output verified still byte-identical; `coeff_br_cdf` dropped its
dead 64x64 slice (tables regenerated).

## Pixel-path status (decoded output correctness) — CORRECT

All probes decode via aomdec and compare against the source. (The
q labels below are the EFFECTIVE QINDEXES the historical runs measured
at — they predate the CLI-qp/qindex split, when RcConfig.qp was consumed
as qindex directly; to reproduce "qindex30" today pass CLI qp 30/4 ≈ 8,
or call the block APIs with qindex 30 directly.)

| Probe | Result |
|---|---|
| uniform-128, flat-140, flat-250 | **bit-exact** |
| edges 64px qindex30 s2 / 96px q50 s4 | **LOSSLESS** (205/367 bytes; C reference also lossless at 172 bytes — remaining delta is RD tuning) |
| gradient 64px qindex30 s4 | **46.76 dB** |
| gradient 128px q50 s8 | 30.39 dB |
| 420 probe 64px q30 (examples/probe_420) | Y 46.64 / U 52.97 / V lossless |
| 420 probe 128px q30 | Y 46.03 / U 51.92 / V 52.86 dB |
| 420 probe 128px q50 | Y 30.39 / U 55.98 / V 57.44 dB (Y == mono ref) |

Fixed en route: live-recon prediction neighbors, real mode/tx-type
signaling, AV1 quantizer tables + decoder-mirrored dequant, per-size
forward cos bits, restored inverse stage-range clamps, C-exact intra edge
fill (127/129/left[0]/above[0] rules), 64-dim coefficient zeroing.
Deblocking is now SIGNALED and applied decoder-exactly (2026-07-13): key
frames carry the q-picked loop_filter levels and the recon-parity gate
holds 216/0 with filtering live. CDEF likewise (2026-07-13): SH
enable_cdef=1, FH cdef_params (cdef_bits=0, qp-picked strengths — C's
use_qp_strength closed form, NOT the RDO search yet), decoder-exact
av1_cdef_frame pass after deblock on the output copy; recon-parity 216/0
with CDEF firing on 168/216 streams (2.34M px filtered, 882k changed;
per-64x64 cdef_idx costs zero EC bits at cdef_bits=0). Restoration stays
disabled+unsignaled. At real high qindexes deblock returns material
levels ([61,61,30,30] at qindex 220, [63,63,60,60] at 255;
examples/deblock_evidence) and CDEF signals y=17/43/63 at qindex
172/220/255, improving gradient content +0.25/+0.50/+0.31 dB and ringing
edges +0.16 dB at 255 with parity exact (examples/cdef_evidence).
The qindex split also exposed + fixed a latent VERT_A/VERT_B bug: their
children now use the C has_tr_vert_*/has_bl_vert_* availability tables
(the search emits ext partitions at preset <= 8; passing the generic
tables coded D-mode children against above-right pixels the decoder
doesn't have yet — recon-parity 211/5 -> 216/0 at qindex {80,172,255}).
Per-SB QP offsets stay disabled until delta_q signaling is ported.

## Known failing test

(none — `multi_frame_bitstream_sizes_decrease` passes again since the
unsignaled loop filters were disabled: the filtered DPB recon had been
corrupting inter references, which was the real reason inter frames
outweighed the key frame. Workspace fully green.)

## Architecture direction

> **STATUS 2026-08-28 — the four items below have all landed; this section is
> kept as the original plan.** (1) chroma 4:2:0 with C decision parity, (2)
> deblock/CDEF/Wiener searches + application, (3) directional-mode edge
> extension, and (4) decision-layer parity are what the CI gate tables in
> `../README.md` measure (every preset 0–13, 8- and 10-bit, tiles, SB128,
> partial SBs, superres). The current backlog is `docs/REFUSED-CONFIGS.md`
> (capability refusals) and the open GitHub issues, not this list.

Module-by-module faithful port of C SVT-AV1 behind `svtav1-cref`
differential harnesses (see `docs/PORT-coeff-writer.md` for the worked
example). Bitstream writer layer (headers, tile groups, coefficient coding)
is now C-exact at the writer level; decision layers (partition/mode RDO,
filters, chroma) still ours and next in line:

1. Chroma 4:2:0 end-to-end — **landed 2026-07-13 (opt-in
   `with_chroma_420` + `encode_frame_420`; still-frame, UV_DC-only,
   min-8x8 luma policy — see CLAUDE.md gap 1a-1d for what remains
   toward C decision parity)**
2. Filter search + signaling ports — deblocking landed 2026-07-13
   (C-exact kernels + q-based level picker + decoder-exact frame walk,
   signaled in the FH; SSE-based level search and inter-frame levels still
   pending); CDEF landed 2026-07-13 (C-exact kernels + C's qp-strength
   fast path at cdef_bits=0 + decoder-exact av1_cdef_frame application;
   the C-default per-fb RDO search moves to the decision-parity wave);
   restoration next
3. Directional-mode edge extension (has_top_right/bottom_left)
4. Decision-layer parity vs C (partition/mode/TX RDO), then per-preset
   bitstream identity gates (see COVERAGE.md for the config-surface
   scoreboard: 121 fields auto-derived from EbSvtAv1EncConfiguration)

## Crate structure

```
rust/
  crates/svtav1-types          Core AV1 types, enums, constants + src/tables/ const lookup tables
  crates/svtav1-dsp            Transforms, prediction, filters, quant (+ generated quant tables)
  crates/svtav1-encoder        Pipeline, partition, mode decision, RC
                               + src/entropy/: range coder, CDFs, OBU, coefficient coding (+ generated CDF/scan tables)
  crates/svtav1-cref           Test-only FFI harness over libSvtAv1Enc.a (the differential oracle)
  svtav1                       Public API, AVIF backend
```

C reference builds required for tests: built BY CARGO since 2026-08-27
(`crates/svtav1-cref/build.rs`, issue #4) — the first `cargo test` / `cargo
build -p zenav1-svt-cref` configures and builds `Bin/Release` (mainline) and
`Bin/ReleaseHdr` (fork) from the `reference/svt-av1` submodule, SHA-stamped.
The hand-typed equivalent, should you need it: `cmake -S reference/svt-av1 -B
cbuild-static -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_OUTPUT_DIRECTORY="$PWD/Bin/Release/"
-DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=ON -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF
-DNATIVE=OFF && cmake --build cbuild-static`.
