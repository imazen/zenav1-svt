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
- **Read what a gate ASSERTS, not what its name suggests.** A gate named
  for decoding may only check that the stream PARSES: `bd10_video_gate.sh`
  did until 2026-09-26 (it now compares the recon with aomdec's output, and
  dav1d's with aomdec's). Every gate header states its own limit; that is
  the contract.
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
- **Video ships in a measured envelope: 4:2:0, presets -1..13, at 8 AND 10
  bits**, verified against a DECODER rather than against C's bytes
  (`tools/video_selfcheck_gate.sh`, 270/270 cells at bd8;
  `tools/bd10_video_selfcheck_gate.sh`, 396/396 cells at bd10 — 2026-09-18).
  Monochrome inter ships at arbitrary sizes (`tools/mono_inter_gate.sh`,
  recon == aomdec == dav1d, including the non-8-aligned legs added
  2026-09-25). Mono qp0 inter refuses — no inter WHT arm. A mono
  animation that hits that refusal is coded all-intra. qp0 coded-lossless
  inter ships at 8-bit 4:2:0 only (`tools/qp0_inter_gate.sh`, 7/7). Do not
  report video as either "working" or "missing".
- **10-bit inter video is supported** (the `hbd_md` question is resolved).
  C derives `hbd_md = 2` at `bd10 && bypass_encdec && perform_md_recon`
  (product_coding_loop.c:9649 area — wraps all of MDS3 + winner select +
  recon, then converts recon back to 8-bit for MD). The port mirrors it as
  `LeafBd10::mds3_hbd`/`FunnelCtx::mds3_hbd()`: canvases plumbed, MDS0/MDS1
  stay 8-bit, MDS3 evaluates and arbitrates at 10 bits. `bd10_video_gate.sh`
  is 24/24 byte-identical to C and `bd10_video_selfcheck_gate.sh` pins
  396/396 cells of encoder-recon == `aomdec` (2026-09-18).
- **Allocation work goes through `crate::vecpool`.** Do not "simplify" the size
  classes or the byte-budgeted depth away — both were measured and the meta
  records what each was worth. `intrabc_hash`'s bucket growth is AT PARITY with
  C and must not be "fixed".
- The facade's raw-OBU API is `svtav1::pipeline`; the internal crates are
  re-exported only with `__expert`.
- Random-access (hierarchical) GOPs encode end to end — decoder-verified
  (`ra_selfcheck_gate.sh`, 11/11), not byte-claimed vs C. Temporal filtering
  is live under RA, including the delayed-intra key path. Open work:
  VBR/CBR rate control, scene-change/adaptive GOP, TPL. The
  MSRV floor is 1.98, matching what the aarch64 dotprod
  intrinsics (`vdotq_u32`/`vdot_u32`, me_sad.rs:163/:169) actually require.

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

- Every byte claim names its C oracle. `rust/oracles/oracles.tsv` is the
  registry and `SVT_ORACLE=<name>` the one switch, read by both
  `capture_c_trace` and `identity_run` ([docs/ORACLES.md](docs/ORACLES.md)).
  The targets are `mainline-4.2.0` and `ghost-robot`; the `hybrid-3115*` rows
  are legacy and retire per
  [docs/PLAN-ORACLES-AND-CLEANUP.md](docs/PLAN-ORACLES-AND-CLEANUP.md).

- C v4.2.0 accepts 8/10-bit 4:2:0. Mono/alpha are Rust extensions; wider chroma
  and 12-bit are alternate-backend/extension work, not shipping-C omissions.
- Constructors default to Mainline420 (pristine v4.2.0) since 2026-09-25, and
  the tools' default oracle is `mainline-4.2.0`; Hybrid3115 is legacy and
  selected explicitly (or by `SVT_HDR_MODE=1`). HDR-off on a hybrid build is
  not proof of pristine parity. SvtParity excludes Zen
  behavior but does not certify all combinations or hide known divergences.
- Signed native −1 is implemented; −2/−3 are refused. C all-intra SGR research
  search and video SGR have different preset selectors; do not say SGR is absent.
- Single-still CRF/CQP equivalence is measured in its stated envelope; multi-frame
  rate control/lookahead remains separate work.
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

Keep every `.rs` file at most 3,000 lines (`tools/file_size_check.py`, a CI
gate); split with the tools in WORKING-ON-THIS.md "Keeping files small".

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

**41 unverified markers**, by file. Regenerate with
`tools/portnote_index.sh`; `--check` is a CI gate, so this cannot drift.
Each marker names a spot whose bit-exactness vs C is ASSERTED but not
PROVEN. Clearing one means deleting the marker in the same commit as the
evidence (an FFI parity test, an identity cell, or a differential).

| file | markers |
|---|---|
| `crates/svtav1-encoder/src/palette.rs` | 9 |
| `crates/svtav1-encoder/src/intrabc.rs` | 8 |
| `crates/svtav1-dsp/src/hbd.rs` | 3 |
| `crates/svtav1-encoder/src/segmentation.rs` | 2 |
| `crates/svtav1-encoder/src/sb128_geom.rs` | 2 |
| `crates/svtav1-encoder/src/leaf_funnel/inject.rs` | 2 |
| `crates/svtav1-encoder/src/entropy/context.rs` | 2 |
| `crates/svtav1-encoder/tests/c_parity_palette.rs` | 1 |
| `crates/svtav1-encoder/src/pipeline/tile_walk/tile_body.rs` | 1 |
| `crates/svtav1-encoder/src/partition.rs` | 1 |
| `crates/svtav1-encoder/src/leaf_funnel/mds3.rs` | 1 |
| `crates/svtav1-encoder/src/intrabc_pred.rs` | 1 |
| `crates/svtav1-encoder/src/bd10.rs` | 1 |
| `crates/svtav1-dsp/tests/c_parity_intra_pred_hbd.rs` | 1 |
| `crates/svtav1-dsp/tests/c_parity_cdef.rs` | 1 |
| `crates/svtav1-dsp/src/inv_txfm.rs` | 1 |
| `crates/svtav1-dsp/src/hbd/filter_intra.rs` | 1 |
| `crates/svtav1-dsp/src/hbd/directional.rs` | 1 |
| `crates/svtav1-dsp/src/hbd/cfl.rs` | 1 |
| `crates/svtav1-dsp/src/hbd/cdef.rs` | 1 |

<!-- PORT-NOTE-INDEX:END -->
