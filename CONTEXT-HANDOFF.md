# Claude handoff — 2026-09-09

This is the current entry point. It replaces the 2026-09-08 handoff, whose
remaining-work queue was re-derived against source on 2026-09-09 and found to be
correct in scope but wrong in three specific technical claims (see *Corrections*).
The chronological 964-line original is
[preserved](rust/docs/history/2026-09-08/CONTEXT-HANDOFF.md).
Do not treat old "local", "next", "blocked", worker state or CI claims as current.

**Host scoping.** The 2026-09-09 audit ran on `i265` (Core Ultra 7 265K, 20
threads, 30 GiB, native Linux). `/mnt/v` and `/mnt/tower` do not exist there.
Every byte-parity result in this document is therefore x86-64 / `i265`-scoped, and
several artifacts named in older docs live on the WSL `lilith` box instead. Name
the host in any new measurement record. On `i265` size heavy jobs `--mem 16G`.

## Verified source and ownership

| Repository | Remote main/master observed 2026-09-09 | What that establishes |
|---|---|---|
| zenav1-svt | `66e68993` | PR #20 merged; policy, signed −1, reference selection, support audit and ARM widening landed. `66e68993` adds root `apidoc/` and `docs/public-api/` from a concurrent session |
| zenavif | local main `dba8f5ee` | The 2026-09-08 audit's `aae9d98d` is **not in the local object store**; fetch before quoting it. Pins SVT `25708931`, five commits behind SVT main |
| zenav1-aom | local main `93d12c35` | The audit's `fbea6b47` is likewise not local. Separate owner |
| zenmetrics | master `3ab5f791`; local HEAD `769bdf33` **detached, 20 commits, on no branch** | The RD harness and every measured RD result exist only off-branch. The audit's `3dfee42d` is not in the local repo |

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

3. **Main has never been CI-tested.** The last `rust gates` run is `34150341118`
   (2026-09-07T18:07:06Z, headSha `8e6f9af4c`). Every commit since carries
   `[skip ci]`. `workflow_dispatch` is enabled, so a run costs one command.
   Separately, the `gh` token holds `gist, read:org, repo` and **not** `workflow`,
   so `.github/workflows/**` cannot be pushed from this machine.

## Remaining work, ordered

Ordering is by *evidence value per unit cost*, not by section number in the goal.
Anchors are `file:line` on `66e68993`. "heavy" means it must be serialized through
`~/work/claudehints/scripts/run-heavy` — one at a time, `--mem 16G` on `i265`.

### 0. Immediate, no build

- Land these corrections and the plan (this commit).
- `rust/crates/svtav1-target/Cargo.toml` is the one workspace member declaring no
  `rust-version`; with `resolver = "3"` its subtree is unconstrained on the next
  `cargo update`, and it is the sole member built on windows-11-arm, macos-15-intel
  and i686. Give it `edition.workspace = true` + `rust-version.workspace = true`.
  Leave its divergent `license` alone — that is a user decision.
  Witness: `cargo metadata --offline --no-deps` prints `1.89 2024` for all six.
- **Trigger CI on current main.** Nine-plus untested commits is the single largest
  unquantified risk in this repo, and the fix is one `workflow_dispatch`.

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
4. **Repair `tools/decode_diff`** — `Cargo.toml:22` hard-codes
   `path = "/root/aom-rs/crates/aom-decode"`, which is permission-denied here.
   Repoint at `../../../../zenav1-aom/crates/aom-decode` (verified to resolve).
   This unblocks `drill_cell.sh`, `real_image_matrix.sh` **and**
   `screen_ibc_gate.sh`. While open: `drill_cell.sh` has *two* `capture_c_trace`
   call sites (`:43` and `:100`) and neither forwards the bit depth as argv[7], so
   its step-5 pickpart dump encodes C at bd8 while the port ran bd10 — silently
   wrong, no error. heavy (cold release build of ~83k LOC).

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
  toolchain steps use `stable`, so the declared 1.89 floor is never exercised.
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
  structural, not an artifact. Blocked only on the source image, which lives at
  `/mnt/v/input/zensim/sources/…` — i.e. on the WSL box, not `i265`.
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

1. **MSRV.** Bump the workspace floor 1.89 → 1.98 (matches all testing evidence and
   what `README.md:33` already tells users to install), or keep 1.89 honest with a
   `build.rs` compiler-version probe that cfg's the aarch64 dotprod arm off below
   1.98 and falls back to plain NEON? Suppressing the lint is not an option — the
   code genuinely does not compile at 1.89 on aarch64. Precedent: the last floor
   correction surfaced 79 previously-hidden lints.
2. **`gh auth refresh -s workflow`?** Without it no CI workflow change can be
   pushed, and the local-only branch `preserve/2026-09-08-86adc9495b17` (an
   animation-metadata CI gate) is structurally unpushable. This is a credential
   change on the user's account.
3. **VBR/CBR**: refuse now (hours, stops a silent wrong-output path, but is an
   externally visible behaviour change for any current Vbr caller), or wire the
   ~4,300 already-C-parity-tested lines (multi-day, near-zero still-image value)?
4. **Push the zenmetrics av1-compare stack to master?** Twenty unreviewed off-branch
   commits adding a benchmark crate with C build dependencies. Every calibration
   step depends on it, and it is currently reachable only from `refs/jj/keep/*`.
5. **Compute budget for the population scout**: ≈180 core-hours for all 1078 train
   origins on the idle LAN fleet, versus a K≈24-origin local scout first, versus a
   reduced arm set.
6. **May the two zenmetrics fixes be made** (CI pin off the pre-#18-fix rev; a fleet
   image carrying `avif-svt` at current SVT)? Sibling repo, and it holds a stale
   `codex-root` marker.
7. **Bump zenavif's SVT pin** from `257089314` to `66e68993`, and does the public
   API grow by 11 `AvifEncoder::with_*` HDR setters or one `with_svt_tuning`?
8. **Eleven `preserve/2026-09-08-*` bookmarks** need disposition; four are provably
   empty, and `preserve/2026-09-08-9292ec8e32bc` is 5,097 commits of unrelated
   upstream C history with no merge-base against main. Deleting remote branches is
   externally visible.

## Evidence boundaries and resumption

- SVT `0cbd1279`: 2631/2631 native workspace tests; 19/19 selected ARM tests under
  QEMU. Strict ARM Clippy retains 17 diagnostics — but only **2** are ARM-specific
  (`incompatible_msrv` on the dotprod arm); the other 15 are
  architecture-independent lints that also fire on x86-64, and 12 collapse to three
  trivial edit sites. The 17 is scoped to `-p zenav1-svt-dsp --lib`; the full
  workspace has additional pre-existing warnings.
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
