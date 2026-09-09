# Codex instructions for zenav1-svt

At session start, read `CONTEXT-HANDOFF.md` — in particular "Corrections to the
2026-09-08 handoff", "Remaining work, ordered" and "Evidence boundaries and
resumption". It is an index: the referenced live documents take precedence over
its snapshots. Confirm current commits and workspace state before acting on WIP
described there.

Read `rust/CLAUDE.md` for the working agreement and `rust/docs/WORKING-ON-THIS.md`
before changes. These rules apply when Codex starts at this repository root,
as well as when it starts inside `rust/`. Read large files in bounded chunks
to avoid truncated tool output. Treat Claude-specific action names as their
available Codex equivalents; preserve historical attribution and evidence.

The Rust workspace is `rust/`. Its inner verification loop is
`cargo nextest run --workspace -j 4` and `tools/regression_spotcheck.sh`,
serialized under the shared `run-heavy` wrapper. Follow the live working
document for additional gates appropriate to the change. Never infer current
pass counts from an old handoff or claim a gate passed without running it.
