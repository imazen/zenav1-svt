# zenav1-svt

Experimental pure-Rust port of SVT-AV1 v4.2.0, with explicit C-reference
selection and opt-in HDR-fork/Zen extensions. The product supports 8/10-bit
4:2:0 still encoding and all-intra animated AVIF, plus Rust monochrome/alpha
extensions. General public streaming video is unfinished and refuses calls.
There is no C dependency in the product library path.

Start with [the current handoff](CONTEXT-HANDOFF.md),
[the feature/support table](rust/docs/API-SUPPORT-AUDIT-2026-09-08.md), and
[the remaining-work tracker](https://github.com/imazen/zenav1-svt/issues/21).

## References, policy and coverage

- Legacy constructors retain **Hybrid3115**, the historical patched-C oracle.
  This is not synonymous with pristine mainline, even with HDR knobs off.
- `SvtReference::Mainline420` names pristine v4.2.0. `SvtParity(reference)`
  restricts the request to the named C envelope and rejects Zen enhancements.
  It does not certify every untested combination or erase known divergences.
- Checked native presets include **−1 through 13**. Effort currently resolves
  to native buckets; fractional adaptive search is not implemented.
- Grain modeling/denoising/tables/synthesis and named Zen intra-edge/restoration
  experiments are wired in their documented envelopes. Experiments stay opt-in.
- Wider chroma and 12-bit are rejected by C SVT and this backend. They are useful
  alternate-backend work, not missing shipping-C translations.

At implementation main **`0cbd1279`**, the latest native workspace gate passed
**2631/2631 tests, zero skips**. PR #20 uses published archmage/magetypes
**0.9.29**; 19 explicitly selected ARM SAD/variance tests passed under QEMU.
[Verification and pre-existing ARM Clippy/MSRV debt](rust/benchmarks/arm_pairwise_release_2026-09-08.md).
These local checks are not a claim of a new CI run.

The preceding eight-bit landing matrix passed **1100/1100** in its named
reference envelope. **Four native10 parity cells remain open**; their streams
decode but differ from C. See [identity status](rust/docs/IDENTITY-STATUS.md).
HDR MODE=ON has a standing 10-bit gate; older 8-bit 48/48 prose is historical,
without a corresponding retained standing gate. Per-reference, per-ISA and
corpus boundaries matter. No universal C parity or calibrated RD/time routing
is claimed.

## Convert a GIF to an animated AVIF

```rust
use svtav1::{avif::AvifEncoder, rgba::RgbaFrame};

let (meta, gif, _) = zengif::decode_gif(&bytes, Default::default(), &zengif::Unstoppable)?;
let frames: Vec<RgbaFrame> = /* full-canvas RGBA + duration_ms per frame */;
let avif = AvifEncoder::new()
    .with_quality(70.0)
    .encode_rgba_animation(&frames, meta.width.into(), meta.height.into())?;
std::fs::write("out.avif", &avif)?;
```

Runnable end to end, decode included:

```sh
cargo run --release --features avif-container --example gif_to_avif -- in.gif out.avif 70
```

`encode_rgba_animation` does the BT.601 conversion, the 4:2:0 subsampling, the
alpha-plane decision (a plane is written only if some pixel is non-opaque) and
the container muxing. The output carries a still poster image and a timed AV1
sequence track, and decodes in `avifdec`, `dav1d` and `ffmpeg`.

MP4 input and true inter-frame AV1 output are not wired yet — see the table
below.

## Support status

The four columns mean exactly this, and the distinction is the point:

| status | meaning |
|---|---|
| **Validated** | Implemented AND held by a standing gate that would fail if it broke. The gate is named. |
| **Supported** | Implemented and exercised, but without a gate that isolates *this* feature — a regression could hide inside a broader byte-identity run. |
| **Partial** | Implemented for part of its envelope. The rest REFUSES rather than emitting a guess; the limit is named. |
| **Not supported** | Refused at the API with a message naming the gap. Some of it is ported but unwired — that is listed too, because "the code exists" and "the encoder uses it" are different facts. |

A refusal is a deliberate design choice in this port: an out-of-envelope
configuration is rejected rather than encoded as a plausible-but-wrong stream.
`rust/docs/REFUSED-CONFIGS.md` is the generated inventory, split into
CAPABILITY (debt) and CONTRACT (permanent caller misuse).

### Still image — the product path

| Feature | Status | Evidence / limit |
|---|---|---|
| 8-bit 4:2:0 still, presets −1…13, full qp range | **Validated** | `identity_full_8bit.sh` — 1100/1100 byte-identical to C |
| 10-bit 4:2:0 still (photographic) | **Validated** | `bd10_photo_gate.sh` 191/191, `bd10_nonflat_gate.sh` 309/309 |
| Non-64-aligned / partial superblocks | **Validated** | `bd10_partial_sb_gate.sh` 159/159, `partial_sb_gate.sh`, `alignment_gate.sh` |
| Palette (screen content) | **Validated** | `screen_palette_gate.sh` 50/50, `screen_palette_bd_gate.sh` |
| Intra block copy | **Validated** | `screen_ibc_byte_gate.sh` 152/152 |
| Tiles, SB128, lossless | **Validated** | `tile_gate.sh`, `sb128_gate.sh`, `lossless_gate.sh` |
| Superres | **Partial** | 8-bit only — the u16 source downscale is unported; `superres_gate.sh` |
| Film grain | **Supported** | 8/10-bit 4:2:0 only (C's own limit) |
| All-intra animated AVIF | **Validated** | CI `animation` job with a PINNED decoder (libavif 1.3.0): 9 in-module tests plus `tests/animation_e2e.rs`, which re-parses the written file with an independent container parser and checks frame count, per-frame durations, the alpha-track decision and that frames differ |
| Monochrome / alpha | **Supported** | Rust extension beyond C's envelope |

### Inter / video — experimental, and gated behind `SvtParity`

General streaming video is **not** a shipping product path: the public API
refuses inter frames. What follows is the state of the machinery behind that
refusal, because it is most of the encoder.

| Feature | Status | Evidence / limit |
|---|---|---|
| Inter frame coding, low-delay P | **Partial** | `inter_byte_gate.sh` 108/108 byte-identical to C; the public API still refuses — the envelope is not the whole grid |
| Real-video inter (derf clips) | **Validated** | `real_video_inter_gate.sh` 24/24 against a pinned per-cell table |
| Decoder conformance of inter streams | **Validated** | `inter_decode_gate.sh`, and every inter gate below checks recon against dav1d |
| 10-bit inter video | **Validated** | `bd10_video_gate.sh` 24/24 encode + decode |
| **Global motion** | **Validated** | `global_motion_gate.sh` — recon byte-identical to dav1d on every frame; anti-vacuity: fails if no cell fits a non-identity model |
| **Warped motion** | **Validated** | `warped_motion_gate.sh` 8/8 — selects it where C does, recon matches dav1d. MDS1 MV refinement wired |
| **OBMC** | **Validated** | `obmc_gate.sh` 6/6 — selects it where C does (22 % of blocks at preset 0), recon matches dav1d. MD-stage MV refinement wired; the injection-time one (preset MR only) is not |
| Interpolation-filter search | **Validated** | `ifs_join_gate.sh` |
| Motion estimation / MVP | **Validated** | `inter_me_join_gate.sh`, `fctx_gate.sh` |
| Sub-8 inter chroma (`inter_chroma_4xn_pred`) | **Supported** | Ported 2026-09-10; covered by the inter recon gates, no isolating gate |
| Compound / bipred / inter-intra | **Not supported** | `allow_bipred` suppressed — `inter_pred_arm` has no two-reference path. Masked-compound and inter-intra DSP are ported and unwired |
| Hierarchical (random-access) GOP | **Not supported** | `generate_rps_info` translates 4 of C's 8 branches; `port_picstruct_ra` ported, not connected to the reference-buffer table |
| Temporal filtering | **Not supported** | `port_temporal_filtering.rs` ported (78 of 80 items), becomes live only with an RA GOP |
| Scene change / adaptive GOP | **Not supported** | `port_picstruct.rs`, ~85 of 119 items unwired |
| VBR / CBR rate control | **Not supported** | Refused at the API. The C ports all exist and are unwired (`port_rc_vbr_cbr*`, `port_rc_rtc_cbr`, `port_pass2_gop`); use CQP or CRF |
| `aq_mode != 0`, TPL r0 | **Not supported** | TPL is structurally off; `use_ref_frame_mvs` at `mfmv_level >= 2` refuses |
| QP 0 (coded-lossless) on inter | **Not supported** | Refused; still-image lossless IS supported |
| 10-bit OBMC | **Not supported** | `bd10_tree_supported` drops such a frame back to the 8-bit output rather than miscoding it |

### Outside the envelope by design

4:2:2 / 4:4:4 and 12-bit are rejected by C SVT v4.2.0 itself, so they are not
missing translations — they are alternate-backend work. Wider chroma, 12-bit
and fractional adaptive effort are all rejected at the API.

### How to read a "Validated" row

Every gate named above is run in CI (`.github/workflows/rust-gates.yml`) and
asserts one of two things: **byte-identity with the C reference**, or, where
the port deliberately diverges from C's search, **reconstruction identical to
an independent decoder** (dav1d/aomdec). The second is not weaker for the
tools it covers — a motion-mode blend depends on neighbour state that is never
re-transmitted, so only a decoder comparison can catch a wrong derivation.
Gates that could pass vacuously carry an explicit anti-vacuity check and fail
if they measured nothing.

## Use

The crates are not published to crates.io. Pin a reviewed Git revision:

```toml
[dependencies]
svtav1 = { package = "zenav1-svt", git = "https://github.com/imazen/zenav1-svt", rev = "0cbd1279f1e9e70ee62e051e56315e10f8b7f969" }
```

```rust
use svtav1::avif::AvifEncoder;
let pixels = vec![128u8; 16 * 16];
let encoded = AvifEncoder::new().with_quality(80.0).with_speed(6)
    .encode_y8(&pixels, 16, 16, 16).unwrap();
// encoded.data is an AV1 OBU sequence, not an AVIF container.
```

Use zenavif for RGB/RGBA conversion, AVIF still muxing and backend routing.
The optional `avif-container` feature provides the all-intra animation API.
Raw native-u16 inputs are available through `EncodePipeline`'s HBD methods;
do not infer a Gray16 zenavif entry point from that raw support.

## Develop

Read [the working guide](rust/docs/WORKING-ON-THIS.md) and [Rust rules](rust/CLAUDE.md).
From `rust/`, run `cargo nextest run --workspace --locked` under the shared
heavy-job wrapper. C-oracle tests need the `reference/svt-av1` submodule and
C build tools; product consumers do not. Mainline and HDR oracle builds are
owned by the dev-only `zenav1-svt-cref` build script. Fresh-machine timing and
portability acceptance remains tracked in issue #4.

[Package/source map](PORTING.md) · [full policy goal](ENCODER-POLICY-GOAL.md) ·
[documentation index and historical records](rust/docs/DOCUMENTATION-INDEX.md).

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
