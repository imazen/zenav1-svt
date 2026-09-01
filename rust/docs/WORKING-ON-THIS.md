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

## 2b. Perf and memory — where they actually are

```bash
tools/perf_gate.sh          # port-vs-C wall clock, paired statistics
tools/mem_gate.sh 6         # peak RSS, port vs C, tiny -> large
tools/fp_cross_isa.sh       # transcendentals, this host vs emulated x86-64
```

**Wall clock (2026-08-13, aarch64):** port/C slope 3.77x at p2, 3.22x p6, 2.74x
p10, 2.73x p13 — and the port is FASTER than C below ~64 px on the fast presets
(0.86-0.90x fixed cost). `docs/perf-status.md` leads with the live table and a
SIMD-coverage queue ranked by measured frame share; read that before optimising
anything, because the top entries are already NEON and the queue is about
quality, not coverage.

**Per-gate wall clock (CI, x86-64 `ubuntu-latest`, measured 2026-08-27):**
`benchmarks/gate_wallclock_ci_2026-08-27.md` — every CI step's duration from
run 33101031800, so "how long does gate X take" has a measured answer. The
whole differential job is ~21 min; the three biggest single steps are the
SIMD tier-invariance suite (207 s), the workspace test suite (167 s) and the
C oracle build (141 s, cached since 2026-08-28). Local arm64 numbers differ;
measure with `time` and record the host.

**Memory (2026-08-16, the first ever measured):** port 3.5 MiB fixed +
~29-34 MiB/MP; C 7-9 MiB fixed + ~27-29. Half C's fixed cost, slightly more per
pixel, crossing near 1 MP; 122 vs 117 MiB at 4 MP.
`benchmarks/mem_2026-08-16.meta`. **Do not quote a MiB/MP figure at a size you
did not measure** — the slope moves with the range (33.6 over 64..1024, 29.2
over 1024..2048), which is the same never-extrapolate rule the wall-clock
harness follows.

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

**A parameter can be SHIFTED OUT of relevance before the code under test sees
it.** `svt_inter_predictor_light_pd1` calls `revert_scale_extra_bits`, which
shifts the sub-pel phases right by `SCALE_EXTRA_BITS` (6). The landed
`inter_predictor_light_pd1_8bit_matches_c` cell sweeps phases `(0,0)`, `(3,0)`,
`(0,9)`, `(15,15)` as raw Q4 values — **all four become (0,0)** after the
revert, so that sweep drives only the COPY corner of `svt_aom_convolve[][][]`
and reports four-corner coverage. Nothing in a pass/fail comparison can see it.
The fix is a positive control that asserts the phase SURVIVES the transform
(`c_parity_port_light_pd1_hbd.rs::the_four_dispatch_corners_are_actually_
reached`), and the same shape applies to any harness that feeds a value through
a normalising step before the code under test.

**A macro's name is not its arithmetic — check the definition before choosing
test values.** `ROUND_UV(x)` is `((x) >> 3) << 3` (definitions.h:348): a
multiple of **8**, not "an even chroma pair". A differential for the OBMC
chroma predictions used origins 3, 5 and 7, which ALL round to 0, so every
shift applied to them was inert and a `>> ss_x`-instead-of-`>> ss_y` mutation
passed the whole suite. Pick inputs the transform cannot collapse, and assert
that it does not collapse them.

**A parameter that is genuinely inert should be SAID to be inert, not swept.**
The OBMC single-prediction functions pass `is_compound = 0`, and the
single-prediction kernels never read `conv_params->dst` — so their CONV_BUF
stride cannot be observed through them at all (measured: changing it leaves
every cell green). Sweeping it anyway would have looked like coverage. The port
reproduces the value for faithfulness and its module doc says why no test can
see it.

**A harness PRECONDITION is a coverage hole.** `identity_run`'s `crop:` mode
rejects odd dimensions ("I420 needs even dims"), so no gate cell could ever
encode an odd-height frame of REAL content. That precondition hid a public-API
panic (`unsupported partition shape (Horz4, 3)`) on a shape only real content
picks, through every sweep in this repo. It was found by a test that builds its
own planes and therefore skips the check. When a harness refuses an input,
write down what that makes untestable — the refusal is not the same as the
input being impossible.

**A `c_parity_*` oracle can be correct by LINKER LUCK. Two of them were.**
Both halves of this were found on 2026-08-31 by running the suite on
x86_64-linux for the first time; both had been green on aarch64-darwin all day.

