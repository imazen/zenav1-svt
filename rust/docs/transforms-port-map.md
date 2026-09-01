# `transforms.c` + `inv_transforms.c` — per-function port map

Scope: `reference/svt-av1/Source/Lib/Codec/transforms.c` (7,960 lines) and
`Codec/inv_transforms.c` (3,517 lines). Written 2026-08-31 by the `wx-txfm`
lane after auditing every function definition in both files against the port.

**Headline, as a fraction, missing first: 0 of 256 C function definitions are
unported.** 80 match a Rust `fn` of the same name; 174 are covered by a
deliberate collapse (one Rust function per FAMILY of C wrappers, listed
below); 2 have no Rust counterpart *by construction* and are named with their
reason. Nothing in either file is a stub.

That is a claim about translation, not about verification. The evidence tier
per family is in the last column — most of this surface is tier 1, and the
two entries this lane added are stated where they are weaker.

---

## Read this before reading `tools/c_surface_inventory.py`'s rows for these files

**The inventory tool could not SEE 76 of `transforms.c`'s 181 functions.** Its
`DEF` regex captured the function name as `([a-z_][a-z0-9_]*)` — all lowercase
— so every C function with an uppercase letter in its name was invisible: the
55 `_N2_c` / `_N4_c` 1-D kernels, the 38 `_N2_c` / `_N4_c` 2-D wrappers, the 5
`svt_handle_transform*_N2_N4_c` entries. It reported `transforms.c 105` where
the file defines **181**. Same class of defect as every other trap in
`WORKING-ON-THIS.md` §5: a probe that silently sees nothing is
indistinguishable from an absence. The regex is fixed (`([A-Za-z_]\w*)`),
which raises the tree-wide surface from 2,673 to 2,756 definitions.

**With the regex fixed, these two files read as `181 total / 7 matched` and
`75 / 30`.** That is the tool working as documented, not a coverage
statement: it matches by NAME and the port collapses families, so a 54-entry
family behind one Rust function reads as 54 misses. Use the table below, not
that row.

---

## `transforms.c` — 181 definitions

| C family | n | Rust counterpart | evidence |
|---|---:|---|---|
| direct name matches (`fdct*`, `fadst*`, `fidentity*`, their `_N2`/`_N4` twins, `fwht4x4`, `energy_computation`, `transform_config`, `gen_fwd_stage_range`, `wht_fwd_txfm`, `estimate_transform`, …) | 50 | same name in `svtav1-dsp/src/{fwd_txfm,fwd_txfm_pf}.rs` | tier 1 — `c_parity_txfm{,_pf,_pf_2d,_pf_entry}.rs` |
| `highbd_fwd_txfm_{WxH}{,_n2,_n4}` + `svt_av1_highbd_fwd_txfm_{n2,n4}` | 56 | `fwd_txfm_pf::highbd_fwd_txfm(.., shape)` + `highbd_entry_shape` | tier 1 — `c_parity_txfm_pf_entry.rs` |
| `svt_aom_transform_two_d_*_N2_c` / `_N4_c`, `svt_av1_fwd_txfm2d_*_N2_c` / `_N4_c` | 38 | `fwd_txfm_pf::fwd_txfm2d_pf(.., tx_size, tx_type, shape)` | tier 1 — `c_parity_txfm_pf_2d.rs` |
| `svt_av1_transform_two_d_{4x4..64x64}_c`, `svt_av1_fwd_txfm2d_{14 rects}_c` | 19 | `txfm_dispatch::fwd_txfm2d_dispatch` → `fwd_txfm::fwd_txfm2d_c_exact` | tier 1 — `c_parity_txfm.rs` |
| `svt_handle_transform{16x64,32x64,64x16,64x32,64x64}_c` + `_N2_N4_c` | 10 | `fwd_txfm_pf::handle_transform(HandleTransform, pf, out)` | tier 1 — `c_parity_txfm_pf_entry.rs` |
| `av1_estimate_transform_{default,N2,N4,ONLY_DC}` | 4 | `fwd_txfm_pf::estimate_transform(.., shape, ..)` | tier 1 — `c_parity_estimate_transform.rs` |
| `av1_tranform_two_d_core_c` / `_N2_c` / `_N4_c` | 3 | `fwd_txfm::fwd_txfm2d_core` + `fwd_txfm_pf::transform_two_d_core_pf` | tier 1 (through the wrappers above) |
| `svt_aom_fwd_txfm_type_to_func` | 1 | `fwd_txfm::get_fwd_txfm_func` | tier 1 — `c_parity_txfm_pf_2d::fwd_txfm_type_to_func_parity` drives the real C table |
| **`av1_fadst32_new`** | 1 | **`fwd_txfm::fadst32`** — added 2026-08-31 | tier 1, two routes — the C table above, and `c_parity_adst32.rs`'s 52 2-D cells |

## `inv_transforms.c` — 75 definitions

