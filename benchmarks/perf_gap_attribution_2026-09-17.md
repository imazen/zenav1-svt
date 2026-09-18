
## 2026-09-18 real-content follow-up: palette/CRC/sc-detect/IntraBC-variance/pd0 arms

Continued NEON work measured on REAL content (gb82-sc terminal.png screen
capture + clic2025 photo, 512x512, interleaved A/B vs the pushed parent):

| commit | kernel | measured |
|---|---|---|
| 8f576ff7 | `palette::calc_indices_dim1` NEON (nearest-centroid argmin, strict-< first-index ties) | **1.023x** screen p4, 1.000x photo — byte-identical |
| 4de02f15 | `intrabc_hash` CRC-32C via `neon_crc` (`__crc32cw` x4, table-identical) | **1.024x** screen p4, neutral photo — byte-identical |
| 59f2843a | `sc_detect::dilate_block` NEON twin of the port-local v3 mask trick + `variance_about_128` NEON (restores C's RTCD `fn_ptr->vf` SIMD reach) | 1.000x screen p4 (detector ~1.6% of encode — below noise floor); kept as coverage alignment |
| 0e48929d | `intrabc::variance_of_diff` NEON (`mefn_vf` — cross-stage DV metric) | 1.000x screen p4 (~0.3% path); kept as coverage alignment |
| 90c7f2e2/797968fe | `pd0::b64_stats` NEON twin of the v3 row-pair shape (SB variance map was `[v3, scalar]`) | **1.032x** screen p10 (bands 0.921-0.994) — byte-identical |

After these, every DSP symbol in the 512p10 screen profile's tail already
carries a NEON arm or is malloc/memmove/entropy driver. Remaining hot
names are structural (`write_coeffs_txb_1d`, `optimize_b`, `tx_unit_inner`,
`inject_candidates`, `eval_candidate`) — the x86-shaped pipeline layer.
