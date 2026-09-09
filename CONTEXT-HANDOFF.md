# Claude handoff — 2026-09-09

This is the current entry point. It replaces the 2026-09-08 handoff, whose
remaining-work queue was re-derived against source on 2026-09-09 and found to be
correct in scope but wrong in three specific technical claims (see *Corrections*).
Work landed on 2026-09-09 is marked **DONE** inline with its commit; treat every
unmarked item as open.
The chronological 964-line original is
[preserved](rust/docs/history/2026-09-08/CONTEXT-HANDOFF.md).
Do not treat old "local", "next", "blocked", worker state or CI claims as current.

**Host scoping.** The 2026-09-09 audit ran on `i265` (Core Ultra 7 265K, 20
threads, 30 GiB, native Linux). `/mnt/v` and `/mnt/tower` do not exist there.
Every byte-parity result in this document is therefore x86-64 / `i265`-scoped, and
several artifacts named in older docs live on the WSL `lilith` box instead. Name
the host in any new measurement record. On `i265` size heavy jobs `--mem 16G`.

`lilith.lan` (192.168.50.159, the WSL box: 32 threads, 23 GiB, `/mnt/v` present)
is reachable by key from `i265` as of 2026-09-09, so `/mnt/v`-resident corpora and
sources can be fetched rather than declared missing. Size heavy jobs there
`--mem 12G`, not 16G — its VM ceiling is 23 GiB.

**`/mnt/tower` on `lilith` is a stale NFS handle.** The mount entry is present
(`tower:/mnt/user/coefficient`, 192.168.50.170) but every access returns "Stale
file handle" — mounted but dead. Anything treating Tower as the durable mirror is
silently writing nowhere. Remount before relying on it as a backup target.

## Verified source and ownership

| Repository | Remote main/master observed 2026-09-09 | What that establishes |
|---|---|---|
| zenav1-svt | `66e68993` | PR #20 merged; policy, signed −1, reference selection, support audit and ARM widening landed. `66e68993` adds root `apidoc/` and `docs/public-api/` from a concurrent session |
| zenavif | remote main `4c33eeb8`; local main `dba8f5ee` is **5 commits BEHIND** | Local remote-tracking refs are stale here: `git status` reports the wrong direction. Still pins SVT `25708931` |
| zenav1-aom | remote main `1ee19571`; local main `496d2496` is **57 commits BEHIND** | Same stale-ref trap, much larger. Separate owner. A force-push of local main would destroy 57 unrecoverable remote commits |
| zenmetrics | master `6b7990d3` | **DONE 2026-09-09**: the 20-commit av1-compare RD harness was rebased onto `3dfee42d` (zero conflicts) and pushed. `benchmarks/av1-compare` and `benchmarks/av1_compare_2026-09-08` are now on master |

**Stale-ref warning.** Six repos here carry local remote-tracking refs whose true
remote head is absent from the local object DB, so any ahead/behind computed from
`@{u}`, `git status` or `origin/*` is wrong until a fetch. Use `git ls-remote`.
Verified 2026-09-09: only zenmetrics was genuinely ahead; zenavif (−5),
zenav1-aom (−57), claudehints (−3), cavif-rs (−1) and homefleet (−1) are behind.

These are observed commits, not eternal pins. Fetch before new work.

## What landed

- Checked native −1, explicit pristine/hybrid references and `SvtParity`.
- Checked effort and actual capability-based zenavif execution routing.
- Exact RGB/RGBA8/16 and Gray8 input-kind handling, portable replay and complete
  configuration/cache identity. Replay requires caller-supplied immutable build
  identity and pinned threads; it does not infer dependency provenance.
- Primary-color SVT grain controls, independent ICC+nclx preservation, transfer
  metadata, and AOM Gray8 range correction in the audited zenavif adapter.
- Forced SCM 0/1 and mainline tune-0 sharpness fixes. Film grain was already
  translated in C/Rust; the recent work added truthful queries and consumer wiring.
- Both C-reference and useful Zen extensions retained. No unmeasured adaptive
  bundle was enabled. Legacy public streaming calls now refuse explicitly.
- PR #20's ARM widening uses registry archmage/magetypes 0.9.29. No Git patch.

## Corrections to the 2026-09-08 handoff

