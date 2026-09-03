# The NEAR candidate C injects and the port did not: `diag 72x72 q40 p6` closed

`benchmarks/inter_edge_shape_mode_2026-09-03.md` left this cell with C's
partition tree exactly and one MODE wrong — the port coded `NEWMV` at
`mi=(8,16)` where C codes `NEARMV`, with the **same MV `(24,0)`**. It named the
join to build next: C's `SVT_IFCOST_OUT` against the port's `SVTAV1_CANDDBG`.
Built, and the answer is that the port's injector was never handed C's
`near_count_ctrls`, so it could not produce a `NEARMV` candidate at all.

Cell: `diag 72x72 q40 p6`, `frames=2`, `SVTAV1_FRAME_SHIFT=3`, low-delay P.
C oracle `reference/svt-av1 @ fix/suspected-c-bug-17` through
`tools/ctrace-linux/run.sh` (linux/arm64 container, this host).
C frame 1 **28 B**, port **29 B** before, **28 B** after.

## The MDS0 candidate lists, side by side

C: `SVT_IFCOST_OUT` with `SVT_IFCOST_XY=64,32` (the block's pixel origin —
`mi=(8,16)` is `(col*4, row*4)`). Port: `SVTAV1_CANDDBG=1 SVTAV1_NSQDBG=1
SVTAV1_DBG_MI=8,16`, `NSQDBG ICAND` lines at that mi. `flr` is
`cand_bf->fast_luma_rate` on both sides. `mode` is C's `PredictionMode`
(13 `NEARESTMV`, 14 `NEARMV`, 16 `NEWMV`, 17 `NEAREST_NEARESTMV`).

| # | C mode | C rf | C mv | C `flr` | port mode | port rf | port mv | port `flr` |
|---|---|---|---|---|---|---|---|---|
| 1 | 13 | 1 | (0,-24) | **2520** | 13 | 1 | (0,-24) | **2520** |
| 2 | **14** | 1 | (24,0) | **2845** | — | — | — | **absent** |
| 3 | 13 | 5 | (24,0) | **4957** | 13 | 5 | (24,0) | **4957** |
| 4 | 17 | 1,5 | (24,0) | 3667 | — | — | — | suppressed (bipred) |
| — | — | — | — | — | 16 | 1 | (24,0) | 4187 (`pmv0=24,0 drl=1`) |

**The port's rate model was already exact on every candidate the two lists
share** — 2520 and 4957 to the unit. The divergence is a candidate that does
not exist on the port side.

C's MDS0 cost is `(lambda * flr) >> 9 + distortion * 128` (derived from the
four rows: `lambda=188575`, rows 2 and 3 share `dist=19360` and differ by
`(4957-2845) * 188575 >> 9 = 777 872`, which is their cost difference exactly).
So C picks row 2 at 3 525 924, ahead of row 4 at 3 828 675 and row 3 at
4 303 795; the port picked its `NEWMV` at 4 020 196. NEARMV predicts `(24,0)`
free from the ref-MV stack; NEWMV pays the MV difference.

**The port's stack already held `(24,0)` at the NEARMV position.** Its own
`NEWMV` line reads `pmv0=24,0 drl=1`, i.e. `ref_mv_stack[1]` — which is what
`NEARMV drl=0` predicts. So this was never an MVP-stack defect.

## The mechanism

`inter_md_arm` handed `port_md::inject`'s `InjectCtx` a
`near_count_ctrls: Default::default()`, and the module header justified it:

> `near_count_ctrls` — C caps the NEAR DRL loop to ZERO unless this control is
> enabled (it REPLACES `max_drl_index`, it does not refine it), so `NEARMV` is
> absent exactly the way C makes it absent.

