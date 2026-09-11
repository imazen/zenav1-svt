# Rust SVT-AV1 port

The [root README](../README.md) describes the supported product surface,
reference selection, installation and licensing. The current source-backed
[support table](docs/API-SUPPORT-AUDIT-2026-09-08.md) supersedes old port-map
completion claims. Start with [the working guide](docs/WORKING-ON-THIS.md); [CONTEXT-HANDOFF.md](../CONTEXT-HANDOFF.md) routes a question to the document that answers it.

The workspace has **six members**: four core libraries (`zenav1-svt`,
`zenav1-svt-encoder`, `zenav1-svt-dsp`, `zenav1-svt-types`), the dev-only
`zenav1-svt-cref` C oracle, and the `svtav1-target` research harness.
Package names and publication flags are authoritative in their manifests;
none of the product libraries needs C to build. The library code forbids unsafe
Rust; SIMD uses archmage/magetypes 0.9.29.

Still encoding supports 8/10-bit 4:2:0 and Rust monochrome/alpha extensions,
including native-u16 pipeline inputs, odd dimensions and supported lossless
paths. `AvifEncoder` returns OBUs. The `avif-container` animation module writes
all-intra AVIF. Unsupported combinations return errors; the public legacy
streaming `Encoder` explicitly refuses submission, flush and packet retrieval.

Legacy defaults select Hybrid3115. Explicit pristine Mainline420 and strict
SvtParity are separate from HDR mode; neutral HDR knobs do not identify a C
source. `NativePreset` supports −1..13; effort is checked and bucketed.
The full goal still requires adaptive fractional budgets and calibrated routing.

## Working commands

Use [WORKING-ON-THIS.md](docs/WORKING-ON-THIS.md) for prerequisites, corpus
controls, serialized heavy-job commands and validation scope. The canonical
runner is `cargo nextest run --workspace --locked`; doctests are separate.
At implementation `0cbd1279`, the native suite passed 2631/2631. Strict ARM
Clippy has 15 pre-existing architecture-independent diagnostics. The declared
floor is 1.98, which is what the aarch64 dotprod intrinsics require.

[Identity status](docs/IDENTITY-STATUS.md) records the four deferred native10
cells. [HDR history](docs/HDR-ON-4.2.md) retains feature/kernel evidence, not a
current completeness claim. [The documentation index](docs/DOCUMENTATION-INDEX.md)
separates historical results from live API and handoff documents.
