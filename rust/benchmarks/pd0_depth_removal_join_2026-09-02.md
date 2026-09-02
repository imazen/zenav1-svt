# The port's depth-removal inputs vs C's, per superblock — two divergences on
# the cells the crash fixes just unblocked; ONE FIXED, one still open

`gradient 168x168 q32 p8`, `SVTAV1_FRAME_SHIFT=3`, 2 frames, low-delay P flat
GOP, **frame 1** (the inter frame). Host r7900x (x86-64 Linux — `-Wl,--wrap`
is rejected by Apple ld64, so none of this can be measured on the mac).
Port at `main` 210d8dd6; oracle `reference/svt-av1` @ `fix/suspected-c-bug-17`.

C: `SVT_PD0CFG_OUT`'s `dr=` / `fastlam=` / `med=` / `mev=` fields, which are
`ctx->depth_removal_ctrls`, `ctx->fast_lambda_md[EB_8_BIT_MD]`,
`ppcs->me_{64,32,16,8}x*_distortion[sb_index]` and
`ppcs->me_8x8_cost_variance[sb_index]`. All are indexed by `sb_index`
explicitly, so `docs/WORKING-ON-THIS.md` §5's "an interposer reads the context
at its own call site" trap does not apply to them.
Port: `SVTAV1_PD0DBG`'s `PD0DR` line, built as the twin of that field set.

| org (x,y) | C `dr` | port `dr` | C `fastlam` | port `fastlam` | C `med` (64/32/16/8) | port `med` | `mev` |
|---|---|---|---|---|---|---|---|
| (0,0)     | 1/0/1/1 | 1/0/1/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (64,0)    | 1/0/1/1 | 1/0/1/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (128,0)   | **1/0/0/0** | **1/0/0/1** | **4878** | 4163 | **36736/35776/32640/23584** | **3332/3244/2960/2139** | 110342 = 110342 |
| (0,64)    | 1/0/1/1 | 1/0/1/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (64,64)   | 1/0/1/1 | 1/0/1/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (128,64)  | **1/0/0/0** | **1/0/0/1** | **4878** | 4163 | **37990/37990/35699/25299** | **3445/3445/3238/2294** | 125504 = 125504 |
| (0,128)   | 1/0/0/1 | 1/0/0/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (64,128)  | 1/0/0/1 | 1/0/0/1 | 3251 | 4163 | 0/0/0/0 | 0/0/0/0 | 0 = 0 |
| (128,128) | **1/0/0/0** | **1/0/0/1** | **4878** | 4163 | **52326/51640/47933/35553** | **2966/2927/2717/2015** | 97383 = 97383 |

`dr` is `enabled/disallow_below_64x64/disallow_below_32x32/disallow_below_16x16`.
The `port` columns above are the state BEFORE the fix in "Divergence 2"; after
it, every `dr` and every `med` in this table matches C, and only `fastlam`
still differs.

## Divergence 1 (STILL OPEN): `fast_lambda_md` is PER-SUPERBLOCK in C and per-FRAME here

