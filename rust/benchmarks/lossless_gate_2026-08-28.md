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
match everywhere. The two C-side knobs that flip exactly at that boundary:
`svt_aom_get_disallow_4x4_default` (4x4 partitions allowed at <= M2, so
C's PD0_LVL_0 decides 8x8-vs-four-4x4 at lossless, with an 8x8 DCT light
encode that the WHT arm does not reach) and `svt_aom_get_bypass_encdec_allintra`
(0 at <= M3). Both port and C streams decode losslessly at every pinned cell
(checked cell-by-cell by the gate; gradient 64x64 p3 also checked by hand
before the pin list was written).
