# There is no handoff document — read the live docs

This file used to be a 697-line snapshot titled *"Claude handoff —
2026-09-10"*, and every entry point in the repository pointed at it first. That
is exactly the failure this project keeps paying for: a session would open it,
absorb several hundred lines of state that had since moved, and act on the
parts that were no longer true. Its own opening paragraph had to warn the
reader off three of its predecessor's claims.

**The rule now: findings live in the document a future session already
re-reads, in the same change that produced them. Never in a "for the next
session" file.** This file survives only so the links pointing at it still land
somewhere useful, and it answers exactly one question — *where do I look?*

| Question | Document |
|---|---|
| What does this encoder support, and what is only partly there? | [README.md](README.md) — the support tables, with the gate that backs each row |
| What is byte-identical to C, what is verified against a decoder instead, and what is open? | [rust/docs/IDENTITY-STATUS.md](rust/docs/IDENTITY-STATUS.md) |
| How do I work in this repo — gates, heavy jobs, hosts, measurement rules? | [rust/CLAUDE.md](rust/CLAUDE.md) and [rust/docs/WORKING-ON-THIS.md](rust/docs/WORKING-ON-THIS.md) |
| What configuration is refused, and why? | [rust/docs/REFUSED-CONFIGS.md](rust/docs/REFUSED-CONFIGS.md) — generated from the refusal strings, so it cannot drift from them |
| What did the inter campaign try, in what order? | [rust/docs/INTER-ENCODE-PLAN.md](rust/docs/INTER-ENCODE-PLAN.md) — a chronology, NOT a status document |
| What was measured, when, on which host? | `rust/benchmarks/*.meta` and `*.md`, each carrying its own date and host |

The original text is preserved verbatim at
[rust/docs/history/2026-09-10/CONTEXT-HANDOFF.md](rust/docs/history/2026-09-10/CONTEXT-HANDOFF.md).
Read it as a dated snapshot if you are tracing how something came to be; do not
read it for current state, and do not resurrect its queue without re-deriving
each item against source.

## Two rules that keep the rest of the tree readable

1. **A dated filename is a snapshot.** `*-2026-MM-DD.md`, anything under
   `rust/docs/history/`, and every `benchmarks/` record describe the day they
   were written. They are evidence, not status. Each carries a first line
   saying so.
2. **Gate output and refusal text are the ledger.** Both are read as fact, so
   the measurement and its date belong inline — and both must be re-measured in
   the same change that moves them. A stale claim is worse than no claim,
   because it also tells the reader not to look.
