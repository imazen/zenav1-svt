# Film-grain C parity and wiring, 2026-09-06

User requirement: all C features must be ported and wired carefully. Optional
or default-off C functionality is still required; a helper implementation or
a grain-off byte-parity sweep does not establish feature coverage. Preserve
configuration, lifecycle, computation, header syntax and decoded behavior.

Reference: SVT source3115c0c1. `Source/API/EbConfigMacros.h` enables
`CONFIG_ENABLE_FILM_GRAIN` by default; the RTC build disables it. Default
`film_grain_denoise_strength = 0` is a runtime setting, not absence of support.

| C path | Rust state | Required completion evidence |
|---|---|---|
| Fork photon noise: `noise_generation.c` | `noise_gen.rs` wired through pipeline config to sequence/frame headers for key frames; C table-parity and live-path tests exist | Keep table differential tests and decoder grain-on/off checks; audit remaining frame types |
| Supplied `fgs_table`: `pic_analysis_process.c::apply_film_grain_table` | Preanalysis selector represents this arm, but has no production callers; no public supplied-table pipeline configuration found | Owned table configuration, C precedence rules, parameter copy with per-frame seed preservation, SH/FH agreement, C and decode tests |
| Denoise/model: `denoise_estimate_film_grain` and `noise_model.c` | Only selected helper kernels in `port_noise_model.rs`; model, flat-block finder, solver/fit, denoiser and owned lifecycle incomplete | Port real C arithmetic and state; expose strength/apply/adaptive controls; wire before analysis; compare denoised planes and grain parameters to C |
| Inter-frame grain syntax and reference reuse | Current Rust grain writer implements KEY syntax; C writes update_parameters/ref_idx for INTER | Trace encoder reachability, port frame-type-dependent syntax and reference parameter lifetime, multi-frame C/decode tests |
| Normative grain synthesis / optional recon grain | `film_grain::synthesize_grain` is an LCG placeholder, not C `grainSynthesis.c` | C `enc_dec_process.c:455–486` applies normative grain when producing reconstruction output; port this output behavior and test against C, keeping reference pictures free of synthesis |

C configuration also resolves interactions in `enc_handle.c`: photon-noise
strength disables film-grain denoising; supplied tables take precedence over
photon-noise generation. These interactions need coverage when the missing
configuration is exposed. The preanalysis selector alone does not wire them.

The unrelated `film_grain::estimate_film_grain` heuristic formerly scanned
source/reconstruction on every encode, returned a flat two-point curve, and
had its result discarded. Removing that unused calculation is a performance
cleanup, not completion or removal of a C feature. Its public helper remains
available. It must not substitute for implementing the paths above.

Existing `c_parity_noise_gen` tests and the photon-noise live-path test pass
in the current2,564-test workspace run. The ordinary1,100+450 still identity
gates run grain off and do not cover missing film-grain configurations.
No all-film-grain parity claim is made.


The fresh three-case photon-noise decode gate passes after removing the
unused estimate: at strengths8/25/120 and QP20/40/55, aomdec with
`--skip-film-grain` equals raw Rust reconstruction and normal decoding differs.
This proves KEY-path signaling and decoder synthesis in those cases. It also
makes the reconstruction-output gap explicit: Rust currently exposes pre-grain
reconstruction, while C's requested recon output applies grain. It does not
prove C-equivalent recon-output semantics or inter-frame behavior.