Each was re-measured or re-read on 2026-09-09; the command that establishes it is
given so the next session can re-check rather than trust this text.

1. **The four native10 cells do not share one root cause.** The old handoff
   pointed every cell at one CfL/independent-UV witness at SB3 `mi(48,8)`. Running
   `tools/identity_diff.py --verbose` over the retained traces gives three
   distinct first-divergence classes, and op 0 is emitted *before* any SB3 block
   data, so the `mi(48,8)` story cannot be the first divergence for the op-0 cells:

   | cell | first divergent op | class |
   |---|---|---|
   | p1q10 | 0 | `lr-taps` (wiener_restore + literal run) |
   | p4q10 | 0 | `lr-taps` (wiener_restore + literal run) |
   | p4q30 | 11758 | cdf, `icdf0=16384`, unrecognized family |
   | p5q10 | 67328 | cdf, `icdf0=20785`, unrecognized family |
   | p1q30 (control) | — | traces identical for all 18413 ops |

   The control makes this anti-vacuous. Detail and the reproduction command are in
   [the parity witness](rust/docs/HANDOFF-2026-09-08-PARITY.md).

2. **The depth-refine experiment is not "preserved off main".** `554412d9d` is an
   *ancestor* of main; its content was reverted by `3522cd856`, so main's
   `depth_refine.rs` equals the pre-experiment file
   (`git diff 554412d9d^ main -- rust/crates/svtav1-encoder/src/depth_refine.rs`
   is empty). Recover the diff with
   `git show 554412d9d -- rust/crates/svtav1-encoder/src/depth_refine.rs`.
   It is also byte-inert on p1q10 and structurally dead at preset ≥ 4, because its
   `bd10` arm (`depth_refine.rs:1527`) requires `!bypass_encdec` while
   `leaf_funnel/rate_tables.rs:1254` sets `bypass_encdec = preset >= 4`.

3. ~~**Main has never been CI-tested.**~~ **RESOLVED 2026-09-09.** It had not
   been: the last green run was `34150341118` (2026-09-07, headSha `8e6f9af4c`)
   and the 12 commits after it all carried `[skip ci]`. Main is now green —
   `34296694287` (`b3e1d37b`, the MSRV bump) passed all four legs including
   `windows-11-arm`, `macos-15-intel` and `i686-unknown-linux-gnu` via cross, and
   `34295564928` (`a046e68b`) and `34295373919`/`34295381305` (`208b89139`) also
   passed. The `gh` token now also carries `workflow`, so `.github/workflows/**`
   is pushable again.

## Remaining work, ordered

Ordering is by *evidence value per unit cost*, not by section number in the goal.
Anchors are `file:line` on `66e68993`. "heavy" means it must be serialized through
`~/work/claudehints/scripts/run-heavy` — one at a time, `--mem 16G` on `i265`.

### 0. Immediate, no build — **ALL DONE 2026-09-09**

- ~~Land these corrections and the plan.~~ `8f6f442e`.
- ~~Give `svtav1-target` the workspace edition and MSRV.~~ `208b8913`. It was the
  one member declaring no `rust-version`, and under `resolver = "3"` its subtree
  was unconstrained on the next `cargo update`.
- ~~Trigger CI on current main.~~ Green — see correction 3. This retired the
  repo's largest unquantified risk; 12 commits had never been tested.
- ~~Raise the MSRV floor to 1.98.~~ `b3e1d37b`. See the decisions section for the
  measurement that justified it.
- ~~Adopt `AGENTS.md` onto main.~~ `0872673b`. It had existed on exactly one ref,
  a branch that was then deleted.

### 1. Native10 parity — four cells, at least three root causes

The evidence is retained under
`~/tmp/svt-tracking/chroma-native-boundary/bd10-p{1,4,5}-q{10,30}/` (c.obu, rs.obu,
c.trace, rs.trace, rs.yuv) plus an SB3 drill in `native10-sb3/`, so localization
costs no compute. The fixture
(`rust/tools/fixtures/partial-chroma-376x512.i420.gz`, sha256 `35ff155e…`) and the
C oracle (`Bin/Release/libSvtAv1Enc.a`, stamp `3115c0c1b`, matching the clean
submodule HEAD) are both present and current.

