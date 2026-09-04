# `skip_mode` wired: FIVE of eight cells are byte-identical at THREE frames

`benchmarks/frame2_skip_mode_2026-09-03.md` localized the frame-2 tile
divergence to one symbol — `skip_mode` — by comparing every frame's
end-of-frame CDF state, and recorded the fix as "three wires, not a port". This
file records the three wires and what they measured.

## The three wires

| C | port site | was | is |
|---|---|---|---|
| `pd_process.c:4958` | `inter_hdr_arm` | `skip_mode_present = allowed.then_some(false)` | `= skip_mode_flag` |
| `entropy_coding.c:5119` | `pipeline::encode_block_syntax` | nothing | `encode_skip_mode` before `write_skip`, gated on `skip_mode_flag && is_comp_ref_allowed(bsize)` |
| `rd_cost.c:562` | `inter_md_arm` | `skip_mode_flag: false`, `skip_mode_ctx: 0` | C's frame flag and `skip_mode_context(neighbors)` |

`skip_mode_context`, `encode_skip_mode`, `is_comp_ref_allowed`,
`setup_skip_mode_allowed` and the `InterFacBits::skip_mode` rate table were all
already ported. Nothing new was transcribed.

Two constants stay, NAMED rather than made plausible:
`skip_mode_ref_frame_idx_{0,1}` on the injector context. `setup_skip_mode_allowed`
derives them, but their only reader is the injector's `NEAREST_NEAREST` arm,
which `allow_bipred: false` makes unreachable — a wrong pair there would be
invisible, so they are left at -1 with that stated.

## Measured

`frames=3`, `SVTAV1_FRAME_SHIFT=3`, low-delay P, the frame-2 refusal lifted
behind a throwaway env for the measurement only (it is still in the tree).
"IDENTICAL" below means **every frame** — 0, 1 and 2 — byte for byte.

| cell | C f2 | before this chunk | after |
|---|---|---|---|
| `gradient 64x64 q32 p8` | 21 | 21 B, differs at byte 18 | **IDENTICAL** |
| `diag 64x64 q40 p8` | 21 | 21 B, differs at byte 18 | **IDENTICAL** |
| `uniform 64x64 q40 p6` | 21 | 21 B, differs at byte 18 | **IDENTICAL** |
| `screen 64x64 q40 p6` | 21 | 21 B, differs at byte 18 | **IDENTICAL** |
| `diag 128x128 q40 p6` | 23 | 23 B, differs at byte 20 | **IDENTICAL** |
| `gradient 64x64 q40 p6` | 21 | 21 B, differs at byte 18 | 21 B, differs at byte 20 |
| `diag 72x72 q40 p8` | 27 | 26 B | 26 B |
| `gradient 128x128 q40 p8` | 35 | 23 B | 23 B |

**Five of eight three-frame cells are now byte-identical end to end.** For
context, the same eight cells coded frame 2 at 466 B against C's 21 at the
start of this chunk sequence.

## The refusal STAYS, and why

Three cells are still wrong. Lifting a blanket refusal that covers all eight
would ship silently-wrong bytes on those three, which is exactly the "partial
lift" the two `refuses_inter3` spot-check cells exist to prevent. Both still
pass.

**This is now a decision for a human, not a measurement**, and it is written
down rather than taken: the frame-1 path solved the same tension with a
PASS/OPEN gate (`inter_byte_gate.sh`) plus a public-API refusal, and frame 2 is
now in the same shape — a majority of cells provably correct, a named minority
not. Converting the blanket refusal into that model would give the five
byte-identical cells regression protection they currently cannot have, because
nothing can measure them while the refusal fires. Until that call is made, the
five are correct and unguarded.

## Byte-inert on everything the gates cover, and structurally so

`inter_byte_matrix.sh` 92 BOTH / 3 F1DIFF / 1 F0DIFF, cell for cell identical
before and after; `identity_full_8bit.sh` 1100/1100. `skip_mode_allowed`
requires two references at DIFFERENT order hints, and on the campaign's first
inter frame every DPB slot still holds the key frame — so the flag is 0 on
every frame any current gate reaches, and all three wires are no-ops there.

## What the three residual cells are

* `gradient 64x64 q40 p6` — same byte count, diverges at byte 20 (was 18), so
  the skip-mode symbol closed part of it and something later in the tile has
  not.
* `diag 72x72 q40 p8` — 26 B against 27, a PARTIAL superblock, and its frame 1
  is one of the grid's three residual F1DIFF cells; frame-2 readings on it are
  downstream of that.
* `gradient 128x128 q40 p8` — 23 B against 35, the largest remaining gap and
  the only one where the port codes substantially FEWER bytes than C.
