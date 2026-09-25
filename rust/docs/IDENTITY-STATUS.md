# Identity status — what is byte-identical, what is decoder-verified

This is the live parity document: read it for what the port matches today and
what it does not. It grows by section, each dated and each naming its
measurement — the heading date below is the section's, not the file's, and a
section without a date is older than the ones that have one.

**The two guarantees are not interchangeable.** Still images are byte-identical
to the C encoder. Inter/video is verified against a DECODER instead — the
encoder's own reconstruction must equal `aomdec`'s, frame for frame — because
that is the property a wrong stream actually violates and because the port's
inter search does not track C's bytes on all content. Never infer one from the
other.

## Random-access stream bytes — 2026-09-24

The RA path's *stream machinery* is now byte-verified against C where the
decision surface is unambiguous, and decoder-verified everywhere per the
generic inter envelope. The measured parity cell is C with `aq_mode 0`:
C's default `aq_mode=2` enables TPL under RA (the `TPL is disabled for
aq_mode 0` warning is the tell), which the port deliberately refuses —
`aq_mode != 0` is a typed refusal, so `aq_mode 0` is the honest envelope.

Driven by `tools/identity_run` (`SVTAV1_FRAMES`, `SVT_PRED_STRUCT=2`,
`SVTAV1_HIER_LEVELS`, `SVTAV1_INTRA_PERIOD=64`, `SVT_ENABLE_TF`) vs
`tools/capture_c_trace/capture_c_trace` (`SVT_FRAMES`, `SVT_PRED_STRUCT`,
`SVT_HIER_LEVELS`, `SVT_INTRA_PERIOD`, `SVT_ENABLE_TF`) on the shared
`.yuv` the port writes, 256x256 p6 q40:

- `uniform` hier 3 9f: **byte-identical**, `SVT_ENABLE_TF` 0 AND 1.
- `screen` hier {1:4f, 2:6f, 3:9f, 4:9f, 5:9f}: **byte-identical** end to
  end — seq header, key tile, TU batching (TD + hidden + shown frames in
  one TU), `show_existing` OBUs, every inter tile's entropy payload, and
  the CDF continuation across reordered pictures (hidden pic8's stored
  context is what pic4's tile seeds from — byte-proven by pic4's tile
  matching, including its MFMV-derived MVP stacks).
- `gradient` / `johnny` real video: diverge inside inter tiles on
  *equivalent* decisions, not structure — e.g. C picks `NEWMV mv=-704`
  where the port picks `NEAREST mv=-192` on the period-64 gradient (both
  predictions SSE-identical), and C splits one SB the port keeps while the
  port splits a different one. Same documented inter-MD envelope as
  low-delay; headers, TU layout and the CDF chain still match.
- `diag` diverges inside the KEY tile (intra path, unrelated to RA).

Two RA-specific defects were fixed to get here: TU emission now batches
hidden+shown frames under one TD (`7600037f0`, matching C's
`count_frames_in_next_tu` semantics), and `frame_hier` for IDR keys now
takes the configured level per `pd_process.c:968` (`db0ce08f3`) — without
it, h5 coded the key at `base_q_idx 67` vs C's `70` (the `percents[0]`
row only reachable via `hierarchical_levels > 4`).

The claim is: RA streams are byte-identical to C on unambiguous
synthetic content; on real video the generic inter envelope applies
(decoder-verified via `ra_selfcheck_gate.sh`, 11/11).

## hierarchical_levels AUTO — 2026-09-25

`EncodePipeline::with_hierarchical_levels` now accepts
`HIERARCHICAL_LEVELS_AUTO` (`u8::MAX`, C's `~0` sentinel), resolved
inside the pipeline by `resolve_hierarchical_levels_auto` — a direct
port of `enc_handle.c:4556-4579` (RA/LD, rate-mode, enc_mode and
resolution arms). `identity_run`/`perf_encode` pass AUTO when
`SVTAV1_HIER_LEVELS` is unset, matching the C driver's API-default
behaviour; explicit 0..=5 is unchanged. Focused tests:
`tests/hier_auto.rs` (7/7).

Two defects fixed for this:

- `frame_hier` read the configured level for `is_key`, skipping the
  delayed-intra re-stamp (`pd_process.c:3936-3943`, "Update the key
  frame pred structure") that `filter_delayed_intra` already mirrors.
  The key frame now reads the decided picture's field like C — measured
  fix: cut-short window key went 70 → 67 = C (the `percents[1][0]` arm
  `cqp_qindex_calc` takes when the following mini-GOP's level ≤ 4).
- `mds0` level 1 (`pruning_method_th = 100`, `M3..=M5` non-base) was a
  panic — unreachable only when hier > 0 never ran. The per-class arm
  is now ported: `mds0_best_cost_per_class` tracker,
  `MIN(md_me_dist, md_pme_dist) / (bw*bh)` gate, per-class thresholds
  {50,10,10,50,0} and the armed-0 global fallback on a gate miss —
  stamped across all four candidate lanes (`inject.rs`). Byte-verified
  vs C on the exact cells that used to panic.

Measured vs C, unset `SVTAV1_HIER_LEVELS` on both sides, 256² qp40:

- **RA (`SVT_PRED_STRUCT=2` → hl 5)**: byte-identical streams
  (seq headers, all coded frames in decode order, `show_existing`
  units, TU batching, entropy payloads) at p4, p5, p6, p8, p10, p13 —
  including n∈{5,9,17} and qp∈{20,55} at p6. The 5f stream is 7 TUs:
  key, two hidden coded pictures, shown, `show_existing`, shown,
  `show_existing`. p0/p2/p3: 7/7 headers identical, 1–2-byte payload
  diffs — the non-base inter-MD envelope below, not emission.
- **Low delay (unset → hl 3, CBR-side hl 2)**: byte-identical at p0,
  p3, p4, p5, p6 (qp 20/40/55) and p13; divergent payloads at
  p2/p8/p10 — non-base inter-MD arms that hier > 0 now exercises by
  default; same envelope family as the carried "inter-MD ladder
  diverges at p0" item. The p0/p3/p4 fixes came from resolving GM's
  `(list, ref)` reference planes through `ref_dpb_index` rather than
  the nearest `pa_ref` — C's `ref_pa_pic_ptr_array` names an older
  picture under hierarchical LD. Explicit `SVTAV1_HIER_LEVELS=0` still
  yields the old flat stream (verified identical to C flat at p10).

Note the asymmetry the AUTO change intentionally removes: an
unset-vs-unset cell comparison now means the same configuration on
both sides; pinning `SVTAV1_HIER_LEVELS=0`/`SVT_HIER_LEVELS=0` still
compares flat-vs-flat as before (the pinned gates are unaffected).

## Tune surface (`--tune`) — 2026-09-19

`SvtTune` (`zenav1-svt::SvtTune`, builder `with_tune`) exposes C's
`static_config.tune` values 0–4 as a typed enum: `Vq`, `Psnr` (default),
`Ssim`, `Iq`, `MsSsim`. Slot 5 is not exposed (mainline VMAF unmodeled;
the fork's FILM_GRAIN=6 is a different thing). The default is byte-neutral:
`Psnr` writes `tune = 1`, the value the pipeline already defaulted to.

Byte-parity vs C is per-variant, measured through `tools/issue9_knobs_gate.sh`
(not in CI; historically red — see below):

- `Vq` / `Psnr`: byte-identical on every probed cell (128²–512², p6/p10,
  qp 20/32/40/55, gradient + photo).
- `Ssim` / `Iq` / `MsSsim`: engage the per-16x16 SSIM-rdmult scaling
  (`pow`/`log`/`exp`, `port_md_lambda.rs` cross-ISA caveat) and diverge
  DECISIONALLY on part of the grid — frame headers match field-for-field
  and the variance-boost plan is C-exact, but near-tie decisions flip
  (first observed: a wiener `lr-taps` symbol; tuneiq-gradient-128-p6 q20:
  3199 B port vs 3205 B C). The gate reports 33/36 today; the three IQ
  cells have differed since the gate landed (`b80c2aa35`) and predate the
  AVX-512 campaign — documented, not silently green. Every variant's output
  is decoder-valid (aomdec-verified in the tune sweep).

`ZenEnhancement::StillImageTune` ("still-image-tune-v1") is the opt-in Zen
bundle: tune IQ's overrides verbatim, with caller-set values on
bundle-covered knobs surviving (its only delta vs `with_tune(Iq)` — with
no caller extras the stream is byte-identical to tune IQ). Requires
`EncodingPolicy::Zen` (SvtParity refuses all enhancements); validated for
all-intra 4:2:0, NOT byte-pinned to C — verified by aomdec decode and the
RD record in `benchmarks/still_image_tune_v1_2026-09-19.meta`. The
extras candidates measured on the subset (variance_octile 6–8, vb
strength/curve, ac_bias, min_qm, sharpness 3–6) were all neutral-or-worse
than plain IQ on ssim2-BD-rate — the best, sharpness 5, buys −0.34%
pooled median at p75 +0.55 (inside per-image noise) — so v1 pins nothing
beyond IQ's own bundle.

`ZenEnhancement::AomAdaptiveCdef` ("aom-adaptive-cdef-v1") and
`ZenEnhancement::AomAdaptiveSharpness` ("aom-adaptive-sharpness-v1") are
libaom-semantics experiments SVT C does not carry (`--enable-cdef=3` /
`--enable-adaptive-sharpness`): adaptive CDEF turns the pick off at
`base_qindex <= 32`, halves every picked strength at `<= 220` and zeroes
the halved-low ones at `<= 140`, and zeroes the qp-path chroma strength;
adaptive sharpness applies the `<=112 -> 7 / <=160 -> 1 / else 0` LF
cap at any tune (identical arithmetic to C's IQ/MS_SSIM ladder — a no-op
under those tunes, by construction). Both are applied post-pick so
signal and application stay in agreement, are validated all-intra 4:2:0,
and are decoder-verified, never byte-claimed against C. Measured on the
42-image subset (`benchmarks/aom_features_2026-09-20.meta`, 1260 port
cells, zero decode errors): adaptive sharpness under IQ is byte-identical
in 252/252 cells (the no-op is proven, not just derived); adaptive CDEF
fires in 53/252 IQ cells and is a small consistent ssim2-BD *regression*
vs SVT's own pick (+0.4% @p6, +0.9% @p10) — opt-in experiment, not
recommended on ssim2 grounds. Against `zenav1-aom`, port tune IQ lands
within +1.5–2.2% BD at matched zones and beats aom-iq at p8-vs-cpu8
(−1.45%); the 30 aom-side DEC-ERR cells (screen/grayscale documents)
count against the reference, not the port.

`ZenEnhancement::AomDeltaQLf` ("aom-delta-q-lf-v1") is libaom's
`delta_q_lf` semantics SVT C ships dead (`delta_lf_present` hardwired 0
in C): when a per-SB delta-q plan exists it signals
`delta_lf_present`/`delta_lf_res=2`/`delta_lf_multi=0`, codes one
delta-lf symbol `((sb_q − base_q)/4 + 1) & !1` immediately after each
per-SB delta-q symbol on `delta_lf_cdf` (new `FrameContext` field,
save/restored through `FrameCdfs`), resets prev per tile, and applies
the same map in encoder deblock with libaom's two-sided edge rule
(filter if either side's level is nonzero; use the current side unless
it is 0). Byte-inert when no delta-q plan exists or the enhancement is
off; decoder-verified — encoder recon == `aomdec` on 8-bit SB64/SB128/
multi-tile and 10-bit cells — never byte-claimed against C. Measured
RD-neutral on the subset (−0.01% @p6, +0.24% @p10 vs tune IQ,
`benchmarks/aom_features_2026-09-20.meta`): a consistency tool, not an
RD lever.

### Tunes on video paths — 2026-09-25

Two harness facts to know before reading ANY multi-frame tune cell:

- `identity_run`'s multi-frame block (`SVTAV1_FRAMES>1`) returned before
  the `SVTAV1_TUNE` override ran, so every multi-frame "tune" encode
  silently produced PSNR — including both earlier "tune-VQ" verdicts.
  Fixed 2026-09-25; `SVTAV1_TUNE` now applies inside that block. The
  env split to remember: the PORT reads `SVTAV1_TUNE`, the C driver
  reads `SVT_TUNE`.
- C v4.2.0 itself forces `--tune 0` → PSNR under low delay
  (`Tune 0 is not applicable for low-delay, tune will be forced to 1`),
  so an LD tune-VQ cell is vacuous on both sides — only RA exercises
  tune-VQ's real arms.

With tune actually armed, the old unconditional subjective-tune refusal
was narrowed: it now fires only when C's arms are provably LIVE —
`is_noise_level == 1` (real under RA+TF, always 0 on LD because C's
`last_i` carry only updates inside `derive_tf_window_params`) or a live
`delta_q_plan` feeding the non-noise-gated `use_sharpness` RDOQ term
(full_loop.c) the port does not model. LD inter + tune-VQ is admitted
and inert-identical.

RA tune-VQ (gradient 256² qp40 p6 hier-auto, 5 frames): the key frame
now codes C's VQ +2 `sharpness_level`/`filter_level=[12,12]` correctly.
Remaining measured gaps vs C:

- `cdef_strengths[0]` on the key frame: C codes 28, the port 0 — a
  CDEF derivation difference under VQ, not yet root-caused.
- ~~Emission structure under hier ≥ 2~~ — RESOLVED 2026-09-25, and it
  was never a port defect: the "missing" `show_existing_frame` units
  and decode-order permutation already worked (`encode_ra_window` +
  `has_show_existing`). What the repro actually exposed was a harness
  env gap — `identity_run` defaulted unset `SVTAV1_HIER_LEVELS` to 0
  (flat) while the C driver leaves `hierarchical_levels` at its AUTO
  sentinel (`~0`), which `enc_handle.c:4556-4579` resolves to 5 under
  RA. With AUTO resolved inside the pipeline the flat-vs-pyramid
  comparison disappeared. See "hierarchical_levels AUTO — 2026-09-25"
  below for the measured envelope.

`tools/decode_diff hdr_diff` gained `HDR_FULL=1` full-field header dump
for this class of divergence; `HDR_DUMP=1` prints diff windows.

`ChromaFormat` (types crate, C `EbColorFormat` numbering) now carries
`Yuv400/420/422/444` on `EncodePipeline` plus `subsampling_x/y`,
`required_profile(bit_depth)` and chroma-dim derivation; `SeqTools` and
`FrameDims` take a format so the sequence header writes the profile-1/2
`color_config` branches per C `write_color_config` and the geometry seam
is format-derived. **Only `Some(Yuv420)` encodes** — the C-parity
surface, byte-identical as ever (spotcheck 145/145). 4:2:2 and 4:4:4
have stable `try_encode_frame_{422,444}` signatures that REFUSE: the
chroma geometry below `encode_frame_impl` is still 4:2:0-derived (a
2026-09-19 bring-up probe produced a stream aomdec reports "Corrupt
frame / Failed to decode tile data"), and C itself refuses both formats
at `verify_settings` (enc_settings.c:470) — extension work with no byte
oracle, decoder-correctness gated when it lands.

**Defect found and fixed in this work:** C's sb-size rule forces 64 when
`enable_variance_boost` is on (enc_handle.c:4077), applied AFTER the tune
overrides set it. The port derived `sb_size` in `new()` — before `hdr`
mutations were visible — so tune IQ / `with_variance_boost` / the still
recipe at sb128-deriving presets (-1..1 on large enough frames) emitted
an sb128 stream with per-64 delta-q symbols: "Failed to decode tile
data" under aomdec while C produced a valid sb64 stream. The derivation
now re-runs inside `encode_frame_impl` after the tune overrides
(matching C's `copy_api_from_app` → `set_param_based_on_input` order),
and the delta-q emission gate uses the real `sb_size` — the forced
`SVTAV1_SB=128`+VB combination (a port extension C cannot express) now
also produces a decodable stream. Verified: t3/t4/still at preset -1 all
decode; auto-derived ≡ explicit `SVTAV1_SB=64` byte-identical; VB-off
paths (t1/t2 at preset -1) byte-identical to pre-fix.

## 4:4:4 chroma — 2026-09-21

`ChromaFormat::Yuv444` ships as a Zen extension on a measured envelope:
**8-bit, key AND inter frames, `sb_size 64`, no superres**. Everything
outside refuses at `encode_frame_impl`'s envelope gate — 10-bit, SB128,
superres, IntraBC (sequence flag forced off), film grain (the 4:2:0-shaped
`validate_film_grain` refusal covers it). C v4.2.0 refuses non-4:2:0 at
`verify_settings` (`enc_settings.c:470`), so **there is no byte oracle and
none is claimed** — the property asserted is decoder reconstruction
equality plus measured RD sanity.

**Inter frames (added 2026-09-21, second commit):** the funnel stays
4:2:0-only, so 4:4:4 inter decisions come from the non-funnel leaf arm —
`RefFrameCtx` carries the reference's padded chroma planes and the frame's
subsampling, `hierarchical_me_centered` finds a luma MV per leaf, and the
walk predicts inter chroma by motion compensation (`predict_inter_chroma_*`
parameterized by `(ss_x, ss_y)`) and residual-codes against it; a genuine
inter block codes no `uv_mode`. Two write-time defects were found by
decoding, not by reading code:

- `overlappable_neighbors`/`num_proj_ref` were hardcoded 0 on this path.
  The decoder recomputes both per block (decodemv.c `av1_findSamples` +
  `av1_count_overlappable_neighbors`), so a `WARPED_CAUSAL`-allowed
  three-symbol `motion_mode` went uncoded on every neighboured block and
  aomdec rejected frame-1 tiles. Both are now derived from the committed
  mode-info map inside `inter_mvp_fields`, like `pred_mv`/`drl_ctx`.
- The recon-only entropy walk dropped chroma TXB eobs entirely
  (`if !recon_only` skipped the `(q, eob)` push), so the recon-mode `skip`
  derivation saw "all chroma eob == 0" and recorded `skip = 1` into the
  deblock geometry while the bit-producing walk wrote `skip = 0` — the
  decoder's CDEF dlist included blocks ours excluded, producing small
  localized luma differences with U/V exact. The recon walk now keeps the
  real eobs and empties only the coefficient vectors.

Decoder verification (aomdec, `tools/chroma_444_inter_gate.sh` driving
`examples/probe_444_{dup,video}.rs` — duplicate, random, integer-shift and
moving-content streams at 64x64..256x128, presets {0,6,13}, qp 30,
2..6 frame chains): **45/45 matrix cells + 4/4 moving-content frames +
6/6 chain frames byte-identical on all three planes** (47/47 gate cells).

Decoder verification (aomdec AND dav1d, `examples/probe_444*.rs`):
10/10 cells — 36/44/60/64/100/120/128/200 px square, qp {0,20,30,35,45} —
encoder reconstruction byte-identical to decoded output on all planes,
including coded-lossless (qindex 0 goes through the FWHT/transpose/
`quantize_b`/IWHT path mirroring `lossless_mono.rs`, because AV1's
lossless transform is Walsh-Hadamard, not DCT). Sequence header signals
profile 1 via `chroma_format.required_profile`. Chroma loop filters are
signalled off (lf 0 / CDEF uv 0 / LR NONE) until their kernels are ported
— decoder-consistent by construction.

Quality (the surface byte-parity cannot see), `tools/rd_ext_sweep.sh` —
396 cells over 11 codec-corpus images x qp {10..60} x {ours, aomenc
`--i444 --profile=1`, aomenc `--monochrome`, ours-4:2:0, aomenc-4:2:0}:
0 failures, monotonic rate/SSIMULACRA2 on every curve. Chroma earns its
bits: U/V PSNR +1.3..+15.3 dB vs the port's own 4:2:0 at matched qp.
Frontier honesty: ours-444 costs x1.74 libaom bytes at matched SSIM2 vs
x1.46 for ours-420 (the byte-exact SVT baseline) — the residual is named:
DC-only chroma prediction and refused screen-content tools (palette /
IntraBC), concentrated in screen content (x1.93..x3.13) while photos sit
at x1.07..x1.40. Mono carries the same shape (x1.59) where chroma does
not exist at all, so the gap is generic intra-search, not the format.

## QP 0 coded-lossless inter — 2026-09-21

8-bit 4:2:0 inter frames at `base_q_idx` 0 are supported: the funnel runs
its normal inter candidates (NearestMv/NewMv/skip-mode incl. WarpedCausal)
with the coded-lossless structure already used on stills — forced 8x8
leaves, TX_4X4 WHT residuals, no tx symbols, filters off.
`tools/qp0_inter_gate.sh` is **7/7**: fourpeople 128x128 presets {0,6,13},
128x128 at sb128, and 256x256 — every frame of each 4-frame encode
byte-identical to the SOURCE in `aomdec` (the lossless oracle, stronger
than recon==dec), with a `SVTAV1_INTERDBG` anti-vacuity leg requiring real
inter block decisions in every run.

Two defects found enabling it, both fixed in the landing: the skip-MODE
arbitration (`svt_aom_full_cost`) expects prediction dists that
coded_lossless had suppressed — a qp0 encode panicked — so the pred-dists
predicate no longer excludes lossless frames; and the pre-existing
`!is_key || intra_period > 1` refusal was the only other gate.

Outside that envelope the refusal is narrowed, not removed: **10-bit**
qp0 inter mis-predicts intra per-BLOCK where the decoder predicts per-TXB
(measured: encoder recon == source but `aomdec` disagrees), and **4:4:4**
qp0 inter has no inter WHT arm — its stream is legal but silently
all-intra. Both refuse with `[C: accepts]` capability text; qp0 stills
remain supported at 8 and 10 bits.

## Superres — 8-bit AND 10-bit stills — 2026-09-21

10-bit superres ships on the still surface. C's order is load-bearing:
the native u16 source is downscaled FIRST by
`svt_av1_highbd_resize_plane_horizontal` (the `port_resize_hbd` ladder,
already C-pinned) and only then unpacked to the u8 canvas
(`filtered_u16 >> 2`); truncating before the resize loses the low bits
the filters accumulate and is NOT byte-parity. The port stages the
full-width u16 planes (`hbd_superres_src`) and produces both canvases in
that order in `superres_downscale_420_hbd`. The final 10-bit
reconstruction is upscaled back to the output width by the u16 normative
kernel (`highbd_upscale_normative_row`, byte-identical to C across
bd {10,12} x 7 widths x denoms 9..16 x 4 contents in
`c_parity_superres`).

`tools/superres_bd10_gate.sh` is **512/512** and
asserts all four properties per cell: OBU byte-identical to C
(`SVT_SUPERRES_KF_DENOM`, bd 10, same u16 .yuv), `aomdec` decodes, the
decoded frame is u16 I420 at the FULL upscaled size, and the port's
`last_recon10_final` equals the decoder's output byte-for-byte — the
strongest leg, since it pins the u16 normative upscale and the whole
10-bit filter chain at output geometry. Anti-vacuity requires the
superres stream to differ from the non-superres one.

Three honest refusals came with it: **inter frames under superres**
(`[C: accepts]` capability — the per-reference geometry under a changing
coded width is decoder-ungated; the still arm is the measured surface),
**mono + superres** (the mono entry has no downscale arm — measured: it
encoded a left-cropped plane under an upscale header), and **bd10 +
film-grain denoise** (the denoiser reads the u8 canvas before the
downscale that produces it under the native-input order).

One latent 8-bit defect fixed in the same change: the post-filter stage
used to replace `recon` with the UPSCALED plane and then build the DPB
reference from it at the coded stride — a diagonal smear any
inter+superres prediction would have scored. `recon`/`recon10` now stay
at coded geometry for the reference and only the published output planes
(`last_recon`/`last_recon10_final`) carry the upscale.

## Still identity — 2026-09-08

Implementation snapshot: main `0cbd1279`. Historical campaigns, pins and old
first-difference investigations are [preserved verbatim](history/2026-09-08/rust/docs/IDENTITY-STATUS.md).
Use them as source-matched evidence, not as current open-bug lists.

Real-image byte identity, measured 2026-09-09 on current source: **180/180**
over 20 CID22-512 photos x presets {2,6,10} x cli_qp {20,40,55}
([record](../benchmarks/real_image_identity_2026-09-09.meta)). This is the
first such number since the "53 real-image parity cases fixed" claim, because
`tools/real_image_matrix.sh` could not build its oracle on any host until
`a046e68b`. Scope: 8-bit 4:2:0 512x512 stills on one x86-64 host.

The latest preceding eight-bit landing matrix was 1100/1100; the partial-chroma
and SB128 fixes also resolved all 53 historical witnesses in 168/168 replay
pairs. Neither result closes native10 or every optional/inter/HDR combination.
Pristine Mainline420 and Hybrid3115 are distinct C targets; source, HDR mode,
compiler and ISA belong in each parity record.

## Native10 cells — closed 2026-09-17

376×512 photo fixture, `SVTAV1_BD=10 SVTAV1_HBD_SRC=1`:

| Native preset | QP | C bytes | Rust bytes | Status |
|---|---|---|---|---|
| 1 | 10 | 11465 | 11465 | byte-identical |
| 4 | 10 | 11752 | 11752 | byte-identical |
| 4 | 30 | 2827 | 2827 | byte-identical |
| 5 | 10 | 11913 | 11913 | byte-identical |

All four formerly deferred cells now match C byte-for-byte and are permanent
`regression_spotcheck.sh` witnesses (`partial-chroma-native10-*`). Two root
causes, both u8 proxies where C reads real u16 data at `hbd_md`: the CfL
complexity detector used `(src8<<2)` vs u16 preds and raw u16 variance — C's
`vf_hbd_10` kernel pre-scales sum/sse to the 8-bit domain (~16× over-fire);
and the NSQ `skip_sub` quadrant gate scored u8 recon distances while C's
`calc_scr_to_recon_dist_per_quadrant` runs `svt_full_distortion_kernel16_bits`
on the u16 source vs u16 `cand_bf->recon`. Hashes, exact fixture and retained
unproven experiment: [deferred manifest](deferred-native10-parity.json).

Historical investigation trail (the divergences, now closed): p1q10 and p4q10
first diverged at arithmetic-coder op 0 in the `lr-taps` class, p4q30 at op
11758 and p5q10 at op 67328; for p1q10 the first real divergence was block
**mi(32,36)** (pixel 144,128), a partition-size flip. Established by capturing
recon planes and the decision tree from one run. Archive retrieval:
[native10 receipt](native10-handoff-receipt.json).

## Inter identity on REAL video (new surface, 2026-09-10)

Until now every inter cell in this repo encoded synthetic content, and the
multi-frame path built later frames by translating frame 0 by a global integer
offset — which open-loop ME finds exactly, so the residual SAD floors to zero
(`avg_me_sad=0`, `is_gm_on=0` across {gradient,diag,screen} × {64,128,256,512}).
The inter surface was being asserted against a motion field C's search never has
to work for.

`tools/real_video_inter_gate.sh` runs the two-frame differential on twelve I420
sequences cut from the six Xiph derf clips whose index entry reads *public
domain*, published at the R2 prefix `video/pd-derf-720p/` and fetched
anonymously. MEASURED 2026-09-11, 6 clips × {128×128, 256×256} × presets {6,8}
× cli_qp 40, 24 cells:

| frame | byte-identical |
|---|---|
| 0 (key) | **22 / 24** |
| 1 (inter) | **10 / 24** |

Two findings moved since the 2026-09-10 measurement
([record](../benchmarks/real_video_inter_2026-09-10.meta), 18/24 and 7/24):

- **All six preset-6 frame-0 failures closed.** The "key frame diverges when
  a second frame follows it" bug
  ([record](../benchmarks/multiframe_keyframe_p2p7_2026-09-10.meta)) was the
  video arm's `skip_sub_depth_lvl` ladder: C's
  `svt_aom_sig_deriv_enc_dec_default` derives level 2 (`coeff_perc` 25) for
  enc_mode > M1 where the allintra ladder stays at level 1 (`coeff_perc` 15)
  through M7. The port had baked level 1 for every picture, so
  `eval_sub_depth_skip_cond1` never fired and flat ≤16×16 blocks were split
  that C keeps — measured at `johnny_256x256_8f` q40 p6, the 16×16 node at
  (16,32) has 39/256 = 15 % nonzero coefficients. `encdec_arm::apply` now
  stamps the per-arm level into `FunnelCfg::skip_sub_depth`, and the allintra
  bake is `<= M7 -> 1 else 2` rather than always 1.
- **Loop restoration is no longer gated on `is_key`.** C's
  `ppcs->enable_restoration` is picture-level (`wn > 0 || sg > 0`), and a
  flat GOP makes every frame `is_not_last_layer`, so C runs luma-only Wiener
  (`lr_type[0]=2`) on frame 1 where the port wrote `lr_type[0]=0`. The
  remaining frame-0 zeros are `vidyo3`/`vidyo4` 256×256 at **preset 8** — a
  different mechanism than the closed preset-6 one.
- **The control makes the gap explicit.** At the identical cell shape
  (128×128 q40 p6 frames=2) synthetic `gradient` is byte-identical on *both*
  frames, with frame 1 coding to 24 bytes — a skip. Real video at that shape
  closed for `johnny` after the 2026-09-13 inter-MD fix set (below);
  `vidyo3` still diverges (C 113 B against the port's 108).

The pinned table in the gate is a measurement: a cell that regresses fails, and
a cell that improves fails too, as `PROMOTED`, so the table cannot silently go
stale.

**Vacuity note worth keeping.** All six clips are 30 fps material published at
60, so every frame is doubled. The first extraction scored `vidyo3` at mean
motion 20.4 while `|f1-f0|` was exactly 0.00, and both encoders coded that frame
to an identical 24 bytes — a cell that would have read as inter parity on real
video while asserting only that two encoders agree a repeated frame is a skip.
`tools/mk_video_assets.py` now decimates duplicates and scores motion as the
**minimum** over consecutive pairs, never the mean.

## Multi-frame video: SHIPPED in a measured envelope, gated against a decoder (re-measured 2026-09-15)

**The envelope is 8-bit 4:2:0, presets -1..13.** Inside it,
`tools/video_selfcheck_gate.sh` is 270 of 270 cells (six public-domain derf
clips x qp {20,40,55} x presets -1..13) whose every frame of an 8-frame encode
reconstructs byte-identically to `aomdec`; presets -1..5 are also clean at
128x128 (126/126). The 2026-09-11 preset floor came off when the residual
low-preset drift was traced to the OBMC neighbour-prediction cache serving
one frame's predictions to the next (`NeighbourKey` carried no frame
identity) — `obmc_pred_arm::begin_leaf` now resets it per `evaluate_leaf`,
matching C's per-`md_encode_block` flag reset. The "spatial-only stack
defect" the 2026-09-11 measurement attributed to `SVTAV1_MFMV_OFF` was that
same cache; a 2026-09-15 MFMV_OFF re-sweep of presets -1..5 is 126/126 clean.

Outside the envelope an inter frame is refused, not approximated, and each
refusal carries its measurement:

| Refused | What was measured |
|---|---|
| bit depth > 8 | SUPERSEDED — 10-bit inter ships since 2026-09-18 (`bd10_video_selfcheck_gate.sh` 396/396, recon == `aomdec`). The 2026-09-11 refusal row measured 8/18 cells clean before the `hbd_md = 2` MDS3 mirror landed |
| monochrome | SUPERSEDED 2026-09-21 — `tools/mono_inter_gate.sh` 12/12: encoder recon == `aomdec` == `dav1d` on every frame across synthetic 13-px-shift legs x presets {0,6,8,13} x 64x64..256x128, sb64+sb128, a 6-frame inter chain, a bd10 leg and the fourpeople clip, with anti-vacuity legs requiring real inter blocks + nonzero MVs. The corrupt stream the refusal cited predated the format-agnostic inter landings. Mono qp0 inter still refuses — the mono lossless arm has no inter WHT residual path, so a qp0 "inter" stream is legal but silently all-intra (the gate pins the refusal) |
| qp0 (coded-lossless) inter outside 8-bit 4:2:0 | measured 2026-09-21: 10-bit inter lossless predicts intra per-BLOCK while the decoder predicts per-TXB — encoder recon == source yet `aomdec` diverges; 4:4:4/mono qp0 inter streams are legal but silently all-intra (no inter WHT residual arm). 8-bit 4:2:0 qp0 inter SHIPS — `tools/qp0_inter_gate.sh` 7/7 |

`SVTAV1_INTER_EXPERIMENTAL` is RETIRED — the bit-depth refusal it lifted
was removed when 10-bit inter shipped (2026-09-18).

## How the envelope came to be (2026-09-11)

The heading below is the 2026-09-10 state and is kept because the trail is
useful; read this paragraph first. Both inter refusals are GONE, and so are the
`SVTAV1_INTER_EXPERIMENTAL` / `SVTAV1_INTER_CHAIN_EXPERIMENTAL` variables that
lifted them — `EncodePipeline`'s 4:2:0 entry points encode inter frames for
every caller. The standing guard is `tools/video_selfcheck_gate.sh`: the port's
own final reconstruction is byte-identical to `aomdec`'s on EVERY frame of an
8-frame encode, for all six public-domain derf clips at qp {20,40,55} — 18 of
18 cells. The monochrome arm shipped inter on 2026-09-21 — the gate is
`tools/mono_inter_gate.sh` (see the superseded refusal row above).

Byte-identity to C on the inter path is a separate, narrower claim and it is
NOT universal. MEASURED 2026-09-13 on the 96-cell frontier grid at frames=4
(2026-09-11 was 95/95/60/58):
95 cells identical on frame 0, 95 on frame 1, 61 on frame 2, 59 on frame 3.
The chain frames' gap concentrates in 72x72 (a PARTIAL superblock: 17 of its
24 cells differ at frame 2) and `gradient` content (19 of 24); `uniform` is
24 of 24 identical on every frame.

The 2026-09-13 frame-1 gains came from four pieces of C behaviour the port was
missing, all in `generate_md_stage_0_cand_light_pd1` / `fast_loop_core`:

1. `merge_inter_cands` (mode_decision.c:3638-3643): when
   `min(md_me_dist, md_pme_dist) / (bw*bh) < (4*(63-qp))>>1`, EVERY inter
   candidate is CAND_CLASS_2 — one merged MDS0 pool, not per-mode lanes. The
   port now computes `md_me_dist`/`md_pme_dist` and stamps `cand_class`.
2. `ctx->global_mv_injection = ppcs->gm_ctrls.enabled` (enc_mode_config.c
   :7847/:7964): GLOBALMV injection is off wherever `gm_level` is 0 (p6+ on
   video); the port had hardcoded it on.
3. `mds0_use_hadamard_sb = false` on the video arm (:7916/:8032): MDS0's
   inter distortion is the VARIANCE arm (`fn_ptr->vf`), not hadamard SATD.
4. `dist_to_cost_th = 0` at mds0 level 2 (product_coding_loop.c:1309-1334):
   a candidate whose rateless distortion cost exceeds the running
   block-wide `mds0_best_cost` is dropped BEFORE its rate is priced. The
   port's intra lane had the gate; the inter lane now applies it too, with
   the best cost shared across classes exactly as C's per-block variable is.

### The 2026-09-10 record

The port refused every frame past frame 1, so "does a longer encode decode?" was
unanswerable. `SVTAV1_INTER_CHAIN_EXPERIMENTAL` (default-off, measurement only)
lifted the second refusal — the byte-parity guard on an inter frame whose LIST-0
reference is itself inter — and made the answer measurable
([record](../benchmarks/video_multiframe_2026-09-10.meta)).

**It works.** On `vidyo3 256×256 q40 p6`, low-delay P: 2, 3, 4, 5 and 6 frames
decode completely under **both** `aomdec` and `dav1d`, and the two decoders'
output is byte-identical to each other. At 7+ frames aomdec reports *"Failed to
decode tile data"* on f6. C encodes and decodes 8/8 on the same `.yuv`, so the
defect is port-side. The limit is not fixed — `johnny` and `fourpeople` do 8/8,
`vidyo3` at q20 fails *earlier* (f5), and preset 8 is clean, the **same
preset 2–7 band** as the frame-0 divergence above.

**Root cause found and FIXED 2026-09-10**
([record](../benchmarks/deblock_skipinter_txsize_2026-09-10.meta)): a **skip
inter block's deblock transform size is the block's max, not its searched
`tx_depth`.** libaom's `get_transform_size` reads the per-TU var-tx size only
for `is_inter_block(mbmi) && !mbmi->skip_txfm`; a skip inter block falls through
to `mbmi->tx_size` = the block max, because a block that codes no residual never
puts its searched depth in the bitstream and a decoder cannot know it. The port
applied its `tx_depth` override regardless, so at the failing edges the decoder
read tx 16 and filtered 14-tap while the port read tx 8 and filtered 8-tap. An
intra block never takes that branch — which is precisely why the key frame was
always exact and only inter frames drifted.

| | f0 | f1 | f2 | f3 | f6 | decoded |
|---|---|---|---|---|---|---|
| before | identical | 33 px Δ1 | 86 px Δ6 | 851 px Δ34 | not produced | 6 of 8 |
| after | identical | **identical** | **identical** | 682 px Δ34 | 1020 px Δ36 | **8 of 8** |

The localisation is worth reusing: a qp sweep found that at qp 5–32 the signalled
`loop_filter_level[0]` is 0 and the recon is identical to the decoder's every
time, which isolated the deblock without needing any new instrumentation; the
diff footprint (six pixels each side of the edge) then named the filter length,
and `SVTAV1_PACKTREE` named the blocks as `inter=1 yeob=0 txd=1`.

**Two of the six clips now produce fully correct video** — `fourpeople` and
`kristenandsara` reconstruct byte-identically to the decoder for all 8 frames,
at both qp 20 and qp 40.

**A second defect remains, in the prediction path.** At qp 20 the signalled
`loop_filter_level[0]` is 0, so no filter stage runs on either side and any
mismatch is purely prediction + residual + reference. Four clips still drift
there, and **every one first drifts at f2 — the first frame whose reference is
itself an inter frame.** That is exactly the condition the original refusal
named, so its framing was right; what was missing was a way to measure it. f0
and f1 are correct on every clip at every qp tested.

**Narrowed 2026-09-10 to an exact signature**
([record](../benchmarks/chroma_subpel_inter_2026-09-10.meta)). At 4:2:0 a motion
vector is eighth-pel for luma and sixteenth-pel for chroma, so `mv.col` an **odd
multiple of 8** is integer in luma (`(mv*2) & 15 = 0`) and **half-pel in chroma**
(`mv & 15 = 8`). Classifying every inter block of one frame:

| luma integer | chroma sub-pel | blocks | contain a differing pixel |
|---|---|---|---|
| no | yes | 107 | 0 |
| yes | no | 16 | 0 |
| **yes** | **yes** | **3** | **2** |

107 ordinary sub-pel blocks and 16 fully-integer blocks are perfect; only that
class fails. Reproducer `vidyo3 256×256 p6 qp34 frames=2`, frame 1: the
reference is the byte-identical key frame, `loop_filter_level[0]` is 0 so no
filter stage runs on either side, luma is byte-identical, and only U and V
differ by ±1..2.

**Root cause found and FIXED.** The interpolation-filter search assigns its
winning pair to the candidate — so it is *signalled* — but rebuilds the
prediction only when `res.invalidates_luma_pred`. The search predicts **luma
only**, so chroma still holds the prediction made with the injector's filters
(packed 0, REGULAR both ways) whatever wins; `invalidates_luma_pred` answers a
different question, namely whether the *luma* buffer already holds the winner
because it was tried last. Instrumented proof on the failing block:
`best_filters=0x10001` (SMOOTH/SMOOTH), `was=0x0`, `invalidates_luma=false` —
nothing rebuilt, so the recon carried REGULAR chroma while the header said
SMOOTH. The fix rebuilds when `invalidates_luma_pred || (has_uv &&
best_filters != org)`; that is idempotent for luma, which already holds the
winning pair's prediction whenever the flag is false.

This is why it needed the odd-multiple-of-8 MV to show at all: an integer luma
MV applies no filter, so luma is right under either pair, while chroma's
non-zero phase exposes the wrong one.

**After the fix**, `johnny` joins the clean set: `johnny`, `fourpeople` and
`kristenandsara` all reconstruct byte-identically for all 8 frames, and `vidyo1`
moves from f1 to f4.

### What is left is the temporal motion-vector field, and only that

`SVTAV1_MFMV_OFF` (default-off, measurement only) builds the ref-MV stack from
spatial candidates alone and signals `use_ref_frame_mvs = 0` to match. With it,
**15 of 16 clip × qp cells reconstruct byte-identically to the decoder for every
frame** ([record](../benchmarks/video_mfmv_isolation_2026-09-10.meta)) — every
clip that drifted or stopped decoding becomes clean. So prediction, residual,
entropy, references, deblock and CDEF are all correct, and the temporal field
accounts for essentially the whole remaining defect.

The shape follows: `NEARESTMV`/`NEARMV` do not signal an MV, they **derive** it
from the ref-MV stack. Frame 1 is immune because its reference is the key frame
and C's projection returns nothing for it, which is why f0/f1 were always clean;
from the first frame whose reference is itself inter, a projection mismatch makes
encoder and decoder derive *different* MVs for the same block. That matches the
observed signature — large whole-block luma deltas spreading into neighbouring
intra blocks. It is what the original inter-chain refusal always named; what was
missing was a way to measure it.

Both halves of the flag must move together: gating only the header
desynchronises from frame 0, which produced 0-of-8 decoded streams that briefly
looked like evidence against the hypothesis and were evidence of nothing.

**One residual is not this defect:** `vidyo1` at qp20 still fails from f2 with
MFMV off — the only failing cell of sixteen, and now a clean reproducer.

## imazen26 K300 production corpus (re-run after 48 days, 2026-09-10)

`tools/imazen26_gate.sh` asserts 40 cells over 20 images and covers content
classes no other corpus here reaches — bilevel patent scans, government document
pages, synthetic plots, AI clipart/illustration/product renders, manuscript
scans. **It had not run since the day it was written.** It ran once, 40/40, in
the commit that created it (`304c5832c`, 2026-07-24), against a *materialised*
cache at `/root/work/imazen26-cache/K300` on `dev-32gb` — a rented fleet box,
which the sweep's own meta records as having ended the run at its "box-lifetime
limit". That cache was never copied anywhere persistent (the box's migration
bundle, `~/work/hetzner-backup-dev-32gb` on lilith, deliberately excludes
re-downloadable corpora), so from that point `corpus_dir imazen26-cache/K300`
resolved to an absent path and every cell reported `MISSING`. It had never run
in CI.

**The corpus itself was never at risk, and that is the part worth knowing.**
K300's *selection* is git-tracked in imazen/codec-corpus at
`imazen-26/manifests/imazen26_representatives_K300_2026-06-14.tsv`: 300 rows of
`url  crop_label  content_class  cluster_id  cluster_size`, a k-means
representative pick over imazen-26's 2,160 images spanning all 20 content
classes, with every `url` pointing at the public
`codec-corpus.r2.imazen.org/imazen-26-png-v3/` prefix. All 20 of this gate's
images are in it. A derived cache whose recipe is versioned is not lost data —
only the materialisation was.

**Open discrepancy.** That manifest assigns a per-image crop region
(`crop_label`: `c50_bl`, `c50_center`, `c25_tl`, `full`, …). The gate
centre-crops every image via `crop:` at `IM26_DIM`. The 40 cells are
self-consistent and measured byte-identical that way, but they are not the
regions the representative selection chose.

Re-run 2026-09-10, first time in 48 days and first time ever in CI:
**40 / 40 byte-identical**, 61 s
([record](../benchmarks/imazen26_k300_2026-09-10.meta)). The gate was correct
all along — it had no input.

The assets are the 20 images centre-cropped to 512×512 (all the gate encodes,
since it feeds `crop:<png>` at `IM26_DIM=512`), published at the R2 prefix
`imazen26-k300-512/`: 290 MiB of originals down to 6.1 MiB. The substitution was
**measured**, not argued — the gate produced the same byte count for all 40
cells from both sets.

**Provenance gotcha worth keeping.** Images resolve from imazen-26's
`variant-sets/png-v3-index.tsv` by numeric `id`, never by filename: two of the
twenty carry transposed dimensions in the gate's own cell names (`3000x4000`
where the index says `4000x3000`), the defect `ACCESS.md` records for 196 of
2,160 images. Filename matching misses exactly those two.

## Separate evidence tracks

- [Named-reference audit](PARITY-REFERENCE-AUDIT-2026-09-08.md): pristine and hybrid
  source differences and scoped normal/research matrices.
- [Support audit](API-SUPPORT-AUDIT-2026-09-08.md): implementation reachability.
- [Refusal inventory](REFUSED-CONFIGS.md): generated source predicates, not a count
  of independent bugs and not proof that every refused configuration is invalid C.
- [C defects/oracle history](SUSPECTED-C-BUGS.md): preserve per-build/ISA distinctions.
- [Inter campaign](INTER-ENCODE-PLAN.md): historical experimental video evidence;
  the public streaming API is still unimplemented.

The latest full native workspace passed 2631/2631 at `0cbd1279`; that test
count is independent of the parity cell count. The earlier 106-case regression
number named a passing regression suite, not 106 newly confirmed flaws.
