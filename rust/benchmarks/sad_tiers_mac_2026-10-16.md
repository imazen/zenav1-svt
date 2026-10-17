# Mac ARM SAD tier check — 2026-10-16

Source: `224c6bbb9a3dc577c64f6e0531bda5c3f60c6b0b` on Apple M4 Pro.

Command from `rust/`:

```sh
TMPDIR="$HOME/tmp" "$HOME/work/claudehints/scripts/run-heavy" --mem 12G --jobs 4 -- cargo bench -p zenav1-svt-dsp --bench kernel_tiers -- --group=sad_32x32 --format=llm
```

The benchmark checks scalar/NEON output equality before timing. For 32×32 SAD, NEON averaged **153.3 ns** and forced scalar **333.0 ns** over 60 paired rounds; the scalar/NEON difference was +117.21% (95% CI +105.76% to +129.20%). Variability was high (CV 49.2% / 39.8%); use this to confirm tier direction, not to rank small changes.

The optional handwritten NEON and magetypes 32×32 implementations were statistically tied at both padding offsets tested: 196.7 vs 198.1 ns (pad 0) and 196.6 vs 198.7 ns (pad 7). Their confidence intervals cross zero. No dispatch change follows from these data.

`run-heavy`: 93 s, peak RSS **0.46 GiB**, minimum available **5217 MiB**, peak load **4.08**. Full output: `~/tmp/zenav1-svt-mac-kernels.log` on the Mac.
