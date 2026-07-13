# Bitstream Identity Status — Rust `EncodePipeline` vs C SVT-AV1

Date: 2026-07-13 · branch `wave2/entropy-c-parity` · C baseline: in-tree
v4.2.0-rc `Bin/Release/libSvtAv1Enc.a` · goal gate: **byte-identical streams
at matched configs** (still-picture/AVIF, CQP, `--lp 1`).

This document is the divergence map of the identity campaign. **2026-07-13
update: the campaign's first byte-identical stream landed** — uniform 64x64
CLI-qp 40 preset 13 prints `VERDICT: streams IDENTICAL` (exit 0): all 22
bytes equal (TD+SH+FH+tile) and all 5 arithmetic-coder ops equal including
rng state. Fixes that got there, each its own commit:

1. `d72a76411` — CQP means CQP: frame-level VAQ/TPL gated behind
   `RcConfig.aq_mode` (default 0 = off, matching C `--aq-mode 0`). Killed
   divergence F1 and its whole FH cascade (F2).
2. `5e5c222cb` — SH parity: C-exact `seq_level_idx` auto-derivation
   (set_bitstream_level_tier port, fps-aware) + CICP-unspecified defaults
   (cp/tc/mc 2/2/2, `color_description_present_flag=0`, studio range) +
   `color_range` honors `ColorDescription::full_range` + full-SH `seq_tier`
   only for level idx > 7. Killed S1-S3; SH byte-identical.
3. `752790777` — TX_MODE_SELECT + C-exact per-block tx_depth symbol
   (`tx_size_cdf[cat][ctx]`, get_tx_size_context port with TXFM neighbor
   arrays). Killed F3, the last structural tile-syntax gap.
4. `85d7e0fd1` — real entropy costs for partition symbols (av1_prob_cost /
   av1_cost_symbol port, DEFAULT partition CDFs, NONE priced + rescored
   with the same lambda formula as the candidates). Killed the op-0
   partition divergence for the flat-content case: uniform 64x64 picks
   PARTITION_NONE like C at every lambda.

**2026-07-13 later the same day: preset-6 landed** (commits `084d2c13e` +
`eab9d8860`) — every uniform cell is now byte-identical at ALL THREE
tracked presets (18/36 matrix cells, was 12/36; the SH divergence stage is
gone):

5. `084d2c13e` — entropy-layer plumbing: SH writer takes
   `SeqTools {enable_filter_intra, enable_restoration}`; FH writer emits
   spec 5.9.20 lr_params (NumPlanes x 2-bit `lr_type` = RESTORE_NONE, no
   unit-size bits — C encode_restoration_mode, entropy_coding.c:2243);
   FrameContext gains `filter_intra_cdfs` (generated defaults,
   drift-tested) + `write_use_filter_intra` + `block_size_index`.
6. `eab9d8860` — C-exact allintra derivations threaded from the preset:
   `seq_tools_for_preset` (filter_intra get_filter_intra_level_allintra
   enc_mode_config.c:12679 / restoration
   svt_aom_get_enable_restoration_allintra :3944 — both ON for M0..M6,
   OFF M7+), the per-block `use_filter_intra` flag for eligible DC blocks
   <= 32x32 (svt_aom_filter_intra_allowed, mode_decision.c:102-108;
   write position entropy_coding.c:5098-5112: after uv/palette, before
   tx_depth), and the CDEF policy split: allintra presets <= M6 run C's
   search (cdef_search_level 7 at M6) — of which the deterministic
   sb_count==0 outcome is ported (all filter blocks skip -> cdef_bits=0,
   strengths 0/0, damping 3+(q>>6); enc_cdef.c:1296-1449) — while
   non-all-skip frames at search presets keep the qp fast path (gap 2a,
   narrowed).

Killed S4, S5, and the M6 half of the FH/cdef class for all-skip frames.
Remaining divergences at the bottom of this doc. The original first-map
analysis below is kept for the record (the per-config reports show the
PRE-fix state).

## The harness

Three parts, all committed:

1. **C-side symbol capture** — `tools/capture_c_trace/` (`capture_c_trace.c`
   + `wrap_odec.c` + `build.sh`; built on demand, NEVER part of the cargo
   workspace). Drives the public API (`svt_av1_enc_init_handle` →
   `set_parameter` → `init` → one `send_picture` + EOS → `get_packet` drain)
   with the repo's canonical matched config (same knobs the perf gate passes
   to SvtAv1EncApp: `--rc 0 --aq-mode 0 --qp Q --avif 1 --lp 1 -n 1`), and
   links with `-Wl,--wrap=` around the COMPLETE od_ec encode surface of
   `bitstream_unit.h` (v4.2): `svt_od_ec_encode_cdf_q15`,
   `svt_od_ec_encode_bool_q15`, `svt_od_ec_encode_bool_eq_q15`, plus
   `enc_init`/`enc_reset`/`enc_done` markers. Every arithmetic-coder op the
   library performs lands in `$SVT_TRACE_OUT` with the pre-op coder range
   (`rng=`) as a state checksum. Verified non-LTO archive with global `T`
   symbols, so `--wrap` intercepts every cross-TU call; the three encode
   functions are self-contained in `bitstream_unit.c` (no intra-TU chains to
   miss). Header bits (SH/FH) go through the `AomWriteBitBuffer` path, not
   od_ec — those are compared at the byte/field level instead.
2. **Rust-side trace** — the `symtrace` feature now logs at the exact seam C
   wraps (`OdEcEnc::encode_cdf_q15` / `encode_bool_q15` in
   `svtav1-entropy/src/range_coder.rs`), same line format, same `rng=`
   checksum, plus `W RESET`/`W DONE` markers. New `svtav1` passthrough
   feature `symtrace`; new runner `svtav1/examples/identity_run.rs` writes
   the shared `.yuv` input (both encoders consume identical bytes), the
   `.obu` stream, and the trace on stderr.
3. **Differ** — `tools/identity_diff.py`: OBU-level byte comparison with
   field-level decode of the reduced-still SH and key-frame FH (names the
   exact field at the first differing bit, both sides walked independently),
   tile-payload isolation, and canonicalized op-trace diff (first divergence
   + context + op-kind histogram). Canonicalization unifies
   arithmetic-identical encodings: C `BOOLEQ v` ≡ `BOOL v f=16384`
   (`aom_write_bit`), and C `BOOL v f` ≡ Rust `CDF nsyms=2 s=v icdf=[f]`
   (C's `aom_write_symbol` routes nsyms==2 through the bool coder with
   `f = cdf[0]`, `bitstream_unit.h:265-271` — provably the same arithmetic;
   Rust keeps the generic CDF path). `rng` equality is enforced alongside op
   equality: matching ops with drifting rng would expose an op that escaped
   one trace.

Run: `tools/identity_diff.sh <w> <h> <cli_qp> <preset> [uniform|gradient]`
(or `just identity ...`). Artifacts + `report.txt` land in
`target/identity/<case>/`. Exit 0 iff streams are byte-identical.

### Matched-config mapping

| C API field (capture_c_trace)   | value      | Rust equivalent                          |
|---------------------------------|------------|------------------------------------------|
| `enc_mode`                      | preset     | `EncodePipeline::new(.., preset, ..)`     |
| `rate_control_mode`             | 0          | `RcMode::Cqp`                             |
| `aq_mode`                       | 0          | `RcConfig.aq_mode` (default 0 = off, C-matched since d72a7641) |
| `qp` (CLI 0..63)                | qp         | `RcConfig.qp`                             |
| `avif`                          | true       | `intra_period=1` → reduced-still SH       |
| `level_of_parallelism`          | 1          | single-threaded                           |
| `encoder_color_format`          | EB_YUV420  | `.with_chroma_420(true)` + `encode_frame_420` |
| `encoder_bit_depth`             | 8          | `bit_depth = 8`                           |
| `frame_rate` num/den            | 30/1       | n/a (no timing info in reduced SH)        |

Content: `uniform` = y 128 everywhere; `gradient` = y[r][c] =
`((r*255/h) ^ ((c*3) & 0x3f))`; u = v = 128 for both. 64x64, single SB,
single tile.

## Per-config reports (verbatim)

### uniform 64x64 q40 p13