C reports **3251** on the six superblocks whose ME distortion is zero and
**4878** on the three right-column ones (x=128), *within the same frame*. The
port reports a flat **4163** everywhere, because `pipeline.rs` builds one
`LambdaContext` per frame and calls `compute_fast_lambda` once
(`pd0_min_sq`'s derivation). `update_lambda` (rc_process.c:404) has a
per-SB arm — its `stats_based` block reads `ctx->sb_ptr->qindex` against the
picture qindex — so a frame-level value cannot be right on both sets.

This is not a rounding gap: 4163 x 150 >> 7 = 4878 exactly, and 150 is
`RD_FRAME_TYPE_FACTOR[0][ArfUpdate]` while 128 (the identity) is
`RD_FRAME_TYPE_FACTOR[1][ArfUpdate]` — so a wrong `hbd` row, a wrong
`gf_update_type` index, or a `pcs->lambda_weight` of 128-vs-150 all produce
exactly this ratio and all are one line. The 3251 arm is a separate question:
whatever selects it is not modelled at all.

`fast_lambda` is the lambda in every `RDCOST(fast_lambda, cost_th_rate, ...)`
threshold `set_depth_removal_level_controls` compares against
(`enc_mode_config.c:3018-3034`), so a wrong value moves all three
`disallow_below_*` decisions at once.

**WHICH superblocks get 4878 is the clue to hand the next chunk, and it is not
what the completeness flag would predict.** C's 4878 lands on org=(128,0),
(128,64) and (128,128) — the whole x=128 COLUMN, i.e. every superblock whose
cropped WIDTH is 40 — and 3251 on the other six, including (0,128) and
(64,128), which are equally partial in HEIGHT and have `is_complete_b64 = 0`
in the same dump. So it tracks neither `is_complete_b64` nor partial-ness in
general. A lambda has no business depending on superblock dimensions at all,
which suggests the varying input is `ctx->sb_ptr->qindex` — `update_lambda`'s
only genuinely per-SB input (rc_process.c:404) — where `pipeline.rs` passes a
flat `base_qindex` into `compute_fast_lambda` even though it already carries a
per-SB `sb_qindex` for `pd0_pick_sb_partition_video`. That is a hypothesis
from a correlation, NOT a measurement: check it by dumping
`ctx->sb_ptr->qindex` before acting on it.

Note also that with divergence 2 fixed, every `dr=` outcome on this cell now
agrees with C *despite* this lambda still being wrong — so a cell that only
compares `dr=` cannot witness it. It needs a direct join on `fastlam`, or a
cell whose thresholds sit near a boundary.

## Divergence 2 (FIXED): the ME DISTORTIONS disagreed by 11-18x, while the COST VARIANCE was exact

`me_8x8_cost_variance` matches C **exactly on all nine superblocks**
(0, 0, 110342, 0, 0, 125504, 0, 0, 97383). `me_{64,32,16,8}x*_distortion` does
not, on exactly the three superblocks where it is non-zero: 11.0x at (128,0),
11.0x at (128,64), 17.6x at (128,128). Not a constant factor, and the two
40x64 superblocks share a ratio the 40x40 one does not — which points at an
AREA or a clipping term rather than a scale constant.

That two statistics out of the same open-loop search disagree this way is the
useful part, and it narrows the search a long way. Both come from ONE array —
`me_ctx->me_distortion[21..85]` — in one function:

* `me_8x8_cost_variance = sum((d[i] - mean)^2) / 64`
* `me_8x8_distortion    = (sum(d[i]) * 4096) / pix_num`

**FOUND AND FIXED, 2026-09-02.** The variance is the clue but not for the
reason first written here: the two statistics are not both normalised.
`me_8x8_cost_variance` is computed from the RAW `me_distortion[]` array and
`me_*_distortion` is that array's sum times `4096 / pix_num`, where
`pix_num = b64_geom->width * b64_geom->height`. **The variance was untouched
because the defect was ENTIRELY in `pix_num`.**

`inter_me_arm::run_frame_me` built ONE `MePicParams` for the frame and set
`b64_geom_width = p.width`, `b64_geom_height = p.height` — the whole
PICTURE's dims — for every b64. C's `b64_geom[i]` dims are the CROPPED
per-superblock extent, `MIN(picture_dim - org, 64)` (pcs.c:1507-1508). So on
`gradient 168x168` the port divided by 28224 on every superblock where C
divides by 4096, 2560 or 1600.

That predicts the observed ratios exactly:
`(4096/1600) / (4096/28224) = 17.64` at the 40x40 corner and
`(4096/2560) / (4096/28224) = 11.025` at the two 40x64 edges — measured 17.64,
11.02 and 11.03. On a COMPLETE superblock C's `pix_num` is 4096 and the port's
was 28224, a 6.89x error that was invisible only because every complete
superblock's distortion on this grid is zero.

AFTER (`SVTAV1_PD0DBG`, same cell, same run shape): **all nine superblocks'
`med=` and `dr=` now equal C's**, and `min_sq` at the three right-column ones
goes 16 -> 8, which is C's. Gated by
`inter_me_arm::tests::a_partial_superblocks_distortions_are_normalised_by_its_own_cropped_extent`,
which pins C's numbers and fails on the old code with
`[3332, 3244, 2960, 2139]` against `[36736, 35776, 32640, 23584]`.

**`compute_distortion` itself was NOT the bug**, and it was checked first
because it is where a reader looks: the port's (`inter_me/candidates.rs:401`)
is a line-for-line transcription of C's (`motion_estimation.c:2739`), the
`pix_num` division included. The defect was in what its CALLER put in that
field — one struct literal, three superblocks, and no byte anywhere moved.

**THE FIX IS BYTE-INERT ON EVERYTHING MEASURED**, which is why it has no
`regression_spotcheck.sh` cell (§3: a cell must have failed before and passed
after, and no byte comparison did): inter byte gate 55 required / 0 failed,
the completion grid's 5 identical cells unchanged, `identity_full_8bit`
1100/1100, `video_key_matrix` 58/60, and the four 40-remainder cells emit the
same frame-1 bytes before and after (168 p8: 35 B both ways against C's 38).
It is a correctness fix whose effect is upstream of anything the current
partition path lets reach the bitstream.

**The blast radius is bounded structurally, not just by measurement:**
`b64_geom_width` and `b64_geom_height` have exactly ONE consumer in the whole
ME module — `pix_num` in `compute_distortion` (`grep` says so). No search
stage, no SAD, no MV reads them. So the fix can move the four
`me_*_distortion` outputs and nothing else, and those feed only
`set_depth_removal_level_controls` and `pd0_detector`'s thresholds.

**One LATENT divergence in that function, found while checking it and not the
cause here:** C computes `(dist_64x64 * b64_size) / pix_num` in `uint32_t`,
where `dist * 4096` OVERFLOWS above `dist == 1_048_576`; the port promotes to
`u64` first and cannot wrap. The sums are already re-narrowed to `u32` with a
comment saying why, so the intent was right and the product was missed. Not
reachable at these magnitudes (dist_64x64 is ~20439 here), and C is the oracle
bugs included, so this is a defect to FIX toward C's wrap, not away from it.

`docs/WORKING-ON-THIS.md` §5 records that on the campaign's 96-cell grid
"every superblock of every cell measured reports `me_*_distortion = 0` on C's
side". That is true of the six interior superblocks here and FALSE of the
three partial ones — so the partial-SB cells are the first cells in this
campaign where those statistics are observable at all, and they are the cells
the crash fixes just unblocked.

## The consequence of divergence 2, and why nothing saw it before

C's `disallow_below_16x16` is **0** at all three x=128 superblocks; the port's
is **1**. So C's `min_sq_size` there is 8 and the port's is 16, and C's PD0
descends to an 8x8 block at mi(40,40) = pixel (160,160) where the port cannot.
Measured directly with a new `SVT_PICKPART0_OUT` interposer on
`svt_aom_pick_partition_pd0` (this commit), which dumps each PD0 node's
`partition` and `rdc.valid`:

```
PICKPART0 poc=1 islice=0 mi=(32,32) bsize=12 partition=3 valid=1 rd=12620084
PICKPART0 poc=1 islice=0 mi=(40,40) bsize=9  partition=3 valid=1 rd=1082758
PICKPART0 poc=1 islice=0 mi=(40,40) bsize=6  partition=3 valid=1 rd=1082758
PICKPART0 poc=1 islice=0 mi=(40,40) bsize=3  partition=0 valid=1 rd=1082758
```
(bsize 12/9/6/3 = 64x64 / 32x32 / 16x16 / 8x8; partition 3 = PARTITION_SPLIT,
0 = PARTITION_NONE.)

**Nothing in this repo could see any of it until 2026-09-02**, because the
port PANICKED on this cell: with `min_sq` 16, `Pd0Ctx::pick_q` force-split the
both-false 16x16 at (160,160) and walked below `min_sq` into a node with no
cost. The panic is fixed (commit 4ae1ffb6) and that fix is correct C modelling
on its own terms — C really does treat such a node as invalid — but on THIS
cell C never reaches the case, because its `min_sq` is 8. The crash was the
visible end of a chain whose first link is above.

## For the next chunk

Divergence 2 is FIXED (see above). **Divergence 1 — the per-superblock
`fast_lambda` — is still open**, and with the distortions corrected its effect
is now measurable in isolation, which is why the fix order was 2 then 1: the
port still reports a flat 4163 on all nine superblocks where C reports 3251 on
six and 4878 on three. It happens not to move any `dr=` outcome on THIS cell
(all nine now agree despite it), so it needs a cell where a threshold is near
a boundary, or a direct join. `SVT_PICKPART0_OUT` then says whether the PD0
tree follows.
