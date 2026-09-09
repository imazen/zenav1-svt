# SVT-AV1 Rust port rules

## Start here

Read [the current handoff](../CONTEXT-HANDOFF.md) and
[working guide](docs/WORKING-ON-THIS.md). They supersede dated status and priority
statements in the [archived instruction/history document](docs/history/2026-09-08/rust/CLAUDE.md).
This cleanup preserves the enduring rules below while removing obsolete active
queues. Historical user attribution remains in that archive. The full scope is
[ENCODER-POLICY-GOAL.md](../ENCODER-POLICY-GOAL.md), not a command to expand an
unrelated new task automatically.

## Current state and known gaps

Main implementation `0cbd1279`: policy/reference/−1 APIs and ARM pairwise work
are merged. Film grain, forced SCM 0/1 and mainline tune-0 sharpness are wired.
Public streaming `Encoder` refuses calls explicitly. Raw pipeline errors,
fallible allocation and cancellation are implemented; they are not new work
merely because a July note calls them absent.

Use [the support table](docs/API-SUPPORT-AUDIT-2026-09-08.md) and
[issue audit](docs/OPEN-ISSUES-AUDIT-2026-09-08.md). Open work includes four
native10 parity cells, broader C video/GOP/temporal/superres combinations,
remaining wrapper controls and broad corpus calibration. #18 deployment/output
replacement is unverified; #19 remains a historical quality-ladder observation.
The MSRV floor is 1.98, matching what the aarch64 dotprod intrinsics
(`vdotq_u32`/`vdot_u32`, me_sad.rs:163/:169) actually require. Strict ARM Clippy
is down to 15 pre-existing diagnostics, all architecture-independent; the two
`incompatible_msrv` ones are gone. PR20 waived nothing.

## Correctness and porting

- Shipping C features include optional/default-off tools. Translate and wire
  configuration, derivation, search, coding, reconstruction and lifecycle before
  declaring completion. A helper or stored enum alone is not support.
- Read the entire C function and its callers. Preserve all constants, tables,
  ordering and tie rules. Cite source symbols and the exact C revision; old
  line numbers are orientation only. Specs derive from older C and cannot
  override the named current reference source.
- Keep faithful translations even when apparently unreachable. Preserve useful
  HDR-fork and Zen extensions; record reachability and positive controls rather
  than deleting them after a null experiment.
- Independent decoder failures and wrong pixels are correctness defects. Fix
  within the task's authorized scope; do not relabel them expected. The four
  byte-only native10 divergences were explicitly authorized for later revisit;
  retain their failing status and evidence, never mark them passing.
- Add regression witnesses that exercise the enabled feature and would fail if
  its wiring disappeared. Test C parity, independent decoding and reconstruction
  separately; source-exact output is the lossless oracle where C is defective.
- Never relax an expectation, threshold or assertion, add a hidden skip, or
  rewrite measured constants just to silence Clippy. Preserve original evidence.
- Caller errors and unsupported combinations must return explicit errors; never
  discard frames/settings or emit plausible-but-wrong output. Query validators
  must agree with real entry points. Raw support is not wrapper support.

## Reference and envelope guards

- C v4.2.0 accepts 8/10-bit 4:2:0. Mono/alpha are Rust extensions; wider chroma
  and 12-bit are alternate-backend/extension work, not shipping-C omissions.
- Legacy constructors use Hybrid3115. Pristine Mainline420 is explicit; HDR-off
  on a hybrid build is not proof of pristine parity. SvtParity excludes Zen
  behavior but does not certify all combinations or hide known divergences.
- Signed native −1 is implemented; −2/−3 are refused. C all-intra SGR research
  search and video SGR have different preset selectors; do not say SGR is absent.
- Single-still CRF/CQP equivalence is measured in its stated envelope; multi-frame
  rate control/lookahead/temporal filtering remains separate work.
- Tile-parallel output is deterministic; tiles may be forced by dimensions.
  Do not assume the caller's zero tile request implies a single tile.
- `COVERAGE.md` is historical C-struct shape coverage, not a count of features.
  The refusal ledger contains contract and implementation guards; its row count
  is not the number of missing features.

