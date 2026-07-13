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

1. **Final coefficient parity (the coding quantizer) — now owns EVERY
   remaining gradient p13/p10 tile divergence** (g64 q40 op8 / q20 op21 /
   q55 op8, g128 q20 op10). Correction to the superseded item 2 above:
   the C path is the REGULAR-PD1 MDS3 quant, not light-PD1 —
   `svt_aom_quantize_inv_quantize` with `quants_8bit` (zbin/round
   84-80/48; pd0.rs `build_quant_entry`/`quantize_b` are the C-exact
   primitives) and RDOQ per `pcs->rdoq_level` from the allintra config
   (enc_mode_config.c:14931: 0 at coeff_lvl HIGH, 3 at NORMAL, else 2 —
   instrument `pcs->coeff_lvl` for these cells before assuming which
   RDOQ level binds), plus the coeff-shaving/eob path of
   `md_stage_3`/`full_loop_core`.
2. **Non-DC-only leaf cost chase** (latent, currently non-binding at
   the tracked cells): where the gate does NOT fire (e.g. the q55
   64x64), C runs the 4-candidate {DC, V, H, SMOOTH} funnel — MDS0
   Hadamard SATD fast cost (`mds0_use_hadamard_sb=true`, mds0_level 0)
   -> NIC counts (nic_level 11 at eff-M9) -> MDS3 full loop (spatial
   SSE level 3, `svt_av1_cost_coeffs_txb` rate, `svt_aom_intra_fast_cost`
   mode rate). Our homegrown loop happens to agree (DC) at q55; any
   future cell where it disagrees lands here.
3. **g128 q40/q55 FH `cdef_uv_pri_strength`** — unchanged, the CDEF
   search gap (2a in the main list).

Preset-6 gradient cells: unchanged, see the note above (different PD0
architecture at M6).
