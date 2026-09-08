> Historical document from `rust/docs/OPEN-ISSUES-AUDIT-2026-09-07.md` at SVT `0cbd1279`. Original text follows unchanged. Status and priorities here are superseded by [the current handoff](https://github.com/imazen/zenav1-svt/blob/main/CONTEXT-HANDOFF.md). Original relative links and line numbers refer to the original location/revision.

# Open issue audit after the main merge

Audited main `11522ed068ef6d75f7abe4a5330791e62a4b8aa2` after pushing and
verifying that exact GitHub SHA. Read every open issue body and all comments:
six issues, thirteen comments. This is a source and existing-evidence audit;
historical benchmark results below were not rerun merely for this report.
No issues were closed or edited on GitHub.

Refresh: main is now `46842091f22f0a66217c360786948c9c49b659cc`.
All six issue bodies and thirteen comments are unchanged. The subsequent
main change corrects the regression harness's configured decoder path;
its CI passed. Canonical review `28d08564` now includes exact animation
ticks and color-format forwarding, through cavif-rs `7f55b540` and
zenrav1e `1447c200` (the latter merged to master after local gates passed).
The wrapper main branches still await the separate quality investigation.

The merged encoder code is identical to the tested merge `4e688a54`; intervening
changes affect only `ANIMATED-AVIF-PLAN.md`. Local evidence remains 2616/2616
tests, 123/123 C regression cells, 336/336 native lossless source comparisons,
and 168/168 native color C-byte comparisons. See
[the merge record](../benchmarks/main_merge_2026-09-07.md).

## Issue-by-issue disposition

| Issue | Current disposition | Remaining work |
|---|---|---|
| [#19: quality ordering and saturation](https://github.com/imazen/zenav1-svt/issues/19) | Historical measurement at `2d75a105`: one corroborated inversion among 1599 distinct adjacent steps. The higher setting also reduced bytes, so this is quality-axis ordering, not a dominated RD point. Not remeasured at current main. | Reproduce the named image/q15→q16 pair against the current consumer, preserving both perceptual metrics. Expose or document distinct effective quality settings in the consumer. Do not call duplicate QP mappings corruption. |
| [#18: 10-bit large-image corruption](https://github.com/imazen/zenav1-svt/issues/18) | Encoder fixes landed: `3121b6a8` scopes nondirectional prediction/re-encode to tiles; `2ca060f4` scopes directional availability too. Both are ancestors of main. All eight `issue18_repro` tests passed in the merged 2616-test run, including forced-area portrait and low-preset directional cases. | Closure candidate for the encoder. The final comment also requires consumer pin/fleet-image rollout and replacement of affected stored outputs. The zenavif review pin includes both fixes, but this audit does not verify fleet deployment or re-encoding. |
| [#17: ignored tune/screen controls](https://github.com/imazen/zenav1-svt/issues/17) | Still actionable, with the original claim split. Forced SCM 3 is correctly identical to the all-intra default at presets ≤7, and can matter at ≥8. SCM 0 still falls through the default arm. The proposed tune-0 sharpness discrepancy is supported by current C/Rust source. | Add C-output regressions for SCM 0 and mainline tune 0, then correct their consumers. Preserve SCM 3's legitimate identity at presets where it is already the default. See the source findings below. |
| [#8: documentation debt](https://github.com/imazen/zenav1-svt/issues/8) | Partially stale. PORT-NOTE generation/check exists and is wired in CI; `rust/Cargo.lock` is tracked; both READMEs now distinguish the standing 10-bit HDR byte gate from the historical 8-bit measurement. | Reconcile residual historical counts, symbol/line references and old capability prose. `rust/README.md` still calls the envelope single-frame and names HDR static metadata as next, despite the implemented animation API. Preserve historical capture paths rather than rewriting evidence. |
| [#7: C capability roadmap](https://github.com/imazen/zenav1-svt/issues/7) | Substantially stale. Native 10-bit input, partial dimensions, mono/alpha across presets, superres in a restricted envelope, lossless stills, film-grain modeling/synthesis and animated AVIF are implemented. The old exclusions for animation/video are superseded by the user's current objective. | Replace the old table with current feature/consumer coverage and the live refusal inventory. Public video, wider GOP/reference behavior, superres combinations and remaining metadata/API wiring remain work. Passing still tests does not establish video completion. |
| [#4: reorganization and C build](https://github.com/imazen/zenav1-svt/issues/4) | The major requested source/configuration work exists: renamed repo/packages, Rust layout, removed dead crates, dev-only C oracle, both HDR/mainline builds, SHA/config cache stamps and Windows ARM/macOS Intel/i686 CI legs. Its last comment's missing dual-build claim is stale. | Closure candidate after checking the remaining checklist details and the resulting CI run. This audit did not perform a fresh machine/toolchain-free build, verify every redirect, or benchmark the issue's “ultrafast” requirement. |

## Confirmed source findings behind #17

`crates/svtav1-encoder/src/pipeline.rs` derives `sc_preset` with
`Some(3) => preset.min(7), _ => preset`. Therefore `Some(0)` does not disable
screen detection; it is indistinguishable from an unspecified override at this
consumer. Existing issue measurements independently report the same outcome.

The same file derives effective loop-filter sharpness through
`lf_sharpness_for_tune` only when `self.hdr.is_fork()`. The pinned C source,
`reference/svt-av1/Source/Lib/Codec/deblocking_filter.c`, applies the KEY-frame
VQ/FILM_GRAIN sharpness increment in `svt_av1_pick_filter_level` without an
`SVT_HDR_MODE` guard. The only conditional HDR block in that file is elsewhere,
around filter application. This settles the specific source-level question
left open by the issue comment: that C sharpness branch is not fork-only.
An enabled C-output test is still required before changing the Rust gate;
config-to-config equality cannot detect this defect.

## Remaining gaps beyond the issue titles

1. **Public video is unfinished.** `svtav1/src/lib.rs::Encoder::send_frame`
   discards frame data; `receive_packet` always returns `NotReady`. Experimental
   inter machinery and byte gates do not make that public API functional.
   The live refusal inventory also names reference/GOP, global-motion and
   inter-lossless limits. Use [INTER-ENCODE-PLAN.md](INTER-ENCODE-PLAN.md) to
   locate the implementation work, not the historical “inter absent” table.
2. **Superres is incomplete across supported combinations.** The pipeline
   explicitly refuses native 10-bit superres and superres with active loop
   restoration. Lossless/superres also remains a capability refusal. These
   are C-supported combinations, unlike 12-bit or 4:2:2/4:4:4.
3. **Animation implementation and consumer wiring are different scopes.**
   SVT has exact tick timing, alpha and metadata support. The separate canonical
   zenavif review branch has additional animation/decode fixes; it has not been
   merged here. Exact non-millisecond timing now works through the four native
   input formats and the concrete codec adapter; the generic zencodec trait
   still takes milliseconds. Chroma, RGB identity and range settings now
   reach animation pixels, codec headers and container metadata. Remaining
   consumer work includes additional encoder controls and auxiliary
   metadata/track coverage.
   See [ANIMATED-AVIF-PLAN.md](ANIMATED-AVIF-PLAN.md).
4. **Film grain existed in C and is now translated and wired in SVT.**
   [film-grain-port-map.md](film-grain-port-map.md) identifies the model,
   denoiser, FFT, synthesis, configuration and lifecycle callers, with enabled
   C/decoder tests. This is not evidence that every higher-level wrapper exposes
   those controls, or that every unsupported inter/GOP shape works with grain.
5. **Refusal classification needs care.** The generated inventory lists
   12 capability and 44 contract refusals. Some contract entries describe
   implementation limits, such as restoration with superres or inter-reference
   continuation, while one capability entry is an unreachable defensive guard.
   Neither count is a count of independent missing features.

C v4.2.0 rejects 12-bit and non-4:2:0 input in `svt_av1_verify_settings`.
Those formats are extension work, not missing translations of shipping C
functionality. Monochrome uses reconstruction/conformance evidence because
there is no accepted C monochrome encode mode.

## Separate wrapper quality investigation

No zenavif/cavif main merge or baseline repin is implied by the SVT merge.
Their RGB quality investigation is independent of this encoder's local gates.
The current low-quality witness (`s2/mixed/q15`, 509×341) scores 48.023 on the
old owner/backend and 31.006 on the current pair; libavif-decoded pixels score
48.025 and 30.898 respectively, so the difference is not unique to the managed
decoder. An old-wrapper/current-backend arm reproduces 848 bytes / 31.006.
Backend revisions `dc0a1165` and `6b3b0493` yield 871/38.592 and 858/38.155.
This narrows the cause without establishing a correct fix. No coding tool,
quality floor or envelope was disabled or relaxed.

Raw issue snapshots and all thirteen comment bodies are retained under
`~/tmp/svt-open-issues/`; diagnostic inputs, encoded files and logs are under
`~/tmp/slower-preset-probe/`. The wrapper's previously committed broader audit
is `zenavif/benchmarks/quality_drift_2026-09-07/`.
