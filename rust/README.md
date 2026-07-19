# zenav1-svt — the Rust port

*(Repo-root overview: [`../README.md`](../README.md). C → Rust map:
[`../PORTING.md`](../PORTING.md).)*

A pure-Rust, still-picture (AVIF/all-intra) port of [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) v4.2.0, verified **byte-identical** to the C encoder on its tested envelope, with the [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) fork's perceptual feature set available behind a runtime switch.

**~75k lines | 7 crates | 775+ tests | `#![forbid(unsafe_code)]` | AGPL-3.0 or commercial**

## Two modes, two verification bars

The encoder runs in one of two modes, selected at runtime via `EncodePipeline.hdr`:

| Mode | What it is | Verification bar |
|---|---|---|
| **Mainline** (default) | Stock SVT-AV1 v4.2.0-final behavior | **Byte-identical bitstreams** vs the real C library at matched configs (see envelope below) |
| **HdrFork** (`HdrForkConfig::hdr_fork()`) | The svt-av1-hdr fork's feature set: psychovisual RD, quant matrices, photon-noise synthesis, variance boost, six tune policies | **Functionally verified**: per-kernel differentials against the exported C functions, per-knob liveness witnesses, and `aomdec` decode gates (encoder recon == reference-decoder output). *Not* byte-gated against a C fork binary |

That distinction is the honest summary of the whole project: mainline mode is a drop-in bit-exact reimplementation on the envelope we have tested; fork mode is a faithful functional port whose every kernel is differentially tested against real C code, but whose end-to-end bytes have no C twin to compare against (the fork was rebased onto v4.2.0 by us — see `docs/HDR-ON-4.2.md`).

## What IS bit-identical (mainline mode)

Verified against the in-tree C build of SVT-AV1 **v4.2.0 final** (`Bin/Release/libSvtAv1Enc.a`), still-picture/AVIF, CQP, `--lp 1`, 8-bit 4:2:0, via an OBU-level differ plus a full arithmetic-coder op trace (every range-coder call compared, including coder state):

- **The full 132/132 synthetic identity matrix**: {uniform, gradient} content × {64, 128} px × qp {20, 40, 55} × **presets 0–10** — every cell byte-identical (`benchmarks/identity_matrix_132_full_2026-07-16.tsv`). Presets 11–13 are covered implicitly: the C library clamps all-intra presets above M9 down to M9.
- **Real-content regression spots**: 7/7 identical at the tracked configs.
- **Tile rows**: frame header + tile group byte-identical to C (multi-tile-row phase 1).

Every stream additionally decodes with the reference decoder (`aomdec`), and the decoder's output matches our encoder's own reconstruction byte-for-byte.

### Known open identity gaps (tracked, not hidden)