| C family | n | Rust counterpart | evidence |
|---|---:|---|---|
| direct name matches (`idct*`, `iadst*`, `iidentity*`, `clamp_value`, `clamp_buf`, `check_range`, `highbd_clip_pixel_add`, `round_shift_array`, `gen_inv_stage_range`, `get_inv_txfm_cfg`, `iwht4x4*`, `dc/ac_quant_qtx`, `invert_quant`, …) | 30 | same name in `svtav1-dsp/src/inv_txfm.rs` | tier 1 — `c_parity_txfm*.rs`, `c_parity_wht.rs`, `c_parity_bd10_quant.rs` |
| `svt_av1_inv_txfm2d_add_{19 sizes}_c` | 19 | `txfm_dispatch::inv_txfm2d_dispatch_bd` + the named `inv_txfm::inv_txfm2d_WxH_dct_dct` wrappers | tier 1 — `c_parity_txfm.rs` |
| `highbd_inv_txfm_add_{18 sizes}` + `svt_av1_highbd_inv_txfm_add_4x4` | 19 | `txfm_dispatch::highbd_inv_txfm_add` — added 2026-08-31 | tier 1 — `c_parity_inv_recon.rs` |
| `inv_txfm2d_add_c` + `inv_txfm2d_add_facade` | 2 | `inv_txfm::{inv_txfm2d_core, inv_txfm2d_c_exact_bd}` (residual form) and `txfm_dispatch::highbd_inv_txfm_add` (adding form) | tier 1 |
| `svt_aom_inv_transform_recon8bit`, `svt_aom_inv_transform_recon`, `svt_av1_inv_txfm_add_c` | 3 | `txfm_dispatch::inv_transform_recon{,_in_place,8bit,8bit_in_place}` — added 2026-08-31 | tier 1 — `c_parity_inv_recon.rs`, u8 + bd10 + bd12 |
| `svt_aom_inv_txfm_type_to_func` | 1 | `inv_txfm::get_inv_txfm_func` | tier 1 — `c_parity_txfm_pf_2d::inv_txfm_type_to_func_parity` |
| `svt_aom_get_qzbin_factor` | 1 | `svtav1_encoder::bd10::qzbin_factor` (takes the resolved `dc_quant_qtx` rather than computing it) | tier 1 — `c_parity_bd10_quant.rs` pins the quant tables it reads |
| **`av1_iadst32_new`** | 1 | **`inv_txfm::iadst32`** — added 2026-08-31 | tier 1, two routes — the C table above, and `c_parity_adst32.rs`'s 44 2-D cells |
| `cast_to_int32` | 1 | **structurally none** — `(const int32_t*)` over a `const TranLow*`. `TranLow` IS `i32` in the port (`svtav1_types::transform::TranLow`), so the cast has no expressible counterpart. |
| `range_check_value` | 1 | **structurally none, and dead in this build** — both of its bodies are behind `#if`s that evaluate to 0 (`CONFIG_COEFFICIENT_RANGE_CHECKING` is `0` at `definitions.h:356`; `DO_RANGE_CHECK_CLAMP` is defined nowhere), leaving `(void)bit; return value;`. Every call site in the tree is COMMENTED OUT (`transforms.c:2127-2140`). |

---

## Findings

**1. ADST-32 was a real divergence, not just an absent kernel.**
`svt_aom_transform_config` picks `TXFM_TYPE_ADST32` from
`av1_txfm_type_ls[3][TX_TYPE_1D_ADST]` (`inv_transforms.h:195`) for any
ADST/FLIPADST 1-D type on a 32-sample dimension. `get_{fwd,inv}_txfm_func`
answered `None` for that, so `fwd_txfm2d_c_exact` / `inv_txfm2d_c_exact_bd`
REFUSED 52 forward and 44 inverse (size, type) cells where C computes a
result. No conformant AV1 ext-tx set offers such a pair — `tx_size_square_up`
of every size with a 32 side is `TX_32X32`, whose set is DCTONLY or DCT_IDTX
— so the encoder never reached it, which is why nothing had noticed.
`av1_txfm_type_ls[4]` IS `TXFM_TYPE_INVALID` for ADST, so 64 remains a hole on
both sides, correctly.

**2. `av1_iadst32_new` clamps differently from `svt_av1_iadst16_new`, in the
same file.** iadst16 clamps only its additive stages, inline, with
`clamp_value`; iadst32 runs `clamp_buf` over the whole 32-entry buffer after
EVERY stage, and its stage 0 clamps the CALLER's input in place through a
`(int32_t*)` cast of a `const` pointer. The port reproduces the clamps
(the stage-0 one on a local copy, so the output matches for any input) and not
the side effect, which is a no-op inside the 2-D composition anyway.

**3. A residual-derived coefficient set cannot witness that clamp.** Deleting
one of iadst32's eleven `clamp_buf`s left every cell built from C's forward
transform of a realistic residual byte-identical. The inverse's real producer
is a conformant BITSTREAM, and spec 7.12.3 bounds a dequantized coefficient to
`-32768..=32767` at 8-bit; at that range the deletion diverges at 32x32
DCT_ADST coefficient 18 (129 vs 102). `c_parity_adst32.rs` drives both
producer bounds for that reason.

