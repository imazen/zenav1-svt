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

## Scope for svtav1-rs (CORRECTED — the agent sampled /root/aom-rs)
svtav1-rs is u8 end-to-end in svtav1-dsp (intra pred, tx/quant/recon
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