```
==============================================================================
OBU-LEVEL COMPARISON   C=22B  Rust=25B
==============================================================================
  C stream:    [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 6), ('FRAME', 10)]
  Rust stream: [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 9), ('FRAME', 10)]
  streams byte-identical: False

[OBU 0] TEMPORAL_DELIMITER: C 0B, Rust 0B -> IDENTICAL

[OBU 1] SEQUENCE_HEADER: C 6B, Rust 9B -> DIFFERS
    SEQUENCE_HEADER field walk (C | Rust):
      DIFF #3: C @5    seq_level_idx[0]=0                     | R @5    seq_level_idx[0]=8
      DIFF #16: C @38   color_description_present_flag=0       | R @38   color_description_present_flag=1
      DIFF #17: C @39   color_range=0                          | R @39   color_primaries=1
      DIFF #18: C @40   chroma_sample_position=0               | R @47   transfer_characteristics=13
      DIFF #19: C @42   separate_uv_delta_q=0                  | R @55   matrix_coefficients=1
      DIFF #20: C @43   film_grain_params_present=0            | R @63   color_range=1
      DIFF #21: C (end)                                        | R @64   chroma_sample_position=0
      DIFF #22: C (end)                                        | R @66   separate_uv_delta_q=0
      DIFF #23: C (end)                                        | R @67   film_grain_params_present=0
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [18] 15 7f fc 20 08
      Rust: [1a] 15 7f fc 22 02 1a 03
      C field at that bit: seq_level_idx[0]=0 (bits 5..9)
      Rust field at that bit: seq_level_idx[0]=8 (bits 5..9)

[OBU 2] FRAME: C 10B, Rust 10B -> DIFFERS
    FRAME field walk (C | Rust):
      DIFF #4: C @4    base_q_idx=160                         | R @4    base_q_idx=152
      DIFF #11: C @18   loop_filter_level[0]=19                | R @18   loop_filter_level[0]=16
      DIFF #12: C @24   loop_filter_level[1]=19                | R @24   loop_filter_level[1]=16
      DIFF #13: C @30   loop_filter_level[2]=9                 | R @30   loop_filter_level[2]=8
      DIFF #14: C @36   loop_filter_level[3]=9                 | R @36   loop_filter_level[3]=8
      DIFF #19: C @50   cdef_y_pri_strength[0]=3               | R @50   cdef_y_pri_strength[0]=2
      DIFF #21: C @56   cdef_uv_pri_strength[0]=3              | R @56   cdef_uv_pri_strength[0]=2
      DIFF #23: C @62   tx_mode_select=1                       | R @62   tx_mode_select=0
    FH decoded length: C 64 bits (8B), Rust 64 bits (8B)
    tile payload: C 2B, Rust 2B -> DIFFERS
      first tile byte diff at +0 (bit 2):
        C:    [98] 20
        Rust: [a7] f2
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [1a] 00 13 4c 92 42 0d 32
      Rust: [19] 80 10 40 82 02 09 20
      C field at that bit: base_q_idx=160 (bits 4..11)
      Rust field at that bit: base_q_idx=152 (bits 4..11)

==============================================================================
TILE OP-TRACE COMPARISON (canonicalized arithmetic-coder ops)
==============================================================================
  op counts: C=5  Rust=7
  markers: C 3 (DONE: ['W DONE nbytes=2']) | Rust 2 (DONE: ['W DONE nbytes=2'])
  RESULT: first divergence at op 0
  op-kind histogram up to divergence: {}

  C ops [0..4]:
        0: W CDF nsyms=10 s=0 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W BOOL val=1 f=1097 rng=40248
        2: W CDF nsyms=13 s=0 icdf=[17180,15741,13430] rng=42816
        3: W CDF nsyms=13 s=0 icdf=[10137,8616,7390] rng=40780
        4: W CDF nsyms=3 s=0 icdf=[26986,21293,0] rng=56342

  Rust ops [0..6]:
        0: W CDF nsyms=10 s=1 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W CDF nsyms=2 s=1 icdf=[1097,0,0] rng=45184
        2: W CDF nsyms=13 s=0 icdf=[17180,15741,13430] rng=48000
        3: W CDF nsyms=13 s=0 icdf=[10137,8616,7390] rng=45788
        4: W CDF nsyms=2 s=1 icdf=[16253,0,0] rng=63356
        5: W CDF nsyms=13 s=0 icdf=[16644,15250,13011] rng=62498
        6: W CDF nsyms=13 s=0 icdf=[9821,8347,7160] rng=61460

==============================================================================
VERDICT: streams NOT IDENTICAL
==============================================================================
```

### uniform 64x64 q40 p6

