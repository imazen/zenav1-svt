# Main quality fixes and animated AVIF merge — 2026-09-07

Merged main `74d92430508f80f6f4faeecef94913977dbf95f2` with animation branch
`19e19bf69c7555c359d62959ae6130251657fd80` after the user's request to check
the newly pushed fixes. Both histories are retained in a merge commit.

Main's geometry/error validation, extreme tile limits, cancellation polling,
latency harness, test invariants and film-grain formatting are retained.
The extracted `pipeline/bd10_reencode.rs` contains all five functions from
the animation branch, including native lossless and coefficient contexts.
A source comparison verifies their bodies are unchanged apart from formatting
and the visibility needed by the parent module. Both the decoder-bound chroma
reconstruction replay and main's cancellation check before publishing recon
are retained. The animation validation regression now matches main's richer
error variant and asserts the submitted 65x67 dimensions.

Local results on the merged tree:

- Workspace nextest with `zenav1-svt/avif-container`: **2616/2616**, zero skips.
- C regression spotcheck: **123/123**.
- Native 10-bit lossless: **336/336** exact decoded source comparisons and
  **168/168** color C-byte comparisons, zero differences. Covers all presets,
  color/mono, partial frames and tiles.
- Refusal inventory regenerated and checked: 12 capability / 44 contract
  refusals. Scoped formatting and whitespace checks pass.

Logs: `~/tmp/animation-metadata/main-merge-nextest.log`,
`main-merge-spotcheck.log`, `main-merge-native.log`, `main-merge-gates.log`.
The initial all-target check caught one stale unit-variant match in the
animation test; the successful nextest build includes its correction.

These results verify the merged encoder. The canonical zenavif optional
decoder failures discovered in its separate all-feature run remain separate
work; this record does not claim that suite passes. CI remains deferred.
