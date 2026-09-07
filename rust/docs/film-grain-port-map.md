# Film-grain translation and wiring, 2026-09-07

User requirement: port C's optional features completely and wire their real
callers before running bitexact harnesses. Grain-off parity is not evidence for
grain coverage. Reference: SVT submodule `3115c0c1b23e860dfd75c94f6740e0298182dd13`.

## History audit

`bdd95ac8bffe` translated eight `noise_model.c` helpers; those remain in
`port_noise_model.rs` and are used by the completed model/denoiser. It did not
translate the full model, denoiser, FFTs or reconstruction synthesis.
`136e779ccf48` removed the unconditional call to the **homegrown** estimator
whose result was discarded. It did not delete the C implementation. Historical
heuristic APIs remain in `film_grain.rs`, explicitly distinguished from the
production C translation.

## Implementation and callers

| C surface | Rust implementation and production wiring |
|---|---|
| `noise_model.c` equations, strength solver, greedy LUT fit | `film_grain_model.rs`: bubble-pivot solve, regularization, original accumulation order and C's residual-array update behavior; existing solver leaves retained |
| Flat-block finder, AR observations, noise variance, gain, latest/combined state, grain conversion | `film_grain_model.rs`: owned matrices, flat map, cross-plane correlation, C status handling and per-picture lifecycle |
| `fft_common.h`, `fft.c`, `noise_util.c` | `film_grain_fft.rs` + transcribed 1D FFT/IFFT bodies for sizes 2/4/8/16/32; original expression trees, rounded constants, packing and Wiener filter |
| Six half-cosine windows | `film_grain_windows.rs`: all 5,460 C literals, including the unused 64 window; `tools/transcribe_film_grain_tables.py` regenerates the FFT bodies and windows without recomputing constants |
| Wiener denoiser / `svt_aom_denoise_and_model_run` | `film_grain_denoise.rs`: exact overlap accumulation order, error-diffusion quantization, adaptive 8/16/32 blocks, 8/10-bit samples; source replacement only after successful fitting |
| Picture-analysis preprocessing | Pipeline pads to C's 8-pixel grid **before** denoising, retains denoised padding through the SB source builder, and processes grain before superres/encoding; native 10-bit low bits retained |
| Supplied `fgs_table` | Owned `FilmGrainConfig::table`, C's forced apply flag and per-picture seed; supplied table overrides photon noise and denoising |
| Fork photon noise | Existing `noise_gen.rs` retained; photon noise suppresses denoising; seed recurrence now matches C across zero wraps |
| Sequence / frame syntax | Canonical `entropy::obu::FilmGrainParams`; KEY syntax and INTER update/reference-index syntax; LAST then B-slice LIST_1[0] comparison / ALTREF index signaling, full parameter-array equality ignoring seed, and `ignore_ref` |
| Reference lifetime | Grain parameters refreshed with the DPB refresh mask; ungrained pixels remain the prediction references |
| `grainSynthesis.c` reconstruction output | `film_grain_synthesis.rs`: exact 2,048-entry Gaussian table, LFSR, AR templates, scaling LUTs, offsets, overlap corner ordering and clipping; applied only to requested output reconstruction |
| Public configuration | `EncodePipeline::film_grain` and `AvifEncoder::with_film_grain`; strength/apply/adaptive/table/ignore-ref controls, parameter validation and sequence-presence checks |

These production paths cover the existing encoder's 8/10-bit 4:2:0 surface.
They do not lift the encoder's unrelated INTER, color-format or bit-depth
limitations. The standalone synthesis wrapper also preserves C's floor-even
processing rectangle; the pipeline synthesizes on its aligned reconstruction
before output cropping.

Example:

```rust
pipeline.film_grain = svtav1_encoder::film_grain_config::FilmGrainConfig {
    denoise_strength: 25,
    denoise_apply: true,
    adaptive: true,
    ..Default::default()
};
```

Reconstruction remains opt-in (`with_recon_output(true)`). With grain enabled,
8-bit `last_recon` and native `last_recon10_final` include grain. The DPB and
intermediate 10-bit diagnostic canvases remain ungrained.

## Evidence

