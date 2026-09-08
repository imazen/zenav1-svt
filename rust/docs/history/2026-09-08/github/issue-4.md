# Historical GitHub issue #4: Reorg + rename (repo + crates) to zenav1-svt-* + cargo-driven C/HDR build (fresh-box easy mode)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/4. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Track the **reorg + rename + cargo-driven C build (incl. HDR fork)** pass for the
svtav1-rs port. Crate **consolidation** (8→4) is the later pass in #3 — here only
rename in place + delete the two dead stubs.

## Decisions already made (don't re-litigate)

- **Layout — keep the combined repo.** This repo is the SVT-AV1 C fork and its C
  is **patched** (the OFF-by-default `SVT_HDR_MODE`), so a pristine upstream
  submodule isn't possible. Keep the fork; **surface the Rust**:
  - Move `svtav1-rs/` → **`rust/`** at the repo root.
  - Root `README.md` leads with the Rust port (consumer-facing); keep the C
    project's docs reachable but secondary.
  - Track upstream C via `git merge` of gitlab tags (`upstream =
    gitlab.com/AOMediaCodec/SVT-AV1`, already set).
- **Baseline = v4.2.0.** Already true: `master` is `v4.2.0-465-g…`; the committed
  C is v4.2.0-final + the gated HDR patch, and `-DSVT_HDR_MODE=OFF` is
  byte-identical to stock v4.2.0. Just confirm + document it.
- **Prefix:** every crate **package name** gets `zenav1-svt-`.
- **Repo rename:** `imazen/svtav1` → **`imazen/zenav1-svt`** (matches the crate
  prefix and the `zenav1-svt` facade). See Phase 5.

## ⚠️ Sequencing — uncommitted WIP under `svtav1-rs/`

The primary checkout has **uncommitted prior-agent WIP** under `svtav1-rs/`
(`crates/svtav1-entropy/src/{context,obu}.rs`, `crates/svtav1-encoder/src/pipeline.rs`,
`specs/*`, `svtav1/examples/dump_obu.rs`). Moving `svtav1-rs/` → `rust/` will
strand/conflict with it. **Land or reconcile that WIP FIRST (never discard it —
CLAUDE.md), then move.** Do the reorg in a sibling `jj` workspace on
`master@origin`; do not edit the primary checkout's working tree.

## THE THREE BUILD/TEST INVARIANTS (most important part)

**A. Dependency usage is ultrafast — zero C.** A consumer of `zenav1-svt-encoder`
/ `zenav1-svt` must compile with **no C toolchain, no cmake, no build.rs cost**.
The port is pure `#![forbid(unsafe_code)]` Rust — no C in the product path. So:
- Put ALL C building in a **new dev-only crate `zenav1-svt-sys-ref`** (build.rs
  cmake-builds the C). It's a **dev-dependency only** of the crates with
  differential tests — never a normal dep of any published crate.
- **No published crate has a `build.rs`.**
- **Acceptance:** a scratch downstream crate depending on `zenav1-svt-encoder`
  builds with no C toolchain and no cmake.

**B. The C build is cargo-driven, incl. the HDR fork — no manual step.** Today the
C is built by shell (`tools/bd10_matrix.sh`, manual `cmake`). Move it into
`zenav1-svt-sys-ref/build.rs`:
- cmake-build the in-repo C (`../Source`, i.e. the fork root) into **both**
  variants: `-DSVT_HDR_MODE=OFF` (neutral, byte-exact vs stock v4.2.0) and
  `-DSVT_HDR_MODE=ON` (fork). The Rust `hdr` runtime mode compares against the
  ON build, neutral against OFF.
- **Toolchain check** (cmake/nasm/cc) → panic with the one-line install if
  missing. Fresh box: `git clone … && cargo test` builds both C variants
  automatically (once).

**C. Tests run fast.**
- **Cache both C-variant builds** — stamp keyed on the `Source/` git SHA (or tree
  hash); never rebuild on an unchanged tree. First build is minutes (once);
  cached forever after. CI caches keyed on the same SHA.
- **Two test tiers, caller-gated (NO runtime skips per CLAUDE.md):** fast default
  (unit + differential *smoke*) on every `cargo test`; exhaustive byte-exact
  sweep opt-in via a gate threaded CI→justfile→test — never a runtime file-exists
  skip.
- Recommend `cargo nextest` + **consolidated test binaries** (`tests/it/main.rs`).

## Phase 1 — reorg

- [ ] (After WIP reconciled) move `svtav1-rs/` → `rust/`. Update every path:
      `rust/Cargo.toml` members, `rust/justfile`, the licensing files now at
      `rust/LICENSE-AGPL3` + `rust/LICENSE-COMMERCIAL`, the root README's
      `svtav1-rs/…` links.
- [ ] Add `zenav1-svt-sys-ref` (dev-only) with the cargo-driven C build (invariant B).

## Phase 2 — rename to `zenav1-svt-*` + delete dead stubs

- [ ] **Delete** `svtav1-disjoint-mut` (dead + misleading — declared by the
      encoder but referenced nowhere; a `&mut self` `Vec` wrapper, not the
      rav1d-style `&self`/`UnsafeCell` type its docs claim) and `svtav1-cuda`
      (4-line unused FFI stub). Both established in #3; doing them here shrinks
      the rename. Update `[workspace] members` + the encoder's stray dep.

