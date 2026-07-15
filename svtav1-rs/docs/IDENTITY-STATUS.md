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

## 2026-07-13 (wave2, later): gradient-64 p13/p10 — op-level diagnosis + root cause

Differ reruns (all six cells, `identity_diff.sh 64 64 {20,40,55} {13,10} gradient`):
p10 and p13 traces are **byte-identical on both sides** — the C library
clamps allintra presets: `enc_handle.c:4634-4644` `if (enc_mode > ENC_M9)
enc_mode = ENC_M9` ("Preset M13 is mapped to M9." in `c.stderr`). Three
distinct cases, each owning 2 matrix cells; TD+SH+FH byte-identical in all:

| case | first diverging op | C op (kind, sym, icdf, rng) | Rust op |
|------|--------------------|------------------------------|---------|
| q40 (op 0) | 64x64 partition, ctx0 | `CDF10 s=3 [12631,11221,9690] rng=32768` = SPLIT | `s=0` = NONE (V_PRED leaf) |
| q55 (op 0) | 64x64 partition, ctx0 | `CDF10 s=0` = NONE (DC, skip=0) | `s=2` = VERT |
| q20 (op 1) | 32x32 partition, ctx0 (op 0 = SPLIT matches) | `CDF10 s=3 [14306,11848,9644] rng=51744` = SPLIT | `s=0` = NONE |

Same CDF, same ctx, same starting `rng` at the diverging op in every case —
pure DECISION divergence (candidate sets + cost model), zero context/state
divergence.

### Root cause (C decision architecture at allintra effective-M9)

Instrumented the C library (SVT_MD_DEBUG prints in `product_coding_loop.c` /
`enc_dec_process.c`, never committed) and captured the full MD walk for all
six cells. The C partition decision for these configs is **entirely PD0**:

- `pd1_signals: pred_depth_only=1 fixed_partition=1 nsq_search_off=1
  depth_refine_mode=2(PD0_DEPTH_PRED_PART_ONLY)` — PD1 (light-PD1) codes
  exactly the PD0-picked tree; **no NSQ shapes are ever searched**
  (`svt_aom_get_nsq_search_level_allintra` = 0 above M3,
  enc_mode_config.c:11936). Rust's VERT pick at q55 is a shape C never
  evaluates.
- Depth set: `set_blocks_to_be_tested` (enc_dec_process.c:1491) limits
  `max_sq_size = ctx->max_block_size` from
  `get_max_block_size_allintra` (enc_mode_config.c:8969): M8+ →
  `var_th_cap = round(7500*qw/qwd)`; SB 64x64 source variance (5425 for
  gradient) > cap at q20 (2381) and q40 (4762) → **64x64 depth is not
  even evaluated (SPLIT at 64 is forced)**; at q55 (cap 6860) 64x64 is in
  the set. min = 8x8 (`disallow_4x4=1`, `disallow_8x8_allintra()=false`).
- Per-SB PD0 level: `pic_pd0_lvl=7` → `PD0_LVL_6` (enc_mode_config.c:12598),
  then `pd0_detector_allintra` (enc_dec_process.c:2373) demotes to LVL_5
  when the variance-across-depths spread is flat (th `round(7500*qw/qwd)`):
  gradient stays LVL_6 at q20, demotes to **LVL_5 at q40/q55**.
- LVL_6 block cost = `compute_lpd0_cost_allintra`
  (product_coding_loop.c:8418): closed-form `area * bias / 1000` from the
  picture-analysis variance map (85 uint16s per 64x64,
  pic_analysis_process.c:312 `compute_b64_variance`, BLOCK_MEAN_PREC_SUB
  even-row subsampling) with qp-scaled thresholds; split rate = 0.
- LVL_5 block cost = real light-PD0 encode (product_coding_loop.c
  `md_encode_block_pd0` → `full_loop_core_pd0` → `perform_tx_pd0`):
  single DC candidate (mode_decision.c `inject_intra_candidates_pd0`),
  prediction from **source-pixel neighbors** (`pd0_use_src_samples=1` for
  allintra, enc_mode_config.c:9437) with spec edge fills, max-square
  TX at depth 0 (subres row-subsampling when the per-SB 64x64 odd/even
  SAD check passes: q55 yes → TX_64X32/32X16/16X8; q40 no 64x64 block →
  forced step 0), `svt_aom_quantize_b` at `qindex+8`
  (`rate_est_ctrls.lpd0_qp_offset`), **freq-domain SSE** distortion
  (coeff vs dq-coeff over the packed <=32x32 region + three_quad_energy,
  shifted by `(1 - tx_scale)*2`), and coefficient rate =
  **`5000 + 100*eob`** (`coeff_rate_est_lvl==0` closed form,
  product_coding_loop.c:4568; verified: eob 980 -> 103000, 97 -> 14700).
  `full_cost = RDCOST(lambda, bits + skip_fac_bits[0][0](26) +
  partition_fac_bits[0][NONE](400), dist)` (rd_cost.c:1335), lambda =
  `svt_aom_compute_rd_mult` KF chain (rc_process.c:452:
  `(3.3+0.0015*dc_q)*dc_q^2` ×150>>7; observed 25650/248207/1527856 at
  qindex 80/160/220).
- Split-vs-parent (`test_split_partition_pd0`, product_coding_loop.c:10897):
  `split_cost = RDCOST(lambda, 2*partition_fac_bits[0][SPLIT], 0)` (the ×2
  because `use_accurate_part_ctx=0` at M9; = 0 at LVL_6-allintra) + sum of
  child costs; parent wins iff `1000*parent <= 1000*split` (bias 1000 for
  allintra); early exits (th 50/0) only at LVL_5.

C final trees: q20 → 16x16 leaves (SPLIT,SPLIT,NONE...), q40 → four 32x32
NONE, q55 → single 64x64 NONE — each matches the stream ops exactly.

**Fix direction (this chunk):** port PD0 verbatim for allintra effective-M9
(variance map, detector, LVL_6 + LVL_5 costs, split compare) and drive the
SB partition tree from it (fixed partition, nsq off), leaving leaf
mode/coeff decisions to the existing coder. That moves every partition
symbol; the next divergence is then the per-leaf mode/coeff syntax (LPD1
parity: DC-only-vs-mode-search, C quantizer, tx_depth/uv syntax).

### Status after the PD0 port (commits ffb73bcf2 + 60b006b85)

`crates/svtav1-encoder/src/pd0.rs` is the verbatim PD0 port (every
constant and per-block cost pinned against instrumented-C captures in its
unit tests — variance map, qp factors, the KF lambda chain incl. the
lambda_weight=150 frame multiply, LVL_6 costs, LVL_5 costs with subres,
rate constants 26/400/1195/1465/2020, and the final q20/q40/q55 trees),
and the pipeline drives still-frame SB partitions from it at presets >= 9
(`encode_fixed_tree`; inter frames and presets <= 8 keep the search).

Differ deltas on the six gradient-64 p13/p10 cells (matrix
`benchmarks/identity_matrix_2026-07-13.tsv` regenerated — still 18/36
identical, no uniform regressions, divergences strictly later):

| cell | first divergence | now diverges on |
|------|------------------|-----------------|
| q40 p13/p10 | op 0 -> **op 3** | leaf y_mode: C DC, ours V_PRED (32x32 leaves) |
| q55 p13/p10 | op 0 -> **op 8** | coefficient bits: partition + skip + y_mode(DC) + uv_mode + tx_depth + txb_skip + eob_pt class ALL match; C quantized levels differ from our dead-zone quantizer |
| q20 p13/p10 | op 1 -> **op 4** | leaf y_mode (16x16 leaves) |

Every PARTITION symbol in these streams now matches C. The two remaining
subsystems for full gradient-64 p13/p10 identity, in dependency order:

1. **[FIXED 2026-07-13, commit b7f362af4 — and the diagnosis below was
   WRONG] Leaf mode parity (y_mode)** — owned q20 op 4 + q40 op 3.
   The light-PD1 attribution was an unverified inference: **allintra
   never takes light-PD1** (`pcs->pic_lpd1_lvl = 0` unconditionally,
   `svt_aom_sig_deriv_mode_decision_config_allintra`,
   enc_mode_config.c:15250, so `pd1_level = REGULAR_PD1` and the
   enc-dec dispatch at enc_dec_process.c:3311 selects
   `svt_aom_sig_deriv_enc_dec_allintra`). See the 2026-07-13 (later)
   section at the bottom for the verified mechanism — the pick is bound
   by the CANDIDATE SET (`is_dc_only_safe`), not by any cost.
   <i>Original (wrong) text kept for the record:</i>
   `generate_md_stage_0_cand_light_pd1` / `md_stage_0_light_pd1` /
   `md_stage_3_light_pd1` -> `full_loop_core_light_pd1` cost-chain port
   claimed required; "there is no smaller C-exact piece" — there was:
   the variance gate.
2. **Final coefficient parity (light-PD1 coding quantizer)** — owns
   q55 op 8 (and everything after mode parity elsewhere). Our leaf coder
   quantizes with a homegrown dead-zone rounding
   (encode_loop.rs:142-154); C's final levels come from the LPD1 MDS3
   quant path: `svt_aom_quantize_inv_quantize` (full_loop.c) with
   `enc_ctx->quants_8bit` (svt_av1_build_quantizer zbin/round 84-80/48
   factors — the PD0 port's `build_quant_entry`/`quantize_b` in pd0.rs
   are the C-exact primitives to reuse) plus RDOQ
   (`ctx->mds_do_rdoq = true` at light-PD1, md_stage_3_light_pd1) —
   RDOQ (`svt_aom_rdoq` / av1_optimize_txb port) is the large piece.
   The q55 64x64 block is DC + DCT_DCT + TX_64X64 with eob 97 vs our
   different levels — a first sub-step could be swapping the leaf
   coder's quantizer to quantize_b (no RDOQ) and measuring the op-8
   delta; only land it if it provably moves the divergence (RDOQ may
   dominate).

Preset-6 gradient cells (op 0/1) are NOT covered by this port: at
allintra M6 the C pipeline runs `pic_pd0_lvl = 1` (PD0_LVL_0/1 —
prediction-based PD0 with ME/intra candidates and depth refinement, not
the LVL_5/6 shortcut) and nsq geometry level 3 with a real nsq search
(`svt_aom_get_nsq_search_level_allintra` != 0 at <= M3 only — M6 still
has nsq GEOMETRY enabled with min 8x8 but search level 0; the M6
divergence is depth-set + PD0-cost differences, enc_mode_config.c:12598
`set_pic_pd0_lvl_allintra` M2..M8 branch).

## 2026-07-13 (wave2, later still): leaf y_mode CLOSED — the C pick is a candidate-set fact, not a cost fact (commit b7f362af4)

Source-verified + instrumented-C-proven correction of the section above,
then the fix. All C cites re-verified against the tree at v4.2.0-rc with
every `EbDebugMacros.h` feature macro = 1.

### The real C decision path at allintra effective-M9 (PD1)

- **Light-PD1 is never used for allintra**: the allintra MD-config
  function sets `pcs->pic_lpd1_lvl = 0` unconditionally
  (enc_mode_config.c:15250), `set_lpd1_ctrls(ctx, 0)` leaves
  `pd1_level = REGULAR_PD1` (enc_mode_config.c:7337,9133), and the PD1
  dispatch (enc_dec_process.c:3301-3312) therefore runs
  `svt_aom_sig_deriv_enc_dec_allintra` (the live `CLN_PD0` variant,
  enc_mode_config.c:11294) + the regular `md_encode_block`
  (product_coding_loop.c:9804).
- PD1 intra controls: `pcs->intra_level = 8` at eff-M9
  (`svt_aom_get_intra_mode_levels_allintra`, enc_mode_config.c:6907:
  1/M4, 2/M5, 6/M6, 7/M7-M8, **8 above**), applied via
  `set_intra_ctrls(pcs, ctx, 8, 0)` (enc_mode_config.c:8449): intra
  mode end SMOOTH_PRED, `angular_pred_level = 4` (only V/H survive the
  D45..D67 skip mask, no angle deltas), `prune_using_best_mode = 1`,
  **`prune_using_edge_info = 1`** — the flag that arms the gate below.
  Filter-intra level 0, palette level 0 (even sc_class5 is 0 above M7,
  enc_mode_config.c:3488), intrabc off, cand_elimination off.
- **The gate: `is_dc_only_safe` (mode_decision.c:845)**, called from
  `generate_md_stage_0_cand` (mode_decision.c:3633). Reads the SAME
  85-entry per-SB variance map the PD0 port already computes
  (`pcs->ppcs->variance[sb_index]` = `compute_b64_variance`;
  `svt_aom_get_blk_var_map` product_coding_loop.c:8368 = pd0.rs
  `blk_var_map`). For PART_N squares in a 64x64 SB: 8x8 ->
  `blk_var < 2000`; >=16x16 -> `blk_var < 2000 && (max-min of the four
  sub-block variances) < 4000`; 4x4/SB-128/non-PART_N -> never. When it
  fires, `inject_intra_candidates` runs with `dc_cand_only_flag = 1`:
  the candidate list is EXACTLY {DC_PRED} and **no cost compare of any
  kind decides the mode**.

Instrumented proof (env-gated `SVT_MDBG2` prints in a scratch copy of
the C tree, never in-repo), gradient-64:

| cell | leaves | per-leaf capture |
|------|--------|------------------|
| q40 | 4x 32x32 | `dc_only=1 safe=1 ... ncand=1 modes: 0/0` -> winner mode=0 (all 4) |
| q20 | 16x 16x16 | same, all 16 |
| q55 | 1x 64x64 | `dc_only=0 safe=0` (var 5425 >= 2000) -> `ncand=4 modes: 0/0 1/0 2/0 9/0` -> winner DC by cost, `mds3_cnt=1` |

Variance check against the pinned map (`C_GRADIENT64_VARS`): 32x32 vars
1343/1353/1733/1893 (< 2000), spreads 437/563/128/64 (< 4000); all 16x16
vars 336..901, spreads <= 1728 — every q40/q20 leaf fires the gate.

### The fix (smallest C-exact slice)

`pd0::is_dc_only_safe` is the verbatim variance half (the C early exits
are the fixed-tree call-site context), threaded as `dc_only` through
`encode_fixed_tree` -> `encode_with_neighbors` -> `encode_single_block`,
which then restricts the intra candidate slice to `[..1]` (= DC, first
in `generate_intra_candidates`) exactly like C's dc_cand_only injection.
Pipeline passes the per-SB variance map + SB origin. Homegrown-search
paths (presets <= 8, inter, partial SBs) are untouched. Pinned test
`dc_only_safe_matches_c` reproduces the instrumented capture.

### Differ deltas (p13 == p10 verified on every moved cell)

| cell | first divergence | now diverges on |
|------|------------------|-----------------|
| g64 q40 | op 3 -> **op 8** | eob_extra/coeff bits (same CDF f=22212, same rng; value differs) |
| g64 q20 | op 4 -> **op 21** | ONE quantized level in coeff_base (first 3 coeff_base symbols match) |
| g64 q55 | op 8 (unchanged) | coeff bits |
| g128 q20 | FH -> **tile op 10** | eob_extra low bits (FH now byte-identical — leaf modes DC-match at 128 too) |

Matrix: still **18/36** (`benchmarks/identity_matrix_2026-07-13.tsv`
regenerated; all uniform cells IDENTICAL, divergences strictly later).
Gates re-run green: recon-parity 216/0, decode conformance 525/0 mono +
700/0 chroma, `cargo test --workspace` all green (no test-input
changes).

### Remaining subsystems for gradient p13/p10 identity (updated)

1. **[FIXED 2026-07-13 — the coding-quantizer port closed EVERY
   remaining M13/M10 cell, see the section below]** Final coefficient
   parity (the coding quantizer) — owned g64 q40 op8 / q20 op21 /
   q55 op8, g128 q20 op10. The C path is the REGULAR-PD1 MDS3 quant —
   `svt_aom_quantize_inv_quantize` with `quants_8bit` and RDOQ per
   `pcs->rdoq_level`.
2. **Non-DC-only leaf cost chase** (latent, currently non-binding at
   the tracked cells): where the gate does NOT fire (e.g. the q55
   64x64), C runs the 4-candidate {DC, V, H, SMOOTH} funnel — MDS0
   Hadamard SATD fast cost (`mds0_use_hadamard_sb=true`, mds0_level 0)
   -> NIC counts (nic_level 11 at eff-M9) -> MDS3 full loop (spatial
   SSE level 3, `svt_av1_cost_coeffs_txb` rate, `svt_aom_intra_fast_cost`
   mode rate). Our homegrown loop happens to agree (DC) at q55; any
   future cell where it disagrees lands here.
3. **g128 q40/q55 FH `cdef_uv_pri_strength`** — MEASURED GONE with the
   quantizer port (those cells are now byte-identical); the CDEF search
   gap (2a) remains only at preset 6 (gradient 64 q55 p6).

Preset-6 gradient cells: unchanged, see the note above (different PD0
architecture at M6).

## 2026-07-13 (wave2, latest): coding quantizer CLOSED — every gradient M13/M10 cell byte-identical, matrix 30/36

The quantizer chunk landed as `crates/svtav1-encoder/src/quant.rs` plus
the still-path wiring. All 12 gradient p13/p10 cells (64+128, q20/40/55)
now print `VERDICT: streams IDENTICAL` — together with the 18 uniform
cells the matrix is **30/36** (`benchmarks/identity_matrix_2026-07-13.tsv`).
The only remaining divergences are the 6 gradient preset-6 cells (5x
tile-op 0/1 partition + 1x FH `cdef_y_pri_strength` q55 — the M6 PD0
architecture + CDEF live-block search, both documented above).

### Verified C path (instrumented library, never in-repo)

Every claim below was confirmed by an SVT_QDBG-gated instrumented build
in a scratch copy of the C tree (prints inside
`svt_aom_quantize_inv_quantize`, `svt_av1_optimize_b`,
`derive_intra_coeff_level`, `svt_aom_sig_deriv_mode_decision_config_allintra`):

- **The MDS3 quantize IS the coding quantizer**: `pic_bypass_encdec = 1`
  above M3 (`svt_aom_get_bypass_encdec_allintra`,
  enc_mode_config.c:12037; assignment :15173) — zero `is_encode_pass`
  quantize calls observed in any tracked cell. Call site:
  `perform_dct_dct_tx` (product_coding_loop.c:5790, or the
  `perform_tx_partitioning` depth-0 leg when the PD0-LVL_6 forced-TXS
  search runs, e.g. g64 q20) with `full_lambda =
  ctx->full_lambda_md[EB_8_BIT_MD]` and contexts 0/0
  (`rate_est_level = 0` above M8 -> `update_skip_ctx_dc_sign_ctx = 0`).
- **rdoq_level** = f(`pcs->coeff_lvl`) at eff-M9
  (enc_mode_config.c:14931: HIGH->0, NORMAL->3, else->2), where
  `coeff_lvl` comes from `derive_intra_coeff_level`
  (md_config_process.c:620): `cmplx = pic_avg_variance / max(1, cli_qp)`
  against intra thresholds {25,50,150} x1.7 (<240p) = {42,85,255};
  `pic_avg_variance` = u16-truncated mean of the per-B64 64x64 variances
  (pic_analysis_process.c:608). Captured: g64 pav=5425 -> q20 cmplx 271
  HIGH (rdoq 0), q40 135 NORMAL (rdoq 3), q55 98 NORMAL (rdoq 3);
  g128 pav=1483 -> q20 cmplx 74 LOW (rdoq 2).
- **rdoq_level 0** -> `av1_quantize_b_facade_ii` -> `svt_aom_quantize_b_c`
  (full_loop.c:31). **rdoq_level > 0** -> `svt_av1_quantize_fp_facade`
  (quantize_fp_helper_c, full_loop.c:222, `_fp` round/quant rows:
  `round_fp = (64*q)>>7`, `quant_fp = 65536/q`) then the
  `svt_av1_optimize_b` trellis (full_loop.c:1038). At MDS3 with bypassed
  enc-dec, `md_stage_3` CLEARS `rdoq_ctrls.skip_uv`/`dct_dct_only`
  (product_coding_loop.c) so chroma runs RDOQ too; `eob_th`/`eob_fast_th`
  are 255 at levels 1-3 (never fire); `coeff_shaving_level = 0`
  (allintra) so no shaving. `rdmult = ((lambda *
  plane_rd_mult[1][0][plane]) * 100/100 + 2) >> 2` with 17 luma / 13
  chroma (sharpness 0 -> rweight 100, rshift 2); captured rdmult
  1054880 @ lambda 248207 and 6493388 @ 1527856 — the lambda equals
  pd0's `kf_full_lambda_8bit` chain at the frame qindex exactly.
