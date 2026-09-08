# Historical GitHub issue #15: svtrs and svtc are NOT byte-identical at 1024px — the 120-cell parity grid tops out at 512

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/15. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

The matched-wall-clock encode campaign (zenavif `scripts/encode_rd/`) found a parity gap **outside** the grid #14 validated.

## Finding

| size | payloads identical (svtrs vs svtc) |
|---|---|
| 256 px | **13 / 13** |
| 1024 px | **0 / 13** |

Three of the 1024 cells have **matching byte counts but thousands of differing bytes**. Divergence starts in **tile data, not headers**.

## This is a coverage gap, not a contradiction of #14

#14's `byteid_fingerprint.sh` grid is size `{32, 64, 128, 256, 512}` — it never tested 1024. Its 120/120 result stands for what it measured. Everything at and below 256 still agrees here.

## Two actions

1. **Extend `byteid_fingerprint.sh` to 1024 and 2048** and find where parity breaks. The 256-clean / 1024-dirty split suggests something block-count or tile-geometry dependent rather than a systematic arithmetic difference.
2. **A byte-count check is not a parity check.** The RD harness's count-based comparison reported three of these cells as agreeing. Any parity gate must compare content hashes — this one silently passed.

Also worth reconciling while here: the campaign measured **svtrs/svtc slope ratio 4.7x / 5.4x / 6.2x at preset 4 / 6 / 8**, against #14's reported 3.5-4.9x. Preset 4 agrees; the faster presets are worse than that range. Different content, different presets, and process wall-clock rather than `perf_encode`'s tighter encode-only clock — **not reconciled**, and the two clock definitions are not interchangeable.

Record: `zenavif/benchmarks/encode_rd_*_2026-08-08.*`
