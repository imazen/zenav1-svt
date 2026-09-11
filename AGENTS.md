# Working in zenav1-svt

A safe-Rust (`#![forbid(unsafe_code)]`) port of SVT-AV1 v4.2.0. The Rust
workspace is `rust/`; the C reference is the `reference/svt-av1` submodule and
is read-only except for temporary instrumentation, which must be reverted
before the change lands.

## Read in this order

1. [README.md](README.md) — what is supported, and the gate behind each claim.
2. [rust/CLAUDE.md](rust/CLAUDE.md) — the working agreement.
3. [rust/docs/WORKING-ON-THIS.md](rust/docs/WORKING-ON-THIS.md) — the current
   workflow: which gates to run for which change, how to run heavy jobs, how to
   measure.

Stop there unless you need something specific.
[CONTEXT-HANDOFF.md](CONTEXT-HANDOFF.md) is a one-page router from a question
to the document that answers it; it is not itself state.

## Two rules that decide what to trust

- **A dated filename is a snapshot.** `*-2026-MM-DD.md`, everything under
  `rust/docs/history/`, and every `rust/benchmarks/` record describe the day
  they were written. Grep will surface them beside live documents — check the
  first line, which says which kind it is. Never infer a current pass count
  from one, and never resurrect its queue without re-deriving each item against
  source.
- **Gate output and refusal text are the ledger.** `rust/docs/REFUSED-CONFIGS.md`
  is generated from the refusal strings, so it cannot drift from them; the
  strings themselves carry the measurement and its date. If a change moves a
  number, re-measure it in the same change.

## The inner loop

From `rust/`, serialized under the shared wrapper (one heavy job at a time):

```sh
TMPDIR="$HOME/tmp" ~/work/zen/scripts/run-heavy --mem 12G -- \
  cargo nextest run --workspace --locked --test-threads 4
TMPDIR="$HOME/tmp" ~/work/zen/scripts/run-heavy --mem 12G -- \
  tools/regression_spotcheck.sh
```

Then the gates that cover what you touched — `rust/docs/WORKING-ON-THIS.md`
lists them, and `.github/workflows/rust-gates.yml` is the authoritative set.
Never claim a gate passed without running it.

Size `--mem` to the host: 12G on the WSL box `lilith` (23 GiB ceiling), 16G on
`i265` / `r7900x` (~30 GiB), up to 40G on `dev` (60 GiB). Name the host in any
measurement you record.

## For Codex

These instructions apply at the repository root and inside `rust/`. Read large
files in bounded chunks. Treat Claude-specific action names as their Codex
equivalents, preserve historical attribution, and do not attribute Codex work
to Claude.
