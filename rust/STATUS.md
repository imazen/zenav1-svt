# SVT-AV1 Rust Port — Status

Last updated: 2026-07-13 (wave2/entropy-c-parity) — C baseline **v4.2.0-rc**

## QP domain (C-exact since 2026-07-13)

`RcConfig.qp` is CLI-domain 0..63 exactly like C's `--qp`; the pipeline
maps it through the verbatim `quantizer_to_qindex[64]` port ONCE at frame
setup and every downstream consumer (quantizer tables, FH base_q_idx,
CDF q bucket, chroma quantization, deblock level picker) operates on the
qindex 0..255. Before the split the 0..63 value was consumed as qindex
directly, capping the reachable quantizer range at qindex 63 (top-quality
quarter) and keeping deblock levels <= 3. All matrices below use
CLI-domain qps.

## Arbitrary dimensions — chunk 1 (task #95, 2026-07-17)

Full-SB arbitrary dimensions land: the pipeline carries TWO dim systems —
TRUE (caller-passed, header/crop) and ALIGNED (round-up-to-8, the encode
grid). `encode_frame_420` edge-replicates the input planes TRUE->ALIGNED
(C `pad_input_picture`), the seq header carries TRUE
`max_frame_width/height_minus_1`, and the small-frame restoration disable
(`enc_settings.c:214-232`, true w|h < 64) is replicated. Scope: aligned
dims a multiple of 64 (dims {57..64} -> a single 64x64 SB, e.g. 60x60).

| Gate | Result |
|---|---|
| 60x60 uniform+gradient vs SvtAv1EncApp, presets 13/10/6 × q20/40/55 | **18/18 byte-identical** |
| default identity_matrix (64/128 full-SB + 60 arb-dims) | **54/54** |

## Arbitrary dimensions — chunk 2: PARTIAL SBs byte-match (task #95, 2026-07-18)

Partial superblocks (aligned NOT a mult of 64) AND ODD dimensions now byte-match
real C. `tools/partial_sb_gate.sh` = **101/101** (presets **6/7/8/9/10/13**, bd8
4:2:0; includes both-partial + straddle + odd dims): the 96x80
milestone (cmp-verified 878B) + full/straddle cells + **11 odd-dim cells** (65x64,
64x65, ...) + 6 bottom-edge/8-aligned-partial + 5 straddle-win. Full-SB identity
matrix stays **54/54**; bd10 36/36 + bd10-nonflat 8/8 untouched. Verified
PANIC-FREE incl. odd dims (484 cells dims×qp, all decodable). Landed pieces:
- **ODD dims** — harness ceiling chroma `(w+1)/2` both sides; LR true-dim search
  (`search_restoration_still`/`write_lr_for_sb` on TRUE luma / CEILING chroma,
  fixing the odd-height FH `lr_type` WIENER-vs-NONE divergence).
- **PD0 boundary-node cost fix** (the high-leverage root, `pd0.rs` +
  `context::partition_alike_split_cost`) — TWO real bugs pinned by a new
  `SVT_PD0COST` C `--wrap` interposer (harness, env-gated, C tree pristine):
  (1) rectangular tx-type rate returned 0 for non-square edge shapes (748 bits
  too cheap) — fixed via `TXSIZE_SQR_MAP`; (2) boundary split used the
  full-alphabet rate instead of C's binary `partition_{vert,horz}_alike` (cross-
  named). Unlocked all single-edge partial + the straddle-win cells at q≤32.

- **SB-extent padded variance** — `encode_input` padded TRUE->sb_ext
  (`frame_geom::pad_input_plane`, edge replication) at the sb_ext stride, so
  `compute_b64_variance`'s unclamped 64x64 walk reads C's replicated border.
- **Partition edge SEARCH** — a partial node is a DETERMINISTIC edge-shape
  decision (`set_blocks_to_test`: one shape injected, `md_disallow_nsq_search`),
  priced on the NON-SQUARE in-frame block (`pd0::lvl1_block_cost_rect`,
  `leaf_funnel::decide_leaf_rect` + tall-rect TX Tx32x64/16x32/8x16), NOT the
  square PART_N cropped nor forced-split. Off-frame quadrants = `Pd0Tree::Off`.
- **Partition edge CODING** — `encode_partition_av1` binary SPLIT-vs-{H,V} with
  the CROSS-named `partition_gather_{horz,vert}_alike` (see arb-dims-port-map),
  no-symbol forced split when both-false, single-child H/V pack arms.
