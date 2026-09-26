# zenav1-svt

Pure-Rust port of SVT-AV1 v4.2.0 — `#![forbid(unsafe_code)]`, no C in the
product library path — with explicit C-reference selection and opt-in
HDR-fork/Zen extensions.

It encodes 8/10-bit 4:2:0 stills and animated AVIF, and **video in a measured
envelope: 8/10-bit 4:2:0, presets -1..13, flat low-delay-P**. Outside that
envelope an inter frame is refused rather than approximated. Monochrome and
alpha are Rust extensions beyond C's envelope.

**The support tables below are the answer to "does it do X?"** — every row
names the gate that backs it, and every gate runs in CI. Remaining work is
tracked in [issue 21](https://github.com/imazen/zenav1-svt/issues/21).

## Choosing the C target

The port targets two C encoders: pristine **SVT-AV1 v4.2.0** (the default)
and the **svt-av1-hdr "Ghost Robot"** fork. One call selects either, on
either API:

```rust
use svtav1::pipeline::{EncodePipeline, SvtReference};
let p = EncodePipeline::new(w, h, 8, rc, 0, 1).with_reference(SvtReference::GhostRobot);
// or: svtav1::avif::AvifEncoder::new().with_reference(SvtReference::GhostRobot)
```

`with_reference` also loads that target's fork defaults. The test tools
switch the same way with one variable, `SVT_ORACLE=mainline-4.2.0` or
`SVT_ORACLE=ghost-robot`, which drives both the port and the pinned C build
([rust/docs/ORACLES.md](rust/docs/ORACLES.md)). Byte parity status differs:
mainline stills are byte-identical (below), and mainline video is ratcheted
cell by cell against C (`tools/video_census_gate.sh`, 248 of 540 real-video
cells identical through 8 frames on 2026-09-26); Ghost Robot is in progress
(function-level suite 869/873, still grid 41/288, real video 0/36 — every
cell differs in the key frame; ORACLES.md has the current numbers).

## References, policy and coverage

- Constructors default to **Mainline420** (pristine v4.2.0) since 2026-09-25;
  the historical patched-C **Hybrid3115** is legacy and must be selected
  explicitly. The Rust extensions (monochrome, 4:4:4, alpha) encode under
  either; only `SvtParity` refuses them.
- `SvtReference::Mainline420` names pristine v4.2.0. `SvtParity(reference)`
  restricts the request to the named C envelope and rejects Zen enhancements.
  It does not certify every untested combination or erase known divergences.
- Checked native presets include **−1 through 13**. Effort currently resolves
  to native buckets; fractional adaptive search is not implemented.
- Grain modeling/denoising/tables/synthesis are wired in their documented
  envelopes. Two opt-in Zen enhancements remain: `AomScreenTools` (screen content:
  -23% ssim2 BD-rate at preset 8 and -29% at 10/12, 9/10 images better and none
  worse, 31-98% slower; byte-identical below preset 8 and under tune IQ;
  `benchmarks/aom_keep_or_drop_2026-09-25.meta`) and `DeepSearch`. Six others were removed on 2026-09-25 for showing no gain.
- 4:4:4 chroma ships on a measured decoder-verified envelope (8-bit key AND
  inter frames, SB64, no superres) as a Zen extension. Mainline C refuses
  it; Ghost Robot accepts it (High profile), and `SVT_CHROMA=444` now drives
  both encoders, so a byte oracle for it is in progress (plan 3.6). 4:2:2
  and 12-bit remain rejected, matching C.

Measured 2026-09-25 on `i265` at main `b4bc75ad`: the workspace nextest
suite passed **2771/2771, zero skips**; `identity_full_8bit.sh` **1100/1100**,
`bd10_photo_gate.sh` **191/191** and `bd10_nonflat_gate.sh` **309/309**
byte-identical to their C oracle. CI (`.github/workflows/rust-gates.yml`)
runs the same suites; read `gh run list` for its current state rather than
this paragraph. ARM: archmage/magetypes **0.9.29**;
[verification and pre-existing ARM Clippy/MSRV debt](rust/benchmarks/arm_pairwise_release_2026-09-08.md). The four formerly deferred **native10 parity cells closed
byte-identical on 2026-09-17**. See
[identity status](rust/docs/IDENTITY-STATUS.md).
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
| Superres | **Validated** | 8-bit: `superres_gate.sh`. 10-bit: `superres_bd10_gate.sh` — byte-identical to C AND `last_recon10_final` == `aomdec` at the upscaled size (u16 downscale + u16 normative upscale, both C-pinned). Stills/KEY frames only: mono, inter frames and bd10+film-grain-denoise refuse |
| Film grain | **Supported** | 8/10-bit 4:2:0 only (C's own limit) |
| Animated AVIF, inter-coded | **Validated** | `AnimationOptions::keyframes` defaults to one key frame every 120 pictures; only key frames are marked `stss`. MEASURED on eight 256x256 frames of `fourpeople` at quality 70: 58,823 B all-intra against 21,104 B as one closed GOP. Monochrome, lossless and 10-bit animations fall back to all-intra rather than failing |
| All-intra animated AVIF | **Validated** | CI `animation` job with a PINNED decoder (libavif 1.3.0): 9 in-module tests plus `tests/animation_e2e.rs`, which re-parses the written file with an independent container parser and checks frame count, per-frame durations, the alpha-track decision and that frames differ |
| Monochrome / alpha | **Supported** | Rust extension beyond C's envelope |
| Per-plane chroma delta-q override (`__expert`: `AvifEncoder::with_chroma_q_override`, `EncodePipeline::chroma_q_override`) | **Validated (decoder)** | Zen extension with no C byte oracle: fixed U/V qindex deltas replace the derived chroma delta-q, for decorrelated-plane research stimuli (not a quality knob). `tools/chroma_q_override_gate.sh` 73/73 (2026-09-24) — recon == `aomdec` == `dav1d` across mainline, tune IQ and the fork, including U != V (`separate_uv_delta_q = 1` on a mainline stream); anti-vacuity on per-plane chroma error; monochrome and a mid-sequence U/V-separation flip are refused. Gated on 8-bit 4:2:0 stills plus a 2-frame low-delay decode; 10-bit and 4:4:4 take the same qindex path without an isolating cell |

### Inter / video — validated in a measured envelope

**Inter frames ship, in a measured envelope: 4:2:0 at 8 and 10 bits, plus
monochrome.** 8-bit covers presets -1..13; 10-bit covers -1..13 at 256x256
and -1..5 at 128x128 (the measured grid — wider cells are untested, not
refused). `EncodePipeline`'s 4:2:0 entry points encode video for every
caller inside it. Monochrome inter is decoder-verified on its own gate
(`tools/mono_inter_gate.sh`) at 8-aligned AND unaligned sizes (the
unaligned legs cover the 2026-09-25 defect table: 65x64..100x96).
Monochrome qp0 inter also refuses — no inter WHT residual path exists
for it yet.

**The guarantee is different from the still one, and the difference is the
point.** Still images are byte-identical to C. Video is verified against a
DECODER — the encoder's own reconstruction must equal `aomdec`'s, frame for
frame — because that is the property a wrong stream actually violates, and
because the port's inter search does not track C's byte-for-byte on all
content. Both claims are measured below; neither is inferred from the other.

| Feature | Status | Evidence / limit |
|---|---|---|
| Inter frame coding, low-delay P, 8-bit, presets -1..13 | **Validated** | `video_selfcheck_gate.sh` — 270/270 cells (six derf clips x qp {20,40,55} x presets -1..13), all 8 frames of each byte-identical to `aomdec`'s reconstruction; presets -1..5 also clean at 128x128. The preset-6 floor came off on 2026-09-15 when the OBMC neighbour-prediction stale-cache fix swept the ladder — the drift the 2026-09-11 measurement recorded below preset 6 was that cache serving one frame's neighbour predictions to the next |
| 10-bit inter video | **Validated** | `bd10_video_selfcheck_gate.sh` — 396/396 cells (six derf clips x qp {20,40,55} x presets -1..13 at 256x256 + -1..5 at 128x128), all 8 frames byte-identical to `aomdec`'s reconstruction, 2026-09-18. Bitstream byte-identical to C on the 2-frame gate (`bd10_video_gate.sh` 24/24) via the `hbd_md = 2` MDS3 mirror. AVIF animation still codes 10-bit all-intra — a `animation_keyframes` policy, not a correctness fallback |
| Byte-identity to C on inter frames | **Partial** | `inter_byte_gate.sh` 108/108 on its curated grid. On the 96-cell frontier grid at frames=4 (MEASURED 2026-09-11): f0 95, f1 95, f2 60, f3 58 identical. The chain gap concentrates in 72x72 (a partial superblock — 17 of 24 differ at f2) and `gradient` content (19 of 24); `uniform` is 24/24 on every frame |
| Real-video inter (derf clips) | **Validated** | `real_video_inter_gate.sh` 24/24 against a pinned per-cell table |
| Decoder conformance of inter streams | **Validated** | `inter_decode_gate.sh`; the warped/global-motion/OBMC gates below each compare RECON against dav1d, which is stronger than parsing |
| **Global motion** | **Validated** | `global_motion_gate.sh` — recon byte-identical to dav1d on every frame; anti-vacuity: fails if no cell fits a non-identity model |
| **Warped motion** | **Validated** | `warped_motion_gate.sh` 8/8 — selects it where C does, recon matches dav1d. MDS1 MV refinement wired |
| **OBMC** | **Validated** | `obmc_gate.sh` 6/6 — selects it where C does (22 % of blocks at preset 0), recon matches dav1d. MD-stage MV refinement wired; the injection-time one (preset MR only) is not |
| Interpolation-filter search | **Validated** | `ifs_join_gate.sh` |
| Motion estimation / MVP | **Validated** | `inter_me_join_gate.sh`, `fctx_gate.sh` |
| Sub-8 inter chroma (`inter_chroma_4xn_pred`) | **Supported** | Ported 2026-09-10; covered by the inter recon gates, no isolating gate |
| Compound / bipred | **Validated** | two-reference prediction wired end to end (`predict_inter_yuv_compound`, `allow_bipred`, skip-mode signalling); compound blocks code where C enables them and the video gates' decoder comparison covers them |
| Masked compound (diffwtd/wedge) / inter-intra | **Validated** | Search + prediction + packing wired end to end (`inter_intra_search`, `calc_pred_masked_compound`, `search_compound_diff_wedge`, `predict_inter_yuv_compound_md` incl. per-ref warp + distwtd, `IiPreds` precompute, IFS/MDS3 rebuild). `tools/inter_intra_masked_census.sh` — 44 inter-intra and 138 compound coded blocks (incl. COMPOUND_WEDGE + COMPOUND_DIFFWTD) over 3 real-video cells at preset 0, every frame recon-identical to `aomdec`; the video selfcheck matrices cover them on every preset |
| Hierarchical (random-access) GOP | **Validated (decoder); byte-identical to C on unambiguous synthetic cells** | `ra_selfcheck_gate.sh` — 11/11 cells at 256x256 p6 q40 across hierarchical_levels 1..5, complete mini-GOPs plus trailing partial windows: every display frame's reconstruction byte-identical to `aomdec`, with a hidden-frame (`show_existing`) anti-vacuity pin. Byte-identity vs C (`aq_mode 0`): measured 2026-09-24 — `uniform`/`screen` streams byte-identical end-to-end at hier 1..5 incl. TU batching, `show_existing` OBUs and the reordered CDF chain; real video keeps the generic inter envelope (docs/IDENTITY-STATUS.md §2026-09-24) |
| Temporal filtering | **Validated (decoder)** | Live under random access: `derive_tf_params`/`tf_params_per_type`, TF-window assembly (`calc_ahd`, noise carry), the full subpel-ladder driver, and filtered-source substitution into the encode + decimated PA refs. Key frames take C's delayed-intra path — held until the next buffer is available, filtered with `tf_params_per_type[0]` against future POC-matched members, emitted ahead of that window. Covered by `ra_selfcheck_gate.sh` (TF is on at these presets) and the `tf_traced`/`c_parity_port_tf_pred` unit tests. BD-rate vs C's own RA+TF output on the 2026-09-24 sweep (pd-derf-720p 256x256, p6, qp {25,40,55}, hier 3, one complete GOP): johnny +0.37%, vidyo4 -0.03%, fourpeople +0.55% |
| Scene change / adaptive GOP | **Not supported** | `port_picstruct.rs`, ~85 of 119 items unwired |
| CBR rate control, low delay | **Validated (decoder); not byte-identical to C** | Accepted under low delay (C's own envelope; refused under random access), with `aq_mode` 2 as C requires. `tools/rc_tpl_gate.sh`: recon == `aomdec` == `dav1d` on every frame, and the target reaches the rate control; against mainline C every measured cell (0/34, 2026-09-26) differs from the KEY frame on, where C spends more bits |
| VBR rate control | **Not supported** | Refused at the API: the two-pass arm needs first-pass statistics, and firstpass.c is not ported; use CQP, CRF or low-delay CBR |
| TPL (random access, `aq_mode` 2) | **Validated (decoder); byte-identical to C on 5 of 8 measured cells** | C's default TPL-gated per-SB delta-q under random access. `tools/rc_tpl_gate.sh` pins each cell's C verdict and requires recon == `aomdec` == `dav1d` and a stream different from `aq_mode` 0 on both sides |
| `aq_mode != 0`, TPL r0 | **Not supported** | TPL is structurally off; `use_ref_frame_mvs` at `mfmv_level >= 2` refuses |
| QP 0 (coded-lossless) on inter | **Validated, 8-bit 4:2:0** | Real inter-coded blocks + WHT residuals; `tools/qp0_inter_gate.sh` 7/7 — every frame byte-identical to source in `aomdec`, presets {0,6,13}, sb64+sb128, with an inter-usage anti-vacuity leg. 10-bit and 4:4:4 qp0 inter still refuse |
| 10-bit OBMC | **Not supported** | `bd10_tree_supported` drops such a frame back to the 8-bit output rather than miscoding it |
| Monochrome inter | **Validated** | `tools/mono_inter_gate.sh` — encoder recon == `aomdec` == `dav1d`, all frames byte-identical across {diag,screen,gradient} content x {64x64..256x128} x presets {0,6,8,13} under a 13-px-per-frame shift, plus the unaligned defect table (65x64, 64x67, 65x67, 66x66, 70x64, 64x70, 96x100, 100x96, a 64x65 control and a 6-frame unaligned chain), sb64+sb128, bd10 legs and a real-clip leg (fourpeople), with anti-vacuity legs requiring real inter blocks and nonzero MVs. The earlier mono streams both decoders rejected were produced before the format-agnostic inter correctness landings (write-time `overlappable_neighbors`/`num_proj_ref`, recon-only eob preservation, OBMC on `SimpleTranslation`). Mono qp0 inter refuses (no inter WHT residual arm); a mono animation that hits that refusal is coded all-intra |
| **4:4:4 chroma, 8-bit key/inter** | **Validated** | Zen extension — C refuses non-4:2:0 (`enc_settings.c:470`), so there is no byte oracle. Envelope: 8-bit, `sb_size 64`, no superres; 10-bit/SB128/superres/IntraBC/film-grain refuse. Stills: byte-identical to `aomdec` AND `dav1d` on 10/10 cells (36..200 px, qp 0/20/30/35/45, incl. coded-lossless). Inter (non-funnel arm: luma-ME MV + motion-compensated chroma prediction, no `uv_mode`): byte-identical to `aomdec` on 45/45 matrix cells (dup/rand/shift × 64x64..256x128 × p{0,6,13}) + 4/4 moving-content + 6/6 inter-chain frames, all planes — `tools/chroma_444_inter_gate.sh` over `examples/probe_444*.rs`. Chroma loop filters signal off until ported. RD quality measured, not assumed: `tools/rd_ext_sweep.sh` (SSIMULACRA2 + per-plane PSNR vs `aomenc --i444 --profile=1`) — 396 cells, 0 failures, monotonic RD everywhere; chroma earns its bits (U/V PSNR +1.3..+15.3 dB vs own 4:2:0 at matched qp); frontier x1.74 vs libaom vs the 4:2:0 baseline's x1.46 |

### Outside the envelope by design

4:2:2 and 12-bit are rejected by C SVT v4.2.0 itself, so they are not missing
translations — they are alternate-backend work. 4:2:2, 12-bit and fractional
adaptive effort are all rejected at the API.

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