- **Real 512×512 content at presets 0–1**: 0/12 — one systematic first divergence: C enables 128-px superblocks above 240p at ≤M1 (`use_128x128_superblock`), the port is 64-SB only. Port map: `docs/sb128-port-map.md` (task #91).
- **Envelope limits**: 64-aligned dimensions, 8-bit, single frame (still), single-threaded. 10-bit and arbitrary dimensions are the next priorities — port-ready maps in `docs/bd10-port-map.md` and `docs/arbitrary-dims-port-map.md`.

`docs/IDENTITY-STATUS.md` is the full divergence map and campaign history.

## What fork mode gives you (the advantages)

Fork mode turns this into a **full-pareto still-image encoder**: 6 tune policies × presets 0–13 × qp 0–63 × an orthogonal perceptual knob set, all in safe Rust. Every feature below is live (flips bytes, gated) as of 2026-07-17:

| Feature | Knob(s) | What it does |
|---|---|---|
| Tune policies 0–5 | `tune` | VQ / PSNR (default) / SSIM / IQ / MS-SSIM / Film-Grain — per-16×16 SSIM rdmult lambda scaling (SSIM family), IQ lambda-weight curve, per-tune chroma-q boosts, per-tune loop-filter sharpness ladders, IQ/MS-SSIM still-image QM curve |
| Quant matrices (QM) | `qm_*` (fork default ON) | AV1 quantization matrices, luma 6..10 / chroma 8..15; tables transcribed from C and validated through the exported C quantize kernels (13,680-cell differential) |
| Photon-noise synthesis | `noise_strength`, `noise_strength_chroma`, `noise_strength_cfl`, `noise_size` | Film-grain table generation (ISO-strength model); the decoder synthesizes grain from our table — proven by an `aomdec --skip-film-grain` gate (1,440-cell differential vs the exported C generator) |
| Per-SB delta-q + variance boost | `variance_boost_strength`, `variance_boost_curve` | Per-superblock qindex from source variance, curves 0–3 + PQ dark-region attenuation |
| Sharp transforms | `sharp_tx` | RDOQ rate-weight 0 + eob-shortening disable — retains more AC detail |
| Noise normalization | `noise_norm_strength` | AC coefficient boost preserving noise energy after quantization |
| Psychovisual distortion | `ac_bias` | AC-energy-aware distortion in mode decision (Hadamard-domain psy cost) |
| MDS0 tx-bias facade | `tx_bias` | Transform-size/class distortion biases in the fast mode-decision stage |
| Complex-HVS mode | `complex_hvs` | MDS0 fast-loop distortion switches Hadamard SATD → whole-block spatial SSD |
| Alt lambda factors | `alt_lambda_factors` (fork default ON) | KF lambda factor 140 vs 150 + per-SB qdiff-stats lambda modulation |
| Alt SSIM tuning | `alt_ssim_tuning` | Block-SSIM distortion at final mode decision + two-pass SSD-envelope→SSIM winner re-pick (reachable on stills, unlike mainline's tune=SSIM arm) |
| CDEF scaling | `cdef_scaling` | Post-search CDEF strength rescale |
| Chroma-q path | (fork defaults) | Fork chroma qindex boosts, Cb +12, per-plane dequant, `separate_uv_delta_q` signaling |
| Loop-filter sharpness | `sharpness` | Search + signal + application, consistent |

Verification per feature is itemized in the status table at the bottom of `docs/HDR-ON-4.2.md`. The standing gates: per-knob liveness witnesses (`svtav1/tests/hdr_fork_e2e.rs` — every knob must actually change the bitstream, which catches "dormant knob" wiring bugs), per-tune `aomdec` decode gates (36/36), and the kernel differentials in `crates/svtav1-encoder/tests/c_parity_*.rs`.

## What we deliberately did NOT bring from the fork

For transparency, everything in svt-av1-hdr that is absent here, and why:

1. **The fork's preset re-tuning ladder** (~25 hunks re-assigning feature levels per preset). We keep mainline v4.2.0 preset semantics so mainline-mode byte-identity holds; fork features are strictly additive/opt-in.
2. **Research presets −2/−3** and the fork's changed default preset (4). Preset selection is explicit here.
3. **Post-"Chromedome" fork commits** (newer than our rebase base): noise chroma auto-strength adjustment (`4889de3`), dampened MDS0 ac-bias strength (`ce5178a`), sharpness default → 1 (`981fe12`), allow complex-HVS for all-intra (`80b48b9` — our wiring already reaches it on stills), LPD1 skip-inter-tx (`5caa3e3`).
4. **Temporal-filter knobs** — `kf_tf_strength`, `tf_strength`, `noise_adaptive_filtering`: config fields exist but are dormant; a single still frame has no temporal window. Unblocks with multi-frame support.
5. **`qp_scale_compress_strength`**: dormant — its only C consumer is the CRF rate-control qp-scale path (`rc_process.c`); this port is CQP-only.
6. **10-bit / high-bit-depth fork paths** (`hbd_mds`, HBD noise tables): the port is 8-bit today (10-bit is a tracked next priority).
7. **TUNE_VQ's `vq_ctrls` video machinery**: video-sequence heuristics, out of scope for stills. Tune 0 selects VQ's still-reachable policies only.
8. **Mainline TUNE_VMAF**: the fork replaces tune slot 5 with FILM_GRAIN; we follow the fork's numbering.
9. **LPD1 psychovisual rate**: the kernel is ported (`svtav1-dsp::ac_bias`), but the port has no LPD1 fast-decision path (all-intra never takes it in C either).

## Quick start

```rust
use svtav1_encoder::hdr_mode::HdrForkConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

let (w, h) = (128u32, 128u32);
let rc = RcConfig { mode: RcMode::Cqp, qp: 40, ..RcConfig::default() };
let mut p = EncodePipeline::new(w, h, /*preset*/ 6, rc, 4, 1);
p.chroma_420 = true;

// Fork mode (optional — default is mainline, byte-identical to C):
p.hdr = HdrForkConfig::hdr_fork();
p.hdr.tune = 3; // 0=VQ 1=PSNR 2=SSIM 3=IQ 4=MS_SSIM 5=FilmGrain

let obu_stream = p.encode_frame_420(&y_plane, &u_plane, &v_plane, /*y stride*/ 128);
```

The higher-level `svtav1::avif::AvifEncoder` wrapper provides quality/speed mapping and AVIF-oriented defaults.

## Architecture

Seven focused crates, minimal external dependencies (archmage for SIMD dispatch):

```
zenav1-svt                  Public API, AVIF backend
  zenav1-svt-encoder        Pipeline, PD0/partition, mode-decision funnel, RDOQ,
                            QM, tunes, fork features, rate control
    zenav1-svt-dsp          SIMD transforms, prediction, filtering, psy kernels
    zenav1-svt-entropy      Range coder, CDF tables, OBU/FH/SH serialization
    zenav1-svt-tables       Const lookup tables, scan orders
    zenav1-svt-types        Core AV1 type definitions
  zenav1-svt-cref           Test-only FFI shims to the real C library (differentials)
```

Those are the **package** names. Each crate pins a short `[lib] name`, so Rust
paths stay `use svtav1_encoder::…` / `use svtav1_dsp::…`, and the crate
directories keep their `crates/svtav1-*` names (the port maps and bug log
reference those paths). See [`../PORTING.md`](../PORTING.md) for the full table.

## How verification works

Three layers, strongest first:

1. **Byte-identity harness** (`tools/identity_diff.sh`, `tools/capture_c_trace/`): drives the real C library through its public API with `--wrap`ed range-coder entry points, then compares OBU bytes field-by-field AND every arithmetic-coder operation (symbol, CDF, coder range state) against the Rust `symtrace` output. Exit 0 iff streams are byte-identical.
2. **Kernel differentials** (`svtav1-cref` + `tests/c_parity_*.rs`): Rust kernels vs the exported C functions from the in-tree static library — quantizers, QM, noise generation, SSIM distortion, variance boost, ac-bias, and more, across randomized/gridded inputs.
3. **Decode gates** (`aomdec`): every gated stream must decode, and the decoder's output must equal the encoder's own reconstruction byte-for-byte — the AV1-conformance floor that holds in both modes, including for streams no C twin exists for.

```bash
cargo test --workspace           # 775+ tests
just identity 64 64 40 6 gradient  # one identity cell vs the C library
```

Building the C reference (needed for differentials and identity runs): see `docs/HDR-ON-4.2.md` § Reproduce.

## Building

Requires Rust 1.85+ (2024 edition).

```bash
cargo build --workspace
cargo clippy --workspace        # 0 warnings
cargo test --workspace
```

## Safety

Every crate uses `#![forbid(unsafe_code)]` except the test-only `zenav1-svt-cref` (FFI to the C reference library, never shipped — dev-dependency only, so no published crate carries a `build.rs` or needs a C toolchain). SIMD goes through archmage's safe token-based dispatch.

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

Upstream C code from [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) — and
the [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) fork whose
feature set is ported in fork mode — is BSD-3-Clause-Clear with the Alliance
for Open Media Patent License 1.0; see [LICENSE.md](../LICENSE.md) and
[PATENTS.md](../PATENTS.md) at the repository root. Those terms continue to
cover the upstream work this port derives from. This dual license applies to
the Rust port in `rust/`; the C tree in the rest of the repository keeps
the upstream licenses.

### Path to MIT

If someone covers Imazen's 2026 AI + server costs, we'll release this port
under MIT — or under the original upstream license (BSD-3-Clause-Clear + AOM
Patent License 1.0). Contact support@imazen.io.

## Acknowledgments

- [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (Intel / Alliance for Open Media) — the battle-tested C encoder this port is built on
- [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) (juliobbv-p) — the perceptual/HDR feature set ported in fork mode
- [rav1d-safe](https://github.com/memorysafety/rav1d) — safe Rust AV1 decoder; DisjointMut pattern adapted
- [archmage](https://github.com/imazen/archmage) — safe SIMD dispatch via CPU feature tokens