- **Some C entries derive an argument from a POINTER'S ADDRESS.**
  `svt_aom_convolve8_{horiz,vert}_c` take no phase index: they recover the
  16-phase table and the phase from the filter pointer itself
  (`convolve.c:54-61`, `get_filter_base` = `ptr & ~0xFF`, commented "this
  assumes that the filter table is 256-byte aligned"). Real call sites satisfy
  it with `DECLARE_ALIGNED(256, …)`; a shim that forwards a Rust
  `&[i16; 8]` straight through does not, and C then applies the taps at
  `addr - (addr % 16)`. The Rust `SUB_PEL_FILTERS_8` static landed at
  `%16 == 0` in the aarch64 test binary (oracle right, by accident) and
  `%16 == 8` in the x86 one (oracle silently wrong, whole-block value
  mismatch) — and the residue moves between binaries on ONE host too: three
  builds on the Mac gave `%16` of 8, 8 and 0. **Stage caller-supplied filter
  taps into `_Alignas(256) int16_t table[16][8]`, replicated into every row**
  (`ref_shims.c:1124` had this right; `inter_me_shims.c` regressed it).
  Pinned by `convolve8_oracle_is_alignment_invariant`, which feeds the same
  taps from every 2-byte residue in a 256-byte window.
- **An RTCD function pointer is NULL on x86 and does not exist on arm.**
  `svt_memcpy` is a pointer in `.bss` (`common_dsp_rtcd.h:1083`) until
  `svt_aom_setup_common_rtcd_internal` runs; the header even provides a
  null-safe `SVT_MEMCPY` for call sites that might run early, and
  `C_DEFAULT/variance.c:92` does not use it. Under NEON devirtualization
  `svt_memcpy` is `#define`d to the concrete `svt_memcpy_neon`
  (`common_dsp_rtcd_neon_devirt.h:266`), so on aarch64 there is no pointer to
  be NULL. A shim entry that skips RTCD setup therefore works on the Mac and
  lands at `rip = 0x0` on x86. **Every shim entry point calls its
  `*_ensure_rtcd()` first**, even when the function it wraps is a `_c` spelling
  — the `_c` body can still reach an RTCD pointer.

- **AVX-512 kernels use ALIGNED stores, and only x86 has AVX-512.**
  `svt_av1_fwd_txfm2d_*_avx512` write columns with `vmovdqa32` (64-byte
  aligned); the real encoder satisfies that because every residual/coefficient
  buffer is `EB_MALLOC_ALIGNED`. A Rust `Vec<i32>` is 4-byte aligned, so the
  store faults — `SIGSEGV` inside `av1_fdct64_new_avx512`, not a NULL
  dereference. `ref_shims.c:1315` had already documented and solved the AVX2
  (`_mm256_load_si256`, 32-byte) form of this for `ref_quantize_b`; the
  transform shims re-hit it one ISA wider. **Stage caller buffers through
  `_Alignas(64)` scratch**, and copy the OUTPUT buffer in as well as out when
  the test prefills it and asserts C leaves untouched positions alone.

- **Every shim is compiled into ONE archive, so a `ref_*` name is
  workspace-global — and a duplicate definition is not a link error
  everywhere.** Two byte-identical `ref_get_wedge_params_bits` (one in
  `inter_pred_shims.c`, one added later in `md_subpel_shims.c`) linked fine
  under Apple's `ld64`, which takes an archive's first definition, and were a
  hard `rust-lld: error: duplicate symbol` on x86_64-linux that took the WHOLE
  workspace down at link time. `grep -rn ref_<name> crates/svtav1-cref/shims/`
  before you add a wrapper; if it exists, declare it in your Rust module and
  call the one that is already there.

The general rule: **a differential passes on the host you ran it on. Nothing
more.** Before a `c_parity_*` file is quoted as tier-1 evidence, run it on the
other ISA — `ssh r7900x` is the x86 box — because the ways an oracle can be
accidentally right (static layout, per-ISA dispatch tables, devirtualized
symbols, an ISA that simply lacks the instruction that would have trapped)
are all invisible from inside one host.

**Measured on 2026-08-31**, the first day the suite was run on both: three
separate lanes landed shims that were green on aarch64-darwin and broken on
x86_64-linux the same day — 2 tests (obmc), 7 (entropy_inter), 9 (transforms),
18 in total, plus a duplicate-symbol link break, one instance of each trap above. Every one of them re-broke a
pattern `ref_shims.c` had already solved and commented. **Before you write a
new shim, grep `ref_shims.c` for the entry closest to yours** — the caller
contract you need is very likely already written down there.

**A differential's GENERATOR has a contract, and the `_c` kernel is usually
domain-wider than the SIMD one — by different amounts per ISA.** The masked
d16 blend's C-vs-C control reported "C's dispatched blend disagrees with its own
`_c` kernel on 20 of 20 cells" **on x86 and nowhere else**, which reads as an
RTCD defect and is not one: the generator drew CONV_BUF values `% 40000`, and
C's x86 kernels multiply through `_mm_madd_epi16` (SIGNED int16), so they leave
`_c` at exactly 32768 while aarch64's unsigned NEON kernel never does. The
encoder cannot produce such a value — `svt_av1_jnt_convolve_2d_c`'s own assert
bounds an 8-bit compound entry to `< 16384`, and driving that convolve measures
`[2919, 12159]`. **Bound a generator by what the PRODUCER can produce, and prove
the bound by driving the producer**, not by a comment about the range. Full
measurement in `docs/SUSPECTED-C-BUGS.md` #19 (and #20, the aarch64 highbd
kernel that takes the 8-bit arm for every bit depth except 10).

