# `diag 72x72 q40 p6` frame 1 after the per-SB lambda: the SHAPE closed, a MODE did not

`docs/INTER-ENCODE-PLAN.md` §1z²² localized the residual 72x72 cells to an
edge-shape DEPTH — C coded two `BLOCK_16X32` where the port coded one
`BLOCK_32X64` — and predicted from §1z²³'s numbers that the per-superblock MD
lambda was the cause. §1z²⁴ wired it. **The prediction was right and the shape
is now C's exactly**, so this file records what is left, because the next
chunk on this cell must not open with the old hypothesis.

Cell: `diag 72x72 q40 p6`, `frames=2`, `SVTAV1_FRAME_SHIFT=3`, low-delay P.
C oracle `reference/svt-av1 @ fix/suspected-c-bug-17` through
`tools/ctrace-linux/run.sh` (linux/arm64 container). Port at §1z²⁴.
C frame 1 is **28 B**, the port **29 B** (it was 27 before §1z²⁴, i.e. the
direction flipped as the missing block arrived).

## The five coded inter blocks, side by side

`mi` is (row, col) in 4-px units; `bsize` is C's `BlockSize` index
(definitions.h:904 — 3 = `BLOCK_8X8`, 7 = `BLOCK_16X32`, 11 = `BLOCK_64X32`,
12 = `BLOCK_64X64`) and `mode` C's `PredictionMode` (definitions.h:1187 —
13 = `NEARESTMV`, 14 = `NEARMV`, 16 = `NEWMV`). C from `SVT_CINTER_OUT`, port from
`SVTAV1_PACKTREE`'s `PDV` line (frame-1 entries only).

| mi | C bsize / part | C mode | C mv | port mode | port mv | agree? |
|---|---|---|---|---|---|---|
| (0,0)   | 12 / 0 (64x64, NONE) | 16 NEWMV | (0,-24) | 16 | (0,-24) | yes |
| (0,16)  | 7 / 2 (16x32, VERT)  | 16 NEWMV | (24,0)  | 16 | (24,0)  | yes |
| (8,16)  | 7 / 2 (16x32, VERT)  | **14 NEARMV** | (24,0) | **16 NEWMV** | (24,0) | **NO** |
| (16,0)  | 11 / 1 (64x32, HORZ) | 13 NEARESTMV | (0,-24) | 13 | (0,-24) | yes |
| (16,16) | 3 / 0 (8x8, NONE)    | 13 NEARESTMV | (0,-24) | 13 | (0,-24) | yes |

Before §1z²⁴ the `mi=(8,16)` row was **absent on the port side** — the port
coded ONE `BLOCK_32X64` covering both halves of the 8-px-wide right edge.
Now the block exists, with C's shape and C's motion vector, and the only
difference on the whole frame is which MODE spends it.

## What that rules in and out

* **NOT the partition cost model, and not the spec-5.11.4 edge predicate.**
  Both edge nodes are now taken at C's depth with C's `PARTITION_VERT`.
* **NOT the motion search.** The MV is `(24,0)` on both sides, exactly.
* **The candidate/MVP lane.** C's line carries `pmv0=0,0`, `drl=0`,
  `drlctx=0,0`, `drlnear=-1,-1`, `npr=2`, `ovl=2`; NEARMV at that block
  predicts `(24,0)` for free from the ref-MV stack, where NEWMV pays the MV
  difference. Either the port's stack does not offer `(24,0)` at the NEARMV
  position for this block, or it does and the two candidates are priced so
  that NEWMV wins.

The join to build next is C's `SVT_INJCFG_OUT` / `SVT_IFCOST_OUT` at
`mi=(8,16)` against the port's `SVTAV1_CANDDBG` — the same pair that found
the inverted `intra_inter_context` in §1z¹⁷. Note the interposer trap in
`docs/WORKING-ON-THIS.md` §5: `SVT_INJCFG_OUT`'s `PMEST` per-reference fields
belong to whatever block MD searched last, so only the `CINTER` half and the
injector-config half of that dump are sound at a named block.
