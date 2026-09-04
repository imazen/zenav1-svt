# C's frame-2 header codes the low two bits of MINUS THREE, and the port was chasing the wrong field

`benchmarks/frame2_last_slot_2026-09-03.md` reported that on
`diag 64x64 q40 p8 frames=3` the first diverging frame-2 header field is
`cdef_damping_minus_3` (C 1, port 2), and called it "a CDEF SEARCH output,
i.e. downstream of the recon". **That reading was wrong**, and the value is
the whole finding.

## The arithmetic that says so

C writes `cdef_damping - 3` as a 2-bit literal (`entropy_coding.c:2349`), and
`CDEF_DAMPING_FROM_QP(base_q_idx) = 3 + (base_q_idx >> 6)` (`enc_cdef.c:895`).
Both sides' `base_q_idx` is 160 — identical in the header — so the field MUST
be `3 + 2 - 3 = 2`. C wrote **1**.

There is exactly one way to get 1: `cdef_damping` is still its
`resource_coordination_process.c:423` initialiser **0**, and `0 - 3 = -3`
written as two bits is `0b01`. `cdef_damping` is only ever assigned inside
`finish_cdef_search` / `svt_pick_cdef_from_qp` (`enc_cdef.c:923/937/1123`), so
**C's frame 2 never ran CDEF at all** — `cdef_process.c:682` takes its `else`
arm, which zeroes `cdef_bits` and both strengths and leaves the damping alone.

So the divergence is a PICTURE-LEVEL CDEF-OFF decision, not a recon defect, and
the port has to reproduce a C quirk (coding the low two bits of a negative
number) to match it. `crate::cdef::CdefFrameParams::never_picked()` already
existed for exactly that; nothing reached it.

## Why C turns CDEF off at frame 2 and not at frame 1

`md_config_process.c:980-985`, and the ORDER is an `else if`:

```c
if (me_based_cdef_skip(pcs) || (cdef_ctrls->skip_th && skip_perc >= cdef_skip_th) ||
    (scs->vq_ctrls.sharpness_ctrls.cdef && pcs->ppcs->is_noise_level)) {
    pcs->ppcs->cdef_level = 0;
} else if (cdef_ctrls->use_reference_cdef_fs || cdef_ctrls->search_best_ref_fs) {
    update_cdef_filters_on_ref_info(pcs);
}
```

with `cdef_skip_th = CLIP3(25, 100, skip_th + (base_q_idx - 128) / 4)` (`:973`).

At preset 8 the CDEF search level is 7, whose controls give
`skip_th = is_base ? 0 : 80`; frames 1 and 2 of a flat low-delay-P GOP are both
non-base, so `skip_th` is 80 and the threshold is `80 + (160-128)/4 = 88`.
`skip_perc` is `pcs->ref_skip_percentage`:

| frame | list-0 reference | `ref_skip_percentage` | 100-scale gate | CDEF |
|---|---|---|---|---|
| 1 | the KEY frame (I_SLICE) | **0** | `0 >= 88` false | ON |
| 2 | frame 1, a 22-byte ALL-SKIP frame | **100** | `100 >= 88` TRUE | **OFF** |

C's own `SVT_REFSTATS_OUT` at poc 2 reports `refskip=100`, and the port writes
exactly that onto frame 1's DPB entry (verified in §1z²⁵). So the input was
already there; only the gate was missing.

The port ran `update_cdef_filters_on_ref_info` UNCONDITIONALLY when the level
asked for a reference-derived candidate set, i.e. it took C's `else` arm
without testing C's `if`.

## What landed

`port_enc_mode_config::cdef_search::cdef_skip_gate(skip_th, base_q_idx,
ref_skip_percentage)` — C's two lines, with four tier-4 unit tests for the two
details that are easy to lose:

* the guard is on the RAW `skip_th`, so 0 disables the gate rather than
  clipping to 25;
* C's `/ 4` is signed integer division and truncates TOWARD ZERO, so at
  `base_q_idx` 127 the term is 0, not -1 as an arithmetic shift would give.

`pipeline.rs` tests it before the reference-derived rewrite and makes that
rewrite an `else if`, as C's is. `ref_skip_percentage` is hoisted into
`inter_hdr_arm::ref_skip_percentage` so the CDEF gate and
`enc_dec_cand_reduction` share ONE call with one set of arguments.

## Measured

`diag 64x64 q40 p8 frames=3`, frame 2: **no frame-header field differs any
more** — the first divergence moves from byte 15 to byte 18, into the tile
payload. Across the eight `frames=3` cells the first differing byte moves 15 ->
18 or 20 on six of them; byte COUNTS are unchanged (six of eight already
matched C).

**Byte-inert on the two-frame envelope, measured:** `inter_byte_matrix.sh` is
92 BOTH / 3 F1DIFF / 1 F0DIFF before and after, cell for cell identical, and
`identity_full_8bit.sh` is 1100/1100. Structurally so: the gate is `!is_key`,
`skip_th` is 0 at every preset up to M7 and on every base frame, and frame 1's
`ref_skip_percentage` is 0 because its reference is an I_SLICE.

## What is still NOT modelled, with its exact reach

* `me_based_cdef_skip` (`md_config_process.c:781`). It returns false
  immediately on an I_SLICE, and on an inter frame it returns false before
  reading any ME statistic whenever `cdef_recon_ctrls.zero_filter_strength_lvl`
  is 0 — which `set_cdef_recon_controls(0)` gives, i.e. **every preset <= 8 on
  the video arm**. So it is inert on this envelope by C's own table, and a real
  gap from preset 9 up, where it needs `rc_me_distortion` and the references'
  `cdef_dist_dev`.
* the `vq_ctrls.sharpness_ctrls.cdef && is_noise_level` arm, which this port
  does not configure.

## The correction this file makes to its own predecessor

"The first diverging frame-header field is a CDEF SEARCH output, downstream of
the recon" was a reasonable reading of a field NAME and a wrong reading of its
VALUE. The lesson is the one `docs/WORKING-ON-THIS.md` §5 already carries in a
different costume: a field's value can be arithmetically impossible for the
thing it is named after, and checking that against the C macro is one line.
`CDEF_DAMPING_FROM_QP(160) = 5` would have said so immediately.