Corollary for cross-ISA work: **a green on the wider ISA is not evidence about
the narrower one.** The aarch64 pass here was structural — NEON is unsigned end
to end — and told you nothing about x86, exactly as #11's aarch64 obmc alias
tells you nothing about the x86 table.

**An exported `_c` symbol is NOT pure C, and one RTCD table is not both of
them.** `svt_av1_resize_plane_c` is exported and looks like the scalar
reference, but on x86-64 its leaves go through the RTCD pointers to the AVX2
kernels — which write a fixed-width block regardless of the requested length
and disagree with their own `_c` twins below it. On aarch64 the same source
line resolves to `_c`, because `aom_dsp_rtcd.c`'s AARCH64 arm is `SET_ONLY_C`
for every resize symbol. So an unpinned differential compares the port against
a DIFFERENT function on each host: three tests were green on aarch64 and two
SIGSEGV'd on x86-64 at the same commit. Separately, `resize_multistep`'s
identity fast path calls `svt_memcpy`, an RTCD pointer owned by
**common**_dsp_rtcd.c — a shim that inits only the aom_dsp table leaves it NULL
and every identity cell jumps to address 0. When a shim drives C, hand it the
contract the ENCODER hands it: **both** tables, and a deliberate decision about
which dispatch tier the oracle is supposed to be. Two rules follow. (1) When a
host passes, ask whether it passed structurally or by luck — here it was
structural twice over (`SET_ONLY_C`, plus NEON devirtualization making
`svt_memcpy` a direct call with no pointer to be NULL), which is checkable with
`nm -u` on the object file, not by reading the source. (2) `rip = 0x0` in a
backtrace is an uninitialised function pointer, not an overread — get the
backtrace before you theorise about buffer sizes. Full measurement:
`docs/SUSPECTED-C-BUGS.md` #26, pinned by
`crates/svtav1-dsp/tests/c_parity_resize_avx2_divergence.rs`.

**On macOS there is no arithmetic-coder op trace IN-PROCESS — run the C side in
a Linux container instead.** `capture_c_trace` needs `-Wl,--wrap`, which
Apple's `ld64` lacks, so `build.sh` falls back to a byte-only driver and
`identity_diff.sh` degrades to a byte + header-field comparison. Byte verdicts
are unaffected; symbol-level localization needs a GNU-ld host, and
`tools/ctrace-linux/` is one:

```bash
# one cell, real content, full op-trace diff (crop:/file:/raw: all work)
tools/ctrace-linux/diff_cell.sh 96 88 33 4 crop:/path/to/screenshot.png
# the VIDEO-mode sibling (low-delay-P GOP on both sides, frame 0 diffed;
# the port's inter refusal is expected, not a failure)
tools/ctrace-linux/vdiff_cell.sh 64 64 40 11 diag
# raw driver, drop-in for tools/capture_c_trace/capture_c_trace's argv
SVT_TRACE_OUT=~/tmp/zenav1-ctrace/c.trace \
  tools/ctrace-linux/run.sh 96 88 33 4 in.yuv out.obu 8
```

