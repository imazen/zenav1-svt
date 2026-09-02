# The port's depth-removal inputs vs C's, per superblock — two live divergences
# on the cells the crash fixes just unblocked

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

## Divergence 1: `fast_lambda_md` is PER-SUPERBLOCK in C and per-FRAME here

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

## Divergence 2: the ME DISTORTIONS disagree by ~11-18x, while the COST VARIANCE is exact

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

**Variance is invariant under adding a constant and NOT under scaling.** The
variance matches C to the digit on all nine superblocks while the sum is
11-18x off, so the port's per-8x8 distortions are C's SHIFTED, not C's SCALED.
Working back through the normalisation at (128,128) (`pix_num = 40*40 =
1600`), the per-block offsets are:

| depth | C sum | port sum | offset per block |
|---|---|---|---|
| 8x8 (64 blocks)   | 13888 | 787  | ~205  |
| 16x16 (16 blocks) | 18724 | 1061 | ~1104 |
| 32x32 (4 blocks)  | 20172 | 1143 | ~4757 |
| 64x64 (1 block)   | 20439 | 1158 | ~19281 |

The offset grows with BLOCK AREA (1 : 5.4 : 23.2 : 94 against areas
1 : 4 : 16 : 64), i.e. roughly 3.2-4.7 per PIXEL — so it reads as a per-pixel
sample difference over the whole block, not a per-block additive rate term.
A per-pixel difference that appears only on partial superblocks points at what
the two searches READ at the picture edge (C's replicated border versus
whatever the port's ME hands its SAD), not at the accumulation.

**`compute_distortion` itself is NOT the bug** — the port's
(`inter_me/candidates.rs:401`) is a line-for-line transcription of C's
(`motion_estimation.c:2739`), including the `pix_num = b64_geom->width *
b64_geom->height` cropping that makes a partial superblock's numbers scale up.
So the divergence is entirely upstream, in `me_ctx->me_distortion[]`.

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

## The consequence, and why nothing saw it before

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

Fix order is 2 then 1: the ME distortions feed the same thresholds
`fast_lambda` scales, so fixing the lambda against wrong distortions would fit
one error to another. Both are checkable per superblock against this table,
and `SVT_PICKPART0_OUT` then says whether the PD0 tree follows.
