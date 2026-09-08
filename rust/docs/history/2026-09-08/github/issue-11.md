# Historical GitHub issue #11: bd10 4:2:0 still encode panics in restoration.rs:985 (index out of bounds: len 2, index 2) on specific real-photo renditions

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/11. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

## Panic

```
thread 'main' panicked at rust/crates/svtav1-encoder/src/restoration.rs:985:30:
index out of bounds: the len is 2 but the index is 2
```

Deterministic, content-dependent. Hit during the HDR phase-2 corpus encode wave (zensim campaign appendix S; zenmetrics run `jobs/hdrgrid-enc-20260806`): **115 of 34,200 zenav1-svt cells** failed with this panic — all on **5 renditions** of the imazen-26-hdr-grid estate, across broad qp ranges (q15..q100 of the corpus grid, i.e. CLI-domain qp ~54..1):

| rendition dims | failed cells |
|---|---|
| 1914x2560 | 26 |
| 2048x1660 | 24 |
| 2297x3072 | 24 |
| 766x1024  | 23 |
| 383x512   | 18 |

(766x1024 and 383x512 are smaller scales of the same origins' family — the trigger follows content, not one absolute size.)

## Repro shape

zenmetrics' `encode_svt_hdr` (crates/zenmetrics-cli/src/sweep/hdr.rs at 9093cc23) = 16-bit PQ RGB -> BT.2020nc limited 10-bit 4:2:0 (`to_yuv420_bd10`, the hdrbudget harness conversion) -> `EncodePipeline::new(w, h, preset=6, RcConfig{Cqp, qp}, 0, 1).with_bit_depth(10).with_tile_rows_log2(0).with_tile_cols_log2(0).with_sb_size(None).with_chroma_420(true)` + `color_description` -> `try_encode_frame_420_hbd`. Port rev: 4c5c1324.

Failing example: rendition `1498_nature_yellow-flowers-garden-bed_colorado_ip13pro_iso50*_3024x4032.scale383x512.hdr.png` at corpus q15 (qp ~54). Sources: `s3://zentrain/refs/imazen-26-hdr-grid-2026-06-14/<name>` (also `/mnt/v/output/imazen-26-hdr-grid-2026-06-14/`); the full failing-cell list with names/q: `/mnt/v/output/hdrgrid-2026-08-06/encode_residue.json`.

Line 985 sits in the loop-restoration unit walk — smells like a restoration-unit count/index edge (len-2 array indexed at 2) that the synthetic `arbitrary_size_robustness` matrix (57/57) doesn't reach with real-content unit decisions at bd10+p6.

## Corpus impact

Non-blocking: the corpus ships at 102,485/102,600 (99.888%) with the residue enumerated per the absent-not-failed discipline; these 115 cells re-enter via a follow-up declare once fixed.
