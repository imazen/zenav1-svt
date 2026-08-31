# `blend_a64_d16_mask` — cross-ISA domain measurement, 2026-08-31

Why: `c_parity_port_masked_blend.rs`'s own C-vs-C control reported *"C's
dispatched d16 blend disagrees with its own `_c` kernel on 20 of 20 cells"* on
x86-64 while aarch64 stayed green. Root cause and the numbers behind it.

**Base commit:** `6b188af65` (`main@origin`).
**Hosts:** macOS aarch64 (`cpu_flags 0xf`, NEON) and Linux x86-64 `r7900x`
(`cpu_flags 0x1cfbf`, AVX2 dispatched — the RTCD pointer resolves to
`svt_aom_lowbd_blend_a64_d16_mask_avx2` on that box).
**Oracle:** each host's own `Bin/Release/libSvtAv1Enc.a`.

## What the encoder can actually put in a CONV_BUF (8-bit compound)

| source | range |
|---|---|
| `svt_av1_jnt_convolve_2d_c`'s own assert (inter_prediction.c:564) | `< 16384` |
| analytic tap-sign extremum, every interp filter x subpel phase | `[1012, 15356]` |
| driving the real convolve, 400 cells, 0/255-checkerboard source | `[2919, 12159]` |

The last row is the in-repo measurement
(`compound_conv_buf_stays_inside_the_blend_domain`); it is identical on both
hosts, as it must be — `_c` is portable integer C.

## Where each host's DISPATCHED kernel stops agreeing with `_c`

All-lanes-equal probe, `v` swept over `0..=65535`, first divergence:

| kernel | aarch64 | x86-64 |
|---|---|---|
| lowbd d16 blend | none in `u16` | **32768**, every shape |
| highbd d16 blend, bd 8 | none in `u16` | none in `u16` |
| highbd d16 blend, bd 10 | none in `u16` | none in `u16` |
| highbd d16 blend, bd 12 | 6152 (8x8, 16x8) / 8654 (4x4) | none in `u16` |

## Full shape grid — 308 cells (22 block sizes x {full, half-w, half-h, both} x subw x subh)

| grid | aarch64 | x86-64 |
|---|---|---|
| lowbd, values `< 16384` (in contract) | 308/308 | **308/308** |
| lowbd, values `< 40000` (the old generator) | 308/308 | **0/308** |
| highbd bd 8, values `< 16384` | 308/308 | 308/308 |
| highbd bd 10, values `< 65536` | 308/308 | 308/308 |
| highbd bd 12, values `< 65536` | **0/308** | 308/308 |

## Roots

* **x86 lowbd:** `_mm_madd_epi16` (`ASM_SSE4_1/blend_sse4.h:188`, reached from
  the AVX2 entry) reads a `CONV_BUF_TYPE` = `uint16_t` entry as SIGNED int16.
  aarch64's kernel is unsigned end to end (`vmull_u16` / `vmlal_u16` /
  `vqsubq_u16` / `vqrshrn_n_u16`, `ASM_NEON/blend_a64_mask_neon.c:208`).
  `docs/SUSPECTED-C-BUGS.md` #19.
* **aarch64 highbd bd 12:** `svt_aom_highbd_blend_a64_d16_mask_neon`
  (`ASM_NEON/highbd_blend_a64_mask_neon.c:453-459`) branches
  `bd == 10 ? 10-bit : 8-bit` and has no 12-bit arm.
  `docs/SUSPECTED-C-BUGS.md` #20. Unreachable — `svt_av1_verify_settings`
  (enc_settings.c:460) rejects any depth but 8/10.

Neither is a dispatch defect on the encoder's own domain: at in-contract
magnitudes both hosts are 308/308 at bd 8 and bd 10.

## Reproduce

```
cd rust
SVT_CREF_LIB_DIR=$PWD/../Bin/Release SVT_CREF_SKIP_HDR=1 \
  cargo nextest run -p zenav1-svt-dsp --test c_parity_port_masked_blend \
  --success-output final
```

`c_lowbd_d16_blend_domain_covers_the_conv_buf_contract` prints the first
divergence per host, `c_highbd_d16_blend_vs_c_scalar_blend` prints the bd-12
split, `c_rtcd_blend_vs_c_scalar_blend` prints the 308-shape count.

## Suite state at this commit + this change

| host | before | after |
|---|---|---|
| aarch64 | 1932/1932 | **1935/1935** |
| x86-64 | 1936/1939 (3 failures) | **1941/1942** (1 failure) |

The x86 residual is `c_parity_rc_process::new_framerate_matches_c`
(`docs/SUSPECTED-C-BUGS.md` #17), a different lane's open item — present in the
pre-change baseline log (`~/tmp/x86-final.log:1557`) and untouched here.
