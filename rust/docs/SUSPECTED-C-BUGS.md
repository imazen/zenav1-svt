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
reproduces the second deliberately; the first is unreachable **on the ALL-INTRA
path** because `RESTORE_SWITCHABLE` needs SGR, which
`svt_aom_get_sg_filter_level_allintra` (`enc_mode_config.c:1431`) enables only at
`ENC_MR` — a preset the port's `u8` cannot express.

**CORRECTION 2026-08-31 (wp-filters lane).** That "unreachable" verdict does NOT
hold in VIDEO mode. The level selector is `pd_process.c:4935-4938` —
`allintra ? _allintra : rtc_tune ? _rtc : _default` — and `scs->allintra` is set
only for `intra_period_length == 0 || avif` (`enc_handle.c:518`). A video-mode
frame therefore takes `svt_aom_get_sg_filter_level_default`
(`enc_mode_config.c:1402`), which returns **3 for `enc_mode <= ENC_M3`**, so SGR
is enabled, `rest_finish_search` sets `force_restore_type_d = RESTORE_TYPES`
(`restoration_pick.c:1566`) and `search_switchable` runs. Both halves of this
suspected C bug are reachable at presets 0..3 once the inter/video path is live,
and a `RESTORE_SWITCHABLE` port must reproduce both. The SGR FILTER chain is
ported (`svtav1-dsp/src/port_sgr/`, tier 1); the SEARCH side is not yet.

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

**HOW WIDE, RE-MEASURED 2026-08-28 — and it is MUCH wider at bd10 than the
paragraph above suggests.** The "~1240 cells, exactly one disagreement" figure
is an **8-bit** figure. Running the bd10 gates on macOS/clang/aarch64 against
the same commit CI passes on x86-64:

| gate | CI (x86-64, Linux/gcc) | local (aarch64, macOS/clang) |
|---|---|---|
| `bd10_matrix.sh` (uniform, synthetic) | 36/36 | 36/36 |
| `bd10_hbd_src_gate.sh` (synthetic, real 10-bit low bits) | 100/100 | 97/100 |
| `bd10_nonflat_gate.sh` (synthetic, non-flat) | **309/309** | **197/309** |
| `bd10_photo_gate.sh` (real photographic; NOT in CI) | — | **53/191** |

The port is not the variable side, checked two ways rather than assumed:
`tier_invariance.rs` holds its bytes constant across every archmage dispatch
tier, and a sample of the failing photographic cells (`1484678 q12 p10`,
`1001682 q12 p7`, `1001682 q12 p5`) was re-encoded by a build of the
PRE-SESSION tree (`bfae1b69`) in a sibling workspace — byte-identical port
output, still differing from local C. So the divergence tracks the C build, not
the port.