- **Cost tables**: default-CDF-derived and frame-static at eff-M9
  (`update_cdf_level = 0` -> `cdf_ctrl.enabled = 0`);
  `svt_aom_estimate_coefficients_rate` (md_rate_estimation.c:495) over
  the `base_q_idx`-bucket default coefficient CDFs.
- Quant table row pinned at qindex 220: dc zbin 326 / round 195 / quant
  -1255 / shift 128 / fp 125/261 / deq 522; ac 583/349/-29571/128/70/466/933.
- RDOQ effect captured: g64 q55 luma TX_64X64 fp-eob 671 -> post-trellis
  521; q40 32x32 fp-eob 997 -> 528 (per block).

### What landed (all under svtav1-rs/, C tree untouched)

- `svtav1-encoder/src/quant.rs`: C-exact `build_quant_table`
  (svt_av1_build_quantizer row incl. `_fp`), `quantize_b`
  (svt_aom_quantize_b_c), `quantize_fp` (quantize_fp_helper_c),
  `build_coeff_cost_tables` (estimate_coefficients_rate incl. the
  base_cost[4..8) deltas + lps_cost extension rows), `optimize_b`
  (svt_av1_optimize_b verbatim: update_coeff_general/eob/simple,
  update_skip, cut_off si_end, golomb diff tables, br/eob ctx via the
  coeff_c ports), `eob_cost` (get_eob_cost), the coeff_lvl/rdoq policy
  fns, and `quantize_inv_quantize_still` tying them together. Unit
  tests pin the instrumented captures (table row @220, rdmult pairs,
  coeff_lvl cells, rdoq policy, q/dq decoder-mirror invariant).
- `svtav1-entropy/src/coeff_c.rs`: pub `lower_levels_ctx_general` +
  `br_ctx_eob` (thin C-exact wrappers the trellis prices with).
- `encode_loop.rs`: `encode_block_tx_cq` — when the frame-level
  `CodingQuantCfg` is present, quantization runs the C path on the
  PACKED (32-capped) block exactly like C (pack -> quantize ->
  optimize -> unpack; dequant mirror preserved); dead-zone otherwise.
- `partition.rs`/`pipeline.rs`: `PartitionSearchConfig.c_quant`
  (Arc), threaded to every luma quantize in `encode_single_block` and
  to `encode_chroma_block_dc` (plane_type 1); the pipeline derives the
  frame cfg (pic_avg_variance over the pd0 variance maps -> coeff_lvl
  -> rdoq_level, kf lambda, cost tables) for key/still frames at
  presets >= 9 on 64-aligned dims.

### Gates after the port

- identity matrix **30/36** (+12; all uniform + all gradient M13/M10)
- recon parity 216/0 (C-exact quantizer keeps encoder recon == aomdec)
- decode conformance 525/0 mono + 700/0 chroma
- `cargo test --workspace` green (no test-input changes needed)

### Next subsystem (owns all 6 remaining cells)

Preset-6 gradient parity: M6 runs `pic_pd0_lvl = 1` (prediction-based
PD0 with ME/intra candidates + depth refinement, not the LVL_5/6
shortcut), nsq GEOMETRY enabled (min 8x8), and the CDEF live-block
search (`svt_av1_cdef_search` per-fb mse + `joint_strength_search_dual`)
— tile-op 0/1 partition divergence on 5 cells, FH `cdef_y_pri_strength`
on gradient-64 q55 p6.

## 2026-07-13 (wave2, M6 chunk): gradient preset-6 diagnosis — the briefed theory was (again) partially wrong

Differ reruns on all 6 cells + C-vs-C tile comparison (p6 vs p10 traces) +
an instrumented scratch build (`/root/svtav1-instr`, SVT_M6DBG env-gated
prints in enc_mode_config.c / product_coding_loop.c / restoration_pick.c /
enc_cdef.c; instrumented OBUs verified byte-identical to baseline on every
cell before trusting any dump). Corrections to the going-in theory:

1. **Loop restoration owns 4 of the 6 first divergences, not partition/mode.**
   C op 0 at g64 q20/q40 + g128 q40/q55 is `BOOL f=21198` = the
   `wiener_restore` flag (default CDF 11570) followed by BOOLEQ subexp tap
   bits: the M6 allintra LR search (`wn_filter_lvl=4`: enabled, use_chroma,
   filter_tap_lvl=2 -> 5x5 taps even for luma, use_refinement=0;
   `sg_filter_lvl=0` -> **sgrproj never searched**, rest_finish force type
   RESTORE_WIENER-vs-NONE only) picks RESTORE_WIENER for LUMA on those 4
   cells. Instrumented picks (rest_finish_search / search_wiener_finish,
   rdmult = the un-weighted lambda chain — see below):
   - g64 q20: luma WIENER; g64 q40: luma WIENER; g128 q40: luma WIENER;
     g128 q55: luma WIENER; chroma NONE everywhere (gradient U=V=128 ->
     sse_none=0); g64 q55 + g128 q20: luma NONE too (FH lr all-NONE).
   - unit_size 256 (lr_unit_shift=2), ntiles=1 at both 64 and 128.
   - example (g64 q55, the NONE case): sse_none=671191 sse_wn=670249,
     bits_none=768 bits_wn=13120, cost_none 86034676.53 < cost_wn
     87879942.74 -> NONE; taps v=[0,8,-17,18,-17,8,0] h=[0,-8,26,-36,26,-8,0].
2. **The differ FH walker was silently broken for 128x128 frames** — it
   skipped the `tile_info` increment bits that exist once a frame has >1 SB
   (uniform_tile_spacing_flag=1 is followed by increment_tile_cols/rows_log2
   bits when max_log2 > 0), so every g128 FH walk failed with a bogus
   "lr unit size" error (base_q_idx decoded as 20, etc.). g128 q20's C FH is
   actually lr-all-NONE (instrumented LRBEST) — its real first divergence is
   tile op 1 (32x32 partition: C NONE vs our VERT_4). Fixed in this chunk
   (differ tile_info + lr unit-size walk).
3. **The M6 tree is decided entirely by PD0** (as briefed):
   `pic_pd0_lvl=1` -> `pd0_level=PD0_LVL_1`, depth refinement level 10 ->
   mode 2 = PD0_DEPTH_PRED_PART_ONLY -> `pred_depth_only=1` (M6SB dump), so
   PD1 codes exactly the PD0 tree. NSQ: geom level 3, search level 0 ->
   square-only trees. `max_block_size=64` unconditionally at M6
   (base_var_th_cap=~0 below M8 — the M9 variance cap does NOT apply).
