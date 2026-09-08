# Pristine mainline and hybrid reference audit

The existing `SvtHdrMode::Mainline` name identifies the hybrid's MODE0 path.
It does **not** currently prove pristine mainline parity. This audit found a
live, default-enabled chroma-search difference, independently reproduced through
the pristine CLI and native API. Strict `SvtParity` must account for it before
it can ship with mainline as its default reference.

## Pinned sources and measured scope

- Pristine SVT v4.2.0: `9292ec8e32bce26f781f277ec8739b53426c4300`.
- In-tree hybrid: `3115c0c1b23e860dfd75c94f6740e0298182dd13`, MODE0.
- The hybrid is one commit above v4.2.0. The pristine archive was extracted
  independently; the in-tree C source, reference pin and build were not changed.
- GCC 15.2.0, Release, static library, LTO off, native tuning off; 8-bit 420,
  AVIF/all-intra, CQP/AQ0, PSNR default, LP1, 30/1 fps.
- Reused all 1,100 retained inputs from the completed default synthetic+dims
  gate. This is a correctness comparison, not RD or timing evidence.

| Comparison | Identical cells |
|---|---:|
| Current Rust vs pinned hybrid MODE0 | 1,100 / 1,100 |
| Pristine v4.2.0 vs pinned hybrid MODE0 | 1,095 / 1,100 |
| Pristine vs hybrid with one chroma-ranking branch changed | 1,100 / 1,100 |

The five mismatches are `diag` 64×64 and 128×128, QP48, presets0/1,
and `gradient` 128×128, QP20, preset0. Native research −1, native10, enabled
optional settings, real images and inter frames are not covered by this run.

## Root cause and controlled experiments

Pristine `product_coding_loop.c::search_best_independent_uv_mode` ranks the
initial chroma candidates by **variance**, using `vf`/`vf_hbd_10`. The hybrid
adds a condition on `ctx->mds0_ctrls.mds0_dist_type`; the field is zero-initialized
to SAD and has no assignment in `Source/Lib`. Its fallback uses SAD at both
depths. This changes which candidates survive the fast-loop pruning. The Rust
implementation in `leaf_funnel/inject.rs` deliberately follows that hybrid SAD
path, and its existing comments incorrectly generalized it to mainline C.

Experiments, each keeping the original pinned oracle untouched:

1. Pristine CLI and a native-API driver both emit the same 72-byte result for
   `diag 64×64 QP48 preset0`; the hybrid emits 71 bytes. Removing the hybrid
   driver's linker interposers preserves its 71-byte output. Thus neither the
   CLI adapter nor trace interposition explains this witness.
2. Restoring only pristine `full_loop.c` in a scratch hybrid leaves all 1,100
   outputs identical to the original hybrid: that file does not explain these
   witnesses.
3. Restoring pristine `product_coding_loop.c` makes all 1,100 scratch outputs
   match pristine C, including the five mismatches.
4. Restoring the hybrid file and changing **only the independent-chroma VAR
   branch condition** to true has the same result: all 1,100 match pristine.
   All other tracked source files were verified byte-equal to the hybrid pin.

This isolates the default-path discrepancy. It does not establish that this is
the only difference across the full supported domain. The shipping Rust metric
has not yet been changed: the next implementation must wire the pristine choice
under an explicit reference identity while preserving the named hybrid target,
then verify both 8/10-bit and research paths. Changing the oracle or its expected
outputs without identifying the reference would conceal the problem.

## Other reference-envelope differences

The hybrid MODE0 validator also retains changes outside pristine v4.2.0:

| Setting | Pristine v4.2.0 | Hybrid, including MODE0 |
|---|---|---|
| Maximum frame rate | 240 fps | 480 fps |
| Tune enum upper bound | VMAF5 | Film Grain6 |
| Variance-boost curve | 0–2 | 0–3 |
| QP scale compression | Integer0–3 | Floating point0–8 |
| Above4K, non-all-intra slow presets | M5 and faster | M2–M4 accepted with warning |
| Fork-only controls | Absent | Validated/compiled in both modes |

These follow the `enc_settings.c::svt_av1_verify_settings` diff. VMAF5 still
rejects all-intra and low-delay in both references; an enum value is not a still
capability. Mainline already supports useful QM, variance boost and grain
controls, so strict validation must not reject every nondefault tuning control.
The hybrid also changes the version tag and reports an HDR identity in MODE0.
MODE1 retains the v4.2 preset ladder and is not the original HDR fork wholesale.

## Evidence and reproduction

[Machine-readable summary](../benchmarks/pristine-reference-2026-09-08.json)
records commits, source archive/binary hashes, settings and all five witness
hashes. Full settings, commands, logs and encoded outputs are retained under
`~/tmp/svt-tracking/pristine-v4.2.0/` (not yet uploaded). The original full gate
artifacts are under `~/tmp/svt-tracking/post-sad4-full/artifacts/`.

`tools/pristine_reference_compare.py` runs the separately built app against a
retained default identity artifact directory. It requires a fresh output
directory, records all commands/hashes, and exits nonzero for any mismatch,
encode error or timeout. It does not infer source provenance from the binary
name. Build the pristine archive from the pinned commit with:

```sh
cmake -S SOURCE -B BUILD -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF -DSVT_AV1_LTO=OFF
cmake --build BUILD -j 4
python3 tools/pristine_reference_compare.py \
  --app SOURCE/Bin/Release/SvtAv1EncApp \
  --artifacts RETAINED_DEFAULT_IDENTITY_ARTIFACTS --output NEW_OUTPUT_DIR
```

Run each heavy command through the shared `run-heavy` wrapper. The retained
input gate must have used no additional coding-setting environment overrides;
its six-axis settings file does not record such overrides.
