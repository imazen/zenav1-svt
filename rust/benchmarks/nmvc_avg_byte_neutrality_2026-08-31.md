# `FrameContext::nmvc` in `avg_cdf_with` — byte-neutrality, MEASURED

Commit under test: `dbdc22a5` (chunk C3, "FrameContext::nmvc + the MV-write
ordering record"). Host: aarch64 macOS (Darwin 25.5.0), `cargo` dev profile,
`tools/identity_run`. Date 2026-08-31.

## Why this was measured rather than argued

`dbdc22a5` adds one line to a REACHABLE function — `FrameContext::avg_cdf_with`
now averages the new `nmvc` field alongside `ndvc`. The CHANGELOG called that
byte-neutral on two grounds: nothing reads `nmvc` yet, and averaging two equal
contexts is the identity (`floor((4l+2)/4) == l`). Both are true, but the CI
run that would have measured it (`33430901505`) died at the compile step on an
unrelated C1a break, so steps 13-25 — every identity and conformance gate —
were SKIPPED. This is that measurement, run locally instead.

## Result

| comparison | cells | verdict |
|---|---|---|
| `avg_nmv(nmvc)` present vs REMOVED | 32 | **32 / 32 byte-identical** |
| reachability probe (`eprintln` in `avg_cdf_with`) | 4 presets | fires 2x/frame at p0/p4/p6, **0x at p8** |
| strong control (`partition_cdf` halved inside `avg_cdf_with`) | 12 | **0 identical / 12 differ** |

Grid: content {gradient, uniform} x size {128x128, 192x160} x qp {20, 45} x
preset {0, 4, 6, 8}.

## The trap this run walked into, recorded so nobody repeats it

**The first positive control was too weak and produced a false "unreached"
reading.** Perturbing `skip_cdf[0][0]` by -2000 inside `avg_cdf_with` changed
**no** byte across all 32 cells — which reads exactly like "the function is
never called", and would have made the 32/32 identical verdict vacuous
(`docs/WORKING-ON-THIS.md` §5: before you trust a ZERO, prove the probe fires).

It was not unreached. A direct `eprintln!` probe showed `avg_cdf_with` firing
**twice per frame** at presets 0/4/6 on 192x160 — and the count is exactly what
the geometry predicts: SBs are 64x64, so 192x160 is a 3x3 SB grid, and the call
site needs `left_avail && topright_avail` = `col > 0 && row > 0 && col + 1 < 3`,
i.e. `col == 1, row in {1, 2}` = 2 superblocks. It fires 0 times at preset 8,
matching `funnel_chain = use_funnel && preset in 0..=6 && multi_sb`
(`pipeline.rs:7779`).

So the weak control's silence meant "this perturbation flipped no RD decision",
not "this code did not run". Halving `partition_cdf` in the same place moved
12 / 12 cells. **A control that produces no change is only evidence when you
have separately shown the code is reached** — count the calls, don't infer them
from a byte diff.

## Reproduce

```bash
# A side, then delete the `avg_nmv(&mut self.nmvc, ...)` line and re-run:
for c in gradient uniform; do for d in "128 128" "192 160"; do set -- $d
  for q in 20 45; do for p in 0 4 6 8; do
    tools/identity_run $c $1 $2 $q $p /tmp/ab_${c}_${1}x${2}_q${q}_p${p}
done; done; done; done
```
