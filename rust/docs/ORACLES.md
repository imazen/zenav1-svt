# C oracles — which C encoder the port matches, and how to switch

The port claims byte identity against a **named, pinned C build**, never
against "C" in general. Every C-facing tool resolves one oracle name, and
the Rust encoder's `SvtReference` is selected from that same name, so the
two sides cannot silently disagree about the target.

## The registry

`rust/oracles/oracles.tsv` is the single source of truth. Each row gives:
- the oracle's **name**;
- the **source** (a submodule path);
- the pinned **commit**;
- the **cmake flags**;
- the **API** the C driver compiles against (`fork` or `mainline`), and
  the **driver defines** that bridge each C API difference the wrappers
  must handle (`ZEN_ORACLE_MAINLINE_API`, `ZEN_ORACLE_NO_MDS0_DIST_TYPE`,
  `ZEN_ORACLE_MV_BY_VALUE`, each documented where `wrap_recon.c` tests it);
- the Rust **`SvtReference`** it pairs with, and its **HDR mode**.

| name | C source | pairs with (Rust) | status |
|---|---|---|---|
| `hybrid-3115` | `reference/svt-av1` @ `3115c0c1b`, `SVT_HDR_MODE=OFF` | `Hybrid3115` + `SvtHdrMode::Mainline` | legacy (the default until 2026-09-25) |
| `hybrid-3115-hdr` | same commit, `SVT_HDR_MODE=ON` | `Hybrid3115` + `SvtHdrMode::HdrFork` | legacy fork target (Chromedome only) |
| `mainline-4.2.0` | `reference/svt-av1` @ `9292ec8e3` (upstream tag `v4.2.0`) | `Mainline420` | pristine mainline target |
| `ghost-robot` | `reference/svt-av1-hdr` @ `9dabe3ca` (svt-av1-hdr 4.2 "Ghost Robot") | `GhostRobot` | fork target; replaces the hybrid |

The hybrid is **not** pristine mainline even with its HDR mode off: five of
1,100 cells differ from v4.2.0 in independent-chroma ranking
([PARITY-REFERENCE-AUDIT-2026-09-08.md](PARITY-REFERENCE-AUDIT-2026-09-08.md)).
It stays only until the two real targets are gated, then retires with the
in-house `SVT_HDR_MODE` patch it depends on.

## Switching

```sh
SVT_ORACLE=ghost-robot tools/capture_c_trace/capture_c_trace ...   # C side
SVT_ORACLE=ghost-robot target/release/examples/identity_run ...    # Rust side
tools/oracle list                   # names, pins, build state
tools/oracle build ghost-robot      # materialise + build (idempotent)
tools/oracle libdir ghost-robot     # where libSvtAv1Enc.a lives
```

`SVT_ORACLE` unset means `mainline-4.2.0` (since 2026-09-25, plan 3.7), and
the port defaults to `Mainline420` to match; `SVT_HDR_MODE=1` still means
`hybrid-3115-hdr`, the fork oracle until Ghost Robot replaces it. The
C-parity suite (`svtav1-cref`) still builds the hybrid by default, because its
fork-feature tests need a fork oracle. A gate that pins byte-identity numbers
must name its oracle explicitly in its header and its
output, the way refusal strings carry their date.

`just cparity-oracle <name>` builds in `target/oracle-<name>` inside the
checkout. Never point two jj workspaces at one `CARGO_TARGET_DIR`: cargo
names a workspace member's artifacts by its path relative to the workspace
root, so one workspace silently runs the other's binaries (measured
2026-09-26: a "main" Ghost Robot count was the sibling workspace's).

Under a `mainline`-API oracle the driver has no svt-av1-hdr config fields,
so a fork knob such as `SVT_FORK_TX_BIAS` is refused (exit 2), never
ignored. On the Rust side, `SvtReference::validate_hdr_config` refuses the
same knobs for `Mainline420`, and `GhostRobot` refuses the mainline mode,
because the fork has none.

Where each oracle stands (measured 2026-09-25, `i265`). Re-measure a row in
the change that moves it.

| oracle | still grid, 288 cells (`tools/oracle_still_grid.sh`) | function-level C parity (`just cparity-oracle`) |
|---|---|---|
| `mainline-4.2.0` | 288/288 (re-measured 2026-09-26); also `identity_full_8bit` 1100/1100, `bd10_photo_gate` 191/191, `bd10_nonflat_gate` 309/309 under `SVT_ORACLE=mainline-4.2.0` | 867/867, 8 fork-only tests excluded by the caller |
| `ghost-robot` | 41/288 (re-measured 2026-09-26 at `2848742f`; every cell's verdict pinned by `tools/still_grid_gate.sh` in CI shard 1) | 869/873 on main at `35252c34` (2026-09-26, re-measured in the main checkout); the 4 open divergences are pinned in `oracles/divergent/ghost-robot.txt` and checked by `tools/cparity_ratchet.sh` in CI shard 1 |
| `hybrid-3115` | not run | 875/875 |

## How an oracle is built

The two `hybrid-*` oracles build from the live `reference/svt-av1` working
tree, because temporary C instrumentation is added there (and reverted
before landing, per `AGENTS.md`). Every other oracle is **materialised** from
its pinned commit with `git archive` into
`$ZENAV1_ORACLE_CACHE/<name>-<commit>/` (default `~/.cache/zenav1-oracles`),
built there, and stamped. A stamped build is reused. A changed pin is a new
directory, so an old build can never answer for a new pin.

## Bumping a pin

1. Change the commit in `oracles.tsv` and the submodule gitlink, in one change.
2. Run `tools/oracle build <name>`, then the pin gate
   (`just pins`). Port output does
   not depend on the oracle, so it must not move.
3. Re-run every gate that names that oracle. Record the new pass counts in
   the gate headers, with the date and host.
4. `tools/citations.py check --base <old> --to <new> --tsv out.tsv`. Its
   `changed` rows are the Rust functions and the shim copies whose C moved
   on: refresh the shim copies, and review the Rust arms against the new C.
5. `tools/citations.py remap --base <old> --to <new>` rewrites the `moved`
   line numbers; review the diff.

The fork rebases each release. Its tags reuse mainline names (`v4.2.0`
points at the fork's own release), so pin by **commit**, never by branch or
tag. Mirror the pinned fork commits into `imazen/zenav1-svt-c` so a
force-push upstream cannot make a pin unfetchable. That is an open action
in the plan.

The program that uses this is [PLAN-ORACLES-AND-CLEANUP.md](PLAN-ORACLES-AND-CLEANUP.md).
