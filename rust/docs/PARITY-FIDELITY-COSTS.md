# Parity fidelity costs — what byte-exactness is costing us

Started 2026-09-09.

Places where the port faithfully reproduces C **even though C's behaviour looks
wasteful, skewed, or not Pareto-optimal**. These are not defects and not
divergences: the port is correct, the gates are green, and the bytes match. They
are the *price* of byte-exactness — compression, quality or speed we give up on
purpose to stay identical to the oracle.

## Why this file is separate from SUSPECTED-C-BUGS.md

[`SUSPECTED-C-BUGS.md`](SUSPECTED-C-BUGS.md) tracks C **defects** — off-by-ones,
NULL derefs, dead code, ISA-dependent output. Its question is *"is the port
deliberately insane here, or accidentally?"*

This file tracks a different question: *"when we stop needing byte-exactness, what
should we revisit?"* An entry here is not a bug report against upstream and not a
parity risk. It is a candidate for the Zen-policy encoder — the non-parity mode
described in [ENCODER-POLICY-GOAL.md](../../ENCODER-POLICY-GOAL.md) §3, where the
port is free to beat C rather than match it.

**Nothing here should be "fixed" while `SvtParity` is the shipping contract.**
Changing any of it breaks byte-identity by construction. That is the point of
recording rather than acting.

## Status vocabulary

- `COST-CONFIRMED` — measured: we know what the fidelity costs in bytes/time.
- `COST-UNMEASURED` — the mechanism is understood, the magnitude is not.
- `SPECULATIVE` — looks suboptimal from reading; no measurement, no proof.

## Entries

### 1. The high-bit-depth z2 intra predictor recomputes per pixel

- **Status:** `COST-UNMEASURED` (a speed cost, not a byte cost)
- **C site:** `svt_av1_highbd_dr_prediction_z2_c`, `intra_prediction.c:2404-2435`
- **Port site:** `crates/svtav1-dsp/src/hbd.rs:69` (module doc), `:525-534`
- C's *low*-bit-depth `svt_av1_dr_prediction_z2_c` (`:386-415`) carries `x`, `y`
  and `base` incrementally across the scan. The high-bit-depth sibling recomputes
  all three from scratch at every `(r, c)`. Same output, strictly more work — a
  divergence inside SVT-AV1's own C source, not between C and us.
- The port translates the inefficient shape literally, because the arithmetic has
  to match. Already documented at length in `hbd.rs`.
- **Revisit:** in Zen mode the hbd path can use the lbd incremental form if the
  results are provably identical. Measure the hbd z2 kernel first — it may not be
  hot enough to matter.

### 2. `skip_chroma_rate_est` double-counts the Cb rate in one branch

- **Status:** `COST-UNMEASURED` (an RD-decision skew, so potentially a byte cost)
- **C site:** `full_loop.c:1922`, caller `svt_aom_full_loop_uv` `full_loop.c:2636-2661`
- **Port site:** `crates/svtav1-encoder/src/leaf_funnel/mds3.rs:2847-2858`
- In the `cb_eob < th && cr_eob >= th` case **only**, C writes an approximate Cb
  rate and then adds the full estimate on top, so Cb is priced as
  `approx + full`. Cr never double-counts, because Cb is checked first.
- This feeds the RD cost that drives mode and transform decisions, so it is not
  cosmetic: in that branch the encoder is choosing against a skewed price.
- **Revisit:** this is the highest-value entry here. Zen mode should price Cb
  once and measure the RD delta; the branch is narrow, so the aggregate effect
  could be anywhere from nil to material and nobody has measured it.

### 3. Per-stage NIC scaling clamps twice instead of composing

- **Status:** `SPECULATIVE`
- **C site:** `svt_aom_set_nics`, `product_coding_loop.c:1358-1391`
- **Port site:** `crates/svtav1-encoder/src/port_md/nics.rs:105-118`
- Two multiplicative factors are applied sequentially, each followed by its own
  clamp to the minimum candidate count, rather than being composed into one
  multiply-then-clamp. Whenever the first clamp fires, the two forms differ.
- This reads as an implementation-order artifact rather than a designed policy,
  but that is a reading, not evidence — the two-step form may be deliberate.
- **Revisit:** determine whether the first clamp ever fires at shipping presets.
  If it never does, the distinction is moot and this entry can be closed.

### 4. Inter-intra eligibility tests enum order, not block dimensions

- **Status:** `SPECULATIVE`
- **C site:** `svt_aom_is_interintra_allowed_bsize`, `mode_decision.h:142-144`
- **Port site:** `crates/svtav1-encoder/src/port_md/predicates.rs:184-195`
- The predicate is a range check over `BlockSize` **enum indices**, which excludes
  the extended shapes `8X32` and `32X8` (indices 18/19) even though their
  dimensions sit inside the intended 8..32 window. Inter-intra prediction is
  therefore unavailable for those two shapes.
- If the intent was "dimensions within 8..32", this is an accident that closes off
  an RD tool for two shapes. If the intent was the enum range, it is deliberate.
  The AV1 spec's own constraint should settle which.
- **Revisit:** inter-only, so it is moot until multi-frame lands. Check the spec
  before assuming C is wrong.

## Adding an entry

An entry needs: the C site (`file:function` or `file:line`), the port site, what
the suboptimality *is*, and a status from the vocabulary above. Do not add an
entry for something that is merely unported or unimplemented — that is a gap, and
gaps belong in the tracking issue. Do not add C correctness defects — those go in
`SUSPECTED-C-BUGS.md`. The test for belonging here is: **the port is byte-exact,
the gates are green, and we still think C left something on the table.**

## Provenance

Entries 1-4 came from a bounded review (2026-09-09) of the repo's own source
comments and of every Claude and Codex session transcript for this project, then
each was spot-checked against source before being recorded here.

**The transcripts were a dead end, and that is worth recording so nobody repeats
the search.** Three reviewers covered roughly 165 MB:

| corpus | result |
|---|---|
| Codex 2026-09-06 (156 MB) | nothing in scope |
| Codex 2026-09-05 + 09-07 (8.8 MB) | nothing in scope |
| Claude transcripts + repo source | the four entries above — **all from source comments** |

The near-misses are instructive about what this category is NOT. In the Codex
material every hit that had the right *shape* — "suboptimal", "worse RD",
"redundant", RD-gap analysis — turned out to be one of: another repository's work
(zenrav1e/zenavif RD-gap docs comparing against libaom, not against SVT-AV1 C); a
correctness defect already tracked in `SUSPECTED-C-BUGS.md`; a **port-side**
performance bug in our own harness rather than a claim about C's algorithm; or a
case where the port was wrong and C was right. None of those belong here.

The productive channel was the port's own comments. This codebase already noticed
these things while translating them — it simply had nowhere to file them.
