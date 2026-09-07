# zenav1-svt

A pure-Rust AV1 encoder — an
algorithm-for-algorithm port of [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1)
v4.2.0, verified **byte-identical** to the C encoder across its tested envelope,
with the [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) fork's
perceptual feature set available behind a runtime switch — and *that* mode
byte-gated too, against a `SVT_HDR_MODE=ON` build of the same C base.

The **shipped product surface includes still images and all-intra animated AVIF**.
The optional `avif-container` feature writes timed color and alpha tracks;
`EncodePipeline` still refuses non-key frames. Behind a harness-only switch, the
**inter path is byte-gated too** — 108 asserted two-frame low-delay-P cells in
CI, 94 of the campaign's 96-cell grid byte-identical on both frames, and the
whole p0–p4 synthetic band since global motion landed. That campaign is
`rust/docs/INTER-ENCODE-PLAN.md`; the index to the whole project is
[`CONTEXT-HANDOFF.md`](CONTEXT-HANDOFF.md).

**`#![forbid(unsafe_code)]` · ~80k lines · 7 crates · 2554 tests (nextest, as of `84621b20d`) · no C in the product path**

> **Experimental.** The envelope below is real and gated, but it is an envelope:
> the shipped API uses CQP and all-intra coding; general-purpose video encoding
> remains incomplete, even though the inter path encodes byte-identically under
> the differential harness. Crates are not on crates.io yet — depend by git.

