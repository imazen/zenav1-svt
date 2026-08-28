# Per-gate wall clock on CI — measured 2026-08-27

Issue #8 item 6 asked for a wall-clock budget per gate; nothing had measured
one. This is every step of the `differential + conformance gates (x86-64)`
job of run [33101031800](https://github.com/imazen/zenav1-svt/actions/runs/33101031800)
(commit `1ed7db46`, `ubuntu-latest`, 4 vCPU, shared runner — treat +-20 % as
noise), taken from the job's `steps[].started_at/completed_at` via
`gh api repos/imazen/zenav1-svt/actions/jobs/98618463917`. Job total 21m06s.

Re-measure the same way after any gate change; a local arm64 run is a
different host and must be recorded as such (`benchmarks/*.meta`).

| step | seconds |
|---|---:|
| Set up job | 2 |
| Run actions/checkout@v5 | 6 |
| Install toolchain deps | 25 |
| Run dtolnay/rust-toolchain@stable | 1 |
| Run Swatinem/rust-cache@v2 | 3 |
| Install cargo-nextest | 1 |
| Build C reference (static, the differential oracle) — cargo-driven | 141 |
| Cache libaom build (aomdec oracle) | 1 |
| Build aomdec (reference decoder) | 0 |
| Workspace tests (differential parity suites vs C) | 167 |
| Decode conformance — mono (aomdec + dav1d) | 61 |
| Decode conformance — 4:2:0 (aomdec + dav1d) | 59 |
| bd10 identity (uniform, port == real aomenc at bit depth 10) | 13 |
| bd10 non-flat identity (u16 MD path, DC-family cells) | 62 |
| bd10 NATIVE 10-bit source identity (task | 20 |
| SIMD tier invariance (encoder bytes must not depend on dispatch tier) | 207 |
| peak-RSS memory sweep (port vs C, tiny -> large) | 32 |
| refusal inventory is current (capability debt stays visible) | 0 |
| PORT-NOTE index is current (verification-debt ledger cannot drift) | 0 |
| regression spot-check (one cell per bug ever fixed) | 9 |
| 8-bit identity — EVERY preset 0..13, full qp range, 4 content classes | 48 |
| screen-content palette identity (bd8 AND bd10) | 15 |
| partial-SB / odd-dimension identity (8-bit) | 30 |
| bd10 partial-SB identity (10-bit at non-64-aligned dims) | 34 |
| Alignment / stride gate (true-vs-aligned x stride x bit depth) | 36 |
| tile identity — rows AND columns (8-bit) | 28 |
| SB128 identity — allintra M0/M1 above 240p (8-bit) | 74 |
| arbitrary-size robustness — panic-free + decodable | 56 |
| superres identity + conformance (chunk B.3) | 104 |
| Recon parity (encoder recon == aomdec output, byte-exact) | 25 |
| Variance-boost delta-q recon parity (mainline, tune IQ) | 2 |
| Complete job | 0 |

Notes:
- "Build C reference" (141 s) is the cost the 2026-08-28 `actions/cache` step
  removes on a submodule-SHA hit (issue #4 invariant C). MEASURED on the
  first run that could hit (33155232145, the third push after the cache
  landed — the first two runs overlapped and both missed): cache restored,
  "Build C reference" 9 s (08:25:10 -> 08:25:19), i.e. 132 s saved per run.
- The pure-Rust matrix jobs in the same run: windows-11-arm 7m23s,
  macos-15-intel 5m02s, i686 via cross 1m55s (facade tests only, no C).
- Corpus-gated gates (`photo_p0_gate`, `bd10_photo_gate`, `screen_*`,
  `real_image_matrix`, `coverage_combos` axis 3, `hdr_bd10_gate`) do not run
  here and have no measured budget yet; when one is run locally, record wall
  time in its `.meta`.
