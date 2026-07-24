# Superres (super-resolution) — port map

**Status: INVESTIGATED, scoped. Upscale DSP exists; encoder-side unported.**
**Priority: LOW** — superres is **OFF by default** in C (`superres_mode = SUPERRES_NONE`,
`enc_settings.c:1095`); it is an opt-in (`--superres-mode`) rate-saving tool, not part of the
default still-image envelope. Sequenced after native-10-bit (`hbd-input-port-map.md`) + HDR.

## What superres does (C)

Encode the frame at a **reduced horizontal resolution** `coded_w = frame_w * 8 / denom`
(denom ∈ 9..16; `SCALE_NUMERATOR = 8`), then the decoder **normatively upscales** back to
`frame_w` with an 8-tap / 16-phase filter. Saves bits at low bitrate. Vertical is unchanged.
Modes (`EbSvtAv1Enc.h:108-121`): `NONE` (default), `FIXED` (one denom), `RANDOM`, `QTHRESH`
(q-based denom), `AUTO` (`AUTO_ALL`/`AUTO_DUAL`/`AUTO_SOLO` RDO over denoms).

## What the port HAS

- **Upscale DSP** — `svtav1-dsp/src/superres.rs`: `superres_upscale_row` (8-tap, 16 phases) +
  `superres_upscale`. **Needs C-parity validation** vs `av1_upscale_normative` (the DSP is
  present but not differentially gated against C).

## What is MISSING (encoder-side, all unported)

1. **Denom selection** — `superres_mode` handling: FIXED (fixed denom), QTHRESH (q-based,
   `pcs.c` superres_denom from qindex), AUTO (RDO over denoms). Start with **FIXED** (one
   denom, no RDO) as the smallest chunk.
2. **Source horizontal downscale** — the encoder downscales the SOURCE to `coded_w` before MD.
   (Downscale filter — note: distinct from the upscale filter.)
3. **Encode at coded_w** — the whole MD/recon pipeline runs at the reduced width; partition,
   SB grid, tile geometry all use `coded_w`. `pipeline.rs:1588` already computes
   `superres_upscaled_width` chroma dims — the geometry hooks are partially anticipated.
4. **Recon upscale** — `svt_av1_superres_upscale_frame` (`cdef_process.c:152`): after CDEF,
   before loop-restoration, the recon is upscaled to `frame_w` (LR then operates on the
   upscaled frame). Wire the existing `superres_upscale` DSP here.
5. **Loop-restoration on the upscaled frame** — LR unit geometry changes (upscaled width).
6. **Frame-header syntax** — `superres_params()`: the `use_superres` flag + `coded_denom`
   (`superres_denom - 9`, 3 bits) in the uncompressed header (`superres_denom != SCALE_NUMERATOR`).
7. **rc_aq `coded_to_superres_mi`** (`rc_aq.c:736`) — mi-coordinate mapping (inert until aq/deltaq
   fires; still-frame deltaq is inert per `benchmarks/crf_cqp_equivalence_2026-07-24.md`).

## Chunk plan

- **Chunk A** — validate the upscale DSP: differential gate `superres_upscale` vs C
  `av1_upscale_normative` (the exported kernel), bit-exact across denoms 9..16 × bit depths.
- **Chunk B** — FIXED-denom still encode: source downscale → encode at `coded_w` → recon
  upscale (DSP) → LR on upscaled → header `superres_params`. Byte-parity gate vs
  `SvtAv1EncApp --superres-mode 1 --superres-denom D`, one denom, one preset.
- **Chunk C** — QTHRESH + AUTO denom selection (the RDO); byte-parity across modes.

## Invariants

`#![forbid(unsafe_code)]`. Superres stays **off** on the default path (`superres_mode = NONE`),
so the whole feature is additive and the existing byte-exact envelope is untouched — every gate
runs at denom 8 (no superres) exactly as today. A superres gate is a NEW opt-in config test.

## Decoder note

Superres also exists on the decode side (see the aom-rs KB-14 fix for a superres header-parse
class in the sibling libaom port) — not relevant to this SVT encoder port, but a reminder that
superres frame-size/header handling is subtle; get the `superres_params` header exact first.
