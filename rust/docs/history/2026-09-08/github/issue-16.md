# Historical GitHub issue #16: MDS1 candidate cost: 3 of 57 differ from C by a constant ~103 rate units at one block (pure rate, mode=DC_PRED/uv=UV_DC)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/16. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Found while proving the port's MDS1 full costs are the same quantity C's `perform_ind_uv_search_last_mds` reads (they are — both minima the new ind-uv gate consumes match C exactly at both cells). Filed because of its SHAPE, not its impact: constant, pure-rate, mode-specific, reproducible.

**Observed** — `terminal 188x256 p2 q55`, mi=(50,42), positional join of `NSQDBG PMDS1` against `SVT_FULLCOST_OUT` `st=1` rows: **54 of 57 costs bit-identical; 3 differ, port cheaper by the SAME amount**:

```
124,588,651  vs  124,896,012
118,117,740  vs  118,425,102
140,267,980  vs  140,575,342
```

`ydist` is identical on all three ⇒ the delta is **pure RATE**. At that block's lambda (1,527,856), −307,361 cost is **~103 rate units / 0.20 bits**.

**Discriminators:**
* All three are `mode=DC_PRED, uv=UV_DC`.
* The 11 palette DC candidates MATCH.
* Of the two IntraBC candidates, **one matches and one does not** — any explanation must split that pair.
* At `p4 q12` mi=(46,46), all 6 MDS1 costs match.

**Ruled out — `use_filter_intra` off-flag.** `25cf3090d` named it as the candidate; that was retracted in `a9ed8636b`. C codes the flag exactly for `DC_PRED` with `palette_size == 0` (`mode_decision.c:105-107`), which fits the palette rows matching — but the port ALREADY prices it on exactly those candidates (`leaf_funnel.rs:4987`, `if fi_elig && mode == 0 { flr += rates.fi_flag[..] }`) and `flr` feeds the MDS1 cost at `:6096`. It also fails to split the IntraBC pair. Deliberately not replaced with another guess.

**Next probe (one run, no implementation needed):** `SVT_FASTCOST_XY="168,200"` through `tools/ctrace-linux/run.sh`. `svt_aom_intra_fast_cost` is already interposed (`wrap_recon.c:543`) and yields C's per-candidate `fast_luma_rate` / `fast_chroma_rate`; the port dumps `flr`/`fcr` (`NSQDBG PFAST`, `:5072`) and `coeff_rate` (`PMDS1`). That splits the 103 units across the three terms and names the symbol. Guessing ahead of that measurement is what cost the four preceding rounds on #15.

**Impact today: none gated.** 648/648 on `tools/unaligned_identity_scan.sh`, byteid fingerprint 168/168 with 0 rows moved, every other gate green.

**Latent risk, which is why this is worth an issue:** the new ind-uv gate (`045a07b37`) reads MDS1 minima, so a cell where a mis-priced candidate IS its class minimum would flip the `inter_vs_intra_cost_th` arm. Syntax-element pricing errors in this port have twice been latent-then-live — the `filter_intra` flag desync and the CDF undo-log cross-field overspill — so a constant, mode-specific rate offset is not something to leave unrecorded.