1. **Separate the loop-restoration hypothesis from the CfL hypothesis** for p1q10
   and p4q10. Both diverge at op 0 in the `lr-taps` class, which is emitted before
   SB3 block data. Either the SB3 CfL flip changes reconstruction and therefore the
   LR search (op 0 is a downstream symptom), or there is an independent native-10
   LR search divergence that no CfL fix will touch. Discriminator: compare the
   *pre-filter* recon planes — C via `SVT_LFRECON_BIN`/`SVT_RECON_BIN`
   (`rust/tools/capture_c_trace/wrap_recon.c`), Rust via `SVTAV1_RECON10_BIN`
   (`rust/crates/svtav1-encoder/src/pipeline.rs:5227`). Byte-identical pre-filter
   planes with differing taps refutes the CfL hypothesis for that cell.
2. **p4q30 and p5q10 are separate investigations.** `identity_diff.py` reports
   "unrecognized family" for both, so the next sub-step is source reading: identify
   which syntax element the `nsyms=14` CDF at p5q10 op 67328 belongs to. Do not
   assume a p1 fix moves them.
3. **The CfL finding itself is narrower and stranger than recorded.** At
   `mi(48,8)` the Rust independent-UV table *agrees* with C (`rs.log:12720` index 7
   = `(2,0)` = `UV_H_PRED` = C's pick), so the ind-uv search is not at fault. Four
   children differ and CfL flips in **both** directions (Rust picks CfL where C
   does not at HORZ nsi=0; C picks CfL where Rust does not at VERT nsi=0), which is
   a near-tie precision problem, not a gate-polarity bug. The structural suspect is
   that `try_encode_frame_420_hbd` (`pipeline.rs:1710-1735`) stores real u16 planes
   but drives the core with `>> 2` u8 proxies, and unthreaded bd10 stages re-widen
   `<< 2`; three such sites sit on the CfL decision path.
4. ~~**Repair `tools/decode_diff`.**~~ **DONE 2026-09-09 (`a046e68b`).** Its
   manifest had hard-coded `/root/aom-rs/crates/aom-decode`, so the pixel oracle
   was unbuildable on every host and `real_image_matrix.sh`, `drill_cell.sh` and
   `screen_ibc_gate.sh` all exited 2 — the real-image parity axis had no runnable
   witness anywhere. Repointed at the sibling checkout; builds in 8 s.
   `drill_cell.sh` also had two `capture_c_trace` sites (`:43`, `:100`) that never
   forwarded the bit depth, so every bd10 drill silently compared against an 8-bit
   C encode; both now pass `${SVTAV1_BD:-8}`. Its unreachable `rc == 3` branch was
   removed and its content-prefix handling fixed.

   **Still to do here:** run `real_image_matrix.sh` for the first current-source
   real-image parity number since the "53 cases fixed" claim. heavy.

### 2. Gates that do not exist — the largest gap the old handoff never named

Every item here is a *missing witness*, not a known-failing one, which is why none
of them appear in any pass count.

- **Research preset −1 has no parity matrix anywhere.** `grep -n 'research\|preset -1'
  .github/workflows/rust-gates.yml` returns zero hits; `identity_full_8bit.sh:190`
  and `bd10_matrix.sh` never emit −1. Only 7 preset-−1 cells exist, in
  `regression_spotcheck.sh`. Goal section 3 requires proven −1 parity across quality
  levels, content classes, 8/10-bit, boundaries and tiles. heavy.
- **HDR mode-on has no CI gate.** `tools/hdr_bd10_gate.sh` is the only harness that
  compares against the `SVT_HDR_MODE=ON` oracle and is absent from the 33-gate CI
  list; its own header records ~10 open cells. ACCEPTANCE G2 needs both witnesses
  per fork feature. heavy.
- **The pristine-mainline oracle — the *default* `SvtParity` target — exists only in
  `~/tmp/svt-tracking/pristine-v4.2.0/`** and has no gate;
  `tools/pristine_reference_compare.py` is committed but not in CI. Archive and
  pointer-file it before a scratch wipe takes it.
- **`real_image_matrix.sh` cannot run on any machine today** (the `decode_diff`
  path above), so the "53 real-image parity cases fixed" claim currently has no
  re-runnable witness. Chain a run onto item 1.4.
- **No clippy, rustfmt, MSRV-floor or semver job exists in CI at all**; all three
  toolchain steps use `stable`, so the declared 1.98 floor is never exercised.
  Platform coverage *is* compliant (windows-11-arm, macos-15-intel, i686 via cross).
- **No fuzz infrastructure exists in this repo** — no `fuzz/`, no targets, no
  regression harness — and there is no determinism-across-thread-count gate, though
  the encoder runs tile-parallel `std::thread::scope` (`pipeline.rs:13382`). The
  cheap half first: a thread-count determinism cell in `regression_spotcheck.sh`.
- **43 `PORT-NOTE(unverified)` markers remain** (ACCEPTANCE G6 requires zero
  unaudited). `tools/portnote_index.sh --check` is a *drift* gate, not a
  debt-reduction gate. Good news nobody recorded: the other half of G6 is already
  clean — zero `#[ignore]` in `rust/crates` and `rust/svtav1`.
- **G4 performance is failing at the parity anchor**: `perf-status.md` records
  native −1 at 6.11 s vs C 2.51 s (~2.4× against a ~1.2× target) with 78% of core
  cycles in `me_sad::__arcane_block_sad_v3`, and `perf_gate.sh` is in no CI step.
  Re-measure before touching code; the last figure predates archmage 0.9.29.

### 3. Correctness bugs found while auditing support gaps

- **`RcMode::Vbr`/`Cbr` are accepted and silently mis-encode.** `target_bitrate`
  appears zero times in `pipeline.rs`; `assign_picture_qp`
  (`rate_control.rs:259-296`, Vbr/Cbr arm at `:271`) starts from
  `RcState::default().qp` = 30 and discards the caller's `config.qp`, while every
  qp-keyed level derivation still reads `rc_config.qp` — a mixed-qp stream. Refusing
  at the `pipeline.rs:1877` choke point is an hours-scale fix and strictly better
  than the current silent wrong output. Wiring the ~4,300 lines of already
  C-parity-tested `port_rc_vbr_cbr*` is multi-day and near-zero value for stills.
- **The superres bd10 refusal text is factually wrong.** It says "the u16 source
  downscale is unported"; `svtav1-dsp/src/port_resize_hbd.rs:215` landed
  2026-08-31, is tier-1 C-gated, and has zero consumers. What is *actually* missing
  is the highbd normative upscale for the recon (`superres.rs:238` clamps to
  0..255). Encode-only bd10 superres is unblocked; recon output is not.
- **The inter envelope in the refusal string is stale**: it says 89/96, the newest
  committed grid (`benchmarks/inter_byte_matrix_2026-09-04-nsqmode.tsv`) is 94/96.
  `REFUSED-CONFIGS.md` is generated from that string and propagates the old number.
- **Multi-frame is capped at two frames** regardless of API work:
  `pipeline.rs:6637-6659` refuses any inter frame whose LIST-0 reference is itself
  an inter frame, and `show_existing_frame` is hardcoded `false` in the header
  writer (`entropy/obu.rs:1422`, `:2338`) even though `port_picstruct_ra` computes
  it. Of five wired RPS branches only `rps_low_delay_cqp` is reachable, because
  `run_picture_decision` (`pipeline.rs:571-592`) hardcodes `LowDelay`/`CqpOrCrf`.
  So "public streaming video" is a multi-day port, not an API surface.
- `HdrForkConfig` has 29 fields; `AvifEncoder` exposes 3. zenavif already exposes 11
  as `expert::SvtParams` (`encoder_svt_rs.rs:186-206`) — that is the proven shape.
- `crates/svtav1-encoder/src/multipass.rs` (225 lines) is referenced nowhere.

### 4. Calibration — blocked on corpus and on an unpushed harness

- **The entire RD/time harness and every measured RD result live on 20 unpushed,
  off-branch jj commits in zenmetrics** (`HEAD` `769bdf33`, on no branch;
  `benchmarks/av1-compare` does not exist on `master`). Nothing downstream is
  reproducible until that lands. Every `rust/tools/` sweep is a *byte-identity or
  wall-time* harness; none computes quality.
- **The imazen-26 K300 cache does not exist** under any path `lib_corpus.sh` probes,
  so `imazen26_gate.sh` and `imazen26_sweep.sh` exit 2. Only 20 unique PNGs
  (~303 MB) are needed for the gate, from the canonical manifest at
  `~/work/zen/imazen-26/manifests/imazen26_representatives_K300_2026-06-14.tsv`.
  Verify each download's sha256 against the git-LFS pointer oid; do not ship an
  unverified cache. Note K300 crosses the canonical split (235 train / 38 val), so
  it is fine for byte parity and **unusable for policy fitting**.
- **A split-clean, train-only 1082-origin population is already hydrated** at
  `~/tmp/av1-training-scout-2026-09-08/corpus`. No validate/test data has ever been
  hydrated. Measured cost: 439.2 s of encode per origin for the declared 78-arm ×
  3-round grid, ≈10 min wall per origin per core ⇒ ≈180 core-hours for all 1078.
- **The fleet ledger-loss root cause is identified in source.** The Nomad job sets
  `ZEN_CHUNK_WALL_SEC="0"`, selecting `execute_gap_claimed`
  (`zenfleet-worker/src/lib.rs:992`), which has no flush callback and returns rows
  only at pass end, while `fleet-entrypoint.sh:117` wraps the pass in `timeout` —
  hence 5 claims, 2 artifacts, 0 durable ledger rows on 2026-09-08. Fix additively
  (`execute_gap_claimed_flushed`, keeping the public signature) or drop the
  `ZEN_CHUNK_WALL_SEC=0` override after measuring the container cpuset. Do not
  hand-roll around zenfleet.
- **Fractional effort has no implementation surface.** `Effort::native_preset()`
  (`rust/svtav1/src/policy.rs:21-24`) maps [0,1] onto 11 discrete buckets
  `[9,8,7,6,5,4,3,2,1,0,-1]`; `still_policy.rs:45-62` proves byte-equality with the
  bucket. No candidate-budget or refinement-depth field reaches `EncodePipeline`.
  This is new work, not wiring.
- **The scout grid is far narrower than goal section 5 requires**: one size
  (max_edge 512), 8-bit, 4:2:0, six quantizers, no lossless/alpha/tile/threading
  axis. Its measured SSIMULACRA2 ceiling at q=5 is ~84–86, so the ssim2 85–98 band
  where AVIF still routing actually lives is entirely unmeasured.
- **`StillSuitability` has exactly one variant, `Uncalibrated`** — routing consumes
  *support* only, never suitability. The versioned shape can be defined and tested
  against a deliberately-empty calibration before any corpus data exists.

### 5. Rollout and quality follow-through

- **#18's rollout evidence already exists on disk and was never consulted.**
  zenmetrics `benchmarks/avif_hdr_rd_baseline_2026-09-03.md:69` records the
  replacement wave `avifhbd-t2a-fix-20260902` complete at 3248/3248 through image
  `ghcr.io/imazen/zenfleet-worker:exec-avifhbd-t2fix-64252836`, and
  `avif_hdr_arm_plan_2026-09-02.md:1436-1442` gives the per-wave validity table.
  What is genuinely open is an independent re-check and a written accounting.
- Two *live* rollout gaps are real: `scripts/jobsys/fleet.env:145` defaults the CPU
  fleet image to a tag whose feature set omits `avif-svt` (so it cannot run an SVT
  cell at all), and zenmetrics CI pins `zenav1-svt` at `aeb619cd8`, which contains
  **neither** #18 tile fix. Both are sibling-repo changes.
- **#19 is fully traced.** `090d19695a8b43c2_512sq`, quality 15→16, SSIMULACRA2
  22.71→22.10, bytes 621→607. The quality→QP map (`avif.rs:443-447` with zenavif's
  `.max(1)`) gives 63 distinct QPs over 101 quality values — 38 duplicates, and it
  reproduces the issue's named collisions exactly, so the saturation observation is
  structural, not an artifact. **No longer blocked** (2026-09-09): the source
  image was retrieved from the WSL box to `~/tmp/issue19/090d19695a8b43c2_512sq.png`
  on `i265`, sha256 `bb3d6c3519d341e9720057d0a6303b13f624977f9378bcfef95f816c69a3146f`
  verified identical on both hosts. `zensim` is also properly checked out on
  `lilith` at `~/work/zen/zensim` (`65dd0c8c`); `i265` has only a pinned cargo
  checkout. The ladder-instrument parquet the older docs name
  (`/mnt/v/output/zensim/ladder-2026-09-05/instruments/dial_grid_372col_ladder.parquet`)
  **does not exist** — there is no `ladder-*` directory under `/mnt/v/output/zensim`
  at all. That path is stale; the image is what the repro needs.
- The **"58 missing-vector failures"** were reported against zenavif `aae9d98d`,
  which is not in the local object store. On the local tree every vector reachable
  from a non-ignored test is present; the missing ones are all behind `#[ignore]`
  or absent directories.

### 6. AOM adoption — 2 of 5 candidates attempted, both measured as no-gain

`enhancements.rs:9-16` declares exactly `AomIntraEdgeFilter` and
`AomRestorationUnitSearch`; both are complete, both measured negative (no RD gain;
+1.4%/1.7% bytes). Never attempted: still SGR, gradient/HOG directional pruning,
learned transform-depth pruning. Completion gate 3 cannot close at 2/5 attempted.
SGR is cheapest — the machinery exists and is preset-gated. Reuse the completed
144-encode ablation harness rather than building a new one.

Also unresolved: the "Zen continuation below native −1" region has no API
coordinate at all (both enhancements are hard-gated to `preset == -1` exactly), and
there is **no test that `SvtParity` rejects the Zen-only region**, which the goal
explicitly requires. Deciding *not* to expose a sub-−1 coordinate is a legitimate
answer; leaving it undecided is not.

## Decisions required from the user

These change what gets built and are not derivable from prior instructions.

1. ~~**MSRV.**~~ **RESOLVED 2026-09-09: floor bumped to 1.98.** The feared lint
   wave did not materialise — measured, x86-64 workspace `clippy --all-targets` is
   byte-for-byte the same lint set at 1.89 and 1.98 (38 occurrences either way).
   On aarch64 the bump *removes* debt: `-p zenav1-svt-dsp --lib` goes 17 → 15
   diagnostics with both `incompatible_msrv` gone, leaving only
   architecture-independent lints.
2. ~~**`gh auth refresh -s workflow`?**~~ **RESOLVED 2026-09-09: the token now
   carries `workflow`.** CI workflow changes are pushable again.
3. ~~**VBR/CBR**~~ **RESOLVED 2026-09-09: tracked, not wired.** Filed as a
   sub-issue of #21 with the ~4,300-line wireup as its todo; the silent
   qp-30 mis-encode is documented there as the interim hazard.
4. ~~**Push the zenmetrics av1-compare stack to master?**~~ **RESOLVED
   2026-09-09: pushed.** Rebased onto `3dfee42d`, zero conflicts, now
   `6b7990d3` on zenmetrics master. It is 39 files / 8,338 insertions of
   AI-authored harness and result TSVs that nobody has audited — treat its
   "verified"/"measured" prose accordingly.
5. **Compute budget for the population scout**: ≈180 core-hours for all 1078 train
   origins on the idle LAN fleet, versus a K≈24-origin local scout first, versus a
   reduced arm set.
6. **May the two zenmetrics fixes be made** (CI pin off the pre-#18-fix rev; a fleet
   image carrying `avif-svt` at current SVT)? Sibling repo, and it holds a stale
   `codex-root` marker.
7. **Bump zenavif's SVT pin** from `257089314` to current SVT main, and does the
   public API grow by 11 `AvifEncoder::with_*` HDR setters or one
   `with_svt_tuning`? Note the local zenavif checkout is 5 commits behind its
   remote (`4c33eeb8`), so fetch and coordinate against *that*, not `dba8f5ee`.
8. ~~**Preserve bookmarks need disposition.**~~ **RESOLVED 2026-09-09: origin
   went from 23 branches to 2.** The count in the previous handoff was wrong —
   there were 13 local / 12 remote preserve bookmarks, not eleven. Deleted: 8
   strict ancestors of main (re-verified `0 unique` commits each), 4 provably
   empty preserves, the 5,097-commit upstream C import (all still reachable from
   tag `v4.2.0`; the `mirror-svt-av1` workflow was confirmed to target the
   separate repo `imazen/zenav1-svt-c`, so nothing broke), and 7 branches whose
   unique content was first archived as tags.

   **Surviving refs:** `main`, and `preserve/2026-09-08-86adc9495b17` — kept
   deliberately because its 33-line `animation-metadata` CI job exists nowhere
   else and is not yet on main. Seven `archive/*` tags hold every rejected
   experiment: `z1-baseline-cpu`, `z1-v3-arcane`, `dct64-cosine-bits`,
   `eob-zero-tails`, `quant-row-cache`, `eng-quality-program`,
   `lint-repairs-2026-09-08`. `v4.2.0` is load-bearing — it is now the only ref
   keeping 5,096 upstream C commits reachable, and it is the named pristine
   parity reference. Do not delete it.

## Evidence boundaries and resumption

- **The ARM dotprod arm has never been measured — do not claim it helped.**
  Verified 2026-09-09 by reading every ARM artifact in the repo. Three separate
  things get conflated here: (a) PR #20 / the "ARM pairwise widening" is a
  *correctness and maintainability* refactor of the plain-NEON arm, measured on
  real Apple M4 Pro hardware with an interleaved randomized paired harness, and
  its measured result is explicitly **no speedup** (42 paired groups, a wash);
  `rust/benchmarks/arm_pairwise_2026-09-07.meta:30` states verbatim that the
  Arm64V2 dotprod dispatch arm is untouched and the result must not be attributed
  to it. (b) `benchmarks/me_sad_ab_2026-09-02.*` (1.018–1.058×) is the only
  end-to-end measurement that involved the dotprod arm at all, but its baseline
  `884f94e8f` had a **pure scalar** ME SAD, so it measures scalar→SIMD confounded
  with token-hoisting and a new 8-wide remainder arm. (c) **No benchmark in the
  repo ever executes `block_sad_arm_v2`** — `crates/svtav1-dsp/benches/kernel_tiers.rs`
  summons `NeonToken` only. `rust/docs/perf-status.md:110-113` already labels the
  dotprod position "Expectation, NOT a measurement". Consequence: the 1.98 MSRV
  floor is a **manifest-honesty fix, not a perf tradeoff** — `vdotq_u32`/`vdot_u32`
  (`me_sad.rs:163`/`:169`) stabilized in 1.98, so the crate provably could not
  compile on aarch64 at its declared 1.89 floor. Pricing dotprod against plain
  NEON remains unmeasured work.
- SVT `0cbd1279`: 2631/2631 native workspace tests; 19/19 selected ARM tests under
  QEMU. Strict ARM Clippy was 17 diagnostics; only **2** were ARM-specific
  (`incompatible_msrv` on the dotprod arm) and the 1.98 floor bump cleared both,
  leaving 15 architecture-independent lints that also fire on x86-64, 12 of which
  collapse to three trivial edit sites. Scoped to `-p zenav1-svt-dsp --lib`; the
  full workspace has additional pre-existing warnings.
  [PR20 record](rust/benchmarks/arm_pairwise_release_2026-09-08.md).
- Earlier eight-bit landing matrix: 1100/1100; four native10 cells remain open.
- The C baseline in every av1-compare table is the **hybrid** submodule `3115c0c1b`
  built with `SVT_HDR_MODE=OFF`, not pristine mainline `9292ec8e`. Every "C vs Rust
  SVT" statement inherits that caveat, and so does every `[C: accepts]` marker in
  the refusal ledger.
- Cross-ISA parity is an open, pinned axis: `bd10_hbd_src_gate.sh:37-47` scopes
  three cells by `uname -m` where the port matches C-on-x86-64 and differs from
  C-on-aarch64 by +3 bytes, with three more in `SUSPECTED-C-BUGS.md` entry 9.
  `fp_cross_isa.sh` already ruled out libm differences (402/402 bit-identical), so
  the cause is unexplained. An aarch64 CI green is **not** an aarch64 parity claim.
- CI was intentionally not awaited. No fleet state, new RD result or hardware
  speedup was measured in this planning pass.

Read only the source/evidence needed for the next task. Use `jj` on main, refresh
`.workongoing`, serialize heavy jobs, run affected local checks and push coherent
verified changes promptly. Do not drop translated controls, weaken or silently skip
tests, or turn historical sample counts into universal claims.

[Issue #21](https://github.com/imazen/zenav1-svt/issues/21) is the single umbrella
tracker. [Issue audit](rust/docs/OPEN-ISSUES-AUDIT-2026-09-08.md) records each
issue's disposition. [The policy goal](ENCODER-POLICY-GOAL.md) retains the full
acceptance criteria; it is not complete.
