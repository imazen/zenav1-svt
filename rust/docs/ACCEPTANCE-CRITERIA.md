# Acceptance criteria — what "done" means for zenav1-svt

## The criterion

`zenav1-svt` is done when it is a drop-in replacement for SVT-AV1 v4.2.0 that emits
**byte-identical bitstreams** — not visually equivalent, not PSNR-matched, not "close
enough at high quality," but the same bytes — for every configuration a still-image
encoder can be asked for: every preset M0–M13, every qp across the full 0–63 range,
8-bit and 10-bit, 4:2:0 / 4:4:4 / monochrome, arbitrary frame dimensions including odd
sizes and partial superblocks, every tile configuration, SB64 and SB128, and every
content class (uniform, gradient, photographic, screen/synthetic) — with the HDR/PSY
fork's features available as an explicitly gated mode that is byte-identical to the
*rebased* fork when on and byte-identical to *mainline* when off; all of it in safe
Rust (`#![forbid(unsafe_code)]`), deterministic across runs and thread counts, panic-
free on adversarial input, and within ~1.2× of C's wall clock. Anything short of that
is a defect with a `file:line`, not an accepted limitation.

## Scope, stated honestly

The envelope above is the **still-image / CQP KEY-frame** envelope — that is what the
port targets and what the harnesses measure. Inter-frame and multi-frame sequence
parity is a later, separately-scoped phase; it is not silently folded into "done."
Lossless is in scope but low priority. The standing priority order is: 10-bit and
arbitrary dimensions first, maintainability continuously, lossless later, performance
last.

## Two products, one baseline

Mainline parity is the baseline; the HDR/PSY fork is a **gated delta on top of it**.
This is not a stylistic choice — the fork is v4.1-based and is *not additive*. It makes
unconditional changes (the loop-filter guard, the uint16→double variance path) that
measured 0/36 parity against mainline. So fork features land behind `SVT_HDR_MODE`,
rebased onto the v4.2 baseline, and each one needs **two witnesses**:

1. **Mode off** → byte-identical to mainline C. This is the anti-regression witness.
2. **Mode on** → byte-identical to the rebased fork-on-4.2 C build.

A feature that satisfies only one of those is not ported — it is a fork of a fork.

## How parity is validated

Every claim is differential against **real C**, never against our own transcription.
Evidence ranks, highest first: a real exported C function > a synthetic facade over a
real C function > verbatim transcription (which can carry shared bugs — a transcribed
oracle agreeing with transcribed code proves only that they were transcribed the same
way).

- **Kernel level** — `c_parity_*.rs` call the actual exported C symbol and compare over
  randomized and edge-case inputs.
- **Stream level** — `identity_diff.sh` feeds one `.yuv` to both encoders and
  byte-compares the OBUs. On mismatch, the od_ec op traces localize the first diverging
  arithmetic symbol and classify the stage (SH / FH / tile-op).
- **Sweep level** — `identity_matrix.sh` is the pass/fail gate over synthetic content.
  `real_image_matrix.sh` is the ratchet over real photographic and screen content,
  where divergences are findings rather than failures **until the end state, at which
  point it becomes a pass/fail gate too**.
- **Anti-vacuous witnesses** — every fix ships with a test that *fails without it*. A
  test that passes before and after the change proves nothing and is not evidence.
- **Landing verification** — every landing is confirmed on `origin/master` with
  `git merge-base --is-ancestor`. A report that something was pushed is a claim, not
  evidence; the two have been different before.

Prohibited, without exception: `#[ignore]` on a failing test, loosened thresholds,
commented-out assertions, runtime "graceful skips," and calling a stub complete. At the
end state no `PORT-NOTE(unverified)` remains unaudited — each is either differentially
tested or consciously carried with a written reason and a named risk.

## Performance

Performance is deliberately the **last** gate: a fast encoder that emits different bytes
is worthless. Once parity holds, the target is ≤ ~1.2× C wall clock at matched preset
and quality, approached in that order — algorithmic parity, then allocation discipline,
then SIMD. Measurement rules: an interleaved paired-statistics harness rather than
back-to-back isolated runs; no `-C target-cpu=native` (runtime dispatch is what users
get); results fitted as `total = intercept + slope · pixels` across tiny / small /
medium / large so per-call fixed cost never hides inside a "ms/MP" figure. Never
extrapolate a measurement from one size to another — measure the size you claim.
Memory numbers come from heaptrack or `time -v`, never from struct arithmetic.

## Reliability

Safe Rust throughout, fallible allocation on untrusted paths, bounded memory. Bitstream
output is deterministic across runs, thread counts, and `--lp` settings — same input,
same bytes, every time. The fuzz corpus produces no panics, no OOMs, and no hangs, and
every fixed crash keeps a minimized regression seed in-tree. CI is green on every target
the crate claims, including `windows-11-arm`, macOS Intel, and `i686-unknown-linux-gnu`
(32-bit correctness is not optional — it is what catches pointer-width bugs and keeps
WASM viable).

Maintainability is part of the deliverable, not overhead. The port's durable value is
that a future maintainer can trace any decision back to a C `file:line`; the C
citations, the `docs/*-port-map.md` files, and the Known-Bugs log are load-bearing. A
byte-identical encoder nobody can safely modify has a short shelf life.

## Summary gate table

| Gate | Criterion | Evidence |
|---|---|---|
| G1 Mainline parity | Byte-identical OBUs vs `SvtAv1EncApp` v4.2.0 across the full still-image matrix | `identity_matrix.sh` + `real_image_matrix.sh` both pass/fail green |
| G2 HDR mode | Mode off == mainline C; mode on == rebased fork-on-4.2 C | Both witnesses per feature |
| G3 Kernel parity | Every kernel differentially tested vs the real exported C symbol | `c_parity_*.rs` |
| G4 Performance | ≤ ~1.2× C wall clock, matched preset/quality | Interleaved paired-stat harness, intercept + slope reported |
| G5 Reliability | Deterministic, panic-free, memory-bounded, CI green incl. arm64-Windows / macOS-Intel / i686 | Fuzz corpus + determinism runs + CI |
| G6 Verification debt | Zero unaudited `PORT-NOTE(unverified)`; no ignored/relaxed tests | Audit sweep at end state |