| now | → |
|---|---|
| svtav1-types | zenav1-svt-types |
| svtav1-tables | zenav1-svt-tables |
| svtav1-dsp | zenav1-svt-dsp |
| svtav1-entropy | zenav1-svt-entropy |
| svtav1-encoder | zenav1-svt-encoder |
| svtav1 *(facade)* | **zenav1-svt** |

- [ ] Rename **package names** (keep short `[lib] name` to limit `use` churn if
      you like). Update members/deps/justfile/CI.

## Phase 3 — docs

- [ ] **Root `README.md` → Rust-consumer facing:** the two modes (neutral =
      byte-exact v4.2.0, hdr = svt-av1-hdr fork), install snippet, status (still
      experimental per the current README), fresh-box test flow, links to
      `PORTING.md`. Keep licensing. Note v4.2.0 baseline + the "merge a gitlab tag
      to bump the C" flow.
- [ ] **`PORTING.md`** (new): C→Rust map — which `Source/…` file each `rust/crates/*`
      module ports + how to run its differential gate.
- [ ] **Crate READMEs:** minimal.

## Phase 4 — CI (currently NONE for the Rust port)

- [ ] Add GitHub Actions for the Rust port (only gitlab C CI exists today). Fast
      default tier + opt-in byte-exact sweep; C built via the dev-dep (cached).
      Matrix: `windows-11-arm`, macOS Intel, `i686-unknown-linux-gnu` via cross.

## Phase 5 — GitHub repo rename → `imazen/zenav1-svt`

- [ ] `gh repo rename zenav1-svt --repo imazen/svtav1` (needs admin). GitHub
      auto-redirects the old web + git URLs; then update every reference to the
      new name:
  - [ ] Every `Cargo.toml` `repository` (+ `homepage`/`documentation`):
        `github.com/imazen/svtav1` → `github.com/imazen/zenav1-svt`.
  - [ ] README badges + any `github.com/imazen/svtav1` links; `PORTING.md` /
        `CHANGELOG.md`; the `rust/LICENSE-COMMERCIAL` upstream-note if it names
        the repo.
  - [ ] Only the **`origin`** (imazen) remote is renamed — the **`upstream`**
        (gitlab SVT-AV1) and **`lilith`** (github.com/lilith/SVT-AV1) remotes are
        unaffected and stay. Update `origin`:
        `git remote set-url origin https://github.com/imazen/zenav1-svt.git`.
- [ ] The C project's own docs (root SVT-AV1 README, `Docs/`) reference gitlab,
      not the imazen repo — leave those; only the Rust port's README/Cargo.toml
      URLs change.
- [ ] Verify redirect + CI badge; **do not** recreate `imazen/svtav1` afterward
      (it would shadow the redirect).
- [ ] End state: repo `zenav1-svt` == facade crate `zenav1-svt` == prefix
      `zenav1-svt-*`.

## Constraints

- **jj on main** in a **sibling workspace** (primary checkout has WIP); small
  commits; push `master`; keep `.workongoing`. Assign to `lilith`. `zenav1-` is a
  full zenword → trailing dash allowed.
- **Byte-exactness is sacred** — no rename/move may change a bitstream or golden.
- Heavy builds under `~/work/zen/scripts/run-heavy`.

## Out of scope

- Consolidation 8→4 → #3 (later). Crates will already be `zenav1-svt-` prefixed
  and the two stubs already deleted.


## Historical comment — 2026-08-04T11:52:29Z

**Verified against `57670b3ab` — Phases 1 and 2 are complete, and invariant A holds.**

- Repo is `imazen/zenav1-svt`; the Rust lives at `rust/` at the repo root.
- Every package is prefixed: `zenav1-svt-{types,tables,dsp,entropy,encoder,cref}` + the `zenav1-svt` facade.
- Both dead stubs are gone: `svtav1-disjoint-mut` and `svtav1-cuda` no longer exist.

**Invariant A (dependency usage is ultrafast, zero C) — HOLDS.** Checked every crate:

| crate | published | build.rs |
|---|---|---|
| `zenav1-svt-cref` | no (`publish = false`) | yes |
| `zenav1-svt-{types,tables,dsp,entropy,encoder}` | yes | none |
| `zenav1-svt` | yes | none |

So no published crate has a `build.rs`, and all C building is confined to the dev-only `cref` crate — exactly the shape the invariant asks for.

**Invariant B (cargo-driven C build) — PARTIAL.** `crates/svtav1-cref/build.rs` does drive the C build, so the manual-cmake step is gone. What I could not find is the **dual-variant** half: nothing in that build.rs mentions `SVT_HDR_MODE`, so it appears to build one variant rather than both `-DSVT_HDR_MODE=OFF` and `=ON`. If the fork-mode differential tests are meant to compare against an ON build, that is the remaining work here.

Invariant C (caching + two test tiers) not audited in this pass.

Suggest narrowing this issue to invariant B's dual-variant build (and C if still wanted), since the reorg/rename it was opened for is done.

