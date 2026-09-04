# The three F1DIFF cells were a CLASS prune C runs only on inter frames

`docs/INTER-ENCODE-PLAN.md` §1z³² localized `diag 72x72 q55 p6` frame 1 to
`nic::stage_mds1_to_mds3`: MDS1 costs agreed candidate-for-candidate, NEARMV
won at MDS1 on both sides, and **C admitted fewer candidates to MDS3 than the
port did**. This file records which threshold, measured rather than reasoned.

## The instrument

`SVT_FULLCOST_OUT` (`svt_aom_full_cost`, already wrapped) was extended to dump
the per-class NIC state beside each candidate's cost, and a `CNIC` header line
carrying the whole `NicPruningCtrls` row plus the SLICE TYPE. Driven through
`rust/tools/ctrace-linux/run.sh` on the same `.yuv` the cell's port run wrote.

Two things about the dump matter as much as the numbers:

* **The `CNIC` header is reprinted whenever the slice type changes**, and every
  `CFULL` line carries `sl=`. The dump is pinned by block ORIGIN only, so a
  2-frame cell puts frame 0's I_SLICE rows and frame 1's inter rows in one
  file. §1z³²'s six-candidate MDS1 table was read from the version without
  that stamp and **mixed frame 0's three INTRA rows into frame 1's candidate
  set** — see the correction below.
* `I_SLICE` is **1** in `definitions.h:1892`, not 0. `sl=1` is the key frame.

## What C does at `mi=(8,16)`, block origin (64,32), 16x32

```
CNIC slice=0 mds1_class_th=200 m1band=16 mds2_class_th=10 m2band=10
     mds3_class_th=5 m3band=16 imult=50 m1ci=300 m1ce=300 m1rank=3
     m2c=3 m2rank=1 m2dev=5 m3c=3 skipmds1=1 mergemult=4 scal=2,1,1 staging=1
```

| stage | per-class counts (C0..C4) | what happened |
|---|---|---|
| injected (`md_stage_0_count`) | **29, 6, 3, 0, 0** | 29 intra, 6 MVP-inter, 3 NEWMV-inter |
| after `post_mds0` | **0, 3, 3, 0, 0** | `mds1_class_th` DELETED the intra class |
| after `post_mds1` | **0, 1, 0, 0, 0** | `mds2_class_th` DELETED the NEWMV class |
| MDS3 | **0, 1, 0, 0, 0** | ONE candidate: NEARMV |

The arithmetic on the second kill, from the same dump's `st=1` rows:

* class 1 head (NEARMV) 27 427 295 — this is `best_md_stage_cost`
* class 2 head (NEWMV) 36 661 249
* `dev = (36661249 - 27427295) * 100 / 27427295 = 33`
* `mds2_class_th = DIVIDE_AND_ROUND(10 * 9146, 10000) = 9` at CLI qp 55
* `33 >= 9` -> `md_stage_2_count[2] = 0; continue`

## Why the port kept three

`leaf_funnel::nic` carried the CANDIDATE half of all three prunes and the CLASS
half of only `post_mds2` — with the `MAX(25, scaled * i_mds3_class_th_mult)`
re-floor applied unconditionally. Every one of those choices is correct for an
I_SLICE and wrong for an inter frame:

| C | I_SLICE | inter frame | the port, before |
|---|---|---|---|
| `mds1_class_th` (`:7826`) | forced `~0` | 200 base / 183 scaled | not carried at all |
| `mds2_class_th` (`:7897`) | forced `~0` | 10 base / 9 scaled | not carried at all |
| `mds3_class_th` re-floor (`:7977`) | `MAX(25, 5*50) = 250` | `5` | 250 unconditionally |
| `mds1_cand_base_th` (`:7840`) | intra half | per class | intra half for all |

`nic_arm`'s module header stated the reason for the first two omissions in as
many words: "every picture this port encodes is an I-slice". That premise
stopped holding when the port started encoding inter frames, and nothing in the
tree re-derived it.

## The correction to §1z³²

That section's MDS1 table lists six candidates at this block — three intra
(modes 4/6/4) and three inter (NEARMV / NEARESTMV / NEWMV) — and reports "five
of six agree to the UNIT". **The three intra rows are frame 0's.** On frame 1 C
admits ZERO intra candidates to MDS1 at this block. The inter rows and their
costs are correct, and so is the conclusion the section draws from them (the
divergence is the count admitted to MDS3, not the costs); the candidate set it
attributes to frame 1 is not.

## Result

| cell | before | after | C |
|---|---|---|---|
| `diag 72x72 q55 p6` frame 1 | 31 B | **29 B, identical** | 29 B |
| `diag 72x72 q55 p8` frame 1 | 30 B | **29 B, identical** | 29 B |
| `diag 128x128 q20 p8` frame 1 | 26 B | 26 B (unmoved) | 25 B |

`inter_byte_gate` 94 required / 0 failed, 2 of 3 known-open cells promoted.

## What did NOT move, and why the brief's prediction was half right

`video_key_matrix.sh` stays at **58 / 60**. Its two unmoved cells
(`gradient p0`, `screenrep p0`) are KEY frames, i.e. I_SLICEs, and every
threshold this chunk turned on is one C forces OFF on an I_SLICE. §1z³²'s
reading that the two scoreboards had become "one target, not two" was based on
both diverging at MDS3; they do, but not through the same code. **This
mechanism structurally cannot move a key-frame cell.**

## Still not modelled, named rather than assumed

* **`merge_inter_cands`** (`mode_decision.c:3637-3643`). `merge_inter_cands_mult`
  is 4 at nic level 8, so it is LIVE on inter frames: when
  `min(md_me_dist, md_pme_dist) / (bw*bh)` is under
  `(mult * (63 - qp)) >> 1`, C puts EVERY inter candidate in class 2 and the
  two inter classes become one. It can only MERGE classes, so it cannot hide a
  candidate — but it changes which class the prunes above measure, and both
  `md_me_dist` and `md_pme_dist` are written by `read_refine_me_mvs`, which
  this port does not have.
* **`MD_STAGE_NICS` picture type.** `svt_aom_set_nics` bases the per-class
  counts on `MD_STAGE_NICS[pic_type]`: `{64,0,0,64,64}` on an I_SLICE,
  `{32,...}` on a reference frame, `{16,...}` on a non-reference one, and the
  minimum-count floor drops from 2 to 1 at pic_type 2. `leaf_funnel`'s
  `nic_counts` hardcodes the I_SLICE row's 64/32/16. MEASURED at this cell: C
  runs pic_type 1, giving MDS1/2/3 caps of 4/2/2 where the port computes 7/2/2
  — the two later stages agree and the MDS1 cap does not.
