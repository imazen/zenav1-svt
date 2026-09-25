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
| `hybrid-3115` | `reference/svt-av1` @ `3115c0c1b`, `SVT_HDR_MODE=OFF` | `Hybrid3115` + `SvtHdrMode::Mainline` | legacy default; every existing gate was measured against it |
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

`SVT_ORACLE` unset keeps today's behaviour: `SVT_HDR_MODE=1` means
`hybrid-3115-hdr`, anything else means `hybrid-3115`. A gate that pins
byte-identity numbers must name its oracle explicitly in its header and its
output, the way refusal strings carry their date.

Under a `mainline`-API oracle the driver has no svt-av1-hdr config fields,
so a fork knob such as `SVT_FORK_TX_BIAS` is refused (exit 2), never
ignored. On the Rust side, `SvtReference::validate_hdr_config` refuses the
same knobs for `Mainline420`, and `GhostRobot` refuses the mainline mode,
because the fork has none.

First measurement (2026-09-25, `i265`, `gradient 128x128 p6`, one switch
driving both encoders):

| oracle | q20 | q45 |
|---|---|---|
| `hybrid-3115` | same | same |
| `hybrid-3115-hdr` | same | differs (357 B port vs 443 B C) |
| `mainline-4.2.0` | same | same |
| `ghost-robot` | differs | differs (phase 3 of the plan) |

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
4. Refresh the C copies in `svtav1-cref/shims` whose cited source changed.
   The shim-citation check lists them; it is still a planned tool, see the
   plan's phase 2.
5. Remap `file.c:NNN` citations in Rust source (planned tool, same phase).

The fork rebases each release. Its tags reuse mainline names (`v4.2.0`
points at the fork's own release), so pin by **commit**, never by branch or
tag. Mirror the pinned fork commits into `imazen/zenav1-svt-c` so a
force-push upstream cannot make a pin unfetchable. That is an open action
in the plan.

The program that uses this is [PLAN-ORACLES-AND-CLEANUP.md](PLAN-ORACLES-AND-CLEANUP.md).