The pattern within it is informative: **flat and low-complexity synthetic
content agrees; non-flat and photographic content diverges.** That is the
signature of an RD comparison decided by a kernel whose implementations
disagree (entry #6), reached far more often once the residual has real
structure. `bd10_nonflat_gate.sh` and `bd10_photo_gate.sh` therefore carry NO
aarch64 pins here — 112 and 138 cells is not a pin list, it is a statement
about the oracle, and pinning them would bury it.

Two consequences already applied:

- `bd10_hbd_src_gate.sh` and `bd10_hbd_pq_gate.sh` carry `uname -m`-scoped
  pins for their specific cells (3 and 12), self-promoting as always.
- **A corpus-free PQ tier was added to `bd10_hbd_src_gate.sh`** precisely so
  the x86-64 reference host sees PQ-shaped 10-bit low bits at preset 6 — the
  band where the photographic PQ gate needs its pins. No CI runner has the
  image corpora (`ZENAV1_SKIP_CORPUS_TESTS` at workflow scope), so a
  photographic gate can never answer a question on the reference host; a
  synthetic one can. That tier is 18/18 on aarch64 too, which is what says the
  photographic p6 pins are about content, not about the low bits.

**The standing recommendation from this entry — an aarch64 CI runner — is now
quantified: it would move ~250 bd10 cells from "unknown on half our hardware"
to measured.** Until it exists, a bd10 parity claim MUST name its host.

## 10. The MD-side ext-tx CDF adapts an IntraBC block on the INTRA DC row

**Status: REPRODUCED (issue #16, 2026-08-27).**

`svt_av1_cost_coeffs_txb` (rd_cost.c) classifies the block for its tx-type
rate as

```c
const bool is_inter = is_inter_mode(cand_bf->cand->block_mi.mode);
```

— `use_intrabc` is not consulted — and passes that to
`av1_transform_type_rate_estimation` (rd_cost.c:107), which at
`allow_update_cdf = 1` does

```c
update_cdf(fc->intra_ext_tx_cdf[ext_tx_set][square_tx_size][intra_dir],
           av1_ext_tx_ind[tx_set_type][transform_type], av1_num_ext_tx_set[tx_set_type]);
```

with the INTRA `ext_tx_set` / `tx_set_type` and `intra_dir = mode = DC_PRED`
(an IntraBC candidate's `mode`). That is the encode pass (coding_loop.c:1539
→ `svt_aom_txb_estimate_coeff_bits(md_ctx, 1 /*allow_update_cdf*/,
&pcs->ec_ctx_array[sb_index], …)`), i.e. the MD-side per-SB context every
rate table is rebuilt from (`svt_aom_estimate_syntax_rate`,
enc_dec_process.c:2900). The bitstream writer, `av1_write_tx_type`
(entropy_coding.c:333-349), uses `is_inter = use_intrabc || is_inter_mode(mode)`
and codes the INTER row. So for every IntraBC luma txb with coefficients, C's
MD context adapts the intra DC row (with the intra set's symbol index — the
table's filler 0, i.e. DCT_DCT, for an inter-only type such as a flip) and
leaves the inter row alone; at 32x32+ the intra set is DCT-only and the MD
side adapts nothing at all where the writer adapts the 12-/2-type inter row.

Why it looks wrong: the same block adapts two different CDF rows depending on
which of C's two `is_inter` definitions you ask, and the MD rate tables
diverge from the stream's own statistics. It is harmless to the bitstream and
invisible to every byte gate until a near-tie lands on a DC / IntraBC
candidate.

**Reachable: yes** — any screen-content frame with IntraBC coefficients
(`sc_class5`, preset ≤ 4). Measured on `terminal` 188x256 p2 q55, mi=(50,42):
3 of 57 MDS1 costs (both DC-family candidates + one IntraBC candidate) were
103 rate units cheaper in the port with identical `ydist`, and every PFAST
signalling rate identical — the delta was the adapted DC row's DCT_DCT rate
(`benchmarks/issue16_mds1_txt_cdf_2026-08-27.md`).

What the port does: the READ half was already reproduced (`coeff_rate.rs`
`cost_dir` remap — an IntraBC candidate prices its tx type on the DC row, and
`MdRates::txt_rate` returns 0 for an out-of-intra-set type). The UPDATE half
is `CoeffFc::md_side_ibc_txt_update`: the pipeline's per-SB chain simulation
(its stand-in for `ec_ctx_array[sb]`) sets it, and `write_coeffs_txb_1d` then
routes an IntraBC txb through `md_update_tx_type_ibc_quirk` (intra set, DC
row, no write) instead of `write_tx_type_inter`. Real writers never set it.
Unit witness `md_side_ibc_tx_type_update_adapts_the_intra_dc_row_like_c`;
after the fix all 57 MDS1 costs at that block equal C's.

## 11. Every NEON `obmc_sub_pixel_variance` above 4x8 is the 4x8 kernel

**Status: AVOIDED (the port transcribes the `_c` kernel, which is correct) —
and it makes the C BINARY a non-oracle for one branch on aarch64.**

`Source/Lib/Codec/aom_dsp_rtcd.c:731-750` wires **every**
`svt_aom_obmc_sub_pixel_variance` size from `4x16` through `128x128` to
`svt_aom_obmc_sub_pixel_variance4x8_neon`:

```c
SET_NEON(svt_aom_obmc_sub_pixel_variance4x16, ..._c, svt_aom_obmc_sub_pixel_variance4x8_neon);
SET_NEON(svt_aom_obmc_sub_pixel_variance8x16, ..._c, svt_aom_obmc_sub_pixel_variance4x8_neon);
SET_NEON(svt_aom_obmc_sub_pixel_variance128x128, ..._c, svt_aom_obmc_sub_pixel_variance4x8_neon);
```

Only `4x4` and `4x8` get their own kernel. The two families immediately above
it in the same block — `obmc_sad` (`:706-727`) and `obmc_variance` (`:752+`) —
are wired correctly per size, which is what makes this read as a copy-paste
slip rather than a deliberate fallback. The x86 table (`:348-369`, `SET_SSE41`)
is correct.

**MEASURED on BOTH ISAs, 2026-08-31**, by counting `obmc_osvf_dispatch_control`'s
own classification over all 10 sizes x 64 offsets (640 cells):

| host | faithful | 4x8-aliased | sizes with a valid `osvf` oracle |
|---|---|---|---|
| aarch64-darwin | 128 | 512 | 4x4, 4x8 only |
| x86_64-linux   | 640 |   0 | all ten |

So the alias is exactly the NEON table and nothing else, and
`obmc_sub_pixel_tree_up_uas0_matches_c_where_the_dispatch_is_faithful` compares
**5x more sizes on x86 than on aarch64**. CI (`ubuntu-latest`) is the host that
covers that branch; a green run on the aarch64 dev box has proven it for two
sizes, not ten. Do not read an aarch64 green as coverage of that test's domain.

**MEASURED, 2026-08-31, macOS aarch64.** For `BLOCK_8X16` on random in-contract
input (`mask` in `[0, 4096]`, `wsrc` in `[0, 255*4096]`), across all 64
sub-pixel offsets, the RTCD kernel returns **exactly** what the `_c` **4x8**
kernel returns on the same pointers — e.g. at offset (0,0): RTCD `236041`,
`_c` 8x16 `960480`, `_c` 4x8 `236041`. Bit-identical to the 4x8 result at every
cell, which is the signature of the alias rather than a rounding difference.

**Reachable:** yes, on any aarch64 build with OBMC enabled.
`svt_av1_find_best_obmc_sub_pixel_tree_up` (av1me.c:878) calls `vfp->osvf` on
its `use_accurate_subpel_search == 0` branch, so the OBMC sub-pel MV of an
8x16 block is chosen by a 4x8 variance. In v4.2.0 the *only* call site
(`mode_decision.c:2148`) passes `USE_8_TAPS`, so the live encoder takes the
upsampled branch and never reaches `osvf` — the defect is latent there, but any
config or fork that sets `use_accurate_subpel_search = 0` walks into it.

**What the port does:** `inter_me::obmc_search` transcribes the `_c` kernel, so
it computes the size it was asked for. Consequence for testing:
`tests/c_parity_obmc_search.rs` compares the sub-pixel tree against the C binary
on the LIVE (`USE_8_TAPS`) path for every size, and on the `osvf` path only for
the sizes where `obmc_osvf_dispatch_control` proves this host's dispatch is
faithful — that control asserts, per (size, offset), either equality with `_c`
or exact equality with the `_c` 4x8 kernel, so it FAILS the day upstream fixes
the table and the exclusion has to be revisited.

## Harness note — `identity_diff.sh` can MIS-NAME the diverging field

Not a C bug; a trap in our own differ, recorded here because it cost time on
2026-08-28 and the file is where a reader looks for "why did the tool tell me
something wrong".

Chasing the tune-IQ residual (`gradient 128x128 q40 p6`, `SVT_TUNE=3` on both
encoders) the differ reported:

    STAGE: FH | cdef_damping_minus_3 C=2 Rust=3

The port's CDEF damping is **5**, measured by instrumenting the pipeline —
`3 + (base_q_idx >> 6)` with `base_q_idx = 160`, which is exactly C's
`CDEF_DAMPING_FROM_QP` (enc_cdef.c:1446) — and the writer emits
`damping - 3 = 2` (`entropy/obu.rs:1372`). So the port writes the SAME value
the differ says C wrote, and the named field is not the one that differs.

The raw bytes say what actually happened: the first differing byte is at
**offset 11**, `0x9e` vs `0x9d` — a single bit — and every byte after it
re-aligns immediately. A same-length field with a different VALUE, not a
length shift, and not the field the differ named.

**Rule: treat the differ's FIELD NAME as a hint and its STAGE as the fact.**
Before acting on a named field, confirm the value at the source (instrument
the producer, or diff the raw bytes and locate the bit). The differ walks the
header with its own model of the layout; when an earlier field's value moves a
later field's meaning, the name it prints can point at the wrong one.

## 12. `highbd_fwd_txfm_4x16_n2` / `_n4` call the UNPRUNED 4x16 transform

**Status: REPRODUCED (`fwd_txfm_pf::highbd_entry_shape`), gated at tier 1.**

`svt_av1_highbd_fwd_txfm_n2` / `_n4` fan out to 18 static per-size wrappers
each. Seventeen of the eighteen call their `_N2` / `_N4` twin. TX_4X16 does
not:

```c
static void highbd_fwd_txfm_4x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x16(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}
```

`transforms.c:4331` (`_n2`) and `:4101` (`_n4`) — both call
`svt_av1_fwd_txfm2d_4x16`, the FULL-shape entry, so a caller that asked for a
half- or quarter-shape block gets a fully populated 4x16 coefficient block
back. Every sibling, TX_16X4 included, calls `..._N2` / `..._N4`.

Found by extracting the callee of all 54 wrappers mechanically, not by reading
a few; a dispatch table written from the pattern gets it wrong.

**Reachability.** TPL never asks for TX_4X16 (its tx sizes come from
`src_ops_process.c:380-382`, min TX_16X4), so `svt_av1_wht_fwd_txfm` cannot
reach it. MD's PF path can, once `apply_pf_on_coeffs` arms on a 4x16 block.

**The port copies it**, in `highbd_entry_shape`
(`svtav1-dsp/src/fwd_txfm_pf.rs`), and
`tests/c_parity_txfm_pf_entry.rs::highbd_fwd_txfm_n2_parity` / `_n4_parity`
drive the real exported symbol at TX_4X16, so the behaviour is pinned rather
than described.

## 13. `svt_av1_fwd_txfm2d_*_neon` NULL-derefs at bd > 8 for any ADST-containing tx_type on a 32-dimension block

**Status: UNREACHABLE in a conformant stream — but it CRASHES the oracle, so
any differential over the entry points has to know about it.**

`ASM_NEON/highbd_fwd_txfm_neon.c:1851`:

```c
static const fwd_transform_1d_col_many_neon col_highbd_txfm32_xn_arr[TX_TYPES] = {
    highbd_fdct32_col_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    ...
```

`svt_av1_fwd_txfm2d_16x32_neon` (`:2069`) takes the `bd == 8` early-out into
the lbd kernel, and above it indexes that table by the whole 2-D `tx_type`.
So DCT_ADST on TX_16X32 at bd 10 fetches NULL and calls it — even though the
32-point dimension of DCT_ADST is a **DCT**; the table is keyed on the 2-D
type, not on the 1-D type of the 32-point side.

MEASURED 2026-08-31: calling `svt_av1_highbd_fwd_txfm(..., DCT_ADST, TX_16X32,
bd 10)` segfaults `libSvtAv1Enc.a` (aarch64, the in-tree Bin/Release build).

**Reachability: none.** No AV1 ext-tx set pairs an ADST with a 32 dimension —
a block that size takes `EXT_TX_SET_DCT_IDTX` — so the encoder never forms the
call. The `_c` implementations have no such hole and handle all 16 types.

**Consequence for tests, and it is the reason this entry exists.**
`tests/c_parity_txfm_pf_entry.rs` and `c_parity_estimate_transform.rs` drive
C through its RTCD dispatch, so they restrict the tx_type set per size to the
conformant one (`types_for`). The full 16 x 19 sweep still runs — against the
`_c` entries, in `c_parity_txfm_pf_2d.rs`. If you widen either RTCD-driven
test's type set, the test binary dies with SIGSEGV and no assertion message,
which reads like a harness failure rather than an out-of-envelope call.

## 16. `get_kf_q_tpl` / `get_gfu_q_tpl` can loop without bound and read out of bounds

`rc_vbr_cbr.c:1641` and `:1666`, the two reverse searches that map a target
active quality back to a qindex:

```c
int q              = rc->active_worst_quality;
int active_quality = get_active_quality(q, rc->kf_boost, ...);
int prev_dif = abs(target_active_quality - active_quality);
while (abs(target_active_quality - active_quality) > 4 &&
       abs(target_active_quality - active_quality) <= prev_dif) {
    if (active_quality > target_active_quality) { q--; } else { q++; }
    active_quality = get_active_quality(q, rc->kf_boost, ...);
}
```

`prev_dif` is computed ONCE, before the loop, and never updated inside it. So
the second clause is not "we stopped improving" — it is "still no worse than
the very first difference", which stays TRUE whenever the active quality stops
changing. It stops changing as soon as `q` leaves the minq tables' 0..=255
domain, because `get_active_quality` then keeps reading the same entry. `q` is
never clamped, and `get_active_quality` indexes `low_motion_minq[q]` /
`high_motion_minq[q]` directly. With a target the tables cannot reach
(`|target - active| > 4` at both ends of the ladder) the loop therefore runs
forever, indexing out of bounds on every iteration.

MEASURED 2026-08-31: the Rust port's first *faithful* transcription of this
loop — same conditions, same non-updating `prev_dif`, an index clamp instead of
an out-of-bounds read — panicked on `get_kf_q_tpl(start_q = 200, boost = 1000,
target = 100_000, bd 8)` with an integer overflow on `q` after ~4 s of walking.
That is the runaway, observed rather than argued.

**Reachability: none in our envelope, and not obviously reachable in C's.**
Both functions are `static` in `rc_vbr_cbr.c` and are only called from the
VBR/CBR qindex path (`svt_av1_rc_calc_qindex_rate_control`), which
`rc_cfg.mode == AOM_Q` skips entirely — and AOM_Q is what CQP/CRF sets
(`svt_aom_set_rc_param`, pass2_strategy.c:906). Inside C's own callers the
target is derived from the same tables, so it is reachable; whether any real
configuration produces an unreachable target has NOT been established here.

**What the port does:** `port_rc_vbr_cbr.rs`'s `reverse_active_quality` keeps
C's loop verbatim and adds exactly one exit — the walk stops at the edge of the
qindex domain. Inside the domain the two are identical; outside it C's
behaviour is an out-of-bounds read, so there is no defined value to reproduce.
`tests/rc_vbr_cbr_tables_and_scalars.rs::
reverse_search_stops_at_the_domain_edge_on_an_unreachable_target` pins the
boundary result, so removing the guard turns a hanging CI job into a red test.


## 17. `av1_rc_update_framerate`'s `(int)` cast is UB, and the two ISAs disagree

**Status: UNREACHABLE in C's own shipping envelope (`verify_settings` rejects
the input that triggers it) — but it makes the C BINARY a host-dependent
oracle for `svt_av1_new_framerate`, so a differential must not probe past the
envelope boundary.**

`pass2_strategy.c:887`:

```c
rc->avg_frame_bandwidth = (int)(scs->static_config.target_bit_rate / scs->new_framerate);
```

`target_bit_rate` is `uint32_t` and `new_framerate` a `double`, so the quotient
is a `double`. Converting a `double` whose value exceeds `INT_MAX` to `int` is
**undefined behaviour** (C17 6.3.1.4p1), and the two ISAs realize it
differently in hardware:

| host | instruction | result for 4e10 |
|---|---|---|
| x86-64 | `cvttsd2si` | `INT_MIN` (`-2147483648`, the "integer indefinite" value) |
| aarch64 | `fcvtzs` | `INT_MAX` (`2147483647`, saturating) |

**MEASURED 2026-08-31** via `c_parity_rc_process::new_framerate_matches_c` at
`(target_bit_rate = 4_000_000_000, framerate = 0.1)`: the C oracle returns
`-2147483648` on x86_64-linux and `2147483647` on aarch64-darwin. The port
returns `2147483647` on both — Rust's `as i32` is defined and saturating — so
it agrees with C on aarch64 and disagrees on x86. **No single port behaviour
can satisfy that cell on both hosts**; it is not a parity fact about the port.

**Reachable:** NO, not through C's own API. `svt_av1_verify_settings`
(`Globals/enc_settings.c:110`) rejects `target_bit_rate > 100000000`, so the
encoder refuses the config long before `av1_rc_update_framerate` runs. At the
envelope maximum (1e8 bits/s) the quotient overflows `int` only below
~0.047 fps, which `verify_settings` also does not accept. The UB is reachable
only by calling the internal function directly, which is exactly what a
differential shim does.

**What the port does:** saturates, deterministically and identically on every
ISA — the behaviour the port's cross-ISA invariant requires
(`tools/cross_isa_port_check.sh`). It does not reproduce either hardware
result, because reproducing one would break the other.

**What a test must do:** compare against C only for inputs inside C's stated
envelope (`target_bit_rate <= 100000000`). A cell above it is asserting the
host's floating-point conversion behaviour, not the port's fidelity.

**RESOLVED 2026-08-31 (`384e1cbd7`).** `c_parity_rc_process.rs` swept
`4_000_000_000` and therefore failed on x86-64 while passing on aarch64. The
sweep is now bounded at `100_000_000`, and — the part worth keeping — a
companion test drives the real `svt_av1_enc_set_parameter` /
`svt_av1_verify_settings` to PROVE the bound (1e8 accepted, 1e8+1 rejected,
4e9 rejected) rather than transcribing the constant, so a future change to the
cap fails loudly instead of silently widening every harness that trusts it.

Second-ISA verification, x86_64-unknown-linux-gnu (gcc) on `r7900x`: the
workspace was 2300/2301 at `c8758e359` with this the ONLY failure, and
2303/2303 at `b4b4b3161` after the bound landed. aarch64-darwin was green at
both. Recorded because "it is green now" without the before-number is not a
measurement.

**Two lanes reached this independently on the same day** — the rate-control
port lane hit it on a second-ISA run and filed a duplicate as #25, since
removed. That is the failure mode this file exists to prevent, and it only
worked in one direction: the duplicate was caught because the entry was
already here to collide with.

## 18. `derive_tf_window_params`' past-window compaction is unreachable dead code

**Status: REPRODUCED (the port keeps the translation, which is likewise never
entered)**

`Codec/pd_process.c` has two copies of the same block, at `3915-3920` (the
low-delay arm) and `4091-4098` (the random-access inter arm):

```c
int actual_past_pics = num_past_pics;      /* 3873, 4042 */
...
pcs->past_altref_nframes = actual_past_pics;
// adjust the temporal filtering pcs buffer to remove unused past pictures
if (actual_past_pics != num_past_pics) {
    pic_i = 0;
    while (pcs->temp_filt_pcs_list[pic_i] != NULL) {
        pcs->temp_filt_pcs_list[pic_i] =
            pcs->temp_filt_pcs_list[pic_i + num_past_pics - actual_past_pics];
        pic_i++;
    }
}
```

`actual_past_pics` is initialised to `num_past_pics` and **never modified** —
`grep -n actual_past_pics Codec/pd_process.c` returns exactly the two
initialisations, the two `past_altref_nframes` assignments and the two
comparisons, with no decrement anywhere. Only `actual_future_pics` is
incremented as pictures are found. So the guard is always false and the
compaction never runs.

The comment says what it was for: when `search_this_pic` fails to find some of
the requested past pictures, their slots stay NULL and the window has holes at
the FRONT (the found pictures sit at their natural indices, so the most recent
past pictures are at the higher ones). The compaction was meant to shift the
list left so the centre lands at `actual_past_pics`. As written, it cannot.

Note the second-order effect if it ever were enabled: the `while` loop starts
at index 0 and stops at the first NULL, which in exactly the case it is meant
to fix is index 0 — so it would still do nothing. Fixing the counter alone
would not fix the block.

**Reachable:** the BLOCK is unreachable. The situation it was written for IS
reachable — `avail_past_pictures` caps `num_past_pics` in the random-access
arm, but the low-delay arm has no such cap and its `search_this_pic` lookups
can miss at the start of a sequence.

**What the port does:** `port_picstruct::compact_tf_past_window` translates the
block faithfully, including the `while ... != NULL` bound, and
`traced_compact_tf_past_window` asserts BOTH that it is a no-op on a
front-holed list (matching C's loop bound) and that it shifts correctly on a
list with no leading hole. The caller must reproduce C's `actual_past_pics ==
num_past_pics` and therefore never invoke it; the translation is kept per
`docs/WORKING-ON-THIS.md` §7.

## 19. The `blend_a64_d16_mask` kernels have ISA-DEPENDENT numeric domains

**Status: IN-CONTRACT (not a defect) — but it makes the C binary a non-oracle
above the contract, and the two ISAs disagree about where that is.**

C's lowbd d16 mask blend takes its two sources as `CONV_BUF_TYPE` = `uint16_t`.
The `_c` kernel reads them unsigned (`(int32_t)src0[...]`, blend_a64_mask.c:56).
The x86 kernels do not:

```c
/* ASM_SSE4_1/blend_sse4.h:188, reached from svt_aom_lowbd_blend_a64_d16_mask_avx2 */
const __m128i res_a = _mm_madd_epi16(s0_s1, m_max_minus_m);
```

`_mm_madd_epi16` is a SIGNED 16-bit multiply-add, so an entry at or above 32768
is multiplied as a negative number. aarch64's kernel is unsigned end to end
(`vmull_u16` / `vmlal_u16` / `vqsubq_u16` / `vqrshrn_n_u16`,
blend_a64_mask_neon.c:208-225) and matches `_c` over the whole `uint16` domain.

**MEASURED 2026-08-31, both hosts, all-lanes-equal probe over `0..=65535`:**

| | first value where dispatched != `_c` | full block grid (308 cells) |
|---|---|---|
| aarch64 (NEON) | none in `u16` | 308/308 faithful at `< 40000` |
| x86-64 (AVX2 + SSE4.1) | **exactly 32768** | 0/308 at `< 40000`, **308/308 at `< 16384`** |

**Nothing in the encoder can reach it.** `svt_av1_jnt_convolve_2d_c` asserts
`0 <= sum && sum < (1 << (offset_bits + 2))` (inter_prediction.c:564) and stores
`ROUND_POWER_OF_TWO(sum, round_1)`, so an 8-bit compound CONV_BUF entry is
`< 1 << 14` = 16384 — half the signed limit. Driving C's own convolve over every
filter x subpel phase measures `[2919, 12159]` (the checkerboard source in
`compound_conv_buf_stays_inside_the_blend_domain`); the analytic tap-sign
extremum over the same grid is `[1012, 15356]`. Both are inside 16384.

**What this cost, and what the port does.** A differential generator that drew
CONV_BUF values `% 40000` put ~18% of them above 32767. The file's own C-vs-C
control then reported "C's dispatched d16 blend disagrees with its own `_c`
kernel on 20 of 20 cells" — **on x86 only**, while aarch64 stayed green, which
reads exactly like a C dispatch defect and is not one. The generator now draws
inside `conv_domain(bd)` (`c_parity_port_masked_blend.rs`), and
`c_lowbd_d16_blend_domain_covers_the_conv_buf_contract` MEASURES the first
divergence per host and asserts only that it clears the contract bound — so it
fires if either kernel's usable domain ever narrows below what the encoder can
feed it, and it records the host's number instead of leaving it to be
re-derived. The port's own kernel transcribes `_c` and is unsigned-correct over
all of `u16`; `d16_blend_matches_pure_c` sweeps it there.

**The general shape, worth carrying:** a scalar `_c` kernel is usually
DOMAIN-WIDER than the SIMD kernel the RTCD installs, and by different amounts on
different ISAs. A differential whose generator leaves the domain the encoder can
produce is testing nothing real, and the failure it reports is misattributed to
C. Bound the generator by what the producer can produce, and prove that bound by
driving the producer.

---

## 20. Every NEON `highbd_blend_a64_d16_mask` bit depth except 10 is the 8-bit kernel

**Status: UNREACHABLE (C rejects 12-bit) — and it makes the aarch64 C BINARY a
non-oracle for one branch, exactly like #11.**

```c
/* ASM_NEON/highbd_blend_a64_mask_neon.c:453-459 */
if (bd == 10) {
    highbd_10_blend_a64_d16_mask_neon(...);
} else {
    highbd_8_blend_a64_d16_mask_neon(...);
}
```

There is no 12-bit arm, so `bd == 12` silently blends and saturates at 8-bit
precision. The `_c` kernel has a real `switch (bd)` and saturates at 4095, and
the x86 AVX2/SSE4.1 kernel is faithful at 12.

**MEASURED 2026-08-31, both hosts:** the all-lanes-equal probe puts the aarch64
first divergence at conv value 6152 (8x8 and 16x8) / 8654 (4x4), and the full
308-cell grid at full-`u16` magnitudes is **0/308 faithful on aarch64 and
308/308 on x86-64**. bd 8 and bd 10 are 308/308 on both.

**Reachable:** no. `svt_av1_verify_settings` (enc_settings.c:460) rejects any
bit depth other than 8/10, so no configuration this port or C accepts reaches
the 12-bit arm.

**What the port does:** `port_masked_blend::highbd_blend_a64_d16_mask`
transcribes the `_c` `switch`, including its fall-through-to-255 default. The
dispatched differential (`build_masked_compound_no_round_hbd_matches_c`) sweeps
bd 8 and 10 only and says why; bd 12 coverage lives in the `_c`-driven cell,
which is green on both hosts. A note in that test previously attributed the
saturation to "the dispatched kernel" without an ISA — corrected in place.

---

## 21. The NEON highbd convolve kernels hardcode `ROUND0_BITS` and ignore `conv_params->round_0`

**MEASURED 2026-08-31, aarch64-darwin** (lane wx-interpred, while gating
`svt_inter_predictor_light_pd1`'s 10-bit arm).

`get_conv_params_no_round` (convolve.h:41) does not always leave `round_0` at
`ROUND0_BITS`. Its `intbufrange` correction fires at 12-bit
(`12 + 7 - 3 + 2 = 18 > 16`) and bumps `round_0` to **5**:

```c
const int32_t intbufrange = bd + FILTER_BITS - conv_params->round_0 + 2;
if (intbufrange > 16) {
    conv_params->round_0 += intbufrange - 16;
    if (!is_compound) conv_params->round_1 -= intbufrange - 16;
}
```

The scalar kernels read the corrected values back out of `conv_params`. The
NEON ones re-derive them from the compile-time macros instead:

```c
/* ASM_NEON/highbd_jnt_convolve_neon.c:1130-1137 */
const int offset_bits  = bd + 2 * FILTER_BITS - ROUND0_BITS;
const int round_offset = (1 << (offset_bits - COMPOUND_ROUND1_BITS)) +
    (1 << (offset_bits - COMPOUND_ROUND1_BITS - 1));
const int round_bits   = 2 * FILTER_BITS - ROUND0_BITS - COMPOUND_ROUND1_BITS;
```

At bd 12 that is `round_bits = 4` / `round_offset = 98304` where the `_c`
kernel uses `bits = 2` / `round_offset = 24576`. Worked on the first diverging
sample (packed value 884, `4x4`, `subpel (0,0)`, compound):

| kernel | closed form | result |
|---|---|--:|
| `svt_av1_highbd_jnt_convolve_2d_copy_c` | `884 << 2 + 24576` | **28112** |
| `..._2d_copy_neon` | `884 << 4 + 98304 = 112448`, stored to a `uint16_t` CONV_BUF | **46912** |

So the NEON value also WRAPS — the CONV_BUF is `uint16_t` and 112448 does not
fit.

**Extent, measured not inferred:** driving `svt_inter_predictor_light_pd1` at
bd 12 over `{4x4, 8x8, 16x8, 8x16, 32x32}` x four sub-pel corners x
`{single, compound}` puts C's dispatch at odds with C's own scalar composition
in **30 of 40 cells**. bd 10 is 240/240 in agreement, which is why nothing saw
this before.

**Reachable:** no. `svt_av1_verify_settings` (enc_settings.c:460) rejects any
bit depth other than 8/10, so neither C nor this port can configure bd 12.
Same reachability verdict, same shape, and the same ISA as #20.

**SECOND MEASUREMENT, same day — it is not only the `jnt_*` family.** The
SINGLE-prediction kernels do it too:

```c
/* ASM_NEON/highbd_convolve_neon.c:1003-1006, svt_av1_highbd_convolve_2d_sr_neon */
const int y_offset_bits = bd + 2 * FILTER_BITS - ROUND0_BITS;
const int y_offset      = (1 << (2 * FILTER_BITS - ROUND0_BITS - 1)) -
    (1 << (y_offset_bits - 1));
```

Driven through `tf_inter_predictor` at bd 12, an 8x8 block at `mv (3, 5)` (the
2-D kernel) comes back from C **all zeros** while the port produces real
samples — the wrong offsets push every result below zero and it saturates.
`mv (0, 0)` (the copy kernel, which reads no rounding at all) agrees at bd 12,
which is what localizes it to the offset derivation rather than to the
dispatch.

**What the port does:** `port_inter_predictor::inter_predictor_light_pd1_hbd`
composes `port_pack::pack_block` with the `_c`-gated kernels, so it follows the
SCALAR arm. `c_parity_port_light_pd1_hbd.rs` asserts exactly that at bd 12 and
only REPORTS the dispatch divergence — it never asserts the divergence exists,
because an x86 build may install a faithful kernel and an `assert_ne!` would
then fail on the wrong host. The tier-1 bd-10 sweep is the reachable evidence.

---

## 22. The two tx-bias distortion facades differ by ONE BRACE, so `tx_bias == 2` biases the spatial metric and not the frequency one

**Status: REPRODUCED (the port copies the asymmetry).**

`pic_operators.c` carries twin facades that wrap a distortion kernel in the
same set of tx-bias multipliers:

* `svt_aom_picture_full_distortion32_bits_single_facade` (`:163`) — the
  frequency-domain `[DIST_CALC_RESIDUAL, DIST_CALC_PREDICTION]` pair.
* `svt_spatial_full_distortion_kernel_facade` (`:252`) — the spatial SSE.

They are line-for-line identical apart from brace placement of the closing
`// Transform size related tweaks` block:

```c
/* 32-bit facade, :168-186 */                /* spatial facade, :265-393 */
if (tx_bias == 1) {                          if (tx_bias == 1) {
    if (is_intra_mode(mode)) { ... }             if (is_intra_mode(mode)) { ... }
    else if (is_inter_compound_mode(...)) {}     else if (is_inter_compound_mode(...)) {}
                                             }
    // Transform size related tweaks         // Transform size related tweaks
    if (tx_bias == 1 || tx_bias == 2) {      if (tx_bias == 1 || tx_bias == 2) {
        if (is_intra_mode(mode)) { ... }         if (is_intra_mode(mode)) { ... }
    }                                        }
}
```

In the 32-bit facade the tx-size block is INSIDE `if (tx_bias == 1)`; in the
spatial facade it is at function scope. Two consequences:

1. At `tx_bias == 2` the spatial facade applies the 64x64 `*3/2` bias and the
   32-bit facade applies **nothing at all** — the same block, the same
   candidate, two different distortions.
2. The 32-bit facade's own `if (tx_bias == 1 || tx_bias == 2)` is **dead**: it
   can only be reached when `tx_bias == 1`, so the `|| tx_bias == 2` disjunct
   never decides anything. Dead code that reads as live is the tell that the
   brace, not the condition, is the mistake.

**Reachable?** Yes wherever `tx_bias == 2` is signalled and both metrics are
consulted for the same candidate — the two arms then disagree by a factor of
3/2 on intra 64x64.

**What the port does:** `svtav1_encoder::dist_facade::picture_full_distortion32_facade`
returns the distortion unchanged for any `tx_bias != 1`, and
`spatial_full_distortion_facade` applies the tx-size block at every
`tx_bias` in `{1, 2}` — i.e. it reproduces both C functions exactly.
`tests/c_parity_dist_facade.rs` pins both against the real exported symbols
over every (mode, uv_mode) pair. The asymmetry was FOUND by that
differential, not by reading: the port initially shared one bias helper
between the two facades and diverged 3:2 on the first `tx_bias == 2`,
64x64, intra cell.

---

## 23. `svt_aom_is_screen_content`'s 8x8 pass measures a 16x16 variance

**Status: REPRODUCED (and UNREACHABLE in v4.2.0 — see below).**

`pic_analysis_process.c:1367` binds the variance kernel once:

```c
const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[BLOCK_16X16];
```

and never rebinds it. The function then runs two passes. The 16x16 pass is
consistent:

```c
int var = svt_av1_get_sby_perpixel_variance(fn_ptr, src, input_pic->y_stride, BLOCK_16X16);
```

The 8x8 pass changes `blk_w`/`blk_h` to 8 and the threshold to 16, but reuses
the same `fn_ptr`:

```c
blk_w = 8; blk_h = 8; var_thresh = 16;
...
int var = svt_av1_get_sby_perpixel_variance(fn_ptr, src, input_pic->y_stride, BLOCK_8X8);
```

`svt_av1_get_sby_perpixel_variance` calls `fn_ptr->vf` and then divides by
`eb_num_pels_log2_lookup[bs]`. So the 8x8 pass reads a **16x16** window through
`svt_aom_variance16x16` and normalises it by **64** instead of 256 — a
per-pixel variance four times too large, measured over a window four times too
big that straddles the block's right and lower neighbours. Two consequences:

1. The variance test at the 8x8 grid is not a property of the 8x8 block.
2. At the last 8x8 block of a row or column the read runs **8 rows and 8
   columns past the frame**, into the picture border. In the encoder that
   border exists, so it does not fault; it does mean the verdict depends on
   padding content.

The AA-aware detector (`svt_aom_is_screen_content_antialiasing_aware`) does NOT
have this bug — it binds `fn_ptr_16` and `fn_ptr_8` separately (:1253-1254).

**How it was found:** not by reading. The port was written first with matched
block sizes, and the tier-1 whole-detector differential
(`c_parity_sc_detect::is_screen_content_matches_c`) disagreed with C on
`sc_class4` for one mixed plane. Confirmed at tier 1 in isolation by
`sby_perpixel_variance_mixed_block_sizes_matches_c`, which drives the real
exported `svt_av1_get_sby_perpixel_variance` with `fn_bs = BLOCK_16X16` and
`norm_bs = BLOCK_8X8` and shows it differs from the well-formed 8x8 call.

**Reachable:** no. `screen_content_mode == 2` is the only selector for this
detector (pd_process.c:4783, pic_analysis_process.c:2018), and
`enc_handle.c:4638-4674` remaps the configured value before the encoder reads
it: `<= 1` passes through, anything else becomes **3** (at enc_mode <= M7/M8)
or **0**. The value 2 is never stored, so the live detector is always the
AA-aware one. The default of 2 in `enc_settings.c:1064` is remapped away.

**What the port does:** `sc_detect::is_screen_content` reproduces both the
16x16 window and the `>> 6` normalisation via
`sby_perpixel_variance_normalized(src, stride, 16, 8)`, and REQUIRES the caller
to supply the eight extra rows and columns C reads — it asserts on a
tightly-sized plane rather than reading whatever follows the slice. The
detector's second, inert quirk (`is_valid_palette_nb_colors(src, stride,
blk_w, blk_h, ...)` against parameters `(.., rows, cols, ..)`, i.e. width and
height swapped) is documented at the port site; both passes are square, so it
changes nothing.

---

## 24. `svt_av1_highbd_warp_affine_c` dereferences `ref2b` unconditionally

**MEASURED 2026-08-31, aarch64-darwin** (lane wx-interpred, while gating
`svt_aom_enc_make_inter_predictor`'s warp leaves).

The 10-bit warp kernel reads BOTH reference planes on every sample, with no
NULL check:

```c
/* warped_motion.c:773-775 */
uint16_t ref = (ref8b[iy * stride8b + sample_x] << 2) |
    ((ref2b[iy * stride2b + sample_x] >> 6) & 3);
```

`highbd_warp_plane` (:829) and `svt_av1_warp_plane` (:868) pass `ref_2b`
straight through, and so does `svt_aom_enc_make_inter_predictor`'s warp leaf
(:2540). So `is_wm && is16bit && src_ptr_2b == NULL` is not a different
answer — it is a NULL dereference. **MEASURED: a differential cell in that
configuration SIGSEGVs the test binary.**

The 8-bit twin `svt_av1_warp_affine_c` has one plane and cannot have the
problem, so this is specific to the high-bit-depth kernel.

**Reachable:** no. The three `svt_aom_enc_make_inter_predictor` call sites
that pass `NULL` for `src_ptr_2b` are `get_single_prediction_for_obmc_luma`
(:988) and `get_single_prediction_for_obmc_chroma` (:1052, :1090), all of
which pass `bit_depth = EB_EIGHT_BIT`, `is16bit = false` and `is_wm = false`.
Every 10-bit site passes both planes.

**What the port does:** `port_enc_make_pred`'s warp leaf accepts all three
`SrcPlanes` forms; `SrcPlanes::Hbd` (an unpacked plane, no 2-bit companion) is
a capability C does not have, so it has no oracle THROUGH THIS ENTRY. It is not
unverified arithmetic: `port_warp::highbd_warp_affine` over an unpacked plane
is tier-1 gated by `c_parity_warp_model.rs::highbd_warp_affine_matches_c`, and
`HbdWarpRef` makes both forms reach that one kernel. The differential here
therefore runs the two representations C can take (`Lbd`, `Split`) and says so
in `WARP_REPRS` rather than quietly omitting the third.

---

## Adding an entry

State the C `file:line`, quote the code, say why it looks wrong, and — this is
the part that matters — say whether it is **reachable in our envelope** and what
the port does. An entry without a reachability verdict is a rumour, and rumours
are what this file exists to replace.

Do **not** file an upstream patch for anything here without first checking what
the fix would do to our byte-parity gates. We are downstream of the bug.
