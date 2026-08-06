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

**Corpus gates look for their images in several roots, and say so when they
miss.** Fourteen gates once hard-coded `/root/work/codec-corpus/...` — the path
on one CI image — so on any other host every image `SKIP-MISSING`d.
`screen_palette_gate.sh` then reported `0 / 0 byte-identical` and failed
anti-vacuity; with the corpora found it is `50 / 50` with 38 palette-coding
cells. They now resolve through `tools/lib_corpus.sh` (`$ZENAV1_CORPUS_ROOT`,
then `~/work/zen`, then `/root/work`, then `~/work`). Same lesson as `ionice`
and `-Wl,--wrap`: **probe, never assume one host.**

**`tools/decode_diff` cannot build off the CI image.** Its `Cargo.toml` has a
literal path dependency on `/root/aom-rs/crates/aom-decode`, and Cargo path deps
take no env override, so `real_image_matrix.sh` and `screen_ibc_gate.sh` cannot
build their pixel-classification oracle elsewhere. They now fail with a message
that says exactly that. Treat it as a harness failure, never as a parity result.

**A harness PRECONDITION is a coverage hole.** `identity_run`'s `crop:` mode
rejects odd dimensions ("I420 needs even dims"), so no gate cell could ever
encode an odd-height frame of REAL content. That precondition hid a public-API
panic (`unsupported partition shape (Horz4, 3)`) on a shape only real content
picks, through every sweep in this repo. It was found by a test that builds its
own planes and therefore skips the check. When a harness refuses an input,
write down what that makes untestable — the refusal is not the same as the
input being impossible.

**On this platform there is no arithmetic-coder op trace.** `capture_c_trace`
needs `-Wl,--wrap`, which Apple's `ld64` lacks, so `build.sh` falls back to a
byte-only driver and `identity_diff.sh` degrades to a byte + header-field
comparison. Byte verdicts are unaffected; symbol-level localization needs a
GNU-ld host.

## 5b. Drills you don't have to write

Localizing a divergence starts with narrowing WHAT changed, not reading code.
These are committed so nobody rebuilds them in a scratch dir:

```bash
tools/drill_two_images.sh     # per-preset/per-qp verdicts for the two open images
tools/sc_tool_bisect.sh       # palette? IntraBC? neither? (SVTAV1_SC_TOOLS)
tools/regression_spotcheck.sh # every fixed bug, ~90s
python3 tools/coverage_matrix.py
```

`SVTAV1_SC_TOOLS={nopalette,noibc,none}` forces a screen-content tool off at
runtime so you can bisect without editing and rebuilding. It deliberately does
NOT touch `allow_screen_content_tools` (the frame-header bit), so the streams
stay comparable — only the RD candidate set changes.

**Read the sizes it prints, not just the verdicts.** On `graph.png` at q32,
turning palette off moves the port FURTHER from C (3792 → 4186 against C's
3781). That is how "the port over-picks palette" was refuted for those cells:
the port's palette is winning real RD.

