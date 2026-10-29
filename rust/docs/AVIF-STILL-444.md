# AVIF still facade: 4:4:4 and coded lossless

The facade now routes 8-bit full-resolution planes through
`EncodePipeline::try_encode_frame_444`. The underlying pipeline already encoded
4:4:4; the facade's format refusal prevented access through `AvifEncoder`.

`encode_yuv444([y, u, v], width, height, strides)` requires an explicit
`ChromaSubsampling::Yuv444` selection. All three planes support their own row
stride and an unpadded final row. Tight planes stay borrowed. The raw pipeline
handles profile-1 signaling and selects SB64 for this format.

`with_lossless(true)` selects QP 0. This preserves the supplied Y/U/V planes;
it does not promise that a caller's RGB-to-YCbCr conversion was reversible.
Identity RGB callers supply G/B/R planes and signal matrix 0. Ten-bit 4:4:4,
film grain, and SvtParity are refused; the raw pipeline retains its guards for
unsupported superblocks, superres, and inter-lossless combinations.

Verification on 2026-10-28:

- `just test-avif-facade`: 40 library and four still-policy tests passed.
  New tests compare facade/raw bytes and independently strided planes,
  including short final rows, and require typed configuration failures.
- `tools/chroma_444_gate.sh`: 22/22 cells, zero skips. The existing raw-pipeline
  reconstruction check also requires identical facade bytes. aomdec and
  dav1d agree exactly; QP-0 cells additionally equal the source planes,
  including sizes 1, 7, 17 and 65.
- `SCREEN_DIR=$HOME/.cache/codec-corpus/v1/gb82-sc tools/regression_spotcheck.sh`:
  147/147 cells passed after resolving the existing codec-corpus cache. An
  earlier run failed on six missing fixtures because its default directory
  did not find the cache; no cell was skipped in the completed run.
- Scoped facade strict lint (`cargo clippy --no-deps -p zenav1-svt --lib
  --test still_policy -- -D warnings`) passes. Workspace strict lint is not clean: pre-existing DSP documentation lints
  fail before the facade package. The source-generated refusal inventory is
  regenerated; the full static precheck must be re-run with its prerequisites.

The identity-color extension exposed a sequence-header defect: `separate_uv_delta_q`
was emitted only in the ordinary color branch. C `write_color_config` at the
pinned reference 3115c0c1 writes it after both non-monochrome branches. All four
new identity cells were rejected by aomdec before the repair; all 22 ordinary
and identity cells pass afterward, with exact source samples in the QP-0 cells.

The existing animation facade remains 4:2:0. Enabling the still format must not
make a 4:4:4 animation setting silently code its 4:2:0 input; an explicit guard
and filtered animation regression now require a named UnsupportedConfig error.
