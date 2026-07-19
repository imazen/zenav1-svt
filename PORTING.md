# PORTING.md — the C → Rust map

Which C file each Rust module ports, and how to run the differential gate that
proves it. This is a navigation aid; `rust/STATUS.md` is the module-progress
source of truth and `rust/docs/IDENTITY-STATUS.md` is the full divergence map.

## Layout

```
Source/          the SVT-AV1 v4.2.0 C fork — READ-ONLY reference + differential oracle
  Lib/Codec/       encoder core (the bulk of what is ported)
  Lib/C_DEFAULT/   scalar reference kernels (what the port's DSP is compared against)
  API/             EbSvtAv1Enc.h — the config surface the coverage gate tracks
rust/            the Rust port
  crates/          the library crates (dir names keep the `svtav1-` prefix; see below)
  svtav1/          the `zenav1-svt` facade — public API, AVIF backend, examples
  tools/           the identity/differential harnesses
  docs/            port maps and the identity campaign history
```

Package names carry the `zenav1-svt-` prefix; each crate pins a **short
`[lib] name`** so Rust paths stay `use svtav1_encoder::…`. Crate *directories*
still read `crates/svtav1-*` — the port maps and bug log reference those paths,
and issue #3 (consolidation 8 → 4) rewrites the layout anyway.

| directory | package | Rust path |
|---|---|---|
| `crates/svtav1-types` | `zenav1-svt-types` | `svtav1_types` |
| `crates/svtav1-tables` | `zenav1-svt-tables` | `svtav1_tables` |
| `crates/svtav1-dsp` | `zenav1-svt-dsp` | `svtav1_dsp` |
| `crates/svtav1-entropy` | `zenav1-svt-entropy` | `svtav1_entropy` |
| `crates/svtav1-encoder` | `zenav1-svt-encoder` | `svtav1_encoder` |
| `crates/svtav1-cref` | `zenav1-svt-cref` | `svtav1_cref` (test-only) |
| `svtav1` | `zenav1-svt` | `svtav1` |

## Prerequisite — build the C reference

Every differential test and identity run links or drives the in-tree C library.
Build it once from the repo root:

```bash
cmake -S . -B cbuild-static -DCMAKE_BUILD_TYPE=Release \
      -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF
cmake --build cbuild-static -j
```

`crates/svtav1-cref/build.rs` links `Bin/Release/libSvtAv1Enc.a` (override with
`SVT_CREF_LIB_DIR`). The `tools/capture_c_trace` driver rebuilds and relinks
itself against `Source/` on every invocation, so it can never run stale.

## Crate → C source

### `zenav1-svt-types` — no runtime code, just definitions
Ported from `definitions.h`, `mv.h`, `block_structures.h`, `av1_structs.h`.
Modules: `block`, `block_mode`, `constants`, `frame`, `interp`, `motion`,
`partition`, `prediction`, `quantization`, `reference`, `restoration`,
`transform`, `bitstream`.
*No gate of its own — exercised by every crate above it.*

### `zenav1-svt-tables` — const lookup tables (`no_std`, no alloc)
Scan orders, filter taps, block/partition geometry. Generated tables are emitted
by the `zenav1-svt-cref` `gen_*` binaries and carry their regeneration command in
the file header.

### `zenav1-svt-dsp` — kernels

| module | C source |
|---|---|
| `intra_pred` | `intra_prediction.c`, `enc_intra_prediction.c` |
| `inv_txfm`, `fwd_txfm` | `inv_transforms.c`, `transforms.c` |
| `loop_filter` | `deblocking_filter.c`, `deblocking_common.c`, `av1_loopfilter.c` |
| `cdef` | `cdef.c`, `cdef_block.c` |
| `restoration`, `superres` | `restoration.c`, `restoration_pick.c`, `convolve.c` |
| `inter_pred`, `scale` | `convolve.c`, `inter_prediction.c` |
| `warp` | `warped_motion.c`, `enc_warped_motion.c` |
| `obmc` | `enc_inter_prediction.c` |
| `intrabc` | `inter_prediction.c` |
| `ac_bias` | `ac_bias.c` |
| `quant`, `quant_tables` | `full_loop.c` + generated |
| `hadamard`, `sad`, `variance` | `C_DEFAULT/` + the AVX2 kernels the RTCD binds |

> The RTCD binding matters: the encoder binds several kernels to their **AVX2**
> implementations, which are *not* equivalent to the `_c` reference outside the
> 8-bit residual range (see the `hadamard` note in `docs/bd10-port-map.md`).
> Port what the encoder actually calls, not the `_c` twin.

Gate: `cargo test -p zenav1-svt-dsp` — kernel differentials against the exported
C functions via `zenav1-svt-cref`.

### `zenav1-svt-entropy` — range coder, CDFs, OBU