**4. The recon entries are only defined on ext-tx-LEGAL (size, type) pairs, and
driving an illegal one segfaults.** `svt_aom_inv_transform_recon8bit` goes
through the RTCD pointer `svt_av1_inv_txfm_add`, which resolves to
`svt_dav1d_inv_txfm_add_neon` on aarch64 (`common_dsp_rtcd.c:1099`) and
`_ssse3`/`_avx2` on x86-64 (`:540`/`:542`). Those are tables indexed by
(tx_size, tx_type) carrying entries only for pairs a bitstream can signal; an
illegal pair reads a null slot and jumps through it. A first version of
`c_parity_inv_recon.rs` swept all 16 types at all 19 sizes and died at
`16x32 ADST_DCT`. The `svt_av1_*_c` symbols `c_parity_txfm.rs` and
`c_parity_adst32.rs` drive are total over all 16 types and are unaffected.
155 of the 304 pairs are legal; the test asserts both counts.

**5. RTCD initialisation is mandatory for those entries and `nm` proves it.**
`nm -gU Bin/Release/libSvtAv1Enc.a` reports `_svt_av1_inv_txfm_add` and
`_svt_av1_inv_txfm2d_add_*` as `C` (`.bss` function pointers) in this aarch64
build — the NEON devirtualisation header that would `#define` them to direct
calls is not active — so both entries reach NULL one level down without
`svt_aom_setup_common_rtcd_internal`. `shims/inv_recon_shims.c` does the
one-shot init and says why.

**6. `svt_av1_highbd_iwht4x4_1_add_c` is unreachable from either shipping C
caller.** `eob` reaches `highbd_iwht4x4_add` only when the recon read and
write pointers alias — `svt_aom_inv_transform_recon*` otherwise overwrites it
with `av1_get_max_eob` — and of C's two callers only TPL aliases
(`src_ops_process.c:1142`), while TPL passes `lossless = 0`; the
mode-decision wrapper (`full_loop.c:1915`) passes distinct buffers. The port's
`*_in_place` entries expose the combination so the branch is gated anyway.

**7. `highbd_inv_txfm_add_32x32`'s `default:` arm writes NOTHING in a release
build.** It has arms for DCT_DCT and IDTX and `assert(0)` for everything else;
`_64x64` asserts DCT_DCT. With `NDEBUG` the destination buffer is simply left
as it was. The port returns a typed `InvReconError` instead — those pairs are
outside the ext-tx sets, so nothing reachable changes.

**8. Cross-ISA, measured on `ssh r7900x` (x86-64 Linux) as well as this
aarch64 host — two defects that aarch64 alone could not see.**

  a. **The recon shim's buffers.** A first version handed C the Rust `Vec`
     pointers at stride `w`. aarch64: 4/4 green. x86-64: SIGSEGV inside
     `svt_dav1d_inv_txfm2d_add_8x8_avx2`, and ONLY through the hbd entry —
     the 8-bit one stages the caller's pixels into its own
     `DECLARE_ALIGNED(32, uint16_t, tmp[MAX_TX_SQUARE])` first
     (`svt_av1_inv_txfm_add_c`, :3269), so C's SIMD never sees a caller
     buffer there. Fixed by staging everything into 64-byte-aligned scratch
     at `MAX_TX_SIZE` stride — the shape `full_loop.c:1915` actually passes.
     Side benefit: the strides are no longer `w`, so a stride bug in the port
     can no longer hide.

  b. **bd12 has no single C answer.** With the crash gone, x86-64 reported
     `recon bd12 4x4 DCT_DCT` = 1023 where aarch64 C and the port both say
     1582 — `(1 << 10) - 1`, i.e. C's x86 arm clipped a 12-bit reconstruction
     to 10 bits. Attributed with a control rather than guessed: against the
     `_c` kernels (`ref_inv_txfm2d_add_c_bd`, which bypasses the RTCD
     pointers) the port matches at bd10 AND bd12 on 310 cells, on both ISAs.
     So the 1023 is C's SIMD. bd12 is outside C v4.2.0's shipping envelope
     anyway (`svt_av1_verify_settings`, `Globals/enc_settings.c:460`), so the
     dispatched-entry test runs bd10 and the scalar test carries bd12.

  Everything in this lane is now green on BOTH ISAs: `c_parity_adst32` 3/3,
  `c_parity_inv_recon` 6/6, `c_parity_txfm` 20/20, `c_parity_txfm_pf_2d` 8/8.

---

## Not done, and why

- **The new recon entries are not WIRED.** `leaf_funnel/tx_pipeline.rs` still
  does its inverse+add inline, byte-identically, and is untouched. Re-pointing
  it at `txfm_dispatch::inv_transform_recon8bit` is behaviour-neutral by
  construction but changes a path every identity gate walks, so it belongs in
  its own separately-verified change.
- **No SIMD for `fadst32` / `iadst32`.** They are unreachable from a
  conformant encode; a NEON kernel would be unmeasurable perf work.