```
==============================================================================
OBU-LEVEL COMPARISON   C=23B  Rust=25B
==============================================================================
  C stream:    [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 6), ('FRAME', 11)]
  Rust stream: [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 9), ('FRAME', 10)]
  streams byte-identical: False

[OBU 0] TEMPORAL_DELIMITER: C 0B, Rust 0B -> IDENTICAL

[OBU 1] SEQUENCE_HEADER: C 6B, Rust 9B -> DIFFERS
    SEQUENCE_HEADER field walk (C | Rust):
      DIFF #3: C @5    seq_level_idx[0]=0                     | R @5    seq_level_idx[0]=8
      DIFF #9: C @31   enable_filter_intra=1                  | R @31   enable_filter_intra=0
      DIFF #13: C @35   enable_restoration=1                   | R @35   enable_restoration=0
      DIFF #16: C @38   color_description_present_flag=0       | R @38   color_description_present_flag=1
      DIFF #17: C @39   color_range=0                          | R @39   color_primaries=1
      DIFF #18: C @40   chroma_sample_position=0               | R @47   transfer_characteristics=13
      DIFF #19: C @42   separate_uv_delta_q=0                  | R @55   matrix_coefficients=1
      DIFF #20: C @43   film_grain_params_present=0            | R @63   color_range=1
      DIFF #21: C (end)                                        | R @64   chroma_sample_position=0
      DIFF #22: C (end)                                        | R @66   separate_uv_delta_q=0
      DIFF #23: C (end)                                        | R @67   film_grain_params_present=0
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [18] 15 7f fd 30 08
      Rust: [1a] 15 7f fc 22 02 1a 03
      C field at that bit: seq_level_idx[0]=0 (bits 5..9)
      Rust field at that bit: seq_level_idx[0]=8 (bits 5..9)

[OBU 2] FRAME: C 11B, Rust 10B -> DIFFERS
    FRAME field walk (C | Rust):
      DIFF #4: C @4    base_q_idx=160                         | R @4    base_q_idx=152
      DIFF #11: C @18   loop_filter_level[0]=19                | R @18   loop_filter_level[0]=16
      DIFF #12: C @24   loop_filter_level[1]=19                | R @24   loop_filter_level[1]=16
      DIFF #13: C @30   loop_filter_level[2]=9                 | R @30   loop_filter_level[2]=8
      DIFF #14: C @36   loop_filter_level[3]=9                 | R @36   loop_filter_level[3]=8
      DIFF #19: C @50   cdef_y_pri_strength[0]=0               | R @50   cdef_y_pri_strength[0]=2
      DIFF #20: C @54   cdef_y_sec_strength[0]=0               | R @54   cdef_y_sec_strength[0]=1
      DIFF #21: C @56   cdef_uv_pri_strength[0]=0              | R @56   cdef_uv_pri_strength[0]=2
      DIFF #23: C @62   lr_type[0]=0                           | R @62   tx_mode_select=0
      DIFF #24: C @64   lr_type[1]=0                           | R @63   reduced_tx_set=0
      DIFF #25: C @66   lr_type[2]=0                           | R (end)
      DIFF #26: C @68   tx_mode_select=1                       | R (end)
      DIFF #27: C @69   reduced_tx_set=0                       | R (end)
    FH decoded length: C 70 bits (9B), Rust 64 bits (8B)
    tile payload: C 2B, Rust 2B -> DIFFERS
      first tile byte diff at +0 (bit 2):
        C:    [98] 20
        Rust: [a7] f2
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [1a] 00 13 4c 92 42 00 00
      Rust: [19] 80 10 40 82 02 09 20
      C field at that bit: base_q_idx=160 (bits 4..11)
      Rust field at that bit: base_q_idx=152 (bits 4..11)

==============================================================================
TILE OP-TRACE COMPARISON (canonicalized arithmetic-coder ops)
==============================================================================
  op counts: C=5  Rust=7
  markers: C 3 (DONE: ['W DONE nbytes=2']) | Rust 2 (DONE: ['W DONE nbytes=2'])
  RESULT: first divergence at op 0
  op-kind histogram up to divergence: {}

  C ops [0..4]:
        0: W CDF nsyms=10 s=0 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W BOOL val=1 f=1097 rng=40248
        2: W CDF nsyms=13 s=0 icdf=[17180,15741,13430] rng=42816
        3: W CDF nsyms=13 s=0 icdf=[10137,8616,7390] rng=40780
        4: W CDF nsyms=3 s=0 icdf=[26986,21293,0] rng=56342

  Rust ops [0..6]:
        0: W CDF nsyms=10 s=1 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W CDF nsyms=2 s=1 icdf=[1097,0,0] rng=45184
        2: W CDF nsyms=13 s=0 icdf=[17180,15741,13430] rng=48000
        3: W CDF nsyms=13 s=0 icdf=[10137,8616,7390] rng=45788
        4: W CDF nsyms=2 s=1 icdf=[16253,0,0] rng=63356
        5: W CDF nsyms=13 s=0 icdf=[16644,15250,13011] rng=62498
        6: W CDF nsyms=13 s=0 icdf=[9821,8347,7160] rng=61460

==============================================================================
VERDICT: streams NOT IDENTICAL
==============================================================================
```

### gradient 64x64 q40 p13

