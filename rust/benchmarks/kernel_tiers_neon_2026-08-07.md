# aarch64 kernel-tier numbers after the 2026-08-07 DSP series

`cargo bench -p zenav1-svt-dsp --bench kernel_tiers` (zenbench, interleaved
arms, host tier vs forced scalar). Apple M4 Pro, darwin 25.5.0.

**These supersede the Wiener figures quoted in the commit message of
`perf(restoration): rewrite the NEON Wiener compute_stats as dot products`.**
That commit reported `win5 91.0 us vs scalar 590.9` and
`win7 300.5 us vs scalar 1325.3`. Both arms were inflated ~1.7x by a foreign
~100 %-CPU process on the shared box; the RATIO survived (6.5x/4.4x vs the
6.1x/4.2x below) because the scalar arm moved with it, but the absolute
microseconds did not. Re-measured twice back to back, agreeing to ~1 %:

| kernel | neon | scalar | speedup |
|---|---|---|---|
| `wiener_compute_stats_win5_64x64` | 53.0 / 53.5 us | 327.9 / 325.7 us | **6.1x** |
| `wiener_compute_stats_win7_64x64` | 181.8 / 183.7 us | 779.6 / 775.2 us | **4.2x** |

The transform figures in `perf(txfm): real int32x4 NEON primitives …` DID hold
up — a re-run reproduces every one of them to within 3 %:

| kernel | neon | scalar | speedup |
|---|---|---|---|
| `fwd_txfm2d_8x8_dct`   |  23.8 ns | 222.4 ns | 9.3x |
| `fwd_txfm2d_16x16_dct` |   189 ns |   593 ns | 3.1x |
| `fwd_txfm2d_32x32_dct` |   760 ns |  2567 ns | 3.4x |
| `fwd_txfm2d_64x64_dct` |   3.7 us |  12.2 us | 3.3x |
| `inv_txfm2d_8x8_dct`   |  37.4 ns | 378.2 ns | 10.1x |
| `inv_txfm2d_16x16_dct` |   246 ns |  1211 ns | 4.9x |
| `inv_txfm2d_32x32_dct` |   1.1 us |   5.0 us | 4.5x |
| `inv_txfm2d_64x64_dct` |   5.7 us |  25.5 us | 4.5x |

## Method note, because it cost time twice in this session

`kernel_tiers` runs both arms in one process with zenbench's interleaving, so
its RATIOS are robust to a busy box — but its ABSOLUTE times are not, and a
foreign process can move them by 1.8x with no code change (observed: the win5
scalar arm at 590.9 / 558.6 / 330.5 us across three runs). It also could not
settle a 5 % encoder-level question at all (see
`alloc_traffic_null_2026-08-07.meta`'s addendum, where it disagreed with the
encoder A/B about a change that was in fact a 0.944x regression).

**Decide with `tools/perf_ab.sh`** — interleaved paired whole-encoder A/B of two
binaries, which is immune to both problems. Use `kernel_tiers` for the
per-kernel ratio and as a sanity check, and quote its absolute numbers only
from a run you can reproduce back to back.