It bind-mounts the repo READ-ONLY and builds the C lib + wrap driver into a
docker volume, so it can never write into the tree (in particular it can never
leave a Linux ELF where the macOS `capture_c_trace` wrapper would exec it). The
`wrap_recon.c` dump vars are forwarded and their paths mapped:
`SVT_CTREE_OUT` (join against `SVTAV1_PACKTREE` with `tools/tree_diff.py`),
`SVT_QLEVELS_OUT` (+ `_XY`/`_COMP`), `SVT_PICKPART_OUT`, `SVT_CCOEF_OUT`,
`SVT_CCOST_OUT`, `SVT_PART_OUT`, `SVT_SEED_OUT`, `SVT_PD0COST_OUT` (+ `_SBY`)
and `SVT_PD0CFG_OUT`. The last two are the PD0 pair: `SVT_PD0CFG_OUT` prints
what `svt_aom_sig_deriv_enc_dec_pd0` RESOLVED for each superblock (level,
subres step, early-exit thresholds, rate-estimation level,
`pd0_use_src_samples`) and `SVT_PD0COST_OUT` prints C's per-block PD0 RD, which
joins field-for-field against the port's `SVTAV1_PD0DBG` `PD0BLK` line. Four
guesses at C's video PD0 were recorded in `INTER-ENCODE-PLAN.md` §1h before
anything observed that function; §1i is what one dump replaced them with.
Scratch must live under
`$CTRACE_WORK` (default `~/tmp/zenav1-ctrace`) — paths outside it are refused
rather than silently written where the host cannot see them.

**`run.sh` is a drop-in for `capture_c_trace`'s ARGV — and argv does not carry
the CONFIGURATION.** Until 2026-09-01 it forwarded the dump-path vars and
nothing else, so `SVT_FRAMES` / `SVT_AVIF` / `SVT_INTRA_PERIOD` /
`SVT_HIER_LEVELS` / `SVT_PRED_STRUCT` / `SVT_CPU_FLAGS` / `SVT_TILE_*` /
`SVT_TUNE` / `SVT_MAX_TX_SIZE` / `SVT_CRF_OFFSET` / `SVT_CSP` /
`SVT_SUPERRES_KF_DENOM` were dropped at the container boundary. A caller
asking for the inter campaign's VIDEO-mode 2-frame GOP got a container that
encoded ONE STILL frame: no error, a valid `.obu`, a valid op trace — of a
different encode than the one requested. So the only op-trace oracle a macOS
host has could not localize anything in the campaign that needed it, and would
have answered confidently if asked. They are forwarded now (`CONFIG_ENV` in
`run.sh`); keep that list in sync with `capture_c_trace.c`'s `getenv` calls.

**The C submodule is a SYMLINK in every `jj workspace add` sibling**, which is
the layout `CLAUDE.md` tells you to work in. A symlink resolves outside the
`/repo:ro` mount, so the container followed it into nothing and
`incontainer.sh` reported the submodule as uninitialised — a setup failure that
reads like a missing `git submodule update --init`. `run.sh` now mounts the
resolved directory over it when `pwd -P` lands outside the repo.

**`identity_diff.py`'s OP INDEX is not reliable on a video cell; its BYTE
verdict is.** Its alignment assumes one frame per trace and the still driver's
prologue, so on a two-frame run it names an op that did not diverge. Use
`tools/ctrace-linux/optrace_first_diff.py` (which `vdiff_cell.sh` runs for you)
for the localization: it splits both traces on `W RESET` and compares C frame 0
against the port's REAL PACK writer, because a run creates more writers than it
packs frames — MEASURED on `gradient 72x88 q40 p4`, where C has 2 segments and
the port has FIVE (the per-SB CDF-chain simulation and the tile re-walks each
have their own). Concatenating the port's segments reports a divergence at op 3
of a byte-IDENTICAL cell. It also normalizes C's `BOOL` / `BOOLEQ` spellings
against the port's 2-symbol `CDF` writes; without that, a raw diff of the two
traces disagrees on every literal bit. Its positive control is any
byte-identical video cell ("op streams identical").

When it names an op, grep the printed `icdf` value in
`crates/svtav1-encoder/src/entropy/default_cdfs.rs` — that names the CDF table,
and the table names the syntax element. That is how a `tx_size` symbol written
under TX_MODE_LARGEST was found (`docs/INTER-ENCODE-PLAN.md` §1j) on a cell
whose tree, every leaf field, every luma level and all three recon planes
already equalled C's.