The SVT-AV1 C tree is **not vendored here** — it lives in the
[`imazen/zenav1-svt-c`](https://github.com/imazen/zenav1-svt-c) submodule at
`reference/svt-av1` — a fork of upstream SVT-AV1 with **full history + all tags**
(`master` mirrors gitlab.com/AOMediaCodec/SVT-AV1); our changes live on the
`imazen-parity` branch as a single commit on the `v4.2.0` tag (the gated
`SVT_HDR_MODE` option), so we can rebase onto future upstream tags. It is the
differential oracle the port is tested against; it is not in the shipping path
of the Rust crates.

## Two modes, two verification bars

| Mode | What it is | Bar |
|---|---|---|
| **Mainline** (default) | Stock SVT-AV1 v4.2.0 behavior | **Byte-identical bitstreams** vs the real C library at matched configs |
| **HdrFork** (`HdrForkConfig::hdr_fork()`) | The svt-av1-hdr feature set: psychovisual RD, quant matrices, photon-noise synthesis, variance boost, six tune policies | **Byte-identical bitstreams** vs a `SVT_HDR_MODE=ON` build of the same C base at **10-bit** (`rust/tools/hdr_bd10_gate.sh`, 64/64, a standing gate). At **8-bit** the fork was measured 48/48 byte-identical on 2026-07-19 (`rust/docs/HDR-ON-4.2.md`) but has **no standing gate script**; its standing 8-bit coverage is functional — per-kernel C differentials, per-knob liveness witnesses and `aomdec` decode gates (`hdr_fork_e2e`) |

We rebased the fork's features onto v4.2.0 ourselves behind compile-time gates
(see `rust/docs/HDR-ON-4.2.md`), so both modes have a real C twin to compare
bytes against.

## Status — what is byte-identical today

Verified via OBU byte comparison **plus** a full arithmetic-coder op trace
(every range-coder call, including coder state), each line an asserted gate
under `rust/tools/`. The first block runs in CI on every push (tallies as of `84621b20d` (CI run 33978673841, 2026-09-05)
— `gh run view 33978673841`); the second block needs image corpora that are
not in-tree, so those tallies are dated local measurements with the committed
record named:

| Axis (CI, every push) | Gate | Cells |
|---|---|---|
| Synthetic, **every preset 0–13**, full qp range, 4 content classes, 64 px | `identity_full_8bit` (superset of the 54-cell `identity_matrix`) | **280/280** |
| Partial superblocks / odd dimensions (spec 5.11.4 edges) | `partial_sb_gate` | **145/145** |
| True-vs-aligned × stride × bit depth, two oracles (bytes vs C AND recon vs `aomdec`) | `alignment_gate` | **74/74** |
| 10-bit synthetic, presets 0–13 | `bd10_matrix` + `bd10_nonflat_gate` + `bd10_hbd_src_gate` | **36/36** + **309/309** + **118/118** |
| 10-bit at non-64-aligned dims, both bd10 producers | `bd10_partial_sb_gate` | **159/159** |
| SB128 superblocks (incl. high-qp partition depths) | `sb128_gate` | **22/22** (CI fetches gb82-sc, so the corpus cells run; 18 without it) |
| Multi-tile (rows × cols, all preset bands) | `tile_gate` | **29/29** |
| Feature intersections: SB128×tiles, bd10×tiles | `coverage_combos_gate` (`CC_AXES="sb128 bd10"`) | **28/28**² |
| Screen content, palette, 8- AND 10-bit | `screen_palette_bd_gate` | **60/60** |
| Screen content, **IntraBC**, gb82-sc × presets 0–4 × qp | `screen_ibc_byte_gate` | **152/152** (41,559 IntraBC blocks, 27,402 palette)³ |
| Screen content, luma palette, gb82-sc × preset 6 | `screen_palette_gate` | **50/50** |
| 10-bit screen content: panic-freedom + decodability | `bd10_screen_panic_gate` | **60/60** |
| Superres, byte-parity + decode at the upscaled size | `superres_gate` | **512/512** |
| Encoder recon == `aomdec` output, byte-exact | `recon_parity` + `variance_boost_recon` | **432/432** + **60/60** |
| Decode conformance (`aomdec` + `dav1d`), mono / 4:2:0 | `decode_conformance` | **1260** / **1575** streams |
| Arbitrary dimensions: panic-free + decodable, every preset | `arbitrary_size_robustness` | **128/128** (0 refused) |
| Regression spot-check (one cell per bug ever fixed) | `regression_spotcheck` | **104/104** |
| Coded-lossless (QP 0): bytes vs C AND `aomdec` output == source | `lossless_gate` | **112/144** byte-identical, +32 pinned, 144/144 lossless (local 2026-08-28; CI runs the 72-cell subset) |

**Inter / video mode (CI, every push).** The inter path is byte-gated but the
shipped `EncodePipeline` still refuses non-key frames — the gates drive it
through `SVTAV1_INTER_EXPERIMENTAL`, a harness-only switch
(`rust/docs/WORKING-ON-THIS.md` §7b). The campaign is
`rust/docs/INTER-ENCODE-PLAN.md`.

| Axis (CI, every push) | Gate | Cells |
|---|---|---|
| 2-frame low-delay P, **both frames** byte-identical to C | `inter_byte_gate` | **108 required, 0 failed, 1 known-open** (`diag 128x128 q20 p8`) |
| Does the port ENCODE the cell at all (panic gate) | `inter_completion_scan` (`SCAN_GATE=1`) | **64 OK / 0 REFUSED / 0 CRASH** per content |
| Does the port's stream DECODE, all grid cells | `inter_decode_census` | **96/96** |
| CDF continuation · inter frame header · decode | `fctx_gate` · `inter_fh_gate` · `inter_decode_gate` | PASS · PASS · **5/5** |
| Port's full-pel MV against C's, per block | `inter_me_join_gate` | 6 cells, 54 joined rows, **0 disagree** |

| Axis (local, corpus-gated) | Gate | Cells |
|---|---|---|
| Real photographs, presets **0–13**, 8-bit (CID22) | `identity_full_8bit` real tier | 403/450, 2026-08-03 (`rust/benchmarks/identity_full_8bit_real_2026-08-03.tsv`): p6/p10/p13 **90/90 each**, p0 66/90, p4 67/90¹ |
| Real photographs, preset 0, 8-bit | `photo_p0_gate` | **8/8** (closed 2026-07-23, `rust/STATUS.md`; no committed artifact) |
| 10-bit real photographs, presets **0–13** (CID22 + CLIC) | `bd10_photo_gate` | **191/191** (191 = the script's cell count, groups A–H; the tally is a local run recorded here 2026-07-24 with no committed 191-cell artifact — the committed record is the p0–p3 `bd10_photo_p0p3_2026-07-23.tsv`) |
| Feature intersections: real×tiles | `coverage_combos_gate` axis 3 | 8/12 byte-exact, 4 pinned (2026-07-22, `coverage_combos_latest.tsv`) |
| Screen content p0–p4 with **IntraBC**, recon vs `decode_diff` | `screen_ibc_gate` | **100/100** (promoted 2026-09-05, `8528c3ef6`; the byte-only twin runs in CI — see above) |
| HDR-fork mode, 10-bit byte-vs-C / 8-bit | `hdr_bd10_gate` / `hdr_fork_e2e`⁴ | **64/64** / 36/36 decode⁴ |

¹ the earlier "177/180 `real_image_matrix`" figure was a 2026-07 local run with
no committed artifact; the committed real-corpus record is the 450-cell sweep
above. The p0/p4 residual is real-content RD divergence at the low presets
(pinned per cell in that TSV), not a decode issue — every cell decodes.
² 28/28 over the two CI axes (`sb128 bd10`) on 2026-09-05 (run 33978673841);
the earlier 16/16 + pins reading is superseded. The bd10×tiles pins were
localized eff-M9 tile-boundary partition near-ties and the gate self-promotes
(a pin that starts matching fails the gate until it is promoted). Local arm64
record: `rust/benchmarks/coverage_combos_2026-08-28_arm64_axes12.tsv`.
³ IntraBC (intra block copy) is fully implemented — hash pyramid, diamond+mesh
DV search, MVP stack, inter var-tx coding — and every stream is self-consistent
(decodes to exactly the encoder's own reconstruction). **The gb82-sc IntraBC
band closed on 2026-09-05** (`8528c3ef6`): the 78 cells that had been pinned as
RD near-ties are byte-identical, `screen_ibc_gate`'s `BYTE_EXACT` list is all
100, and `screen_ibc_byte_gate.sh` asserts 152 cells in CI without needing the
`decode_diff` oracle that only builds on the CI image.
⁴ `hdr_fork_e2e` is a liveness + `aomdec` decode witness suite (36/36 per-tune
decode gates), not a byte-vs-C gate. The 8-bit fork's 48/48 byte-identity is a
2026-07-19 measurement recorded in `rust/docs/HDR-ON-4.2.md` with no standing
gate script; only the 10-bit fork is byte-gated (`hdr_bd10_gate`).

Every gated stream decodes under `aomdec` and `dav1d`, and the decoder's output
matches the encoder's own reconstruction byte-for-byte. Known open gaps are
tracked, not hidden — the pinned-cell maps live in `rust/benchmarks/` and the
port maps in `rust/docs/`.

**Envelope:** 8- and 10-bit, 4:2:0 (and luma-only/monochrome), single frame.
4:4:4 / 4:2:2 / 12-bit are **not port gaps** — C SVT-AV1 v4.2.0 itself rejects
them at init (`enc_settings.c:460` permits only 8/10-bit; `:470` "Only support
420 now"), so the port already matches C's shipping *format* envelope exactly;
the 422/444/12-bit code in the C tree is dead-gated behind those lines. **QP 0
(coded-lossless)** is implemented on the 8-bit 4:2:0 still path (issue #5):
TX_4X4 Walsh-Hadamard txbs, no in-loop filters, byte-identical to C at presets
4–13 and lossless under `aomdec` at every preset (`rust/tools/lossless_gate.sh`);
presets 0–3 are pinned byte-diverging (lossless in both encoders). 10-bit, mono,
fork-mode, screen-content and superres at QP 0 are refused with a typed error.
**Monochrome** is decode-conformance-validated (aomdec + dav1d accept it, and the
decoder output matches the encoder's recon bit-for-bit) rather than byte-vs-C —
C v4.2.0 can't encode mono (`EB_YUV400` is rejected at init), so no C oracle
exists for it; the byte-identity gates below run the 4:2:0 path.

**Rate control for a single still is already covered.** SVT-AV1's default /
guide-recommended still mode is CRF, but for one frame `--crf N` == `--cqp N` ==
`--qp N` **byte-for-byte** — aq-mode-2's deltaq needs TPL lookahead, which a single
still frame has none of (verified against the C encoder,
[benchmark](rust/benchmarks/crf_cqp_equivalence_2026-07-24.md)). So the port's
`qp = N` already emits SVT-AV1's default-CRF bytes; `RcMode::Crf` == `Cqp` is
correct-by-design, not a stub. VBR/CBR bitrate-targeting is multi-frame / degenerate
for one still (iterate CRF to hit a size). Multi-frame / GOP / ALT-REF are the
separate video-scale future.

Two envelope details for *consumers*: the 10-bit path is byte-gated internally,
and the public encode API takes **native 10-bit `&[u16]` input** via
`try_encode_frame_420_hbd` / `try_encode_frame_hbd` — the low 2 bits reach the
mode decision, the coded levels, and the deblock/CDEF/Wiener searches
(`tools/bd10_hbd_src_gate.sh`, 100/100 vs C). Envelope: 64-aligned dims and
either preset ≥ 9 or a full-RD-capable preset ≤ 8; out-of-envelope configs are
rejected with `UnsupportedConfig`, never silently truncated.
Non-multiple-of-64 dimensions encode at **preset ≥ 6** (partial superblocks,
byte-identical); presets 0–5 require multiples of 64.

## Production API

The encoder is hardened for library use, not just parity testing:

- **Typed, located errors** — `try_encode_frame` / `try_encode_frame_420`
  return `EncodeResult<Vec<u8>>` with `EncodeError`
  (`InvalidDimensions` / `UnsupportedConfig` / `AllocFailed` / `Cancelled`)
  carrying [`whereat`](https://lib.rs/crates/whereat) source locations. The
  legacy `encode_frame*` keep their panicking contract.
- **Cooperative cancellation** — `with_stop(...)` accepts any
  [`enough`](https://lib.rs/crates/enough)`::Stop`; the encode checks at
  superblock-row granularity.
- **Bounded threading** — `with_thread_count(n)` caps the tile-parallel spawn
  (0 = auto); output is byte-identical at every thread count.
- **Fallible allocation** — the `fallible-alloc` feature routes every
  frame-scaled buffer through `try_reserve` so untrusted dimensions return
  `Err(AllocFailed)` instead of aborting.
- **CICP color description** — `with_color_space(cp, tc, mc, full_range)` plus
  presets incl. Display-P3, BT.2020+PQ (HDR10) and HLG; written into the
  sequence header exactly as C does.
- Deterministic: repeated encodes are byte-identical, and
  `#![forbid(unsafe_code)]` holds across every crate in the product path.

## Install

```toml
[dependencies]
zenav1-svt = { git = "https://github.com/imazen/zenav1-svt" }
```

Requires Rust 1.89+ (2024 edition; `rust-version` in `rust/Cargo.toml` is the
floor CI exercises). **No C toolchain, no cmake, no `build.rs`**
in the product crates — the port is pure safe Rust; the C reference is a
*test-time* dependency only (and only after `git submodule update --init`).

```rust
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

let rc = RcConfig { mode: RcMode::Cqp, qp: 40, ..RcConfig::default() };
let mut p = EncodePipeline::new(128, 128, /*preset*/ 6, rc, 4, 1)
    .with_chroma_420(true);

// Optional — default is mainline (byte-identical to C):
// p.hdr = svtav1_encoder::hdr_mode::HdrForkConfig::hdr_fork();
// p.hdr.tune = 3; // 0=VQ 1=PSNR 2=SSIM 3=IQ 4=MS_SSIM 5=FilmGrain

let obu = p.try_encode_frame_420(&y, &u, &v, /*y stride*/ 128)?;
```

`svtav1::avif::AvifEncoder` wraps this with quality/speed mapping and AVIF
defaults. With `avif-container`, `encode_animation_yuv420` accepts 8-bit frames,
and `encode_animation_yuv420_hbd` accepts native 10-bit `u16` frames with
`with_bit_depth(10)`. The `_with_options` variants accept `AnimationOptions`
for repetition, ICC, Exif, XMP, CLLI, MDCV, and premultiplied-alpha association.
Native 10-bit currently requires 64-aligned dimensions, and alpha requires
preset 9 or higher. [The animation plan](rust/docs/ANIMATED-AVIF-PLAN.md) lists
verification and remaining format, lossless, spatial-property and video work.

Rust paths use the short `svtav1_*` names; the *package* names carry
the `zenav1-svt-` prefix (see [PORTING.md](PORTING.md)).

## Testing on a fresh box

```bash
git clone --recurse-submodules https://github.com/imazen/zenav1-svt && cd zenav1-svt

# 1. The C reference oracle is built BY CARGO (rust/crates/svtav1-cref/build.rs):
#    the first `cargo test` / `cargo build -p zenav1-svt-cref` configures and
#    builds BOTH variants — Bin/Release (mainline, SVT_HDR_MODE=OFF) and
#    Bin/ReleaseHdr (fork, =ON) — keyed on the submodule's commit, so an
#    unchanged tree never rebuilds. It needs cmake + a C compiler (+ nasm on
#    x86, ninja optional) and panics with the install one-liner if one is
#    missing. Knobs: SVT_CREF_LIB_DIR=<dir> links a prebuilt libSvtAv1Enc.a
#    instead; SVT_CREF_SKIP_HDR=1 skips the fork variant.
#    (Consumers of the published crates never run this — see "Install".)

# 2. Tooling the gates assume but cargo does not install: the test runner,
#    `just` (the recipes in rust/justfile), and the AV1 reference decoder
#    (`aomdec` — the recon/decode gates and svtav1/tests/issue13_repro.rs need
#    it on PATH or in $AOMDEC; `dav1d` for decode_conformance.sh).
#    `tools/screen_ibc_gate.sh` additionally needs the tools/decode_diff
#    binary built (see rust/docs/WORKING-ON-THIS.md).
cargo install cargo-nextest just

# 3. Run the port's tests and byte-identity gates.
cd rust
cargo nextest run --workspace
export SVT_CREF_LIB_DIR=$(pwd)/../Bin/Release
./tools/identity_full_8bit.sh     # 1100 cells (synthetic + dims) — THE 8-bit gate
./tools/identity_matrix.sh        #   54 cells (a scoreboard: it always exits 0)
./tools/partial_sb_gate.sh        #  145 cells
./tools/bd10_photo_gate.sh        # 191 cells (needs the CID22/CLIC corpus
                                  #  paths — see the script header)
```

Each gate prints `<pass> / <total> byte-identical`. To drill into one cell:
`./tools/drill_cell.sh <content> <w> <h> <qp> <preset>` (encode both sides,
locate the first divergent block, dump both decision trees).

The fork-mode gates (`tools/hdr_bd10_gate.sh`) link the ON oracle that the
same cargo build produced in `Bin/ReleaseHdr`; the harness selects it via
`SVT_HDR_MODE=1` (see `rust/tools/capture_c_trace/build.sh`).

## Layout

```
rust/               the Rust port — start at rust/README.md
  crates/             types, tables, dsp, entropy, encoder, cref (test-only FFI)
  svtav1/             the zenav1-svt facade: public API, AVIF backend, examples
  tools/              identity + differential gates (the tables above)
  docs/               port maps, identity-campaign history, HDR-ON-4.2.md
  benchmarks/         committed gate scoreboards + perf records
reference/svt-av1   git submodule → imazen/zenav1-svt-c @ imazen-parity (C oracle)
specs/              our AV1 algorithm specifications (port docs, read-only)
PORTING.md          which C file each Rust module ports, and its gate
```

## The C baseline

[`imazen/zenav1-svt-c`](https://github.com/imazen/zenav1-svt-c) is a fork of
upstream **SVT-AV1** (full history + tags on `master`); the `imazen-parity`
branch is **v4.2.0** plus one patch: an OFF-by-default `SVT_HDR_MODE` CMake option
(15 guarded files) that switches the C build between mainline and svt-av1-hdr
semantics on the same base.

```bash
cmake -S reference/svt-av1 -B build -DSVT_HDR_MODE=OFF  # default — mainline v4.2.0
cmake -S reference/svt-av1 -B build -DSVT_HDR_MODE=ON   # svt-av1-hdr semantics
```

The port's mainline mode is byte-gated against the OFF build; fork mode against
the ON build. To bump the C baseline: in the `zenav1-svt-c` fork, fetch upstream
(its `master` mirrors gitlab SVT-AV1) and rebase the `imazen-parity` branch onto
the new tag, then bump the submodule pin here and re-run every gate — divergences
are real work, and the gates are the todo list.

## License

The Rust port (`rust/`, and everything outside the submodule) is dual-licensed
**AGPL-3.0-only OR a commercial license** — the standard Imazen "zen" model
(same as zenavif et al.): [LICENSE-AGPL3](LICENSE-AGPL3) /
[LICENSE-COMMERCIAL](LICENSE-COMMERCIAL). Use it under the AGPL, or
[contact Imazen](https://imazen.io) for a commercial license.

**If someone covers Imazen's 2026 AI + server costs, we'll release the port
under MIT or the original upstream license.**

The SVT-AV1 **C tree** (the `reference/svt-av1` submodule) keeps its upstream
licensing: BSD-3-Clause-Clear plus the Alliance for Open Media Patent License
1.0 — see `LICENSE.md` / `PATENTS.md` *inside the submodule*. The Rust port is
a derivative work of that BSD-licensed C source; its upstream attribution and
patent terms are preserved, and relicensing the derivative is permitted by
BSD-3-Clause-Clear.

## Acknowledgments

- [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (Intel / Alliance for Open
  Media) — the battle-tested C encoder this port is built on
- [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) (juliobbv-p) — the
  perceptual/HDR feature set ported in fork mode
- [rav1d](https://github.com/memorysafety/rav1d) — safe Rust AV1 decoder
- [archmage](https://github.com/imazen/archmage) — safe SIMD dispatch via CPU
  feature tokens