`SVT_CPU_FLAGS=<mask>` does the same job on the C side — it pins C's RTCD
dispatch level, which is how you test whether a divergence is C's own SIMD
choice (see `docs/SUSPECTED-C-BUGS.md` #9). `SVT_CPU_FLAGS=0` is pure-C kernels
and works on x86-64; it SEGFAULTS on aarch64, where Neon is mandatory.

## 5c. Cross-ISA questions need an emulator, not an argument

CI runs ONE architecture. Every cross-ISA question was therefore answered by
inference until 2026-08-05, and the inference had a hole big enough to matter:

> `tier_invariance.rs` walks the SIMD tiers present on the host it runs on. A
> difference that is uniform across tiers on EACH host and differs BETWEEN hosts
> — a per-ISA libm, a compile-time-selected kernel variant — is invisible to it.
> Tier-invariance within a host does not imply invariance across hosts.

Set up the local emulator once:

```bash
brew install qemu lima-additional-guestagents
colima start --profile x86 --arch x86_64 --cpu 4 --memory 6 --vm-type qemu
```

Then:

```bash
tools/fp_cross_isa.sh            # are the transcendentals bit-identical?
tools/cross_isa_port_check.sh    # does the PORT emit the same bytes on both?
```

The second needs no C oracle: to ask "is the PORT the variable side?" you only
need the port's own bytes on two ISAs. Run it whenever a pinned cell looks
host-dependent, BEFORE concluding anything about C.

**Three traps, each of which yields a confident wrong answer:**

- **LLVM constant-folds transcendentals.** With `-O` and loop-constant inputs it
  evaluates them at compile time with its own host-independent evaluator and
  never calls either libm — so a naive dump compares LLVM against itself and
  prints "identical" no matter what the libms do. `black_box` every input, and
  check the folded and unfolded runs agree before trusting a cross-host result.
- **musl is not glibc.** CI is Ubuntu. A musl container compares a libm CI never
  uses, and can report a difference that does not exist there (or miss one that
  does). The tools use `rust:1-slim` deliberately.
- **The emulated build SHARES `target/`.** No `--target` is passed, so it leaves
  an x86-64 ELF at `target/release/examples/identity_run`. Left alone the next
  gate silently runs a foreign binary. `cross_isa_port_check.sh` rebuilds
  natively at the end; if you build by hand, do the same. For the C library,
  mount the repo **read-only** and copy out — the host's
  `Bin/Release/libSvtAv1Enc.a` is aarch64 and load-bearing.

## 6. Refuse, never emit a plausible-but-wrong stream

Out-of-envelope configs return a typed `Err` from `encode_frame_impl`. They do
**not** encode. A wrong-pixels output indistinguishable from a correct one at
the integration seam is a shipping bug, not a known limitation.

Corollary the harness must respect: **a refusal is not a crash.** `identity_run`
exits **3** on a typed refusal, and gates count that separately.
`arbitrary_size_robustness.sh` once reported 48 correct refusals as PANIC — it
could not tell the port's best behaviour from its worst.

## 6b. A refusal is not a solution — check the ledger

```bash
tools/refusal_inventory.sh          # regenerate docs/REFUSED-CONFIGS.md
python3 tools/coverage_matrix.py    # what is COVERED
```

Refusing an out-of-envelope config beats emitting a wrong bitstream (§6). That
rule is right and it stays. Know its side effect:

- `arbitrary_size_robustness.sh` counts its **48 refusals as PASSES**, because
  refusing IS the correct behaviour. Nothing in that line separates "genuinely
  out of scope" from "nobody did the work".
- `coverage_matrix.py` prints `--` for an untested axis — but a REFUSED config
  produces no cell at all, so it cannot even show as `--`. The one tool built to
  surface gaps is blind to this one.
- Nothing ages a refusal. No owner, no expiry.

**Measured cost, 2026-08-04:** 10-bit at non-64-aligned dimensions — the actual
AVIF product case — sat behind `bit_depth_config_error` while every gate was
green. It was quoted in a status report the same day and moved past, because the
scoreboard said fine.

`docs/REFUSED-CONFIGS.md` splits refusals into **CONTRACT** (caller misuse,
permanent) and **CAPABILITY** (unimplemented — debt), and is CI-gated so the list
cannot accrete quietly. **Read the CAPABILITY table as a backlog.**

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

### Open, as of 2026-08-06

- **`tools/bd10_hbd_src_gate.sh` is RED on aarch64: 97/100.** Failing cells:
  `gradient_64_q8_p6`, `gradient_128_q8_p8`, `gradient_128_q20_p8`. Verified
  pre-existing (same three cells, same counts, on a clean tree). CI runs this
  gate unnarrowed on x86-64, so this is most likely **instance #7 of the
  cross-ISA class** in `docs/SUSPECTED-C-BUGS.md` #9 — but that has NOT been
  established, and §7's rule applies: do not file a possible port bug as an
  upstream quirk. Resolving it needs the documented protocol — extend
  `tier_invariance.rs` over these exact three cells first, then
  `tools/cross_isa_port_check.sh` (which needs the colima x86 VM from §5c).
  Until then the pins stay un-scoped and the gate stays red here.
  `STATUS.md`'s "Known failing test: (none)" line is about the TEST SUITE,
  which is green (1039/1039); it does not cover the shell gates.
- **The `no_std` configuration of `zenav1-svt-encoder` does not compile.**
  `cargo check -p zenav1-svt-encoder --no-default-features` fails on two ungated
  `std::env::var` calls in `pipeline.rs` (`SVTAV1_SC_TOOLS`). The crate's
  `#![cfg_attr(not(feature = "std"), no_std)]` and its ~40 feature gates are
  therefore aspirational, not a shipping configuration — and `just test-minimal`
  does not catch it, because `--no-default-features` still turns encoder `std`
  on through the `zenav1-svt -> zenav1-svt-encoder` edge. If you fix those two
  lines, you MUST also resolve `intrabc::libm_exp`'s `unimplemented!()` in the
  same change: it is reachable at screen content + qp >= 46 on presets M0..M4,
  so repairing the build would silently turn a low-quality screen-content
  encode into a panic. The stub carries this note too.
- **The perf gate ran for the first time since 2026-07-23** — it had been
  silently unrunnable (stale `-I` paths in `tools/perf_c_encode/build.sh` after
  the C tree moved into the `reference/svt-av1` submodule). Fixed, and the FIRST
  aarch64 port-vs-C numbers are in `benchmarks/perf_2026-08-06-arm64.meta`:
  slope-ratios 7.07x/7.95x/8.03x at p6/p10/p13, roughly 4x further from C than
  the x86-64 post-campaign figures, because the SIMD campaign was AVX2-only.
  Every other number in `docs/perf-status.md` is x86-64 and ~2 weeks stale.
- **Cancellation latency is measured now**, not assumed:
  `benchmarks/cancel_latency_2026-08-06.meta`. It meets a 20 ms bar at 64/256/
  1024 at every preset measured (worst p99 4.12 ms) and does NOT at 4096x4096
  (p99 18-28 ms), where the residual is frame-buffer teardown on the way out,
  not poll density — the success path pays the same 15-23 ms.

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
