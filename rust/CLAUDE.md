# SVT-AV1 Rust port rules

## Start here

Read [the working guide](docs/WORKING-ON-THIS.md) for the workflow and the
gates. Read [README.md](../README.md) for what is supported and which gate
backs each row, and [docs/IDENTITY-STATUS.md](docs/IDENTITY-STATUS.md) for what
is byte-identical to C versus verified against a decoder. The full scope is
[ENCODER-POLICY-GOAL.md](../ENCODER-POLICY-GOAL.md) — a description of the
destination, not a licence to expand an unrelated task.

There is no handoff document. [CONTEXT-HANDOFF.md](../CONTEXT-HANDOFF.md) is a
one-page router from a question to the document that answers it, and it says
why the 697-line version that used to live there was removed.

## What to trust, and what will mislead you

- **A dated filename is a snapshot.** `docs/*-2026-MM-DD.md`, everything under
  `docs/history/`, and every `benchmarks/` record describe the day they were
  written. Grep surfaces them beside live documents; the first line of each
  says which kind it is. Never infer a current pass count from one.
- **Gate output and refusal text are the ledger.** `docs/REFUSED-CONFIGS.md` is
  generated from the refusal strings, so it cannot drift from them, and the
  strings carry their own measurement and date. Move a number only by
  re-measuring it in the same change.
- **Read what a gate ASSERTS, not what its name suggests.** `bd10_video_gate.sh`
  says "decodes", and its decode leg only checks that the stream PARSES — it
  does not compare reconstructions, and 10-bit inter recon is in fact 8 of 18.
  Every gate header states its own limit; that is the contract.
- **A tool that can report a confidently wrong number is a defect.** The
  multi-frame `SVTAV1_FINAL_RECON` dump wrote the 8-bit canvas whatever the bit
  depth, which made a 10-bit comparison read as a total mismatch from frame 0.
  Fix such a tool in the change that finds it, and prefer a tool that refuses
  over one that substitutes.

## Current state and known gaps

Everything here is kept short on purpose; the tables it points at are the
detail, and they are regenerated or gated rather than narrated.

- **Stills are the byte-identical surface** — 8-bit `identity_full_8bit.sh`
  1100/1100, 10-bit `bd10_photo_gate.sh` 191/191 and `bd10_nonflat_gate.sh`
  309/309.
- **Video ships in a measured envelope: 8-bit 4:2:0, presets 6..13**, verified
  against a DECODER rather than against C's bytes
  (`tools/video_selfcheck_gate.sh`, 144/144 cells). Below preset 6, above 8
  bits, and monochrome are REFUSED, each with its measurement in the refusal
  text. Do not report video as either "working" or "missing".
- **`hbd_md` is the open question behind 10-bit video.** Two independent
  measurements point at it: 10-bit STILLS on the same content are 16/16
  identical, and the bd10 chroma-recon bisect found C's chroma recon matching
  the u8 quantizer's. Establish what C derives before changing it.
- **Allocation work goes through `crate::vecpool`.** Do not "simplify" the size
  classes or the byte-budgeted depth away — both were measured and the meta
  records what each was worth. `intrabc_hash`'s bucket growth is AT PARITY with
  C and must not be "fixed".
- Public streaming `Encoder::send_frame` / `receive_packet` are an unimplemented
  scaffold and say so; use `EncodePipeline` or `AvifEncoder`.
- Open work: hierarchical (random-access) GOPs, compound/bipred and inter-intra,
  temporal filtering, VBR/CBR rate control, and the two inter defects below
  preset 6. The MSRV floor is 1.98, matching what the aarch64 dotprod intrinsics
  (`vdotq_u32`/`vdot_u32`, me_sad.rs:163/:169) actually require.

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
