# Historical GitHub issue #9: Mainline v4.2.0 quality knobs are fork-gated: tune/QM/variance-boost/sharpness unreachable for still images

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/9. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

A defaults-and-recommended-settings audit against C v4.2.0 found that **several MAINLINE v4.2.0 features are implemented in this port but gated behind `HdrForkConfig::is_fork()`**, which makes SVT-AV1's own still-image recommendations unreachable in mainline mode.

They are mainline, not fork additions — only their *defaults* differ in the fork. Evidence: `Docs/Parameters.md:77` (`--tune`, "3 = IQ (still image only)"), `:95-97` (`--enable-qm`, `--qm-min/max`), `:123-125` (`--enable-variance-boost`, strength), `:132`/`:576` (`--sharpness`, "used in Tune 3 (IQ) and Tune 4 (MS_SSIM), which is designed for still image compression"), plus `Docs/Appendix-Variance-Boost.md:43` ("Strength 3 is best for still images"). The genuinely fork-added config fields are a different, disjoint set (`git show 3115c0c1b -- Source/API/EbSvtAv1Enc.h`).

Port gates: `pipeline.rs` `is_fork()` at the tune sites (~:1393, :1525, :1543, :1557, :1650), variance boost (~:1296), QM (~:1553), sharpness (~:1974, :2030).

## Done already (this issue tracks the rest)

Setting one of these in mainline mode used to be a SILENT no-op — the caller asked for different output and got the default. `encode_frame_impl` now returns `EncodeError::UnsupportedConfig` for `tune != 1`, `enable_qm`, `enable_variance_boost`, or `sharpness != 0` when not in fork mode, matching the precedent set for `lossless`. That converts silent wrong-output into a typed error; it does not make the features reachable.

## The work

1. **Un-gate the mainline knobs.** Either move `tune` / `enable_qm` + levels / `enable_variance_boost` + strength/octile/curve / `sharpness` out of `HdrForkConfig` into a mainline encoder config, or narrow `is_fork()` so it gates only the 14 genuinely fork-added fields. Then drop the rejections above.
2. **Port C's TUNE_IQ / TUNE_MS_SSIM override block** (`enc_handle.c:4891-4916`). Without it, `tune = 3` in the port is not `--tune 3` in C: C's tune-IQ also sets `enable_qm=1`, qm 4..10 luma+chroma, `sharpness=7`, `enable_variance_boost=1`, strength 3, curve 2, `max_tx_size = qp<=45 ? 32 : 64`, `scm=3`, plus the still-image QM polynomial (`md_config_process.c:185,258-266`). Grep shows this block is not ported anywhere.
3. **Add `max_tx_size` (32|64, default 64)** — the only still-image-specific quality knob with its own doc paragraph (`Docs/Parameters.md:536-539`, "increase in output quality consistency, especially for still images") that the port cannot express at all. C consumes it at `enc_dec_process.c:1494-1500, :1815`.
4. **Add fractional CRF.** `RcConfig.qp: u8` quantises rate control to 4-qindex steps; C offers 0.25-CRF granularity via `extended_crf_qindex_offset` (`enc_settings.c:1662-1669`, `rc_crf_cqp.c:471`) — the difference between hitting and missing a target size on a still.
5. **Expose `chroma_sample_position`** — hardcoded 0/UNKNOWN at `obu.rs:799`; C exposes it (`Docs/Parameters.md:367`) and AVIF/HEIF 4:2:0 consumers care about siting.
6. **`AvifEncoder::encode_yuv420` does not emit an AV1 bitstream.** It returns three concatenated monochrome streams with u32 length prefixes (`avif.rs:417-466`, and its own TODO says so) as `Ok(...)`. Route it through `with_chroma_420(true)` + `encode_frame_420`, or make it `UnsupportedConfig` until migrated. `encode_y8` likewise emits monochrome — correct only for gray images; document or split.
7. **`AvifEncoder`'s remaining inert knobs** (`with_trellis`, `with_vaq`, `with_qm`, `with_seg_boost`, `with_still_image_tuning`) are documented-inert but not rejected; once (1) lands, wire them or reject them.
8. **Document the `aq_mode` semantic divergence.** C's default is 2 (inert for a still: `rc_aq.c:899` needs TPL, off for allintra); the port's non-zero `aq_mode` runs a HOMEGROWN VAQ/TPL shift (`pipeline.rs:1248-1277`), so a caller copying C's default gets non-C output. Consider rejecting `aq_mode != 0`.
9. **Prune or privatize `SpeedConfig`'s dead fields** (`speed_config.rs:13-54`) — `enable_cdef`, `enable_restoration`, `max_intra_candidates`, `subpel_precision`, `hme_levels`, `me_search_*` are homegrown and unconsumed on the still path, but read as an authoritative preset table.

## Verified as already correct (no action)

`rate_control_mode`, `aq_mode`-by-effect, `intra_period`, `pred_structure`, `enable_cdef`, `enable_restoration` (incl. the <64px force-off), `enable_dlf`, `enable_tf` (forced off for allintra), overlays, `screen_content_mode` (scm 3 at <=M7, off at M8+), IntraBC level table, film grain, superres/resize defaults, tiles, bit depth, CICP + colour range, profile/tier/level derivation, all 14 fork fields, and every inert-for-stills field. Two defaults were corrected alongside this issue: `RcConfig::default().qp` 30 -> 35 (C `DEFAULT_QP`), and `AvifEncoder`'s speed->preset map now clamps to M9 (C remaps all-intra >M9 to M9, `enc_handle.c:4416-4419`; byte-neutral).

**Pin currency:** v4.2.0 is the newest upstream release (`ls-remote --tags` against both our mirror and gitlab.com/AOMediaCodec/SVT-AV1 agree on `v4.2.0 = 9292ec8e32…`, which is exactly our submodule HEAD's parent), so no default has changed upstream since the pin. Unreleased `master` past v4.2.0 was NOT audited.
