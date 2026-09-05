# Per-gate wall clock on CI — re-measured 2026-09-05 (the screen gates wired in)

Supersedes `gate_wallclock_ci_2026-08-27.md` for the step list; that file's
notes on the C-oracle cache still hold. This is every step of the
`differential + conformance gates (x86-64)` job of run
[33957277464](https://github.com/imazen/zenav1-svt/actions/runs/33957277464)
(commit `c3f0e394`, `ubuntu-latest`, 4 vCPU, shared runner — treat +-20 % as
noise), taken from `steps[].started_at/completed_at` via
`gh api repos/imazen/zenav1-svt/actions/jobs/101282776418`.
**Job total 41m00s** (09:10:54Z -> 09:51:54Z), against 21m06s on 2026-08-27
and 29m10s on the immediately preceding run (33953651200).

WHY IT MOVED: this is the run that first fetched the gb82-sc corpus and ran
the three screen gates (docs/INTER-ENCODE-PLAN.md §1z⁴⁰). They cost
**894 s = 14m54s** of the job, and the corpus fetch itself costs **1 s**.
`timeout-minutes` was raised 90 -> 120 to carry them.

| step | seconds |
|---|---:|
| Set up job | 2 |
| Run actions/checkout@v7 | 5 |
| Install toolchain deps | 11 |
| Fetch the gb82-sc screen corpus (sparse checkout, 2.9 MB) | 1 |
| Run dtolnay/rust-toolchain@stable | 11 |
| Run Swatinem/rust-cache@v2 | 2 |
| Install cargo-nextest | 1 |
| C oracle cache key (submodule SHA + build.rs) | 0 |
| Cache the cargo-built C oracle (both variants) | 1 |
| Build C reference (static, the differential oracle) — cargo-driven | 5 |
| Cache libaom build (aomdec oracle) | 1 |
| Build aomdec (reference decoder) | 0 |
| Workspace tests (differential parity suites vs C) | 265 |
| Decode conformance — mono (aomdec + dav1d) | 83 |
| Decode conformance — 4:2:0 (aomdec + dav1d) | 49 |
| bd10 identity (uniform, port == real aomenc at bit depth 10) | 22 |
| bd10 non-flat identity (u16 MD path, DC-family cells) | 52 |
| bd10 NATIVE 10-bit source identity (task | 22 |
| SIMD tier invariance (encoder bytes must not depend on dispatch tier) | 170 |
| peak-RSS memory sweep (port vs C, tiny -> large) | 89 |
| refusal inventory is current (capability debt stays visible) | 0 |
| PORT-NOTE index is current (verification-debt ledger cannot drift) | 0 |
| regression spot-check (one cell per bug ever fixed) | 43 |
| INTER byte gate (both frames of a 2-frame low-delay P encode) | 27 |
| INTER completion gate (does the port ENCODE the cell AT ALL) | 59 |
| INTER decode census (does the port's stream DECODE, on all 96 cells) | 16 |
| INTER ME join gate (the port's full-pel MV against C's, per block) | 1 |
| INTER frame-context + header + decode gates | 1 |
| 8-bit identity — EVERY preset 0..13, full qp range, 4 content classes | 32 |
| screen-content palette identity (bd8 AND bd10) | 12 |
| screen IntraBC byte gate — gb82-sc x presets 0..4 x qp (152 cells) | 551 |
| screen-content luma palette byte gate — gb82-sc x preset 6 (50 cells) | 52 |
| bd10 screen panic-freedom + decodability (gb82-sc, 60 cells) | 291 |
| partial-SB / odd-dimension identity (8-bit) | 25 |
| bd10 partial-SB identity (10-bit at non-64-aligned dims) | 27 |
| Alignment / stride gate (true-vs-aligned x stride x bit depth) | 33 |
| tile identity — rows AND columns (8-bit) | 30 |
| SB128 identity — allintra M0/M1 above 240p (8-bit) | 79 |
| arbitrary-size robustness — panic-free + decodable | 44 |
| feature-combination identity — SB128 x tiles, bd10 x tiles (8-bit + 10-bit) | 202 |
| coded-lossless identity + lossless decode (QP 0, issue | 32 |
| superres identity + conformance (chunk B.3) | 83 |
| Recon parity (encoder recon == aomdec output, byte-exact) | 24 |
| Variance-boost delta-q recon parity (mainline, tune IQ) | 1 |
| Post Cache libaom build (aomdec oracle) | 0 |
| Post Cache the cargo-built C oracle (both variants) | 0 |
| Post Run Swatinem/rust-cache@v2 | 0 |
| Post Run actions/checkout@v7 | 0 |
| Complete job | 1 |

Sum of steps: 2458 s.

The three steps this run added (all NEW to CI — every one of them had zero
coverage here before, see §1z⁴⁰):

| new step | seconds | cells | its own verdict line |
|---|---:|---:|---|
| Fetch the gb82-sc screen corpus (sparse checkout) | 1 | 10 PNGs | `gb82-sc PNGs fetched: 10 -> .../corpora/codec-corpus/gb82-sc` |
| screen IntraBC byte gate | 551 | 152 | `screen_ibc_byte_gate: 152 / 152 byte-identical, 0 diverging, 0 errors; port IBC blocks: 41559, palette blocks: 27402; recon legs: 0 bad, 0 skipped` |
| screen-content luma palette byte gate | 52 | 50 | `screen palette gate (preset 6 bd8): 50 / 50 byte-identical  (palette-coding cells: 38)` |
| bd10 screen panic-freedom + decodability | 291 | 60 | `bd10 screen panic-freedom: 60 / 60 encode-without-panic + decodable` |

Steps whose CELL COUNT changed because the corpus is now present:

| step | before | after |
|---|---|---|
| regression spot-check | `98 / 98`, six gb82-sc cells listed under "SKIPPED (corpus/tool absent — these cells guarded NOTHING this run)" | `regression spot-check: 104 / 104`, no SKIPPED section |
| SB128 identity | `sb128 gate: 18 / 18` (codec_wiki cells dropped with a warning) | `sb128 gate: 22 / 22` |
| `tier_invariance::intrabc_output_is_tier_invariant_on_real_screen_content` | skipped by workflow-scope `ZENAV1_SKIP_CORPUS_TESTS` on every run | runs, `... ok` (42.4 s under nextest, ~27 s in the dedicated step) |

Notes:
- "Build C reference" is 5 s here (cache hit on the submodule SHA), not the
  141 s of a cold build — same invariant the 2026-08-27 file records.
- The pure-Rust matrix jobs in the same run: windows-11-arm 5m57s,
  macos-15-intel 7m11s, i686 via cross 2m08s. All three still set
  `ZENAV1_SKIP_CORPUS_TESTS=1` at JOB scope (they have no C oracle and no
  corpus); it is no longer set at workflow scope, which is what let the
  `gates` job run the IntraBC tier cell.
- Still not in CI and still without a measured CI budget: `photo_p0_gate`,
  `bd10_photo_gate`, `bd10_hbd_pq_gate`, `real_image_matrix`,
  `coverage_combos` axis 3 (all need CID22-512, 94 MB, which is NOT fetched)
  and `screen_ibc_gate` (needs `tools/decode_diff`, whose Cargo.toml has a
  literal path dep on `/root/aom-rs/crates/aom-decode`).
- Re-measure the same way after any gate change.