## Safe SIMD and verification

- Product Rust forbids unsafe code. Use archmage/magetypes (registry 0.9.29), not
  direct `core::arch` or handwritten assembly. Do not add unsafe to escape a
  missing abstraction. No `#[inline(always)]` on arcane/rite functions.
- Summon a token once per hot path; pass it through helpers. Use `#[arcane]` for
  guarded entries, `#[rite]` for inner helpers, and `incant!` for real dispatch.
- Prefer `cargo nextest run --workspace --locked`; process isolation matters.
  Token disabling is process-global. Tests that depend on a tier must use
  `lock_token_testing`; cross-tier tests must consume `PermutationReport`, assert
  empty warnings and meaningful permutation counts. Avoid target-cpu=native
  when measuring dispatch coverage.
- Floating-point parity is exact where the reference contract requires it.
  Diagnose stage/ISA/compiler differences; do not hide them with tolerances.
- Check the declared MSRV as well as current Rust before claiming floor support.
  The current ARM discrepancy remains open. Do not mechanically round C literals
  to resolve excessive-precision warnings.

## Work and measurement discipline

Use the shared global rules and run-heavy wrapper. Serialize heavy jobs and
refresh `.workongoing`; never run a changing shell script while editing it.
Use coherent commits on main, relevant local validation, frequent verified
pushes, and no CI wait unless requested. Docs-only edits need documentation
checks, not repeated codec sweeps.

Profile before optimizing. Use the repository's interleaved paired benchmark
harnesses; preserve bytes/quality/time and source/hardware identities. Historical
M4 or two-origin results do not establish current general speed/RD conclusions.
Full imazen-26 training and held-out validation precede representative selection
or automatic adaptive defaults. Keep source/trace archives retrievable.

The backend stays codec-only; zenavif owns integration traits and container
routing. Preserve structured errors, stop tokens and configurable allocation.
Additional wrapper cancellation/metadata/support limits remain explicit rather
than inferred complete from a lower-level helper.

## Generated PORT-NOTE(unverified) index

Run `tools/portnote_index.sh` to regenerate; `--check` is the drift gate.

<!-- PORT-NOTE-INDEX:BEGIN (generated by tools/portnote_index.sh — do not edit by hand) -->

**43 unverified markers**, by file. Regenerate with
`tools/portnote_index.sh`; `--check` is a CI gate, so this cannot drift.
Each marker names a spot whose bit-exactness vs C is ASSERTED but not
PROVEN. Clearing one means deleting the marker in the same commit as the
evidence (an FFI parity test, an identity cell, or a differential).

| file | markers |
|---|---|
| `crates/svtav1-encoder/src/palette.rs` | 9 |
| `crates/svtav1-encoder/src/intrabc.rs` | 8 |
| `crates/svtav1-dsp/src/hbd.rs` | 7 |
| `crates/svtav1-encoder/src/segmentation.rs` | 2 |
| `crates/svtav1-encoder/src/sb128_geom.rs` | 2 |
| `crates/svtav1-encoder/src/pipeline.rs` | 2 |
| `crates/svtav1-encoder/src/leaf_funnel/inject.rs` | 2 |
| `crates/svtav1-encoder/src/entropy/context.rs` | 2 |
| `crates/svtav1-encoder/tests/c_parity_palette.rs` | 1 |
| `crates/svtav1-encoder/src/partition.rs` | 1 |
| `crates/svtav1-encoder/src/leaf_funnel/mds3.rs` | 1 |
| `crates/svtav1-encoder/src/intrabc_pred.rs` | 1 |
| `crates/svtav1-encoder/src/frame_geom.rs` | 1 |
| `crates/svtav1-encoder/src/bd10.rs` | 1 |
| `crates/svtav1-dsp/tests/c_parity_intra_pred_hbd.rs` | 1 |
| `crates/svtav1-dsp/tests/c_parity_cdef.rs` | 1 |
| `crates/svtav1-dsp/src/inv_txfm.rs` | 1 |

<!-- PORT-NOTE-INDEX:END -->
