<!-- dated snapshot: scope estimates as of 2026-09-19, not a commitment -->

# Scope: 4:4:4 chroma + aom-side features for zenav1-svt

Context: the still-image target-quality path (`svtav1-target`) is now
multi-metric + tune-aware. Two follow-ons were scoped: (a) 4:4:4 input
support in the port, (b) the aom-encoder feature set as a port gap map.
Numbers are file/grep counts on the stated HEADs; KLOC figures are
estimates for implementation + witnesses, not measurement.

## Baselines

- Port: `crates/svtav1-encoder` + `crates/svtav1-dsp` ≈ 271 KLOC.
- C reference: SVT-AV1 v4.2.0 (`reference/svt-av1`) — **supports
  420/422/444 natively** (`Docs/Appendix-Alt-Refs.md`: "8-bit and 10-bit
  sources as well as 420, 422 and 444 chroma sub-sampling"; app flag
  `--color-format` 1/2/3). 4:4:4 is therefore *unported C surface* —
  byte-parity against C is the oracle, exactly like the 420 campaign.
- C subsampling plumbing already parameterized: `subsampling_x/y`
  derived from `chroma_format_idc` (enc_handle.c:4636, pcs.c:473,
  enc_dec_process.c:453, noise_model.c:2168) — ~1,026 lines across 59
  codec files touch `ss_x`/`ss_y`/`color_format`.
- Sibling: `zenav1-aom` (~284 KLOC libaom port) ships
  420/422/444/mono × bd8/10/12 — the in-family existence proof and a
  source of 444-specific tables (`aom_get_qmlevel_444_chroma`, intrabc
  chroma predict over all formats).

## A. 4:4:4 in zenav1-svt — estimated 7–10 KLOC touched

The port hardcodes `chroma_420: bool` and `w/2`-chroma geometry; C
parameterizes by `subsampling_x/y`. The work is *re-parameterizing the
chroma plane geometry*, then re-wiring the chroma-aware passes.

| subsystem | what changes | est. loc |
|---|---|---|
| API/config | `with_chroma_420(bool)` → `ChromaFormat` enum (400/420/422/444); `try_encode_frame_*` plane-size validation; `encoder_color_format` plumbing; profile/seq-header `subsampling_x/y`, `chroma_sample_position` syntax | 600–900 |
| Chroma geometry | `ss_x/ss_y` fields on pcs-equivalent; replace `w/2`, `h/2`, `>>1`, `div_ceil(2)` sites — ~51 sites in pipeline.rs, ~32 files total encoder+dsp | 1,500–2,500 |
| Prediction | CfL: `cfl_luma_subsampling_420` downsample → 422/444 variants (or direct at 444); intra chroma pred at full res; inter subpel offsets; obmc chroma paths | 1,000–1,800 |
| Tx/coeff | chroma tx sizes unsubsampled (max 32 chroma tx at 444 vs capped at 420); coeff-code contexts/eob/scan per C's `blk_geom` chroma rules | 800–1,400 |
| Entropy/syntax | `separate_uv_delta_q` gating per format; `diff_uv_delta`; CDF chroma contexts; profile 1/2 signaling | 400–700 |
| Loop filters | deblock/CDEF/LR on full-res chroma planes (2× plane area vs 420) | 400–800 |
| QM | C's chroma QM level derivation at 444 (aom-side has a separate `444_chroma` table family — check C SVT's equivalent before assuming reuse of the 420 tables) | 200–400 |
| Tune IQ at 444 | VB is luma-only (safe); QM chroma levels + `max_tx_size` chroma interaction need re-derivation checks | 100–300 |
| bd10 444 | hbd path mirrors bd8 once geometry is parameterized | +30–40% on the above |
| Tests/gates | new identity cells (444 C-parity), aomdec 444 decode leg, ss_x/ss_y spot gates, refusal strings for unsupported combos | 1,500–2,500 |

Order: geometry enum + ss_x/ss_y core first (everything else keys off
it), then syntax, then prediction, then filters. 422 and 400(mono)
fall out of the same parameterization for ~15–20% marginal cost —
worth taking in the same campaign since C ships all four and the
mono surface is already half-present (`EB_YUV400` paths exist in C).

