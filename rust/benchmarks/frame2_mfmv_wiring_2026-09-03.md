# The temporal motion field was PORTED and never WIRED — frame 2's byte count now matches C on 6 of 8 cells

`benchmarks/frame2_last_slot_2026-09-03.md` localized the frame-2 residual to
the temporal motion field, and its first draft said that field "is unported".
**It is not, and grepping instead of asserting is the finding here.** Three
pieces were already in tree, tested, and called by nothing:

| C | line | port | tier |
|---|---|---|---|
| `av1_copy_frame_mvs` | `coding_loop.c:1038` | `port_coding_loop::copy_frame_mvs` | 4 |
| `motion_field_projection` | `md_config_process.c:427` | `inter_mvp::motion_field_projection` | 4 |
| `av1_setup_motion_field` | `md_config_process.c:523` | `inter_mvp::setup_motion_field` | 4 |

`port_coding_loop`'s own module doc had said since it landed that its absence
means "every frame from the SECOND inter frame onward gets wrong TMVP
candidates ... needed the moment the GOP is three frames or longer". Nothing
called it.

What did not exist was the STATE between them. This chunk wired exactly that.

## What landed

* `picture::ReferenceFrame` gains `mvs` (C `EbReferenceObject::mvs`, the
  per-8x8 `MV_REF` grid) and `ref_order_hint[7]` (this picture's own
  references' order hints, which the projection reads to scale a saved MV).
* `CodedAreaAcc` — already the port's `update_b` — gains the MFMV writeback
  beside the three coded areas, under C's own gate
  (`scs->mfmv_enabled && slice_type != I_SLICE && ppcs->is_ref`,
  `coding_loop.c:1748`). Same placement argument as the areas: `update_b` is
  one C function and this is one port of it.
* `pipeline.rs` calls `setup_motion_field` once per picture where it built the
  all-`INVALID_MV` `tpl_mvs` constant, and carries BOTH of its products — the
  field to the MVP scan, `ref_frame_side` to the walk's `copy_frame_mvs`. C
  derives both in one function; so does this.

## Measured: the temporal MV reaches the ref-MV stack

`SVTAV1_CANDDBG` at frame 2's block, `diag 64x64 q40 p8 frames=3`, against C's
`SVT_CINTER_OUT`:

| | mode | mv | `refmvcnt` |
|---|---|---|---|
| C (coded) | 13 `NEARESTMV` | **(0,-24)** | 0 spatial matches, `imc=8` |
| port BEFORE | 13 `NEARESTMV` | (0,0) | **0** |
| port AFTER | 13 `NEARESTMV` | **(0,-24)** | **1** |

The stack goes from empty to holding C's own temporal candidate.

## The frames=3 frontier, before and after

C / port, frame 2 (frames 0 and 1 byte-IDENTICAL on every cell):

| cell | C | after LAST fix | after MFMV wiring |
|---|---|---|---|
| `gradient 64x64 q32 p8` | 21 | 22 | **21** |
| `gradient 64x64 q40 p6` | 21 | 22 | **21** |
| `diag 64x64 q40 p8` | 21 | 21 | 21 |
| `uniform 64x64 q40 p6` | 21 | 21 | 21 |
| `screen 64x64 q40 p6` | 21 | 21 | 21 |
| `diag 128x128 q40 p6` | 23 | 24 | **23** |
| `diag 72x72 q40 p8` | 27 | 26 | 26 |
| `gradient 128x128 q40 p8` | 35 | 24 | 23 |

Six of eight now match C's byte COUNT. **None is byte-identical**, so the
frame-2 refusal STAYS. On `diag 64x64 q40 p8` the first diverging
frame-header field is `cdef_damping_minus_3` (C 1, port 2) — a CDEF SEARCH
output, i.e. downstream of the recon; on `diag 128x128 q40 p6` and
`gradient 64x64 q40 p6` no frame-header field differs at all and the whole
divergence is in the tile payload.

## Byte-inert on the two-frame envelope, and not by luck

`inter_byte_matrix.sh` is **92 BOTH / 3 F1DIFF / 1 F0DIFF before and after,
cell for cell identical**. The reason is structural: frame 1's LAST is the KEY
frame, and C's own `motion_field_projection` returns 0 for a key-frame start
frame (`start_frame_buf->frame_type == KEY_FRAME`, `md_config_process.c:441`),
so every cell of `tpl_mvs` stays `INVALID_MV` on both sides.

## Two defects this chunk introduced and its own mutation test found

1. **A short slice, not an empty field.** `mvs` was first allocated only when
   the writeback gate was on. C allocates `EbReferenceObject::mvs` on every
   reference object and simply does not WRITE it when the gate is false,
   leaving zeros that the projection skips. Forcing `mfmv_active` false
   panicked `inter_mvp.rs:2266` on both `refuses_inter3` cells — the
   projection indexes before anything can tell it the slice is short. Now
   allocated unconditionally, at C's reset value (`NONE_FRAME`, which like
   C's zero is `<= INTRA_FRAME`), plus a length check at the wire.
2. **The gate's observable is the NAMED count, not the length.** The first
   spot-check asserted the key frame's field was zero-LENGTH, which the fix
   above makes false. It asserts zero NAMED cells now, which is C's gate.

## The cells that guard it

`mfmvField` in `tools/regression_spotcheck.sh`, two cells (`gradient 64x64
q32 p8` one superblock / whole-frame block, `diag 128x128 q40 p6` four
superblocks and a real tree). They read the census the port prints beside the
coded-area statistics — `PORTREFSTATS ... mfmv=<named>/<len>` — and assert the
inter frame's field is full and every cell names a reference while the key
frame's names none. **This wire has NO byte observable**: its only consumer is
frame 2, which the port still refuses, so without a census a wire nothing can
see would rot. Mutation-verified: forcing the gate false reports
`inter frame's motion field is 0/64` and `0/256` and nothing else fails.
