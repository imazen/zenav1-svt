# zenav1-svt

A pure-Rust, still-picture (AVIF / all-intra) AV1 encoder — an
algorithm-for-algorithm port of [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1)
v4.2.0, verified **byte-identical** to the C encoder across its tested envelope,
with the [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) fork's
perceptual feature set available behind a runtime switch — and *that* mode
byte-gated too, against a `SVT_HDR_MODE=ON` build of the same C base.

**`#![forbid(unsafe_code)]` · ~80k lines · 7 crates · 900+ tests · no C in the product path**

> **Experimental.** The envelope below is real and gated, but it is an envelope:
> single still frame, CQP, single tile group semantics (`--lp 1`). Not yet a
> general-purpose video encoder. Crates are not on crates.io yet — depend by git.

The SVT-AV1 C tree is **not vendored here** — it lives in the
[`imazen/svt-av1-ref`](https://github.com/imazen/svt-av1-ref) submodule at
`reference/svt-av1` (SVT-AV1 v4.2.0 + a gated `SVT_HDR_MODE` option). It is the
differential oracle the port is tested against; it is not in the shipping path
of the Rust crates.

## Two modes, two verification bars

| Mode | What it is | Bar |
|---|---|---|
| **Mainline** (default) | Stock SVT-AV1 v4.2.0 behavior | **Byte-identical bitstreams** vs the real C library at matched configs |
| **HdrFork** (`HdrForkConfig::hdr_fork()`) | The svt-av1-hdr feature set: psychovisual RD, quant matrices, photon-noise synthesis, variance boost, six tune policies | **Byte-identical bitstreams** vs a `SVT_HDR_MODE=ON` build of the same C base (8-bit 48/48, 10-bit 64/64 gated cells) |

We rebased the fork's features onto v4.2.0 ourselves behind compile-time gates
(see `rust/docs/HDR-ON-4.2.md`), so both modes have a real C twin to compare
bytes against.

## Status — what is byte-identical today

Verified via OBU byte comparison **plus** a full arithmetic-coder op trace
(every range-coder call, including coder state), each line an asserted gate
under `rust/tools/`:

| Axis | Gate | Cells |
|---|---|---|
| Synthetic matrix (content × size × qp × preset) | `identity_matrix` | **54/54** |
| Partial superblocks / odd dimensions (spec 5.11.4 edges) | `partial_sb_gate` | **101/101** |
| Real photographs, presets **0–13**, 8-bit (CID22) | `photo_p0_gate` + `real_image_matrix` | **8/8** + 177/180¹ |
| 10-bit synthetic, presets 0–13 | `bd10_matrix` + `bd10_nonflat_gate` | **36/36** + **309/309** |
| 10-bit real photographs, presets **0–13** (CID22 + CLIC) | `bd10_photo_gate` | **191/191** |
| SB128 superblocks (incl. high-qp partition depths) | `sb128_gate` | **22/22** |
| Multi-tile (rows × cols, all preset bands) | `tile_gate` | **29/29** |
| Feature intersections: SB128×tiles, bd10×tiles, real×tiles | `coverage_combos_gate` | **40/40**² |
| Screen content, palette, preset 6 | `screen_palette_gate` | **50/50** |
| Screen content p0–p4 with **IntraBC** | `screen_ibc_gate` | **20/100**³ |
| HDR-fork mode, 8-bit / 10-bit | `hdr_fork_e2e` + `hdr_bd10_gate` | **48/48** / **64/64** |
| Arbitrary dimensions: panic-free + decodable, every preset | `arbitrary_size_robustness` | **57/57** |

¹ the 3 open cells are a pinned palette near-tie on one image (tracked, self-promoting).
² 16/16 SB128×tiles byte-exact; the 12 pinned cells are localized eff-M9
tile-boundary partition near-ties (bd10 / real content).
³ IntraBC (intra block copy) is fully implemented — hash pyramid, diamond+mesh
DV search, MVP stack, inter var-tx coding — and every stream is self-consistent
(decodes to exactly the encoder's own reconstruction; 25k+ IBC blocks verified,
zero desync). The 80 open cells are pinned RD near-ties, each localized; the
gate self-promotes them as they close.

Every gated stream decodes under `aomdec` and `dav1d`, and the decoder's output
matches the encoder's own reconstruction byte-for-byte. Known open gaps are
tracked, not hidden — the pinned-cell maps live in `rust/benchmarks/` and the
port maps in `rust/docs/`.

**Envelope:** 8- and 10-bit, 4:2:0 (and luma-only/monochrome), single frame.
4:4:4 / 4:2:2 / 12-bit are out of scope because upstream SVT-AV1 rejects them at
init (no oracle exists). QP 0 (coded-lossless) is rejected with a typed error
rather than implemented (issue #5). Multi-frame / rate control beyond CQP are
future program-scale work.

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

Requires Rust 1.85+ (2024 edition). **No C toolchain, no cmake, no `build.rs`**
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
defaults. Rust paths use the short `svtav1_*` names; the *package* names carry
the `zenav1-svt-` prefix (see [PORTING.md](PORTING.md)).

## Testing on a fresh box

```bash
git clone --recurse-submodules https://github.com/imazen/zenav1-svt && cd zenav1-svt

# 1. Build the C reference oracle from the submodule. Needs cmake + nasm + cc.
cmake -S reference/svt-av1 -B cbuild-static -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_OUTPUT_DIRECTORY="$PWD/Bin/Release/" \
      -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF
cmake --build cbuild-static -j

# 2. Run the port's tests and byte-identity gates.
cd rust
cargo nextest run --workspace     # or: cargo test --workspace
export SVT_CREF_LIB_DIR=$(pwd)/../Bin/Release
./tools/identity_matrix.sh        #  54 cells
./tools/partial_sb_gate.sh        # 101 cells
./tools/bd10_photo_gate.sh        # 191 cells (needs the CID22/CLIC corpus
                                  #  paths — see the script header)
```

Each gate prints `<pass> / <total> byte-identical`. To drill into one cell:
`./tools/drill_cell.sh <content> <w> <h> <qp> <preset>` (encode both sides,
locate the first divergent block, dump both decision trees).

For fork-mode gates, configure the ON oracle too:
`cmake -S reference/svt-av1 -B cbuild-static-hdr -DSVT_HDR_MODE=ON -DCMAKE_OUTPUT_DIRECTORY="$PWD/Bin/ReleaseHdr/" …` — the
harness selects it via `SVT_HDR_MODE=1` (see
`rust/tools/capture_c_trace/build.sh`).

## Layout

```
rust/               the Rust port — start at rust/README.md
  crates/             types, tables, dsp, entropy, encoder, cref (test-only FFI)
  svtav1/             the zenav1-svt facade: public API, AVIF backend, examples
  tools/              identity + differential gates (the tables above)
  docs/               port maps, identity-campaign history, HDR-ON-4.2.md
  benchmarks/         committed gate scoreboards + perf records
reference/svt-av1   git submodule → imazen/svt-av1-ref (the C oracle, read-only)
PORTING.md          which C file each Rust module ports, and its gate
```

## The C baseline

[`imazen/svt-av1-ref`](https://github.com/imazen/svt-av1-ref) is **SVT-AV1
v4.2.0** plus one patch: an OFF-by-default `SVT_HDR_MODE` CMake option
(15 guarded files) that switches the C build between mainline and svt-av1-hdr
semantics on the same base.

```bash
cmake -S reference/svt-av1 -B build -DSVT_HDR_MODE=OFF  # default — mainline v4.2.0
cmake -S reference/svt-av1 -B build -DSVT_HDR_MODE=ON   # svt-av1-hdr semantics
```

The port's mainline mode is byte-gated against the OFF build; fork mode against
the ON build. To bump the C baseline, merge the upstream tag inside the
submodule repo, then re-run every gate here — divergences are real work, and
the gates are the todo list.

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