**Verify the container oracle before trusting a trace from it.** Encode a cell
that ALREADY agrees on the host and confirm the container's C bytes are
identical; only then read the trace. Done for issue #15 on Linux arm64 vs
macOS arm64: identical on the diverging cell (`terminal` 96x88 p4 q33, 523 B)
and on an aligned control (`terminal` 64x64 p4 q33, 297 B, where port == C ==
container-C). Re-done 2026-09-01 for the VIDEO-mode path the config
passthrough opened up: `SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0
SVT_PRED_STRUCT=1 ./run.sh 64 64 40 6 rs.yuv c.obu 8` gives 961 + 22 B, and
its `c.obu.pts0` is BYTE-IDENTICAL to the host driver's — so `SVT_CTREE_OUT`
from the container is a trace of the same encode the byte gate compares. Build
the image for the SAME architecture as the host oracle (`run.sh` does) — C's
kernels are runtime-dispatched, so an x86 container is a different oracle, not
the same one.

## 5b. Drills you don't have to write

Localizing a divergence starts with narrowing WHAT changed, not reading code.
These are committed so nobody rebuilds them in a scratch dir:

```bash
tools/drill_two_images.sh     # per-preset/per-qp verdicts for the two open images
tools/sc_tool_bisect.sh       # palette? IntraBC? neither? (SVTAV1_SC_TOOLS)
tools/regression_spotcheck.sh # every fixed bug, ~90s
python3 tools/coverage_matrix.py
```

The VIDEO-mode key frame's tree diff, which is the inter campaign's inner loop
and now runs on macOS (the container gained the config passthrough on
2026-09-01, §5 above):