```
==============================================================================
OBU-LEVEL COMPARISON   C=287B  Rust=229B
==============================================================================
  C stream:    [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 6), ('FRAME', 274)]
  Rust stream: [('TEMPORAL_DELIMITER', 0), ('SEQUENCE_HEADER', 9), ('FRAME', 213)]
  streams byte-identical: False

[OBU 0] TEMPORAL_DELIMITER: C 0B, Rust 0B -> IDENTICAL

[OBU 1] SEQUENCE_HEADER: C 6B, Rust 9B -> DIFFERS
    SEQUENCE_HEADER field walk (C | Rust):
      DIFF #3: C @5    seq_level_idx[0]=0                     | R @5    seq_level_idx[0]=8
      DIFF #16: C @38   color_description_present_flag=0       | R @38   color_description_present_flag=1
      DIFF #17: C @39   color_range=0                          | R @39   color_primaries=1
      DIFF #18: C @40   chroma_sample_position=0               | R @47   transfer_characteristics=13
      DIFF #19: C @42   separate_uv_delta_q=0                  | R @55   matrix_coefficients=1
      DIFF #20: C @43   film_grain_params_present=0            | R @63   color_range=1
      DIFF #21: C (end)                                        | R @64   chroma_sample_position=0
      DIFF #22: C (end)                                        | R @66   separate_uv_delta_q=0
      DIFF #23: C (end)                                        | R @67   film_grain_params_present=0
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [18] 15 7f fc 20 08
      Rust: [1a] 15 7f fc 22 02 1a 03
      C field at that bit: seq_level_idx[0]=0 (bits 5..9)
      Rust field at that bit: seq_level_idx[0]=8 (bits 5..9)

[OBU 2] FRAME: C 274B, Rust 213B -> DIFFERS
    FRAME field walk (C | Rust):
      DIFF #4: C @4    base_q_idx=160                         | R @4    base_q_idx=156
      DIFF #11: C @18   loop_filter_level[0]=19                | R @18   loop_filter_level[0]=17
      DIFF #12: C @24   loop_filter_level[1]=19                | R @24   loop_filter_level[1]=17
      DIFF #13: C @30   loop_filter_level[2]=9                 | R @30   loop_filter_level[2]=8
      DIFF #14: C @36   loop_filter_level[3]=9                 | R @36   loop_filter_level[3]=8
      DIFF #23: C @62   tx_mode_select=1                       | R @62   tx_mode_select=0
    FH decoded length: C 64 bits (8B), Rust 64 bits (8B)
    tile payload: C 266B, Rust 205B -> DIFFERS
      first tile byte diff at +0 (bit 3):
        C:    [b5] db b0 49 c5 87 4b 21
        Rust: [ad] f9 f3 5d c7 a9 35 1e
    first payload diff: byte +0 bit 6 (bitpos 6)
      C:    [1a] 00 13 4c 92 42 0d 32
      Rust: [19] c0 11 44 82 02 0d 30
      C field at that bit: base_q_idx=160 (bits 4..11)
      Rust field at that bit: base_q_idx=156 (bits 4..11)

==============================================================================
TILE OP-TRACE COMPARISON (canonicalized arithmetic-coder ops)
==============================================================================
  op counts: C=2710  Rust=2220
  markers: C 3 (DONE: ['W DONE nbytes=266']) | Rust 2 (DONE: ['W DONE nbytes=205'])
  RESULT: first divergence at op 0
  op-kind histogram up to divergence: {}

  C ops [0..7]:
        0: W CDF nsyms=10 s=3 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W CDF nsyms=10 s=0 icdf=[14306,11848,9644] rng=51744
        2: W BOOL val=0 f=1097 rng=58370
        3: W CDF nsyms=13 s=0 icdf=[17180,15741,13430] rng=56428
        4: W CDF nsyms=14 s=0 icdf=[22361,21560,19868] rng=53800
        5: W CDF nsyms=3 s=0 icdf=[19782,17588,0] rng=34206
        6: W BOOL val=0 f=1097 rng=54600
        7: W CDF nsyms=11 s=10 icdf=[26070,24434,20807] rng=52786

  Rust ops [0..7]:
        0: W CDF nsyms=10 s=2 icdf=[12631,11221,9690] rng=32768 <-- DIVERGENCE
        1: W CDF nsyms=2 s=0 icdf=[1097,0,0] rng=49280
        2: W CDF nsyms=13 s=1 icdf=[17180,15741,13430] rng=47644
        3: W CDF nsyms=7 s=3 icdf=[30588,27736,25201] rng=34288
        4: W CDF nsyms=13 s=0 icdf=[23255,5887,5795] rng=63056
        5: W CDF nsyms=2 s=0 icdf=[1229,0,0] rng=36718
        6: W CDF nsyms=11 s=10 icdf=[26070,24434,20807] rng=35356
        7: W CDF nsyms=2 s=1 icdf=[17255,0,0] rng=48832

==============================================================================
VERDICT: streams NOT IDENTICAL
==============================================================================
```

