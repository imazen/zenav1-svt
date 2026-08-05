# Suspected C bugs — the WTF list

Things in `reference/svt-av1` that look like upstream **defects**, not
intentional behaviour. Kept separate from the port maps for one reason:

> **A C bug is still the oracle.** Byte-identity means reproducing it. This file
> exists so that when you find the port doing something that looks insane, you
> can check whether it is *deliberately* insane before you "fix" it — and so
> that nobody files an upstream patch without first checking what it would do to
> our parity.

Every entry states: the C site, why it looks wrong, whether it is
**reachable** in the still-image/AVIF envelope, and what the port does about it.

**Status vocabulary**
- `REPRODUCED` — the port copies the behaviour bug-for-bug on purpose.
- `UNREACHABLE` — real in C, but no config this port accepts can reach it.
- `AVOIDED` — the port takes a different path that never hits it.
- `UNCONFIRMED` — looks wrong from reading; not yet proven by execution.

---

## 1. `variance_adjust_qp` at qp 0 makes mainline C internally inconsistent

**Status: UNREACHABLE (the port refuses qp 0) — but it poisons any future
lossless oracle capture.**

`rc_aq.c:454` (MAINLINE `svt_av1_variance_adjust_qp`) clamps every per-SB
qindex to `>= 1` (`:504`, `:539`) but `(void)`-ignores `readjust_base_q_idx`, so
`base_q_idx` stays 0. `md_config_process.c:1016` then derives
`coded_lossless = !base_q_idx` = 1. The encoder therefore **quantizes at
`blk_ptr->qindex >= 1`** (`product_coding_loop.c:10245`) while the frame header
signals `base_q_idx = 0`, no delta-q, and `CodedLossless` — encoder and decoder
disagree by construction.

Reachable in C via `--tune 3` (TUNE_IQ forces `enable_variance_boost = 1`,
`enc_handle.c:4901`) or any explicit `--enable-variance-boost`, at qp 0. Note
`--lossless 1` forces aq-mode off but **not** variance boost, so a fork-default
build hits it too.

**Consequence for us:** when lossless (`#4` in the backlog) is ported, any C
oracle captured at qp 0 **must** use mainline defaults with variance boost OFF,
or the reference itself is wrong. Write that into the lossless gate when it
lands.

---

## 2. `roi_map_apply_segmentation_based_quantization` falls through with a stale segment id

**Status: REPRODUCED (bug-for-bug), assert mirrored.**

`segmentation.c:121-129` walks `for (i = segment_id; i >= 0; i--)` and only
assigns `blk_ptr->segment_id = i` when `base_q_idx + ALT_Q > 0`. If **every**
candidate down to 0 is lossless it assigns nothing, leaving whatever
`blk_ptr->segment_id` happened to hold — a stale value from the previous block.
The trailing `assert` at `:131-133` catches it in a debug build; a **release**
build ships it.

The port takes the incoming id as a parameter and returns it on fall-through
(`segmentation.rs`), with a `debug_assert!` mirroring C's. So a debug build
stops where C stops, and a release build reproduces what C ships.

---

## 3. `svt_av1_wht_fwd_txfm` is not the WHT

**Status: AVOIDED (naming trap, not a behavioural bug).**

`transforms.c:4543`. Despite the name it hardcodes `tx_type = DCT_DCT,
lossless = 0` and is the TPL forward DCT. The actual Walsh-Hadamard transform
is `svt_av1_fwht4x4` (`transforms.c:3879`).

It is the first hit for anyone grepping `wht`, and it cost time once already.
The port's WHT kernels (`svtav1-dsp::fwd_txfm::fwht4x4` and the `iwht` pair)
cite the correct sites.

---

## 4. `exhaustive_mesh_search` tail-loop off-by-one

**Status: REPRODUCED bug-for-bug; UNCONFIRMED whether reachable.**

The tail loop bounds read `end_col - c` where every sibling loop uses
`+ 1`, so the last column of a mesh row is skipped. The port copies it
verbatim (`intrabc.rs`, with a `PORT-NOTE`) because byte-identity requires it,
but nobody has proven the case is reachable on the real `mesh_patterns` grids.

**If you are ever debugging an IntraBC DV that differs by exactly one mesh
column, start here.**

---

## 5. Temporal segmentation update is `SVT_ERROR` + `assert(0)`