```bash
W=~/tmp/zenav1-ctrace/refcell; mkdir -p $W
SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
  SVTAV1_PACKTREE=$W/rs.tree tools/identity_run gradient 64 64 40 6 $W/rs
SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
  SVT_CTREE_OUT=$W/c.tree tools/ctrace-linux/run.sh 64 64 40 6 $W/rs.yuv $W/c.obu 8
head -14 $W/c.tree > $W/c.f0.tree    # BOTH dumps append across frames (§5)
python3 tools/tree_diff.py $W/c.f0.tree $W/rs.tree
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

## 5d. Scripting a file split — three traps, all measured

`leaf_funnel.rs` (11,247 lines) became `leaf_funnel/` on 2026-08-16, byte-neutral
at 1100/1100. If you split another mega-file, the compiler catches everything —
but only after you avoid these, each of which cost a rebuild cycle:

- **A line regex cannot tell a struct field from a function parameter.** Bumping
  `^    name: type,` to `pub(super)` also hit multi-line fn parameter lists: 744
  errors. Parameters live inside `(`, fields inside `{`; only brace/paren depth
  tracking distinguishes them.
- **Locating sections by title text matches PROSE.** "The funnel" appears at
  line 424 as well as its banner at 2852, so taking the first hit mis-sliced
  every section and one module came out 9 lines long. Require the preceding line
  to be a `// ----` rule.
- **A glob re-export CAPS visibility.** `pub(crate) use m::*;` silently demoted a
  genuinely-`pub` item and broke two integration tests; a blanket `pub use` then
  warned on every module exporting nothing public. Use crate-scoped globs for
  internals plus explicit `pub use` for the few real public items.

File-private becomes `pub(super)` — the same scope, now that the "file" is a
module tree. And the acceptance test is byte-identity, not a reading of the diff.

**Pre-split line numbers.** Docs written before 2026-08-16 cite
`leaf_funnel.rs:LINE`. Those numbers are stale for anything that moved into
`tx_pipeline` / `rate_tables` / `predict` / `coeff_rate`. **Re-locate by symbol
— every name is unchanged.** Do not chase the numbers.

**A control that produces NO change is only evidence once you have separately
shown the code is REACHED.** Measured 2026-08-31 while checking that a new line
in `avg_cdf_with` was byte-neutral: the verdict was 32/32 cells identical, and
the positive control — perturbing `skip_cdf[0][0]` by -2000 in the same
function — ALSO changed no byte. That reads exactly like "the function is never
called", which would have made the 32/32 vacuous, and it was one step from
being recorded that way.

It was reached. An `eprintln!` probe fired **twice per frame** at presets 0/4/6
and **zero** times at preset 8 — and 2 is what the geometry predicts (64x64 SBs
make 192x160 a 3x3 grid; the call site needs `left_avail && topright_avail`,
i.e. `col == 1, row in {1,2}`). Zero at p8 matches `funnel_chain = use_funnel
&& preset in 0..=6 && multi_sb` (pipeline.rs). A stronger control (halving
`partition_cdf` at the same site) then moved 12/12 cells.

So the weak control's silence meant *"this perturbation flipped no RD
decision"*, not *"this code did not run"* — two readings a byte diff cannot
tell apart. **Count the calls; do not infer reachability from a byte diff.**
This bites hardest on preset- and geometry-gated paths: a grid that misses
`multi_sb`, or sits at preset >= 7, exercises none of the funnel chain.
Record: `benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md`.

**`cargo build -p <crate>` hides test-target breakage; build `--all-targets`.**
On 2026-08-31 a field was added to `SeqTools` and a `#[cfg(test)]` literal in
`entropy/obu.rs` was not updated. The lib built clean, so the author saw
nothing; CI's `Workspace tests` step failed to COMPILE, and because that is
step 12, steps 13-25 were **skipped** — decode conformance, bd10 identity, SIMD
tier invariance, the spot-check and the 8-bit all-preset sweep never ran, for
that commit or for the nine others from three lanes that inherited the same
parent. A compile error in one lane silently erases every gate's evidence for
everyone, so a skipped gate reads as "no result", never as "pass".

**A shim may only reference a symbol `nm -g` shows in the archive — and
`objcopy --globalize-symbol` exits 0 when it matches NOTHING.** Three lanes hit
this on 2026-08-31 and it took `main` red on Linux twice, invisibly to every
aarch64 developer.

Several `static` C functions are promoted to linkable symbols by
`crates/svtav1-cref/build.rs` (`llvm-objcopy --globalize-symbol` on a private
copy of the object) so they can be reached at evidence tier 1. That mechanism
is sound. What was not: success was read from objcopy's exit status.

**GCC renames statics.** Its interprocedural passes emit `.isra.N`
(scalar replacement), `.constprop.N`, `.part.N` and `.cold` clones, and may
eliminate a function outright. Measured, same source, two hosts:

| C symbol | clang / macOS | gcc / Linux |
|---|---|---|
| `clamp_qindex` | `_clamp_qindex` | `clamp_qindex.isra.0` |
| `aom_ssim2` | `_aom_ssim2` | `aom_ssim2.part.0` |
| `get_regulated_q_overshoot` | present | absent entirely |

So `--globalize-symbol=clamp_qindex` matched nothing, exited 0, the cfg and the
shim's `#ifdef` define both switched on, and the link failed with
`undefined symbol`. On macOS the plain names survive, so it linked and the
breakage was invisible on the host every lane develops on.

Rules, if you add a promotion site:
- Verify the RESULT, never the exit code — `globalized_symbols_present()` runs
  `nm -g` on the promoted object and requires each name to be global, matching
  the WHOLE name so `clamp_qindex.isra.0` does not satisfy `clamp_qindex`.
- Guard the shim wrappers on the matching `SVTAV1_CREF_*` define, so a failed
  promotion means the C side does not reference the symbol at all. Those
  functions then fall back to tier 4, and `SVT_CREF_REQUIRE_*_STATICS=1` turns
  that skip into a loud failure for a caller who requires it.
- Before writing any shim at all, `nm -g` the archive **on both hosts**. A
  symbol `nm` reports as `t`/`b` (local) is not linkable, and one that is `T`
  on your host may be renamed on the other.

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

## 9. Where the bodies are

| you want | look at |
|---|---|
| what is byte-identical, and where it is not | `STATUS.md` |
| coverage per preset per axis | `python3 tools/coverage_matrix.py` |
| every bug we have fixed, with its reproducer | `tools/regression_spotcheck.sh` |
| C code that looks broken | `docs/SUSPECTED-C-BUGS.md` |
| which C file a Rust module ports | `../PORTING.md` |
| the leaf funnel (SPLIT 2026-08-16) | `leaf_funnel/{mod,tx_pipeline,rate_tables,predict,coeff_rate}.rs` |
| perf + memory | `docs/perf-status.md`, `benchmarks/mem_2026-08-16.meta` |
| the working agreement + envelope guards | `CLAUDE.md` |
| per-feature plans and open chunks | `docs/*-port-map.md` |
| why a `product_coding_loop.c` row reads MISSING | `docs/pcl-md-port-map.md` |
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