`tests/c_parity_film_grain.rs` calls exported pinned C implementations via
`svtav1-cref/shims/film_grain_shims.c`. FFT, filter and flat-block intermediates
compare float **bits**, not tolerances. Solver matrices/solutions, fitted LUTs,
denoised planes, final model parameters and synthesized planes compare exactly.
Coverage includes all FFT sizes, 8/10-bit samples, all estimator block sizes,
strengths 1/25/50, adaptive on/off, all synthesis AR lags, overlap on/off,
chroma-from-luma, restricted/full range, strides and partial block sizes.
The exact noisy-luma/flat-chroma stream witness is also retained.

Seven pipeline tests exercise source replacement versus explicitly denoised
input, failed-estimate behavior, native low-bit preservation, supplied-table
precedence, DPB/output separation, invalid parameters and seed wraps.

`tools/film_grain_gate.py <scratch-directory>` drives both real encoders on
identical inputs. **15/15** cases are byte-identical to C and reconstruction
matches aomdec sample-for-sample; **15/15** differ when decoding with
`--skip-film-grain`. The cells include 8/10-bit adaptive/apply combinations,
supplied tables overriding denoising, partial dimensions 70x66 and 136x72, and
two-frame supplied-table INTER reuse versus forced updates. INTER uses the
existing preset-8 parity envelope. A preset-10 probe encountered the pre-existing
reference-map frontier (C refresh mask 253 vs port 2), outside grain syntax;
its grain seed/update/reference fields agreed. No wider INTER claim is made.

The two failures discovered during live testing are permanent spot-checks:

* Zero-noise chroma: original translation produced 153-byte streams with the
  wrong AR shift/coefficients. C macro NaN semantics and its conversion behavior
  are now explicit; the stream matches C exactly.
* Denoised padding: 70x66 produced 154 bytes versus C's 156 because SB padding
  overwrote denoised mi-grid pixels. Keeping those pixels produces C's 156 bytes.

## Pinned C edge behavior

C can divide `0/0` when chroma noise strength is zero. Its AOMMIN/AOMMAX macros
select their second operand on unordered comparisons; Rust's float min/max do
not. Conversion of NaN/out-of-range doubles to C integers has no portable C
language result. The translation explicitly models x86 CVTTSD2SI (INT_MIN) and
AArch64 FCVTZS saturation, and wrapping subtraction for the subsequent shift.
The x86 witness is verified here; the AArch64 witness requires that target's C
oracle and is not claimed measured on this host.

C's HBD reconstruction synthesis tests chroma point counts without its 8-bit
`chroma_scaling_from_luma` alternative. The standalone translation preserves
that behavior and tests it against C; for this C edge case, matching C's recon
is not a claim of matching a decoder that applies chroma-from-luma grain.

Default-off behavior is checked by the broader workspace and identity gates.
Run all verification through the shared `scripts/run-heavy` wrapper; do not
run the heavyweight gates concurrently. Logs/artifacts live outside the repo.

C's B-slice reuse arm compares LIST_1[0] (BWDREF) but writes ALTREF's map index
(`entropy_coding.c:3126–3131`, including C's TODO). The port retains this
asymmetry. A source-backed branch test uses unequal BWDREF/ALTREF slots; the
live two-frame gate verifies LAST reuse and ignore-ref, not a three-frame
BWDREF/ALTREF divergence outside the existing INTER envelope.

## Verification record (i265)

* Workspace nextest: **2,580 passed, zero skipped**.
* Regression spot-check: **106/106**, including both new grain regressions.
* Full 8-bit identity sweep: **1,100/1,100**, zero pins or harness errors.
* Real-image sweep: **450/450 C-identical and decodable** (50 CID22 images,
  presets 2/6/10, QPs 20/40/55, 8-bit), using `wider_corpus_sweep.sh` with four
  workers. The older `real_image_matrix.sh` could not build its hard-coded
  `/root/aom-rs` decoder dependency on this host; no result is claimed from it.
* Final grain differentials: **7/7**; final live grain stream gate: **15/15**.

Artifacts: `~/tmp/film-grain-2026-09-07/` on i265. Reproduce the live gate with
`tools/film_grain_gate.py <scratch-directory>` under the shared heavy-job wrapper.
Scoreboard SHA-256:

* `full8.tsv`: `cd508f024be7456ee423c87ed78a2a882cef5d5e8b5c0d6b4accc16bf4b37fcf`
* `real450.tsv`: `4a18f11a223be0cf185d8224be956f522ab0dea4c2e56dbc46eab5bcb9212e07`
