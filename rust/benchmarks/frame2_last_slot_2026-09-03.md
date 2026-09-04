# The frame-2 refusal named the wrong mechanism: the port predicted frame 2 from the KEY frame

`docs/INTER-ENCODE-PLAN.md` §1z²⁵ reinstated the frame-2 refusal keyed on
`pd0_detector`'s per-superblock `ref_obj_l0->sb_intra[sb_index]`, and quoted
**466 B against C's 21** as the cost of lifting it. This file records that the
466 B was a DIFFERENT defect, that the defect is fixed, and what the measured
first divergence at frame 2 actually is now.

Cells: `gradient 64x64 q32 p8` and `diag 64x64 q40 p8`, `frames=3`,
`SVTAV1_FRAME_SHIFT=3`, low-delay P. C oracle
`reference/svt-av1 @ fix/suspected-c-bug-17` through `tools/ctrace-linux/run.sh`
(linux/arm64 container). The refusal was lifted behind a throwaway env for the
measurement only; it is still in the tree.

## The defect: a hard-coded DPB SLOT where C resolves LAST

Three pipeline sites read the reference picture:

| site | what reads it |
|---|---|
| `ref_frame_data` | the open-loop ME's reference plane |
| `ref_padded_luma` | what motion COMPENSATION indexes (and `padded_by_ref`) |
| PD0's `ref_mins` | the reference's `sb_min_sq_size` for depth removal |

All three were `self.dpb.get(0)` — a hard-coded slot. C resolves LAST through
`pcs->ppcs->ref_pic_ptr_array[REF_LIST_0][0]`, i.e. the picture's own
`rps.ref_dpb_index[LAST]`.

