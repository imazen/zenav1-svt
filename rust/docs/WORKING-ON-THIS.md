# Working on this port — read this before your first change

The goal of this file is that you **fall into doing the right thing**. Every
rule below was paid for: each one exists because someone (usually an AI session,
often a very careful one) drew a confident wrong conclusion and cost hours.

---

## 1. The one-minute loop

```bash
cd rust
cargo nextest run --workspace -j 4      # ~3s, 1000+ tests. NOT `cargo test`.
tools/regression_spotcheck.sh           # ~90s, one cell per bug we ever fixed
```

That is your inner loop. Both must be green before you look at anything else.

The **spot-check** is the important one: every cell in it is the minimal
reproducer of a bug that once shipped, so a red cell *names its own regression*
instead of leaving you to bisect a 2,000-cell sweep.

## 2. The full sweeps — when you actually need them

```bash
tools/identity_full_8bit.sh             # ~25 min, 1036 cells, all presets
IF_TIER=real tools/identity_full_8bit.sh    # ~45 min, real corpora
python3 tools/coverage_matrix.py        # instant: what is COVERED, not what passes
```

Run these before landing anything that touches mode decision, partition,
quantization or the bitstream writer. Not on every edit.

**Read `coverage_matrix.py` output before you read any pass rate.** It prints
`--` for an axis with no cells. A missing cell count is a coverage claim nobody
tested, and it is strictly more dangerous than a failing one — see §5.

## 3. Adding a fix? Add its cell.

When you fix a bug, add a line to `tools/regression_spotcheck.sh`. The rule,
which the file enforces on itself:

> A cell earns its place ONLY if it **failed before** the fix and **passes
> after**. If you cannot state the observed failure — the byte counts, the panic
> message, the decoder error — the cell does not go in.

A cell that never failed cannot detect the regression of a fix it never
witnessed. Two cells were rejected from that file on exactly this ground the day
it was written; one of them (`end_tx_depth`) is a real, faithful fix that is
byte-inert on everything measured, so it deliberately has **no** cell and says
so.

If the fix is a size/quality win rather than byte-identity, use the `ratio`
helper instead of `byte` — pretending a size fix delivered byte-parity is how a
registry rots into something people delete.

## 4. Evidence tiers — say which one you have

Ranked, strongest first. State the tier in your commit message.

1. **A differential against the real exported C symbol** (`crates/svtav1-cref` +
   a `c_parity_*.rs` test). This drives the actual C code.
2. **A byte-identity cell** against `capture_c_trace` (the real encoder).
3. **A decode gate** — `aomdec`/`dav1d` accept the stream, and the encoder's own
   recon equals the decoder's output.
4. **Hand-derived vectors traced against the C source.** The weakest tier. Use
   only when the C function is `static` with no exported symbol, and say so.

A transcribed oracle agreeing with transcribed code proves only that both were
transcribed the same way.

## 5. The harness traps

Each of these produced a confident wrong answer in a single day. They are not
hypothetical.

**A silent harness and a genuine absence are indistinguishable.** An inline
shell loop that never ran gave `grep -c` = 0, which was read as "this C rule has
no counterpart to fix". The rule was live at preset 7. **Before you trust a
ZERO, prove the probe fires somewhere.** Print a positive control. Prefer a
script *file* over an inline shell loop for anything whose result you will act
on.

**`SVTAV1_PACKTREE` appends.** `rm -f` it before every run. A first pass at a
per-preset IntraBC table reported preset 7 coding 3502 blocks when it codes
zero, because the counts were cumulative. It was caught only because p7 exactly
equalled p4.

**Never edit a shell script while bash is executing it.** Bash reads
incrementally; a mid-run edit corrupts the running script and killed a 300-cell
sweep mid-flight.

**Never rebuild Rust while a sweep is using the binary.**

**A gate that cannot reach a feature cannot guard it.** The panic-freedom gate
encoded `gradient` only — which never arms the screen-content detector — so
palette and IntraBC were switched off in all 64 of its cells, and it sailed past
two real out-of-bounds panics. Synthetic content also **never** codes an IntraBC
block at any preset (measured), so IntraBC can only be tested on the real screen
corpus. Ask what your test actually reaches, not what it nominally covers.

**On this platform there is no arithmetic-coder op trace.** `capture_c_trace`
needs `-Wl,--wrap`, which Apple's `ld64` lacks, so `build.sh` falls back to a
byte-only driver and `identity_diff.sh` degrades to a byte + header-field
comparison. Byte verdicts are unaffected; symbol-level localization needs a
GNU-ld host.

## 6. Refuse, never emit a plausible-but-wrong stream

Out-of-envelope configs return a typed `Err` from `encode_frame_impl`. They do
**not** encode. A wrong-pixels output indistinguishable from a correct one at
the integration seam is a shipping bug, not a known limitation.

Corollary the harness must respect: **a refusal is not a crash.** `identity_run`
exits **3** on a typed refusal, and gates count that separately.
`arbitrary_size_robustness.sh` once reported 48 correct refusals as PANIC — it
could not tell the port's best behaviour from its worst.

## 7. Dead-looking C stays translated

If a faithful translation appears to have no effect: **keep it, document the
reachability, do not revert.** The analysis calling it dead is often wrong (this
happened and was reversed within the hour), and upstream can re-enable a path
with one commit. Write down what you measured — which presets reach it, which do
not.

Suspected *C* bugs go in `docs/SUSPECTED-C-BUGS.md`, not into a fix. A C bug is
still the oracle; byte-identity means reproducing it.

## 8. What is actually true right now

`STATUS.md` leads with the measured envelope; `docs/*-port-map.md` holds
per-feature plans. Both contain claims written by earlier sessions that
measurement has since overturned — **at least three were wrong on the day they
were written**, and the corrections are recorded in place rather than quietly
patched.

So: **re-measure before you build on a doc claim.** If a doc and the source
disagree, the source wins and the doc gets fixed in the same change.

## 9. Where the bodies are

| you want | look at |
|---|---|
| what is byte-identical, and where it is not | `STATUS.md` |
| coverage per preset per axis | `python3 tools/coverage_matrix.py` |
| every bug we have fixed, with its reproducer | `tools/regression_spotcheck.sh` |
| C code that looks broken | `docs/SUSPECTED-C-BUGS.md` |
| which C file a Rust module ports | `../PORTING.md` |
| the working agreement + envelope guards | `CLAUDE.md` |
| per-feature plans and open chunks | `docs/*-port-map.md` |
| committed measurements | `benchmarks/*.tsv` + the `.meta` beside each |

## 10. The habits that matter most

- **Measure the premise before building on it.** One unverified assumption
  cascades into hours of wrong work.
- **Report what you ran, not what you believe.** "I did not run it" is a fine
  sentence; "verified" for something you inferred is not.
- **An honest localization beats a speculative fix.** "The first divergence is
  at block (x,y), the port picks A at cost C1 and C picks B at C2, here is the
  differing term" is a complete result even with nothing fixed.
- **When you are wrong, correct it in place** — the doc, the comment, the commit
  message. This file's whole value is that its predecessors did that.
