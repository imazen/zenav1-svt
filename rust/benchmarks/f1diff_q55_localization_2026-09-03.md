# The three residual F1DIFF cells are a COST comparison, not a search — and one module header was stale

`diag 72x72 q55 p6` is one of the three cells `inter_byte_gate.sh` still lists
as open (port 31 B against C's 29 on frame 1). This file records where it
actually diverges, because two plausible readings of it are both wrong.

## Both sides code the SAME SIX BLOCKS at the same positions

C `SVT_CINTER_OUT` against the port's `SVTAV1_PACKTREE` `PDV`, frame 1:

| mi | C bsize/part | C mode | C mv | port mode | port mv |
|---|---|---|---|---|---|
| (0,0) | 12 / 0 | 16 | (0,-24) | 16 | (0,-24) |
| (0,16) | 7 / 2 | 16 | (24,0) | 16 | (24,0) |
| **(8,16)** | 7 / 2 | **14 NEARMV** | **(24,0)** | **16 NEWMV** | **(32,8)** |
| (16,0) | 8 / 1 | 13 | (0,-24) | 13 | (0,-24) |
| (16,8) | 8 / 1 | 13 | (0,-24) | 13 | (0,-24) |
| (16,16) | 3 / 0 | 13 | (0,-24) | 13 | (0,-24) |

**The partition tree is C's exactly.** One block of six differs, and it is the
same `mi=(8,16)` right-edge NSQ block the q40 cell diverged on before §1z²⁶.

## Reading 1, WRONG: "the port never injects NEARMV there"

It does, since §1z²⁶. `SVTAV1_CANDDBG` at that block:

```
mode=13 rf=1 mv0=0,-24  flr=2520
mode=14 rf=1 mv0=24,0   flr=2845   <- C's coded candidate, C's exact MV
mode=13 rf=5 mv0=24,0   flr=4957
mode=14 rf=5 mv0=0,-24  flr=5699
mode=16 rf=1 mv0=32,8   flr=6774   <- what the port coded
```

C's own candidate is present, at the rate the q40 join measured C pricing it
at (2845). The port simply picks the other one.

## Reading 2, ALSO WRONG: "`md_nsq_motion_search` is unported, so the NSQ MV differs"

`inter_md_arm`'s module header said `md_nsq_motion_search` is "PORTED but NOT
CALLED here ... so an NSQ block here takes the square path: `raw_me_mv * 8`,
then sub-pel", and measured that as 94 of 259 coded inter blocks on the
then-55 F1DIFF cells. **The second half of that is stale**: it is not called
in THAT module, but `inter_search_arm` builds its MVC list
(`nsq_sub_block_mvs`) and passes it into `refine_me_mv_for_ref` under
`b_w_ne_h && cfg.md_nsq_me_enabled`. The NSQ search runs.

And C's own dump settles it from the other side. `SVT_SUBPEL_OUT` — which
wraps the EXPORTED `svt_av1_find_best_sub_pixel_tree_pruned` and fires per
(block, list, ref, stage), the join point `docs/WORKING-ON-THIS.md` §5
recommends — at that block:

```
SUBPEL stage=0 org=(64,32) bsize=7 bw=16 bh=32 li=0 ri=0
       start=(32,8) best=(32,8) err=11043 refmv=(24,0)
       fpme=(32,8) subme=(24,0) fpdist=11043 mvpn=2 bestidx=1 bestdist=9895
       mvp=(0,-24),(24,0)
```

**C's list-0 ME MV at that block is `(32,8)` — the port's value exactly.** C
does not code it because NEARMV's cost wins, not because its search found
something else. `md_nsq_me_ctrls.enabled` is confirmed ON at preset 6 from C's
own `SVT_INJCFG_OUT` (`nsqme=1`), so the search really did run on both sides
and really did agree.

## What that leaves

The residual is a **cost comparison at one block**: two candidates both
present on both sides, priced so that C keeps NEARMV `(24,0)` (MDS0 rate 2845)
and the port keeps NEWMV `(32,8)` (MDS0 rate 6774, so it must be winning on
DISTORTION). That is the same class as `video_key_matrix.sh`'s two unmoved
cells, where MDS1 is exact and the divergence sits in MDS3.

**The instrument for the next chunk is `SVT_FULLCOST_OUT`** — C's per-candidate
FULL cost with `SVT_FULLCOST_XY=64,32` — joined against the port's funnel
`NSQDBG CAND` line. `flr` is MDS0 rate only; it cannot say which of distortion,
lambda or the later stages flipped the winner, and guessing between them is
exactly what the two wrong readings above did.

## The correction this makes

`inter_md_arm`'s header now says the search IS wired and records that C's own
`SVT_SUBPEL_OUT` agrees with the port's ME MV at the one block that still
diverges on this cell. The 94-of-259 census it quotes was taken on 2026-09-02,
before the search was wired and before §1z²⁶; it is kept, dated, and marked as
a measurement of a state that no longer holds.
