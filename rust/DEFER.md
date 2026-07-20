# DEFER — production-hardening follow-ups (post bd10-MD)

These items from the prod-hardening plan were intentionally NOT landed on the
`prod/hardening` branch because they require flipping the encode body
(`EncodePipeline::encode_frame_impl` and the functions it calls) from infallible
to fallible (`-> EncodeResult<..>`), which collides with the concurrent bd10-MD
agent editing the same `pipeline.rs` hot loops. Land them after the bd10 work
merges, on a fresh tree.

The **foundation** is already in place and byte-inert:
- `svtav1_types::try_vec!` + `alloc_vec_fallible` (`fallible-alloc` feature) — Feature 3.
- `EncodeError` / `EncodeResult` / `At<..>` tracing — Feature 2.
- `EncodePipeline::stop` (`almost_enough::StopToken`) + `with_stop` builder + the
  frame-granular entry check in `try_encode_frame*` — Feature 1 (infrastructure).

> **Line numbers below are from the base commit `9cd0d5d02` `pipeline.rs`.** The
> `prod/hardening` additive edits shifted them by roughly +150 lines, and the
> bd10-MD agent is independently editing these hot loops, so treat the numbers
> as approximate — re-locate each site by its allocation pattern / surrounding
> code, not by absolute line.

## Feature 3 — fallible-alloc call-site conversions (~20 sites)

Convert each `vec![val; len]` (or equivalent large/​untrusted-size allocation) to
`try_vec![val; len]?`, propagating `EncodeResult`. This requires the enclosing
function to return `EncodeResult<..>` (hence the deferral).

- `crates/svtav1-encoder/src/pipeline.rs`: 165, 516, 636, 748–749, 756/759,
  1009–1012, 1327/1413, 4403, 4646–4647, 5119–5121, 5221–5222, 5227, 5255, 5266
- `crates/svtav1-encoder/src/deblock.rs`: 439
- `crates/svtav1-encoder/src/temporal_filter.rs`: 70–71, 118
- `crates/svtav1-encoder/src/cdef.rs`: 1265, 1477
- `crates/svtav1-encoder/src/restoration.rs`: 703, 821

## Feature 1 — in-loop (SB-granular) cooperative stop-checks (5 points)

Add `self.stop.check().map_err(EncodeError::from).map_err(whereat::at)?;` (or the
`&dyn Stop` / `may_stop()` hot-loop optimization) at these superblock-loop points
so cancellation is honored mid-frame, not only at frame entry. Also requires the
fallible impl flip.

- `crates/svtav1-encoder/src/pipeline.rs`: 1048, 1055, 1334, 1517, 5303

## Sequencing note

1. Land the impl-flip (`encode_frame_impl` + callees `-> EncodeResult`) first,
   keeping `encode_frame` / `encode_frame_420` as infallible wrappers that
   `.expect()`/`.unwrap()` the fallible core (preserving the current panicking
   contract and byte output), and route `try_encode_frame*` straight to the
   fallible core.
2. Then convert the alloc sites and add the SB-loop stop-checks.
3. Re-run every gate under both default and `--features fallible-alloc`; both
   must stay byte-identical on the success path.
