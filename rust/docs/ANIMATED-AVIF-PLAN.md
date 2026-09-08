# Animated AVIF and public video — current scope

The user's objective remains: complete useful animated AVIF with transparency,
metadata and specification coverage, then useful public video encoding. The
existing all-intra animation is a working step, not completion of that objective.

SVT's `avif::animation` supports timed all-intra color/alpha frames and tested
metadata. zenavif has landed its animation/API work; do not re-merge historical
`animation-avif-complete` branches. Current consumer pins and later AOM format
support are in [the handoff](../../CONTEXT-HANDOFF.md).

Remaining acceptance areas:

- Public streaming frame ownership, timestamps, queues, flush/drain and packets.
  Legacy `Encoder` currently refuses all submission/retrieval; experimental
  pipeline inter gates do not make that interface functional.
- Real GOP/reference/refresh/rate-control/lookahead/temporal behavior, plus
  broader inter-lossless, global-motion and restoration combinations.
- Generic animation timing beyond milliseconds, per-frame hint maps and
  actual inter hint application. Native/concrete-adapter exact ticks exist.
- Transparent grids, wider display transforms (fractional clean aperture,
  track matrices, non-square pixel presentation), advanced auxiliary/metadata
  associations, collections/layers/entity groups and sample transforms.
- Bounded/fallible allocation and cancellation throughout container work;
  independent parsing, reconstruction and lifecycle tests for each new surface.

The [original full plan](history/2026-09-08/rust/docs/ANIMATED-AVIF-PLAN.md)
preserves detailed requirements, old experiment results and counterexamples.
Its historical checkboxes and “unmerged/local” statements are not current.
Use issue #21 as the live tracker. Historical wrapper quality measurements
remain evidence to reproduce at a named current pin, not blanket proof that
already merged wrappers should be rolled back.