**Reusable from zenav1-aom:** the ss_x/ss_y conventions are identical
(same AV1 syntax); the 444 QM table family and CfL-444 subsample
variants are direct references for the table-level pieces. The control
flow is NOT portable (libaom's cm/xd structures vs SVT's pcs/scs).

## B. aom-side features → SVT port gap map

Status on the port HEAD (file counts = files containing the machinery):

| feature | aom | SVT port | verdict |
|---|---|---|---|
| tune=IQ bundle | `apply_tune` (84-cell gated) | `hdr.tune = TUNE_IQ` + `apply_tune_overrides` — shipped, verified −4 to −14% BD | **done** |
| QM (min/max levels) | `qmatrix` + `aom_get_qmlevel_*` | 21 files, `with_qm`, tune-driven levels | **done** |
| sharpness | fixed + adaptive | `hdr.sharpness` (22 files); IQ pins 7 | done, adaptive open |
| variance-boost delta-q | `deltaq-mode` | 63 files, `enable_variance_boost` + curve/octile | **done** (incl. SB128 fix) |
| palette | wired | `palette.rs` 1.3 KLOC, 34 files | done |
| intrabc / IBC | wired | `intrabc.rs` 3.0 KLOC, 56 files | done |
| screen-content mode | `tune_content` | `screen_content_mode` (IQ forces 3) | done |
| **chroma delta-q ramps** | `chroma_delta_q`, six tune ramps, byte-exact vs aomenc | `chroma_q.rs` 190 loc + `separate_uv_delta_q`/`diff_uv_delta` syntax in fork — **"fork wiring pending chroma-q quant threading"** | **~0.8–1.5 KLOC**: the fork already has the syntax + delta math; the gap is threading chroma qindex through the quant/dequant path and the tune-ramp derivation |
| **adaptive sharpness** | content-derived sharpness | fixed `hdr.sharpness` only | ~0.3–0.6 KLOC: derive from `port_preanalysis` variance stats; needs an RD sweep before defaulting (measured: sharpness 5 ≈ −0.3% BD but inside noise) |
| **adaptive CDEF** (strength halving on low-variance sources) | `adaptive_cdef` (2 files) | **0 files** | ~0.5–1.0 KLOC + gate: cdef strength modulation at frame level; aom's halving is `tune=IQ`-only — decision: enhancement flag, not default |
| **delta_q_lf** (loopfilter delta-q) | `deltaq_mode = DELTA_Q_LF` (5 files) | delta-q exists (22 files) but luma/VB only | ~0.5–1.0 KLOC: SB-level delta-lf symbols + emission; interacts with VB plan — sequence after the chroma-delta-q threading |
| ssimulacra2/iq tuning | `tune=SSIMULACRA2`/`IQ` | SVT's own TUNE_IQ is the equivalent | N/A — different bundle, already shipped |

**Not portable (aom-only, no C SVT basis):** aom's RD-cost internals
(`psnr_rd_adjust_qdelta` trellis weighting), aom's `x->e_mbd` delta-q
propagation shape, arf/alt-ref temporal structures (SVT has its own
hierarchical design). Those are architectural, not features.

## C. Recommended sequencing

1. **chroma delta-q completion** (~1 KLOC) — smallest gap, fork syntax
   already exists, directly improves IQ-family chroma RD.
2. **adaptive CDEF + adaptive sharpness** (~1 KLOC combined) — both are
   small content-conditioned knobs; gate each on the still subset
   before defaulting (sharpness-5 was inside noise — measure, don't pin).
3. **4:4:4** (7–10 KLOC) — largest item but fully C-oracle'd; the
   geometry parameterization is the long pole and it amortizes 422+400.
4. **delta_q_lf** (~1 KLOC) — after chroma-q threading lands.

Total: ~10–13 KLOC for the whole program; items 1+2 alone are ~2 KLOC
and close the chroma-RD gap that IQ currently leaves on the table.

## D. Explicitly out of scope

- Byte-parity claims at 444 until the identity cells exist — aomdec
  decode is the interim gate, then C-parity cells.
- 12-bit input (C envelope stops at 10; that's alternate-backend work).
- aom-internal RD machinery with no C SVT counterpart.
