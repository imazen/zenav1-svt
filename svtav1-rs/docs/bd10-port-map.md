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

**Scope boundary (measured):** uniform byte-matches at M0/M2/M3 but NOT M6/M13.
The faster presets derive LF/CDEF from the bd10 quantizer (LPF_PICK_FROM_Q uses
`av1_ac_quant_QTX(qindex, .., bd)` = the bd10 qlookup, already FFI-verified),
which diverges from the port's bd8 LF params. **NEXT bd10 chunk = the LF-level-
from-Q derivation at bd10** (thread bd into `lf_search::pick_filter_level_from_q`
+ the quantizer it reads), which should close M6/M13 uniform. THEN the u16 MD
path for non-flat content (precision-sensitive RD).

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
