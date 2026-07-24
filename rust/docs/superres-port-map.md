# Superres (super-resolution) — port map

**Status: CHUNKS A + B.1 + B.2 LANDED 2026-07-24** (5c69edcb2, f4a1b7516,
2f4d24cba) — the normative upscale, the source downscale, and the header
syntax are all ported and byte-exact vs real C. **B.3 (the encoder wiring)
is the remaining work.** Superres stays OFF by default, so everything landed
so far is additive and byte-neutral.

| piece | state | gate |
|---|---|---|
| normative upscale (`svtav1-dsp::superres`) | LANDED (chunk A) | `c_parity_superres` — 64/64 filter phases + 224 upscale cells == C |
| source downscale (`svtav1-dsp::resize`) | LANDED (chunk B.1) | `c_parity_resize` — 256 plane cells == C, all 5 filter banks, both down2 arms |
| `enable_superres` + `superres_params()` | LANDED (chunk B.2) | `superres_header` — SH and FH bytes == real C at denom 12/16 |
| denom selection / encode at coded_w / recon upscale / LR on upscaled | **OPEN (chunk B.3)** | byte-parity vs `--superres-mode 1 --superres-kf-denom D` |

**MEASURED (2026-07-24), do not re-derive:** for a STILL (KEY) frame the C knob
is `--superres-kf-denom`, NOT `--superres-denom`. With `--superres-mode 1
--superres-denom 12` C signals `enable_superres = 1` in the sequence header but
codes `use_superres = 0` on the key frame — the denom-12 and denom-16 streams
come out byte-identical. `--superres-kf-denom 12` is what actually scales.

---

**Original scoping (below) — Upscale DSP existed as a stub; encoder-side unported.**
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

- **Chunk A — DONE** (5c69edcb2). The 16-phase Q14 stub was replaced with a
  faithful `super_res.c` port: the 64-phase `RESIZE_FILTER_NORMATIVE` table,
  `upscale_convolve_step` / `upscale_convolve_x0` (the RS_SCALE_EXTRA_OFF +
  err/2 geometry the stub got wrong), `upscale_normative_row` with
  `upscale_normative_rect`'s border policy as a `TileColPad`, the whole-plane
  driver, and `scaled_size`.
- **Chunk B.1 — DONE** (f4a1b7516). `svtav1-dsp::resize`: the four band-limited
  filter banks (the 1000 bank IS the normative table, resize.h:75),
  `choose_interp_filter`, `interpolate_core`, `down2_symeven`/`down2_symodd` +
  `down2_steps`/`down2_length` (denominator 16 only), `resize_multistep`,
  `resize_plane_horizontal`. New cref shims — note `svt_av1_interpolate_core` /
  `svt_av1_down2_symeven` are RTCD pointers from **aom_dsp_rtcd.c**, not
  common_dsp_rtcd.c (initialising the wrong table leaves them NULL → segfault).
- **Chunk B.2 — DONE** (2f4d24cba). `SeqTools::enable_superres` +
  `ScSignal::superres` / `SuperresParams` write `superres_params()`; the
  `allow_intrabc` bit is now gated on the frame being unscaled. Pinned to real
  C bytes at denom 12 and 16.
- **Chunk B.3 — OPEN, the remaining wiring.** In order:
  1. `EncodePipeline::with_superres(denom)` (opt-in; default `None`), carrying
     `upscaled_w` (the SH/`max_frame_width_minus_1` value, already the TRUE
     width) and `coded_w = superres::scaled_size(upscaled_w, denom)`.
  2. Downscale the SOURCE with `resize::resize_plane_horizontal` (luma + both
     chroma planes at their own widths) and run the whole existing MD/recon
     pipeline at `coded_w` — the partition/SB/tile geometry all follow the
     smaller width, which is exactly what the arbitrary-dims work (#95) already
     supports.
  3. After CDEF and BEFORE loop restoration, upscale the recon to `upscaled_w`
     with `superres::upscale_normative_plane` (C `svt_av1_superres_upscale_frame`,
     cdef_process.c:152), then run the LR search/apply on the UPSCALED frame
     (LR unit geometry uses the upscaled width).
  4. Signal it: `SeqTools::enable_superres = true` and
     `ScSignal::superres = SuperresParams { enabled_in_seq: true, denom }`.
  5. Gate: `tools/superres_gate.sh` — port vs `SvtAv1EncApp --superres-mode 1
     --superres-kf-denom D` (D in 9..=16) × preset × qp, byte-identical OBUs,
     plus an aomdec/dav1d decode-conformance run (the upscale is normative, so
     a decode check is not optional).
- **Chunk C** — QTHRESH + AUTO denom selection (the RDO); byte-parity across modes.

## Invariants

`#![forbid(unsafe_code)]`. Superres stays **off** on the default path (`superres_mode = NONE`),
so the whole feature is additive and the existing byte-exact envelope is untouched — every gate
runs at denom 8 (no superres) exactly as today. A superres gate is a NEW opt-in config test.

## Decoder note

Superres also exists on the decode side (see the aom-rs KB-14 fix for a superres header-parse
class in the sibling libaom port) — not relevant to this SVT encoder port, but a reminder that
superres frame-size/header handling is subtle; get the `superres_params` header exact first.
