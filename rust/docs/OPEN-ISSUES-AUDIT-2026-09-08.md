# GitHub issue handoff audit — 2026-09-08

Audited all 15 SVT issue bodies/states, all 13 comments on the seven initially
open issues, current source and main ancestry. Implementation main: `0cbd1279`.
The original reports and open-issue comments are [preserved](history/2026-09-08/github/issue-21.md)
under the adjacent `github/issue-N.md` files. Historical closed reports retain
their closed state; this audit does not claim every historical experiment was
rerun or every old external artifact remains retrievable.

## Final disposition

| Issue | Disposition | Remaining acceptance |
|---|---|---|
| #21 | Open, rewritten as the one umbrella tracker | Four native10 cells, calibrated adaptive routing, useful C/video/format/AVIF gaps and cross-repo coordination |
| #19 | Open, narrowed to current-source reproduction | Historical 1/1599 inversion, 621→607 bytes; retain both perceptual metrics and distinct-QP exposure |
| #18 | Open, narrowed to fleet rollout/output replacement | Both encoder tile fixes and zenavif pin landed; deployment and replacement accounting unverified |
| #17 | Closed, completed | Forced SCM 0/1 and mainline tune sharpness fixed and forwarded; SCM3 default-equivalence is legitimate |
| #8 | Open, narrowed to validation portability | Docs refreshed; explicit corpus reduction, fresh corpus setup and gate runtime/disk budgets remain |
| #7 | Closed, superseded by #21 | All unresolved goals carried into #21; closure is consolidation, not feature completion |
| #4 | Open, narrowed to fresh-build/platform acceptance | Layout, core rename, dual oracle and workflows exist; cold/warm isolation/cache/floor evidence remains |
| #3, #5, #6, #9, #11, #13, #15, #16 | Remain closed; bodies marked historical | Original reports retained, no new closure or universal parity claim |

No open PR remains in SVT after #20 merged. Its implementation uses published
0.9.29; [validation record](../benchmarks/arm_pairwise_release_2026-09-08.md).
Issue bodies are rewritten to current scope; old discussion comments remain
attributed history and should not override the current body.

## Source/evidence checks

- `add618a1`, `3121b6a8` and `2ca060f4` are ancestors of main. The current
  workspace run passed 2631/2631, including control and issue18 regressions.
- Raw input validators and legacy streaming refusal are present. `NativePreset`
  is signed; policy/reference and actual frontend execution/replay landed.
- Registry archmage/magetypes are 0.9.29. C normal/HDR builds and stamps exist
  in cref; product dependency edges remain separate from its dev dependency.
- PORT-NOTE and refusal generators pass checks: 43 markers, 60 entries. These
  numbers are inventories, not missing-feature counts. `photo_p0_gate.sh` still
  conditionally omits two CLIC cells, so #8 is not closed by a prose cleanup.
- Remote zenavif `aae9d98d` pins SVT `25708931` and AOM `fbea6b4`, and its updated
  adapter table documents AOM alpha/444/identity/full-range/lossless support.
  The old “query only/no alpha” descriptions are no longer current.
- AOM main observed `fbea6b47`, no open PRs; the former #13/#12 integration queue
  is obsolete. AOM implementation ownership stays separate.
- No fleet jobs/images, new corpus sweep, missing-vector installation or new CI
  run was performed by this docs audit. Those claims remain unverified.

The [current handoff](../../CONTEXT-HANDOFF.md), [support table](API-SUPPORT-AUDIT-2026-09-08.md)
and [documentation index](DOCUMENTATION-INDEX.md) replace the older competing
status queues. Historical measurements and source/trace receipts were preserved.
