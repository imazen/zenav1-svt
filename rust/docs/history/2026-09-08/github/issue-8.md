# Historical GitHub issue #8: Doc debt: residual STALE items from the 2026-07-25 publication audit

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/8. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

A full documentation audit ahead of publication fixed the blockers (commit on `main`: "docs+api: fix what a publication/handoff audit found wrong"). This issue tracks the residual **STALE** items it found, so they are not lost.

## Cross-cutting

- **`PORT-NOTE(unverified)` index in `rust/CLAUDE.md` has drifted both ways.** 38 real markers exist (41 grep hits, 3 are past-tense prose). The index under-counts `palette.rs` (lists 6, actual 10), `leaf_funnel.rs` (2 vs 3), `sb128_geom.rs` (1 vs 2), omits `intrabc_pred.rs` and `pipeline.rs:6756` entirely, and still lists two `pipeline.rs` markers that are gone. **Fix:** regenerate mechanically and add the check to `just ci` so it cannot drift.
- **Test-count and line-count tallies** are quoted as current in ~8 docs (`HDR-ON-4.2.md` 669/669, `ibc-port-map.md` 902/902 and 915/915, `sb128-port-map.md` 873/873, `bd10-port-map.md` 915/915, `perf-status.md` 864). Suffix each with "(as of &lt;commit&gt;)".
- **`cargo test --workspace` appears in ~6 docs**; the runner is `cargo nextest run --workspace` (process-per-test isolation is load-bearing for archmage tier pinning).
- **Line-number citations drift** by 1.3–2.4k lines in `finishing-survey.md`, `bd10-port-map.md`, `ibc-port-map.md`. Add a "line numbers as of &lt;commit&gt;; re-locate by symbol" header rather than chasing them.
- **Dead `/root/svtav1/Source/` paths** in `HDR-ON-4.2.md`, `finishing-survey.md`, `sc-detection-port-map.md`, `bd10-port-map.md`, `ibc-port-map.md`, `V4.2-AUDIT-AND-HDR-PLAN.md` — the C tree is `reference/svt-av1/`. Two docs also cite deleted worktrees.

## Contradictions to settle

- **HDR fork verification bar**: `README.md:30` claims byte-identity vs a `SVT_HDR_MODE=ON` C build at 8-bit **48/48**, while `rust/README.md:17` says the fork is explicitly *not* byte-gated. Evidence: `tools/hdr_bd10_gate.sh` IS a byte-vs-C gate (10-bit, 64 cells), but the 8-bit 48/48 figure traces only to prose in `docs/HDR-ON-4.2.md:291` with no gate script and no committed artifact.
- **`bd10_photo_gate` cell count** is quoted as 187, 191 and 154 in three places.
- **`identity_matrix` 132/132** (`rust/README.md:25`, `C-TEST-PORTING-AUDIT.md`) — the default grid is 54 cells; 132 was a retired wider sweep.
- **`screen_ibc_gate` 20/100** — the script's `BYTE_EXACT` list now has 22 entries.

## Docs describing landed work as open

`ibc-port-map.md` §B.1/§B.4 (IntraBC described as unwired, and naming a deleted `svtav1-dsp/src/intrabc.rs` plus three functions that do not exist), `finishing-survey.md` D7 (same), `C-TEST-PORTING-AUDIT.md` 1h (superres "stubbed" — it is ported and gated; `scale.rs` genuinely is still a stub), `bd10-port-map.md` (u8-end-to-end claims), `STATUS.md` roadmap sections, `practical-usage-plan.md`, `sc-detection-port-map.md`, `arbitrary-dims-port-map.md`, `IDENTITY-STATUS.md` harness-axis list, `specs/README.md` (pinned at v4.0.1 `003643d4`, oracle is v4.2.0).

## MEASURED numbers with no committed artifact

Against the "commit benchmark results" rule: `rust/CLAUDE.md`'s SAD/fwd_txfm throughput figures; `README.md:46` real_image_matrix 177/180; `perf-status.md`'s `benchmarks/perf_{before,after}_cdef.tsv` (never existed) plus four other unbacked measurement blocks; `HDR-ON-4.2.md` 48/48 and 0/36; `bd10-port-map.md`'s 540-cell sweep (cited to `/tmp`); `ibc-port-map.md`'s 25,356-block figure. Also `perf-status.md` cites `benchmarks/perf_2026-07-20.*` while those files are modified-uncommitted with a DIFFERENT run.

## New-developer gaps (the highest-value part)

1. **Corpora are undocumented.** Six gates default to `/root/work/codec-corpus/{CID22,clic2025,gb82-sc}` with no doc saying what those are or where to get them — so several headline gate numbers are unreproducible off this box. Document names, sources, layout, and the override env vars (`PHOTO_P0_CORPUS`, `BD10_PHOTO_CORPUS`, `SCREEN_DIR`, `SC_CORPUS`, `RIM_CORPUS`, `WCS_*`).
2. **`photo_p0_gate.sh` silently drops to 6 cells** when CLIC is absent while docs claim 8/8 — a graceful skip inside a gate. Make it fail loudly or document it.
3. **`just` and `tools/decode_diff`** are prerequisites nothing lists (`screen_ibc_gate.sh` hard-fails without the built binary).
4. **The `SVT_HDR_MODE=ON` second oracle** needs `Bin/ReleaseHdr` *and* `capture_c_trace.hdr.bin` — the full sequence is not written down.
5. **`rust/Cargo.lock` is gitignored.** For a project whose product is byte-identical output, an unpinned `archmage` resolve on a fresh box is a reproducibility hazard — either commit it or state why not.
6. **No wall-clock/disk budget** per gate (309-cell and ~190-cell gates run two encoders each).
7. **Add the five corpus-free gates to CI** (`identity_matrix`, `partial_sb_gate`, `arbitrary_size_robustness`, `sb128_gate`, `tile_gate`, `coverage_combos_gate`) — CI currently runs six of 32.

## Historical comment — 2026-08-04T10:58:09Z

Three of the cross-cutting items are done as of `d00c19e14`.

**PORT-NOTE index — generated and gated, not just corrected.** The true count is **44**. Correcting a hand-maintained index of a mechanically-checkable fact only resets the clock, so `tools/portnote_index.sh` now generates the table into a delimited block in `CLAUDE.md` and `--check` runs as a CI step. Verified the gate fires: appended a marker, `--check` exited nonzero; reverted, green. Your suggested fix ("regenerate mechanically and add the check to just ci so it cannot drift") implemented as written.

**Dead C-tree paths** — `/root/svtav1/Source` → `reference/svt-av1/Source` across docs, source comments and xtask.

**Wrong test runner** — `cargo test --workspace` → `cargo nextest run --workspace` in 8 files.

One deliberate exception worth flagging, because it looks like a miss: `benchmarks/*.meta`, `benchmarks/*.tsv` and `docs/captures/*` still contain `/root/svtav1`. Those are **records** of runs that really did read that path on the machine of the day. My first pass rewrote them too; rewriting a path inside a record makes it assert something untrue at capture time, which is falsifying evidence to tidy a grep. They stay as-is.

Still open from this issue: the test/line-count tallies needing `(as of <commit>)` suffixes, the line-number drift header, and the HDR-fork verification-bar contradiction between `README.md:30` and `rust/README.md:17`.
