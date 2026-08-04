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

## Adding an entry

State the C `file:line`, quote the code, say why it looks wrong, and — this is
the part that matters — say whether it is **reachable in our envelope** and what
the port does. An entry without a reachability verdict is a rumour, and rumours
are what this file exists to replace.

Do **not** file an upstream patch for anything here without first checking what
the fix would do to our byte-parity gates. We are downstream of the bug.