### Supplementary control: uniform 64x64 CLI-qp 42 preset 13 (VAQ-corrected)

Rust's frame-level VAQ shifts uniform content by −2 CLI steps, so Rust at
CLI qp 42 encodes at effective qp 40 / qindex 160 — the qindex C uses at CLI
qp 40. FH walk excerpt from that control run
(`target/identity/uniform_64x64_q42_p13/report.txt`):

```
      DIFF #4: C @4    base_q_idx=168                         | R @4    base_q_idx=160
      DIFF #11: C @18   loop_filter_level[0]=22                | R @18   loop_filter_level[0]=19
      DIFF #12: C @24   loop_filter_level[1]=22                | R @24   loop_filter_level[1]=19
      DIFF #13: C @30   loop_filter_level[2]=11                | R @30   loop_filter_level[2]=9
      DIFF #14: C @36   loop_filter_level[3]=11                | R @36   loop_filter_level[3]=9
      DIFF #22: C @60   cdef_uv_sec_strength[0]=1              | R @60   cdef_uv_sec_strength[0]=0
      DIFF #23: C @62   tx_mode_select=1                       | R @62   tx_mode_select=0
```

Rust@qindex160 signals base_q **160**, lf **19/19/9/9**, cdef y_pri **3** —
exactly what C signaled at qindex 160 in the qp-40 run. The deblock picker
(`svt_av1_pick_filter_level_by_q` port) and the CDEF use_qp_strength fast
path track C bit-exactly at equal qindex; **the entire FH numeric cascade is
VAQ-induced**, and `tx_mode_select` is the only structural FH divergence at
p13.

## Divergence analysis — owning C subsystem per class

### SH (all configs)

| # | field | C behavior (source) | Rust behavior (source) |
|---|-------|---------------------|------------------------|
| S1 | `seq_level_idx[0]` = 0 vs 8 | Level auto-computed from dims/rate: `write_bitstream_level` ← `major_minor_to_seq_level_idx(bl)`, `Source/Lib/Codec/entropy_coding.c:3746-3749`; 64x64 still → level 2.0 → idx 0 | Hardcoded 8 (level 4.0), `svtav1-entropy/src/obu.rs:286` (the obu.rs:989 test already documents the mismatch). Fix = port the level calculator (pure fn of dims/fps), not a constant swap |
| S2 | `color_description_present_flag` = 0 vs 1 (+3 bytes CICP) | Library defaults cp/tc/mc = 2/2/2 "unspecified" (`Source/Lib/Globals/enc_settings.c:1043-1045`) → flag 0, nothing written | Always writes `ColorDescription::srgb()` (cp=1, tc=13, mc=1) with flag 1 (`obu.rs` write_sequence_header_inner). Needs a "no color description" knob for match-C mode |
| S3 | `color_range` = 0 vs 1 | Default `EB_CR_STUDIO_RANGE` (`enc_settings.c:1046`) | Hardcoded full range, `obu.rs:382` |
| S4 (p6 only) | `enable_filter_intra` = 1 vs 0 | Per-preset: `get_filter_intra_level_allintra(enc_mode)` → `scs->seq_header.filter_intra_level`, `Source/Lib/Codec/enc_mode_config.c:4017-4025` (on at M6, off at M13) | Always 0 (filter_intra unported) |
| S5 (p6 only) | `enable_restoration` = 1 vs 0 | Per-preset allintra derivation, `enc_mode_config.c:4057` | Always 0 (restoration unported — CLAUDE.md gap 2). Forces FH shape difference: C's p6 FH carries `lr_type[0..2]` (3×2 bits → 70-bit FH vs Rust 64) |

### FH (all configs)

| # | field | C behavior (source) | Rust behavior (source) |
|---|-------|---------------------|------------------------|
| F1 | `base_q_idx` 160 vs 152 (uniform) / 156 (gradient) | CQP (`rc 0` + `aq_mode 0`): qindex = `quantizer_to_qindex[qp]`, no content adjustment → 160 = table[40] | Frame-level VAQ **always on**: `qp += clamp(log2(activity/10), −2, 2)` before the table, `svtav1-encoder/src/pipeline.rs:225-230` (uniform → −2 → 152; gradient → −1 → 156). No aq_mode=0 equivalent exists |
| F2 | `loop_filter_level[0..3]`, `cdef_*_strength` | q-driven pickers on qindex 160 | Same pickers (pinned bit-exact ports) on the VAQ-shifted qindex — pure F1 cascade, proven by the qp-42 control above |
| F3 | `tx_mode_select` = 1 vs 0 | **Always TX_MODE_SELECT**: `frm_hdr->tx_mode = TX_MODE_SELECT` under `FTR_COUPLE_VLPD0_TXS_PER_SB` ("Use TX_MODE_SELECT even when txs_level == 0, as the decision may change from OFF to Fastest at the SB level"), `Source/Lib/Codec/enc_mode_config.c:15140-15143`; written at `entropy_coding.c:3659`; per-block tx_depth symbol then coded at `entropy_coding.c:4704` | `tx_mode_select = 0` (TX_MODE_LARGEST), `obu.rs:571`; **no per-block tx_depth symbols exist in the Rust tile** — the first structural tile-syntax divergence |

