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
