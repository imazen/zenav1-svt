# The frame-2 refusal, re-measured — and the FIFTH mechanism the list did not name

`benchmarks/ref_coded_area_stats_2026-09-02.md` scoped the port's frame-2
refusal to four mechanisms. This file records (a) that its measurement
REPRODUCES on this host, (b) which of the four landed, and (c) a fifth
mechanism, not on that list, that would have made every frame-2 measurement
void had it not been found.

Cell throughout: `gradient 64x64 q32 p8`, `SVT_FRAMES=3`,
`SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1`, shift 3.
C oracle `reference/svt-av1 @ fix/suspected-c-bug-17` through
`tools/ctrace-linux/run.sh` — the linux/arm64 container, because Apple ld64
has no `-Wl,--wrap`. C codes **1480 / 22 / 21 B**.

## (a) The 09-02 verdict reproduces, on THIS host

The 09-02 dump was taken on `r7900x`, and `ctrace-linux/run.sh` did not
forward `SVT_REFSTATS_OUT` until the drift guard was added — so the brief that
commissioned this work flagged its evidence path as unverified. Re-run here,
byte for byte the same three lines:

```
REFSTATS poc=0 sb=0 slice=1 l0cnt=0 l1cnt=0 l0=-1/-1/-1/-1 l1=-1/-1/-1/-1 refhp=-1 refskip=0   refintra=100
REFSTATS poc=1 sb=0 slice=0 l0cnt=1 l1cnt=1 l0=1/0/0/0     l1=1/0/0/0     refhp=-1 refskip=0   refintra=0
REFSTATS poc=2 sb=0 slice=0 l0cnt=2 l1cnt=0 l0=0/0/100/0   l1=-1/-1/-1/-1 refhp=0  refskip=100 refintra=0
```

**The verdict stands.** `ref_skip_percentage` is the value that disagreed
(100 against the port's placeholder 0); `ref_hp_percentage` is 0, which the
placeholder matched by accident.

## (b) What landed, joined to that dump

The port now accumulates C's three coded areas in its own walk
(`CodedAreaAcc`, placed beside the `skip` symbol the walk already writes,
because C's own skip accumulator IS `blk_ptr->block_has_coeff == 0`) and
stores the normalised percentages plus per-superblock `sb_intra` / `sb_skip`
on the DPB entry. `SVTAV1_REFSTATS=1` prints them for the frame that WRITES
them, against C's line for the frame that READS them:

| what | C (read at poc 2) | port (written at poc 1) |
|---|---|---|
| reference `slice_type` | 0 (B) | 0 |
| `intra_coded_area` | 0 | 0 |
| `skip_coded_area` | **100** | **100** |
| `hp_coded_area` | 0 | 0 |
| `ref_list0_count_try` | 2 | 2 |
| `ref_list1_count_try` | 0 | 0 |

Frame 1 is a 22-byte all-skip frame, so 100 % skip area and 0 % intra is the
right answer twice over, and the port's per-SB flags say the same thing:
`poc=0 sbintra=[1] sbskip=[0]` (a key frame codes intra and has
coefficients), `poc=1 sbintra=[0] sbskip=[1]`.

Landed: **mechanism 1** (the three accumulators, plus `sb_intra` / `sb_skip`
and `slice_type` on the DPB entry) and **mechanism 4**
(`ref_intra_percentage` / `ref_skip_percentage` / `ref_hp_percentage` through
`MdConfigInputs`, all three derived by the already-ported
`rc_process.c:66/96/118` readers).

**A latent defect mechanism 4 exposed.** `pipeline.rs` filled
`MdConfigInputs::ref_list{0,1}_count_try` from `ref_list{0,1}_count` — the
UNCAPPED counts. They agree on every cell this port has encoded (both are
`min(found, base_ref_listN_count)` on a base-layer frame) and diverge under
`list0_only` or off the base layer, and `ref_list1_count_try` is exactly the
field C's dump reports going 1 -> 0 at frame 2. Corrected.

## (c) The FIFTH mechanism: the port's DPB never received an inter frame

`PictureControlSet::new_inter_frame` hard-codes `refresh_frame_flags: 0`, and
that constant is what reached `self.dpb.refresh(..)`. The frame HEADER did not
use it — `inter_hdr_arm` writes `pic.rps.refresh_frame_mask`, C's real value —
so the stream announced one thing and the encoder's own DPB did another.

MEASURED before the fix, at poc 2: `rps.ref_dpb_index[0]` is slot 1 and
**every one of the 8 slots still held the KEY frame**, so LAST resolved to
poc 0 where C's resolves to poc 1. After the fix, `poc=1 refresh=0x02` and
poc 2's slot 1 reports `is_islice=false`.

This is the FIFTH "a caller passes a constant where the derivation is already
ported" finding of this campaign, and it is invisible at two frames because
nothing reads the DPB after frame 1 — which is exactly the envelope every
gate in this repo covers. **Any frame-2 reading taken before it is void**,
including any that a future chunk might be tempted to take from the 09-02
notes.

## What is still refused, and why the refusal MOVED rather than lifting

With mechanisms 1 and 4 in and nothing else changed, frame 2 ENCODES — at
**466 B against C's 21**, and `SVTAV1_REFSTATS` reports it coded
`intra=100`, i.e. every block intra. So the refusal is reinstated, keyed on
the gap that is actually left rather than on the one that is closed:

> an inter frame whose LIST-0 REFERENCE is itself an inter frame needs C's
> per-superblock `pd0_detector` inputs

`pd0_detector` reads `ref_obj_l0->sb_intra[sb_index]` per superblock
(`enc_dec_process.c:2126`); `part_arm::VideoPic` has no `InterOnInterRef`
variant and `video_pd0_params` answers a constant `was_intra: Some(1)`, which
is true only of a KEY-frame reference — every superblock of one codes intra.
On an inter reference it is a guess, and it picks `pic_pd0_lvl`, the level the
whole partition search runs at.

The DPB now carries `sb_intra` and `sb_skip` per superblock, so the remaining
work is the `InterOnInterRef` arm and the per-SB detector inputs it consumes —
including the CURRENT picture's `sb_intra[left]` / `sb_skip[top]`, which C has
because its MD and EncDec are one loop and this port's are two passes.

**Mechanisms 2 and 3 are NOT landed. The refusal stays.**
