# Lossless partition and MD context parity — 2026-09-07

Local x86_64 verification against the in-tree C reference
`3115c0c1b23e860dfd75c94f6740e0298182dd13`, still I420 8-bit
CQP, flat-128 chroma, single tile and SB64. No push or CI run.

The old 32 self-promoting lossless pins were missing C translation/wiring.
All original 144 cells now match C bytes and decode to the source under aomdec.
The gate still checks QP-0 versus QP-1 anti-vacuity on textured inputs.

| Witness | Before the relevant correction | C and final port |
|---|---:|---:|
| gradient 64x64 QP0 p3, fixed 8x8 leaves | 2966 bytes | 2973 bytes |
| same, PD0 tree without PD1 | 2970 bytes | 2973 bytes |
| diag 64x64 QP0 p3, preset depth pruning | 1391 bytes | 1268 bytes |
| gradient 128x128 QP0 p0, no lossless MD tx-type update | 9560 bytes | 9567 bytes |

C probes establish the intermediate values:

- Gradient64 p3: all 320 PD0 candidate cost rows match (distortion,
  coefficient bits, full cost and lambda). Diag64 p3: all 263 rows match.
- `md_config_process.c` sets lossless depth-refinement level 0,
  `PD0_DEPTH_NO_RESTRICTION`. It still performs PD1 depth decisions.
- `rd_cost.c` adapts transform-type probabilities in the MD context at
  QP0, despite the real entropy writer omitting those symbols. After wiring
  that update, all four gradient128 p0 MD seed rows match C's captured
  partition, mode, angle, skip, CfL and transform-type values.

Three full C streams are committed in `crates/svtav1-encoder/tests/data/`;
`lossless_fh_c_capture.rs` checks bytes and encoder reconstruction. Three
new spotchecks use the live C encoder and guard the distinct missing paths.

Local logs and intermediate traces:
`~/tmp/animation-metadata/lossless-{p3,diag,g128}-drill/`,
`lossless-refined-{c-gate-final,nextest,spotcheck,full8}.log` in the same
animation-metadata directory. The original no-pruning-override matrix
closed 14 pins; adding the override closed 16 more; the MD CDF correction
closed the final two. No lossless source-pixel failure was observed.

Verification: 2,598/2,598 workspace nextest cases (zero skips), 115/115
regression spotchecks, and 1,100/1,100 full 8-bit byte comparisons (zero pins
or harness errors). These complement the 144/144 lossless byte/source gate.

Workspace clippy completes with existing warnings; scoped formatting and diff
checks pass. The refusal inventory remains at 48 entries. Both repository
fetches reported no changes; no push or CI run was triggered.
