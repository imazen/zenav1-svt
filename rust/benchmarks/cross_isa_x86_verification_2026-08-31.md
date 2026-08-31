# x86-64 cross-ISA verification of the inter campaign — 2026-08-31

## Why this exists

Four lanes landed 27+ commits on `main` on 2026-08-31, and **not one of them had
CI evidence**. GitHub's runners were saturated all day — 16 runs queued, zero
started, across ~1.5 hours of watching. Worse, the runs that did exist were
poisoned: a compile error in `entropy/obu.rs` (a `#[cfg(test)]` `SeqTools`
literal not updated when a field was added) failed CI at step 12, which
**skipped steps 13-25** — decode conformance, bd10 identity, SIMD tier
invariance, the spot-check and the 8-bit all-preset sweep — for that commit and
the nine others from three lanes that shared its parent.

So every number the campaign reported was aarch64-darwin only. Per
`docs/WORKING-ON-THIS.md` §5c that is host-specific until a second ISA measures
it. This is that measurement, taken on the `r7900x` workstation
(x86_64-unknown-linux-gnu, 24 cores) rather than waiting for the queue.

Commit under test: `3c1836f4c`. The C oracle was built from scratch on that
host, so the differential suites ran against a genuinely different ISA *and* a
different C compiler.

## Results

| gate | x86-64 result |
|---|---|
| `identity_full_8bit.sh` — 4 content x 2 sizes x 5 qp x 14 presets, plus 15 geometries x 2 qp x 9 presets x 2 content | **1100 / 1100 byte-identical**, 0 harness errors |
| `regression_spotcheck.sh` | **35 / 35** |
| `cargo nextest run -p zenav1-svt --test tier_invariance` | **5 / 5** |
| `cargo nextest run --workspace --no-fail-fast` | 1149 / 1152 |

**The campaign's central invariant holds on a second ISA:** 27+ commits from
four concurrent lanes moved zero still-image bytes.

## The three workspace failures, and what each one is

1. `c_parity_obmc_search::convolve8_matches_c` — **a real x86-only value
   divergence**, not off-by-one (`convolve8_horiz 4x4`: `[63, 134, 34, 87, …]`
   vs `[186, 152, 128, 34, …]`).
2. `c_parity_obmc_search::upsampled_pred_matches_c` — **SIGSEGV** on x86.
3. `tier_invariance::intrabc_output_is_tier_invariant_on_real_screen_content` —
   environmental, the `gb82-sc` corpus was absent. Fixed by copying the corpus
   to the host; it and the four spot-check cells then passed (the 31/31 → 35/35
   move below).

(1) and (2) are being root-caused separately. What makes them worth chasing
rather than dismissing: the port side, `inter_me/obmc_search.rs::convolve8_horiz`,
is pure scalar integer Rust — a triple loop of `i32 * i32` with a rounding
shift, no SIMD, no floats, no dispatch — so it is ISA-invariant by construction
and cannot itself differ between the hosts. Both tests landed the same day
carrying **tier-1** evidence; if the oracle is unsound for that path on one
ISA, the aarch64 green is not evidence either.

There is precedent in this tree: `docs/SUSPECTED-C-BUGS.md` #11 records that on
aarch64 SVT's RTCD wires **every** `svt_aom_obmc_sub_pixel_variance` size to
the 4x8 NEON kernel, so the C binary is already known not to be a valid oracle
for one branch of this family on one host.

## A coverage gap this run exposed

The first spot-check run reported **31 / 31**, not 35 — and named the four
cells it could not run under "SKIPPED (corpus/tool absent — these cells guarded
NOTHING this run)". The harness behaved exactly as §5 demands. But all four
guard **IntraBC and palette** paths, and IntraBC is what the `has_top_right`
fix changed that same day: the cells most relevant to the day's riskiest change
were precisely the ones not running. With the corpus copied over, the run is
**35 / 35**.

Generalisable: a host missing a corpus does not fail loudly in aggregate — it
silently narrows what the gate covers, and the narrowing correlates with
whatever is least standard about that host. Check WHICH cells ran, not just the
ratio.

`aomdec` was also absent (`issue13_bd10_final_recon_matches_aomdec_when_wiener_is_signalled`
refuses to skip itself and says so in its panic). Installed `aom-tools` 3.13.1
rather than taking the sanctioned `ZENAV1_SKIP_DECODER_TESTS=1` skip, so the
one check that the published 10-bit recon is what a decoder produces actually
ran.

## Reproducing

```
ssh r7900x
cd ~/work/zen/zenav1-svt && git merge --ff-only origin/main && git submodule update --init --recursive
cd rust && source ~/.cargo/env
nice -n 19 cargo nextest run --workspace -j 12 --no-fail-fast
nice -n 19 ./tools/regression_spotcheck.sh
nice -n 19 ./tools/identity_full_8bit.sh
```
The host needs `aom-tools` and a `~/work/zen/codec-corpus/gb82-sc` tree; both
are now present.
