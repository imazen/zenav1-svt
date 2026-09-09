# Current identity status — 2026-09-08

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

## Explicitly open native10 cells

376×512 photo fixture, `SVTAV1_BD=10 SVTAV1_HBD_SRC=1`:

| Native preset | QP | C bytes | Rust bytes | Status |
|---|---|---|---|---|
| 1 | 10 | 11465 | 11462 | open |
| 4 | 10 | 11752 | 11718 | open |
| 4 | 30 | 2827 | 2831 | open |
| 5 | 10 | 11913 | 11913 | open |

The stored C and Rust streams all decode independently. These are byte
mismatches, not demonstrated decoder failures. Hashes, exact fixture and
retained unproven experiment: [deferred manifest](deferred-native10-parity.json).

**These four cells do not share one root cause** (measured 2026-09-09): p1q10 and
p4q10 first diverge at arithmetic-coder op 0 in the `lr-taps` class, p4q30 at op
11758 and p5q10 at op 67328, both in unrelated CDF families, with p1q30 identical
across all 18413 ops as the control. A fix for one is not evidence for the others.
Start at [the coding-order witness](HANDOFF-2026-09-08-PARITY.md), not downstream
loop filters or the old raster-first pixel. **Updated 2026-09-09:** for p1q10 the
first real divergence is block **mi(32,36)** (pixel 144,128), a partition-size flip
(C bsize=3 vs port bsize=1) — not the previously recorded mi(48,8) CfL witness,
which is downstream of it. Established by capturing recon planes and the decision
tree from one run. Archive retrieval:
[native10 receipt](native10-handoff-receipt.json).

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