The first clause is true of C's `enabled == 0` arm
(`mode_decision.c:1377-1381`: `cap_max_drl_index` inits 0 and is only ever
assigned inside the `if`). The conclusion does not follow, because
**`enabled` is 1 in every one of the seven arms of `set_cand_reduction_ctrls`**
(`enc_mode_config.c:4113 / 4138 / 4163 / 4193 / 4224 / 4255 / 4290`), and the
video arm's `pcs->cand_reduction_level` is 0, 1 or 2 (`:9039-9050`) — all three
of which carry `near_count = 3`. Level 6, the only arm with `near_count = 0`,
is assigned solely under `scs->rc_stat_gen_pass_mode` (`:9052`).

So at every preset this port can express, C injects up to
`MIN(3, svt_aom_get_max_drl_index(ref_mv_count, NEARMV))` `NEARMV` candidates
per single reference, and the port injected none.

**The NEAR loop itself was already ported and unit-tested**
(`port_md/inject.rs:849-880`, plus
`tier4_mvp_near_loop_is_zero_without_near_count_ctrls`). So was the control's
derivation (`port_enc_mode_config::encdec::set_cand_reduction_ctrls`, tier 1
through `svt_aom_sig_deriv_enc_dec_default`). Only the wire between them was
missing. That makes this the **SIXTH** "a caller passes a constant where the
derivation is already ported" finding of this campaign, after `dlf_level = 0`,
PD0's `inter` argument, `md_config.rs:948`, `was_intra: Some(1)` and
`refresh_frame_flags: 0`.

## The experiment came before the refactor

A throwaway `SVTAV1_XNEAR` env forced `{enabled, 3, 3}` into that one field.
With it set the cell went 29 B -> **28 B**, and the port's candidate list
gained `mode=14 flr=2845` and LOST its `NEWMV` — C's own
`mv_is_already_injected` dedup, which the port already had, drops the ME
`NEWMV` at `(24,0)` once `NEARMV` has injected that MV. The full 96-cell grid
under the hack is **byte-for-byte identical to the grid under the landed
wiring**, which is what made the refactor worth writing rather than guessing.

## The grid

`tools/inter_byte_matrix.sh`, 96 cells, before and after on this host:

| verdict | before | after |
|---|---|---|
| BOTH | 91 | **92** |
| F1DIFF | 4 | **3** |
| F0DIFF | 1 | 1 |
| CRASH | 0 | 0 |

Records: `benchmarks/inter_byte_matrix_2026-09-03-near.{tsv,meta}` and the
`-before-` sibling. The three residual F1DIFF cells did not move by a single
byte — `diag 72x72 q55 p6` 31 vs C's 29, `diag 72x72 q55 p8` 30 vs 29,
`diag 128x128 q20 p8` 26 vs 25, identical counts before and after. A fix that
closes one cell's mechanism and leaves three untouched is the honest reading;
it is not evidence about theirs.

## A SECOND divergence this join exposed, and it is not this cell's

With NEAR injection on, the port emits a **fourth** candidate C does not have:
`mode=14 rf=5 mv0=(0,-24)`, i.e. a `NEARMV` off `BWDREF_FRAME`. C's dump at
this block has no such line, so C's `ref_mv_count[BWDREF_FRAME]` is below 2
there while the port's is at least 2. It loses on cost here (`flr=5699`) and
moves nothing on the grid, so it is recorded rather than chased — but it is a
real BWDREF ref-MV-stack divergence and the next chunk on the `q55` cells
should check whether it is live on them before assuming it is not.

## What this does NOT close

* `is_intra_bordered` is still the constant `false` in `inter_md_arm`. The
  wiring now passes C's real `use_neighbouring_mode_ctrls.enabled` (1 from
  level 2 up, i.e. at preset 8), and it is read ONLY in conjunction with
  `is_intra_bordered` — so the pair is one unported input, and the field
  cannot move a byte until the other half is derived.
* `redundant_cand_ctrls` and `reduce_unipred_candidates` are wired from the
  same derivation and are 0 at levels 0..2, i.e. inert on this envelope by
  measurement of C's table, not by assumption.
