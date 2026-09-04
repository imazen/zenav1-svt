# One CDF told us which symbol frame 2 is missing: `skip_mode`

With §1z²⁷–§1z²⁹ landed, frame 2 of `diag 64x64 q40 p8 frames=3` is 21 B against
C's 21 B with **no frame-header field differing at all** — the whole divergence
is in the tile payload, and both sides code the SAME single block:

```
C     CINTER poc=2 mi=(0,0) bsize=12 part=0 mode=13 rf=1,-1 mv0=0,-24 ... skip=1
port  PDV    mi=(0,0) inter=1 mvr=0 mvc=-24 rf=1 mode=13   (PTREE yeob/ueob/veob all 0)
```

Same partition, same mode, same reference, same MV, same skip. A tile that
differs anyway is either a different SYMBOL SET or a different CDF state.

## The instrument: compare EVERY frame's end-of-frame context, not just frame 0

`tools/fctx_gate.sh` compared frame 0 alone, justified by "frame 1's saved
context can only match once the inter tile does". That stopped being true — on
the campaign's cells frame 1's tile IS byte-identical. Extended to every frame
both dumps carry, on this cell:

| frame | shared fields | identical | differ |
|---|---|---|---|
| 0 | 96 | 96 | 0 |
| 1 | 96 | 96 | 0 |
| 2 | 96 | **95** | **1 — `skip_mode`, first value C=138 port=147** |

Frames 0 and 1 agree to the value, so **frame 2 starts from the same CDF
state**; 147 is `skip_mode`'s default, i.e. the port never adapted it. **A CDF
that adapted on C's side and not on the port's is proof C CODED that symbol.**

## What that names

`entropy_coding.c:5119`, inside the non-intra-frame arm of `write_modes_b`:

```c
write_inter_segment_id(...);
if (frm_hdr->skip_mode_params.skip_mode_flag && is_comp_ref_allowed(bsize))
    encode_skip_mode_av1(blk_ptr, frame_context, ec_writer, skip_mode);
if (!skip_mode)
    encode_skip_coeff_av1(...);
```

and `pd_process.c:4958`:

```c
frm_hdr->skip_mode_params.skip_mode_flag = frm_hdr->skip_mode_params.skip_mode_allowed;
```

The port derives `skip_mode_allowed` — `port_picstruct::setup_skip_mode_allowed`
is C's EXPORTED `svt_av1_setup_skip_mode_allowed` at tier 1 — and then
`inter_hdr_arm` writes

```rust
let skip_mode_present = (pic.skip_mode.skip_mode_allowed != 0).then_some(false);
```

with the comment "`skip_mode_flag` itself is left at C's initialisation value
(0, `resource_coordination_process.c:355`); nothing in the encoder assigns it."
**`pd_process.c:4958` assigns it.** That is the NINTH "a caller passes a
constant where the derivation is already ported" finding of this campaign.

It is invisible before frame 2 because `skip_mode_allowed` needs two references
at DIFFERENT order hints: at poc 1 every DPB slot still holds poc 0, so it is 0
and the constant is right by accident. At poc 2 the references are poc 1 and
poc 0 and it becomes 1 — so C signals the header bit AND codes a `skip_mode`
symbol on every block whose `bsize` allows compound, while the port signals 0
and codes none.

## Everything the fix needs is already ported

| piece | port | state |
|---|---|---|
| `svt_av1_setup_skip_mode_allowed` | `port_picstruct::setup_skip_mode_allowed` | tier 1, WIRED |
| `av1_get_skip_mode_context` | `port_entropy_inter::modes::skip_mode_context` | ported, **uncalled** |
| `encode_skip_mode_av1` | `port_entropy_inter::modes::encode_skip_mode` | ported, **uncalled** |
| `is_comp_ref_allowed` | `port_entropy_inter::refframe::is_comp_ref_allowed` | tier-1-header, used elsewhere |
| the rate | `InterFacBits::skip_mode`, `InterFrame::skip_mode_flag`, `InterBlock::skip_mode_ctx` | ported, fed a constant `false` |

So the remaining chunk is three wires, not a port:

1. `inter_hdr_arm`: `skip_mode_present = Some(skip_mode_allowed != 0)`, C's
   `pd_process.c:4958`.
2. the pack: call `encode_skip_mode` immediately before `write_skip`, gated on
   `skip_mode_flag && is_comp_ref_allowed(bsize)`, with the neighbour context
   the port already computes. C writes it in the non-intra-FRAME arm, so it
   applies to intra blocks of an inter frame too.
3. `inter_md_arm`: stop passing `skip_mode_flag: false` into the cost model, so
   MD pays C's skip-mode rate (`rd_cost.c:562`).

**It cannot move a byte before frame 2** — `skip_mode_allowed` is 0 on every
frame of the campaign's two-frame envelope — which is exactly what makes it
both safe and untestable by any gate this repo currently runs. The
every-frame `fctx_gate` above is the closest thing: it would see the adapted
CDF the moment a third frame is encodable.

## The gate strengthening this leaves behind

`fctx_gate.sh` now compares every frame both dumps carry (2 on its default
cell, and the mutation check has teeth: changing ONE value of frame 1's
`skip_mode` row makes it report `95 identical, 1 differ` and exit 1). Frame 1's
end-of-frame CDF state had never been under test.