- **Straddle boundary blocks** — C codes blocks that reach PAST aligned (the
  "leaves inside ALIGNED" assumption was false — even both-true nodes straddle,
  e.g. 48x56's 64-root); recon+chroma working buffers are sized to the sb_ext
  PRODUCT so straddling reads/writes never OOB. Verified PANIC-FREE: 240
  partial-SB cells (dims x qp) all decodable, 0 panics.

REMAINING (decodable-DIFF, documented in docs/arbitrary-dims-port-map.md, NOT
gated): straddle-WIN cells (80x88, 104x88, 72x88 — C keeps a straddling leaf)
need cropped-RDO distortion + a true sb_ext chroma STRIDE (not just product
slack); 65x65 odd-width (harness even-dim + DLF floor-vs-ceiling chroma); the
M9+ boundary edge-shape cost (wired on LVL_1 only). See CLAUDE.md #95.

## Decode conformance (AV1 reference decoder)

`tools/decode_conformance.sh` — 525-stream mono matrix (gradient/uniform/
edges x 32..128 px x CLI qp {20,32,43,55,63} = qindex {80,128,172,220,
255} x speeds 2..10) plus a 700-stream 4:2:0 matrix
(`tools/decode_conformance.sh <dir> chroma`: same grid + a `color`
content whose chroma planes carry real patterns), every stream must
decode under **aomdec**:

| Gate | Result |
|---|---|
| 525/525 mono streams decode | **PASS** (was 0/525 before this wave) |
| 700/700 chroma-420 streams decode | **PASS** (new 2026-07-13; opt-in `with_chroma_420`) |

The old rav1d-based "decode PASS" claims were leniency artifacts; aomdec is
the gate now. **2026-07-18: the 4:2:0 gate gained palette-forcing `stripes`
content (1575/1575) after fixing a palette `filter_intra` desync (a0b505b4f)
that had held CI red — see CLAUDE.md.**

## 10-bit (bd10) encode — uniform, ALL presets (task #94, 2026-07-18)

`tools/bd10_matrix.sh` (also a CI gate): uniform {64,128} x qp{20,40,55} x
preset{0,2,3,6,10,13} encodes byte-identical to real aomenc at bit depth 10
(**36/36**) and decode under aomdec. Harness: `capture_c_trace <..> 10` (packed
u16 LE) + `identity_run SVTAV1_BD=10` + the pipeline's `with_bit_depth`. Three
frame-header chunks landed: the first cell (uniform, aa89a83be — the port stays
u8 because flat->skip makes the tile bit-depth-independent), the M6+
LF-level-from-Q bd10 derivation (be1ea0770), and the qp-fast-path CDEF
strength-from-Q bd10 derivation (885ece6da: `q = AC_QLOOKUP_10[qindex] >> 2`,
same f32 fit — proven C-exact for all 256 qindexes by the `c_parity_cdef_pick`
bd10 differential, and end-to-end by the gradient bd10 op-trace's first
divergence moving off the FH cdef line into the tile). The 5 bd10 DSP kernel
families are FFI-verified (see the differential-suites table).

## bd10 NON-FLAT — first cells with a coded residual byte-match (task #94, 2026-07-18)

`tools/bd10_nonflat_gate.sh` (CI gate): `gradient 64x64 q40` at preset **10 and
13** byte-match real aomenc at bd10 (**2/2**) — the first non-flat bd10 cells.
Root cause of the prior tile divergence: the port quantized the residual with
the bd8 Q8 tables while C uses bd10 Q10 (~4xQ8 but NOT exactly) → different coded
levels. Fixed via an ADDITIVE, bd10-gated u16 re-encode (the "M4+ bypass_encdec
re-predict" shape — the u8 partition/mode/tx decisions are RD-scale-invariant for
`sample<<2` content, so only the bit-depth-sensitive coded luma LEVELS + true
10-bit recon are recomputed; NOT a full u8->u16 refactor). Pieces:
`quant::build_quant_table_bd` (Q10 + qzbin), `quant::quantize_fp_hbd` (**THE FIX**:
the INT16 clamp in `quantize_fp` is bd8-only — C dispatches bd>8 to
`highbd_quantize_fp_helper_c`, full_loop.c:367-395), `leaf_funnel::{predict_unit_hbd,
tx_unit_hbd}`, `pd0::kf_full_lambda_bd10` (EXACT C full_lambda_md[1], not ×16 of
bd8), a bd-aware inverse transform, and `pipeline::bd10_reencode_luma`. The u8 path
is byte-UNCHANGED (bd8 identity 54/54, bd10 uniform 36/36).

ENVELOPE (narrow, honest): only the **DC-family / tx_depth-0 / rdoq-fp** subset is
ported. Out-of-envelope bd10 frames (directional or filter-intra intra, tx_depth>0,
rdoq level 0, non-uniform chroma) FALL BACK to the non-panicking u8 output via the
`bd10_tree_supported` gate — the encoder stays panic-free on the public
`encode_frame_420` API; the u16 predict/tx path panics loudly only where a
future-ported case would land, and the gate never lets it. FOLLOW-UPS (#94):
`dr_predict_hbd` (directional), `predict_filter_intra_hbd`, `quantize_b_hbd`
(rdoq-0, same INT16-clamp class), tx_depth>0 re-encode, the u16 chroma path, and
native (non-`<<2`) u16 ingestion. See docs/bd10-port-map.md.

NOTE (2026-07-18): the prior session's bd10 + palette-conformance work (10
commits, 58bd3b4c9..885ece6da) was committed+verified-green locally but **never
pushed to origin** — origin CI had been red since 2026-07-16 without the palette
`filter_intra` conformance fix. Recovered this session: pushed + origin-verified
(`merge-base --is-ancestor HEAD origin`), all gates green locally (workspace
tests, bd8 54/54, bd10 uniform 36/36, mono conformance 1260/1260, chroma
1575/1575).

## Bit-exact-vs-C differential suites (svtav1-cref harness)

All verified against the linked `libSvtAv1Enc.a` (v4.2.0-rc) on every test
run:

| Module | Verification |
|---|---|
| Range coder (`OdEcEnc`) | byte-identical streams: 30k update_cdf cases, 300 random static/adaptive streams, carry torture, tiny streams |
| `update_cdf` | bit-identical, all alphabet sizes 2–16 |
| Default CDF tables (13 coef families x 4 q-buckets + 12 mode families) | drift test re-extracts from C every run |
| Scan orders (19 x 3) | drift test vs `eb_av1_scan_orders` |
| Quantizer step tables | generated from `svt_aom_dc/ac_quant_qtx` |
| Coefficient writer helpers (level maps, nz/br ctx, eob tokens, txb dims) | fuzzed vs exported C impls |
| Deblocking kernels (`svt_aom_lpf_{h,v}_{4,6,8,14}_c`) + sharpness limits | bit-exact over all (level, sharpness) x content classes (c_parity_lpf) |
| CDEF kernels (`svt_cdef_filter_block_c` dst8, `svt_cdef_filter_block_8bit_c`, `svt_aom_cdef_find_dir{,_8bit}_c`) | bit-exact over all 64 signalable strengths x damping 2..=6 x dirs x 8x8/4x4 x frame-border sentinel patterns + randomized wide/torture (c_parity_cdef) |
| CDEF qp-strength picker (`svt_pick_cdef_from_qp` intra branch) | bit-exact for all 256 qindexes vs C float semantics (c_parity_cdef_pick) |
| **bd10** quant step tables (`svt_aom_dc/ac_quant_qtx` at `EB_TEN_BIT`) | all 256 qindexes DC+AC vs real C (c_parity_bd10_quant) — #94 |
| **bd10** loop filters (`svt_aom_highbd_lpf_{h,v}_{4,6,8,14}_c`) | bit-exact at bd10+bd12 over all (level, sharpness) x content (c_parity_lpf_hbd) — #94 |
| **bd10** distortion/variance/SAD (`svt_full_distortion_kernel16_bits_c`, `svt_aom_variance_highbd_c`, `svt_aom_sad_16b_kernel_c`) | bit-exact at bd10+bd12 over 14 block shapes, strided (c_parity_hbd_distortion) — #94 |
| **bd10** intra predictors (sized `svt_aom_highbd_*_predictor_WxH_c`) | bit-exact at bd10+bd12: 10 modes (DC×4 / V / H / Paeth / Smooth×3) × 19 sizes, 7600 preds (c_parity_intra_pred_hbd) — #94 |

v4.2.0-rc note: upstream refactored the coder internals (borrowed buffer,
ptr walk) — output verified still byte-identical; `coeff_br_cdf` dropped its
dead 64x64 slice (tables regenerated).

## Pixel-path status (decoded output correctness) — CORRECT

All probes decode via aomdec and compare against the source. (The
q labels below are the EFFECTIVE QINDEXES the historical runs measured
at — they predate the CLI-qp/qindex split, when RcConfig.qp was consumed
as qindex directly; to reproduce "qindex30" today pass CLI qp 30/4 ≈ 8,
or call the block APIs with qindex 30 directly.)

| Probe | Result |
|---|---|
| uniform-128, flat-140, flat-250 | **bit-exact** |
| edges 64px qindex30 s2 / 96px q50 s4 | **LOSSLESS** (205/367 bytes; C reference also lossless at 172 bytes — remaining delta is RD tuning) |
| gradient 64px qindex30 s4 | **46.76 dB** |
| gradient 128px q50 s8 | 30.39 dB |
| 420 probe 64px q30 (examples/probe_420) | Y 46.64 / U 52.97 / V lossless |
| 420 probe 128px q30 | Y 46.03 / U 51.92 / V 52.86 dB |
| 420 probe 128px q50 | Y 30.39 / U 55.98 / V 57.44 dB (Y == mono ref) |

Fixed en route: live-recon prediction neighbors, real mode/tx-type
signaling, AV1 quantizer tables + decoder-mirrored dequant, per-size
forward cos bits, restored inverse stage-range clamps, C-exact intra edge
fill (127/129/left[0]/above[0] rules), 64-dim coefficient zeroing.
Deblocking is now SIGNALED and applied decoder-exactly (2026-07-13): key
frames carry the q-picked loop_filter levels and the recon-parity gate
holds 216/0 with filtering live. CDEF likewise (2026-07-13): SH
enable_cdef=1, FH cdef_params (cdef_bits=0, qp-picked strengths — C's
use_qp_strength closed form, NOT the RDO search yet), decoder-exact
av1_cdef_frame pass after deblock on the output copy; recon-parity 216/0
with CDEF firing on 168/216 streams (2.34M px filtered, 882k changed;
per-64x64 cdef_idx costs zero EC bits at cdef_bits=0). Restoration stays
disabled+unsignaled. At real high qindexes deblock returns material
levels ([61,61,30,30] at qindex 220, [63,63,60,60] at 255;
examples/deblock_evidence) and CDEF signals y=17/43/63 at qindex
172/220/255, improving gradient content +0.25/+0.50/+0.31 dB and ringing
edges +0.16 dB at 255 with parity exact (examples/cdef_evidence).
The qindex split also exposed + fixed a latent VERT_A/VERT_B bug: their
children now use the C has_tr_vert_*/has_bl_vert_* availability tables
(the search emits ext partitions at preset <= 8; passing the generic
tables coded D-mode children against above-right pixels the decoder
doesn't have yet — recon-parity 211/5 -> 216/0 at qindex {80,172,255}).
Per-SB QP offsets stay disabled until delta_q signaling is ported.

## Known failing test

(none — `multi_frame_bitstream_sizes_decrease` passes again since the
unsignaled loop filters were disabled: the filtered DPB recon had been
corrupting inter references, which was the real reason inter frames
outweighed the key frame. Workspace fully green.)

## Architecture direction

Module-by-module faithful port of C SVT-AV1 behind `svtav1-cref`
differential harnesses (see `docs/PORT-coeff-writer.md` for the worked
example). Bitstream writer layer (headers, tile groups, coefficient coding)
is now C-exact at the writer level; decision layers (partition/mode RDO,
filters, chroma) still ours and next in line:

1. Chroma 4:2:0 end-to-end — **landed 2026-07-13 (opt-in
   `with_chroma_420` + `encode_frame_420`; still-frame, UV_DC-only,
   min-8x8 luma policy — see CLAUDE.md gap 1a-1d for what remains
   toward C decision parity)**
2. Filter search + signaling ports — deblocking landed 2026-07-13
   (C-exact kernels + q-based level picker + decoder-exact frame walk,
   signaled in the FH; SSE-based level search and inter-frame levels still
   pending); CDEF landed 2026-07-13 (C-exact kernels + C's qp-strength
   fast path at cdef_bits=0 + decoder-exact av1_cdef_frame application;
   the C-default per-fb RDO search moves to the decision-parity wave);
   restoration next
3. Directional-mode edge extension (has_top_right/bottom_left)
4. Decision-layer parity vs C (partition/mode/TX RDO), then per-preset
   bitstream identity gates (see COVERAGE.md for the config-surface
   scoreboard: 121 fields auto-derived from EbSvtAv1EncConfiguration)

## Crate structure

```
rust/
  crates/svtav1-types          Core AV1 types, enums, constants
  crates/svtav1-tables         Const lookup tables (no_std)
  crates/svtav1-dsp            Transforms, prediction, filters, quant (+ generated quant tables)
  crates/svtav1-entropy        Range coder, CDFs, OBU, coefficient coding (+ generated CDF/scan tables)
  crates/svtav1-encoder        Pipeline, partition, mode decision, RC
  crates/svtav1-cref           Test-only FFI harness over libSvtAv1Enc.a (the differential oracle)
  crates/svtav1-disjoint-mut   Region-based borrow tracking
  svtav1                       Public API, AVIF backend
```

C reference builds required for tests:
`cmake -S . -B cbuild-static -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF && cmake --build cbuild-static -j`
