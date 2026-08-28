# Coded-lossless gate — first full-ladder measurement, 2026-08-28

`tools/lossless_gate.sh` (issue #5 chunk 2), run locally on the aarch64
laptop against the cargo-built C oracle (`Bin/Release`, `SVT_CREF_LIB_DIR`),
`aomdec` from PATH, at the change that landed the tile half (parent
`f7abcff5`). Three runs covering the whole default grid — 3 contents x
4 geometries x 12 presets = 144 cells:

| run | presets | result | wall |
|---|---|---|---|
| a | 6 7 8 9 10 13 | 72 / 72 byte-identical, 72/72 lossless | 1:35 |
| b | 4 5 (from the 0..5 run) | 24 / 24 byte-identical, lossless | (part of 1:46) |
| c | 0 1 2 3 (with pins) | 16 / 48 byte-identical (uniform) + 32 pinned-diverging, 48/48 lossless | 1:08 |

**Total: 112 / 144 byte-identical, 32 pinned, 144 / 144 decode to the source
under aomdec.** No anti-vacuity failure (every textured qp-0 stream differs
from its qp-1 twin).

The 32 pinned cells are exactly {gradient, diag} x {64x64, 128x128, 96x80,
200x136} x presets {0, 1, 2, 3}. Byte sizes (port vs C) from run b, before
the pin list existed:

| cell | port B | C B |
|---|---:|---:|
| gradient 64x64 p0 / p1 / p2 / p3 | 2862 / 2940 / 2940 / 2966 | 2854 / 2965 / 2965 / 2973 |
| gradient 128x128 p0 / p1 / p2 / p3 | 10067 / 9652 / 9652 / 10053 | 9567 / 9505 / 9505 / 9538 |
| gradient 96x80 p0 / p1 / p2 / p3 | 5113 / 5222 / 5222 / 5317 | 5047 / 5194 / 5194 / 5192 |
| gradient 200x136 p0 / p1 / p2 / p3 | 15096 / 15411 / 15411 / 15834 | 14992 / 15374 / 15374 / 15435 |
| diag 64x64 p0 / p1 / p2 / p3 | 312 / 1344 / 1344 / 1391 | 341 / 1264 / 1264 / 1268 |
| diag 128x128 p0 / p1 / p2 / p3 | 624 / 4664 / 4664 / 4754 | 654 / 4499 / 4499 / 4499 |
| diag 96x80 p0 / p1 / p2 / p3 | 493 / 2377 / 2377 / 2455 | 494 / 2277 / 2277 / 2279 |
| diag 200x136 p0 / p1 / p2 / p3 | 882 / 7569 / 7569 / 7706 | 856 / 7319 / 7319 / 7319 |

Observations that bound the next chunk: p1 and p2 are the same configuration
on both sides (identical sizes); p0 and p3 differ from them; presets >= 4
match everywhere. Localization on gradient 64x64: the PORT's p3 stream is
byte-identical to its own p4 stream (2966 B) and to C's p4, while C's p3
(2973 B) differs from C's p4 from the first coded symbol of the tile (payload
byte 4, after the 4-byte lossless frame header) — so a C knob that is live
only at qp 0 separates M3 from M4. Of the all-intra knobs that flip at M3,
the only one live under `mimic_only_tx_4x4` on an I-slice is
`svt_aom_get_disallow_4x4_allintra` (4x4 partitions allowed at <= M3): C's
lossless partition search decides 8x8-vs-four-4x4, the port forces 8x8.
`bypass_encdec` (also 0 at <= M3) was ruled out — EncDec re-runs with the
same lossless wrapper and its MD-side sites are routing / qp-0-inert resets.
Both port and C streams decode losslessly at every pinned cell (checked
cell-by-cell by the gate; gradient 64x64 p3 also checked by hand before the
pin list was written).

Real-content spot check (CID22-512 crops, `crop:` content, same host/oracle):

| cell | result |
|---|---|
| 1001682 + 4666751 x {64x64, 512x512} x {p7, p12} | 8 / 8 byte-identical, 8/8 lossless |
| 1001682 + 4666751 x {64x64, 512x512} x p1 | 0 / 4 byte-identical (port 3930/184662/4095/146304 B vs C 3924/184196/4096/145575 B), 4/4 lossless |

The issue's original sweep cells (64x64 presets 1 / 7 / 12 on CID22 photos)
are therefore all lossless now; the p1 byte delta is the pinned class above.

Negative result, same day: a PD0_LVL_0 lossless pick (`max_sq` 8 / `min_sq` 4,
DCT-8x8 vs WHT-4x4 light encodes at `qindex + 8`, closed-form rate) moved every
pinned cell without closing one — gradient 64x64 p0/p1/p2/p3 2949/2969/2969/2979 B
(C 2854/2965/2965/2973), diag 64x64 p0/p1/p2/p3 558/1366/1366/1391 B (C
341/1264/1264/1268), all still lossless. Reverted; the C-side PD0 cost dump is
the next step (rust/CLAUDE.md, "FIXED 2026-08-28 — coded-lossless").
