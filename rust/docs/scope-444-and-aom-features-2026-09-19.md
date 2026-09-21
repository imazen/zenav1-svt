<!-- dated snapshot: scope estimates as of 2026-09-19, not a commitment -->

# Scope: 4:4:4 chroma + aom-side features for zenav1-svt

Context: the still-image target-quality path (`svtav1-target`) is now
multi-metric + tune-aware. Two follow-ons were scoped: (a) 4:4:4 input
support in the port, (b) the aom-encoder feature set as a port gap map.
Numbers are file/grep counts on the stated HEADs; KLOC figures are
estimates for implementation + witnesses, not measurement.

## Baselines

- Port: `crates/svtav1-encoder` + `crates/svtav1-dsp` ≈ 271 KLOC.
- C reference: SVT-AV1 v4.2.0 (`reference/svt-av1`) — the app *exposes*
  `--color-format` 1/2/3, but `verify_settings` refuses every non-420
  value with "Only support 420 now" (enc_settings.c:470, verified
  2026-09-19). 4:4:4/4:2:2 are therefore **extension work with no C byte
  oracle** — the gate is decoder correctness (aomdec/dav1d) plus
  RD-quality comparisons vs `zenav1-aom`, not byte parity. This doc's
  earlier reading of the appendix as "natively supported" was wrong.
- C subsampling plumbing already parameterized: `subsampling_x/y`
  derived from `chroma_format_idc` (enc_handle.c:4636, pcs.c:473,
  enc_dec_process.c:453, noise_model.c:2168) — ~1,026 lines across 59
  codec files touch `ss_x`/`ss_y`/`color_format`.
- Sibling: `zenav1-aom` (~284 KLOC libaom port) ships
  420/422/444/mono × bd8/10/12 — the in-family existence proof and a
  source of 444-specific tables (`aom_get_qmlevel_444_chroma`, intrabc
  chroma predict over all formats).

## A. 4:4:4 in zenav1-svt — estimated 7–10 KLOC touched

The port hardcodes `w/2`-chroma geometry; C parameterizes by
`subsampling_x/y`. The work is *re-parameterizing the chroma plane
geometry*, then re-wiring the chroma-aware passes. **Landed 2026-09-20:**
the `ChromaFormat` enum (400/420/422/444), `ss_x/ss_y` on `FrameDims`,
`SeqTools` profile-1/2 color-config branches per C `write_color_config`,
and staged `try_encode_frame_{422,444}` entry points that refuse until
the deep port lands — the table below is what remains.

| subsystem | what changes | est. loc |
|---|---|---|
| API/config | ~~enum + seq-header syntax~~ **done**; remaining: `encoder_color_format` plumbing through scs/pcs, plane-buffer allocation at format-derived strides | 300–500 |
| Chroma geometry | `ss_x/ss_y` fields on pcs-equivalent; replace `w/2`, `h/2`, `>>1`, `div_ceil(2)` sites — ~51 sites in pipeline.rs, ~32 files total encoder+dsp | 1,500–2,500 |
| Prediction | CfL: `cfl_luma_subsampling_420` downsample → 422/444 variants (or direct at 444); intra chroma pred at full res; inter subpel offsets; obmc chroma paths | 1,000–1,800 |
| Tx/coeff | chroma tx sizes unsubsampled (max 32 chroma tx at 444 vs capped at 420); coeff-code contexts/eob/scan per C's `blk_geom` chroma rules | 800–1,400 |
| Entropy/syntax | `separate_uv_delta_q` gating per format; `diff_uv_delta`; CDF chroma contexts; profile 1/2 signaling | 400–700 |
| Loop filters | deblock/CDEF/LR on full-res chroma planes (2× plane area vs 420) | 400–800 |
| QM | C's chroma QM level derivation at 444 (aom-side has a separate `444_chroma` table family — check C SVT's equivalent before assuming reuse of the 420 tables) | 200–400 |
| Tune IQ at 444 | VB is luma-only (safe); QM chroma levels + `max_tx_size` chroma interaction need re-derivation checks | 100–300 |
| bd10 444 | hbd path mirrors bd8 once geometry is parameterized | +30–40% on the above |
| Tests/gates | aomdec/dav1d 444+422 decode legs (no C byte oracle — C refuses non-420), recon-vs-decoded cells, ss_x/ss_y spot gates, refusal strings for unsupported combos | 1,500–2,500 |

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
| **chroma delta-q ramps** | `chroma_delta_q`, six tune ramps, byte-exact vs aomenc | `chroma_q.rs` — **done**: deltas derived, per-plane qindexes, QM levels and frame-header signaling all threaded (mainline shared U/V, fork `separate_uv_delta_q`) | **done** (was already wired; stale module doc corrected 2026-09-20) |
| **adaptive sharpness** | content-derived sharpness | `AomAdaptiveSharpness` variant — shipped, measured byte-identical to IQ in 252/252 cells (SVT's IQ ladder already applies the same cap) | **done** — no-op under IQ by construction and by measurement |
| **adaptive CDEF** (strength halving on low-variance sources) | `adaptive_cdef` (2 files) | `AomAdaptiveCdef` variant — shipped as post-pick transform | **done**, measured +0.4–0.9% ssim2-BD vs plain IQ: small regression, kept opt-in |
| **delta_q_lf** (loopfilter delta-q) | `deltaq_mode = DELTA_Q_LF` (5 files) | `AomDeltaQLf` variant — shipped: FH `delta_lf_present/res2/multi0`, per-SB symbol after delta-q on `delta_lf_cdf`, per-tile prev reset, two-sided edge levels in deblock | **done** — decoder-verified (recon == aomdec at bd8 SB64/SB128/multi-tile and bd10), measured RD-neutral (−0.01%/+0.24% BD vs IQ) |
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
   (Note: C v4.2.0 *refuses* non-420 at `verify_settings` — see §D —
   so "C-oracle'd" means C's unported machinery is the semantic
   reference, not a byte oracle.)
4. ~~delta_q_lf~~ — shipped 2026-09-20 as `AomDeltaQLf`.

Items 1, 2 and 4 are all landed as of 2026-09-20; the remaining program
is the 4:4:4 deep port.

## D. Explicitly out of scope

- Byte-parity claims at 444/422 *ever* — C v4.2.0 refuses non-420 at
  `verify_settings`, so there is no byte oracle. The gate is aomdec/dav1d
  decode + encoder-recon equality + RD comparison vs `zenav1-aom`.
- 12-bit input (C envelope stops at 10; that's alternate-backend work).
- aom-internal RD machinery with no C SVT counterpart.
