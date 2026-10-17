# Mac ARM warp_affine NEON — 2026-10-17

Source: `60b965c88e2b3bce2a85c4531af36cdbbc523b21` plus the working-copy change
under this commit (the NEON arm itself), on Apple M4 Pro (arm64).

`port_warp::warp_affine` dispatched `[v3, scalar]`, so aarch64 ran the scalar
sub-block while C binds `svt_av1_warp_affine` to `neon`/`i8mm`/`sve`. The change
moves the x86 `#[arcane]`/`#[rite]` pair onto one `#[magetypes(v3, neon,
-scalar)]` driver calling one generic `warp_affine_subblock_simd<T>` body — the
magetypes ops it already used (`u8x16::widen_low`, `i16x8::madd_adjacent`,
`i32x4` mul/add/reduce) all have NEON backends. Dispatch is now
`[v3, neon, scalar]`; the `full && interior && !is_compound` gate (which keeps
the reference reads in-bounds and the non-compound writeback semantics) is
unchanged, so edge/compound cells still run the scalar sub-block.

Command from `rust/` (binary already built by `cargo bench --no-run`):

```sh
TMPDIR="$HOME/tmp" "$HOME/work/claudehints/scripts/run-heavy" --mem 12G --jobs 4 -- \
    ./target/release/deps/kernel_tiers-* --group=warp --median-of-processes=7 --format=llm
```

Case: `warp_affine_32x32` — 32x32 non-compound prediction interior to a 64x64
reference (all 16 sub-blocks take the vector path), one fixed legal affine
model, output equality asserted before timing.

Result (median of 7 processes, 110 rounds × 113 calls each):

| tier   | mean | vs neon |
|--------|------|---------|
| neon   | 4.04 µs | — |
| scalar | 17.31 µs | +333% (95% CI +300.7%..+365.4%, p=0.0000) |

NEON is ~4.3× faster than scalar on this shape. CV ~11–12%.

Oracle: `cargo test -p zenav1-svt-dsp --lib dsp_parity::c_parity_warp_model`
(12 tests) and `cargo nextest run -p zenav1-svt-dsp` (591 tests) all pass —
the parity cells compare against the C NEON kernel on this machine, covering
non-compound, compound, chroma-subsampled, partial-window and highbd inputs.

`run-heavy`: 65 s, peak RSS **0.02 GiB**, minimum available **4993 MiB**, peak
load **13.69**. Full output: `~/tmp/warp-bench-7p.log` on the Mac.