**They agree on every frame this repo's gates cover.** Poc 1's LAST *is* slot
0. They diverge at poc 2, because frame 1 refreshes slot 1
(`refresh_frame_mask` 0x02, which §1z²⁵'s DPB fix made real): C's LAST is
slot 1 = poc 1, and `get(0)` is still the KEY frame — two displacements away
instead of one.

That is the **SEVENTH** "a caller passes a constant where the derivation is
already ported" finding of this campaign, after `dlf_level = 0`, PD0's `inter`
argument, `md_config.rs:948`, `was_intra: Some(1)`, `refresh_frame_flags: 0`
and `near_count_ctrls: Default::default()`.

## Measured, before and after

`SVTAV1_CANDDBG` at `mi=(0,0)` of frame 2, `gradient 64x64 q32 p8 frames=3` —
the port's own MD, BEFORE:

```
NSQDBG ICAND mi=(0,0) 64x64 mode=13 rf=1,-1 mv0=0,0   ... flr=2499
NSQDBG ICAND mi=(0,0) 64x64 mode=16 rf=5,-1 mv0=0,0   ... flr=3412
NSQDBG ICAND mi=(0,0) 64x64 mode=16 rf=1,-1 mv0=2,-36 ... flr=7267
```

`mv=(2,-36)` is 4.5 px of horizontal search against a picture 6 px away; the
true poc-1 displacement is `(0,-24)`. Frame 2 coded `intra=100` at **466 B**
against C's 21.

AFTER (`last_ref_slot`), the same frame's NEWMV is `(0,-24)` and:

| cell (frames=3) | C frame 2 | port BEFORE | port AFTER |
|---|---|---|---|
| `gradient 64x64 q32 p8` | 21 | 466 | **22** |
| `gradient 64x64 q40 p6` | 21 | — | **22** |
| `diag 64x64 q40 p8` | 21 | — | **21** |
| `uniform 64x64 q40 p6` | 21 | — | **21** |
| `screen 64x64 q40 p6` | 21 | — | **21** |
| `diag 72x72 q40 p8` | 27 | — | **26** |
| `diag 128x128 q40 p6` | 23 | — | **24** |
| `gradient 128x128 q40 p8` | 35 | — | **24** |

Frames 0 and 1 stay byte-IDENTICAL on every one of them.

**Byte-inert on the two-frame envelope, measured:** `inter_byte_matrix.sh` is
92 BOTH / 3 F1DIFF / 1 F0DIFF before and after, cell for cell identical.
That is not luck — at poc 1 the RPS's LAST *is* slot 0.

## The measured first divergence NOW: the temporal motion field

`tools/fh_fields.py` on frame 2 of `diag 64x64 q40 p8 frames=3` (C 21 B, port
21 B, first differing byte 15) reports every frame-header field identical up to
`cdef_damping_minus_3` (C 1, port 2).

**CORRECTED — that field is NOT "a CDEF search output downstream of the recon",
which is what this file first called it.** `CDEF_DAMPING_FROM_QP(160) = 5`, so
the field must be 2 on both sides; C's 1 is the low two bits of `0 - 3`, i.e.
C's frame 2 never ran CDEF at all. It is a picture-level CDEF-OFF gate keyed on
`ref_skip_percentage`, now wired — full account in
`benchmarks/frame2_cdef_skip_2026-09-03.md` and §1z²⁹. With it, NO frame-header
field differs on that cell.

And critically:

```
use_ref_frame_mvs                             1        1
```

**C's MFMV block is live at poc 2 and so is the port's** — but the port's
`tpl_mvs` are all `INVALID_MV`.

**CORRECTION, and it is this file's own first draft being wrong.** That draft
said `av1_setup_motion_field` / `motion_field_projection` "are unported". They
are not. `inter_mvp::{motion_field_projection, setup_motion_field}` are C's
`md_config_process.c:427/523` at tier 4 with traced vectors
(`tests/inter_mvp_motion_field.rs`), and `port_coding_loop::copy_frame_mvs` is
C's `av1_copy_frame_mvs` (`coding_loop.c:1038`) — whose module doc has said
since it landed that it is needed "the moment the GOP is three frames or
longer". Written before grepping, which is the exact failure
`docs/WORKING-ON-THIS.md` §4 names ("grep before you write the second"), and
caught by grepping afterwards.

What is missing is the STATE BETWEEN them, and only that:
`picture::ReferenceFrame` carries no per-8x8 `MV_REF` grid, nothing folds one
during the walk (C's `update_b` calls `av1_copy_frame_mvs` per coded block),
and `inter_mvp_env.tpl_mvs` is built as a constant all-`INVALID_MV` vector
instead of by `setup_motion_field`. So the frame-2 gap is a WIRE, not a port —
the same shape as the six constants this campaign has already found.

The join says so directly. C's `SVT_CINTER_OUT` at poc 2:

```
CINTER poc=2 mi=(0,0) bsize=12 part=0 mode=13 rf=1,-1 mv0=0,-24 pmv0=0,0 imc=8 skip=1
```

— `NEARESTMV` with `(0,-24)`, `imc=8`, i.e. **zero spatial matches**. The
port's `SVTAV1_CANDDBG` at the same block reports `refmvcnt=0 imc=8` and a
`NEARESTMV` of `(0,0)`. With no spatial neighbours, no compound, and identity
global motion, the ONLY source of C's `(0,-24)` is the temporal field.

**On the two-frame envelope the port is FAITHFUL here**, and not by accident:
C's own `motion_field_projection` returns 0 for a KEY-frame start frame
(`start_frame_buf->frame_type == KEY_FRAME`, `:441`), so the field is empty on
both sides.

## What this does and does not close

* **Closed:** the reference-picture selection. Frame 2 is now within a byte or
  two of C on every cell measured instead of 22x off.
* **Re-keyed, not lifted:** the refusal now names the temporal motion field,
  with this measurement behind it. It is not lifted, because none of the eight
  cells above is byte-identical.
* **Still open and still unported:** `part_arm::VideoPic`'s missing
  `InterOnInterRef` arm, which §1z²⁵ named. It is a real gap; it is simply not
  the FIRST divergence, and the DPB already carries the `sb_intra` / `sb_skip`
  it needs.
* **No byte cell can witness the fix**, because the refusal fires before frame
  2 is coded and lifting it needs an env var the crate resolves once per
  process. The premise is pinned instead by
  `pipeline::inter_decision_probe::last_is_not_dpb_slot_zero_from_poc_two`:
  LAST walks 0 -> 0 -> 1 while frame 1's `refresh_frame_mask` leaves slot 0
  holding the key frame — i.e. the two name DIFFERENT pictures from poc 2 on,
  which is what makes the constant a defect rather than a spelling.
