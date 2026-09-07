# Quality-program branch audit — 2026-09-07

Audited all ten commits in `main...eng/quality-program` against main
`e86f9a9e6d931b342d2e9e6f331da8de7cff6d18`, in an isolated worktree.
The branch was 697 commits behind main. Applying it wholesale would restore
obsolete assumptions about crate layout, SIMD, and the encoder pipeline.

| Branch commit | Finding on main and disposition |
| --- | --- |
| `bbaefef823fd` | Cancellation latency harness absent. Recovered the harness and added the facade's `enough` dev dependency; measured current code on i265. |
| `701068995fb` | Encode-loop polls existed, but expensive post-filter searches/applications and phase boundaries still lacked cancellation. Added private cancellable variants for both bit depths, preserving public filter signatures; deterministic tests cancel at every observed checkpoint. |
| `38d9b4920f4` | Table properties and standalone types test import absent. Recovered five property tests in the consolidated types crate and the `alloc::vec::Vec` import. |
| `906eb4c8a2` | AVIF validation still erased dimensions, reasons and bad quality values. Restored structured payloads, non-exhaustive errors and error tests; adapted current animation callers and checked chroma-size arithmetic. |
| `c2aa84dad` | Mixed. SIMD permutation tests already have stronger non-vacuity guards (`21171ad62`); architecture-specific warnings were already addressed or their constants now have both ISA consumers. Removed the remaining dead reconstruction accessor, corrected current pipeline/dispatch/funnel comments, and excluded test fixtures from the refusal ledger. Added scanner regressions and tile-validation inventory coverage. |
| `1a701b75560` | Invalid superres/SB builders and impossible tile layouts could still panic. Builders now retain invalid inputs for encode-time errors, and tile validation precedes frame work. Added boundary tests and fixed an additional extreme-size shift overflow in the recovered validator. The commit only documented no_std failures; it did not fix them. |
| `b6b03d7299` | C performance-driver include paths already fixed by `bed839749`, with newer oracle handling in `609b4cf25`. Historical ARM benchmark data is superseded by current performance records, not transferable to this x86 host. |
| `a3e98a0f9` | 10-bit level re-encode functions remained embedded in the large pipeline. Extracted the five current function bodies byte-for-byte into private `pipeline/bd10_reencode.rs` (`4e1302966`), retaining subsequent fixes. |
| `7210c7e2d` | Historical evidence/documentation commit. This audit and the changelog replace its obsolete current-state claims. The three ARM native-10-bit disagreements are now explicitly handled by the ISA-aware gate and SUSPECTED-C-BUGS #9. |
| `4a68be7aa` | Cleanup dependent on the old extraction. The new extraction introduces no duplicate lint allowances. |

## Validation

All runs used this worktree's Rust artifacts and the pinned C reference
`3115c0c1b23e860dfd75c94f6740e0298182dd13` (HDR off), using the existing
matching oracle library read-only. Heavy jobs were serialized with the shared
`run-heavy --mem 16G --jobs 8` wrapper at niceness 19 on i265 (x86-64).

- Baseline: 2581 workspace nextest tests passed.
- Standalone types: `cargo test -p zenav1-svt-types --no-default-features` passed.
- Cancellation regressions: deterministic per-checkpoint coverage of CDEF,
  deblocking and restoration, search and application, at 8 and 10 bits.
- C byte identity: regression spotcheck 106/106, widened-10-bit matrix 36/36,
  partial-superblock 10-bit gate 159/159, with no pinned residuals in that gate.
- Refusal inventory: four parser regressions pass; regenerated inventory
  contains 64 shipping refusals, including the extracted tile validator.

- Final all-features nextest: **2603/2603 passed**, zero runner skips;
  all-features doctests: **2/2 passed**. Some optional C-internal capture
  tests lack local object files and retain their existing lower evidence tier.
- Native 10-bit source C gate: **118/118 byte-identical**; all **26/26**
  configurations have at least one quantizer whose low source bits affect output.
- Scoped workspace formatting and `git diff --check` pass.
- Final all-targets/all-features Clippy completes with existing warnings. The
  only diagnostic on semantically changed/extracted lines is the unchanged
  parenthesized `try_vec!` expression moved into `bd10_reencode.rs`; generated
  film-grain formatting retains its existing excessive-precision warnings.
  Strict Clippy is not clean. This is not an MSRV/ARM lint sweep.
- `just` is unavailable on this host; the new recipes' underlying test and
  benchmark commands were run directly.

## Cancellation measurement and limits

See `benchmarks/cancel_latency_*_2026-09-07_i265.tsv` and the companion metadata.
These are current measurements, not a before/after speedup claim. All
ask-before-return cells are included, including requests that returned success.
Missed asks are recorded separately rather than counted as cancellation.

Preset 6 worst observed return latency: 15.991 ms; preset 8: 13.741 ms;
preset 13: 9.244 ms. One preset-13 4096-square request was unhonoured in the
final return window. These sampled 8-bit still-image results do not establish
a universal 20 ms bound, nor prove animation or ARM cancellation latency.

## Remaining limitations not fixed by the branch

`cargo check -p zenav1-svt-encoder --no-default-features` still fails (59 errors
on this checkout, including missing `alloc::Vec` imports and ungated `std`
diagnostics). The no_std `intrabc::libm_exp` placeholder also remains. The old
branch documented this limitation; closing it must not be interpreted as
claiming encoder no_std support. The standalone types tests are supported.

Baseline strict Clippy already fails on an empty line after a doc comment in
`svtav1-cref/build.rs`; rustc also reports the existing public `pd0` function
exposing crate-private `Pd0Mode`. Neither is evidence of a new warning from
these changes. ARM performance and cross-ISA exceptions were source-audited,
not remeasured on i265.
