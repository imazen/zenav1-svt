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
| Key-frame video matrix | `video_key_matrix.sh` | p0..13 on real clips. |
| Screen tools on video | `screen_ibc_gate.sh`, `screen_ibc_byte_gate.sh`, `screen_palette_gate.sh`, `screen_ibc_fh_gate.sh` | IntraBC + palette on screen-content video frames, incl. byte parity. |
| Inter encodability | `inter_completion_scan.sh` (SCAN_GATE) | sizes 64..2048 x p{6,8,10,13}: floor MIN_OK=52, cap MAX_REFUSED=12 — the frontier inventory of where inter encodes at all. |
| Inter byte matrix | `inter_byte_matrix.sh` | q{20,40,55} x p{6,8} configurable. |
| qp0 coded-lossless inter, 8-bit 4:2:0 | `qp0_inter_gate.sh` | 7 cells: 4-frame encodes decode byte-identical to SOURCE via aomdec at p{0,6,13}, sb64+sb128, 128x128+256x256, with an inter-usage anti-vacuity leg. |
| Monochrome inter | `mono_inter_gate.sh` | 12 cells: recon == aomdec == dav1d (TWO independent decoders) on every frame — 13-px-shift synthetic legs x p{0,6,8,13} x 64x64..256x128, sb64+sb128, 6-frame chain, bd10 leg, fourpeople clip; anti-vacuity legs require real inter blocks + nonzero MVs. Extension surface — no C oracle (C cannot encode mono). |
| Tool-specific | `warped_motion_gate.sh`, `obmc_gate.sh`, `global_motion_gate.sh`, `gm_join_gate.sh`, `inter_me_join_gate.sh`, `ifs_join_gate.sh` | warped motion, OBMC, GM *signaling* (not search), ME join, IFS. |

## Where C works but we refuse or diverge — the video gap list

| Case | State | What closing it needs |
|---|---|---|
| Random-access / hierarchical GOPs | `port_picstruct::generate_rps_info` translates 4 of C's 8 pred-struct branches; `hierarchical_levels <= 5` accepted at the API but picture decision only runs when `intra_period > 1` | port remaining pred-struct branches + RA reference management; largest single video item |
| TPL (temporal dependency) | structurally off — `aq_mode` forced 0; `use_ref_frame_mvs` at mfmv>=2 needs TPL r0 | port TPL + r0 plumbing; unblocks aq_mode != 0 and MFMV>=2 |
| VBR/CBR rate control | refused — `target_bitrate` read nowhere; C ports exist unwired (`port_rc_vbr_cbr*`, `port_rc_rtc_cbr`, `port_pass2_gop`) | wire the ported RC stack + lookahead |
| Global motion *search* | refused — `global_me.c:190` search unported; FH writer IS ported (`write_global_motion`) | port `svt_aom_global_motion_estimation` + the picture-analysis reference it reads |
| qp0 coded-lossless on inter | DONE at 8-bit 4:2:0 (`qp0_inter_gate.sh`); still refused at 10-bit (per-block vs per-TXB intra pred divergence) and 4:4:4 (no inter WHT arm) | 10-bit: per-TXB intra pred on the inter-frame lossless path; 4:4:4: inter WHT residual arm on the non-funnel path |
| Mono inter | DONE on the measured envelope (`mono_inter_gate.sh` 12/12: recon == aomdec == dav1d, presets {0,6,8,13}, sb64+sb128, bd10 leg, 6-frame chain, nonzero-MV + inter-usage anti-vacuity). The invalid-stream defect predated the format-agnostic inter landings. Mono qp0 inter still refused | mono qp0 inter needs an inter WHT residual arm on the lossless path |
| 4:4:4 inter | DONE on the measured envelope (`chroma_444_inter_gate.sh`, 47/47 vs aomdec — no C oracle); refused at qp0 / sb128 / 10-bit / IntraBC / film-grain | qp0 needs the inter WHT arm; the rest needs their kernels |
| 10-bit superres | DONE on stills (`superres_bd10_gate.sh`: byte == C + recon == aomdec at the upscaled size + anti-vacuity). Inter frames under superres refuse (reference geometry under a changing coded width is decoder-ungated); mono+superres refuses (no downscale arm on the mono entry); bd10+film-grain-denoise refuses (the u8 canvas exists only after the downscale, which runs after denoise) | inter+superres: decoder-gate the DPB/reference geometry, then a video superres gate |
| Partial-SB inter | `inter_completion_scan` frontier — some (size,preset) cells refuse; cap is 12 | cell-by-cell, driven by the scan's refused list |
| Sequence length | selfcheck runs 8 frames; longer GOP chains and scenecut-driven key insertion untested vs C | long-run differential gate |

## How to read "every case"

- **p{6,8} low-delay P, <=256x256**: byte-parity is pinned-witness strong.
- **Other presets, low-delay**: decoder-verified (selfcheck ladder), not
  byte-pinned — the differential witness set doesn't cover p-1..13 except
  where `inter_byte_matrix`/`real_video_inter_gate` rows exist.
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
