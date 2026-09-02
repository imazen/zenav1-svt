# What the port needs to encode a SECOND inter frame — measured, not read

`gradient 64x64 q32 p8`, 2-frame-shift synthetic, low-delay P, flat GOP,
`SVT_FRAMES=3 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1`.
Host r7900x (x86-64 Linux — `-Wl,--wrap` does not work on Apple ld64, so this
cannot be taken on the mac). Oracle `reference/svt-av1` @ `fix/suspected-c-bug-17`.
Port at `main` f321cb04. Dump: `SVT_REFSTATS_OUT`, added to
`tools/capture_c_trace/wrap_recon.c` for this measurement.

**This run is itself the first three-frame C encode this repo has taken** —
`capture_c_trace` died at `SVT_FRAMES=3` until commit ab253150. C codes
1480 / 22 / 21 bytes here.

```
REFSTATS poc=0 sb=0 slice=1 l0cnt=0 l1cnt=0 l0=-1/-1/-1/-1  l1=-1/-1/-1/-1 refhp=-1 refskip=0   refintra=100
REFSTATS poc=1 sb=0 slice=0 l0cnt=1 l1cnt=1 l0=1/0/0/0      l1=1/0/0/0     refhp=-1 refskip=0   refintra=0
REFSTATS poc=2 sb=0 slice=0 l0cnt=2 l1cnt=0 l0=0/0/100/0    l1=-1/-1/-1/-1 refhp=0  refskip=100 refintra=0
```

Fields: `lN = <ref slice_type>/<intra_coded_area>/<skip_coded_area>/<hp_coded_area>`
(`-1` = no reference in that list); `slice` 1 = I_SLICE, 0 = B_SLICE;
`refhp` / `refskip` / `refintra` are `pcs->ref_{hp,skip,intra}_percentage`.

## What this says, and where it CORRECTS the reading of the source

The port refuses frame 2 in `inter_hdr_arm::md_config_inputs`, which returns
`None` for any `get_ref_hp_percentage` answer other than its `-1` sentinel.
Reading `rc_process.c` alone suggests the blocker is `hp_coded_area`. **The
measurement says otherwise on this cell:**

| value | C | the port's placeholder | agrees? |
|---|---|---|---|
| `ref_hp_percentage` | 0 | 0 (`hp_coded_area: 0` in `md_config_inputs`'s `mk()`) | YES, by accident |
| `ref_skip_percentage` | **100** | **0** (`MdConfigInputs::ref_skip_percentage`) | **NO** |
| `ref_intra_percentage` | 0 | 0 | yes |

So the refusal is CONSERVATIVE rather than tight — it fires on a
`ref_hp_percentage` the port would have got right — and the real data gap on
this cell is `skip_coded_area`, which is **100** because frame 1 is a 22-byte
all-skip frame. `ref_skip_percentage` is read by `interpolation_search_level`
on the `enc_mode > ENC_M8 && !is_base` arm (`enc_mode_config.c:9088-9096`),
which `md_config_inputs` refuses separately today. Do not conclude from one
cell that `hp_coded_area` is inert: it is zero here because nothing coded an
odd MV component, which a shift-by-3 synthetic makes likely.

**The reference structure also changes at frame 2**, and nothing in the port
models it: `ref_list0_count_try` goes 1 -> **2** and `ref_list1_count_try`
goes 1 -> **0**, where frame 1 has one reference in each list and both are the
key frame. `part_arm::VideoPic` has exactly two variants — `IntraSlice` and
`InterOnIntraRef` — and deliberately no `InterOnInterRef`, so that needing one
is a compile error rather than a silently wrong PD0 level.

## The list, then

1. **`intra_coded_area` / `skip_coded_area` / `hp_coded_area` on the DPB
   entry.** C accumulates them per coded block in `update_b`
   (`coding_loop.c:1605-1638`) — intra area, high-precision-MV area (only
   while `allow_high_precision_mv`), and no-coeff area — sums them into the
   picture at `enc_dec_process.c:3167-3169`, normalises to
   `100 * area / aligned_pixels` at `rest_process.c:347-349` and copies them
   onto the `EbReferenceObject` at `:195-197`. The port already folds per-SB
   trees onto its DPB entry for `sb_min_sq_size`, so the plumbing exists.
2. **Per-SB `sb_intra` on the DPB entry.** `pd0_detector`'s `use_ref_info`
   arms read `ref_obj->sb_intra[sb_index]`; `part_arm::video_pd0_params`
   currently hardcodes `was_intra: Some(1)` because the only reference in the
   port's envelope is the key frame.
3. **A `VideoPic::InterOnInterRef` arm** and every call site it forces.
4. **`ref_skip_percentage` and `ref_intra_percentage`** through
   `MdConfigInputs`, replacing the placeholder zeros, and then tightening
   `md_config_inputs`' refusal from "any non-sentinel" to "a value we do not
   carry".

Each of 1-4 is checkable against this dump. The three-frame oracle that makes
that possible did not exist before 2026-09-02.