### Tile op-trace

**Op 0 diverges in all three configs, and it is always the 64x64 partition
symbol.** Both sides code it from the same CDF table with the same starting
state (`icdf=[12631,11221,9690]`, `rng=32768` — partition_cdf 64x64 ctx 0,
icdf[0] = 32768−20137), so context modeling agrees; the *decision* differs:

| config | C symbol | Rust symbol |
|--------|----------|-------------|
| uniform p13 | 0 = PARTITION_NONE | 1 = PARTITION_HORZ |
| uniform p6  | 0 = PARTITION_NONE | 1 = PARTITION_HORZ |
| gradient p13 | 3 = PARTITION_SPLIT | 2 = PARTITION_VERT |

**Which C decision produces it (p13 uniform):** the partition symbol is
written by the entropy stage from the final mode-decision block tree
(`aom_write_symbol(.., frame_context->partition_cdf[ctx], ..)`,
`Source/Lib/Codec/entropy_coding.c:1029`). The decision itself is made by
the mode-decision partition RDO: the PD0/PD1 passes in
`Source/Lib/Codec/product_coding_loop.c` (M13 runs the light-PD0 path,
`product_prediction_fun_table_light_pd0`, product_coding_loop.c:57), with
the candidate depth set constrained per preset by
`set_depth_removal_level_controls` (`Source/Lib/Codec/enc_mode_config.c:4197`,
per-preset levels assigned around `:13259-13276`). For a flat SB the RD
compare lands on 64x64 NONE (one DC-skip block: 5 tile ops total —
partition, skip, y_mode, uv_mode, tx_depth — 2-byte tile). Notably C's
uniform tile op stream is **identical at p6 and p13** (same 5 ops, same rng
sequence): preset only moved SH/FH tool bits, not the flat-content
decisions.

**Rust side:** `partition_search_with_config`
(`svtav1-encoder/src/partition.rs:420`) uses a homegrown RD:
`rd = distortion + (lambda*rate)>>8` with a hardcoded 48-unit
partition-flag overhead (partition.rs:488) and strict `<` preference
(partition.rs:547), with estimate-based rates rather than C's bit-estimate
cost tables. On uniform content every candidate reconstructs exactly
(distortion 0), so the pick is decided purely by those rate estimates — the
model prices one 64x64 NONE block *higher* than two 64x32 HORZ children plus
overhead, so HORZ wins. Which rate term overprices NONE is the first
decision-parity investigation (encode_with_neighbors rate accounting); it is
deliberately not root-caused-and-fixed here.

**Everything after op 0 is derivative.** Once the block trees differ, mode
pair counts, the uv alphabet (C codes nsyms=14 CFL-allowed uv_mode for its
32x32 chroma-ref blocks; Rust's 32x64 children are correctly CFL-disallowed
nsyms=13 — both spec-correct *for their own trees*), angle_delta ops, and
all coefficient syntax diverge as consequences. No independent tile
divergence is observable until partition parity lands. The one structural
(non-derivative) tile difference already known: C codes a per-block CDF3
tx_depth symbol (F3) that Rust never emits.

**Non-divergences worth recording:**
- skip flag: C `encode_skip_coeff_av1` (`entropy_coding.c:1055`) goes
  through the nsyms==2 bool specialization; Rust codes the same CDF through
  the generic path — canonically identical (harness proves equal arithmetic
  by rng checksum on the aligned prefix… once one exists).
- range coder, CDF tables, update_cdf: already differentially fuzzed
  bit-exact (existing gates); every CDF fingerprint observed in the traces
  (partition 12631, skip 1097 = 32768−31671, kf_y_mode 17180 = 32768−15588,
  uv_mode 10137, tx_depth 26986, eob_pt CDF11) matches the shared default
  tables on both sides.

### Op-kind glossary for the traces (default-CDF fingerprints)

