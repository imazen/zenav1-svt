# 10-bit (bd10) port map (task #94, extracted 2026-07-17)

C baseline note: Source/ = v4.2.0 + the SVT_HDR_MODE hybrid (PR #2). The
mainline-mode build the identity harness links is v4.2.0-equivalent
(36/36 + real spots green post-merge). This fork's intake is PACKED u16
only (EbSvtIOFormat unpacked-plane fields removed); enc_settings.c:996
defaults bit_depth 10 under SVT_HDR_MODE.

## Input/config
- bd 8 or 10 only; 4:2:0 only; profile MAIN. Seq header: exactly ONE new
  bit at bd10 (high_bitdepth, entropy_coding.c:2676-2684 write_bitdepth);
  twelve_bit/profile-2 unreachable.
- Ingestion splits u16 -> 8-bit MSB plane (>>2 TRUNCATION, no rounding)
  + 2-bit plane packed 4/byte (svt_unpack_and_2bcompress). The split is
  MEMORY LAYOUT ONLY for the input picture; input_frame16bit and every
  other 10-bit buffer (MD cand, EncDec recon, refs, DLF/CDEF/LR) is
  PLAIN PACKED u16. PORT: use plain u16 planes; never implement 8+2.

## hbd_md (the switch) — enc_mode_config.c:2476-2483 (allintra)
- enable_hbd_mode_decision = bd>8 ? DEFAULT : 0 (Globals/enc_handle.c
  ~:4518); --hbd-mds NEVER read on the allintra path.
- MR-and-faster tiers: hbd_md = 1 (true 10-bit MD). M0..M13: hbd_md = 2
  (DUAL). DUAL == true 10-bit EVERYWHERE except 3 inter-compensation
  helpers (IntraBC downgrades to 8-bit search under DUAL) — all other
  consumers test truthiness/>8BIT.
- PD_PASS_0 is UNCONDITIONALLY 8-bit at every preset (enc_dec_process.c
  :2965 saves hbd_md, forces 0, restores before PD1). PORT WIN: pd0.rs
  stays u8, reading the MSB-truncated plane.
- hbd_md=0 pockets read enhanced_pic = MSB-TRUNCATED plane with 8-bit
  lambda/variance tables. RD decisions are PRECISION-SENSITIVE: the port
  must replicate the exact hbd_md per preset/pass/block, not just
  produce correct pixels.
- bypass_encdec (allintra: 0 at <=M3, 1 at M4+): at M4+ MD recon IS the
  coded recon, so the winner is re-predicted at 10-bit + converted back
  (product_coding_loop.c:9149-9174 / :9640-9699 save/restore dance).
  At <=M3 the separate EncDec stage always runs at TRUE depth
  (is_16bit_pipeline), independent of hbd_md.

## Pixel pipeline
- Highbd intra predictors (intra_prediction.c u16 family incl. dr_z1/z2,
  filter-intra, CfL 420 hbd subsample). Residual i16 either way.
- TX: coeffs i32 already; recon add clip = clip_pixel_highbd(bd); range
  check (1<<(7+bd))-1+(914<<(bd-7)); tx_scale UNCHANGED (size-keyed).
- Quant: dc/ac_qlookup_10_QTX tables; qzbin factor thresholds x4 ladder.
  qindex domain 0..255 at every bd.
- LAMBDA (the (bd-8)*2 site): full_lambda[10bit] *= 16, fast *= 4
  (md_process.c:724-765 :753-754); rd_mult ROUND_POWER_OF_TWO(...,4) at
  bd10 (rc_process.c:365-393); selection = hbd_md truthy.
- SAD/variance: vf_hbd_10 function-pointer family (av1me.c:24-33).

## Loop filters — keyed on is_16bit_pipeline (TRUE depth), NEVER hbd_md
- DLF: highbd kernels; LEVEL SEARCH compares vs the TRUE 10-bit source
  (input_frame16bit), even when MD searched at 8-bit.
- CDEF: dir search u16-native already; filter dst8/dst16 dual out.
- LR: u16 working buffers. Recon/ref buffers = plain u16 (2 B/px).

## Entropy — ZERO bit-depth references in coeff coding + CDF init
(cabac_context_model.c has none; qindex 0..255). ONE leak: palette color
literals are written with bit_depth bits (entropy_coding.c:4256-4370) —
our landed palette writer/cost hardcodes 8; parameterize when bd10
meets sc content.

## Scope for zenav1-svt (CORRECTED — the agent sampled /root/aom-rs)
zenav1-svt is u8 end-to-end in zenav1-svt-dsp (intra pred, tx/quant/recon
kernels), svtav1-encoder (funnel pred/recon Vec<u8>, pipeline &[u8]
planes, deblock/cdef/restoration u8), harness. svtav1-entropy needs no
change except palette literal width. Real work:
1. Config/harness intake: u16 planes + bit_depth knob; identity_run bd10
   axis (y4m/raw 10-bit LE) + capture_c_trace bd10 flag.
2. svtav1-dsp: u16 (or generic) intra pred + recon-add clip(bd) + hbd
   SATD/SAD/variance/SSE; quant already table-driven — add the _10
   tables + qzbin ladder.
3. Funnel/pipeline: thread bit_depth as a no-op-for-bd8 param first
   (chunk 1, byte-identical gate), then the u16 plane plumbing; lambda
   *16/*4 selection; PD0 stays u8 on the truncated plane (build it at
   ingestion).
4. Filters: u16 DLF/CDEF/LR variants; DLF level search vs true source.
5. The M4+ bypass_encdec re-predict dance; <=M3 EncDec walk at true
   depth (the port's walk = the EncDec analogue).
MILESTONE per the map: our targets are M0+ (DUAL == true 10-bit for
intra) — smallest cell = uniform 64x64 bd10 at a <=M3 preset (bypass 0,
no dance), single fixed partition; needs chunks 1-4 only.

## FIRST CELL LANDED 2026-07-18 (aa89a83be) — uniform <=M3 bd10 byte-identical

`tools/bd10_matrix.sh` 18/18: uniform {64,128} x qp{20,40,55} x preset{0,2,3}
byte-match real aomenc at bd10 AND decode under aomdec. Harness bd10 axis:
`capture_c_trace <..> 10` (packed u16 LE input) + `identity_run` `SVTAV1_BD=10`
(writes u16 for C, port stays u8) + `with_bit_depth`. The pipeline already had
the bit_depth field + SH high_bitdepth bit; only the harness was missing.

**Why u8-port is correct for uniform (not a hack):** flat -> every block skip
-> no residual coded -> tile bytes bit-depth-INDEPENDENT (measured: C uniform
bd8 vs bd10 differ in exactly ONE byte = the SH high_bitdepth bit); the decoder
fills DC from the 10-bit default (512) and skips residual, so it decodes to
uniform-512 correctly.

**LF-from-Q at bd10 LANDED (be1ea0770):** uniform bd10 now byte-matches C at
ALL presets — `tools/bd10_matrix.sh` **36/36** ({64,128} x qp{20,40,55} x
preset{0,2,3,6,10,13}). The M6+ LPF_PICK_FROM_Q closed form is now bd10-aware in
`deblock::pick_filter_levels_key_frame` (bd10 KEY: `ROUND_POWER_OF_TWO(q*20723 +
4060632, 20) - 4`, q = AC_QLOOKUP_10). bd8 byte-neutral (matrix 54/54).

**CDEF-from-Q at bd10 LANDED (this commit):** the qp-fast-path CDEF strength
(`cdef::pick_cdef_params_key_frame`, presets M7+) is now bd-aware, mirroring the
LF-from-Q fix — C `q = ac_quant_qtx(qindex,0,bd) >> (bd-8)` (enc_cdef.c:829-830),
i.e. `AC_QLOOKUP_10[qindex] >> 2` at bd10, with the SAME f32 fit constants. The
knee shifts (16 qindexes give a different strength than bd8) because the
higher-precision bd10 q crosses the CDEF-off threshold at a different qindex.
Proven C-exact for all 256 qindexes by the `c_parity_cdef_pick` bd10 differential
(port vs real `svt_aom_ac_quant_qtx`) AND end-to-end: the gradient bd10 op-trace's
first divergence moved OFF the FH cdef line into the tile payload once this landed.
bd8 byte-neutral (identity matrix 54/54, bd10 uniform 36/36). NOTE: this fixes the
qp-fast-path CDEF *header* value only; the CDEF **search** path (presets M0..M6,
`cdef_search_still`) has its own bd-dependency (u16 recon MSE + bd10 lambda in
`finish_cdef_rd`) — but for 8-bit-representable content the search's strength is
moot until the coefficient/quant divergence below is closed (a differing coefficient
already desyncs the tile).

**FIRST NON-FLAT bd10 CELLS LANDED (2026-07-18) — the "big one" started.** The u16
MD path did NOT need the full generic-`Pixel` refactor below: it was done ADDITIVELY
via the M4+ bypass_encdec re-predict shape — the u8 partition/mode/tx decisions are
RD-scale-invariant for `sample<<2` content, so `pipeline::bd10_reencode_luma`
recomputes only the bit-depth-sensitive coded luma LEVELS + true-10-bit recon,
bd10-gated, leaving the u8 path byte-unchanged. THE FIX was `quantize_fp`'s INT16
clamp being bd8-only (C dispatches bd>8 to `highbd_quantize_fp_helper_c`,
full_loop.c:367-395) → `quant::quantize_fp_hbd`. Cells: `gradient 64x64 q40 p10/p13`
(`tools/bd10_nonflat_gate.sh`, 2/2, CI). Envelope = DC-family / tx_depth-0 / rdoq-fp;
out-of-envelope frames fall back to the non-panicking u8 output via the
`bd10_tree_supported` gate (encoder stays panic-free on the public API). FOLLOW-UPS:
`dr_predict_hbd` (directional), `predict_filter_intra_hbd`, `quantize_b_hbd` (rdoq-0,
same clamp class), tx_depth>0, u16 chroma, native u16 ingestion. The generic-Pixel
plan below is SUPERSEDED for the coded-levels path (the additive re-encode is the
maintainable shape that landed); it may still guide a future full-u16 recon/filter
pass if one proves necessary for the follow-ups.

**FOLLOW-UPS LANDED (2026-07-18, this session) — non-flat gate 2/2 -> 8/8.** Four
additive, bd10-gated pieces (bd8 byte-unchanged: identity_matrix 54/54, bd10 uniform
36/36):
- **`quant::quantize_b_hbd`** (rdoq level 0): C `svt_aom_highbd_quantize_b_c`
  (full_loop.c:85) — [`quantize_b`] minus the INT16 clamp (same clamp-is-bd8-only class
  as the fp fix; the `idx_arr` prescan is outcome-identical to the contiguous one). Wired
  into `tx_unit_hbd`'s do_rdoq==false branch (was calling the buggy u8 `quantize_b`).
- **64-dim qcoeff re-expansion** in `bd10_reencode_node`: `tx_unit_hbd` returns the tight
  pw-stride packed txb; the entropy walk (like u8 `funnel_block_decision`) expects
  `d.qcoeffs` as a full w*h stride-w raster. Was a PANIC on a TX_64X64 leaf at high
  qindex (q55). Now re-expands, mirroring the u8 path.
- **`hbd::predict_filter_intra_hbd`** (filter-intra): wired into `predict_unit_hbd`'s fi
  arm (above[0]=top_left via extract_neighbors_hbd, base=512 corner fallback). Gate widened
  (dropped the `fi == FI_NONE` restriction).
- **`intra_edge::dr_predict_hbd`** (directional): u16 twin of `dr_predict` — same
  geometry/availability/edge-array construction, flat-fill `{129,127,128}` -> `{base+1,
  base-1,base}` (base=128<<(bd-8), C `build_intra_predictors_high` enc_intra_prediction.c
  :261-374), hbd edge-filter/corner/upsample/dr kernels. Wired into `predict_unit_hbd`'s
  directional arm with the same DrGeom as u8. Gate widened to admit directional leaves when
  the SH edge filter is OFF (the re-encode passes filt_type=0 — valid only then; edge_filter
  is now threaded through `bd10_reencode_luma`/`_node`/`bd10_tree_supported` from
  `FunnelCfg::for_preset(preset).edge_filter`). VERIFIED: `dr_predict_hbd(bd=8)` byte-matches
  the C-verified u8 `dr_predict` across modes/sizes/positions/edge-filter (new
  `intra_edge::tests::dr_predict_hbd_bd8_matches_u8_dr_predict`); the base constants are
  checked vs C source and the hbd kernels are FFI-verified.

New byte-exact cells (`tools/bd10_nonflat_gate.sh`): `gradient 64x64 q55 {p3,p6,p10,p13}` +
`gradient 128x128 q55 {p10,p13}` (all non-flat: total_eob 521/1476; rdoq levels 1/2/3).

**MEASURED — the gradient non-flat sweep is dominated by TWO blockers OUTSIDE the four
follow-ups (op-traced, `tools/identity_diff.py`):**
1. **The u8 tree is NOT bit-depth-scale-invariant at low qindex / 128px.** e.g. `gradient
   64x64 q20 p10`: C's bd10 tree keeps a 32x32 PARTITION_NONE where the u8 (bd8) tree
   SPLITs to 16x16 — a **partition-symbol** divergence (identical CDF, different coded
   symbol), not a level bug. The re-encode reuses the u8 decisions, so it structurally
   CANNOT fix these — they need a true bd10 MD pass (the generic-Pixel refactor). Affects
   q20 (all) and most 128px cells.
2. **bd10 CDEF-search + Wiener-LR post-filter divergence at M0..M6.** e.g. `gradient 64x64
   q40 p6`: after filter-intra, the TILE PAYLOAD byte-matches (264B==264B) — the ONLY
   divergence is the Wiener LR taps; `gradient 128x128 q55 p3`: the FH `cdef_y_pri_strength`
   differs (C=8, port=12). These are the unported bd10 dependencies of `cdef_search_still`
   /`finish_cdef_rd` (u16 recon MSE + bd10 lambda) and the Wiener tap search — NOT prediction
   /quant. High-qindex (q55) cells converge (why q55 p3/p6 64x64 match); q40/mid-q diverge.

**Directional is NOT exercised by `gradient`** (it selects DC-family only: modes {0,1,2,9,
10,12}, angle_delta 0). Added a `diag` content generator (identity_run.rs, constant along
r-c) that DOES pick D45/D135/D67 at M3 — but M3 has the CDEF/LR bd10 divergence above, so
`diag` cells don't byte-match yet (the divergence is CDEF/LR, tile sizes equal). Directional
is therefore verified by the bd8-equivalence unit test, not e2e.

**STILL-open re-encode follow-ups (unchanged priority order): tx_depth>0** (every gradient
cell that needs it — e.g. `128x128 q40 p6` — is also M0..M6 CDEF/LR-blocked, so it would
widen the envelope but not grow the gate on this content); **directional WITH edge_filter on**
(M5 — needs the live per-block `get_filt_type` smooth-neighbour derivation in the re-encode
walk); **the u16 chroma path**; **native (non-`<<2`) u16 ingestion**. The bigger unlock for
real bd10 content is the **CDEF-search / Wiener-LR bd10** dependencies and a **true bd10 MD**
(scale-variant decisions) — both OUTSIDE the coded-levels re-encode.

## FALL-BACK MAP + blocker root-cause (2026-07-19, task #94 investigation)

**Gate grown 8 -> 21** (`tools/bd10_nonflat_gate.sh`, all `cmp`-verified byte-identical):
added the re-encode's previously-ungated working qindex range `gradient 64x64 q{42,44,46,48,50}
x p{3,6,10,13}` (base_qindex 168..200) + `128x128 q58 p{10,13}`. Those cells are LOAD-BEARING
on the re-encode (`last_recon10_y` populated on all; Q10/Q8 = 3.997..3.999 there, so the u16
quant genuinely changes the coded levels vs the u8 fallback — see the quant-ratio table below).
No product code changed — these cells already byte-matched; the gate simply now covers the
range between the old q40 and q55 anchors. bd8 identity_matrix 54/54, partial_sb 101/101,
bd10 uniform 36/36 byte-UNCHANGED.

### The MAP (op-traced first divergence, classified per cell)

Method: per cell, encode rs (`SVTAV1_BD=10`, `SVTAV1_PACKTREE`) + C (`capture_c_trace .. 10`,
`SVT_CTREE_OUT`), `cmp`; if DIFF, classify via `tools/identity_diff.py` (STAGE + op-class) AND
`tools/tree_diff.py` (C bd10 tree vs port u8 tree). The tree diff is the decisive discriminator:
partition/mode flip => blocker 1; trees identical => the divergence is post-filter (blocker 2)
or coefficients.

- **Synthetic sweep** `{gradient,diag} x {64,128} x q{5,12,20,32,40,55} x p{0,3,6,10,13}` = 120
  cells: **69 BLOCKER1_part** (C bd10 tree != port u8 tree — partition geometry flip),
  **40 BLOCKER1_mode** (geometry same, mode/uv/txd/angle flip), **8 IDENTICAL** (the old gate
  cells), **2 BLOCKER2_lr** (Wiener taps), **1 BLOCKER2_filt** (CDEF strength), **0 COEFF**.
- **Photographic sweep** (CID22-512, `file:`, `q{12,32,55} x p{6,10,13}`) = 18 cells:
  **18/18 BLOCKER1_part**. On real content the partition tree ALWAYS diverges at bd10.
- **Zero panics** across all 138 cells (every out-of-envelope cell fell back via
  `bd10_tree_supported`, none crashed the public API).

**KEY RESULT: 109/120 synthetic + 18/18 photographic cells are BLOCKER1 (true-bd10-MD).**
There are **ZERO COEFF cells** — wherever the tree is identical the re-encode already produces
byte-exact coefficients, so nothing on this content is a re-encode-envelope gap. The tractable
tail is 3 pure post-filter cells; everything else needs a true bd10 MD.

The 3 pure-BLOCKER2 cells (trees identical, coeffs identical, only a post-filter value differs):
`gradient 64x64 q20 p6` + `q40 p6` (Wiener LR taps) and `q55 p0` (`cdef_y_pri_strength` C=4/port=5).

### BLOCKER 1 — ROOT-CAUSED + QUANTIFIED (the big one; true bd10 MD)

**The u8-tree-reuse assumes the RD is exactly 16x-scale-invariant for `sample<<2` content**
(dist scales 16x, lambda scales 16x, so NONE-vs-SPLIT ordering is preserved). That holds ONLY
if the effective quantizer at bd10 is EXACTLY 4x the bd8 quantizer. It is not:

```
qidx  (cli)  dc8  dc10  dc10/dc8   ac8  ac10  ac10/ac8
  48  (q12)   48   170    3.542     55   195    3.545
  80  (q20)   74   287    3.878     87   337    3.874
 128  (q32)  140   559    3.993    176   700    3.977
 160  (q40)  223   891    3.996    305  1218    3.993
 220  (q55)  522  2088    4.000    933  3731    3.999
```
Q10 != 4*Q8 at **251/256 qindexes**; the ratio reaches 4.000 only near qindex 220 (cli q55).

This **exactly explains the MAP's qindex boundary**: q55 (ratio 4.000) is scale-invariant, so
the u8 tree is correct and the re-encode (levels only) suffices -> the gate cells are q55 +
q40..q50 (64x64, where 3.996..3.999 is close enough that the single-SB near-ties don't tip);
q<=32 (ratio <=3.99) and all 128px non-flat cells tip a partition/mode RD near-tie -> BLOCKER1.

**DECISIVE PROOF it is intrinsic, not a port bug** (`gradient 64x64 q20 p10`): C's OWN tree
differs by bit depth on byte-identical `<<2` content — **C bd8 = 16x BLOCK_16X16 (bsize 6);
C bd10 = 4x BLOCK_32X32 (bsize 9)** — and the port's u8 tree byte-matches C bd8 EXACTLY (the
same cell is a bd8 byte-match: 54/54). So the u8 MD is faithful; the divergence is purely from
replaying the bd8 partition at bd10, where C keeps 32x32 PARTITION_NONE and the u8 tree SPLITs.
The re-encode (fixed tree, recompute levels) structurally CANNOT fix this.

**SCOPE — true bd10 MD** = re-run the partition/mode/tx RD SEARCH (not just final levels) at
bd10, so each candidate's `dist + lambda*rate` is evaluated at bd10 and the NONE-vs-SPLIT /
mode / tx-type choice is made at bd10. Every KERNEL it needs is already ported + FFI-verified:
bd10 quant (`build_quant_table_bd`/`quantize_*_hbd`), bd10 lambda (`kf_full_lambda_bd10`,
`*16` full / `*4` fast), bd10 distortion (`c_parity_hbd_distortion` SSE/SATD/variance), bd10
intra prediction (`predict_unit_hbd`/`dr_predict_hbd`/`predict_filter_intra_hbd`). The work is
threading them through the funnel/partition RD SEARCH (the generic-`Pixel` pass in the SURFACE
section, or a bd10 MD pass), because the RD COMPARISON must be at bd10 — a large hot-path pass,
NOT dribble-able. This is the single highest-impact bd10 item (100% of real content).

### BLOCKER 1 — ROOT CAUSE CORRECTED (2026-07-19): it is the bd10 `PD0_LVL_6 -> PD0_LVL_0` switch, decided at **8-BIT**

**The "16x-scale-invariance / true-bd10-MD" framing above is WRONG for the PARTITION flip.**
The partition tree is NOT decided by a bd10 RD comparison at all — it is decided by an **8-bit
full-RD PD0** whose *level* C switches on bit depth. Traced end-to-end against the real C
(read-only `/root/svtav1/Source/`) with a temporary `svt_aom_pick_partition_pd0` / DRMODE wrap
(added to `wrap_recon.c`, used, then reverted — the harness is back at its committed state):

**THE bd-dependent gate** — `set_pd0_ctrls` (`enc_mode_config.c:5413-5418`):
```c
static void set_pd0_ctrls(ModeDecisionContext* ctx, uint8_t lpd0_lvl) {
    if (ctx->hbd_md) { ctx->pd0_ctrls.pd0_level = PD0_LVL_0; return; }  // <-- bd10 (DUAL)
    switch (lpd0_lvl) { ... }                                          // <-- bd8: pic level
}
```
- **bd8** (`hbd_md == 0`): PD0 uses the picture level `pcs->pic_pd0_lvl` (`set_pic_pd0_lvl_default`,
  `enc_mode_config.c:8592`; M10 <=360p KEY, `coeff_lvl` HIGH -> level 5..6) => **`PD0_LVL_6`**, the
  *variance-heuristic* cost `compute_lpd0_cost_allintra` (`product_coding_loop.c:8191`, the port's
  FFI-verified `pd0::lvl6_cost_allintra`). It **over-splits** flat-ish high-var SBs to 16x16.
- **bd10** (`hbd_md == 2` DUAL): the gate FORCES **`PD0_LVL_0`** = the *full-RD* PD0
  (`md_encode_block_pd0`'s `md_stage_0_pd0`+`md_stage_3_pd0` path + `test_split_partition_pd0`,
  `product_coding_loop.c:10424`), which makes an RD-optimal NONE-vs-SPLIT choice and keeps 32x32.

**CRUCIAL: `PD0_LVL_0` runs entirely at 8-BIT.** `enc_dec_process.c:2965-2966` saves `hbd_md`
and forces it to **0** for all of PD0 (restored at `:3024`, before PD1). The PD0 distortion
kernel is `hbd_md ? svt_full_distortion_kernel16_bits : svt_spatial_full_distortion_kernel`
(`product_coding_loop.c:1129-1130,1273-1274`) -> **8-bit spatial SSE**; lambda =
`full_lambda_md[EB_8_BIT_MD]`; source = the 8-bit MSB-truncated plane. `pd0_level` is fixed at
sig-deriv time from the *original* `hbd_md` (=2), so it stays LVL_0 even though PD0 then runs at
`hbd_md=0`. => the bd10 partition tree is a pure function of the 8-bit input — **NO bd10 pixel /
quant / lambda kernel is on the partition path.**

**Empirical proof (`gradient 64x64 q20 p10`), same 8-bit `<<2` input both encodes:**
- `PD0 mi=(0,0) bsize=12 part=3 rd=5013       hbd_md=0 pd0_lvl=6 encbd=8`   (bd8)
- `PD0 mi=(0,0) bsize=12 part=3 rd=99809485   hbd_md=0 pd0_lvl=0 encbd=10`  (bd10)
- Both: `DRMODE mode=2(PD0_DEPTH_PRED_PART_ONLY) pred_only=1 bias=995` and
  `PICCFG coeff_lvl=3 depth_refine_lvl=10`. `depth_refine_lvl 10 -> PD0_DEPTH_PRED_PART_ONLY`
  (`enc_mode_config.c:6985-6986`) => PD1 codes the PD0 tree verbatim (`pred_depth_only=1`, the
  `test_split_partition` assert at `:10814` proves no PD1 merge runs). So **final tree == PD0
  tree**, and the PD0 tree differs ONLY because `pd0_lvl` is 6 vs 0. rd 5013 is LVL_6-heuristic
  scale (`area*bias/1000`); rd 99809485 is LVL_0 full-RD scale (`RDCOST(8bit_lambda, rate, 8bit
  SSE)`, `dist<<7`). Trees: bd8 = 16x `BLOCK_16X16`, bd10 = 4x `BLOCK_32X32`.

**Why the port matches bd8 but not bd10:** the port's `pd0_pick_sb_partition` (preset>=9) ALWAYS
uses the LVL_6/LVL_5 heuristic (there is NO faithful `PD0_LVL_0` port anywhere — presets 0-2,
where C also uses LVL_0, are approximated by the LVL_1 `pd0_pick_sb_partition_m6_eval` path,
tuned to reproduce C's bd8 *finals*, not the PD0 algorithm). At bd8 the LVL_6 heuristic
coincides with C's bd8 final; at bd10 C runs LVL_0, so the port's LVL_6 tree is simply the wrong
algorithm's output.

**THE FIX (corrected):** at bd10, run an **8-bit `PD0_LVL_0` full-RD partition search** matching
C's `svt_aom_pick_partition_pd0` at `pd0_level==0` (candidate gen `generate_md_stage_0_cand_pd0`
-> DC-only where `is_dc_only_safe`; `md_stage_3_pd0` full loop with `pd0_fast_coeff_est_level` /
`subres_step`; `test_split_partition_pd0`'s `parent_cost_bias * parent <= split_cost*1000` with
`split_cost = RDCOST(8bit_lambda, split_rate, 0) + Σ children`), all at 8-bit using the port's
EXISTING 8-bit funnel / tx / quant / spatial-SSE kernels. Feed its tree to the current
`bd10_reencode_luma`, which already recomputes the bd10 coded LEVELS. **The partition needs no
bd10 pixels** — only an 8-bit LVL_0 PD0 the port does not yet have. (Building it byte-exact is a
substantial pass: the exact `md_stage_3_pd0` cost/rate model, `subres`, DC-only cand set, and the
`test_split_partition_pd0` early-exits must all match C. Not attempted this session — scoped +
verified only.)

### PD0_LVL_0 LANDED (2026-07-19) — M9+ partition flip FIXED, 10 new byte-exact cells

`pd0::pd0_pick_sb_partition_lvl0` + `Pd0Mode::Lvl0` (pd0.rs); wired at preset>=9 bd10 in
`encode_tile_rows` (pipeline.rs), gated on `bit_depth == 10` (bd8 byte-UNCHANGED). The LVL_0
block cost turned out to be the port's EXISTING LVL_5 closed-form encode with subres OFF —
proven against real C: `rate_est_level 0` above M8 (`pcs->rate_est_level = 0` at enc_mode > M8)
=> PD0 `coeff_rate_est_lvl = 0` (closed form `5000 + ires*1600 + 100*eob`) + `lpd0_qp_offset = 8`
(qindex+8); `use_accurate_part_ctx = 0` above M8 => SPLIT rate DOUBLED; `pd0_level <= PD0_LVL_2`
=> subres OFF; parent_cost_bias 1000, split_cost_th 50; max_sq from `get_max_block_size_allintra`
(the 64-variance cap fires at M8+), min_sq 8. Per-block costs + tree pinned vs real C's
`SVT_PD0COST_OUT` / `SVT_CTREE_OUT` dumps (`lvl0_block_costs_match_c`, e.g. gradient-64 q20 32x32
cost 26185862, tree 4x BLOCK_32X32). Gate `bd10_nonflat_gate.sh` 21 -> **31** (added the 10
partition-only cells: gradient 64x64 & 128x128 at q12/q20/q32 x p10/p13). bd8 identity_matrix
54/54, partial_sb 101/101, bd10_matrix 36/36 byte-UNCHANGED; 576-cell bd10 no-panic sweep 0
panics (added a partial-SB fall-back guard on the bd10 re-encode — `tx_unit_hbd` is not yet
partial-SB-aware and would panic on an edge block; a pre-existing gap, now falls back to u8).

**Preset-dependence discovered (IMPORTANT for widening):** the fix is correct ONLY for M9+.
- **M9+ (p9-13):** bd8 `pic_pd0_lvl` is 5/6 (LVL_6/LVL_5 variance heuristic) — that over-splits
  where C's forced LVL_0 keeps the parent. THIS is the partition flip; `pred_depth_only` (PD0
  tree == final tree) means the LVL_0 partition suffices. FIXED.
- **M6-M8 (p6-8):** `LVL_0 == LVL_1` (identical PD0 config — same `coeff_rate_est_lvl`,
  `use_accurate_part_ctx`, depth set, subres, no detector — only the M9+ jump to LVL_5/6 differs).
  The port's existing m6_eval (LVL_1) path ALREADY produces C's partition (verified: C bd8 p6
  tree == C bd10 p6 tree == port p6 tree, all 4x BLOCK_32X32). p6-8 divergences are pure MODE
  flips (bd10 leaf funnel), NOT partition. Do NOT apply `pd0_pick_sb_partition_lvl0` there — its
  hardcoded M9+ config (closed form / qindex+8 / doubled split) is WRONG below M9 (yields no
  byte-matches; reverted a trial extension).
- **M3-M5 (p3-5):** PD1 depth refinement (PD0_DEPTH_ADAPTIVE) runs over the PD0 tree AND adds NSQ
  (C's p3 tree has HORZ part=1 down to 8x8) — the remaining true partition flips (all 12
  synthetic PARTFLIP cells are p3). Needs the LVL_0 PD0 fed into a bd10-aware depth-refine + NSQ,
  a larger scope than the PD0-only fix. FOLLOW-UP.

Accurate synthetic map (dedup, gradient/diag x {64,128} x q{5,12,20,32} x p{3,6,10,13}, 64 cells):
10 byte-MATCH (all p10/p13), 42 SAMEPART-DIFF (partition correct, mode/level flip = true-bd10-MD),
12 PARTFLIP (all p3 = depth-refine axis).

**Separately, the `BLOCKER1_mode` cells (40 synthetic) are a DISTINCT axis:** `pred_depth_only`
fixes only the PARTITION; PD1 still runs its per-block MODE decision at `hbd_md=2` (TRUE 10-bit),
so a bd10-sensitive mode/uv/tx pick differs from the port's reused u8 modes. THAT axis IS a
"true bd10 MD" (10-bit predict/dist/lambda in the leaf funnel) — the original framing above is
correct for MODES, wrong for PARTITIONS. The gate cells match because their modes (DC-family)
are bd10-stable; the mode-flip cells are not.

### BLOCKER 2 — CDEF-search / Wiener-LR bd10 (bounded; 3 cells on this content)

CONFIRMED CDEF+LR DO run in this harness config (SVT still-picture defaults, NOT libaom
allintra-off): the p0/p6 cells carry searched CDEF strengths + per-SB Wiener taps. The bd10
searches are genuinely bit-depth-dependent (measured, `gradient 64x64 q55 p0`): even though
Q10=4*Q8 there, the bd10 recon != `recon8<<2` (3962/4096 luma px differ by ~+20, from the
hbd intra-predictor rounding), so the CDEF/LR MSE genuinely needs the TRUE bd10 recon.

Precise scope (per search; all operate on u8 recon today):
- **DLF level search** (`deblock::pick_filter_levels_full_search`, M0..M5): u8 SSE on u8 recon.
  Needs u16 recon + u16 source + hbd deblock kernels (FFI-verified, `c_parity_lpf_hbd`) + bd10
  SSE. (from-Q LF at M6+ is ALREADY bd10-aware.)
- **CDEF search** (`cdef::cdef_search_still` + `finish_cdef_rd`): the encoder's `filter_fb_packed`
  writes a **u8** dst and hardcodes `coeff_shift=0`; `finish_cdef_rd` uses the bd8 lambda. Needs
  a **u16-dst CDEF filter** (only the u8-dst `svt_cdef_filter_block_8bit_c` arm is ported/verified;
  the u16-dst `svt_cdef_filter_block_c` must be ported + FFI-verified), `coeff_shift=2` threaded
  (C `svt_cdef_filter_fb`: `pri=(strength/4)<<coeff_shift`, `damping+=coeff_shift`), bd10 lambda,
  and the bd10 recon (`last_recon10_y` + flat-512 uniform chroma). q55 p0 (deblock is a no-op
  there, LF=0) needs ONLY this; it is the smallest blocker-2 cell.
- **Wiener LR search** (`restoration::search_restoration_still`, M0..M6): needs the post-CDEF
  bd10 recon (so DLF + CDEF at bd10 first) + hbd Wiener + bd10 lambda. q{20,40} p6 need the full
  DLF+CDEF+Wiener bd10 chain (deblock RAN on both, so not skippable). Larger than the CDEF cell.

For single-frame identity ONLY the SEARCHES (which pick the signaled levels/strengths/taps) need
bd10 recon — the filter APPLICATION to the stored recon does not affect the OBU. That bounds the
CDEF-only cell to: port+verify the u16-dst CDEF filter, then a bd10 `cdef_search_still` on
`last_recon10_y`. Deferred to a dedicated pass (needs the u16-dst filter FFI-verified first; no
C-side CDEF-search instrumentation exists in `capture_c_trace` yet to diff intermediate MSE).

**(historical) NEXT bd10 chunk = the u16 MD path for NON-FLAT content (the big one).** Uniform
works because every block is skip (no residual); any content with a coded
residual needs the precision-sensitive u16 MD. MEASURED (2026-07-18): gradient
bd10 diverges in the **tile payload / coefficients**, not the frame header — the
port quantizes the residual with the bd8 tables (Q8) while C uses the bd10 tables
(Q10 ≈ 4×Q8 but NOT exactly), so even 8-bit-representable content quantizes to
different levels (e.g. g128 q40 p13: port 791B vs C 669B). The fix is the u16 MD:
u16 intra prediction (hbd predictors — FFI-verified), residual/transform/quant with
the bd10 qlookup + qzbin ladder + lambda *16/*4 (kernels FFI-verified), recon-add
with clip_pixel_highbd. This is the large hot-path pass (the funnel/pipeline plane
plumbing from u8 to u16). Candidate decomposition into byte-verifiable sub-chunks:
**(2a)** plumb u16 through funnel/pipeline/filters with bd8 stored as u16 (values
≤255) — a pure refactor, gate = bd8 identity matrix stays 54/54; **(2b)** flip the
bd10 tables (quant/lambda/pred/recon-clip) on the u16 path, gate = first non-flat
bd10 cell. PD0 stays u8 on the MSB-truncated plane.

SURFACE MEASURED (2026-07-18): the u8 pixel-buffer surface in the encode hot path
is ~374 `u8`/`[u8]`/`Vec<u8>` refs — pipeline.rs 107, leaf_funnel.rs 148, pd0.rs 27
(stays u8), deblock.rs 48, cdef.rs 44 — plus the svtav1-dsp kernels (intra_pred,
fwd/inv_txfm, quant, loop_filter, restoration; an `hbd.rs` already exists for the
FFI-verified highbd kernels). This is a large generic-over-`Pixel` refactor (u8/u16),
NOT a duplicate-path change (maintainability is a standing priority). Because there
is no byte-verifiable slice smaller than "the whole MD path aligned for one cell",
chunk 2a must land as ONE reviewed pass gated by bd8 identity 54/54 — it cannot be
dribbled in. Recommended shape: a `Pixel` trait (`u8`/`u16` impls, `to_i32`,
`from_i32_clamped(bd)`) threaded through the funnel recon/pred buffers first (they
carry the intra-neighbour chain that forces exact recon precision), then deblock/
cdef/restoration, keeping every existing u8 call path as the `Pixel=u8` instantiation.

## Concrete wiring anchors (measured 2026-07-18)
- **Kernel FFI verification (landed, derisks chunk 2):** bd10 quant tables
  (`c_parity_bd10_quant.rs`), hbd loop filters (`c_parity_lpf_hbd.rs`), hbd
  distortion/variance/SAD (`c_parity_hbd_distortion.rs`) all byte-match real C
  at bd10+bd12. (hbd intra pred + CDEF are the remaining kernel classes.) So
  when the wiring pass lands, a divergence is a WIRING bug, not a kernel bug.
- **Constructor (chunk 1 API shape):** `EncodePipeline::new` (pipeline.rs:128)
  takes NO bit_depth — it is implicitly 8-bit and its planes are `&[u8]`. Add
  bd via a builder mirroring `with_chroma_420` (e.g. `with_bit_depth(bd)` /
  `with_10bit(true)`), NOT a `new` signature change — matches house style and
  keeps churn additive. Store `self.bit_depth`; bd8 keeps every current path.
  NOTE: chunk 1 (the stored field alone) is nearly vacuous — the value is
  chunks 2-4 (u16 plumbing that CONSUMES it), so do not land chunk 1 as a
  standalone "win"; land it together with the first consumer.
- **Harness:** `tools/capture_c_trace/capture_c_trace.c:82` hardcodes
  `cfg.encoder_bit_depth = 8`. A bd10 gate needs an optional 7th arg
  (bit_depth, default 8 → byte-neutral for every existing 6-arg caller in
  identity_diff.sh) AND a 10-bit `.yuv` producer in `identity_run` (u16 LE
  planes). Both are prerequisites for the FIRST bd10 identity cell.
- **PD0 stays u8** (map §hbd_md): build the MSB-truncated 8-bit plane at
  ingestion; `pd0.rs` reads it unchanged. Only MD/recon/filters go u16.

## TXS-COUPLING GATE LANDED (2026-07-19) — the SAMEPART-DIFF tx_depth flips were an 8-bit gate, NOT bd10 precision

**A large subset of the "42 SAMEPART-DIFF = true-bd10-MD" cells were mis-classified.** The
`tx_depth` flips (the target cell `gradient 64 64 20 10`: sole flip `txd C=0/port=1`) are NOT a
bd10 RD-precision decision — they are an **8-bit speed-feature gate** C flips because bd10 forces
`pd0_level = PD0_LVL_0`, exactly the same mechanism as the PD0_LVL_0 partition fix.

**Root cause (traced end-to-end vs real C, read-only `/root/svtav1/Source/`):** the eff-M9 per-SB
TXS coupling in `svt_aom_sig_deriv_enc_dec_allintra` (enc_mode_config.c:8114-8118):
```c
uint8_t txs_level =
    (pcs->txs_level == 0 && ctx->pd0_ctrls.pd0_level == PD0_LVL_6) ? MAX_TXS_LEVEL - 1 : pcs->txs_level;
set_txs_controls(pcs, ctx, txs_level);
```
`set_pd0_ctrls` (enc_mode_config.c:5416) forces `pd0_ctrls.pd0_level = PD0_LVL_0` at bd10 (hbd_md
set) BEFORE this reads it (order: `svt_aom_sig_deriv_enc_dec_common` @ 7096 -> `_allintra` @ 8045).
At eff-M9 `pcs->txs_level == 0`, so the coupling would bump txs to level 5 — but ONLY when
`pd0_level == PD0_LVL_6`. bd10 = LVL_0 => the bump NEVER fires => **TXS off, tx_depth 0 on every
leaf**, where bd8 bumps it to level 5 for undemoted PD0_LVL_6 SBs and codes tx_depth 1 on some.
Those tx_depth-1 leaves were out of the bd10 re-encode envelope (`bd10_reencode_node` asserts
`tx_depth==0`) -> the whole frame fell back to u8 and DIFFERED.

**Fix (commit a901b4862):** force `sb_is_lvl6 = false` at bd10 in `partition.rs`
(`fx.frame.bit_depth != 10 && !pd0_detector_allintra_demotes(...)`), via a new
`FunnelFrame.bit_depth` field. bd8 byte-UNCHANGED (the `!= 10 &&` short-circuit keeps the bd8
expression). Gate `bd10_nonflat_gate.sh` **31 -> 41**: +gradient 64/128 q5/q20/q40 p10/p13
(closes the whole gradient eff-M9 tx_depth cluster) + diag 64 q40 p10/p13 (exercises the
directional `dr_predict_hbd` re-encode arm). Verification: bd8 identity_matrix 54/54, partial_sb
101/101, bd10_matrix 36/36 byte-UNCHANGED; 588-cell bd10 no-panic sweep (incl partial-SB + non-64
dims) 0 panics; encoder unit tests 210/210.

### CORRECTED SAMEPART-DIFF map (post-TXS-fix survey: {gradient,diag} x {64,128} x q{5,12,20,32,40,55} x p{3,6,10,13})

- **gradient eff-M9 (p10/p13): ALL MATCH** — the tx_depth flips were the sole issue; now closed.
- **diag eff-M9 splits into TWO classes** (verified: bd8 diag q5/q12/q20 p10 all byte-MATCH, so
  partition/mode/tx_type are correct at bd8 — the bd10 divergences are bd10-specific):
  - **q20/q32/q55 (p10/p13): MODE flips** — genuine bd10 RD precision. e.g. `diag 64 64 20 10`
    flips LUMA mode DC(0)->SMOOTH(9) on the diagonal-edge 8x8 blocks mi=(4,4)/(8,8)/(12,12) (uv
    follows: uv 0->9). DECISIVE that this is precision, not a gate: the harness input is
    `sample<<2`, so a u8-predicted SATD scales EXACTLY 4x at bd10 and preserves the DC-vs-SMOOTH
    ordering — the flip can ONLY come from the true bd10 recon differing from `recon8<<2` (the
    ~+20/px hbd intra-predictor rounding, docs above) feeding a different prediction into the
    near-tie. At eff-M9 `nic_counts(_, (0,0,0)) = (1,1,1)`, so the winner is dominated by the MDS0
    fast SATD survivor — the flip lives in the fast loop's u8 predict + u8 SATD. Needs the u16 mode
    funnel.
  - **q5/q12 (p10/p13): COEFF divergence, trees IDENTICAL** — a DISTINCT, SMALLER class. tree_diff
    is 0 flips (partition + every mode/uv/txd match C) AND bd8 byte-matches, yet the bd10 tile
    payload diverges (`diag 64 64 12 10`: first differing byte 133 of 469, 336 bytes differ). So it
    is NOT a mode flip and NOT a post-filter header field — it is a **bd10 re-encode coded-LEVEL
    divergence** on some leaf: given matching modes/tx_type, the only sources are the bd10 quant /
    `optimize_b` RDOQ (`tx_unit_hbd`) or an accumulated u16-canvas prediction drift feeding a later
    leaf (the first ~132 bytes / several leaves match, then one diverges). This is a re-encode WIRING
    bug (all kernels FFI-verified), likely more tractable than the mode funnel. NEXT: coeff-level
    dump of the first diverging leaf (map byte 133 -> leaf), compare port `tx_unit_hbd` qcoeffs vs a
    C bd10 coeff dump; check the RDOQ path (rdoq_level, `rdoq_rdmult_full(lambda_bd10)`) and the
    64-dim fold. NOT yet root-caused this session.
- **gradient/diag M6 (p6): mode/uv/fi flips** — same bd10 RD-precision class, plus fi/CFL (M6 runs
  more candidates). NOT the TXS gate (M6 `pcs->txs_level != 0`, so the coupling is inert at both bd).
- **p3 (M3-M5): partition depth-refine + NSQ (bsize flips)** — the PARTFLIP axis, separate.
- **A few pure-BLOCKER2 cells** (trees identical, only a post-filter value differs): CDEF/Wiener bd10.

**REMAINING SCOPE = true bd10 MD (the u16 mode funnel).** The diag/M6 MODE flips need the leaf
funnel's mode RD evaluated at bd10: a u16 recon canvas maintained through the funnel walk
(`encode_fixed_tree` -> `decide_leaf`/`evaluate_leaf`/`commit_leaf`, + the pipeline SB loop) so
each candidate predicts at bd10 (`predict_unit_hbd` from the u16 canvas), its residual/tx/quant/
distortion run at bd10 (`tx_unit_hbd` extended to return dist+bits, `full_distortion_kernel16_bits
<< 4`, bd10 lambda `kf_full_lambda_bd10`), and the MDS0/MDS1/MDS3 winner is picked at bd10. All
kernels exist + are FFI-verified; the work is the u16-canvas plumbing (luma AND chroma — the
joint block cost includes chroma) + the bd10 branches, gated on bit_depth==10 (bd8 byte-UNCHANGED).
Per this file's SURFACE analysis there is no byte-verifiable sub-chunk smaller than "the eff-M9
mode path aligned at bd10 for one cell", so it lands as one reviewed pass. This is the true-bd10-MD
axis the original SAMEPART-DIFF framing named — correct for the MODE flips, wrong (now fixed) for
the tx_depth flips.

## bd10 CHROMA RE-ENCODE LANDED (2026-07-19) — the diag q5/q12 COEFF class was CHROMA, not luma quant/RDOQ

**The "diag q5/q12 = bd10 re-encode coded-LEVEL divergence on some LUMA leaf (quant / optimize_b
RDOQ / 64-dim fold)" framing above (the CORRECTED-SAMEPART-DIFF q5/q12 bullet) was WRONG about the
plane.** Root-caused end-to-end (read-only `/root/svtav1/Source/`) via a NEW C `--wrap` interposer
plus decode-both localization:

**Method (the instrumentation the diagnosis was blocked on):** the existing `SVT_CCOEF_OUT` wrap
(`wrap_recon.c`, on `svt_aom_txb_estimate_coeff_bits`) only fires on the `allow_update_cdf` pass,
which does NOT run at eff-M9 (bypass_encdec, no update_coeff_cdf). Added `SVT_QLEVELS_OUT` — a wrap
of the exported cross-TU `svt_aom_quantize_inv_quantize` (full_loop.c:1649, the FULL MD quant+RDOQ
entry a tx_depth-0 luma leaf reaches via `perform_dct_dct_tx`, product_coding_loop.c:5478) that
dumps the post-quant/post-RDOQ `quant_coeff` levels at ANY preset (optional `SVT_QLEVELS_XY` pin /
`SVT_QLEVELS_COMP` component filter). Clean reusable tool, kept (like `SVT_PD0COST`). Port side:
extended `SVTAV1_PACKTREE_COEFF` to a file-dump-all-leaves mode (a path arg vs the old "r,c" pin).

**The MAP (`diag 64 64 12 10`):**
- C's 22 luma leaves vs the port's 22 (`SVT_QLEVELS_COMP=0` vs `SVTAV1_PACKTREE_COEFF=<file>`):
  **every luma level byte-IDENTICAL.** So `bd10_reencode_luma` + `quantize_b_hbd`/`quantize_fp_hbd`
  are already correct — NOT a luma quant/RDOQ bug.
- `decode-diff` on the two bd10 OBUs: **plane0 (luma) = 0 diffs; planes 1+2 (chroma) diverge** —
  port codes flat **512** (bd10 DC default) where C codes a coded **511** (q5: chroma(0,0), ALL 1024
  px; q12: chroma(6,0), 991 px). C makes ZERO `svt_aom_quantize_inv_quantize` chroma calls (chroma
  quantizes via `svt_aom_full_loop_uv`), confirming the split.

**Root cause:** `bd10_reencode_luma` recomputes ONLY luma levels; **chroma stays at the u8 MD
decision** (`chroma_dec`). On `diag` the subsampled chroma is NOT flat (the diagonal edge), so its
bd10 prediction (the ~+20/px hbd-predictor rounding — same mechanism as the luma mode flips) yields
a small DC residual that quantizes to ±1 at bd10 where the MSB-truncated u8 path rounds to 0.
gradient/uniform chroma IS flat -> DC residual 0 at every bd -> the u8 chroma was coincidentally
correct (why gradient cells matched without a chroma re-encode).

**THE FIX (pipeline.rs `bd10_reencode_chroma` + `_node` + `_plane`):** mirror the luma re-encode on
the U and V planes — predict at bd10 (`predict_unit_hbd` on a running bd10 chroma recon), residual/
tx/quant at bd10 (`tx_unit_hbd` plane 1, the derived `uv_tx_type(uv_mode,cw,ch)`, the bd10 chroma
quant table `build_quant_table_bd(base_qindex,bd)` — chroma qindex == base in mainline), then
OVERWRITE `chroma_dec`. Same coding-order walk as luma (chroma origin/dims = the walk's
`blk_has_uv` derivation). Gated on the SAME complete-SB + `bd10_tree_supported` (now also rejects
UV_CFL_PRED(13) / directional-uv-with-edge-filter -> u8 fallback, `predict_unit_hbd` has no CfL).
The stored u8 recon in `chroma_dec` is inert (only copied into the u8 chroma plane, which no
`chroma_dec` block reads). bd10-gated: bd8 byte-UNCHANGED; flat-chroma content re-encodes to the
SAME zero result so the existing gate cells stay byte-identical.

**Gate `bd10_nonflat_gate.sh` 41 -> 45:** +`diag 64 64 q{5,12} p{10,13}` (the whole eff-M9 diag
COEFF class; each was a chroma DIFF before, `cmp` byte-match after). Verification: bd8
identity_matrix 54/54, partial_sb 101/101, bd10_matrix 36/36 byte-UNCHANGED; bd10 no-panic sweep
(incl. partial-SB + non-64 dims) 0 panics, decodable; encoder unit tests 261/261.

**REMAINING diag scope (still DIFF; Task-2 axis):** `diag {64,128} q{20,32,55} p{10,13}` (LUMA MODE
flips — DC->SMOOTH on the diagonal-edge 8x8s, needs the u16 mode funnel), all `diag 128` (multi-SB
near-ties), `p{3,6}` (depth-refine + M6 fi/CFL). The chroma re-encode also now rides the gradient
q40/q55 cells (previously matched by flat-chroma coincidence) and is the missing "u16 chroma path"
follow-up the luma-landing notes listed — but ONLY the coded-level (skip/non-CfL) subset; the u16
chroma FILTERS (DLF/CDEF/LR) and CfL chroma remain unported.

### Task-2 threading map (u16 LUMA MODE funnel) — TRACED 2026-07-19, NOT attempted (baseline risk)

`diag 64 64 20 10` (post-chroma-fix) is a pure LUMA divergence: `decode-diff` = plane0 1570 px,
chroma 0; the flip is LUMA mode **DC(0)->SMOOTH(9)** at mi=(4,4)/(8,8)/(12,12) (the diagonal-edge
8x8s). At eff-M9 `nic_counts=(1,1,1)`, so the coded MODE == the MDS0 fast-SATD survivor; the flip
lives in `evaluate_leaf`'s fast loop, which predicts each candidate from the **u8** recon canvas and
scores u8 Hadamard SATD. On `sample<<2` content a u8-SATD scales EXACTLY 4x, so the ordering flips
ONLY because the true bd10 recon canvas differs from `recon8<<2` (the hbd-predictor rounding) and
feeds a different prediction into the DC-vs-SMOOTH near-tie.

**Why it is NOT a post-pass (the coupling):** deciding block N's mode at bd10 needs block <N's bd10
recon neighbours, which needs block <N decided+reconstructed at bd10 — a sequential bd10 funnel
walk. A second "re-decide the u8 tree's modes over a bd10 recon" pass is inconsistent (the bd10
recon it would read was built from the u8-decided modes) and cascades wrong after the first flip.

**The exact threading (gated `frame.bit_depth == 10`; bd8 must stay byte-identical):**
- `FunnelCtx` (leaf_funnel.rs:2277) carries `u_recon`/`v_recon: &mut [u8]` (chroma) — add a LUMA
  bd10 canvas. The luma u8 canvas is `y_recon: &[u8]` passed into `evaluate_leaf` (2532) and
  `commit_leaf` (4838); add a parallel `y_recon10: &[u16]` / `&mut [u16]` (frame-luma dims).
- `evaluate_leaf` (2527): in the MDS0 fast loop (`predict_unit` at 2643/2658/2785 + `hadamard_satd`
  at 1043) predict each candidate at bd10 (`predict_unit_hbd` from `y_recon10`) and SATD the bd10
  residual (`y_src10 - pred10`). `y_src10 = encode_input<<2`. The `intra_fast_cost` RATE is
  bd-independent; only the SATD term (`satd<<4`) changes. This alone re-orders the survivor.
- `commit_leaf` (4838): reconstruct the winner at bd10 (`predict_unit_hbd` + `tx_unit_hbd`, exactly
  the `bd10_reencode_node` body) and write its recon into `y_recon10` for the next block's
  neighbours. This is the coupling closure — the per-block bd10 re-encode moves INTO the walk.
- Pipeline SB loop (partition.rs `encode_fixed_tree` :1545 / `funnel_block_decision` :1462 + the
  pipeline caller): allocate + thread `y_recon10` per SB, same lifetime as the u8 canvas.
- MDS1/MDS3 rate/levels can stay u8 for the DECISION (1 survivor -> mode already fixed); the final
  coded LEVELS still come from the existing `bd10_reencode_luma` post-pass (or fold it into
  `commit_leaf` and drop the post-pass). Chroma follows the same idea if a chroma MODE ever flips
  (none observed on `diag` — uv follows luma there).

**Smallest demoable chunk:** bd10 MDS0 SATD + bd10 `commit_leaf` luma canvas, gated eff-M9 bd10; gate
= `diag 64 64 20 10` byte-match AND bd8 identity 54/54 + partial_sb 101/101 + bd10 36/36 + nonflat
45/45 all UNCHANGED. It is one reviewed pass (shared hot path) with no byte-verifiable slice smaller
than "the eff-M9 mode path aligned at bd10 for one cell" — attempt it in isolation and land ONLY on
a clean byte-match; a partial thread that mispredicts silently corrupts luma. NOT attempted this
session (Task-1 chroma landed clean; a speculative funnel thread risks the green baseline).

## u16 LUMA MODE FUNNEL LANDED (2026-07-19) — target `diag 64 64 20 10` byte-exact; gate 45 -> 47

The Task-2 thread above is IMPLEMENTED, exactly as scoped. A TRUE 10-bit luma recon canvas
(`FunnelCtx::y_recon10: Option<&mut [u16]>`) is threaded through the eff-M9 leaf funnel so each
block's MDS0 mode decision is made on the 10-bit recon, not the MSB-truncated u8 recon. bd10-gated;
bd8 and every other preset/partial-SB pass `None` -> the funnel is byte-IDENTICAL (verified).

**Files/functions (leaf_funnel.rs + pipeline.rs):**
- `FunnelCtx.y_recon10` (new field) — the frame-strided 10-bit recon canvas, `Some` only for
  complete-SB (`w%64==0 && h%64==0`) eff-M9 (`preset>=9`) bd10 frames.
- `hadamard_satd_hbd` (new) — u16 mirror of `hadamard_satd`: 10-bit residual (`src<<2 - pred10`),
  same square-tile Hadamard/SATD kernels (bit-depth-independent).
- `evaluate_leaf`: when the canvas is present, the MDS0 fast loop predicts each candidate at 10-bit
  (`predict_unit_hbd` from `y_recon10`) and scores `rdcost(lambda_bd10_fast, flr+fcr, satd10<<4)`
  — re-ordering the survivor (the prune + rates are bit-depth-independent, computed as on the u8
  path). At the end it reconstructs the winner at 10-bit (`predict_unit_hbd` + `tx_unit_hbd`, bd10
  quant table + full bd10 lambda + the frame RDOQ level) into `LeafEval.win_recon10`.
- `commit_leaf`: writes `win_recon10` into the canvas for the next block's neighbours (the
  sequential coupling — the per-block bd10 re-encode moved INTO the walk, exactly as the map said).
- `pipeline.rs`: allocates `tile_frame_recon10` (bd10-gated) before the SB loop and threads it into
  the eff-M9 `FunnelCtx`; the u8 tree + the existing `bd10_reencode_luma`/`_chroma` post-pass (coded
  LEVELS) are UNCHANGED — the funnel only fixes the MODE, the post-pass reads the bd10-decided modes.

**THE FAST LAMBDA (root-caused vs REAL C, not guessed):** C's fast loop calls `av1_intra_fast_cost(
.., fast_lambda_md[1], satd<<4)`, and the port's `rdcost(λ, rate, satd<<4)` has the IDENTICAL
structure, so the port's fast lambda must be C's `fast_lambda_md[1]` EXACTLY. Added `lam=` to the
`SVT_FASTCOST_OUT` interposer (`wrap_recon.c`) and read it: `fast_lambda_md[1] == kf_full_lambda_bd10
/ 16` (the value BEFORE md_process.c's `full_lambda_md[1] *= 16`; integer-exact since `*16` adds no
low bits) — verified 22505@q20, 94716@q32, 2053848@q55 all match the real C. (A bd10 coincidence of
the rdmult-vs-SAD tables under the ×16-vs-×4 scaling; the u8 path keeps `frame.lambda`.) Proof it
WORKS: `diag 64 64 20 10`'s `tree_diff` goes from 3 flips -> the SMOOTH(9) flips at mi(4,4)/(8,8)/
(12,12) all match C, and block (0,0)'s 3 candidate fast costs match C to the UNIT (verified via
`SVT_FASTCOST_OUT`).

**Gate `bd10_nonflat_gate.sh` 45 -> 47:** +`diag 64 64 20 {p10,p13}` (the documented DC->SMOOTH/V
flip — a LUMA MODE flip DIFF before, `cmp` byte-match after). Verification: bd8 identity_matrix
54/54, partial_sb 101/101, bd10_matrix 36/36 byte-UNCHANGED; encoder unit tests 210/210; 135-cell
bd10 no-panic sweep (incl. partial-SB 96/200/72 + non-64 dims) 0 panics, all decodable.

### REMAINING (still DIFF) — a SEPARATE, PRE-EXISTING bd10 recon-precision cascade (NOT the funnel)

`diag {64,128} q{32,55} p{10,13}` still DIFF. The funnel is CORRECT — the blocker is that the shared
bd10 recon path drifts from C on **dense coded content at coarse qindex**, and the funnel (reading
that recon as neighbours) surfaces it as a mode flip. DECISIVELY ATTRIBUTED (dumped luma levels with
the funnel FORCE-DISABLED via a temporary env gate, then reverted):
- With the funnel OFF (u8 modes), `diag 64 q55` STILL diverges — **14/22 luma blocks DIFF from C**,
  OBU diverges at the same byte 12. So the divergence is pre-existing, independent of the funnel.
- The signature: for the DC-mode blocks the **AC coeffs match C, only the DC coefficient drifts**
  (e.g. mi(0,2): port DC=4 vs C DC=0). DC_PRED is flat, so the residual AC is prediction-independent
  (always matches); the residual DC = `src_DC - pred_DC`, so a drifting DC coeff means the **DC
  prediction average (of the neighbour recon) drifts from C**. The forward quant is verified CORRECT
  (mi(0,0) levels byte-match C exactly, `SVT_QLEVELS_OUT` vs `SVTAV1_PACKTREE_COEFF`); the drift is a
  recon cascade: the 10-bit recon of a dense/coded neighbour differs from C by a small amount, the
  next block's DC average picks it up, and on a 32x32 block it accumulates enough to flip the mode
  (port satd10_DC = 141056 vs C's 51456 -> port avoids DC, picks V/SMOOTH which don't use the drifted
  left neighbour). q20 byte-matches because there the recon does NOT drift (the funnel fixes it).
- **This is a bd10 RECON precision item (inv-tx/dequant/recon cascade in `tx_unit_hbd`, shared by the
  post-pass), NOT a mode-funnel item.** It also blocks `diag 128` (multi-SB) and every real-content
  coarse-q cell. NEXT: dump C's true 10-bit recon (make `SVT_RECON_BIN` 16-bit-aware) and diff it
  against the port's `tile_frame_recon10`/`last_recon10_y` to pin the first diverging pixel/block.
  The p3/p6 depth-refine + M6 fi/CFL axes remain separate (as before).

## AVX2-HADAMARD ROOT CAUSE — the "recon-precision cascade" was NOT recon at all (2026-07-19)

**The section immediately above is WRONG about the mechanism** (kept for the trail). The residual
`diag {64,128} q{32,55} p{10,13}` divergence was NOT an inv-tx / dequant / recon-add / DC-predictor
precision cascade. It was the **MDS0 fast-loop SATD kernel**: the port was ported from
`svt_aom_hadamard_{16x16,32x32}_c`, but the encoder binds those RTCD pointers to the **AVX2**
implementations, and the two are **not equivalent above the 8-bit residual range**.

### How it was localized (method, in order)

1. `SVT_RECON_BIN` made 16-bit-aware (`wrap_recon.c`: at `is_16bit` the recon is plain packed u16,
   `buffer[p]` pre-offset to the frame origin, `stride[p]` in SAMPLES) and diffed against the port's
   existing `SVTAV1_BD10_RECON` dump of `last_recon10_y`.
   -> `diag 64 64 32 10`: 691/4096 luma pixels differ, **all inside the single 32x32 block at
   (x=32,y=0)**; the other three 32x32 blocks are byte-identical. So the neighbours of the diverging
   block were CORRECT — which already refutes "a drifting neighbour recon cascade".
2. Per-block levels (`SVT_QLEVELS_OUT` vs the port's re-encode dump): every AC level identical, only
   the DC differs (C `qcoeff[0] = -110`, port `-19`) — the documented "AC matches, DC drifts"
   signature. But the coded TREE showed the real cause: **C codes mode 0 (DC_PRED) at (32,0), the
   port codes mode 1 (V_PRED)** — a MODE flip, not a recon drift. Both predictions are flat (the
   block is at the frame top, so `has_above = false`), which is exactly why every AC coefficient
   agreed and only the DC moved.
3. `SVT_FASTCOST_OUT` extended to dump the candidate PREDICTION and the branch selectors:
   `pred0/pred1/predS/predmean` prove C's DC pred is flat **752** and V flat **554** — *identical to
   the port's* (`predict_unit_hbd` is correct; the left column feeding it is byte-identical). The
   selectors show `dtype=0 (SAD)`, `hadblk=1`, `subres=0`, i.e. C takes `hadamard_path` with
   `luma_fast_dist = satd << 4` (`fast_loop_core`, product_coding_loop.c:1283-1287).
4. The wrap also dumps `cand_bf->residual->y_buffer` (which `hadamard_path` fills just before the
   SATD): C's residual is **exactly** `src10 - 752` (`res0=-240 res1=-256 resmean=-240.00
   resmin=-736 resmax=256`) — identical to the port's. Same source, same prediction, same residual,
   same kernel... yet C reported `rawsatd = 49152` where the port computed **143360**.
5. Feeding that exact residual to the REAL C `svt_aom_hadamard_32x32_c` via `cref` returned
   **143360** — matching the PORT, not the encoder. So the encoder was not running `_c`. Calling
   `svt_aom_hadamard_32x32_avx2` on the same residual returned **49152 / 52480**, reproducing the
   encoder's DC/V distortions EXACTLY. Root cause identified.

### The defect

`SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2)` / `SET_AVX2(svt_aom_hadamard_16x16, ...)`
(common_dsp_rtcd.c:1047-1048) bind the MD fast loop to the AVX2 kernels, which were written for
8-bit residuals (their own comment: `// src_diff: 9 bit, dynamic range [-255, 255]`):

- `_c` carries the 8x8 sub-results into the 16x16 cross-combine as `int32_t`, and the 16x16
  sub-results into the 32x32 combine as `int32_t`; nothing after the 8x8 stage can wrap.
- `_avx2` keeps BOTH stages in 16-bit lanes: the 16x16 combine is `_mm256_{add,sub}_epi16` +
  `_mm256_srai_epi16` (wrapping), and `svt_aom_hadamard_32x32_avx2` buffers its four 16x16
  sub-transforms in an `int16_t temp_coeff[32*32]` (`is_final = 0`,
  pic_operators_intrin_avx2.c:1721-1732) before sign-extending to 32-bit, doing the `>> 2` in
  32-bit, SATURATING back to 16-bit (`_mm256_packs_epi32`) and finishing with wrapping 16-bit
  add/sub.

At 8-bit the 16x16 stage spans [-32640, 32640] and the post-shift 32x32 operands span
[-16320, 16320] — no wrap or saturation is reachable and the kernels agree bit-for-bit (which is why
every 8-bit gate was unaffected and why the 8-bit fuzzers never caught it). At **10-bit** the 16x16
stage reaches ~+/-130560 and the AVX2 kernel WRAPS. The bd10 mode funnel (`hadamard_satd_hbd`) feeds
exactly such residuals, so the port produced a different MDS0 fast cost and a different winner.

### The fix

`crates/svtav1-dsp/src/hadamard.rs`: `aom_hadamard_16x16` / `aom_hadamard_32x32` re-ported from the
**AVX2** kernels (wrapping i16 combine; i16-buffered sub-transforms, i32 add/sub + `>> 2`,
saturating narrow, wrapping i16 finish). **bd8 byte-UNCHANGED by construction** — at 8-bit the values
cannot reach the wrap/saturate thresholds, so the new code is bit-identical to the old there (and the
existing `_c` 8-bit parity tests still pass unmodified).

Differential (`tests/c_parity_hadamard.rs`, 9 tests): the port is pinned against `_c` over the 8-bit
range (unchanged) AND against `_avx2` over BOTH the 8-bit and the new **bd10 (-1023..1023)** ranges,
plus `c_and_avx2_hadamard_diverge_at_bd10_range` — an anti-vacuity witness that fails if upstream
ever unifies the two kernels. NOTE the AVX2 kernels emit coefficients in a different ORDER ("the
final transpose may be skipped"), so parity is asserted on the coefficient MULTISET + the SATD —
strictly stronger than the SATD alone, and exactly the invariant `svt_aom_satd` depends on.
`cref::hadamard_avx2` was added for this.

### Result

`tools/bd10_nonflat_gate.sh` **47 -> 79** byte-identical cells. The whole eff-M9 bd10 envelope is
closed: the full sweep `{diag,gradient} x {64x64,128x128} x q{5,12,20,32,40,48,55,63} x p{10,13}` is
**64/64 with ZERO diffs** — including every `diag 128` cell (previously written off as "multi-SB
near-ties") and the q48/q63 cells that were never gated. bd8 `identity_matrix` 54/54,
`partial_sb_gate` 101/101, `bd10_matrix` 36/36 all byte-UNCHANGED; encoder 210 + dsp unit tests green.

### Remaining scope (unchanged by this landing)

- **Low presets p0/p3/p6 at bd10**: 2/36 match on the spot sweep (only the two `gradient 64 64 55
  {p3,p6}` cells already gated). Still blocked by the separate, previously-documented axes — PD1
  depth-refinement + NSQ (p3-p5), M6 fi/CFL, and the bd10 CDEF-search / Wiener-LR post-filter
  dependencies. NOT a hadamard issue.
- **PRE-EXISTING 65x65 failures (found by this session's 360-cell no-panic sweep; NOT caused by this
  change — verified by re-running the identical sweep with the fix stashed, failure sets IDENTICAL):**
  `65x65 p0` at q32/q63 PANICS in `txb_ctx_from_spans` (leaf_funnel.rs:5417) — and it panics at
  **bd8 too**, so it is a general partial-SB bug, unrelated to bd10; `65x65 bd10 p0/p6` at q5/q32
  additionally produce UNDECODABLE streams. Excluding 65x65 the sweep is 353/353 clean and decodable
  (aomdec). Neither is covered by `partial_sb_gate.sh` (101/101 green).

---

## bd10 x HDR-FORK mode (2026-07-19)

Fork mode (`SVT_HDR_MODE=1`) at bd10 is now byte-measurable against a real C
oracle and is **46/64** byte-identical (was 0/64). Full write-up, including the
oracle build and the two roots fixed, is in **`docs/HDR-ON-4.2.md` § "Fork mode
at 10-bit"**. Gate: `tools/hdr_bd10_gate.sh` (46/46).

Two bd10 findings from that work belong here because they are **not**
fork-specific:

1. **`qm::quantize_fp_hbd_qm` / `quantize_b_hbd_qm`** now exist — the bd10
   re-encode (`tx_unit_hbd`) previously had NO quantization-matrix support at
   all (non-QM highbd kernels, `iwt: None` into the trellis). Any future bd10
   work that turns QM on (`--enable-qm`, `tune=IQ`, `tune=SSIMULACRA2`) needs
   these, mainline or fork.
2. **`var_boost::convert_qindex_to_q_fp8` / `compute_qdelta_fp` take a bit
   depth.** C changes both the qlookup table and the shift per depth
   (`ac_quant_qtx(q,0,bd) << 6` at 8-bit, `<< 4` at 10-bit, `<< 3` at 12-bit).
   These two are the ONLY bit-depth entry points in the variance-boost chain;
   everything else there runs on the 8-bit MSB luma plane at every depth
   (C creates the picture-analysis reference at `EB_EIGHT_BIT`,
   `reference_object.c:246`), so its 8-bit-domain constants are correct at bd10
   and must NOT be rescaled.

**`hbd_md` remains the open "true bd10 MD" axis** and is a MAINLINE knob, not a
fork one (v4.2 MR !2644; the fork's duplicate field was dropped in the rebase).
C's ladder (`enc_mode_config.c:2152-2165`) yields 0/1/2 by preset and bit depth
and switches source/recon buffer selection, distortion-kernel dispatch
(`svt_full_distortion_kernel16_bits` vs the 8-bit form), the dequant table, the
lambda shift (`>> (2*(bit_depth-8))`), and whether LPD1 is reachable at all.
The port's u8-search + 10-bit-re-encode strategy models none of that, which is
the same gap already described above as "true bd10 MD".

---

## LOW-PRESET (M0/M3/M6) failure MAP + PHOTOGRAPHIC bd10 (2026-07-19)

Two measured results, both of which **overturn a premise recorded above**.

### 1. PHOTOGRAPHIC bd10 at eff-M9 is BYTE-IDENTICAL — 94/94 (was "18/18 diverge")

The "Photographic sweep ... 18/18 BLOCKER1_part. On real content the partition
tree ALWAYS diverges at bd10" result in the FALL-BACK MAP section is **STALE**.
It was measured before four landings (PD0_LVL_0, the TXS-coupling gate, the bd10
chroma re-encode, the AVX2-hadamard fix). Re-measured with all four in:

- **94/94 byte-identical** across **12 distinct CID22-512 photographs** x
  q{5,12,20,32,40,55,63} x p{10,13}. ZERO diffs.
- Gated: **`tools/bd10_photo_gate.sh`** (94 cells, ~2m50s). Corpus
  `BD10_PHOTO_CORPUS` (default CID22-512); **fails loudly if absent**, never
  skips silently.

So real 10-bit photographic content is fully byte-exact at eff-M9. The bd10 gap
is now a **low-preset** gap, not a real-content gap.

### 2. The low-preset failure MAP — the CDEF/LR axis owns ONE cell, not the bulk

Sweep `{gradient,diag} x {64x64,128x128} x q{12,20,32,40,55} x p{0,3,6}` = 60
cells, classified by `tools/bd10_classify.sh` (new; see its header for the
classifier contract). Every DIFF is assigned its **upstream-most** axis, because
a partition/mode flip changes the recon and therefore ALSO perturbs the
CDEF/LR frame header — that FH difference is a downstream symptom, not evidence
of an FH bug. Only a cell whose tree AND tile payload are byte-identical proves
the post-filter search itself diverges.

| axis | p0 | p3 | p6 | total |
|---|---|---|---|---|
| **PART** (geometry flip: PD1 depth-refine + NSQ) | 13 | 9 | 0 | **22** |
| **MODE** (geometry identical; mode/uv/fi/txd flip) | 6 | 10 | 19 | **35** |
| **FH** (tree AND tile payload identical: CDEF/LR search) | 1 | 0 | 0 | **1** |
| MATCH | 0 | 1 | 1 | 2 |

**The working hypothesis that "CDEF/LR search dominates at p<=6 since it changes
the frame header on every cell" is REFUTED.** 36 of 58 DIFF cells do show an FH
difference, but in 35 of those the tree also flips, so the FH value is
downstream. Exactly **one** cell (`gradient 64x64 q55 p0`, `cdef_y_pri_strength`
C=4/port=5) is a pure post-filter divergence — the same single cell the FALL-BACK
MAP already identified as the smallest blocker-2 cell.

**Structure worth keeping:**
- **p6 has ZERO partition flips** (19/19 MODE, geometry identical on every cell)
  — confirming the `LVL_0 == LVL_1` note above. p6 is a pure MODE axis.
- **p0/p3 are partition-dominated** (22 of 29 DIFFs), i.e. PD1 depth-refine + NSQ.
- The p6 MODE flips are `fi` (filter-intra DC vs DC_PRED near-tie), `mode`
  (DC(0) <-> SMOOTH(9)/V(1)) and `uv`/`txd` — few flips per cell (1-9).
- **Photographic at p6 is PART, not MODE**: 9/9 cells (3 images x q{12,32,55})
  diverge on partition geometry. So real content at p6 needs the depth-refine
  axis too; the synthetic p6 grid understates the work.
  > **SUPERSEDED 2026-07-19 — the second half of that sentence is WRONG.** The
  > classifier label is right (geometry does flip) but the ROOT is not the
  > depth-refine axis: at p6 the tree IS the PD0 tree, C's PD0 for SB(0,0) is
  > bit-depth-IDENTICAL, and the port's p6 geometry inside SB(0,0) matches C
  > EXACTLY (0 bsize flips, 0 port-only keys). The geometry flips in later SBs
  > are a CDF-chain cascade from a **CfL mode flip** in SB(0,0) (C codes
  > `UV_CFL_PRED` on 7/28 blocks there, the port on 0). See "bd10 PART AXIS
  > LANDED" at the end of this file; the fix is the bd10 CfL arm, not a
  > partition change.

### 3. MEASURED NEGATIVE — widening the u16 MDS0 mode funnel to M6..M8 closes NOTHING

Tried and REVERTED (recorded so it is not re-tried): extending `bd10_luma_funnel`
from `preset >= 9` to `preset >= 6` and passing the 10-bit canvas into the
M6-M8 `FunnelCtx` (M6..M8 share eff-M9's `encode_fixed_tree` walk, so it wires up
in ~10 lines). Result: **p6 stays 19/19 DIFF** — no cell closed. The existing
gate stayed 79/79 (the change is byte-neutral there), and a few p6 payload sizes
shifted by 1-5 bytes, proving the canvas WAS live; it just does not decide the
winner.

**Why:** at eff-M9 `nic_counts = (1,1,1)`, so the MDS0 fast-SATD survivor IS the
coded mode and a bd10 MDS0 is sufficient. At M6 `nic_num = (6,6,6)` — many
candidates survive MDS0 and the winner is chosen by the **MDS1 / MDS3 full-RD**
stages (`leaf_funnel.rs:3579` MDS1, `:3802` MDS3), which still run u8 prediction,
u8 `tx_unit`, the u8 quant table and the bd8 lambda.

**So the p6 MODE axis needs the full-RD stages at bd10, not just MDS0.** Concrete
prerequisite discovered while scoping: **`tx_unit_hbd` returns no `dist` and no
`bits`** (`TxUnitOutHbd` = `{eob, qcoeff, recon, cul}`) — it is a level-producing
core, not an RD unit. Threading bd10 into MDS1/MDS3 therefore requires FIRST
extending it with the freq/spatial distortion + coeff-rate model that u8
`tx_unit` already returns, then carrying a per-candidate `pred10` (MDS0 already
computes one, it is currently discarded), then the MDS3 TXS/TXT/RDOQ/chroma
interactions (~1150 lines). This is the "true bd10 MD" pass the map calls
un-dribbleable; the MDS0-only shortcut is now measured to be insufficient.

### Remaining scope per axis (honest)

- **MODE (35 cells; 19 at p6, all geometry-clean)** — the largest and, at p6, the
  structurally simplest target: extend `tx_unit_hbd` with dist+bits, thread
  `pred10`, run MDS1+MDS3 at bd10 with `lambda_bd10_full` + the bd10 quant table.
  One reviewed pass; land only on a clean byte-match (a partial thread
  mispredicts silently).
- **PART (22 cells at p0/p3, plus ALL photographic p6)** — bd10-aware PD1
  depth-refine + NSQ over the PD0 tree. Largest real-content impact below M9.
- **FH (1 cell)** — `coeff_shift=2`, bd10 lambda in `finish_cdef_rd`, and the
  bd10 recon (`last_recon10_y`) threaded into `cdef_search_still` /
  `filter_fb_packed` (which today writes a u8 dst and hardcodes `coeff_shift=0`).
  **CORRECTION to the BLOCKER 2 section above: the u16-dst CDEF filter is ALREADY
  ported AND FFI-verified** — `svtav1_dsp::cdef::cdef_filter_block` (the `dst16`
  arm of `svt_cdef_filter_block_c`) plus `hbd::cdef_filter_block_hbd`, pinned by
  `c_parity_cdef.rs::filter_block_randomized_wide`, which exercises
  `coeff_shift = 2` explicitly. So axis 3 is pure WIRING, not a kernel port. Note
  a general fix also needs the bd10 **DLF** first (the CDEF search consumes the
  post-deblock recon); the one open cell happens to have `lf_level == 0`, so
  `last_recon10_y` IS its post-deblock recon. Bounded, but it unblocks only this
  one cell until the MODE and PART axes land (every other FH difference is
  downstream of a tree flip).

### 4. The eff-M9 BAND was under-gated — presets 9/11/12 were already closed

The two bd10 gates were pinned on presets **10 and 13** only, leaving the rest of
the eff-M9 band unmeasured. Measured this session, all byte-identical:

- Synthetic `{gradient,diag} x {64,128}^2 x q{5,20,40,63} x p{9,11,12}` = **48/48**.
- Photographic 3 x CID22-512 x q{12,40} x p{9,11,12} = **18/18**.

No product code changed — these cells already matched. Both gates now cover the
whole band, so a regression anywhere in p9..p13 is caught rather than slipping
between the two pinned presets:
- `tools/bd10_nonflat_gate.sh` **79 -> 127** byte-identical cells.
- `tools/bd10_photo_gate.sh` **94 -> 112** byte-identical cells.

**Net bd10 position: the eff-M9 band (p9..p13) is CLOSED for both synthetic and
real photographic content at bd10. Everything still open is p0..p8.**

## bd10 FULL-RD MODE FUNNEL LANDED (2026-07-19) — p7/p8 CLOSED (40/40), p6 MODE class 19 -> 7

The "MODE (35 cells; 19 at p6)" axis from the low-preset map above. Landed
exactly as that section scoped it: `tx_unit_hbd` extended with dist+bits, a
per-candidate `pred10` threaded, and MDS1 + MDS3 run at bd10.

### The mechanism (unchanged from the scoping, now implemented)

At eff-M9 `nic_counts == (1,1,1)`, so the coded mode IS the MDS0 fast-SATD
survivor and a bd10 MDS0 suffices — which is why p9..p13 was already closed.
At **M6..M8 `nic_counts == (6,6,6)`**: MDS0 only shortlists, and the winner
comes from the **MDS1 + MDS3 full-RD** stages, which ran at 8 bits. The port
therefore coded C's *bd8* winner. Measured at `gradient 64x64 q40 p6`
mi=(0,0): four candidates reach MDS3 and the u8 compare puts
`mode=0 fi=FILTER_DC` at 241322877 vs `mode=0 fi=NONE` at 243339291 — a 0.83%
margin that inverts at 10 bits (C codes `fi=NONE`).

### What runs at 10 bits now (`FunnelCtx::full_rd10`)

- **MDS1** — 10-bit residual from `cand.pred10`, bd10 quant table, bd10
  lambda, and the freq-domain distortion (`svt_aom_picture_full_distortion32_
  bits_single` is bit-depth-INDEPENDENT, so the u8 expression is reused on the
  10-bit coefficients). This drives both the sort and the NIC pruning.
- **MDS3 luma** — the whole TXS depth loop: `txt_search` gained a bd10 arm
  (every gate around it — group order, ext-tx set, rate-cost th, SATD early
  exit, non-signalable-eob — is bit-depth-independent, so only the cost source
  switches), a `dep_recon10` carries the intra-block coupling, and
  `predict_unit_overlay_hbd` predicts deeper-depth txbs from the 10-bit canvas.
  The spatial distortion is `svt_full_distortion_kernel16_bits << 4`, which is
  what `svt_spatial_full_distortion_kernel_facade` (pic_operators.c:257)
  dispatches at `hbd_md != 0`.
- **MDS3 chroma** — `chroma_eval10`, predicting from new bd10 chroma canvases
  (`FunnelCtx::u_recon10`/`v_recon10`, committed by `commit_leaf` exactly like
  the luma one). This is NOT optional: the MDS3 block cost is JOINT, so with
  luma at 10 bits and chroma at 8 the chroma term would be ~16x
  under-weighted and every uv-follows-luma mode flip would be decided on luma
  alone.
- One `lambda3` substitution covers every MDS3 rdcost (depth compare, txb
  early exits, final block cost) — C uses `full_lambda_md[hbd_md ? 1 : 0]`
  throughout.

### Results (MEASURED, not inferred)

| band | before | after |
|---|---|---|
| p7/p8 (`{gradient,diag}` x `{64,128}` x q`{12,20,32,40,55}`) | **12/40** | **40/40** |
| p6 (same grid) | 1/20 | 4/20 |

The p7/p8 baseline was measured by re-running the identical sweep with the
change stashed — 28 cells are newly closed, not pre-existing. **p7/p8 had
never been gated at all** (the gate jumped p6 -> p9), so that band was
closed-but-unmeasured on one side and open on the other.

`tools/bd10_nonflat_gate.sh` **127 -> 170** (all 40 p7/p8 + the 3 new p6).
Unchanged: identity_matrix 54/54, partial_sb_gate 101/101, bd10_matrix 36/36,
bd10_photo_gate 112/112, hdr_bd10_gate 46/46, `cargo test --workspace` green,
135-cell bd10 p6/p7/p8 panic sweep (incl. partial-SB 96/200/72) 0 failures.

### p6 residual — the axis mix CHANGED, which is the useful part

p6 went from **19 MODE cells** to **7 MODE + 9 COEFF + 2 FH**. The 9 COEFF
cells now have a **byte-identical coded tree** (0 partition/mode/uv/tx flips)
AND an identical tile-payload SIZE — only the coefficient bytes differ. So the
p6 mode decision is largely correct now and the residual has moved one layer
down. Remaining p6 DIFFs by axis:

- **COEFF (9)** — `gradient` q20..q55 at both sizes, `diag 64 q55`. Trees
  match; the coded levels do not.
- **MODE (7)** — all `diag` at q20..q55. Still a genuine mode/uv flip.
- **FH (2)** — CDEF strength; the known post-filter axis.

### MEASURED NEGATIVE — do not re-try: funnel levels do NOT beat the post-pass

The bd10 full-RD also computes 10-bit coded levels, and it computes them with
each txb's **real** `txb_skip_ctx`/`dc_sign_ctx`, whereas the level-only
re-encode post-pass (`bd10_reencode_luma`/`_chroma`) hardcodes the RDOQ
contexts to **0/0** — correct only where `real_coeff_ctx` is off (eff-M9,
rate_est_level 0), and M6 has it ON. Replacing the post-pass with the funnel's
levels was therefore expected to close the COEFF class outright.

It was implemented and **A/B measured on the p6 grid: 4/20 with the post-pass,
3/20 without** (`gradient 64x64 q12` regresses into a CDEF-strength
divergence). So the post-pass stays authoritative for the coded levels and the
COEFF class is NOT simply the RDOQ-context gap. The funnel's 10-bit levels do
stay live where the post-pass does not reach — the neighbour `cul` bytes that
drive later blocks' coefficient contexts, and the u8 chroma recon the CDEF/LR
searches read (`recon10 >> (bd-8)`, the convention the post-pass established).

### COEFF class — TRACED (2026-07-19), and it is NOT a level bug

Done on `gradient 64x64 q40 p6` (264B payload, 4 blocks, RDOQ level 3):

1. `decode-diff` on the two OBUs: **plane 0 only** (chroma 0 diffs), first
   differing pixel `x=4 y=0`, i.e. the very first block, 1553/4096 luma px.
   (`gradient 64 q20` behaves identically: luma-only, SB(0,0), 2135 px.)
2. `SVT_QLEVELS_OUT` + `SVT_QLEVELS_COMP=0` vs `SVTAV1_PACKTREE_COEFF` on
   block mi=(0,0): C emits **25 records at org=(0,0)** — one per candidate x
   tx_type x depth, all `enc=0` (there is no encode-pass marker to filter on).
   Nine are `txs=3 txt=0`; they fall into two families, `eob=996` (records
   1-5) and `eob=528` (records 6-9).
3. **The port's coded levels are byte-identical to C's `eob=528` family** —
   `yeob=528`, `0:-76, 2:-1, 16:1, 32:-25, 33:-1, 34:-4, 36:2, 37:-1, 40:-1,
   46:-1, 64:2, ...` matches records 6-9 exactly, coefficient for coefficient.

So the port's forward quantize + RDOQ for this txb reproduce a real C result
exactly. **CAUTION for the next session: an earlier draft of this section
claimed a "trellis-shaping difference (C 32:-26 vs port 32:-25,33:-1)". That
was WRONG — it compared against record 2, a LOSING candidate.** Do not chase
the quantizer on this evidence.

That leaves two live hypotheses, in order:
- **(a) Winner identification.** C may actually code the `eob=996` family and
  the port matches a losing candidate. Distinguish by extending the
  `SVT_QLEVELS_OUT` wrap with the candidate/stage identity (or by adding an
  `is_encode_pass`-equivalent marker), so the winning record is unambiguous
  rather than guessed from dump order.
- **(b) Write-side contexts** (the KB-6 class from the sibling aom-rs port):
  identical symbols written on different-probability cdf rows desync the
  parse. Both the tile payload SIZE and the coded tree already match, which
  fits this shape.

Measured NOT to be the cause: the neighbour `cul` source. Routing the MDS3
`cul` bytes from the 10-bit levels vs the 8-bit ones was A/B'd on the whole p6
grid and is decision-NEUTRAL (4/20 either way); the 10-bit form is kept
because it is consistent with the levels the same stage computes.

### Scope deliberately NOT taken in this landing

- **CfL at bd10.** The two CfL arms inside MDS3 (`cfl_rd_search`, the non-CfL
  freq re-run, the chosen-alpha chroma TX) are 8-bit only. Rather than feed an
  8-bit CfL compare into a 10-bit block cost, the CfL candidate is simply **not
  offered** under the bd10 full-RD, so a CfL block stays a VISIBLE mode
  divergence instead of a silent mixed-domain decision (the re-encode's
  `bd10_tree_supported` already rejects UV_CFL_PRED for the same reason).
  MEASURED: the chroma complexity detector never arms on the p6 bd10 grid
  (`cfl_would_run == false` on every block of every gradient/diag cell), so
  nothing gated here is affected. The kernels a port would need
  (`cfl_luma_subsampling_420_hbd`, `cfl_predict_hbd`) already exist and are
  FFI-verified.
- **eff-M9 (p9..p13) is excluded from `bd10_full_rd_supported`** — that band is
  CLOSED (170-cell gate) via the MDS0 funnel + post-pass and is left byte-for-
  byte as it was. Widening the full-RD to it is plausible (with
  `nic_counts == (1,1,1)` a single candidate reaches MDS3, so it should be
  decision-inert) but it MUST be re-verified against the whole gate first.
- **presets 0..=5** are the PART axis (PD1 depth-refine + NSQ), unrelated.
- **Palette** is excluded (a palette candidate has no 10-bit prediction here);
  `palette_level == 0` for M6..M8 so this is not currently binding.
- **ac-bias / noise-norm** (fork-only) are not applied in `tx_unit_hbd` — they
  need a u16 psy kernel that is unported.

## bd10 PART AXIS LANDED (2026-07-19) — the PD1 walk runs at 10 bits; p0..p5 gated

The "PART (22 cells at p0/p3, plus ALL photographic p6)" axis from the
low-preset map. **Two DIFFERENT roots wear the PART label**, and separating
them is the main result of this pass. Root 1 is fixed; root 2 is measured and
is NOT a partition bug at all.

### Root 1 — p0..p5: C's PD1 decides the geometry with 10-bit leaf costs (FIXED)

`svt_aom_pick_partition` compares whole partitions, and both comparators sum
**leaf block costs**:

- `test_depth` (product_coding_loop.c:10857): `part_cost = RDCOST(full_lambda,
  part_rate, 0)` then `+= block_data[shape][nsi]->cost` per NSQ sub-block.
- `test_split_partition` (:10770): `split_cost = RDCOST(full_lambda,
  above_split_rate, 0)` then `+= split[i]->rdc.rd_cost`.

Both select `full_sb_lambda_md[EB_10_BIT_MD]` when `hbd_md != 0` (:10782,
:10887), and PD1 runs at `hbd_md = 2` at every allintra preset. Those leaf
`cost`s come from an MDS3 that predicted, quantized and measured distortion at
**10 bits**. The port ran the whole `decide_sb_refined` walk on 8-bit leaf
costs with the 8-bit lambda, so at bd10 it reproduced C's *bd8* geometry.

**What is NOT bit-depth dependent here — measured, and the reason the fix is
one substitution rather than a new algorithm:**

1. **The PD0 pass.** At p0..p2 bd8 `pic_pd0_lvl == 0` (`set_pic_pd0_lvl_default`,
   enc_mode_config.c:8613) and bd10 forces `PD0_LVL_0` (`set_pd0_ctrls`,
   :5415) — the same level; and PD0 runs at `hbd_md = 0` on the MSB-truncated
   plane either way. VERIFIED: `SVT_PD0COST_OUT` for `diag 64x64 q12 p0` dumps
   **89 records at bd8 and 89 at bd10, byte-identical** (`diff` empty).
2. **The depth-refinement scan.** `perform_pred_depth_refinement` is called at
   enc_dec_process.c:3017, INSIDE the window where `hbd_md` is forced to 0
   (:2965) and restored only at :3023. So `is_parent_to_current_deviation_small`
   / `is_child_to_current_deviation_small` take `full_lambda_md[EB_8_BIT_MD]` /
   `full_sb_lambda_md[EB_8_BIT_MD]` at BOTH bit depths, over PD0 costs that are
   themselves identical. **The scan must stay 8-bit — using the bd10 lambda
   there would be a bug.** The asymmetry (8-bit scan, 10-bit walk) IS the fix.

**Landed** (`c171a18af`): `bd10_full_rd_supported` widened from presets 6..=8 to
0..=8; the p0..=5 `refined` `FunnelCtx` gets `y_recon10`/`u_recon10`/`v_recon10`
+ `full_rd10`; `decide_sb_refined` gets `kf_full_lambda_bd10` (which also feeds
`update_skip_nsq_based_on_split_rate` :9725 and
`update_skip_nsq_based_on_sq_recon_dist` :9859 — both ratio-compare an
`RDCOST(λ, rate, 0)` term against a block cost, so the lambda must move WITH
the costs). bd8 is structurally untouched (every branch is gated on
`bd10_full_rd`, which requires `bit_depth == 10`).

**Measured** on the classifier grid (`{gradient,diag} x {64,128} x
q{12,20,32,40,55} x p{0,3}`, 40 cells): **PART 18 -> 9**, and **13 cells became
COEFF** (coded tree AND every mode field byte-identical; only coefficient bytes
differ). `diag 64x64 q12 p0` went from 2 port-only geometry keys plus bsize
flips (C 16X16 vs port 8X16, C 16X32 vs port 8X32 — the port over-splitting
into narrower NSQ shapes) to **0 geometry diffs**.

**Gated** (`7e7c7a2f6`): `bd10_nonflat_gate.sh` **170 -> 180**. Six cells are
newly closed and A/B-MEASURED (rebuilt with the change reverted: all six DIFF
before, MATCH after) — `gradient 64x64 q12` at p2/p3/p4/p5, `diag 64x64 q20 p4`,
`diag 128x128 q20 p4`; four more (`gradient 64x64 q55` at p1/p2/p4/p5) were
already byte-exact but the band had never been gated at all. The gate also now
asserts **aomdec decodability** on every cell before consulting `cmp`.

### Root 2 — photographic p6 "PART" is a CfL MODE flip that CASCADES (NOT a partition bug)

The low-preset map recorded "Photographic at p6 is PART, not MODE: 9/9 cells
diverge on partition geometry", and grouped it under the depth-refine axis.
**That grouping is WRONG, and the p6 geometry is already correct.** Measured on
`CID22-512/2119713 512x512 q32 p6`:

1. p6 is `PD0_DEPTH_PRED_PART_ONLY`, so the coded tree IS the PD0 tree.
2. C's PD0 costs DO differ bd8 vs bd10 (4881 vs 4880 records) — **but the first
   SB is byte-identical (68 records) and the divergence starts at SB(64,0)**,
   and in the diverging records `dist` and `lambda` are IDENTICAL and only
   `ybits` moves (e.g. 64x64 `dist=7726356` both, `ybits` 795090 vs 794836).
   A pure RATE divergence with identical distortion is the signature of a
   different **entropy-context chain**, not different pixels.
3. Inside **SB(0,0)** (whose PD0 is provably identical), C's bd8 and bd10 final
   trees already differ on **11 of 28 mi keys — with `bsize` identical on every
   one of them**. The flips are `mode` / `uv` / `fi` / `txd`, e.g. mi(0,8)
   bd8 `uv=2` vs bd10 `uv=13 cflidx=1 cflsgn=6`.

So: PD1's 10-bit MODE decisions in SB(0,0) change the coded symbols -> the
per-SB CDF chain that PD0 rebuilds its rate tables from changes -> later SBs'
PD0 `ybits` change -> their PD0 trees change -> the classifier sees geometry
flips and labels the cell PART. **The partition machinery is not implicated.**

**And the mode flip is overwhelmingly CfL.** Port vs C at bd10, same cell,
SB(0,0): **0 C-only mi keys, 0 port-only mi keys, 0 bsize flips** — the port's
geometry is exactly C's — but **C codes `UV_CFL_PRED` on 7 of 28 blocks and the
port on 0**, accounting for 7 of the 10 uv flips. Whole-frame at p3 the same
picture is much larger: 112 `cflidx` flips, 739 `uv` flips, first divergence at
mi(0,4) with `uv: C=13 port=0`.

The cause is known and deliberate: `evaluate_leaf`'s `cfl_gate` carries
`&& bd10_rd.is_none()`, i.e. **the CfL candidate is not offered at all under the
bd10 full-RD**, because `cfl_rd_search` / the non-CfL freq re-run / the
chosen-alpha chroma TX are 8-bit only and mixing an 8-bit CfL compare into a
10-bit block cost would be silently wrong. That gate was justified by a
measurement — "the chroma complexity detector never arms on the p6 bd10 grid" —
which is TRUE for synthetic gradient/diag content and **FALSE for real
photographs**. This is exactly why the synthetic grid understated the p6 work.

### Next step (highest real-content value, bounded)

**Port the CfL arm at 10 bits.** It gates ALL real photographic content below
eff-M9 and sits upstream of everything the classifier currently calls PART
there. It is bounded, not open-ended:

- The two arms in `leaf_funnel.rs` are `cfl_uv_follows` (~215 lines, from
  `if cfl_gate && cfl_uv_follows`) and `cfl_ind_uv`; both are self-contained.
- Every kernel exists and is FFI-verified: `cfl_luma_subsampling_420_hbd` and
  `cfl_predict_hbd` (`svtav1-dsp/src/hbd.rs:806/834`), `tx_unit_hbd` (now
  returning dist+bits), `predict_unit_hbd`.
- The pieces needing an hbd twin are `cfl_ac_subsample` (small — subsample the
  10-bit winner recon), `md_cfl_rd_pick_alpha` (135 lines, the iterative alpha
  search), and the two `tx_unit` -> `tx_unit_hbd` swaps in each arm.
- Land it only on a clean byte-match; a partial thread mispredicts silently.
  Then re-run `bd10_classify.sh` on photographic p3/p6 — that is where it pays.

Only after CfL is the residual p0..p5 partition question worth re-opening; on
synthetic content the remaining 9 PART cells are the visible part of it.
