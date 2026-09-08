# Historical GitHub issue #13: 10-bit published recon never gets loop restoration applied (recon10 feeds the LR search, only u8 recon gets the apply)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/13. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Found while fixing #11; unrelated to it and deliberately left alone there.

## The asymmetry

`recon10` feeds the loop-restoration **search** — the frame-level RD that decides Wiener vs SGR vs `RESTORE_NONE`. But only the u8 `recon` is passed to `apply_restoration_frame`.

So on a 10-bit encode the frame header can signal a restoration type, the search can pick it on 10-bit data, and the **published 10-bit recon never has it applied**.

## Why it matters

The bitstream is unaffected — the apply runs after the tile is entropy-coded, so a conforming decoder reconstructs correctly either way. The damage is to anything that consumes OUR recon:

- 10-bit recon-vs-decoder comparisons will disagree whenever LR is signalled, and the disagreement is *our* fault, not the decoder's — exactly the kind of mismatch that costs a day to attribute (see imazen/rav1d-safe#446, where a similar recon-vs-oracle divergence was mis-filed against the wrong repo first).
- Any RD decision downstream of the recon at 10-bit is made against pixels that differ from what a decoder will produce.
- Multi-frame / reference use would compound it, though this port's shipping envelope is still-picture.

## Scope

Not measured: how often the 10-bit search actually selects a non-`NONE` type on real content, and therefore how much recon drift this causes in practice. Worth measuring before deciding urgency — if 10-bit content almost always picks `RESTORE_NONE` the practical impact is small, and if it does not, it is not.

## Note

#11's fix corrected the extent/stride handling on the apply path and added `PaddedPlaneT::from_strided` / `copy_crop_to_strided`, which is likely the plumbing a 10-bit apply would want to reuse.