4. **PD0_LVL_1 block encode vs the ported LVL_5** (M6PD0CFG dump:
   intra_lvl=1(PD0), subres=0, rate_est_lvl=2 -> upd_skip_dc=1,
   upd_skip_coeff=0, coeff_rate_lvl=1, qp_off=0, fast_coeff=2,
   parent_bias=1000, ee ths 50/0, pf=0, use_src=1):
   - quantize at qindex+0 (LVL_5 used +8);
   - NO subres row-subsampling, ever;
   - coeff rate = real `svt_av1_cost_coeffs_txb(ctx0, dc_sign ctx0,
     reduced=0)` over the MD rate tables (rd_cost.c:1207
     svt_aom_txb_estimate_coeff_bits_pd0; eob=0 -> av1_cost_skip_txb), NOT
     the 5000+100*eob closed form (that is coeff_rate_est_lvl==0);
   - full cost shape unchanged: RDCOST(lambda, coeff_bits +
     skip_fac_bits[0][0] + partition_fac_bits[0][NONE], dist)
     (rd_cost.c:1335), lambda = same kf chain (1527856 @ qindex 220);
   - split compare unchanged (parent_bias 1000, split-rate at ctx 0,
     depth-early-exit ths 50/0) BUT `use_accurate_part_ctx=true` at M6 so
     the x2 SPLIT-rate penalty is OFF (svt_aom_partition_rate_cost raw:
     1195/1465/2020 at 64/32/16, drifting per SB — see 5).
   - PD0 walk captures (verbatim in the dumps): q55 g64 all-PARENT ->
     64x64 NONE (parent 1791569177 <= split 2270708565); q40 g64 64->SPLIT
     (1176293547 > 956921042) with all four 32x32 PARENT -> 4x32 NONE;
     q20 g64: also 4x32 NONE (M6 tree is SHALLOWER than M9's 16x16 tree);
     g128 q40: SBs 0-2 64-NONE, SB3 SPLIT (M9 splits all four).
5. **update_cdf is ON at M6** (`svt_aom_get_update_cdf_level_allintra`: 2
   for M4..M6 -> update_se=1, update_coef=1, update_mv=0) — MD rate tables
   are REBUILT per SB from the evolving frame context
   (enc_dec_process.c:2991-3044): ec_ctx_array[sb] = md_frame_context for
   the first SB, else left-neighbor context (3x) averaged with top-right
   (1x) via avg_cdf_symbols (pic_based_rate_est=false, enc_handle.c:4617),
   then svt_aom_estimate_syntax_rate + estimate_coefficients_rate
   regenerate rate_est_table. Observed: PD0 64x64 SPLIT rate drifts
   1195 -> 1221 -> 1244 -> 1268 across the 4 SBs of g128 q55. Single-SB
   64x64 frames always use the default tables (chain starts at
   md_frame_context).
6. **The M6 PD1 leaf layer is materially different from eff-M9** (config
   dump: intra_lvl=6, txt_level=8, txs_level=3, cfl_level=4, chroma_level=5,
   nic_level=6, filter_intra level 2, rate_est_level=1, rdoq f(coeff_lvl)
   same policy, spatial_sse 3, lambda_weight 150):
   - g64 q40 leaf (0,0) 32x32 picks **use_filter_intra=1 with
     FILTER_DC_PRED** (PD1WIN fi_mode=0; the tile then codes the CDF5
     filter_intra_mode symbol icdf=[23819,19992,15557] our writer never
     emits), leaves (0,32)/(32,32) pick **H_PRED** (mode=2);
   - g64 q20 leaves: DC,DC,DC,H — so C's M6 y_modes differ from both our
     homegrown picks and from C's own M9 picks; closing any q20/q40 cell
     requires the M6 candidate funnel (MDS0 SATD -> NIC -> MDS3 full loop
     with filter-intra + TXT + real rate ctx (rate_est_level=1 ->
     update_skip_ctx_dc_sign_ctx=1) + per-SB updated cost tables).
   - g64 q55 leaf = 64x64 DC, txdep 0, DCT_DCT — identical decisions to
     M9; **C's whole q55-g64 tile is op-for-op identical between p6 and
     p10** (0 diff lines, W-op streams equal incl. rng), so our existing
     C-exact leaf coder already produces the exact tile once the PD0 tree
     is driven at p6.
7. **CDEF at M6 = search level 7** (allintra split, enc_mode_config.c:3543:
   M6 -> 7; cdef_recon_level=0 -> zero_fs_cost_bias=0, no mse scaling):
   4 candidate strengths {0, 60, 2, 62} (first_pass {pf_gi[0], pf_gi[15]},
   second_pass +2 each), subsampling_factor=4 (luma rows; chroma 4x4 ->
   capped to 1), uv candidates only {0, 60} (second-pass uv = -1 ->
   sentinel default_mse_uv*64 = 1040400*64 = 66585600), uv_from_y=false,
   damping = 3+(qindex>>6) both planes (luma; -1 chroma inside kernel),
   use_qp_strength=false -> the finish_cdef_search RD path: lambda from
   svt_aom_lambda_assign = **the kf full-lambda chain WITHOUT the
   lambda_weight multiply** (1303771 @ 220 = 1527856*128/150 exactly;
   211804 @ 160; 21888 @ 80), cost = RDCOST(lambda,
   av1_cost_literal(sb_count*i + nb*12), tot_mse*16) over i=0..3 signal
   bits, joint_strength_search_dual over (y,uv) pairs, then filter_map
   remap. Captures: g64 q55 mse0=[885020,900992,875920,892836] -> best
   gi 2 -> strength 2 = (pri 0, sec 2), uv gi 0 -> (0,0), damping 6,
   cdef_bits=0 — exactly the C FH the differ shows. g128 q20 picks y=2;
   g64 q20/q40 + g128 q40/q55 pick y=62 (pri 15, sec 2).
   The mse rows come from svt_av1_cdef_search (cdef_process.c:334-640):
   per 64x64 filter block, dlist = non-skip 8x8s, svt_cdef_filter_fb per
   strength on the POST-DEBLOCK recon with 2px halo, mse =
   compute_cdef_dist(subsampled rows) * subsampling_factor; V-plane mse
   accumulates INTO mse[1] (uv joint).
8. Per-cell requirement stack (updated):

   | cell | close requires |
   |------|----------------|
   | g64 q55 | M6 PD0 tree (64 NONE, same leaf coder) + CDEF search port |
   | g64 q40 | + LR wiener (search+FH+tile syntax+apply) + M6 leaf funnel (filter-intra RDO picks fire) |
   | g64 q20 | same as q40 (H_PRED leaf, wiener luma) |
   | g128 q20 | M6 PD0 tree + per-SB cost-table refresh + M6 leaves + CDEF |
   | g128 q40 | + LR wiener | 
   | g128 q55 | M6 PD0 (tree==M9) + per-SB tables + leaf deltas + LR wiener + CDEF |

   Fix order this chunk: differ-walker fix -> PD0_LVL_1 port (drives all
   p6 still trees) -> CDEF live search port (closes g64 q55) -> LR /
   leaf-funnel documented as the next subsystems.

## 2026-07-14 (M6 chunk, landed): PD0_LVL_1 + CDEF search — matrix holds 30/36, every p6 divergence moved strictly later and is component-classified

Three commits (each differ-verified, gates green):

1. `0e5b9a129` — differ FH walker fix (tile_info increment bits for
   multi-SB frames per spec 5.9.15 + lr unit-size fields per 5.9.20).
   All 6 gradient-p6 FH walks now decode correctly and match the
   instrumented ground truth.
2. `4fe2b1bec` — **prediction-based PD0 (PD0_LVL_1) ported**
   (pd0.rs: tx_quant_core split out; cost_coeffs_txb_pd0 = verbatim
   svt_av1_cost_coeffs_txb at zero contexts with the fast_coeff_est=2
   half-scan slice + intra DCT_DCT tx-type rate for 8x8/16x16;
   lvl1_block_cost at qindex+0/no-subres; undoubled split rate; no
   depth-set variance cap). Pipeline: still frames at presets 6..8 now
   drive partitions from the M6 PD0 + the existing fixed-tree leaf
   coder, c_quant extended to >= 6, leaf lambda preset-pinned on the
   PD0 path. 17 instrumented block costs + 3 trees pinned in unit tests.
   **Every partition symbol in every gradient p6 stream now matches C.**
3. `87045f52f` — **CDEF RDO search ported** (cdef.rs: packed
   svt_cdef_filter_fb search mode, subsample-4 luma / full chroma mse,
   uv sentinel, joint_strength_search_dual + finish RD with the
   unweighted kf lambda). finish_cdef_rd pinned against 3 instrumented
   capture sets. **No cdef FH field differs on any cell any more.**
   cdef_bits>0 outcome falls back to the qp path (per-SB cdef_idx
   syntax unported — documented; all tracked cells pick bits=0).

Matrix `benchmarks/identity_matrix_2026-07-14.tsv`: **30/36**, zero
regressions (all uniform + all gradient M10/M13 IDENTICAL). Gates:
recon-parity 216/0 (CDEF firing on 127 streams — count moved because
s6/s8 now use searched strengths), decode conformance 525/0 mono +
700/0 chroma under aomdec AND dav1d, `cargo test --workspace` 588/0
(no test-input changes).

### Remaining divergence per cell (all strictly later than before)

| cell | first divergence | owning subsystem |
|------|------------------|------------------|
| g64 q20 | FH `lr_type[0]` C=2(WIENER) vs 0 | LR wiener search |
| g64 q40 | FH `lr_type[0]` | LR wiener search |
| g128 q40 | FH `lr_type[0]` | LR wiener search |
| g128 q55 | FH `lr_type[0]` | LR wiener search |
| g64 q55 | tile op 2: leaf y_mode C=DC vs SMOOTH | MDS3 leaf compare |
| g128 q20 | tile op 2565: use_filter_intra C=1 vs 0 (one 32x32 leaf; 2565 ops match incl. every partition + all leaf y_modes before it) | filter-intra RDO |

### Next-op spec A: MDS3 leaf compare (closes g64 q55)

Instrumented per-candidate decomposition (MDS3CAND/MDS3RATE dumps,
g64 q55, MDS3 set {SMOOTH,DC,V,H} in that order, all reaching MDS3):

| mode | fast_luma | fast_chroma | ybits | cb/cr bits | ydist | cb/crdist | full |
|------|-----------|-------------|-------|------------|-------|-----------|------|
| SMOOTH | 1556 | 1292 | 177025 | 112/112 | 11043824 | 4384/4384 | 1956055335 |
| DC | 547 | 273 | 176560 | 112/112 | 10963760 | 0/0 | **1937245493** |
| V | 2874 | 1033 | 175503 | 112/112 | 10963760 | 16384/16384 | 1947497507 |
| H | 2555 | 1009 | 176514 | 112/112 | 10963760 | 16384/16384 | 1949490882 |

- `full = RDCOST(1527856, flr + fcr + ybits + cbbits + crbits +
  tx_size_bits(1280 = tx_depth CDF3 sym0 @ icdf 26986) +
  skip_fac[0][0](26), ydist + cbdist + crdist)` — verified additive on
  all four rows (svt_aom_full_cost, rd_cost.c:1417-1430;
  block_has_coeff path, skip_coeff_ctx 0).
- dists are spatial SSE << 4; the chroma dists are pure EDGE-FILL
  artifacts (first block, no neighbors): V = (128-127)^2*1024 << 4 =
  16384, H via the 129 left fill, DC exact -> 0. eob 521 on every
  candidate (the RDOQ path), cb/cr eob 0 -> skip-txb cost 112 each.
- ybits = svt_av1_cost_coeffs_txb over the RDOQ-optimized levels at
  ctx0 with `mds_fast_coeff_est_level = 1` (FULL scan — the PD0 port's
  /2 slice must be parameterized).
- flr = kf_y_mode cost at ctx (0,0) + angle0 (CDF7 sym 3) for V/H;
  fcr = uv_mode cost (CFL-allowed row conditioned on y) + uv angle0
  for V/H. All from default CDFs already in the entropy crate.
- Architectural prerequisite: CHROMA terms in the decision stage —
  today chroma is predicted/coded only inside the entropy walk, so the
  decision stage lacks chroma neighbor state (first-block cells like
  g64 q55 need only the 127/129/128 fills; general blocks need the
  walk's chroma recon at decision time).
- NIC boundary: at q40 g64 the MDS3 set is pruned to 3/3/2 candidates
  per block (PD1WIN ncand) by MDS0 SATD ranking + nic_level 6 — the
  funnel port is required for cells where a pruned candidate would
  win the naive 4-way compare.

### Next-op spec B: LR wiener search (closes the 4 lr_type cells)

M6 controls (source + dumps): `wn_filter_lvl 4` -> enabled, use_chroma,
filter_tap_lvl 2 (**5x5 taps for luma AND chroma** — WIENER_WIN_CHROMA),
use_refinement 0 (no finer search: taps come straight from the solve);
`sg_filter_lvl 0` -> sgrproj NEVER searched (rest_finish force-type
WIENER-vs-NONE only, restoration_pick.c:1565). Pipeline to port:
1. `svt_av1_compute_stats` (integer M/H over dgd=POST-CDEF recon vs
   src, restoration_pick.c:652) per 256-unit (single unit at 64/128).
2. `wiener_decompose_sep_sym` (double linsolve) + `finalize_sym_filter`
   (tap quantize/clamp) + `compute_score > 0` revert
   (restoration_pick.c:1360-1370).
3. `finer_tile_search_wiener_seg` at use_refinement=0 = one
   `try_restoration_unit` = wiener apply + SSE (needs
   av1_wiener_convolve_add_src + stripe boundary setup).
4. `search_wiener_finish`: bits_none=768 / bits_wn = 13120-ish via
   count_wiener_bits<<4 vs x->wiener_restore_cost, RDCOST_DBL at
   rdmult = unweighted kf lambda (1303771 @ q220); frame pick in
   rest_finish_search. Captured picks: luma WIENER at g64 q20/q40 +
   g128 q40/q55 with taps in the LRWNSEG dumps (e.g. g64 q55
   v=[0,8,-17,18,-17,8,0] h=[0,-8,26,-36,26,-8,0], sse 671191->670249,
   cost loses -> NONE at that one cell).
5. FH lr_params unit-size bits (shift 2 -> 256) + per-LRU tile syntax
   (wiener_restore bool CDF 11570 + refsubexpfin tap deltas at the
   LRU-covering SB) + the decoder-exact APPLICATION (stripe machinery)
   for recon parity.

### Latent for the 128 cells: per-SB MD rate-table refresh

`cdf_ctrl.enabled` at M6 (update_cdf_level 2): rate_est_table rebuilt
per SB from ec_ctx_array chained md_frame_context -> left(3x)/
topright(1x) avg (enc_dec_process.c:2991; serial for 2-SB-wide frames).
Observed: 64x64 SPLIT rate 1195 -> 1221 -> 1244 -> 1268 across g128
q55's SBs. The PD0_LVL_1 port prices every SB with the default tables
(exact for SB 0 / all single-SB frames); the g128 partition symbols
happen to match C anyway on all tracked cells (differ-verified), but
any future 128-cell tree flip should suspect this first.

## 2026-07-14 (wave2, LR-wiener chunk): Wiener loop restoration END-TO-END — search proven C-exact on C's inputs; all 4 lr cells move strictly later into the M6 leaf funnel

Four commits (`a724ebdc0` kernel+machinery+tap-coding, `d3c6bb20e`
search+signaling+application+pipeline, `bbf1ddb69` LR counters,
`6ccc102f9` golden-test tracking). Matrix
`benchmarks/identity_matrix_check.tsv`: **30/36 held, zero regressions**,
every gradient-p6 first divergence strictly later and tile-op-classified.

### What landed (all differentially proven)

1. **Kernel + stripe machinery** (`svtav1-dsp/src/restoration.rs` +
   `tests/c_parity_wiener.rs`): `svt_av1_wiener_convolve_add_src_c` port
   (the InterpKernel base/offset pointer dance cancels exactly — fuzz
   proves it), `svt_av1_compute_stats_c`, the full solver chain
   (`linsolve_wiener`/`update_{a,b}_sep_sym`/`wiener_decompose_sep_sym`/
   `finalize_sym_filter`/`compute_score`), `svt_extend_frame`, and the
   COMPLETE `svt_av1_loop_restoration_filter_unit` stripe walk
   (RESTORATION_UNIT_OFFSET=8 geometry, setup/restore of the
   deblock/CDEF boundary lines, 16px proc-unit width round-up).
   400-case kernel fuzz over the full signalable tap space + 200-case
   filter_unit fuzz (luma+chroma geometry, both need_boundaries arms,
   random boundary lines; dst AND post-call data byte-equal — the
   setup/restore round trip restores exactly). Boundary capture ports
   (`save_{deblock,cdef,tile_row}_boundary_lines`) follow the two-pass
   scheme (dlf_process.c:134 after_cdef=0, cdef_process.c:707 =1).
2. **Tap-coding chain** (`svtav1-entropy/src/lr.rs` +
   `tests/c_parity_lr_syntax.rs`): recenter/quniform/subexpfin/
   refsubexpfin write+count (entropy_coding.c:2895-3046) — EXHAUSTIVE
   (ref, v) byte+count parity over all three tap alphabets through a
   fresh od_ec coder; `write_wiener_filter` ref chaining
   (entropy_coding.c:4074); `count_wiener_bits`
   (restoration_pick.c:1005). `FrameContext.wiener_restore_cdf` =
   AOM_CDF2(11570) (ICDF 21198 = the trace fingerprint).
3. **Search** (`svtav1-encoder/src/restoration.rs`):
   `search_restoration_still` = restoration_seg_search +
   rest_finish_search at the allintra controls
   (`wn_filter_ctrls_allintra`: lvl 3 presets <=3 with one-step
   refinement, lvl 4 <=6 without; **sgrproj NEVER searched**,
   sg_filter_lvl=0). Key C facts baked in: try-unit filtering runs
   need_boundaries=0 (`use_boundaries_in_rest_search = 0`,
   enc_handle.c:4483) on the 4/3-extended post-CDEF recon; the NONE
   frame walk carries ZERO bits (search_norestore_finish); the per-unit
   compare prices the wiener_restore flag ([768, 320] from the default
   CDF — pinned) + `count_wiener_bits` at the SEARCH window (win-5 luma
   at these presets) against the ref-chained previous WIENER pick;
   RDCOST_DBL at `x->rdmult` = the unweighted kf lambda
   (21888/211804/1303771 @ qindex 80/160/220 — pinned).
4. **Signaling**: FH lr_params (spec 5.9.20 — per-plane lr_type pairs,
   unit-size bits 256 -> (1,1), lr_uv_shift when chroma restores) via
   `LrSignal`/`write_key_frame_header_full_lr`; per-SB tile syntax
   (`corners_in_sb` + `write_lr_for_sb` at the head of the SB walk,
   BEFORE the partition symbol — decoder order decodeframe.c:1325;
   refs reset per tile like svt_av1_reset_loop_restoration). The tile
   is RE-walked when the search signals wiener (the walk is a
   re-runnable pass; decisions are entropy-state-independent, verified
   by debug_assert on the chroma recon) — C's EC kernel runs after
   rest_process and sees the same state.
5. **Application**: `apply_restoration_frame` =
   svt_av1_loop_restoration_filter_frame (extend 3/3, per-unit filter
   WITH boundaries into a dst buffer, crop copy-back) on the OUTPUT
   recon only; prediction sources untouched.

### Instrumented ground truth + the C-dgd validation (the chunk's key experiment)

Scratch build `/root/svtav1-instr` (SVT_LRDBG prints in
restoration_pick.c; **instrumented OBUs byte-identical to baseline on
all 6 gradient-p6 cells** before trusting any dump; deleted after).
Captures at `docs/captures/gradient_*_p6.lrdbg.txt`: per-cell solved
taps (pre/post finalize), compute_score, sse_none/sse_wn, bits/rdmult/
RDCOST decomposition, per-unit + frame picks.

**Feeding OUR search C's exact post-CDEF dgd reproduces C's solved taps,
per-unit picks and frame types on ALL SIX cells** (incl. the 128x128
unit geometry). Two 4KB dgd fixtures + tap expectations are pinned as
`svtav1-encoder/tests/lr_search_c_capture.rs`; RD-arithmetic pins
(restore costs 768/320, RDCOST_DBL rows, ctrls table) live in-module.
Frame-type decisions match C at every cell even from OUR recon
(WIENER luma at g64 q20/q40 + g128 q40/q55; NONE at g64 q55 + g128
q20; chroma NONE everywhere — the flat-chroma ill-posed solve keeps
default taps with score 0, also pinned).

### Gates (all green, LR firing)

- recon-parity **216/0** with **59/216 streams signaling wiener, 107
  RUs restored** (incl. chroma units; speeds 2/4/6 run the search,
  speed 2 exercises the refinement arm) — the applied LR equals
  aomdec's byte-for-byte everywhere it fires.
- decode conformance **525/0 mono + 700/0 chroma** under aomdec AND
  dav1d (LR syntax parses on both reference decoders).
- `cargo test --workspace` **605/0** (+17 new; the one pre-existing
  test touched — golden `wiener_filter_identity` — now drives the
  C-exact kernel with a STRICTER exact-equality assertion, replacing
  the deleted sketch's +-1-tolerance check).
- identity matrix **30/36**, zero regressions.
- Differ hardening: `identity_diff.py` compares the LAST coder segment
  (RESET..DONE) per side — the two-pass walk logs both passes.

### Honest close-out: the 4 lr cells did NOT reach IDENTICAL — and exactly why

The brief's target (34/36) assumed lr_type was these cells' ONLY gap;
it was only their FIRST. With LR C-exact end-to-end, each first
divergence moved strictly later, into TAP BITS whose values derive
from the recon:

| cell | was | now | first-diverging syntax |
|------|-----|-----|------------------------|
| g64 q20 | FH `lr_type[0]` | tile op 10 | luma tap bit (v-taps) |
| g64 q40 | FH `lr_type[0]` | tile op 17 | luma tap bit (v-tap2 literal) |
| g128 q40 | FH `lr_type[0]` | tile op 7 | luma tap bit |
| g128 q55 | FH `lr_type[0]` | tile op 11 | luma tap bit |
| g64 q55 | tile op 2 | tile op 2 (unchanged) | leaf y_mode (MDS3 compare) |
| g128 q20 | tile op 2565 | tile op 2565 (unchanged) | use_filter_intra |

The tap values are pure functions of the post-CDEF recon, and C's
recon at these cells embeds M6 leaf picks our funnel doesn't make yet
(g64 q40: 32x32 leaf (0,0) FILTER_DC_PRED + H_PRED leaves — with C's
taps substituted our stream matches C op-for-op THROUGH the LR syntax,
partition and first-leaf prefix up to op 37 = `use_filter_intra`;
g64 q20: H_PRED leaves; g128: leaf deltas). The C-dgd fixtures prove
the search contributes zero divergence. **All six residual cells are
owned by one subsystem: the M6 leaf funnel** (filter-intra RDO + MDS0
SATD/NIC pruning + MDS3 4-candidate compare with chroma-in-decision,
spec'd as next-op spec A above) — landing it closes the leaf picks,
which fixes the recons, which makes the already-C-exact search emit
C's taps: potentially all 6 cells at once.

## 2026-07-14 (wave2, M6 leaf-funnel chunk): MATRIX COMPLETE — 36/36 byte-identical

The M6 leaf intra-mode decision funnel — the last remaining subsystem —
is ported C-exactly; all 6 gradient preset-6 cells print
`VERDICT: streams IDENTICAL` and `tools/identity_matrix.sh check`
reports **36 / 36 byte-identical** (scoreboard
`benchmarks/identity_matrix_check.tsv`). Four commits:

1. `265830cf7` — instrumented M6FNL captures for all 6 cells
   (docs/captures/gradient_*_p6.m6fnl.txt; instrumented OBUs verified
   byte-identical to baseline before trusting any dump).
2. `2fc88e564` — C-exact aom Hadamard/SATD kernels (MDS0 fast dist) +
   the existing `predict_filter_intra` pinned bit-exact vs
   `svt_av1_filter_intra_predictor_c` (cref differential suites:
   700 hadamard blocks, 5 modes x 4 sizes x 200 edge patterns).
3. `725fd3b09` — the funnel itself (`leaf_funnel.rs`) + walk wiring:
   closed all 3 gradient-64 cells.
4. `661efa7bc` — the per-SB CDF refresh chain (funnel + M6 PD0 tables):
   closed all 3 gradient-128 cells.

### The verified C funnel at allintra M6 (all instrumented; file:line in leaf_funnel.rs docs)

- **Config**: intra_level 6 -> mode_end SMOOTH / angular_pred_level 4
  (only V,H survive; no angle deltas; no prune flags — `is_dc_only_safe`
  is DEAD at M6, its arming flag prune_using_edge_info is 0), filter
  intra level 2 -> FILTER_DC_PRED only (<= 32x32), nic_level 6
  (scaling 6/6/6 over I-slice class-0 base 64/32/16, qp-scaled;
  pruning ths mds1 1200/rank 3, mds2 15/rank 1/rel-dev 5, mds3 15,
  class ths dead for the single intra class), staging mode 1 (MDS1
  runs, MDS2 bypassed), CHROMA_MODE_1 (uv follows luma; no independent
  uv search), cfl level 4 (gated by the chroma-complexity detector —
  never arms on flat chroma), txt_level 8 (intra groups 4 / 5;
  DCT-only >= 32 via the ext-tx sets; depth-1 group offset 3), txs
  level 3 (intra sq max depth 1, prev-depth coeff exit 1),
  spatial_sse SSSE_MDS3, rate_est_level 1 (REAL txb_skip/dc_sign
  contexts in cost AND RDOQ — unlike the eff-M9 path's 0/0),
  update_cdf_level 2 (per-SB rate-table refresh), mds0 hadamard SB
  flag true, cand_reduction 0 (tot_itr = 1: single MDS0 pass).
- **MDS0**: whole-block prediction from MD recon neighbors,
  `hadamard_path` SATD (32-capped aom-hadamard tiles), fast cost =
  RDCOST(lambda, flr + fcr, satd << 4); flr = kf_y[above][left] +
  angle0 (V/H) + fi flag/mode bits (eligible DC blocks); fcr =
  uv_mode[cfl_allowed][y][uv] + uv angle0.
- **MDS1**: luma-only, depth 0, DCT_DCT, `quantize_b` (NO rdoq —
  mds_do_rdoq false: captured eob 996 vs fp 997 vs trellis 528 at q40),
  freq-domain SSE (+ three_quad, RIGHT_SIGNED_SHIFT (1 - scale) * 2),
  real contexts, full cost with zero chroma dist/bits but fcr counted.
- **MDS3**: per candidate — TXS depths 0..1 with per-txb prediction
  from the depth-local canvas + TX-local (dc_sign|cul) overlay
  contexts; per-txb TXT search (SATD early exit th 10 qp-scaled, rate
  th 100); RDOQ per the frame policy (q20 HIGH -> rdoq 0 ->
  quantize_b at MDS3 too); spatial SSE << 4; chroma pred/residual/
  quant/SSE at tx_size_uv with the uv-derived tx type (UV_H ->
  DCT_ADST etc. — capture-confirmed ttuv 1/2/3) + per-plane contexts;
  full cost = svt_aom_full_cost (skip_fac[skip_ctx], tx_size bits at
  the chosen depth). Winner = lowest full cost, first-in-order ties.
- **Per-SB chain**: ec_ctx_array[sb] = left copy (col > 0) / top-right
  copy (column 0, multi-row) / md_frame_context (SB 0), evolved by the
  SB's coded symbols; MdRates AND M6Pd0Tables (split 1195 -> drifting
  rates) rebuilt per SB. Simulated by re-coding each decided SB
  through the real entropy walk against the chain contexts (bypass
  enc-dec == MD symbols are the coded symbols).

### What landed (svtav1-rs only)

- `svtav1-dsp`: `aom_hadamard_8x8/16x16/32x32` + `aom_satd`
  (differential vs C); `predict_filter_intra` differential coverage.
- `svtav1-entropy`: `FrameContext.filter_intra_mode_cdf` +
  `write_filter_intra_mode` (CDF5); `write_use_filter_intra` accepts
  used=1.
- `svtav1-encoder/leaf_funnel.rs`: MdRates (syntax + coeff tables from
  arbitrary chained contexts), the staged funnel, cost_coeffs_txb
  (full-scan, real-ctx, any plane/type generalization of the pd0
  port), TX-unit pipeline, chroma detector, exact NIC/pruning walks.
- `partition.rs`: BlockDecision carries fi/uv/tx_depth/per-txb/chroma
  decisions; `encode_fixed_tree` routes leaves to the funnel at
  preset 6 + 4:2:0 still.
- `pipeline.rs`: funnel state + per-SB chain in `encode_tile_rows`;
  the walk codes decided uv_mode (+ uv angle 0), use_filter_intra +
  fi mode symbol, tx_depth (incl. depth-1 per-txb residuals with
  per-txb contexts + chosen-tx-dims txfm records + per-txb deblock
  geometry), copies decision chroma recon, chroma tx types derived
  from uv mode.
- `pd0.rs`: `build_m6_pd0_tables_from_ctx` (chained partition/skip/
  coeff/tx-type rates); LVL_1 costs consume them.
- `quant.rs`: `build_coeff_cost_tables_from_fc`.

### Gates

- identity matrix **36/36** (`benchmarks/identity_matrix_check.tsv`) —
  all uniform + ALL gradient cells at presets 13/10/6, both sizes,
  all qps; zero regressions.
- `cargo test --workspace --no-fail-fast`: **609 passed / 0 failed**
  (+4 new differential suites; no test-input changes).
- recon-parity + decode conformance: see the gate outputs in the
  session close-out (run after this doc's commit).

### Honest residual gaps (none owns a matrix cell)

- **CFL evaluation**: when the chroma-complexity detector arms
  (complex chroma, <= 32x32), C evaluates cfl_prediction and may pick
  UV_CFL; unported — we keep the non-CFL uv mode. Never arms on the
  tracked cells (flat chroma). Streams remain valid on content where
  it arms; they may differ from C's.
- **avg_cdf_symbols**: frames wider than 2 SBs chain left-only where C
  averages left 3x + top-right 1x. No tracked frame is that wide.
- **Funnel scope**: preset 6 + 4:2:0 still only. Presets 7/8 keep the
  homegrown leaf coder (C uses intra_level 7 / prune_using_best_mode
  there — unverified constants); mono keeps the legacy path (C cannot
  emit mono).
- The MDS3 chroma quantize prices RDOQ with the funnel's own chain
  tables; the walk codes the funnel's coefficients verbatim (no
  re-quantization), so decision == stream by construction.

## 2026-07-14 (wave2, M7/M8/M9 leaf-funnel chunk): presets 6-10 COMPLETE — matrix 47 -> 60/132

The C-exact leaf intra funnel now covers still/420 allintra presets 6, 7,
8, and eff-M9 (presets >= 9 clamp to M9, enc_handle.c:4634). Every tracked
identity cell at presets 6-10 (uniform + gradient, {64,128}, q{20,40,55})
prints `VERDICT: streams IDENTICAL` — **60/60 for presets 6-10**, lifting
the all-presets matrix from 47/132 to **60/132** (M0-M5 unchanged at 0/60;
they run PD0_LVL_1 with lower intra/rdoq/txt levels — unported). Two commits.

### Verified C config deltas (instrumented `svt_aom_sig_deriv_enc_dec_allintra` dump)

Scratch build `/root/svtav1-instr` (SVT_M7DBG env-gated prints in
enc_mode_config.c + product_coding_loop.c + mode_decision.c; instrumented
OBUs byte-identical to baseline; deleted after). Config dump captured for
M6/M7/M8/M9 (docs/captures/m7m8m9_funnel.txt):

| field | M6 | M7 | M8 | eff-M9 |
|-------|-----|-----|-----|-----|
| intra_level | 6 | 7 | 7 | 8 |
| prune_using_best_mode | 0 | 1 | 1 | 1 |
| prune_using_edge_info (is_dc_only) | 0 | 0 | 0 | **1** |
| filter_intra_level | 2 | **0** | **0** | 0 |
| nic_level | 6 | 7 | 11 | 11 |
| -> nic scaling nums | 6/6/6 | **4/4/4** | **0/0/0** | 0/0/0 |
| -> nic bases (mds1/2/3) | 24/12/6 | 16/8/4 | 1/1/1 | 1/1/1 |
| -> mds{1,2,3}_cand_base_th | 1200/15/15 | 1200/15/15 | **1/1/1** | 1/1/1 |
| txt_level | 8 | 10 | 10 | **0 (off)** |
| txs_level | 3 | 3 | **0 (off)** | 0 (off) |
| rate_est_level | 1 | 4 | 4 | 0 |
| -> update_skip_ctx_dc_sign_ctx | 1 (real) | **0** | 0 | 0 |
| -> coeff_rate_est_lvl | 1 | 2 | 2 | 0 |
| update_cdf_level | 2 (chain) | **0 (static)** | 0 | 0 |
| cfl_level | 4 | **0** | 0 | 0 |
| chroma_level / rdoq | 5 / f(coeff_lvl) | 5 / same | 5 / same | 5 / same |

### The binding fixes (each proven by the differ + per-candidate C captures)

1. **Chroma coeff-rate approximation (OPT_APPROX_COEFF_RATE)** — the term
   that flipped M7 leaf picks. At coeff_rate_est_lvl >= 2 (M7/M8) the chroma
   coeff bits are the eob-based `skip_chroma_rate_est` estimate
   (full_loop.c:1922): th = (tx_w_uv*tx_h_uv)>>6; **eob==0 -> 0 bits**,
   eob<th -> 3000+eob*500, else full. M6 (lvl 1) prices the real
   cost_skip_txb (3578 for a flat-chroma DC skip). Per-candidate C captures
   for g64 q40 p7 (0,0): C DC full 243123564 < H 247349473; our funnel had
   DC 246592645 > H 244810199 (H wins) purely because it charged 3578x2 for
   DC's flat-chroma skip that C charges 0 for. With the approximation:
   DC wins, matching C.
2. **prune_using_best_mode** (product_coding_loop.c:1688): the MDS0
   order-dependent skip (H when V beats DC, SMOOTH when DC stays best) —
   changes the candidate pool that reaches MDS3.
3. **0/0 coeff contexts** (rate_est 4/0 -> update_skip_ctx_dc_sign_ctx 0):
   all txb_skip/dc_sign/skip_coeff contexts priced at 0 in cost AND RDOQ
   (cul never accumulates, full_loop.c:1880), unlike M6's real contexts.
4. **txt off for eff-M9** (txt_level 0): DCT_DCT forced for every tx size,
   incl. < 32 blocks. Without it the funnel over-searched a 16x16 dc-only
   block and picked ext-tx index 3 where C (txt off) codes DCT (index 1).
5. **NIC bases + M8/M9 = 1/1/1**: at nic 1/1/1 only the MDS0 SATD winner
   reaches MDS3, so the mode is the SATD pick; TXS/coeff_rate_est_lvl
   never affect a single-candidate MDS3.

### eff-M9 routing + the is_dc_only gate

eff-M9 partition trees are all-DC-winner across every tracked cell; only
g64 q55's single 64x64 leaf is NON-dc-only (var 5425 >= 2000) — where the
homegrown coder had picked SMOOTH but C's funnel picks DC (op-2 divergence).
Routing all still/420 leaves through the funnel with the is_dc_only gate
(dc_cand_only -> {DC_PRED} when the variance gate fires) closes g64 q55 p9
and reproduces every dc-only leaf; the funnel's dc-only path needed txt-off
(fix 4) to match. The per-SB CDF chain stays M6-only (update_cdf_level 0
elsewhere -> static default tables).

### Gates

- identity matrix **60/60 for presets 6-10** (M6/M7/M8/M9/M10 all 12/12);
  all-presets total **60/132** (M0-M5 unported).
- `cargo test --workspace --no-fail-fast`: **613 passed / 0 failed**.
- recon-parity **216/0** (eff-M9 funnel self-consistent on real content;
  CDEF 127/216, wiener 58/216).
- decode conformance **525/0 mono + 700/0 chroma** (aomdec + dav1d).

### Residual gaps (none owns a preset 6-10 cell)

- Presets 0-5: PD0_LVL_1 with lower intra_level (1/2/6), rdoq_level 1,
  txt 2/3 — unported (0/60).
- The M7/M8 funnel reuses M6's txt group counts (5/4) rather than 3/2
  (txt_level 10); harmless because every tracked M7/M8 block is >= 32 ->
  DCT-only. A tracked cell with a < 32 M7/M8 block would need the group
  counts parameterized.
- Mixed-frame chroma: a 420 eff-M9 frame with BOTH dc-only and non-dc-only
  leaves would need the funnel decision chroma plane synced with the
  fast-path leaves. No tracked cell mixes (g64 q55 is a single leaf);
  routing all eff-M9 leaves through the funnel avoids the seam entirely.
- CFL, avg_cdf_symbols (>2-SB-wide chains), mono: as before.

## 2026-07-14 (wave2, M0-M5 chunk): FH filter searches ported — matrix 60 -> 96/132, every uniform cell at every preset byte-identical

Session scope: the M0-M5 frontier (was 0/12 per preset). Instrumented
scratch build `/root/svtav1-instr` (SVT_M5DBG + SVT_DLFDBG prints;
**all 24 instrumented OBUs byte-identical to baseline** before trusting
dumps; deleted after). Ground truth: `docs/captures/m0m5_config_dlf.txt`
(the complete per-preset config dump, the DLF search walks, and the M5
per-leaf winner + independent-uv captures for the funnel port).

### Verified per-preset config map (M5DBG CFG, source-cross-checked)

| field | M0 | M1 | M2 | M3 | M4 | M5 | M6 ref |
|---|---|---|---|---|---|---|---|
| intra_level / mode_end / ang | 1/12/1 | 1/12/1 | 1/12/1 | 1/12/1 | 1/12/1 | **2/12/2** | 6/9/4 |
| filter_intra lvl (fi_max) | 1 (4=all) | 2 (0) | 2 | 2 | 2 | 2 | 2 |
| nic_level (scal, mds1/2/3 cls) | 1 (20; ~0/25/25) | 3 (12; ~0/25/25) | 3 | 5 (6; 300/25/15) | 5 | **6 (6; 200/10/5)** | 6 |
| txt_level (groups, satd, rate_th) | 2 (6/6, 20, 250) | 2 | 2 | 2 | 3 (6/6, 15, 250) | **3** | 8 (5/4, 10, 100) |
| txs_level (sq depth, d1_off) | 2 (2, 0) | 2 | 2 | 2 | 3 (1, 3) | 3 (1, 3) | 3 |
| rdoq_level (ctrls) | 1 (cutnum 0, skip_uv 0, dct_only 0) | 1 | 1 | 1 | 1 | **1** | f(coeff_lvl) |
| rate_est_level | 1 | 1 | 1 | 1 | 1 | 1 | 1 |
| update_cdf_level | 1 | 1 | 1 | 1 | 2 | 2 | 2 |
| chroma_level (uv ctrls) | 1 (ind mds0, nic 16) | 2 (ind mds1, nic 8) | 4 (ind mds2, skip-dc, nic 1) | 4 | 4 | **4** | 5 (uv=luma) |
| cfl_level | 1 (itr 2, cplx 0) | 4 | 4 | 4 | 4 | 4 | 4 |
| depth refinement lvl (mode) | 6 (ADAPTIVE s1/e1 15) | 6 | 6 | 6 | 6 | **9 (ADAPTIVE s1/e1 10)** | 10 (PRED_ONLY) |
| nsq geom (min, hv4) / search | 2 (0, 1) / 5* | 2 / 12* | 2 / 16* | 2 / 18* | 3 (8, 0) / 0 | 3 / 0 | 3 / 0 |
| pd0_lvl | 0 | 0 | 1 | 1 | 1 | 1 | 1 |
| bypass_encdec | 0 | 0 | 0 | 0 | 1 | 1 | 1 |
| disallow_4x4 | 0 | 0 | 0 | 0 | 1 | 1 | 1 |
| dlf_level (conv_th) | 1 (0) | 1 | 1 | 1 | 2 (1) | 2 (1) | 5 (q path) |
| cdef search level | 2 | 3 | 3 | 3 | 5 | 5 | 7 |
| wn_filter lvl | 3 (refine) | 3 | 3 | 3 | 4 | 4 | 4 |
| SH intra_edge_filter | 0 | 0 | 0 | 0 | 0 | **1** | 0 |

*nsq_search levels shown at CLI qp 40 (seq_qp_mod offsets +2 applied by
`svt_aom_get_nsq_search_level_allintra`; qp <= 39 -> +3, qp 55 -> +0).

### Landed this session (each differ-verified, gates green)

1. `e2ae6569d` — **full-image deblock-level search** (still presets <= 5):
   C-exact `search_filter_level` hill-climb with real per-plane filter
   trials + SSE vs source, conv_th 0/1 split, chroma forced 0 when luma
   picks 0. Killed the FH `loop_filter_level C=0` class on every uniform
   cell (all M0-M4 uniform q40 cells went IDENTICAL immediately).
2. `46c5531c9` — **SH enable_intra_edge_filter=1 at M5** (the only
   allintra preset with the bit: intra_level 2 -> angular_pred_level 2,
   enc_mode_config.c:4036/6907/:18). All 6 uniform M5 cells IDENTICAL.
3. `5e4559a73` — **CDEF search candidate sets for levels 2/3/5** (M0 /
   M1-M3 / M4-M5: 48/20/6 candidate strengths, subsampling 1, uv
   sentinel on second-pass slots; search machinery generalized to N
   candidates).
4. `6999c0933` — **preset-5 recon containment + gate strengthening**:
   SH bit gated to still/420 (mono keeps 0), homegrown preset-5 leaf
   coder drops D45..D203 (V/H are exactly 90/180 deg — the decoder's
   edge filter skips them) until the M5 funnel predicts with the C edge
   filter. recon_parity + decode_conformance extended with speed 5
   (270 recon cases, 630 mono + 840 chroma streams — all green under
   aomdec AND dav1d).

### Matrix: 96/132 (`benchmarks/identity_matrix_allpresets.tsv`)

Presets 6-10: 60/60 (zero regressions). M0-M5: 36/72 — **every uniform
cell at every preset is byte-identical**; all 36 remaining cells are
gradient, all first-diverge at FH (35) or tile (1: g128 q55 p3), and
every class is a RECON CASCADE: the dlf/cdef/lr searches are C-exact on
C's inputs, but they run on OUR recon, which still embeds non-C leaf
decisions at M0-M5 (the funnel covers presets >= 6 only).

### The one subsystem that owns all 36 cells: the M0-M5 leaf decision layer

Verified facts for the port (captures in m0m5_config_dlf.txt):

- **Partition trees need NO new machinery for the tracked cells**: C's
  final M5 partition streams equal the M6/PD0 trees on ALL 12 gradient
  cells (partition-symbol streams p5 == p6 verified from the traces;
  same for M2-M4 pd0_lvl 1). Depth refinement (ADAPTIVE 6/9) evaluated
  extra depths — the M5DBG WIN dumps show 16x16 winners under 32x32
  PD0 picks — but they LOSE the inter-depth compare everywhere tracked.
  Port later as a latent-correctness item (like the >2-SB CDF chain).
- **M5 funnel deltas vs M6** (all in the config table): mode_end PAETH
  (13 modes), angular_pred_level 2 = deltas {-3, 0, +3} for all 8
  directional modes (`inject_intra_candidates` skips |delta| 1/2 at
  level >= 2, mode_decision.c:3268), SH-driven EDGE-FILTERED prediction
  (corner filter + per-side strength + <=8x8 upsample —
  enc_intra_prediction.c:181-215, kernels intra_prediction.c:156/
  C_DEFAULT:39), independent-uv search before MDS3 over the luma modes
  that reached MDS3 (`search_best_independent_uv_mode`,
  product_coding_loop.c:7778; skip when only-DC; uv_nic_scaling 1),
  txt groups 6/6 with satd th 15 / rate th 250, rdoq_level 1 (cut_off
  0 = full trellis; skip_uv 0; dct_dct_only 0), update_cdf chain also
  at preset 5 (M4..M6). M5 winners include SMOOTH_V (q40) and H+3
  angle deltas (q20) — capture-verified with full rate/dist/cost rows.
- **M0-M4 further deltas**: intra_level 1 (ALL 7 deltas injected),
  NSQ search at M0-M3 (levels 5-18 qp-dependent), 4x4 partitions at
  M0-M3, PD0_LVL_0 at M0/M1 (full PD0 with its own candidate search),
  bypass_encdec=0 at M0-M3 (the encode pass re-runs — MD == final only
  because intra MD already predicts from real recon; verify when
  chasing), filter-intra ALL 5 modes at M0, chroma ind-uv at mds0/mds1
  for M0/M1, cfl_level 1 at M0 (cplx_th 0 -> CFL evaluated for every
  <= 32x32 block: the CFL port becomes binding at M0).

Recommended attack order (next session): M5 funnel extension (candidates
+ edge-filtered prediction + ind-uv + txt/rdoq cfg — closes up to 6
cells), then M4 (identical config minus intra_level 2 -> 1 and depth
refinement 9 -> 6), then M2/M3 (+nsq search + bypass_encdec=0), then
M1/M0 (+PD0_LVL_0, +cfl_level 1, +fi all-modes, 4x4).

## 2026-07-14 (wave2, M5 leaf-funnel chunk): preset 5 COMPLETE — matrix 96 -> 102/132, presets 5-10 all 72/72

Session scope: the 6 gradient M5 cells (the prior chunk's attack order).
No fresh C instrumentation was needed: every briefed config delta was
re-verified against the C SOURCE this session (file:line below) and the
prior chunk's per-leaf captures (m0m5_config_dlf.txt WIN/INDUV rows)
supplied the ground truth; all 12 preset-5 cells went byte-identical on
the first full run after routing, so the captures were never
contradicted. C tree untouched (read-only greps only).

### Verified M5 funnel deltas vs M6 (C cites, all re-read this session)

- **Candidates**: intra_level 2 -> mode_end PAETH + angular_pred_level 2
  (set_intra_ctrls case 2, enc_mode_config.c:8477). Injection order
  (inject_intra_candidates, mode_decision.c:3254-3306): DC, then each
  directional mode V,H,D45,D135,D113,D157,D203,D67 with the delta
  counter loop -3..+3 skipping |1|/|2| at level >= 2 (:3268-3271) —
  per-mode delta order **-3, 0, +3** — then SMOOTH, SMOOTH_V, SMOOTH_H,
  PAETH, then FILTER_DC (fi level 2, fi_max 0). 30 candidates >= 8x8.
  The angular mask at level >= 4 covers D45..D67 ONLY (:3246-3250) —
  V/H stay in the M6/M7/M8 sets (the one regression this session
  caught: 14 cells, fixed 1099a6aa8).
- **Edge-filtered prediction** (SH enable_intra_edge_filter=1):
  build_intra_predictors' dr branch (enc_intra_prediction.c:181-215) —
  corner filter at txw+txh >= 24, per-side strength
  (svt_aom_intra_edge_filter_strength, intra_prediction.c:180),
  <=16-blk_wh upsampling (svt_aom_use_intra_edge_upsample :144,
  svt_av1_upsample_intra_edge C_DEFAULT/intra_prediction_c.c:39), all
  gated on p_angle != 90/180; filt_type = get_filt_type
  (enc_intra_prediction.c:20) = above/left COBED-BLOCK smoothness per
  plane (EntropyCtx above/left_uv_mode arrays added for the chroma
  side). Kernels differentially fuzzed vs libSvtAv1Enc.a
  (tests/c_parity_intra_edge.rs, 4 suites). Whole-block, sub-txb
  (av1_intra_luma_prediction, product_coding_loop.c:4072 — row_off/
  col_off geometry) and chroma (ss=1) all route through
  intra_edge::dr_predict.
- **Independent-uv at MDS3** (chroma_level 4, set_chroma_controls
  enc_mode_config.c:5779: ind_uv_last_mds=2, skip_ind_uv_if_only_dc=1,
  inter_vs_intra_cost_th=100, uv_nic 1): gate
  perform_ind_uv_search_last_mds (product_coding_loop.c:1461) — at
  least one MDS3 survivor with injected uv != UV_DC; the
  inter-vs-intra arm is I-slice-dead (MAX_MODE_COST*100 = 1.1e16, no
  u64 overflow). search_best_mds3_uv_mode (:7561): distinct survivor
  (uv, uv_delta) pairs in order + UV_DC appended, each full-looped once
  (rdoq on, spatial SSE, real ctx); per surviving luma mode the best
  pair by RDCOST(coeff_rate + svt_aom_get_intra_uv_fast_rate
  (rd_cost.c:476), dist), strict less. update_intra_chroma_mode
  (:7326) rewrites each MDS3 candidate's uv/uv_delta/fast chroma rate.
  Candidates are INJECTED with uv-follows-luma + the LUMA delta
  (ind_uv_avail reset per block, :9866).
- **txt_level 3**: groups 6/6 (get_tx_type_group :4358 over the
  tx_type_group table definitions.h:1094 — group 6 order FLIPADST_DCT,
  DCT_FLIPADST, ADST_FLIPADST, FLIPADST_ADST, V_ADST, H_ADST,
  V_FLIPADST, H_FLIPADST), satd_early_exit_th_intra 15 (:4724
  qp-scaled), txt_rate_cost_th 250 (:4787: RDCOST(rate,0)*1000 >
  dct_cost*th), d1 offset 3 (unchanged).
- **Unchanged vs M6** (dump-verified): nic_level 6 (same 1200/3, 15/5,
  15 pruning ths — set_nic_controls case 6, enable_skipping_mds1=0),
  staging MDS1+bypass-MDS2, rdoq_level 1 full trellis, rate_est_level
  1 (real contexts), txs_sq depth 1 + prevcoeff_exit, update_cdf 2
  (per-SB chain — gate extended to 5..=6), FILTER_DC candidate.
- **uv tx type**: full g_intra_mode_to_tx_type (mode_decision.c:2991)
  replaces the 4-mode subset (SMOOTH_V->ADST_DCT etc.).
- **Routing**: PD0 fixed tree + c_quant + funnel gates extended to
  preset 5 still/420. M5 depth refinement (ADAPTIVE lvl 9, s1/e1 10)
  evaluates extra depths but they lose the inter-depth compare on
  every tracked cell — coded tree == PD0 tree (capture partition
  streams p5 == p6, re-confirmed by 12/12 byte-identity incl. both
  128x128 chain cells).
- **Angle deltas end-to-end**: Cand/LeafChoice/BlockDecision carry
  y/uv deltas; the walk signals write_angle_delta with real values
  (was hardcoded 0).

### The recon-parity catch: deblock chroma TX dims (c8dd27091)

New-surface bug, pre-existing latent: the walk recorded tx_depth-1
blocks into DeblockGeom as four per-txb pseudo-blocks, so the chroma
edge mask derived chroma TX = luma_tx/2 (4px) where the decoder uses
the bsize-based av1_get_max_uv_txsize (block/2 — chroma never splits
with luma tx_depth): chroma filter length 4 vs C's 6 at block edges
flanked by depth-1 16x16 blocks. First armed by the M5 funnel
(c420_gradient_96pad_q20_s5: 28 U px off by 1-2 in boundary column
pairs; svtav1/examples/m5_chroma_repro.rs bisected it). DeblockGeom
now carries block dims (chroma TX + pu_edge) AND a per-mi luma TX grid.

### Gates (all green, verbatim tallies)

- identity matrix FULL sweep: **102 / 132 byte-identical**
  (benchmarks/identity_matrix_2026-07-14.tsv + .meta) — presets 5-10:
  72/72; presets 0-4: every uniform cell identical, all 30 gradient
  cells divergent (29 FH + 1 tile g128q55p3).
- `IM_PRESETS="5 6 7 8 9 10" identity_matrix.sh check`: 72/72.
- recon_parity: **270 passed, 0 failed** (CDEF fired 154/270 streams,
  2104864 px filtered / 879030 changed; LR wiener 87/270, 150 RUs).
- decode conformance (aomdec AND dav1d 1.5.1): mono **630/0**, chroma
  **840/0**.
- cargo test --workspace: 36/36 suites, 0 failures (new:
  c_parity_intra_edge 4 suites, m5 config pins, full uv-tx rows).

### Remaining for M0-M4 (the 30 gradient cells)

Unchanged from the prior chunk's plan (attack order M4 first: config
== M5 minus intra_level 2->1 — ALL 7 angle deltas per directional
mode — and dr 9->6; then M2/M3 +nsq_search +bypass_encdec=0 +4x4;
then M1/M0 +PD0_LVL_0 +cfl_level 1 +fi all-modes +ind-uv at mds0/1
with uv_nic 16/8 — search_best_independent_uv_mode :7778, whose
allintra nfl count needs an is_highest_layer instrument check).

## 2026-07-14 (wave2, M4 chunk): preset 4 COMPLETE — matrix 102 -> 108/132, presets 4-10 all 84/84

Session scope: the 6 gradient M4 cells. No fresh C instrumentation was
needed (the /root/svtav1-instr scratch was never built): every config
delta was re-verified against the C SOURCE, the M5DBG CFG enc_mode=4
capture row supplied the runtime ground truth, and — for the one cell
the config alone did not close — the C-p4-vs-C-p5 trace comparison plus
source elimination pinned the owning subsystem. C tree untouched
(read-only greps only). Three commits: `a4ec124a0` (funnel config +
evaluate/commit split + PD0 eval tree), `ee5b08c65` (PD1 depth
refinement + inter-depth decision), `1e3187715` (fmt collateral).

### Verified M4 deltas vs M5 (C cites, all re-read this session)

The capture rows (m0m5_config_dlf.txt lines 14-15) diff in exactly four
groups; everything else is field-identical:

1. **intra_level 2 -> 1** (svt_aom_get_intra_mode_levels_allintra
   enc_mode_config.c:6907 `<= ENC_M4`; set_intra_ctrls case 1 :8469):
   mode_end PAETH unchanged, `angular_pred_level[1] = 1` (:18) — the
   |delta| 1/2 skip (mode_decision.c:3268-3271) only arms at level >= 2,
   so ALL SEVEN deltas -3..+3 inject per directional mode in counter
   order: 61 regular candidates + FILTER_DC (fi level 2 unchanged).
2. **SH enable_intra_edge_filter 1 -> 0** (enc_mode_config.c:4035-4048:
   the bit is `dist_ang >= 1 || angular_pred_level[intra_level] in
   {2,3}`; angular[1]=1 fails) -> directional prediction UNFILTERED
   (disable_edge_filter, enc_intra_prediction.c:526), like M6.
3. **nic_level 6 -> 5** (svt_aom_get_nic_level_allintra :5986
   `<= ENC_M4`; set_nic_controls case 5): same scaling 6 / mds1_base
   1200 / mds3_base 15 / staging MODE_1, but `mds1_cand_th_rank_factor
   0`, `mds2_cand_base_th 20`, `mds2_cand_th_rank_factor 0`,
   `mds2_relative_dev_th 0`. The C walk semantics for the zeros were
   ported exactly: divisor `(rank ? rank*count : 1)`
   (product_coding_loop.c:8095/:8171) and the rel-dev exit DISABLED at
   th 0 (`!mds2_relative_dev_th ||`, :8170); the mds2 +2
   winner-coincide staging only fires when the config rank is nonzero
   (:8158-8166). Class ths (300/25/15) + band counts stay dead (single
   intra class on I-slices).
4. **depth refinement level 9 -> 6** (set_block_based_depth_refinement_
   controls cases 6/9): s1/e1 15 vs 10, parent_max_cost_mult 10 vs 0,
   band modulation OFF vs on (max_cost 400/bands 4/dec [MAX,MAX,10,5]),
   lower_split 20 vs 100, split_rate 10 vs 5 (+20 CLN_PD0), use_ref 0
   vs 1 (I-slice-dead), unavail 2 vs 0. THIS delta owned the last cell
   (below).

### The g128 q20 flip — depth refinement finally binds

After the funnel config extension, 5 of 6 gradient cells (+ uniform
6/6) went IDENTICAL immediately; g128 q20 diverged at tile op 2554 —
a 32x32 partition symbol with the SAME evolved CDF and rng on both
sides: C codes SPLIT, we coded NONE. Aligning C's own p4-vs-p5 traces
localized it to SB0's (0,32) quadrant (C-M5 keeps 32x32 NONE, C-M4
splits into four 16x16 NONEs). Since every PD0 knob is
capture-identical between M4 and M5 (pd0=1, parent_bias, sse, subres),
C-M4's PD0 tree == C-M5's == NONE there — so the CODED tree flip is
owned by the PD1 depth machinery, by elimination: at M0..M5 `dr_mode=1`
(PD0_DEPTH_ADAPTIVE) means PD1 RE-DECIDES depths around the PD0
prediction; the M6+ PRED_PART_ONLY shortcut (coded tree == PD0 tree)
was only ever C-exact at M5 because the admitted extra depths LOSE the
inter-depth compare on every tracked cell — the m0m5 capture's 16x16
WIN rows under (32,0) at q20/q40 are exactly those losing evaluations.
At M4 (e1 15 + intra_level-1 leaf costs) the 16x16 depth WINS once.

### What landed (svtav1-rs only)

- `leaf_funnel.rs`: FunnelCfg::for_preset(4) + mds2_rank_factor field +
  the C NIC ternaries; `decide_leaf` split into `evaluate_leaf`
  (read-only C md_encode_block) + `commit_leaf`
  (md_update_all_neighbour_arrays + MD recon writes, now including the
  MD partition-context bytes — partition_context_lookup[bsize] span
  writes, product_coding_loop.c:179-192).
- `pd0.rs`: `Pd0Eval` — the pick recursion now returns per-node
  tested/cost/partition (the C pc_tree fields the refinement reads,
  incl. untested early-exit-skipped quadrants); tree APIs unchanged.
- `depth_refine.rs` (new): DrCtrls 6/9; `build_refined_scan` =
  perform_pred_depth_refinement / refine_depth / set_start_end_depth +
  both deviation gates + update_pred_th_offset (enc_dec_process.c:
  1545-1997) — s/e in {0,-1}/{0,1} (s2/e2=255 -> MIN_SIGNED), RAW
  thresholds (depths_qp_based_th_scaling=0 for allintra <= M6,
  enc_handle.c), parent-depth admissions bubbling up through PD0-SPLIT
  nodes (tot_shapes=1 + s++); `DepthWalk` = svt_aom_pick_partition /
  test_depth / test_split_partition (product_coding_loop.c:
  11304-11597) — funnel evaluation + partition rate at REAL partition
  contexts (PartRates over the chained partition CDFs), per-quadrant
  early exits (ths 50/1000), parent_cost_bias 995 compare,
  use_accurate_part_ctx=1 (no split-rate doubling), skip_sub_depth
  cond1 (<= 16x16, f32 quadrant-SSE stddev 250 + coeff 15%). Commit
  discipline: eager quadrant commits + parent-win overwrite,
  state-equivalent to C's index<3/deferred-q3 scheme because every
  neighbour/recon write spans exactly the block.
- `pipeline.rs`: still/420 presets 4..5 route through the refined walk
  (c_quant + use_funnel gates >= 4, per-SB CDF chain 4..=6 per
  update_cdf_level 2 :12154); presets 6+ keep the fixed tree
  (PRED_PART_ONLY: provably the same outcome).
- `partition.rs`: `funnel_block_decision` factored out (shared).

### Gates (all green, verbatim tallies)

- identity matrix FULL sweep: **108 / 132 byte-identical**
  (benchmarks/identity_matrix_2026-07-14.tsv + .meta regenerated) —
  presets 4-10: 84/84; presets 0-3: every uniform cell identical, all
  24 gradient cells divergent (23 FH + 1 tile g128q55p3).
- `IM_PRESETS="4 5 6 7 8 9 10" identity_matrix.sh check`: **84/84**
  (all 12 M5 cells re-verified through the NEW refined walk — the
  extra-depth evaluations now run and lose exactly like C's).
- recon_parity: **270 passed, 0 failed** (CDEF fired 150/270 streams,
  2043808 px filtered / 807364 changed; LR wiener 86/270, 144 RUs —
  counts moved vs M5 close-out because speed-4 chroma streams now take
  the C-exact funnel).
- decode conformance (aomdec AND dav1d): mono **630/0**, chroma
  **840/0** (both matrices already include speed 4, so the new walk is
  covered without test changes).
- `cargo test --workspace --no-fail-fast`: **36 suites, 625 passed,
  0 failed** (+8 new pins: m4 cfg/candidate-shape, dr ctrls, m5/m4
  scan shapes vs the WIN-dump admission pattern, pred-part-only
  equivalence, mds2_rank_factor rows).

### Remaining for M0-M3 (the 24 gradient cells)

All four presets run the SAME depth-refinement level 6 as M4 (now
ported) — the new deltas are the decision layer: nsq GEOMETRY 2 with a
real NSQ SEARCH (levels 5/12/16/18 qp-offset), 4x4 partitions
(disallow_4x4=0, nsq_min 0, hv4 1), bypass_encdec=0 (the encode pass
re-runs — MD == final only if intra MD predicts from real recon;
verify when chasing), PD0_LVL_0 at M0/M1 (pd0=0: full PD0 with its own
candidate search), txt_level 2 (satd 20), txs_sq 2 (depth-2 TX splits),
update_cdf_level 1 at M0-M3 (vs 2 — verify the se/coef/mv split),
chroma ind-uv at mds0/mds1 with uv_nic 16/8 at M0/M1, cfl_level 1 at
M0 (cplx 0 -> CFL evaluated per block: the CFL port becomes binding),
fi_max 4 at M0 (all five filter-intra modes). Attack order per the
prior plan: M2/M3 first (nsq_search + bypass_encdec=0 + 4x4 on top of
the now-complete M4 base), then M1/M0.

## 2026-07-14 (wave2, M2+M3 NSQ chunk): presets 2 and 3 COMPLETE — matrix 108 -> 120/132, presets 2-10 all 108/108

Session scope: the 12 gradient M2/M3 cells (M3 first per the plan).
Fresh instrumentation in `/root/svtav1-instr` (SVT_NSQDBG prints:
per-shape BLK/SKIP/SHAPE/ABORT rows with gate ids, TS/TSX inter-depth
compares, TREE dumps, PD0B per-node PD0 evals, NSQCFG runtime rows,
PSQ residual checksums + buffer-pointer traces; **all instrumented
OBUs byte-identical to baseline** before trusting any dump; scratch
deleted after). Captures committed:
`docs/captures/nsq_m2m3/*.nsq` (12 cells). Rust mirrors the dump
format under `SVTAV1_NSQDBG=1` (depth_refine.rs) so MD walks diff
line-by-line against C.

### Verified M2/M3 deltas vs M4 (C cites + capture evidence)

- **nsq_geom level 2** (svt_aom_set_nsq_geom_ctrls case 2,
  enc_mode_config.c:6427): min_nsq 0, allow_HV4 1, **allow_HVA_HVB 0**
  — the d1 shape set is N/H/V/H4/V4 only (Part-enum order, H4/V4
  before the AB shapes; set_blocks_to_test enc_dec_process.c:1403);
  8x8 nodes test N/H/V; 4x4 none.
- **nsq_search levels** (svt_aom_get_nsq_search_level_allintra
  :11936: base M2 14 / M3 16; seq_qp_mod=2 unconditional,
  enc_handle.c:4221 -> +3/+2/+1 at qp<=39/45/48, -1 at qp>59):
  M3 19/18/16 and M2 17/16/14 at qp 20/40/55 — NSQCFG
  capture-verified. set_nsq_search_ctrls rows :6496-6786 + the tail:
  nsq_qp_based_th_scaling=0 for allintra <= M3
  (set_qp_based_th_scaling_ctrls_all_intra, enc_handle.c:4085) so
  thresholds stay RAW except the unconditional max_part0_to_part1_dev
  -= 5 (:6797).
- **The four NSQ gates** (get_skip_processing_nsq_block :10826, in
  order): split-rate cluster (:10181 — nsq_split_cost_th with the
  lte16 offset SUBTRACTED min 1, H_vs_V/non_HV rate ratios with the
  offset ADDED, lower_depth_split_cost_th on split_flag nodes,
  component_multiple_th over RDCOST(rate,0) vs RDCOST(0,dist));
  parent-SQ TXS (:10533, psq lvl 1 at search levels 17-19: hv_to_sq
  1000 / h_to_v 100 over `non_normative_txs` min-eob H/V re-splits);
  recon-dist quadrants (:10317 — parent-mode modulated max_dev, C's
  ratio-assignment quirk in the >max_ratio arm ported verbatim);
  sq/hv-weight (:10454, CONSERVATIVE_OFFSET_0=5 for H4/V4; the HA/HB
  coeff arms are geometry-dead). faster_md_settings_nsq is
  I-slice-dead (:11470 gates on slice_type != I_SLICE).
- **test_depth d1 loop** (:11396): per-shape partition rate at the
  node's real (left, above) contexts, per-child funnel evals with
  neighbour commits between children, the running-best abort
  (part_cost >= rdc.rd_cost AFTER adding each child), and the
  copy_neighbour_arrays [0]<->[1] save/restore — expressed as node
  snapshots (EntropyCtx clone + recon rects) restored at each
  subsequent shape / before the split walk / before the winner
  commit (state-equivalent: every write spans exactly its block).
- **txt_level 2** (case 2): satd_early_exit_th_intra 20 (vs 15),
  groups 6/6 + rate_th 250 unchanged. **txs_level 2**
  (set_txs_controls :7992): intra max depth sq/nsq 2/2 (vs 1/0),
  depth1/2_txt_group_offset 0/0 (vs 3/3). TX geometry for rect + depth
  2 from tx_depth_to_tx_size / tx_blocks_per_depth / intra tx_org
  (common_utils.c:95 / transforms.c:22,48 — instrument-dumped, plain
  raster everywhere, sub_tx chain halves the long dim; pinned by
  txb_geometry_matches_c_tables).
- **update_cdf_level 1 vs 2**: set_cdf_controls (:12047) differs only
  in update_mv, forced 0 on I-slices — NO funnel/chain impact (the
  per-SB CDF chain gate extended 2..=6).
- **bypass_encdec=0** (svt_aom_get_bypass_encdec_allintra :12037,
  <= M3): the encode pass re-runs prediction/TX/quant per block
  (perform_intra_coding_loop, coding_loop.c:722) — verified a NO-OP
  for our still/420 path: same quantize_inv_quantize (is_encode_pass
  only bypasses the rdoq dct_dct_only/skip_uv exemptions, both 0
  here; full_loop.c:1750-1760), same contexts once trees match, and
  MD recon is already conformant. Zero port surface; byte-identity
  confirms.
- **PD0 at M2/M3**: identical LPD0 config to M4/M5 EXCEPT
  disallow_4x4=0 (svt_aom_get_disallow_4x4_allintra :11638 <= M3 ->
  false) -> min_sq 4 (set_blocks_to_be_tested, enc_dec_process.c:
  1494): C's PD0 evaluates 4x4 blocks (PD0B dumps: 2903 4x4 evals
  across the 12 cells) — pd0.rs walks one more level (Tx4x4 in
  tx_quant_core). PD1 refinement e-caps drop the disallow_4x4 arms
  (set_start_end_depth :1811). PD0 trees still bottom out at 32x32
  on every tracked cell; PD1 never admits below 16x16.
- **wn_filter level 3** (get_wn_filter_level_allintra :1928 <= M3):
  use_refinement=1 + max_one_refinement_step — already ported
  (restoration.rs finer_tile_search_wiener); binds on the cells where
  wiener signals.

### The two C quirks the differ caught

1. **psq residual = the LAST MDS3 candidate's depth-0 residual** —
   NOT the winner's. Buffer-pointer instrumentation proved ALL MDS3
   candidates share ONE residual workspace (the per-candidate RESW
   rows show PAETH 25097 -> H 25526 -> DC 30140 into the same
   pointer, and non_normative_txs reads 30140); depth-1/2 trials
   write the per-depth scratch buffers (init_tx_cand_bf copies OUT,
   :5160), so the base buffer keeps the last candidate's DEPTH-0
   residual. LeafEval.psq_resid implements exactly that.
2. **Multi-strength CDEF finally binds** (g128 q40 p3: C picks
   nb_strengths=2). Landed the full finish_cdef_search nb loop +
   per-fb best_gi (enc_cdef.c:1369-1435, zero_fs_cost_bias=0 at
   allintra <= M7 per cdef_recon_level 0 :3602), FH cdef_bits +
   (1<<bits) strength pairs (obu::CdefSignal), the per-SB cdef_idx
   literal at the first non-skip block (write_cdef,
   entropy_coding.c:4034-4065; EntropyCtx.cdef_pending armed per SB),
   per-fb application, and a cdef re-walk when bits > 0 (recon-
   neutral, same pattern as the LR re-walk; the LR re-walk carries
   the cdef state too). Gap 2a is now CLOSED for the still path.

### M2 extras (the 3 cells the M3 port didn't already close)

- **is_chroma_reference pairing** (common_utils.h:315): the V4-at-16
  4x16 children (12 evals, q40 cells only) carry chroma only at odd
  mi_col; chroma-ref children evaluate the PAIR block (bsize_uv
  max(dim,8)/2 at ROUND_UV origins), non-ref children price ZERO
  chroma (fast rate rd_cost.c:619 has_uv gate; no chroma full loop,
  no skip-txb bits, no commit writes).
- **64x16/16x64 three_quad folds** (svt_handle_transform64x16_c,
  transforms.c:3223): cols 32.. over h rows — the H4-at-64 eval read
  past the coefficient buffer before this fix.

### Gates (all green, verbatim tallies)

- identity matrix FULL sweep: **120 / 132 byte-identical**
  (benchmarks/identity_matrix_2026-07-14.tsv + .meta regenerated) —
  presets 2-10: 108/108; presets 0-1: every uniform cell identical,
  all 12 gradient cells divergent.
- `IM_PRESETS="3 4 5 6 7 8 9 10" identity_matrix.sh check`: **96/96**
  (mid-session), `IM_PRESETS="2 3 4 5 6 7 8 9 10"`: **108/108**
  (close-out).
- recon_parity: **324 passed, 0 failed** (speed 3 ADDED to the gate —
  the funnel now owns presets 2/3; CDEF fired 176/324 streams,
  2313760 px filtered / 920602 changed; LR wiener 104/324, 178 RUs).
- decode conformance (aomdec AND dav1d, speed 3 added): mono
  **735/0**, chroma **980/0**.
- `cargo test --workspace --no-fail-fast`: **36 suites, 628 passed,
  0 failed** (+3 pins: txb geometry vs the C dump, M2/M3 funnel cfg,
  NsqCfg rows vs the NSQCFG captures; finish_cdef_rd pins updated to
  the new full-lev return SHAPE, same values).

### Remaining for M0-M1 (the 12 gradient cells)

- **PD0_LVL_0** at M0/M1 (capture pd0=0): the light-PD0 walk with
  intra_level = MAX_INTRA_LEVEL-1 (svt_aom_sig_deriv_enc_dec_light_
  pd0 :9310 — more PD0 intra candidates than LVL_1's set). NSQ stays
  PD0-disabled (md_disallow_nsq_search=1 unconditional, :9257).
- **nic_level 1 at M0 / 3 at M1** (M0: scal 20, mds1_base MAX ->
  no mds1 pruning, mds2/3 base 50; M1 == M2's case 3).
- **chroma ind-uv at mds0/mds1** (M0 ind_last_mds=0 uv_nic 16; M1
  ind_last_mds=1 uv_nic 8 — search_best_independent_uv_mode
  :7778 BEFORE the stages, not the mds3 variant).
- **cfl_level 1 at M0** (cfl_itr 2, cplx 0 -> CFL evaluated for every
  <= 32x32 block: the CFL port becomes binding).
- **fi_max 4 at M0** (all five filter-intra modes as candidates).
- **intra_level 1 with dist_ang... M0/M1 both already mode_end 12 /
  ang 1** (same candidate set as M3/M4).
- nsq_search levels: M0 3 (+offsets -> 6/5/3), M1 10 (13/12/10).
- 4x4-depth PD1 admissions could now bind (PD0 trees may bottom at
  8x8 on M0/M1's richer PD0) — the sub-8x8 walk/writer surface
  (partition ctx at 4x4 granularity, 4x4 tx_depth syntax, sub-8
  chroma in the WALK) is still unproven.

## 2026-07-14 (wave2, M0/M1 chunk): M1 COMPLETE — matrix 120 -> 126/132; M0 analyzed but blocked

Session scope: the last identity tier (12 gradient cells at M0/M1).
**M1 gradient is now byte-identical (6/6); matrix 126/132.** M0 was
verified + prototyped but is NOT shipped: routing it exposed a
conformance blocker (invalid tile data on sub-8-width blocks — see the M0
subsection). Every config delta below was re-verified against the C
source this session (file:line); divergences were localised with the
differ + Rust-side funnel instrumentation (removed after use), no
scratch-C build.

### M1 (chroma_level 2) — the binding delta was the INDEPENDENT uv search

Routed preset 1 through the PD0 fixed-tree + depth-refine + leaf-funnel
path (pipeline.rs gates extended to preset 1). The M1-vs-M2 deltas:

- **pd0_lvl 0 vs 1** (set_pic_pd0_lvl_allintra, enc_mode_config.c:12602
  `<= ENC_M1` -> 0). PD0_LVL_0 uses intra_level MAX_INTRA_LEVEL-1
  (svt_aom_sig_deriv_enc_dec_light_pd0:9308). NON-BINDING on the tracked
  cells: the existing LVL_1 pick reproduced C's LVL_0 partition tree on
  all 6 M1 cells (every partition symbol matched in the differ).
- **nsq_search 10 vs 14** (:11941). NsqCfg::for_preset_qp already carried
  M1=10 (-> 13/12/10 by qp). Trees matched C -> non-binding on these cells.
- **chroma_level 2 vs 4** (svt_aom_get_chroma_level_allintra:12233). THIS
  was binding, even on flat chroma. chroma_level 2 (ind_uv_last_mds=1)
  runs `search_best_independent_uv_mode` (product_coding_loop.c:7778), NOT
  `search_best_mds3_uv_mode`. The independent search injects ALL uv modes,
  fast-loop-prunes by residual variance to the uv_nic-scaled nfl, and
  forces UV_DC; UV_PAETH (injected last) is pruned, so a luma-PAETH block
  resolves to UV_DC — where the mds3 variant picks UV_PAETH (uv-matches-
  luma is cheap in the luma-conditioned CDF). Differ-verified: C M1 codes
  UV_DC at g128 q55 where C M2 codes UV_PAETH.
  - **nfl base is 32, not 16**: a still KF has is_highest_layer=FALSE
    under OPT_USE_HL0_FLAT (pd_process.c:6212 `(tli==hl) && hl!=0`, hl=0),
    so `uv_mode_nfl_count = allintra ? 32` (:7919). At uv_nic 8 -> nfl 16
    (includes DC,V,H+deltas,D45(-3)); the first cut (nfl 8) mispicked
    uv=DC for luma=H because H's delta-0 was pruned — the q20/q40 cells
    caught it.

Ported `search_best_independent_uv_mode` into the funnel behind
`FunnelCfg::ind_uv_independent(uv_nic)` (leaf_funnel.rs): inject all uv
modes with the C angle-delta rules, fast-loop residual variance
(`svt_aom_mefn_ptr[bsize].vf` = sse - sum^2/N), stable prune to the nfl
(sort_fast_cost_based_candidates is a strict-less selection sort = stable),
force UV_DC, full-loop coeff+dist, then per-luma best-uv by RD. M1's other
funnel config equals M2's (nic_level 3, txt_level 2, txs_level 2,
filter_intra level 2). **All 6 M1 gradient cells IDENTICAL.**

### M0 — ANALYZED but NOT shipped (blocked on the sub-8 partition writer)

M0's config was verified and a funnel-routing prototype built, but it is
**NOT committed** because it produces INVALID bitstreams (a conformance
regression), not merely a parity divergence. It is held in a git stash
(`M0-routing-WIP`) for the next chunk; the matrix stays at 126/132.

Verified M0-vs-M1 config deltas (for the next chunk, all re-checked vs C):

- **nic_level 1** (svt_aom_get_nic_level_allintra:5988 `<= ENC_M0` under
  OPT_NSC_STILL_IMAGE -> 1; set_nic_controls case 1): nic_num (20,20,20)
  (MD_STAGE_NICS_SCAL_NUM[0]), mds1_cand_base_th MAX (no mds1 pruning),
  mds2/mds3 base 50.
- **chroma_level 1** (svt_aom_get_chroma_level_allintra:12231 -> 1):
  independent uv search, uv_nic 16 (nfl 32), ind_uv_last_mds=0 (C runs it
  BEFORE MDS0).
- **filter_intra level 1** (get_filter_intra_level_allintra:12681 -> 1):
  all five filter-intra modes are candidates (fi_max 4), vs M1's fi_max 0.
- **nsq_search 3** (-> 6/5/3/2 by qp; NsqCfg level-2 row = (105,0,150,3,
  0,0,10,0,0,115) from set_nsq_search_ctrls case 2 needed — M0 hits level
  2 at qp>59, which the 3..=19 table panicked on).

Prototype result on the identity matrix: **6/6 uniform + 4/6 gradient
(q40,q55) byte-identical**. BUT recon_parity (speed 0 added) then showed
**4 DECODE FAILURES** — `c420_gradient_{64,128}_q20_s0`,
`c420_gradient_96pad_{q20,q43}_s0` — aomdec: "Failed to decode tile data".

Root cause: M0's aggressive nsq (level 6 at q20 / 5 at q43) picks
**VERT4/HORZ4 -> sub-8-width (4x16) blocks**, which hit the **unproven
sub-8 partition/writer surface** (CLAUDE.md gap: partition ctx at 4x4
granularity, 4x4 tx_depth syntax, sub-8 chroma pairing in the WALK). The
tile entropy for those blocks is malformed -> non-conformant stream. M1's
higher nsq levels (10-13) prune VERT4 away, so M1 never hits it (M1 is
fully valid + identical).

Two things must land before M0 can be routed:
1. **The sub-8 partition/writer** (the conformance blocker) — port the
   C 4x4-granularity partition context, sub-8 tx_depth syntax, and the
   is_chroma_reference sub-8 chroma pairing in the entropy walk so 4x16 /
   16x4 leaves emit valid tile data.
2. **The filter-intra chroma-mode** (a parity item exposed while chasing
   q20): C `update_intra_chroma_mode` (mode_decision.c:3332) keys the
   best-uv table on `fimode_to_intramode[fi]` (definitions.h:1342 =
   {DC,V,H,D157,PAETH}) — FILTER_H's uv is best_uv[H], not best_uv[DC].
   Applying that refinement alone did NOT close q20 and regressed q40, so
   a deeper independent-uv `best_uv_mode` / ind_uv_last_mds=0-timing
   divergence on sub-8 blocks is also present — needs C-side best_uv_mode
   instrumentation for a 4x16 block at M0 q20.

### Gates (all green on the committed tree — M1 chunk, matrix 126/132)

- identity matrix: **126/132** (benchmarks/identity_matrix_allpresets
  .tsv + .meta): presets 1-10 = 12/12 each; M0 = 6/12 (6 uniform
  identical; 6 gradient still homegrown — the last identity tier).
- recon_parity: **378/0** (speed 1 ADDED — the funnel now owns speed 1).
- decode_conformance: mono **840/0** + chroma **1120/0** (aomdec + dav1d,
  speed 1 ADDED).
- cargo test --workspace: **628/0**.

## 2026-07-14 (wave2, M0 sub-8 chunk): filter-intra tx_type CDF fix — M0 ROUTED, conformance unblocked, matrix 126 -> 130/132

Session scope: land the stashed `M0-routing-WIP` (route preset 0 through the
PD0 + depth-refine + leaf-funnel path) and fix the conformance blocker it
exposed. **The blocker was NOT a missing "sub-8 partition writer"** (the
prior chunk's hypothesis) — the 4x16 / 8x32 partition + block + coeff writer
was already correct. The real bug is a one-line **luma tx_type CDF index**
error that only bites filter-intra blocks, which M0 is the first preset to
emit (filter_intra level 1 injects all five fi modes; M1..M6 use fi_max 0).

### Root cause (differ + aomdec-debug gdb, no scratch-C build needed)

The 4 recon_parity decode failures (`c420_gradient_{64,128}_q20_s0`,
`c420_gradient_96pad_{q20,q43}_s0`) AND the M0 gradient q20 identity cells
produced a **self-inconsistent** tile: the encoder wrote bits the reference
decoder could not read back. Localised it precisely:

1. **aom-inspect + gdb on `/root/aomdec-debug/aomdec`** (libaom source at
   `/root/aom-rs/reference/libaom`): the decoder read the first 16x16's four
   4x16 leaves fine, then read the *second* 16x16's partition as VERT where
   the encoder wrote VERT_4 — a coder-state desync inside the first 16x16.
2. **Per-txb gdb dump vs encoder WTXB log**: every txb (tx sizes, skip/dc
   contexts, eobs, and coeff values under the transposed level-buffer layout
   `enc(r,c) == dec c*16+r`) matched for x=32, x=36, x=40; **x=44 desynced**.
3. **Full rng-sequence diff (encoder symtrace vs decoder `dec->rng`)**:
   first divergence at the CDF op ENTERING x=44's coeffs — the **luma
   tx_type symbol** (nsyms=7). Encoder CDF `[31456,25911,24693]`, decoder
   CDF `[32096,29521,...]`: SAME rng in, DIFFERENT adapted CDF — i.e. the
   two sides indexed a *different* `intra_ext_tx_cdf` instance.

**Why:** C `av1_read_tx_type` (decodemv.c:637) indexes the luma tx_type CDF
by `filter_intra ? fimode_to_intradir[fi_mode] : mbmi->mode`. For a
filter-intra block the coded luma mode is DC, but the tx_type CDF must use
`fimode_to_intradir[fi]` (`{DC,V,H,D157,DC}`). x=32 (mode V) had adapted the
`[V]` instance; x=44 (filter-intra FILTER_H) must reuse `[V]`, but the Rust
writer passed the plain DC mode -> used the `[DC]` instance (adapted by x=40)
-> CDF mismatch -> desync. The FUNNEL already used `FIMODE_TO_INTRADIR` for
the candidate's tx_type *rate* (leaf_funnel.rs:2223); only the **bitstream
writer** (`encode_block_syntax`, both the single- and multi-tx luma paths)
was missing it.

### Fix (2 hunks)

- `leaf_funnel.rs`: `FIMODE_TO_INTRADIR` -> `pub(crate)`.
- `pipeline.rs` `encode_block_syntax`: compute
  `tx_intra_dir = filter_intra ? FIMODE_TO_INTRADIR[fi] : intra_mode` once
  and pass it (not `decision.intra_mode`) as the tx_type `intra_dir` to
  `write_coeffs_txb_1d` on both luma tx paths. Chroma tx_type is derived
  (never signalled), so it is unaffected.

Plus the `M0-routing-WIP` config (all re-verified vs C last chunk): preset 0
funnel cfg (nic_level 1 -> nic (20,20,20) + mds1_base MAX no-prune; chroma
level 1 independent-uv nfl 32; filter_intra level 1 fi_max 4; nsq_search 3),
NsqCfg level-2 row, and the pipeline gates extended to `preset >= 0`.

### Result

Every M0 sub-8 stream now decodes. **matrix 126 -> 130/132.** M0 is 10/12:
all 6 uniform + gradient q40/q55 (4) byte-IDENTICAL; the 2 residual cells are
`gradient {64,128} q20 p0`.

### Residual: 2 cells (gradient q20 p0) — a DECISION-layer tie-break, NOT a writer bug

Both residual cells now **decode + recon byte-exact** (self-consistent valid
streams); they are not byte-identical to C. First divergence (differ) is a
genuine mode pick on a 4x16 block: **C codes H_PRED, Rust codes FILTER_H**
(the filter-intra H). The two predictions are near-identical, so it is a
close RD tie-break the funnel resolves differently only at q20 (q40/q55
agree). This matches the prior chunk's note (M0/M1 chunk residual item 2):
"a deeper independent-uv `best_uv_mode` / `ind_uv_last_mds=0`-timing
divergence on sub-8 blocks" — the sub-8 chroma-ref total-RD (luma+chroma)
tips the luma winner. Closing it needs C-side per-candidate RD instrumentation
of the x=36/x=44 4x16 blocks at M0 q20 (which fi/H candidate C ranks where).

### Gates (all green on the committed tree, matrix 130/132)

- identity matrix (presets 0-10): **130/132**
  (benchmarks/identity_matrix_allpresets.tsv regenerated) — presets 1-10 =
  12/12 each (no regression); M0 = 10/12 (2 gradient q20 residual).
- recon_parity: **432/0** (speed 0 in the gate; the 4 prior DECODE FAILURES
  are gone).
- decode_conformance: mono **945/0** + chroma **1260/0** (aomdec AND dav1d,
  speed 0 owned by the funnel).
- cargo test --workspace: **662/0**.
- Build profile: opt-level 2 for release+test (deps stay opt-3); identity
  matrix re-verified byte-identical at opt-2 (opt-level does not affect
  bit-exactness — no FMA contraction without target-feature=+fma), ~faster
  rebuilds for iteration.

## 2026-07-14 (wave2, real-image M10 chunk): PD0 partition RD — the input-resolution coeff-rate term (first real-content C-exactness fix)

First diagnosis + fix driven by the CID22 real-image harness
(`tools/real_image_matrix.sh`, 0/66 byte-identical vs 130/132 synthetic).
Target: **preset 10 (eff-M9)**, the cleanest real cell — its FH already
matches C (closed-form LF + qp CDEF ported), so the divergence is pure
partition/mode/tx RD. Cell: `1001682.png 512x512 q40 p10`.

### Localization (differ + tree dump, no code change)

- Differ first divergence: **tile op 9460**, a 64x64 partition symbol
  (CDF nsyms=10, `icdf=[8303,7518,6666]`, `rng=47188` — identical CDF+state
  on both sides): **C codes s=0 (PARTITION_NONE), Rust codes s=3
  (PARTITION_SPLIT)**. Pure decision divergence; 68 partition symbols
  matched before it.
- `SVTAV1_DUMP_TREE=1` + partition-symbol counting maps op 9460 to the
  **64x64 superblock at (x=256, y=192)** = sb_index 28, mi(48,64). At
  preset >= 9 the whole tree is `pd0::pd0_pick_sb_partition`
  (`encode_fixed_tree`), so this is a PD0 partition-tree divergence.

### Root cause (scratch-C instrumented, byte-identical-verified, then deleted)

Instrumented `/root/svtav1-instr` (2 TUs recompiled with the exact CODEC
flags, `ar r` into a *copy* of `libSvtAv1Enc.a`; env-gated prints; OBUs
byte-identical to baseline with the env unset — proven before trusting).
Captured C's per-block PD0 costs for SB 28 vs Rust's (`SVTAV1_PD0DBG`):

- Variance + detector **byte-identical**: `var64=314 var32=1140 var16=4096
  th=4762 -> demote to PD0_LVL_5` on both sides. Not the cause.
- Every LVL_5 block cost was **uniformly lower in Rust by exactly 775,647**
  (64x64, all 32x32, all 16x16, all 8x8 — same delta everywhere). An
  exactly-constant offset proves the transform/quant/distortion/eob are
  already C-exact; only a fixed per-block **rate** term differs.
  `775,647 = (1600*lambda + 256) >> 9` at lambda 248207 -> the missing
  rate is **1600 per block**.
- 64x64 decision (C | Rust before fix):
  parent `767,309,306 | 766,533,659`; split `773,289,166 | 765,532,696`.
  The per-leaf 1600 accumulates: the 10-leaf split loses `10 * 775,647 =
  7,756,470` while the NONE parent loses only `1x` -> C picks NONE (parent
  < split by 0.78%), Rust flips to SPLIT.

**The bug:** `perform_tx_pd0` at `coeff_rate_est_lvl == 0`
(product_coding_loop.c:4579) sets
`y_coeff_bits = 5000 + input_resolution_factor[input_resolution]*1600 +
100*eob`, with `input_resolution_factor[7] = {0,1,2,3,4,4,4}`. Rust's
`pd0::lvl5_block_cost` hardcoded `5000 + 100*eob` (factor 0). For 512x512
the picture is `INPUT_SIZE_360p_RANGE` (262144 px; `240p_TH(0x28500) <=
262144 < 360p_TH(0x4CE00)`, sequence_control_set.c:120) -> factor **1** ->
+1600/block. The 64/128 synthetic cells are both < 240p_TH -> factor 0,
which is exactly why synthetic identity never exposed this.

### The fix (`pd0.rs`, `pipeline.rs`)

- New `pd0::input_resolution_factor(pixels)` — verbatim
  `svt_aom_derive_input_resolution` thresholds mapped through the factor
  table.
- Threaded `ires_factor` into `Pd0Ctx`; `lvl5_block_cost` now uses
  `5000 + self.ires_factor*1600 + 100*eob`. Pipeline computes it from the
  frame `w*h` (LVL_1/LVL_6 paths pass 0 — the term is LVL_5-only).
- Post-fix SB 28 costs reproduce C **exactly** (parent 767,309,306, split
  773,289,166 -> NONE), verified with the same instrumentation.

### Differ delta + gates

- `1001682 q40 p10`: first divergence **op 9460 -> op 20836** (2.2x more
  matching ops); Rust tile 10,904 -> 10,769 B (C 10,874), op count
  100,603 -> 98,838 (C 99,104). The partition tree now tracks C through
  the fixed-tree walk.
- Multi-cell (p10): `1424246 q40` first divergence at **op 49440** (Rust
  6967 B vs C 6966 — off by one byte); `1147124 q40` at **op 553** — its 6
  partition symbols all match C, the divergence is the CDF3 leaf below.
  Across images the partition layer now agrees with C; what's left is a
  single new class (below).
- Synthetic identity matrix (presets 0-10): **130/132** — unchanged (2 =
  pre-existing M0 gradient q20 residual). Factor 0 for all 64/128 cells.
- `cargo test --workspace`: **662/0** (pd0 cost tests
  `lvl5_block_costs_match_c_q40`, `gradient64_trees_match_c` etc. green —
  they pass ires_factor 0 for 64x64 frames).
- recon_parity: **432/0** (AOMDEC=/root/aomdec-build/aomdec; CDEF fired on
  236/432, wiener on 137/432 — the fix is on the still allintra path only).

### Next divergence (op 20836 — NOT this chunk)

After the fix the remaining first divergence is consistently a **CDF
nsyms=3, C s=1 vs Rust s=0** (same CDF+rng) on a PARTITION_NONE leaf,
right after partition/skip/y_mode(DC)/uv_mode(DC) — seen at op 20836 for
`1001682` and op 553 for `1147124`. That CDF3 is the **tx_depth / tx-size
symbol** (`tx_size_cdf`): C picks depth 1 (split the TX once), Rust picks
depth 0 (largest TX). So the next real-content gap is the **per-leaf
TX-size RD at eff-M9** (the funnel's TXS decision in
`leaf_funnel.rs`/`quant.rs`): C evaluates splitting the transform and its
RD favors depth 1 on these blocks where ours keeps depth 0. Needs the
same per-candidate C capture (tx_size RD for the diverging leaf) to pin
whether it's a missing TXS candidate, a cost term, or a threshold. Left
documented, not chased (this chunk = smallest fully-verified fix).

## 2026-07-14 (wave2, real-image M10 chunk 2): eff-M9 per-SB TXS bump + lvl-0 coeff-rate — REAL M10 now 60/60 byte-identical

Closed the tx_depth/TXS divergence the prior chunk documented (op 20836).
**Every tracked real CID22 preset-10 cell is now byte-identical: RIM
`RIM_PRESETS=10` = 60/60 (20 images x q{20,40,55}), up from 0/66.**
`1001682 q40 p10` was VERDICT IDENTICAL directly; `1147124` (was op 553)
and `1424246` (was op 49440) also IDENTICAL. Diagnosed with the differ +
scratch-C instrumentation of `perform_tx_partitioning` (OBUs verified
byte-identical to baseline with the env unset, scratch then deleted).

### Root cause: eff-M9 turns TXS ON per-SB (VLPD0 coupling), the funnel had it OFF

At allintra eff-M9 `pcs->txs_level = 0` at the picture level
(sig_deriv_mode_decision_config_allintra, enc_mode_config.c:15112), but
with `FTR_COUPLE_VLPD0_TXS_PER_SB` (=1) two things fire:
- `frm_hdr->tx_mode = TX_MODE_SELECT` unconditionally (:15143).
- `svt_aom_sig_deriv_enc_dec_allintra` bumps the per-SB txs_level from 0 to
  `MAX_TXS_LEVEL-1 = 5` **iff `ctx->pd0_ctrls.pd0_level == PD0_LVL_6`**
  (:11366, CLN_RENAME_PD0). eff-M9's `pic_pd0_lvl = 7` -> base PD0_LVL_6
  (set_pd0_ctrls case 7, :7127), which `pd0_detector_allintra`
  (enc_dec_process.c:2373) **demotes to PD0_LVL_5** per-SB when the
  variance profile lacks a dominant depth (flat SBs). So: **undemoted
  (PD0_LVL_6) SBs -> TXS on (level 5); demoted (PD0_LVL_5) SBs -> TXS
  off.** The demoted `pd0_level` set in PD0 persists into the PD1 sig_deriv,
  so the bump reads the per-SB value. (This is why synthetic identity never
  exposed it: flat/uniform content demotes every SB -> TXS off -> depth 0.)

set_txs_controls case 5 (:8024): `intra_class_max_depth_sq/nsq = 1`
(evaluate depth 0 + 1), `prev_depth_coeff_exit_th = 100`,
`quadrant_th_sf = 100`.

Instrumentation confirmed on `1001682 q40 p10`: 484 PD1 intra leaves ->
**352 at PD0_LVL_6 (TXS on, end_depth 1), 132 at PD0_LVL_5 (TXS off)**; of
the 352, **21 pick tx_depth=1** (the C-vs-Rust divergences), 331 keep 0.

### Second cause: eff-M9 luma coeff-RATE uses the fast approximation (coeff_rate_est_lvl 0)

Enabling TXS surfaced a coeff-rate mismatch (~5x). eff-M9 has
`rate_est_level = 0` (:15043) -> `coeff_rate_est_lvl = 0`
(set_rate_est_ctrls, :8349). In `tx_type_search` (product_coding_loop.c
:4976) the luma coeff bits are the fast per-txb estimate
`th=(txw*txh)>>6; eob<th ? 6000+eob*1000 : 3000+eob*100`, NOT the real
`cost_coeffs_txb` the funnel computed (built for M6's lvl 1). Captured C
for the diverging leaf x=176 y=256 16x16 DC: depth0 cbits **19300** = 3000
+ 163*100 (eob 163), depth1 cbits **20700** = 4*3000 + 87*100 (eob 87,
4x8x8) — reproduced exactly. depth0 cost 54,689,200 vs depth1 50,581,790 ->
C (and now Rust) picks depth 1. The quadrant early-exit also fires on
other LVL_6 blocks (e.g. x=128 y=256 32x32 -> depth 1 aborted -> keeps
depth 0), reproduced exactly.

### The fix (funnel only; C read-only)

`leaf_funnel.rs` — 4 new `FunnelCfg` fields, eff-M9 config gains them:
- `txs_on = true, txs_max_sq = 1, txs_max_nsq = 1` (was off).
- `txs_prev_depth_exit = 100` (was hardcoded 1 in the depth loop; now
  config-driven — depth 1 only tried when depth-0 total eob >= 100, so
  flat blocks stay depth 0 and synthetic identity is unchanged).
- `txs_quadrant_sf = 100` — new per-txb quadrant early-abort of a deeper
  depth (product_coding_loop.c:5437), ported into the depth loop.
- `txs_lvl6_gate = true` — the per-SB gate. `evaluate_leaf` now takes
  `sb_is_lvl6`; `end_depth` is 0 unless `!lvl6_gate || sb_is_lvl6`.
- `coeff_rate_est_lvl = 0` — the depth loop prices the luma coeff rate with
  the lvl-0 approximation above (only when `end_depth > 0`, i.e. C's
  perform_tx_partitioning path); the real `tx_unit` bits still drive
  RDOQ/eob. M6/M7/M8 keep `coeff_rate_est_lvl = 1` (real) — unchanged.

`partition.rs` `encode_fixed_tree`: computes
`sb_is_lvl6 = !pd0_detector_allintra_demotes(sb_vars, cli_qp)` (the exact
decision the PD0 tree build used — same fn, same inputs) and passes it to
`decide_leaf`. `depth_refine.rs`: pass-through (gate off for presets <=8).

### Gates (all green, verbatim)

- `1001682 q40 p10` differ: **op 20836 -> VERDICT IDENTICAL** (10897B,
  99104 tile ops). `1147124 q40 p10` IDENTICAL (14117B); `1424246 q40 p10`
  IDENTICAL (6966B).
- **real-image M10: 60 / 60 byte-identical** (`RIM_PRESETS=10`,
  benchmarks/real_image_identity_txs_m10_2026-07-14.tsv) — was 0/66.
- synthetic identity matrix (presets 0-10): **130/132** — unchanged (2 =
  pre-existing M0 gradient q20 residual; flat content demotes to PD0_LVL_5
  so TXS stays off, factor unchanged).
- recon_parity (AOMDEC): **432 passed, 0 failed** (CDEF 236/432, LR
  wiener 137/432).
- `cargo test --workspace`: **662 passed, 0 failed**.
- C tree pristine (`git -C /root/svtav1 status` clean of non-svtav1-rs);
  scratch `/root/svtav1-instr` deleted.

### Next real-content divergence (NOT this chunk)

M10 real content is closed for the tracked corpus. The next real-content
target is the other presets: M7/M8 real content will hit the same
coeff-rate class (they use `coeff_rate_est_lvl 2` in C — `eob<th ?
6000+eob*1000 : real` — while the funnel uses the real cost; latent, not
yet stressed since only synthetic M7/M8 is tracked), and M2-M6 real
content adds NSQ / depth-refine RD. Run `RIM_PRESETS="2 6"` to surface the
next class.

## 2026-07-15 (wave2, real-image M7 chunk): chroma coeff-rate CB double-count — real M7 36 -> 57/60

Root-caused and fixed the M7 real-content residual (task #70). The prior
chunk (0a7ead360) framed it as a "DCT_ADST chroma coeff-cost divergence"
(C cb=10746 vs ours 6246). Instrumented scratch C (`/root/svtav1-instr`,
`-Wl,--wrap` of `svt_aom_txb_estimate_coeff_bits` + `svt_aom_full_cost` —
no lib rebuild, OBUs byte-identical to baseline, deleted after) DISPROVED
that framing: `svt_av1_cost_coeffs_txb` returns the exact SAME chroma bits
as ours for every candidate. The 10746 is an accumulator artifact.

### The diverging leaf (CID22 2775196 q40 p7, SB(224,192) 32x32)

Chroma 16x16, `th = (16*16) >> 6 = 4`. C commits y=DC; Rust committed
y=H (op 8850 y_mode flip: C s=0 DC, Rust s=2 H). Every RD input matches C:

| candidate | luma y_bits | C cb (full) | C cr (full) | C full_cost | Rust cost |
|---|---|---|---|---|---|
| DC (uv=DC, DCT_DCT, cb_eob=6/cr_eob=9) | 40617 | 7621 | 11736 | 80283353 | 80283353 |
| H  (uv=H, DCT_ADST, cb_eob=3/cr_eob=4) | 40222 | **10746** | 12848 | 82229386 | 80047879 |

`svt_av1_cost_coeffs_txb` for the H u-plane returns **6246** (== ours), but
`svt_aom_full_cost` sees **cb = 10746 = 4500 + 6246**.

### Root cause: order-dependent CB approximation leak + accumulate-add

C `skip_chroma_rate_est` (full_loop.c:1922, coeff_rate_est_lvl 2) processes
CB first: `cb_eob (3) < th (4)` so it writes `*cb_coeff_bits = 3000+3*500 =
4500` STRAIGHT INTO the accumulator, then processes CR: `cr_eob (4) >= th`,
lvl 2 -> `return false` — WITHOUT clearing the CB write. The caller
`svt_aom_full_loop_uv` (full_loop.c:2636-2661) then runs the full estimate
and ADDS it: `*cb_coeff_bits += cb_txb_coeff_bits` -> 4500 + 6246 = 10746.
So CB is priced as approx + full, but ONLY in the `cb_eob < th &&
cr_eob >= th` case (CB is checked first; a `>= th` CB returns before
leaking, so CR can never double-count; the DC candidate's cb_eob=6 >= th so
its CB is clean). Undercharging our H cb by ~4500 (~2.18M RD at
lambda 248207 = the exact 82229386-80047879 gap) flipped the leaf y_mode.

### Fix (funnel only; C read-only)

`leaf_funnel.rs` (the `!real_coeff_ctx` chroma coeff-rate branch, ~2532):
replaced the "all-or-nothing use_approx" logic with a byte-exact replica of
`skip_chroma_rate_est`'s order-dependent per-plane control flow + the
caller's accumulate-add. The CB leak is retained and added to the full
estimate exactly in the `cb_eob < th && cr_eob >= th` case; every other
case is unchanged (both < th -> per-plane approx; a `>= th` CB -> both
full; lvl 0 -> per-plane approx). Commit 5e0937ef4.

### Gates (all green, verbatim)

- real-image matrix `RIM_PRESETS="7 8 10"` (CID22-512 training, 20 imgs x 3
  qps = 180): **176/180** — **M7 57/60** (was 36/60), M8 59/60, M10 60/60
  (no regression). CID22 validation probe 2775196 q40 p7: VERDICT IDENTICAL.
- `cargo test --workspace`: **662 passed / 0 failed**.
- synthetic `identity_matrix` (presets 0-10): **130/132** (unchanged — the 2
  pre-existing M0 gradient q20 cells).
- recon_parity (AOMDEC): **432 passed / 0 failed** (CDEF 236/432, wiener 137/432).
- C tree pristine; scratch `/root/svtav1-instr` deleted.

### Residual (next tier, DISTINCT class — not chroma coeff-cost)

- **EPICA Delta D Plot p7 (3 cells: q20/q40/q55)** — a screen-content plot.
  Diverges at tile **op 4** with a syntax-TYPE mismatch: after y_mode +
  uv_mode (both match), C emits two extra bools (f=318, f=307 — palette /
  screen-content-tools per-block signaling) where Rust emits none. This is
  the screen-content/palette path, NOT the chroma coeff-cost class.
  Pre-existing (identical op-4 divergence in the 2026-07-14 tsv). This is
  the only M7 residual left on the tracked corpus.
- **6763758 q20 p8 (1 cell)** — the known 27 KB high-bitrate tile divergence
  (documented in the prior chunk; separate class).

## 2026-07-15 (wave2, real-image M6 chunk): ROOT CAUSE — per-SB CDF averaging (`avg_cdf_symbols`) unimplemented for frames > 2 SBs wide

Diagnosis only (no fix landed — the fix is large and regression-risky; banked
as an honest, fully-verified root cause). Target: **preset 6 real content**
(CID22-512, the next tier below M7/M8/M10). Preset 6 is the first allintra
preset that runs the CDEF *search* (presets <= M6, enc_mode_config.c:3543-3600)
and `update_cdf_level 2` per-SB CDF chaining — neither exercised by the 64/128
synthetic identity cells (1-2 SBs wide), so both are latent there and binding
on real 512x512 (8x8 SBs).

### The M6 real divergence classes (differ scan, 4 imgs x 3 qps)

`RIM_PRESETS=6` cells diverge in three stream-order classes:
`FH | cdef_*` (CDEF search over live blocks), `FH | lr_type` (Wiener search),
`tile-op` (leaf coeff/RDOQ). **All three are DOWNSTREAM of one root cause: the
post-deblock recon diverges from C**, so both filter searches (which read the
recon) and the tile coding diverge. This corrects the briefed framing — the
CDEF/LR *search code already exists* (cdef.rs `cdef_search_still`,
restoration.rs) and is not the primary M6 gap; the recon feeding it is.

### Localization (scratch-C instrumented; OBUs byte-identical baseline; deleted)

Instrumented `/root/svtav1-instr` (dlf_process.c recon dump at the pre-cdef-prep
point; entropy_coding.c `write_modes_b` leaf dump; full_loop.c
`svt_aom_quantize_inv_quantize` + `svt_av1_optimize_b` `update_coeff_general`
per-coeff dump — every build verified to reproduce baseline OBUs byte-for-byte
with the env unset, scratch then rm'd, C tree pristine). Cell
`1001682.png 512x512 q40 p6`:

- **Post-deblock recon (C vs Rust) differs.** First difference SB row 0 clean
  (SB(0,7)'s 54 px are all rows 59-63 = bottom-edge deblock smear from SB(1,7)),
  first REAL divergence = **SB(1,5)** (x320 y64), then cascades (SB rows 2+ all
  diverge). Clean top-left + whole-block interior divergence = a leaf-decision
  cascade via intra prediction, not deblock (which smears edges).
- **Leaf tree at SB(1,5) MATCHES C**: 64x64 V_PRED, tx_depth 1, per-txb tx_type
  all DCT_DCT, per-txb eob (1,1,3,4) — ALL identical to C. So partition / mode /
  TXS / TXT / eob decisions are all C-exact.
- **Divergence = one quantized coefficient.** txb3 (352,96), raster position 64
  (row 2 col 0, the eob coeff): **C level 3, Rust level 2**. Every other coeff in
  the block matches.

### Root cause (fully pinned, verified both sides)

Drilled the quantizer for txb3 pos 64 (both `SVTAV1_PQ_DUMP` / `SVTAV1_RDOQ_DUMP`
captures):

| quantity | C | Rust | match |
|----------|-----|------|-------|
| pre-quant transform coeff | 413 | 413 | YES (transform+prediction identical) |
| quantize_fp level | 3 | 3 | YES (formula `(413+76)*214>>15`=3 both) |
| rdmult | 1054880 | 1054880 | YES (lambda 248207, prm 17, rweight 100, rshift 2) |
| dist(lvl3) / dist_low(lvl2) | 7744 / 46656 | 7744 / 46656 | YES |
| **rate(lvl3)** | **5458** | **5797** | **NO (+339)** |
| **rate_low(lvl2)** | **3706** | **3044** | **NO (-662)** |
| optimize_b decision | KEEP 3 | LOWER to 2 | divergent |

The RD compare (`update_coeff_general`, full_loop.c:851 / quant.rs:696) is
`rd_low < rd`. dist matches; only the **rate** differs, and it traces to the
`base_eob_cost` rate table row (same coeff_ctx=1 on both sides):
`base_eob_cost[1] = [_, 3194, 4694]` (C) vs `[_, 2532, 5033]` (Rust). That table
is built by `syntax_rate_from_cdf(&fc.coeff_base_eob_cdf[...])` — so **the CDF
STATE feeding the per-SB RDOQ rate tables differs at SB(1,5).**

**The mechanism:** C configures each SB's rate-estimation CDF
(`pcs->ec_ctx_array[sb_index]`, enc_dec_process.c:2991-3022) from its neighbors
because `scs->pic_based_rate_est == false` (only ever set false,
enc_handle.c:4617) so the `avg_cdf` branch (`:2999`) is taken:
- neither left nor top-right available -> `md_frame_context` (default);
- left only -> copy left (`sb_index-1`);
- top-right only -> copy top-right (`sb_index - pic_width_in_sb + 1`);
- **both -> copy left, then `avg_cdf_symbols(left, top_right, 3, 1)`**
  (enc_dec_process.c:2668-2710: each CDF entry
  `= (left*3 + tr*1 + 2) / 4` over EVERY FRAME_CONTEXT cdf array + the coeff
  CDFs). Weights `AVG_CDF_WEIGHT_LEFT=3`, `AVG_CDF_WEIGHT_TOP=1`.

Rust's per-SB CDF chain (`pipeline.rs:2407-2425`, `funnel_chain`) only implements
the **left-only copy** — the doc comment there (`:2415-2417`) already flags
"Frames wider than 2 SBs would need avg_cdf_symbols ... unimplemented: such SBs
fall back to the left-only copy (no identity-matrix frame is that wide)". Every
512x512 real cell is 8 SBs wide, so from SB row 1 on, every SB with a top-right
neighbor gets the wrong (un-averaged) CDF. SB(1,5) = sb_index 13 has left
(idx 12) AND top-right (idx 6 = 13-8+1); C averages, Rust doesn't -> the
`base_eob_cost` divergence -> RDOQ over-reduces -> recon cascade -> CDEF/LR/tile.
This is the "avg_cdf_symbols for frames > 2 SBs wide" residual noted at the top
of this doc; it is the M6 (and, by construction, M2-M5 and M0/M1) real-content
blocker.

### The fix (next chunk — well-defined, NOT landed)

Port `avg_cdf_symbols` into the Rust per-SB CDF chain:
1. Store each decided SB's evolved rate-CDF (FrameContext + CoeffFc) in a 2D
   `ec_ctx_array`-equivalent indexed by sb_index (currently only a running left
   snapshot is kept), so top-right (`sb_index - sb_cols + 1`) is reachable.
2. Implement the neighbor-selection rule (enc_dec_process.c:3002-3022) using
   the tile-relative availability tests.
3. Implement `avg_cdf_symbols` (verbatim `(left*3 + tr*1 + 2) / 4` per entry,
   over the SAME CDF-array list C averages — every FRAME_CONTEXT cdf +
   the coeff CDFs) and apply it in the both-neighbors case.
4. Rebuild the MD rate tables (`build_md_rates`) from the configured/averaged
   context per SB.
Verify: SB(1,5) `base_eob_cost[1]` == C `[_, 3194, 4694]`, then
`1001682 q40 p6` moves past the recon cascade. GATE RISK: this touches the
rate-estimation CDF that drives every MD decision — must keep synthetic
identity 130/132 and recon_parity 432/0 (the change is MD-rate-only; it does
NOT touch the entropy coder's running CDF, so decode conformance is
structurally unaffected, but the synthetic gate must still be re-run).

### Gates (this chunk — diagnosis only, no functional change)

- `cargo test --workspace`: **662 passed / 0 failed**.
- recon_parity (AOMDEC): **432 passed / 0 failed** (CDEF 236/432, wiener 137/432).
- synthetic `identity_matrix` (presets 0-10): **130/132** (unchanged).
- real M7/M8/M10 spot (4 imgs q40): **12/12 IDENTICAL** (no regression); real
  M6 same imgs **0/4** (honest baseline: 3 FH / 1 tile, all the avg_cdf cascade).
- C tree pristine; scratch `/root/svtav1-instr` deleted.
- Repo change: diagnostic aids only — `identity_run` env-gated recon dump
  (`SVTAV1_RECON_DUMP`) + `tx_depth` added to the `SVTAV1_DUMP_TREE` leaf line.
  No encoder-output change.

## 2026-07-15 (wave2, real-image M6 chunk 2): CDEF UV search VERIFIED C-exact — the real M6 blocker is a RESIDUAL recon cascade (avg_cdf fix incomplete), not the CDEF search

VERIFY-BEFORE-PORTING result on `1001682.png 512x512 q40 p6` (the cell whose
differ first-divergence is `FH | cdef_uv_pri_strength[0] C=0 Rust=15`). The
briefed premise was "our CDEF UV strength search is not yet C-exact on real
content." **That premise is wrong: the CDEF search is already C-exact.** The
`cdef_uv` FH field is only the first *bitstream* symptom of a deeper problem —
the post-deblock recon that feeds the CDEF/LR searches still diverges from C on
real content, because the `avg_cdf_symbols` fix (9563ac471) resolved only SB
rows 0-2, and rows 3+ still cascade.

### CDEF search is C-exact (proven, not asserted)

Scratch-C instrumented `cdef_seg_search` + `finish_cdef_search`
(`/root/svtav1-instr`, surgically recompiled `cdef_process.c.o`/`enc_cdef.c.o`
into a copy of `Bin/Release/libSvtAv1Enc.a` — no full-tree rebuild; env-gated
`CDEFDBG`/`CDEFRECON`; deleted at end). Dumped, for 1001682 q40 p6, the per-fb
per-candidate mse rows (luma + joint UV), the config, and the RD pick; compared
to the Rust `cdef_search_still` (new env-gated `SVTAV1_CDEF_DBG` aid, `cdef.rs`):

- **Config identical**: `fs=[0,60,2,62]`, `first_pass=2`, `second_pass=2`,
  `subsampling=4`, `zero_fs_cost_bias=0`, `uv_from_y=0`, `use_reference_cdef_fs=0`,
  `use_qp_strength=0`, `lambda=211804`. (Confirms `cdef_search_cfg_for_preset`,
  the `search_one_dual`/`joint_strength_search_dual` port, the `default_mse_uv*64`
  sentinel, and the M6=level-7 `set_cdef_search_controls` mapping are all correct.)
- **Every fb whose post-deblock recon matches C produces byte-identical mse
  rows.** fbs 0-15 (SB rows 0-1): C and Rust Y and UV rows are exactly equal
  (e.g. fb0 Y=[2020,1992,1980,1976] UV=[1730,1730,S,S] both sides). The search
  math (filter kernel, `dist_packed`, subsampling, dir reuse, chroma damping-1)
  is C-exact.
- The final `UVsum` differs (**C=[831224,871785,S,S]** picks uv-idx 0;
  **Rust=[1311110,1303964,S,S]** picks uv-idx 1) *only because the recon it
  measures differs from fb row 2 on* — not because of any search-code delta.

### The real blocker: post-deblock recon still diverges (rows 3+ luma, row 2+ chroma)

Dumped C's `cdef_input_recon`/`cdef_input_source` (tightly packed Y|U|V) and
diffed vs the Rust `SVTAV1_RECON_DUMP` post-deblock recon:

- **Source is identical**: C `cdef_input_source` == the shared `rs.yuv`, 0 bytes
  differ. (Rules out any RGB->YUV / chroma-source mismatch.)
- **Post-deblock recon differs by ~half the frame**: 183309/393216 bytes
  (Y 119057/262144=45%, U 30673/65536=47%, V 33579/65536=51%).
- **Luma interior coeff-divergence map (deblock-untouched pixels, `rs.pre==rs.post`
  but `crec!=rs.post`): SB rows 0-2 CLEAN, rows 3-7 all diverge.** First genuine
  luma coeff divergence at **SB(3,0)**. Chroma diverges one row earlier (row 2:
  the small `UV[0]` deltas that flip the razor-thin CDEF UV pick).
- Deblock is not the cause: the LF levels are deterministic per qindex and the
  kernel is `c_parity_lpf`-validated; a 45% post-deblock delta means the
  **pre-deblock (coeff/mode) recon** diverges, and deblock merely smears it into
  the SB-row-2 bottom edge (`y=191`).

### Hypotheses ruled out this chunk (all verified, not argued)

1. **CDEF search code** — byte-identical mse on matching-recon fbs (above).
2. **Source / chroma conversion** — C source == rs.yuv, 0 diffs.
3. **Deblock kernel / levels** — validated + deterministic; divergence is in
   deblock-untouched interior pixels.
4. **Broken chain evolution** — the per-SB evolved chroma coeff CDF
   (`chain_snaps[sb].1.coeff_base_eob_cdf` plane-1 checksum) *does* vary per SB,
   so the sim re-code is evolving chroma, not stuck at default. The chain
   neighbor rule + `avg_cdf_with` + 2D store structurally match C
   (enc_dec_process.c:3002-3022; C `ec_ctx_array[sb]` is evolved in place by the
   coding_loop.c encode pass, matching the Rust `encode_partition_tree` sim).
5. **CFL** — `chroma_detector_fires` (`leaf_funnel.rs:2491`, result currently
   discarded, a known unported gap) arms only at SB (1,1)/(1,3)/(1,4) [row 1,
   which is CLEAN] and (7,3) [already cascaded]; it **never arms at row 2/row 3**
   where the recon first diverges. Not the first-divergence cause.

### M2-M6 real all diverge at FH — same root cause, different first symptom

`RIM_PRESETS="2 3 4 5 6" RIM_QPS=40 RIM_IMAGES=3`: **0/15 identical**, all FH.
M2-M5 first-diverge at `loop_filter_level[*]` (the LF-level search reads the
diverged recon), M6 at `cdef_uv_pri_strength[0]` or `lr_type[*]` (the CDEF/LR
searches read it). Every one is downstream of the residual recon cascade —
consistent with all of presets 0-6 running the `funnel_chain` per-SB CDF path.
(A separate, latent question for M2-M5: whether the LF-level *search* itself is
fully ported — CLAUDE.md notes "we ship LPF_PICK_FROM_Q only" — vs. purely
reflecting the diverged recon; not disentangled this chunk.)

### Next step (well-defined, NOT this chunk)

The residual is a **chroma-specific coeff/mode divergence at the first row-2
chroma-reference leaf** (first symptom), then luma at SB(3,0). Repeat the prior
chunk's leaf-level method there: `SVTAV1_DUMP_TREE` + scratch-C `write_modes_b`
leaf dump to confirm the tree matches, then a per-coeff `update_coeff_general`
RDOQ compare (C vs Rust) at the first divergent chroma txb to find whether it is
(a) a residual `avg_cdf` coverage gap in a chroma-relevant CDF whose
`AVG_CDF_STRIDE` (padded, e.g. `uv_mode_cdf[0]`) is over-averaged by Rust's
flatten-everything `avg_cdf_with` vs C's `j<=nsymbs` per-cdf loop, or (b) a
chroma RDOQ/context bug that only bites on non-flat chroma. Then re-check that
real M6 advances past the recon cascade; M2-M5 share it.

### Gates (this chunk — verification only, no encoder-output change)

- `cargo test --workspace`: **664 passed / 0 failed**.
- recon_parity (AOMDEC): **432 passed / 0 failed** (CDEF 236/432, wiener 137/432).
- synthetic `identity_matrix` (presets 0-10): **130/132** (unchanged).
- real M7/M8/M10 spot (4 imgs q40): **12/12 IDENTICAL** (no regression).
- real M2-M6 (3 imgs q40): **0/15**, all FH (honest baseline; unchanged by this
  chunk — output byte-for-byte the same as 9563ac471, so the committed
  `benchmarks/real_image_identity_2026-07-15.tsv` remains current).
- C tree pristine (`git -C /root/svtav1 status --short` clean of non-svtav1-rs);
  scratch `/root/svtav1-instr` deleted.
- Repo change: one diagnostic aid — `cdef.rs` env-gated `SVTAV1_CDEF_DBG`
  (per-fb mse rows + RD pick). No encoder-output change.

## 2026-07-15 (wave2, real-image M6 chunk 3): the avg_cdf chain is C-EXACT — the M6 blocker is a LEAF coeff (EOB over-retention) at SB(3,0), NOT the CDF chain

VERIFY-BEFORE-PORTING on `1001682.png 512x512 q40 p6`, targeting the prior
chunk's two leads (avg_cdf padding over-average; leaf drill at SB(3,0)). **Both
leads' framing — "the per-SB avg_cdf chain is wrong" — is REFUTED.** The chain is
bit-exact vs C at and beyond the divergent SB; the divergence is a leaf
coefficient decision downstream of a *correct* rate context.

### Lead 1 (avg_cdf padding / counter) — RULED OUT (rigorous + empirical)

`avg_cdf_symbol` (enc_dec_process.c:2668-2679) loops `j <= nsymbs` per CDF, i.e.
it averages exactly `CDF_SIZE(nsymbs)` entries = all probs + the always-0 + the
adaptation counter at `[nsymbs]`, skipping only storage padding beyond that.
Rust's `avg_cdf_entries` (cdf.rs:46) flat-averages the whole stored array. The
"padding over-average" is a proven NO-OP:
- `update_cdf` only ever writes `[0..nsymbs-2]` and the counter `[nsymbs]`; it
  NEVER touches padding beyond `CDF_SIZE(nsymbs)`. Padding therefore stays at its
  init value, identical on both left and top-right neighbors -> `avg(x,x)=x` ->
  Rust's padding == C's untouched padding, and nothing ever reads it.
- The counter at `[nsymbs]` is averaged identically by both.
- The arrays C averages that Rust lacks (cfl_sign/alpha, sgrproj/switchable
  restore, seg, delta_lf, intrabc) are not in the Rust `FrameContext` at all, so
  they cannot be an averaging bug.

### The avg_cdf chain is C-EXACT (instrumented C; OBUs byte-identical baseline; deleted)

Scratch C `/root/svtav1-instr` (enc_dec_process.c: dump `ec_ctx_array[sb]`
post-configure per SB — the exact chain_base seed; every build verified to
reproduce the baseline OBU byte-for-byte with env unset; deleted at end, C tree
pristine). Compared chroma `coeff_base_eob_cdf[0][1][0..3]` (cbeobU) + luma
`[0][0]` (cbeobY) per SB to a matching Rust `SVTAV1_CHAIN_DUMP` (new committed
env-gated aid, pipeline.rs) of chain_base:

- **chain_base at SB(3,0)=sb24 matches C EXACTLY** (both default:
  cbeobY 10271,1570… cbeobU 4891,1184…). Neighbor rule + `avg_cdf_with` + 2D
  store are all correct.
- **chain_base first diverges only at sb37 (chroma) / sb46 (luma)** — both LATER
  than the recon divergence. C's per-SB coeff CDFs stay DEFAULT through row 2
  (txbskpU first non-default at sb25, cbeobU at sb35): with few coded coeffs the
  chain barely evolves, so through SB(3,0) BOTH sides feed the RDOQ the SAME
  default coeff rate tables. The sb37/46 chain divergence is a DOWNSTREAM
  CONSEQUENCE of the leaf divergence (the sim re-code of a divergent leaf evolves
  the CDF differently), not a cause.
- The funnel RDOQ correctly consumes the chained tables (`optimize_b(…,
  rates.coeff …)`, leaf_funnel.rs:1134; `rates.coeff` = `build_coeff_cost_tables_from_fc(chain_base.cfc)`).
  So the RDOQ rate context at SB(3,0) is C-exact (= default on both sides).

### The real blocker: leaf EOB over-retention at SB(3,0), the first textured SB row

Pre-CDEF recon dump (instrumented enc_cdef.c `svt_av1_cdef_frame`; C
`cdef_input_source` == rs.yuv; deleted) vs Rust `SVTAV1_RECON_DUMP`:
- **First genuine (deblock-untouched) recon divergence at SB(3,0)=sb24.**
  Per-SB-row untouched-luma diverging-px: rows 0-2 = 0 (row 2's 43 px at y=191
  are a deblock smear from row 3), row 3 = 19813, rows 4-7 ~20k each. Within
  SB(3,0) the x0-31 leaves are clean; the x32-63 32x32 leaves carry it. Interior
  diff VARIES (mean 0.3, stdev 3.4, ±13) — a coeff difference, NOT a constant
  DC-prediction shift.
- **C leaf decisions (write_modes_b) vs Rust tree at SB(3,0): partition tree,
  y-mode, uv-mode ALL MATCH; luma EOB is inflated ~2-3x in Rust:**

  | leaf (x,y) | size/mode | C eobY | Rust eob |
  |-----------|-----------|--------|----------|
  | 0,192  | 16x16 DC | 36  | 113 |
  | 16,192 | 16x16 H  | 14  | 48  |
  | 0,208  | 8x8 DC   | 4   | 17  |
  | 32,192 | 32x32 DC | 299 | 706 |
  | 0,224  | 32x32 DC | 359 | 802 |
  | 32,224 | 32x32 DC | 466 | 961 |

  Rust retains ~2-3x more (small, high-freq) coefficients than C on EVERY SB(3,0)
  leaf — including the frame-left-edge x0y192 block (minimal neighbor
  dependency), which rules out a localized prediction cascade. It is
  content-dependent: flat rows 0-2 (few coeffs, e.g. SB(2,0) 64x64 txb0 eob 10)
  match C exactly; the first TEXTURED row (row 3) is where the over-retention
  bites.

### Conclusion + next drill (well-defined, NOT this chunk)

The M6 (and, by the shared path, M2-M5 / M0-M1) real-content blocker is **leaf
coefficient over-retention** — Rust's coding quantizer / RDOQ (`quantize_fp` then
`optimize_b`, leaf_funnel.rs:1127-1149 / quant.rs) keeps far more high-frequency
coefficients than C's `svt_aom_quantize_inv_quantize` + `av1_optimize_txb` on
textured blocks, with an IDENTICAL (default) rate context. Entirely independent
of avg_cdf. M7/M8/M10 real = 24/24 because their tracked content/config doesn't
hit this here (and they never chain), but the quantizer is shared — so the
divergence is specific to the higher-eob 32x32 blocks M6 encodes here, or an
eob/size-dependent RDOQ path.

Next: instrument full_loop.c `svt_aom_quantize_inv_quantize` + `av1_optimize_txb`
(`update_coeff_general`) at the x32y192 TX_32X32 leaf and compare, in order:
(1) pre-quant transform coeffs — settles residual/prediction/transform vs quant
(if equal, prediction+transform are C-exact and it is purely the
quantizer/RDOQ); (2) post-`quantize_fp` levels+eob — settles quantizer
rounding/deadzone; (3) post-`optimize_b` levels+eob — settles RDOQ
tail-truncation. The uniform 2-3x eob inflation points at (3) or (2), not (1).

### Gates (this chunk — diagnosis only; one committed env-gated aid, no output change)

- Rust encoder output byte-identical (`1001682 q40 p6` = 10706 B, unchanged)
  with and without the aid.
- `cargo test --workspace`: **664 passed / 0 failed**.
- recon_parity (AOMDEC): **432 passed / 0 failed** (CDEF 236/432, wiener 137/432).
- synthetic `identity_matrix` (presets 0-10): **130/132** (unchanged; 2 tile =
  known M0/M1 gradient).
- real M7/M8/M10 spot (4 imgs q40): **12/12 IDENTICAL** (no regression).
- C tree pristine (`git -C /root/svtav1 status --short` shows only
  `svtav1-rs/.../pipeline.rs`, the `SVTAV1_CHAIN_DUMP` aid); scratch
  `/root/svtav1-instr` deleted.
- Repo change: one diagnostic aid — pipeline.rs env-gated `SVTAV1_CHAIN_DUMP`
  (per-SB chain_base coeff CDF). No encoder-output change.

## 2026-07-15 (wave2, real-image M2-M5 chunk): `ind_uv_avail` is PER-BLOCK, not a preset constant — real M2-M5 56 -> 65/96

**The "per-SB rate-est CfL CDF chain drift" theory recorded by the previous
session is WRONG and is retracted here.** Re-measured from scratch with a 4-way
dump (port chain / port pack / C rate-est / C bitstream) on the repro cell
`258947 q40 p3` (512×512 bd8 4:2:0; 8169/8169 bytes but DIFFERS):

- C is internally **consistent**: `has_uv == is_chroma_reference` exactly
  (889 == 889 blocks) and C's rate-est (`sum_intra_stats`) `uv_mode` equals C's
  bitstream `uv_mode` on **all 889** blocks (0 diffs). `sum_intra_stats` was never
  the problem.
- The port's chain codes the **same 889** cfl events as C, and the chain == the
  pack by construction (both walk `all_trees` through `encode_block_syntax`,
  which just codes `decision.uv_mode` — there is no revert logic there).
- Joining port-chain vs C-rate-est over all 889 blocks yields **exactly ONE**
  real `uv_mode` mismatch in the whole frame: **x328 y432 8×8 — port
  `uv=13 (UV_CFL_PRED)` js=4 idx=16, C `uv=0 (UV_DC_PRED)`**. (230 other
  "mismatches" are noise: `uv_mode` matches and only the don't-care `js`/`idx`
  differ, because C leaves stale cfl garbage on non-CfL blocks while the port
  zeroes them.)

So there is **no chain drift** — the port's *bitstream itself* was wrong at that
one block, `x416 y408` never flips, and op 48810 **is** x328 y432 (the earlier
note misattributed it).

### Root cause (a real structural stub)

The port modeled C's `ctx->ind_uv_avail` as a **static per-preset config flag**;
in C it is **per-block runtime state**. C resets it to 0 for every block
(`product_coding_loop.c:9931`) and sets it to 1 only when the independent-uv
search actually **runs**, gated at `:10165` on

```c
uv_ctrls.uv_mode == CHROMA_MODE_0 && uv_ctrls.ind_uv_last_mds &&
blk_geom->sq_size < 128 && ctx->has_uv &&
perform_ind_uv_search_last_mds(ctx, cand_bf_ptr_array, best_cand_idx_array)
```

`perform_ind_uv_search_last_mds` (`:1470`) counts MDS3 intra candidates as
`!is_inter && (!uv_ctrls.skip_ind_uv_if_only_dc || cand->block_mi.uv_mode !=
UV_DC_PRED)` and returns `count > 0` (its `inter_vs_intra_cost_th` arm is inert
on I-slices — `best_inter_cost` stays `MAX_MODE_COST`). At M2..M5 the chroma
level (`enc_mode_config.c:5781`) is `ind_uv_last_mds=2,
inter_vs_intra_cost_th=100, **skip_ind_uv_if_only_dc=1**` — so when **every**
MDS3 candidate is UV_DC the search is skipped and `ind_uv_avail` stays 0. C then
reaches

```c
if (cfl_performed) { if (ctx->ind_uv_avail) check_best_indepedant_cfl(...); }   // :7258
```

with a FALSE `ind_uv_avail`, so **no revert runs** and CfL is decided by the
uv-follows-luma **TRANSFORM-domain** compare inside `cfl_prediction` — not the
ind-uv **SPATIAL** compare. Measured: **263 of 7323** MDS3 blocks have
`ind_uv_avail == 0` (not size-determined — 16×16 appears with both values). At
x328 y432 C's gate reads `cfl_cplx=1 cplx_th=10 is_inter=0 mds=3 enabled=1
maxdim=8 arm1=1 **ind_uv_avail=0**`, its only MDS3 candidate being lm=0 with
`cand_uv=0`. The port always took the SPATIAL path at M2..M5 → picked CfL where
C keeps DC.

### Fix

`leaf_funnel.rs`: `cfl_uv_follows = ind_uv.is_none()` / `cfl_ind_uv =
ind_uv.is_some()` (was keyed off `cfg.ind_uv_mds3` / `cfg.ind_uv_independent`).
`ind_uv` is already `Some` iff that same search ran — its `any(uv != 0)` gate
**is** `perform_ind_uv_search_last_mds` for `skip_ind_uv_if_only_dc = 1`, and the
port's `update_intra_chroma_mode` equivalent already gates on it, mirroring C
`:7435` `if (ctx->ind_uv_avail && ind_uv_last_mds) update_intra_chroma_mode(...)`.
So `ind_uv.is_some()` **is** `ind_uv_avail`; it simply was not wired to the CfL
path selection.

Provably a no-op outside M2..M5: M6-M10 have `ind_uv_mds3=false` +
`ind_uv_independent=None` so `ind_uv` is always `None` (old expression also
`true`); M0/M1 have `ind_uv_independent=Some` so `ind_uv` is always `Some` inside
the `has_uv`-gated CfL block (old expression also `true`).

### Results

- **real M2-M5: 65/96 IDENTICAL** (was 56/96 at `ef8fed79c`). Per-cell diff vs
  that baseline: **0 regressions, +9 newly identical** — 1001682 q20 p5,
  1459534 q20 p4/q20 p5/q40 p5, 1963557 q40 p5, **258947 q40 p3** (the repro),
  3571065 q20 p5/q40 p5, **5739122 q20 p5** (the second cell the prior session
  flagged as "the identical root" — confirmed).
  p2 13/24, p3 11/24, p4 19/24, p5 22/24.
- synthetic identity M0-M6: **82/84, unchanged** (the only 2 non-identical are
  the pre-existing M0 sub-8 gaps #58/#59, both preset-0 gradient).
- `cargo test -p svtav1-encoder`: **146 passed, 0 failed**.
- C tree verified pristine; all instrumentation reverted.
- Residual 31 = 28 FH (mostly `loop_filter_level` — the LF search reads the
  recon, so DOWNSTREAM of a remaining per-block recon/mode difference) + 3 tile.
  Mean Rust/C size ratio over non-identical cells: 1.0004.

### Latent bug found while root-causing (task #85, NOT fixed here)

The CfL chroma-complexity detector's **SAD arm** mismatches C on **74/7323** MDS3
blocks. C's `chroma_complexity_check_pred` (`product_coding_loop.c:6117`) reads
`cand_bf->pred->u_buffer`, and `md_stage_3` (`:7435`) does **not** re-predict
chroma before `full_loop_core` — so that buffer holds a **stale** prediction from
an earlier MD stage — while the port re-predicts fresh from `cand.uv`. Measured
at x328 y440 8×8 lm=10: C `y_dist2=122 cb_dist=142 cr_dist=10 sad_arm=1` vs port
`y_dist2=122 cb_dist=110 cr_dist=6 sad_arm=0` — the **luma** SAD matches exactly,
only the **chroma** SADs differ. The **variance arm is byte-exact** (0/7323
mismatches), and `chroma_detector_fires`' shift/rows/cw/`y_dist<<1` structure is
C-exact. Currently latent: 0 visible uv divergences on this cell (at x328 y440
both sides still land on DC).

### Method note

C dumps `SVT_CFLDUMP` / `SVT_CFLRD` / `SVT_CFLDET` / `SVT_CFLGATE` vs port
`SVTAV1_CFLDUMP` / `SVTAV1_CFLRD` / `SVTAV1_CFLDET`, joined on `(x,y,w,h,lm)`.

**Stale binaries can no longer haunt this harness — made structurally
impossible.** The trap that cost a cycle here: the C lib was rebuilt with new
instrumentation, but `capture_c_trace` still linked the PREVIOUS archive, so the
dump printed nothing and "C never calls this function" was briefly concluded from
a binary that predated the lib. Nothing failed loudly. The fix is not a reminder:

- `tools/capture_c_trace/capture_c_trace` is now a **tracked wrapper script**;
  the real binary is `capture_c_trace.bin` (gitignored). The wrapper always runs
  `build.sh` and then execs, so the driver cannot be invoked without a freshness
  check — the raw binary is no longer sitting at the path you'd type.
- `build.sh` now also runs `cmake --build cbuild-static` first (a fast no-op when
  current), closing the other half: `Source/*.c` edited but the lib not rebuilt.
  A failed C build now **aborts** instead of silently reusing the old archive.
- `tools/identity_run` is the same guarantee for the Rust side (`cargo build`,
  then exec) — running `target/release/examples/identity_run` directly could
  otherwise compare C against a PREVIOUS Rust encoder.
- Both wrappers **buffer build output and emit it only on failure**. This is
  required, not cosmetic: `identity_diff.sh` captures the runner's stderr into
  `rs.trace` (the op stream the differ parses), so unconditional build chatter
  would corrupt every comparison.
- `identity_diff.sh` no longer builds anything itself; it just calls the
  wrappers. Do not "optimize" it back to the raw binaries.

Verified: touching `Source/Lib/Codec/md_rate_estimation.c` and then invoking only
the wrapper relinked `capture_c_trace.bin` with zero human action and kept the
output byte-correct; `rs.trace` contains 0 cargo-chatter lines.