| trace line | syntax element |
|------------|----------------|
| `CDF nsyms=10 icdf=[12631,..]` | partition, 64x64 ctx0 |
| `B f=1097` / `CDF2 [1097]` | skip, ctx0 (default 31671) |
| `CDF nsyms=13 icdf=[17180,..]` | kf y_mode, ctx (0,0) |
| `CDF nsyms=13 icdf=[10137,..]` | uv_mode, CFL-disallowed, y=DC row |
| `CDF nsyms=14 icdf=[22361,..]` | uv_mode, CFL-allowed (C 32x32 chroma-ref) |
| `CDF nsyms=3 icdf=[26986,21293]` | tx_depth (square ≥32 tx_size_cdf) — C only |
| `CDF nsyms=7` | angle_delta (Rust directional modes) |
| `CDF nsyms=11 s=10 icdf=[26070,..]` | eob_pt, 1024-coeff class (32x32 TX) |
| `CDF2 [1229]/[16253]` | txb_skip rows / adapted skip ctx |

## Status after the 2026-07-13 fixes (commits d72a7641..85d7e0fd + 084d2c13e/eab9d8860)

Items 1-3 and the flat-content half of item 5 of the original list below
are DONE, and the p6 tool bits landed the same day (header items 5-6).
Matrix: **18/36 byte-identical** (`benchmarks/identity_matrix_2026-07-13.tsv`)
— every uniform cell at every tracked preset (13/10/6), both sizes, all
qps. Verdicts at the tracked configs:

| config | verdict | residual divergence |
|--------|---------|---------------------|
| uniform 64x64 q40 **p13** | **IDENTICAL** (exit 0) | none |
| uniform {64,128} q{20,40,55} **p6** | **IDENTICAL** (exit 0) | none — SH tool bits (S4/S5), 70-bit FH with `lr_type[0..2]`, and the all-skip cdef-search outcome all match; tile stays 5-op with rng equality (64x64 blocks are filter-intra-ineligible, exactly like C) |
| gradient 64x64 q40 **p13** | NOT IDENTICAL | TD+SH+FH all byte-identical; tile op 0 = C picks PARTITION_SPLIT (full RDO with real coeff-rate estimation), ours picks NONE (crude coeff-rate estimates + ctx-0 partition costs) |
| gradient 64x64 q55 **p6** | NOT IDENTICAL | FH `cdef_y_pri_strength[0]`: C searched 0, our qp fast path 10 — the narrowed gap 2a (search outcome for frames with live filter blocks) |

## Priority-ordered fixes — remaining

1. **[DONE 2026-07-13, commits 084d2c13e+eab9d8860] p6 tool bits** —
   SH `enable_filter_intra`/`enable_restoration` per-preset (C-exact
   allintra derivations), FH `lr_type[0..2]` all-RESTORE_NONE syntax,
   per-block `use_filter_intra` flag for eligible DC <=32x32 blocks
   (fires for real in the s2 gradient conformance cells — trace-verified
   non-vacuous — and both aomdec + dav1d parse all 525+700 streams),
   plus the CDEF all-skip search outcome. All 6 uniform p6 matrix cells
   -> IDENTICAL.
2. **CDEF RDO search for non-all-skip frames** (gap 2a, narrowed) — the
   sb_count==0 branch of `finish_cdef_search` is ported and C-exact
   (uniform cells prove it); the mse path over live filter blocks
   (svt_av1_cdef_search per-fb strength mse + joint_strength_search_dual
   + lambda rate) remains. Repro: gradient 64 q55 p6 — C searched
   y_pri 0, qp path says 10. Also owns the 6 gradient128 M10/M13
   `cdef_uv_sec_strength[1]` FH cells (C search vs qp fast path at
   multi-SB sizes).
3. **Partition/mode-decision parity for textured content**: port C's md
   coefficient-rate estimation (the crude `estimate_coeff_rate` is now
   the binding constraint — partition symbol costs are already real
   entropy costs; threading the live md partition_context into the cost
   rows is the follow-up) + per-preset depth-removal gates. Repro:
   gradient 64x64 p13 — C SPLITs where we keep NONE. Owns the 11
   tile-op matrix cells.
4. **uv_mode alphabet / CFL** and everything downstream of decision
   parity — derivative until item 3 lands.

## Harness limitations (known, accepted for now)

- 64-aligned dims only (CLAUDE.md gap 5), single tile, key frame,
  reduced-still SH. The FH field walker covers exactly the branches these
  configs exercise and raises loudly on unimplemented ones (segmentation,
  lr unit sizes, delta-lf updates, non-reduced SH).
- The differ's SH/FH walkers decode each side independently, so a field
  divergence that changes *which* later fields exist is reported as an
  offset field list, not silently misaligned.
- C stderr (config dump) is preserved at `c.stderr` per case; C INIT/RESET/
  DONE markers segment the trace and confirmed exactly one EC instance and
  one tile per stream at `--lp 1`.
