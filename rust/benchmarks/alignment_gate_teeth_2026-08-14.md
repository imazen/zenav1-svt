# `tools/alignment_gate.sh` — teeth proof

A gate is only worth its runtime if it FAILS when the defect it exists for is
present. This repo has found vacuous gates before (a probe that silently never
ran; a gate that could not reach the feature it was named for), so this one was
proved by reverting each fix one at a time and re-running it.

* host: Apple M-series (darwin 25.5.0), aarch64
* baseline commit: `18888bb47` (+ the six palette cells added after arm 2 came
  back empty, see below)
* method: `git worktree add --detach ~/tmp/zenav1-teeth HEAD`, revert one fix
  per arm with `git show <sha> -- <path> | git apply -R -`, rebuild, run
  `ALIGN_GATE_MODE=fast tools/alignment_gate.sh`, restore. The C oracle,
  `reference/`, `cbuild-static/` and `Bin/` are symlinked from the primary
  checkout, so every arm uses ONE C library — the port is the only variable.
  (The primary working tree was never modified: a concurrent session owns
  `leaf_funnel.rs`, which arm 2 reverts.)

## Result — 4 arms, 4 sets of failures

| arm | reverted | gate |
|---|---|---|
| baseline | — | **74 / 74** (recon leg: 62 cells, 1,790,779 samples) |
| 1 | `84e3c8627` palette-search crop | **68 / 74** — 6 BYTE failures |
| 2 | `215af947d` + `0163004cc` intra reference clamp | **70 / 74** — 4 BYTE failures |
| 3 | `9f716d791` deblock `onScreen` bound | **70 / 74** — 1 BYTE + **3 RECON** failures |
| 4 | `18888bb47` luma-stride gather | **71 / 74** — 2 BYTE failures |

Verbatim failures:

```
=== ARM: REVERT 84e3c8627 palette-search crop ===
FAILED: screen_96x88_q55_p4_bd8_st96[BYTE 162B vs C 160B]
FAILED: screen_96x88_q55_p6_bd8_st96[BYTE 207B vs C 205B]
FAILED: screen_104x88_q55_p4_bd8_st104[BYTE 187B vs C 185B]
FAILED: screen_88x88_q55_p6_bd8_st88[BYTE 199B vs C 197B]
FAILED: screen_72x88_q55_p4_bd8_st72[BYTE 141B vs C 138B]
FAILED: screen_80x88_q55_p6_bd8_st80[BYTE 182B vs C 181B]

=== ARM: REVERT 215af947d+0163004cc intra reference clamp ===
FAILED: screen_125x129_q33_p2_bd8_st125[BYTE 369B vs C 369B]
FAILED: screen_125x129_q12_p4_bd8_st125[BYTE 345B vs C 345B]
FAILED: screen_190x130_q33_p2_bd8_st190[BYTE 534B vs C 533B]
FAILED: screen_190x130_q12_p4_bd8_st190[BYTE 510B vs C 510B]

=== ARM: REVERT 9f716d791 deblock onScreen guard ===
FAILED: gradient_128x188_q33_p2_bd8_st128[RECON 11px first Y@r186 c56 dec=213 enc=214]
FAILED: gradient_124x188_q33_p2_bd8_st124[RECON 36px first Y@r6 c123 dec=51 enc=52]
FAILED: gradient_127x129_q33_p2_bd8_st127[BYTE 1369B vs C 1369B]
FAILED: gradient_188x256_q33_p2_bd8_st256[RECON 15px first Y@r47 c187 dec=22 enc=23]

=== ARM: REVERT 18888bb47 luma-stride gather ===
FAILED: gradient_128x128_q33_p2_bd8_st192[BYTE 1378B vs C 1307B]
FAILED: gradient_128x128_q33_p6_bd8_st135[BYTE 2694B vs C 1516B]
FAILED: screen_72x88_q55_p4_bd8_st72[c-err]
```

## Three things worth keeping

**Arm 1 came back EMPTY on the first pass — 74/74 with the palette crop
reverted.** The cell list at that point had `screen` content on straddling dims
at q33/q12, which is not enough: the padded rows only change the colour
histogram / dominant-colour argmax / k-means seed when the block's in-frame part
is a MINORITY of its colours. A sweep with the fix reverted (12 geometries x
presets {2,4,6} x qp {12,33,55}) found the reachable set to be **q55 at presets
4 and 6, on a true height of 88 (aligned 128 — a 40-row bottom straddle)**, and
only there. Those six cells are now in the gate. This is precisely the
"a gate that cannot reach the feature cannot guard it" failure, caught by doing
the experiment rather than by reasoning about it.

**Arm 3 is the case for the second oracle.** Three of its four failures are on
the RECON leg — the encoder's own reconstruction disagreeing with `aomdec` —
and only one is a byte difference from C. A C-only gate would have reported
that defect as one cell of "we differ from C" instead of three cells of "the
encoder reconstructed pixels the decoder does not". The failing positions name
the mechanism directly: `Y@r47 c187` on a 188-wide frame is the last true
column, and `Y@r186 c56` on a 188-tall frame is two rows above the true bottom
edge.

**One unexplained line.** Arm 4's third failure is a `c-err` (the C driver
returned nonzero), not a byte mismatch, on a cell whose luma stride EQUALS its
width — where the reverted code path is byte-identical to the fixed one, so it
should not fail at all. It did not reproduce: the same cell, same reverted
build, ran 3/3 clean afterwards (`port_rc=0 c_rc=0`, 138 B). Recorded as a
transient rather than dropped, because the two BYTE failures above it are the
arm's real teeth and this line is not evidence of anything yet.
