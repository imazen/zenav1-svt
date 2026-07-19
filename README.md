# zenav1-svt

A pure-Rust, still-picture (AVIF / all-intra) AV1 encoder — an
algorithm-for-algorithm port of [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1)
v4.2.0, verified **byte-identical** to the C encoder on its tested envelope, with
the [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) fork's perceptual
feature set available behind a runtime switch.

**`#![forbid(unsafe_code)]` · ~75k lines · 7 crates · 775+ tests · no C in the product path**

> **Experimental.** The envelope below is real and gated, but it is an envelope:
> single still frame, CQP, `--lp 1`. Not yet a general-purpose video encoder.
> Crates are not on crates.io yet — depend on it by git.

This repository is **both** the SVT-AV1 C fork and its Rust port. The C tree
(`Source/`) is the reference implementation and the differential oracle the port
is tested against — it is not in the shipping path of the Rust crates.

## Two modes, two verification bars

| Mode | What it is | Bar |
|---|---|---|
| **Mainline** (default) | Stock SVT-AV1 v4.2.0 behavior | **Byte-identical bitstreams** vs the real C library at matched configs |
| **HdrFork** (`HdrForkConfig::hdr_fork()`) | The svt-av1-hdr feature set: psychovisual RD, quant matrices, photon-noise synthesis, variance boost, six tune policies | **Functionally verified**: per-kernel differentials against the exported C functions, per-knob liveness witnesses, `aomdec` decode gates. *Not* byte-gated — the fork has no C twin at this base |

That distinction is the honest summary of the project. Mainline mode is a
drop-in bit-exact reimplementation on the envelope we have tested; fork mode is a
faithful functional port whose every kernel is differentially tested against real
C code, but whose end-to-end bytes have nothing to compare against (we rebased
the fork onto v4.2.0 ourselves — see `rust/docs/HDR-ON-4.2.md`).

## Install

```toml
[dependencies]
zenav1-svt = { git = "https://github.com/imazen/zenav1-svt" }
```

Requires Rust 1.85+ (2024 edition). **No C toolchain, no cmake, no `build.rs`** —
the port is pure safe Rust; the C reference is a *test-time* dependency only.

```rust
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

let rc = RcConfig { mode: RcMode::Cqp, qp: 40, ..RcConfig::default() };
let mut p = EncodePipeline::new(128, 128, /*preset*/ 6, rc, 4, 1);
p.chroma_420 = true;

// Optional — default is mainline (byte-identical to C):
// p.hdr = svtav1_encoder::hdr_mode::HdrForkConfig::hdr_fork();
// p.hdr.tune = 3; // 0=VQ 1=PSNR 2=SSIM 3=IQ 4=MS_SSIM 5=FilmGrain

let obu = p.encode_frame_420(&y, &u, &v, /*y stride*/ 128);
```

`svtav1::avif::AvifEncoder` wraps this with quality/speed mapping and AVIF
defaults. Rust paths use the short `svtav1_*` names; the *package* names carry
the `zenav1-svt-` prefix (see [PORTING.md](PORTING.md)).

## Status

Byte-identical to the C encoder, verified via OBU comparison **plus** a full
arithmetic-coder op trace (every range-coder call, including coder state):

- **Synthetic identity matrix** — {uniform, gradient} × {64, 128, 60} px × qp
  {20, 40, 55} × presets {13, 10, 6}: **54/54**.
- **Partial superblocks / odd dimensions** — non-64-multiple frames with
  spec-5.11.4 partition edges, presets 6–13: **101/101**.
- **10-bit** — uniform across presets M0–M13 **36/36**; non-flat (coded residual,
  u16 mode funnel) **79/79**.
- Every gated stream decodes under `aomdec` and `dav1d`, and the decoder's output
  matches the encoder's own reconstruction byte-for-byte.

Known open gaps are tracked, not hidden — see `rust/docs/IDENTITY-STATUS.md`
(full divergence map) and the port maps in `rust/docs/`. Current envelope: 8- and
10-bit, 4:2:0, single frame, single-threaded.

## Testing on a fresh box

```bash
git clone https://github.com/imazen/zenav1-svt && cd zenav1-svt

# 1. Build the C reference (the differential oracle). Needs cmake + nasm + cc.
cmake -S . -B cbuild-static -DCMAKE_BUILD_TYPE=Release \
      -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF
cmake --build cbuild-static -j

# 2. Run the port's tests and byte-identity gates.
cd rust
cargo test --workspace
./tools/identity_matrix.sh        #  54 cells
./tools/partial_sb_gate.sh        # 101 cells
./tools/bd10_matrix.sh            #  36 cells
./tools/bd10_nonflat_gate.sh      #  79 cells
```

Each gate prints `<pass> / <total> byte-identical`. To drill into one cell:
`./tools/identity_diff.sh <w> <h> <qp> <preset> <content>`.

## Layout

```
rust/            the Rust port  — start at rust/README.md
  crates/          types, tables, dsp, entropy, encoder, cref (test-only)
  svtav1/          the zenav1-svt facade: public API, AVIF backend, examples
  tools/           identity + differential harnesses
  docs/            port maps, identity campaign history
Source/           the SVT-AV1 C fork (reference + oracle) — read-only
PORTING.md        which C file each Rust module ports, and its gate
README-SVT-AV1.md the upstream SVT-AV1 project README (C encoder docs, Docs/)
```

## The C baseline

The C tree is **SVT-AV1 v4.2.0** plus one patch: an OFF-by-default `SVT_HDR_MODE`
CMake option (15 guarded files) that switches the C build between mainline and
svt-av1-hdr semantics on the same base.

```bash
cmake -S . -B build -DSVT_HDR_MODE=OFF   # default — mainline v4.2.0 behavior
cmake -S . -B build -DSVT_HDR_MODE=ON    # svt-av1-hdr (Chromedome) semantics
```

The Rust port's mainline mode is byte-gated against the OFF build; fork mode
targets the ON build's semantics.

To bump the C baseline, merge an upstream tag — the gitlab remote is already
configured as `upstream`:

```bash
git fetch upstream --tags
git merge v4.3.0        # then re-run the gates; divergences are real work
```

## License

The Rust port (`rust/`) is dual-licensed **AGPL-3.0-only OR a commercial
license** — the standard Imazen "zen" model (same as zenavif et al.):
[rust/LICENSE-AGPL3](rust/LICENSE-AGPL3) /
[rust/LICENSE-COMMERCIAL](rust/LICENSE-COMMERCIAL). Use it under the AGPL, or
[contact Imazen](https://imazen.io) for a commercial license.

**If someone covers Imazen's 2026 AI + server costs, we'll release the port under
MIT or the original upstream license.**

The SVT-AV1 **C tree** (`Source/`, `Docs/`, the build system) keeps its upstream
licensing: BSD-3-Clause-Clear plus the Alliance for Open Media Patent License 1.0
— see [LICENSE.md](LICENSE.md), [LICENSE-BSD2.md](LICENSE-BSD2.md) and
[PATENTS.md](PATENTS.md). The Rust port is a derivative work of that
BSD-licensed C source; its upstream attribution and patent terms are preserved,
and relicensing the derivative is permitted by BSD-3-Clause-Clear.

## Acknowledgments

- [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (Intel / Alliance for Open
  Media) — the battle-tested C encoder this port is built on
- [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) (juliobbv-p) — the
  perceptual/HDR feature set ported in fork mode
- [rav1d](https://github.com/memorysafety/rav1d) — safe Rust AV1 decoder
- [archmage](https://github.com/imazen/archmage) — safe SIMD dispatch via CPU
  feature tokens