| module | C source |
|---|---|
| `range_coder`, `writer` | `bitstream_unit.c/h` |
| `cdf`, `context` | `cabac_context_model.c/h` |
| `coeff`, `coeff_c` | `entropy_coding.c`, `coefficients.c` |
| `mv_coding` | `entropy_coding.c`, `md_rate_estimation.c` |
| `tile` | `entropy_coding.c` |
| `lr` | `restoration.h`, `entropy_coding.c` |
| `obu` | `enc_settings.c` + the AV1 spec header syntax |

Gate: `cargo test -p zenav1-svt-entropy`, plus the arithmetic-coder op trace in
the identity harness (every range-coder call compared, including coder state).

### `zenav1-svt-encoder` — decisions

| module | C source |
|---|---|
| `pipeline`, `picture` | `pcs.h`, `sequence_control_set.h`, the process chain |
| `partition`, `pd0` | `product_coding_loop.c`, `md_config_process.c`, `enc_mode_config.c` |
| `mode_decision`, `leaf_funnel` | `mode_decision.c`, `product_coding_loop.c`, `full_loop.c`, `rd_cost.c` |
| `depth_refine` | `enc_dec_process.c`, `rd_cost.c` |
| `encode_loop` | `coding_loop.c`, `enc_dec_process.c` |
| `quant`, `qm`, `qm_tables` | `full_loop.c`, `md_config_process.c`, `q_matrices.h` |
| `deblock` | `deblocking_filter.c`, `av1_loopfilter.c` |
| `cdef` | `enc_cdef.c` |
| `restoration` | `restoration_pick.c`, `restoration.c` |
| `rate_control`, `multipass` | `rc_process.c`, `firstpass.c`, `pass2_strategy.c` |
| `sb_qindex`, `var_boost` | `rc_aq.c` |
| `chroma_q` | `rc_crf_cqp.c` |
| `palette` | `palette.c`, `k_means_template.h` |
| `motion_est`, `intrabc` | `motion_estimation.c`, `av1me.c`, `mcomp.c` |
| `sc_detect` | `pic_analysis_process.c` |
| `speed_config` | `enc_mode_config.c` |
| `temporal_filter` | `temporal_filtering.c` |
| `noise_gen`, `film_grain` | `noise_generation.c`, `noise_model.c` |
| `noise_norm` | `full_loop.c` |
| `ssim_md` | `mode_decision.c`, `product_coding_loop.c` |
| `tx_bias` | `pic_operators.c` |
| `tune`, `hdr_mode` | fork feature policy (`docs/HDR-ON-4.2.md`) |
| `bd10`, `frame_geom`, `sb128_geom` | `definitions.h`, `common_utils.c` |

Gate: `cargo test -p zenav1-svt-encoder`, including the
`tests/c_parity_*.rs` suites (quant, QM, bd10 quant, CDEF pick, motion est,
noise gen/norm, palette, SB qindex, SC detect, SSIM MD, temporal, tx bias,
var boost) — each compares against the **exported C function**, not a
transcription.

### `zenav1-svt` — facade
Public API and the AVIF backend. Owns the examples that drive the end-to-end
gates (`identity_run`, `recon_parity`, `decode_conformance`, `perf_vs_c`).

## Running the gates

All from `rust/`. Each prints `<pass> / <total> byte-identical` and exits
non-zero on any failure (identity_matrix is a tracking scoreboard and always
exits 0 — read its printed tally).

```bash
./tools/identity_matrix.sh        # 54 cells — synthetic {content×size×qp×preset}
./tools/partial_sb_gate.sh        # 101 cells — partial-superblock / odd dims
./tools/bd10_matrix.sh            # 36 cells — 10-bit uniform
./tools/bd10_nonflat_gate.sh      # 79 cells — 10-bit with coded residual
cargo test --workspace            # unit + differential parity suites
```

Single cell, with a full first-divergence classification and an op-level trace
diff (the tool to reach for when a gate cell goes red):

```bash
./tools/identity_diff.sh <w> <h> <qp> <preset> <content>   # or: just identity …
```

Other harnesses: `real_image_matrix.sh` (photo corpus), `recon_parity.sh`
(encoder recon == `aomdec` output), `decode_conformance.sh` (every stream
decodes under `aomdec` + `dav1d`), `drill_cell.sh` (per-SB drill-down).

## How verification is layered

1. **Byte identity** — `tools/identity_diff.sh` + `tools/capture_c_trace/` drive
   the real C library through its public API with `--wrap`ped range-coder entry
   points, then compare OBU bytes *and* every arithmetic-coder operation against
   the Rust `symtrace` output. Exit 0 iff byte-identical.
2. **Kernel differentials** — `zenav1-svt-cref` links the real
   `libSvtAv1Enc.a`; `c_parity_*.rs` compares Rust kernels against the exported C
   functions over randomized/gridded inputs.
3. **Decode gates** — every gated stream must decode under `aomdec`/`dav1d`, and
   the decoder's output must equal the encoder's own reconstruction byte-for-byte.

Priority of evidence: real exported C function > synthetic facade over a real
function > verbatim transcription. Transcribed oracles can carry shared bugs.
