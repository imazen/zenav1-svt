# Video parity matrix — where the port equals C SVT, and where it does not

Living document (undated on purpose — update the rows as gaps close).
"As good as SVT for video in every case" decomposes into: every case C
encodes, we either (a) byte-match it, (b) decode-equivalently handle it
with a gate saying so, or (c) refuse with a named reason. Column "C does"
is the honest baseline; "we are" is measured state, not intent.

## Verified surfaces (gate-backed)

| Surface | Gate | What it proves |
|---|---|---|
| Recon == aomdec, 8-bit | `video_selfcheck_gate.sh` | 270 cells: 6 Derf clips x qp{20,40,55} x **p-1..13** @ 256x256, 8 frames, low-delay. Full preset ladder, decoder-verified. |
| Recon == aomdec, 10-bit | `bd10_video_selfcheck_gate.sh` | 396 cells, same shape at 10-bit. |
| Byte == C, inter 8-bit | `inter_byte_gate.sh` | 91 pinned witness cells ({uniform,gradient,diag,screen} x {16,64,72,128} x q{20,40,55} x p{6,8}, 2-frame low-delay P). OPEN list empty. |
| Byte == C, real clips | `real_video_inter_gate.sh` | 24 cells: 3 clips x {128,256}^2 x p{6,8} x 1/1 frames. |
| Byte == C, 10-bit | `bd10_video_gate.sh` | 24/24. |
| Byte == C, real video, every preset | `video_census_gate.sh` | Ratchet over 540 cells (6 derf clips x {128,256} x q{20,40,55} x p-1..13, 8 frames low delay): each cell's first differing temporal unit is pinned in `tools/pins/video_census.tsv` (244 IDENTICAL at the census; the pins hold the current count); earlier fails as regressed, later as promoted until the pin moves. |
| Byte == C, TPL and CBR | `rc_tpl_gate.sh` | TPL under random access 5/8 IDENTICAL, low-delay CBR 0/8 (key-frame qp); every verdict pinned, recon == aomdec == dav1d. |
| Key-frame video matrix | `video_key_matrix.sh` | Frame 0 of a 2-frame video encode vs C; default content is the five synthetic classes and presets 0, 3..13 (-1, 1, 2 are not in its default list). |
| Screen tools on video | `screen_ibc_gate.sh`, `screen_ibc_byte_gate.sh`, `screen_palette_gate.sh`, `screen_ibc_fh_gate.sh` | IntraBC + palette on screen-content video frames, incl. byte parity. |
| Inter encodability | `inter_completion_scan.sh` (SCAN_GATE) | sizes 64..2048 x p{6,8,10,13}: floor MIN_OK=52, cap MAX_REFUSED=12 — the frontier inventory of where inter encodes at all. |
| Inter byte matrix (sweep, not a gate) | `inter_byte_matrix.sh` | Any grid of synthetic classes and derf clips, any frame count; names the first temporal unit that differs from C. Real-video census 2026-09-26 (`benchmarks/video_parity_census_2026-09-26.meta`): 244/540 cells byte-identical through 8 frames (6 clips x {128,256} x q{20,40,55} x p-1..13, low delay). |
| qp0 coded-lossless inter, 8-bit 4:2:0 | `qp0_inter_gate.sh` | 7 cells: 4-frame encodes decode byte-identical to SOURCE via aomdec at p{0,6,13}, sb64+sb128, 128x128+256x256, with an inter-usage anti-vacuity leg. |
| Monochrome inter | `mono_inter_gate.sh` | recon == aomdec == dav1d (TWO independent decoders) on every frame — 13-px-shift synthetic legs x p{0,6,8,13} x 64x64..256x128 AND the unaligned defect table (65x64, 64x67, 65x67, 66x66, 70x64, 64x70, 96x100, 100x96, a 64x65 control, an unaligned 6-frame chain), sb64+sb128, bd10 legs, fourpeople clip; anti-vacuity legs require real inter blocks + nonzero MVs. Extension surface — no C oracle (C cannot encode mono). |
| Tool-specific | `warped_motion_gate.sh`, `obmc_gate.sh`, `global_motion_gate.sh`, `gm_join_gate.sh`, `inter_me_join_gate.sh`, `ifs_join_gate.sh` | warped motion, OBMC, GM coding+selection, GM derivation/model join vs C, ME join, IFS. |

## Where C works but we refuse or diverge — the video gap list

