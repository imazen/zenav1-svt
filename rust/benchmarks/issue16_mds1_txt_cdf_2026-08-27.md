# Issue #16 — MDS1 candidate cost: 3 of 57 differ from C by ~103 rate units

Root-caused and fixed 2026-08-27. Cell: `gb82-sc/terminal.png` cropped to
188x256, preset 2, qp 55, block mi=(50,42) = org (168,200), 8x8. Host: Apple
M-series (darwin 25.5.0). C oracle: v4.2.0 in-tree, run in the Linux
`-Wl,--wrap` container (`tools/ctrace-linux/run.sh`,
`SVT_FASTCOST_XY="168,200"` + `SVT_FULLCOST_XY="168,200"`). Port dump:
`SVTAV1_NSQDBG=1 SVTAV1_CANDDBG=1 SVTAV1_DBG_MI=50,42` (`NSQDBG PFAST` /
`PMDS1`). Streams byte-identical on both sides (772 B), before and after.

## The probe the issue asked for

| term | port vs C |
|---|---|
| `PFAST` fast cost (= `flr + fcr` at the block lambda) | **57 / 57 identical** to `CFAST cost` (all 46 intra + 11 palette rows) |
| `ydist` | identical (already known) |
| `port.coeff_rate − C.ycb` | **1457** on every directional candidate, **1354** on the two DC-family ones (DC, FILTER_DC) — the 103 |

C's extra over `ycb` is `non_skip_tx_size_bits + skip_fac_bits[ctx][0]`
(`svt_aom_full_cost`, rd_cost.c:1349), constant per block; the port adds the
same. So the 103 lives inside the port's `dec_bits`, whose only mode-dependent
input is `intra_dir` → the **tx-type rate row**. The differing rows (intra
`DC_PRED`, inter) are exactly the rows earlier IntraBC / DC blocks in this
screen-content tile had ADAPTED; averaging or copying two default rows returns
the default, which is why the directional rows agreed.

## Root cause (C, shipped)

`svt_av1_cost_coeffs_txb` (rd_cost.c) derives
`is_inter = is_inter_mode(cand_bf->cand->block_mi.mode)` — **without**
`use_intrabc`. It feeds `av1_transform_type_rate_estimation` (rd_cost.c:107),
which at `allow_update_cdf = 1` (the encode pass, coding_loop.c:1539 →
`svt_aom_txb_estimate_coeff_bits`) UPDATES `intra_ext_tx_cdf[eset][sqr][DC_PRED]`
for an IntraBC txb with the intra set's symbol, and never touches the inter row.
The bitstream writer (`av1_write_tx_type`, entropy_coding.c:333-349) uses
`use_intrabc || is_inter_mode(mode)` and codes the inter row. So C's MD-side
`ec_ctx_array[sb]` — the context every per-SB rate table is rebuilt from — carries
a DC row IntraBC blocks adapted and an inter row they did not, and the stream is
unaffected. The READ half of the same quirk (an IntraBC candidate pricing its
tx type on the DC row via `cost_dir`) was already ported (`coeff_rate.rs`);
this is the UPDATE half.

## Fix (port)

`CoeffFc::md_side_ibc_txt_update` (chain contexts only) routes an IntraBC
luma txb's tx-type adaptation to `md_update_tx_type_ibc_quirk` — intra set,
DC row, intra symbol (filler 0 for an out-of-intra-set type), no update at
32x32+ where the intra set is DCT-only — instead of `write_tx_type_inter`.
The pipeline's chain simulation sets the flag; every real writer keeps it off.

## After the fix

| | before | after |
|---|---|---|
| port MDS1 costs present in C's `st=1` 8x8 rows | 54 / 57 | **57 / 57** |
| missing | 124,588,651 / 118,117,740 / 140,267,980 | none |
| stream | 772 B == C | 772 B == C |

Gates (this commit): `regression_spotcheck.sh`, `alignment_gate.sh`,
`screen_ibc_gate.sh`, `screen_palette_gate.sh` — results in the commit message.