**Status: UNREACHABLE, deliberately unported.**

`entropy_coding.c:4908-4912` and `:4917-4920`: both
`segmentation_temporal_update` branches are an error print and `assert(0)` with
the real work commented out. `svt_aom_setup_segmentation` (`segmentation.c:239`)
and `roi_map_setup_segmentation` (`:167`) both hardcode the flag to `false`.

C cannot reach an enabled temporal update. The port skips it and says so.
Porting it would mean inventing behaviour, which is worse than a gap.

---

## 6. `_c` and SIMD kernels genuinely disagree

**Status: REPRODUCED — port the one the ENCODER calls, not the `_c` twin.**

`svt_aom_hadamard_32x32_c` and `_avx2` produce different results at bd10
magnitudes (the `_c` version carries `int16_t` intermediates that wrap). Pinned
in `c_parity_hadamard.rs:236-262`.

The RTCD binds several kernels to their SIMD implementation, so **the `_c`
function is not always what runs.** `PORTING.md` says this; it is repeated here
because it is a live trap: a differential against `_c` can be green while the
encoder diverges.

Checked and found EQUIVALENT (so not a bug, but worth recording so nobody
re-derives it): `svt_av1_fwht4x4_c` vs `_sse4_1` agree over 20,000 random
full-`int16` blocks plus all 65,536 saturated corners; the only semantic
difference is `int64` vs `int32` lanes, and the measured peak intermediate is
2^19 — 4096x below `i32::MAX`.

---

## 7. `search_switchable` prices Wiener with a different window than `search_wiener_finish`

**Status: REPRODUCED (one half); UNREACHABLE (the other half).**

`restoration_pick.c:1154` prices Wiener with the *syntax* window (7-tap luma)
while `search_wiener_finish` (`:1276`) prices with the *search* window (5-tap).
A future SGR/`RESTORE_SWITCHABLE` port must reproduce **both**. The port already
reproduces the second deliberately; the first is unreachable because
`RESTORE_SWITCHABLE` needs SGR, which C only enables at `ENC_MR` — a preset the
port's `u8` cannot express.

---

## 8. `initial_display_delay_present_flag` is written unconditionally

**Status: UNREACHABLE (multi-frame is refused).**

`enc_handle.c:4981-4993` sets it to 1 always, then writes a per-operating-point
flag and an `f(4)`. Combined with the other malformed non-reduced-header fields
(spec-5.5.1 field order, an illegal 8-bit `refresh_frame_flags` on a *shown* key
frame, a missing `disable_frame_end_update_cdf`), the multi-frame header is a
mess — but it is C's mess, and reproducing it is what byte-identity would
require.

Moot for now: `encode_frame_impl` **refuses** inter frames outright, because the
port's own inter path emits a stream neither `aomdec` nor `dav1d` can decode
(measured). Revisit only if multi-frame is ever really ported.

---

## 9. C's encoded bitstream depends on the HOST ISA (bd10 screen, preset 7)

**Status: REPRODUCED (we match C-on-x86-64 and therefore differ from
C-on-aarch64). Pins are ISA-scoped as a result.**

The same input produces a DIFFERENT C bitstream on x86-64 than on aarch64:
`screen 64x64 q55 p7 bd10` is **119 bytes on x86-64 and 117 on aarch64**, and
`screen 128x128 q55 p7 bd10` is **356 vs 350**. Not a length-only difference in
principle — these are simply the two cells where it was caught.

**How we know it is C and not us.** `svtav1/tests/tier_invariance.rs` encodes
these exact cells under every archmage dispatch tier and asserts byte-identical
output. It is green, and the scalar tier is portable integer Rust, so
`port(aarch64) == port(scalar) == port(x86-64)`. The port emits 119 / 356
everywhere. `screen_palette_bd_gate.sh` matches on x86-64 and differs on
aarch64, so C must be emitting 119 / 356 on one host and 117 / 350 on the other.

**Why it is believable rather than surprising.** Entry #6 above: C's `_c` and
SIMD kernels genuinely disagree — `svt_aom_hadamard_32x32_c` vs `_avx2` at bd10
magnitudes, pinned in `c_parity_hadamard.rs`. Preset 7 runs the MDS0 Hadamard
fast loop, and these are bd10 cells. An RD comparison decided on a kernel whose
two implementations disagree will flip a mode somewhere, and a flipped mode is a
different bitstream.

