# Claude handoff — 2026-09-08

This is the current entry point. It replaces the chronological 964-line handoff;
[the original is preserved](rust/docs/history/2026-09-08/CONTEXT-HANDOFF.md).
Do not treat old “local”, “next”, “blocked”, worker state or CI claims as current.

## Verified source and ownership

| Repository | Remote main/master observed during this audit | What that establishes |
|---|---|---|
| zenav1-svt | `0cbd1279` | PR #20 merged; policy, signed −1, reference selection, support audit and ARM widening landed |
| zenavif | `aae9d98d` | API/replay/grain/routing work `dba8f5ee` is on main; manifest pins SVT `25708931` and AOM `fbea6b4` |
| zenav1-aom | `fbea6b47` | Separate owner has advanced main; no open PRs at audit time. The old “merge #13 then #12” handoff is obsolete |
| zenmetrics | master `3dfee42d` | Fleet/comparator home; current jobs and deployment were not queried in this docs audit |

These are observed commits, not eternal latest pins. Fetch before new work.
SVT PR #20's tested tree and merge tree were identical. zenavif has not yet
picked up that ARM-only implementation merge; its SVT pin contains the prior
API/support fixes. Coordinate any consumer bump with current consumer main.

## What landed

- Checked native −1, explicit pristine/hybrid references and `SvtParity`.
- Checked effort and actual capability-based zenavif execution routing.
- Exact RGB/RGBA8/16 and Gray8 input-kind handling, portable replay and complete
  configuration/cache identity. Replay requires caller-supplied immutable build
  identity and pinned threads; it does not infer dependency provenance.
- Primary-color SVT grain controls, independent ICC+nclx preservation, transfer
  metadata, and AOM Gray8 range correction in the audited zenavif adapter.
- Forced SCM 0/1 and mainline tune-0 sharpness fixes; SCM 3 can correctly equal
  the default at lower presets. Film grain was already translated in C/Rust;
  the recent work added truthful queries and consumer wiring.
- Both C-reference and useful Zen extensions retained. No unmeasured adaptive
  bundle was enabled. Legacy public streaming calls now refuse explicitly.
- PR #20's ARM widening uses registry archmage/magetypes 0.9.29. No Git patch.

## Remaining work, ordered for handoff

1. **Four native10 byte divergences**, 376×512: p1q10, p4q10, p4q30, p5q10.
   [Exact hashes/bytes](rust/docs/deferred-native10-parity.json),
   [first coding difference](rust/docs/HANDOFF-2026-09-08-PARITY.md),
   [retrievable archive receipt](rust/docs/native10-handoff-receipt.json).
   These are real open parity cells, not ignored/passing tests. Both stored
   streams decode. The unproven depth-refine experiment is preserved off main.
2. **Corpus/RD/time calibration and adaptive search.** Complete full imazen-26
   scouting and held-out validation before deriving representatives or routing
   crossovers. Two-origin ablations do not establish a useful representative
   set. Keep output bytes/bpp, perceptual metrics and time together; never pool
   incompatible hardware cohorts. Effort currently selects buckets.
3. **Useful support gaps.** Public streaming video; real GOP/reference/rate
   control/temporal combinations; native10 superres/active restoration; remaining
   HDR-fork wrapper controls; AOM wrapper 4:2:2, Gray16/gain-map and other
   remaining auxiliary paths. AOM RGBA, 4:4:4/RGB identity, full range and the
   documented lossless path are already in zenavif `aae9d98d`.
   Preserve extensions and use backend-reported support, not dead C enum values.
4. **Rollout and quality follow-through.** #18's encoder and zenavif pin are
   fixed; fleet image rollout and replacement of affected outputs are unverified.
   #19's historical quality inversion needs a current-source reproduction.
   The newer zenavif owner reports 58 missing-vector fixture failures in its
   broad local suite (also present at the previous baseline), not 58 codec
   regressions; provision those fixtures before claiming that suite passes.
   Generic animation timing, remaining AVIF surfaces and historical wrapper RD
   findings remain bounded follow-ups, not reasons to undo landed APIs.
5. **Tooling debt.** ARM MSRV/Clippy discrepancy; fresh-machine build/cache
   acceptance (#4); corpus/harness portability and measurement budgets (#8).

[Issue #21](https://github.com/imazen/zenav1-svt/issues/21) is the single umbrella
tracker. [Issue audit](rust/docs/OPEN-ISSUES-AUDIT-2026-09-08.md) records each
issue's disposition. [The policy goal](ENCODER-POLICY-GOAL.md) retains the full
acceptance criteria; it is not complete.

## Evidence boundaries and resumption

- SVT `0cbd1279`: 2631/2631 native workspace tests; 19/19 selected ARM tests
  under QEMU; ARM library/benchmark builds pass. Strict ARM Clippy still has
  17 pre-existing diagnostics. [PR20 record](rust/benchmarks/arm_pairwise_release_2026-09-08.md).
- Earlier eight-bit landing matrix: 1100/1100; four native10 cells remain open.
- zenavif API audit at `dba8f5ee`: 416 selected library/integration tests,
  89 serializer tests, five feature-off replay tests; one existing ignored probe
  test. Later consumer/AOM commits have their own validation records.
- CI was intentionally not awaited. No fleet state, new RD result or hardware
  speedup was measured in this handoff cleanup.

Read only the source/evidence needed for the next task. Use `jj` on main,
refresh `.workongoing`, serialize heavy jobs, run affected local checks and push
coherent verified changes promptly. Do not drop translated controls, weaken
or silently skip tests, or turn historical sample counts into universal claims.
