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

## Addendum, same day: the full costs, and where the two sides part

`SVT_FULLCOST_OUT` with `SVT_FULLCOST_XY=64,32` against the port's
`NSQDBG CAND`, at that block. C's lines carry their MD STAGE (`st=`); the
port's do not, so the two are aligned by the one value that is identical on
both sides.

C, the two competing inter candidates:

```
CFULL org=(64,32) 16x32 st=1 mode=14 ycb=2792 ydist=160867 cost=27427295   <- NEARMV
CFULL org=(64,32) 16x32 st=1 mode=16 ycb=2362 ydist= 95239 cost=36661249   <- NEWMV
CFULL org=(64,32) 16x32 st=3 mode=14 ycb=2792 ydist=143296 cost=25223692   <- NEARMV, MDS3
```

port:

```
NSQDBG CAND mi=(8,16) 16x32 ci=30 flr=2845 coeff_rate=161 dist=143296 full=25178207  <- NEARMV
NSQDBG CAND mi=(8,16) 16x32 ci=33 flr=6774 coeff_rate=161 dist= 38192 full=20660324  <- NEWMV, and it WINS
```

Three things this pins.

1. **The port's NEARMV is C's, to the unit.** `dist = 143296` equals C's MDS3
   `ydist = 143296` exactly, and the full costs are 25 178 207 against
   25 223 692 — 0.18 % apart. The distortion pipeline, the rate and the lambda
   all agree on that candidate.
2. **C DROPS NEWMV AT MDS1.** There is no `st=3 mode=16` line: at MDS1 NEARMV
   is 27 427 295 against NEWMV's 36 661 249, so NEWMV never advances. C's
   choice is made a stage earlier than the port's.
3. **The port carries NEWMV to its final stage and it wins there**, at
   `dist = 38192`. Whether the port's MDS1 distortion for that candidate is
   C's 95 239 is **NOT measured** — the port's `CAND` line carries no stage,
   and its NEARMV value aligns with C's MDS3, so 38192 is an MDS3-domain
   number and 95239 an MDS1-domain one. **Comparing them directly would be the
   third wrong reading of this cell.**

So the next probe is the port's MDS1 stage specifically: if its MDS1 NEWMV
distortion is C's 95 239, the candidate is dropped there and the cell closes;
if it is not, the divergence is in the MDS1 distortion of a NEWMV candidate
whose NEARMV sibling is already exact — a much narrower target than "the cost
model".

Both MVs are full-pel — `(32,8)` is (4,1) px and `(24,0)` is (3,0) px — on a
`diag` sequence translated by exactly 3 px, so the TRUE motion is `(24,0)` and
BOTH encoders measure it as the worse-distortion candidate at this block. That
is the right-edge partial superblock: only 8 of the 16 columns are in frame, so
the replicated margin dominates the SAD. It is not a defect on either side, and
it is why this block is the one that keeps diverging.

## Second addendum, same day: MDS1 IS exact — the divergence is the post-MDS1 PRUNE

The addendum above said "whether the port's MDS1 distortion is C's 95 239 is
NOT measured". It is now, and the port already had the instrument: the funnel
emits `NSQDBG PMDS1` per candidate at MDS1, beside the `CAND` line it emits at
MDS3. Reading it at that block, against C's `st=1` rows:

| candidate | C `ydist` / `cost` (st=1) | port `dist` / `full` (PMDS1) |
|---|---|---|
| intra mode 4, delta +3 | 193982 / 42 731 002 | 193982 / **42 731 002** |
| intra mode 6, delta -3 | 195675 / 43 391 877 | 195675 / **43 391 877** |
| intra mode 4, delta 0 | 163494 / 31 020 555 | 163494 / **31 020 555** |
| NEARMV | 160867 / 27 427 295 | 160867 / **27 427 295** |
| NEARESTMV | 160867 / 32 230 458 | 160867 / **32 230 458** |
| NEWMV | 95239 / 36 661 249 | 95239 / **36 552 086** |

**Five of six agree to the UNIT — distortion, rate and lambda.** The sixth
agrees on distortion exactly and differs on cost by 109 163, i.e. 0.30 %.

And on BOTH sides NEARMV wins at MDS1 (27 427 295 against NEWMV's ~36.6 M). So
the port's MDS1 does not pick NEWMV either.

### What actually differs: how many candidates reach MDS3

C's `st=3` rows at that block: **two** — the intra mode 4 and NEARMV. There is
no `st=3 mode=16`.

The port's `NSQDBG CAND` rows (its MDS3 dump): **three** — `ci=11` (intra),
`ci=30` (NEARMV, `dist=143296 full=25178207`, matching C's MDS3 to 0.18 %) and
`ci=33` (NEWMV, `dist=38192 full=20660324`), which wins.

**C's post-MDS1 pruning DROPS NEWMV and the port's keeps it.** That is
`nic::stage_mds1_to_mds3` — C's `sort_full_cost_based_candidates` +
`post_mds1_nic_pruning` + `post_mds2_nic_pruning`. The MDS3 distortion of that
candidate collapses from 95 239 to 38 192 once the real transform and RDOQ run,
which is why keeping it is decisive: C never computes that number.

### The target, stated as narrowly as the evidence allows

Not the motion search (both find `(32,8)`), not the candidate set (both have
both modes), not the MDS0 rate (2845 / 6774 on both), not the MDS1 cost (exact
on five of six candidates). **The number of candidates the port's post-MDS1 NIC
pruning admits to MDS3, at this block, is one more than C's** — and the
0.30 % cost gap on the very candidate that survives is the obvious first thing
to check, since NIC pruning thresholds are relative to the class best.

This is the same shape as `video_key_matrix.sh`'s two unmoved cells, where the
recorded state is already "MDS1 is exact and the divergence moved to MDS3". The
two are now one target, not two.