C's aarch64 output does **not** vary within the architecture: `SVT_CPU_FLAGS=1`
(Neon only) and the default (all Neon extensions) give identical bytes. The
split is x86-64 vs aarch64.

**What this means for this port — the load-bearing part.** *"Byte-identical to
C" is a per-ISA claim wherever a bd10 RD path touches a disagreeing kernel.*
Consequences already applied:

- `screen_palette_bd_gate.sh` scopes those two pins with `uname -m`. A flat list
  cannot be right on both hosts: pinning unconditionally fails x86-64 (the
  pinned cell matches), not pinning fails aarch64 (the unpinned cell differs).
- A parity result should say which host produced it. A green sweep on one
  architecture is not automatically a green sweep on the other, and until this
  entry existed nothing in the repo said so.
- Do **not** "fix" the port toward C-on-aarch64. It already agrees with
  C-on-x86-64, which is the reference configuration the gates were built on, and
  chasing the other would just move the divergence to the other host.

`capture_c_trace` now takes `SVT_CPU_FLAGS` (default: unchanged library
behaviour) to pin C's RTCD level. `SVT_CPU_FLAGS=0` — pure C kernels, the
cleanest way to test this class — **segfaults on aarch64**, where Neon is
mandatory and zeroing the flags leaves null RTCD pointers. It works on x86-64.

**WIDER THAN FIRST THOUGHT (updated 2026-08-04).** A third cell turned up from
a completely different corner: `gradient 96x80 q48 preset 0`, **8-bit**,
synthetic gradient, partial-SB geometry. Byte-identical on aarch64
(C=217B port=217B) and failing on the x86-64 runner. The port is again not the
variable side — `tier_invariance.rs` covers that exact cell across every
dispatch tier and is green.

That matters because the tidy explanation no longer covers it. The first two
cells were bd10 / preset 7 / screen, which the `svt_aom_hadamard_32x32_c` vs
`_avx2` disagreement at bd10 magnitudes explains neatly. This one is bd8 /
preset 0 / gradient. Whatever the mechanism is, **it is not confined to bd10, to
screen content, or to the hadamard kernel** — so do not assume an 8-bit sweep is
architecture-independent either.

How it reached CI is itself the lesson: the cell was added by a run on the host
where it passes. **A gate cell validated on one architecture is a per-architecture
claim.** Cells now get `uname -m` scoping when they turn out to be one
(`partial_sb_gate.sh`, `screen_palette_bd_gate.sh`).

**How wide, measured rather than feared (2026-08-04).** The x86-64 CI run that
caught the third cell also ran the full default 8-bit gate — **1098 cells**
across every preset 0..13, the full qp range, four content classes, and the
dims tier including partial geometry — and PASSED it, with the identical 1098
passing locally on aarch64. Adding `partial_sb_gate.sh`'s 141 cells, that is
~1240 cells exercised on both architectures with **exactly one** disagreement.

So this is rare, not pervasive: it is not a reason to distrust the parity
gates wholesale. It IS a reason to treat any single new cell as provisional
until seen green on both, because the three known instances span bd8 and bd10,
presets 0 and 7, gradient and screen, aligned and partial — i.e. there is no
sub-domain you can declare safe in advance.

**INSTANCES 4-6 (2026-08-05), bd10 partial-SB gradient.** Three cells in
`bd10_partial_sb_gate.sh` match C on x86-64 and differ on aarch64:

| cell | on aarch64 |
|---|---|
| `gradient 48x48 q20 p9` bd10 | C=573 port=573 — same length, different bytes |
| `gradient 96x80 q20 p4` bd10 | C=1648 port=1647 |
| `gradient 65x65 q20 p2` bd10 | C=959 port=959 — same length |

The port is again not the variable side, measured on THESE EXACT CELLS rather
than their neighbourhood (`tier_invariance.rs::
bd10_partial_sb_pinned_cells_are_tier_invariant`). That distinction mattered:
the pins sit at presets 9/4/2 and a 48x48 geometry no surrounding sweep reaches,
so neighbourhood coverage would have proven nothing about them. bd10 at
partial-SB geometry had NO tier coverage at all before this — those configs
could not run until the 64-alignment refusal was lifted — which is exactly where
a port-side ISA dependence could have hidden.