| Case | State | What closing it needs |
|---|---|---|
| Random-access / hierarchical GOPs | DONE at the RPS layer — all 9 of C's `av1_generate_rps_info` top-level branches translated (RTC-flat, LD-CQP/CRF, LD-CBR hier 1-2, RA-flat, RA hier 1..5 via `port_picstruct_ra`, incl. cut-short LD-in-RA arms; residual errors mirror C's own logged rejects). `hierarchical_levels <= 5` accepted at the API and picture decision runs for every `intra_period != 1` (0 = key-once-then-inter, byte-verified vs C `intra_period_length` ≥ clip on flat-LDP) | RA path is byte-identical on the unambiguous cells (IDENTITY-STATUS §2026-09-24); remaining gap is inter-MD decision tracking, not reference structure |
| TPL (temporal dependency) | Live under random access with `aq_mode` 2 (`scs_tpl`); `rc_tpl_gate.sh`: byte-identical to C on 5 of 8 synthetic cells, decoder-verified on all. `use_ref_frame_mvs` at mfmv_level >= 2 with TPL on still refuses (the header needs `r0` and the references' `is_mfmv_used`) — not reached by gradient 640x480 at presets -1..10 | close the 3 DIFFERS cells; measure real clips |
| CBR rate control, low delay | Live; `rc_tpl_gate.sh`: decoder-verified, 0/34 byte-identical — every cell diverges in the KEY frame's qp (C spends more bits) | port C's first-frame qp pick under one-pass CBR |
| VBR rate control | refused — the two-pass arm needs first-pass statistics; firstpass.c is not ported | port firstpass.c |
| Global motion *search* | DONE — `svt_aom_global_motion_estimation` ported (`port_global_me`): frame-level derivation joins C's `GMFRAME` fields and fitted models join `GMREF`/`wmmat` field-for-field (`gm_join_gate.sh`); models code through `write_global_motion`, GLOBALMV selects on fitted cells with recon == dav1d (`global_motion_gate.sh`). Refusal remains only where the search cannot run: a (list, ref) PA pyramid absent from `pa_slots`, or a non-GM_FULL downsample level (`GmSearchError` — unreachable while `set_gm_controls` assigns only GM_FULL) | exercised across presets -1..4 (the only presets where `derive_gm_level` is non-zero); RA/hierarchical lists and TPL-gated paths still belong to the GOP/TPL items below |
| qp0 coded-lossless on inter | DONE at 8-bit 4:2:0 (`qp0_inter_gate.sh`); still refused at 10-bit (per-block vs per-TXB intra pred divergence) and 4:4:4 (no inter WHT arm) | 10-bit: per-TXB intra pred on the inter-frame lossless path; 4:4:4: inter WHT residual arm on the non-funnel path |
| Mono inter | DONE on the measured envelope (`mono_inter_gate.sh`: recon == aomdec == dav1d, presets {0,6,8,13}, aligned AND unaligned sizes, sb64+sb128, bd10 legs, 6-frame chains, nonzero-MV + inter-usage anti-vacuity). The invalid-stream defect predated the format-agnostic inter landings. Mono qp0 inter still refused | mono qp0 inter needs an inter WHT residual arm on the lossless path |
| 4:4:4 inter | DONE on the measured envelope (`chroma_444_inter_gate.sh`, 47/47 vs aomdec — no C oracle); refused at qp0 / sb128 / 10-bit / IntraBC / film-grain | qp0 needs the inter WHT arm; the rest needs their kernels |
| 10-bit superres | DONE on stills (`superres_bd10_gate.sh`: byte == C + recon == aomdec at the upscaled size + anti-vacuity). Inter frames under superres refuse (reference geometry under a changing coded width is decoder-ungated); mono+superres refuses (no downscale arm on the mono entry); bd10+film-grain-denoise refuses (the u8 canvas exists only after the downscale, which runs after denoise) | inter+superres: decoder-gate the DPB/reference geometry, then a video superres gate |
| Partial-SB inter | `inter_completion_scan` frontier — some (size,preset) cells refuse; cap is 12 | cell-by-cell, driven by the scan's refused list |
| Sequence length | selfcheck runs 8 frames; longer GOP chains and scenecut-driven key insertion untested vs C | long-run differential gate |

## How to read "every case"

- **p{6,8} low-delay P, <=256x256**: byte-parity is pinned-witness strong.
- **Other presets, low-delay**: decoder-verified (selfcheck ladder), not
  byte-pinned. The 2026-09-26 real-video census measured them: presets 6,
  8, 9 are 32..34/36 byte-identical through 8 frames, 4/5/7/10 about half
  to two thirds, 0/2/3 diverge early in inter frames (5..7/36), and at
  presets 1 and -1 the KEY frame of a video encode already differs from C
  (0/36) although those presets are byte-identical as stills.
- **Everything in the gap list**: refused with a named reason — honest
  state, not a failure. A refusal moves to "verified" only with a gate
  in the same change.

## Measurement disciplines this matrix relies on

- Byte parity is claimed ONLY where a differential gate pins it;
  decoder-verified everywhere else (the two are never conflated).
- A refusal is a named state, never a silent skip — the refusal ledger
  (`REFUSED-CONFIGS.md`) is generated from the strings.
- Recon-vs-decoder checks assert *reconstruction equality*, which is
  stronger than "the stream parses".