**Revise the "rare, not pervasive" reading above accordingly.** Six instances
now span bd10/p7/screen, bd8/p0/gradient, and bd10/partial-SB/gradient at three
separate presets. The count is still small against ~1240 cells, but there is no
longer any sub-domain that can be assumed architecture-independent — not a bit
depth, not a preset band, not a content class, not a geometry. Every ISA-scoped
pin so far was found by CI going red, never by looking.

**PROVEN DIRECTLY 2026-08-05 — the port is ISA-invariant, measured, not
inferred.** Every earlier argument here rested on `tier_invariance.rs`, and that
argument had a HOLE worth naming because it looked airtight:
`for_each_token_permutation` walks the tiers present on the host it runs on, so
a difference that is uniform across tiers on each host and differs BETWEEN hosts
— a per-ISA libm, or a compile-time-selected kernel variant — is invisible to
it. "The scalar tier is portable integer Rust" collapses the moment a float
transcendental sits in a decision path, and there are 24 such sites.

Closed with a local emulated x86-64 Linux VM (colima + QEMU, glibc — the CI
runner's libc), no CI involved:

1. **Transcendentals: 402/402 bit-identical** (`tools/fp_cross_isa.sh`). Apple
   libm vs glibc, with `black_box` on every input — without it LLVM folds these
   at compile time with its own host-independent evaluator and the comparison is
   of LLVM against itself.
2. **The port's own bytes: identical on both ISAs.** Same source built for both,
   same cells, SHA-256 compared (`tools/cross_isa_port_check.sh`):

   | cell (bd10) | aarch64 | x86-64 |
   |---|---|---|
   | `48x48 q20 p9` | 573 B `6500a491aa0cd508` | 573 B `6500a491aa0cd508` |
   | `96x80 q20 p4` | 1647 B `f184844f744c2415` | 1647 B `f184844f744c2415` |
   | `65x65 q20 p2` | 959 B `3284cf968b415ef7` | 959 B `3284cf968b415ef7` |

   Those are exactly the three cells that byte-match C on x86-64 and differ here.
   The port emits the same stream on both; C does not. **C is the variable
   side** — no longer an inference.

This also rules out a compile-time-selected SIMD variant (magetypes uniform vs
optimized) for these cells: any such selection is byte-neutral, or the hashes
would differ.

3. **C's OWN bytes, measured on both ISAs — the last inferential step, closed.**
   Built `libSvtAv1Enc.a` + `capture_c_trace` inside the emulated x86-64 VM
   (repo mounted READ-ONLY, copied out to build, so the host's aarch64 lib was
   never touched) and ran the same cells on both:

   | cell (bd10) | C on aarch64 | C on x86-64 | port (both) |
   |---|---|---|---|
   | `48x48 q20 p9` | 573 B `fb8cd18f…` | 573 B `6500a491…` | 573 B `6500a491…` |
   | `96x80 q20 p4` | **1648 B** `3c20b288…` | **1647 B** `f184844f…` | 1647 B `f184844f…` |
   | `65x65 q20 p2` | 959 B `5218b9a6…` | 959 B `3284cf96…` | 959 B `3284cf96…` |
   | `96x80 q32 p6` **(control)** | 882 B `f449391c…` | 882 B `f449391c…` | 882 B `f449391c…` |

   C emits a DIFFERENT stream per architecture on all three. The port emits one
   stream on both, and it is C's x86-64 stream. The control matches everywhere,
   so this is not harness noise.

**What that means for the pins.** The ISA-scoped pins are pinning C's *aarch64*
behaviour, not a port gap. On the reference architecture the port already agrees
with C at these cells. Do NOT "fix" the port toward C-on-aarch64: it would break
agreement on x86-64, which is where the gates were built and where CI runs.

**Still open:** CI runs x86-64 only, so the aarch64 side is guarded only by
whoever happens to run the gates locally. The systematic fix is a second CI
runner building the C oracle on aarch64 and diffing the verdict sets; until
that exists, discoveries will keep arriving one cell at a time, the way this
one did.

---

## Adding an entry

State the C `file:line`, quote the code, say why it looks wrong, and — this is
the part that matters — say whether it is **reachable in our envelope** and what
the port does. An entry without a reachability verdict is a rumour, and rumours
are what this file exists to replace.

Do **not** file an upstream patch for anything here without first checking what
the fix would do to our byte-parity gates. We are downstream of the bug.
