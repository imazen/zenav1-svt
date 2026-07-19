# Screen-content detection port map (task #71, extracted 2026-07-16 from v4.2.0 C)

Complete port-ready map of the allintra sc-detection pipeline. Verified against
`/root/svtav1/Source` at the v4.2.0 tag by whole-chain trace. Feeds #71 work
items 1–3 (detection → level tables → header bit).

## Call chain (allintra, default multi-threaded)

1. `enc_handle.c:4286-4291` — `scs->allintra` derived (intra_period 0 / avif / ALL_INTRA).
2. `enc_handle.c:4257` — `fast_aa_aware_screen_detection_mode = (enc_mode >= ENC_M3)`
   (changes the SCAN PATTERN, not just speed — see below).
3. `enc_handle.c:4514-4527` — allintra `screen_content_mode`: user 0/1 pass through;
   else `<= ENC_M7` → **3** (AA-aware auto-detect), M8+ → 0 (warned off).
   **TUNE_IQ override** at `:4738-4752` forces mode 3 unconditionally (even M8+).
   CLI default is 2 (`enc_settings.c:983`); mode 2 = classic detector, ≤1080p only,
   never sets sc_class5.
4. Picture-analysis kernel (`pic_analysis_process.c:1927`): pad → preproc → stats →
   `:2005-2022` switch on scm: case 3 → `svt_aom_is_screen_content_antialiasing_aware`
   (`:1207`, no resolution gate); case 1/0 force all classes 1/0.
5. `pd_process.c:4769 perform_sc_detection` — I-slice: reuse (MT) or recompute (ST);
   caches `last_i_picture_sc_classN` (dead branch for pure allintra).
6. `enc_mode_config.c:2337 svt_aom_sig_deriv_multi_processes_allintra`:
   intrabc level (`:2346-2370`, sc_class5-gated: MR→1 M0→3 M1→4 M2→5 M3→6 M4→7 M5+→0),
   palette level (`:2374-2390`, sc_class5-gated: M0-M2→2 M3→3 M4-M5→4 M6→5 M7→7 M8+→0),
   `frm_hdr->allow_intrabc = intrabc_ctrls.enabled` (`:2371`),
   `frm_hdr->allow_screen_content_tools = (palette_level || allow_intrabc) ? 1 : 0` (`:2393`).
7. Writer: seq `seq_force_screen_content_tools = 2` always (`sequence_control_set.c:96`,
   written `entropy_coding.c:2807-2811`); frame bit `:3345-3348`; `allow_intrabc` bit
   `:3466/:3474` gated on sct && superres-unscaled.

## The AA-aware detector (`pic_analysis_process.c:1207-1350`)

Input: **padded** (multiple-of-8, edge-replicated — `pic_operators.c:393`) 8-bit luma
plane only. At 10-bit input it reads the 8-bit MSB plane (truncation, NOT rounding).
Runs before PD, per picture.

Two passes over non-overlapping blocks, row-major (`svt_aom_sc_AA_collect_counts`
`:1088-1190`), 16×16 then 8×8. Per block:

- `count_colors_with_threshold(src, 40)` → n (unique-value scan, early exit).
- n in (1, 4]: `is_palette=1`; if `variance > var_thresh` also `is_intrabc=1`.
- n in (4, 40]: dominant-color 8-neighbor **dilation** (`:1024-1076`), recount with
  final thresh (16×16: 6, 8×8: 8); if recount ok AND variance(ORIGINAL block) >
  var_thresh (16×16: 5, 8×8: 50): `is_palette=1` AND `is_intrabc=1` (both or neither).
- n > 40: `is_photo=1`. n ≤ 1: nothing (solid).
- Counters: global count_{palette,intrabc,photo} + per-quadrant region_* (quadrant =
  `((r>=h/2)?2:0)+((c>=w/2)?1:0)`).

Variance = `Σ(x-mean)²` scaled: `vf(src, stride, all-128 buf, b_stride=0)` then
`ROUND_POWER_OF_TWO(var, log2pels)` (÷64 for 8×8, ÷256 for 16×16, rounded).

**Checkerboard fast mode** (`fast_detection`, enc_mode ≥ M3): each block-row starts at
col 0 or blk_w alternately, steps 2·blk_w — half the blocks visited; ALL counters
(incl. region) ×2 at the end (`:1177-1187`). Bit-exactness requires replicating the
exact visited set per preset.

Class formulas (`:1297-1321`, area = w·h as i64, region_area = area>>2):

```
sc_class0 = (palette16 - photo16/16)·256·10 > area
sc_class1 = sc_class0 && (intrabc16 - photo16/16)·256·12 > area
sc_class2 = sc_class1 || (palette16·256·15 > area·4 && intrabc16·256·30 > area)
sc_class3 = sc_class1 || (palette16·256·8  > area   && intrabc16·256·50 > area)
pass = #quadrants with (region_palette8·64·10 > region_area && region_intrabc8·64·25 > region_area)
sc_class4 = pass>=3 && palette8·64·5  > area
sc_class5 = pass>=3 && palette8·64·10 > area && intrabc8·64·23 > area
```

## Other sc_class5 consumers (post-header, port later)

- Depth refinement level (`enc_mode_config.c:10058-10080`, sc arm: M1→1 M2→5 M4→6 M5→9 M6+→10).
- CDEF QP-strength pick (`enc_cdef.c:897-925`, `sc = allintra ? sc_class5 : sc_class1`,
  only at cdef_search_level 10 / use_qp_strength).
- ME boost (`enc_mode_config.c:804-815`) — inert on allintra.
- CDEF search forced 0 when `allow_intrabc` (`:2396-2428`).

## Port order (each standalone-testable)

1. Leaf primitives: count_colors_with_threshold, find_dominant_value, dilate_block,
   variance (8×8/16×16) — golden-block unit tests + C parity via wraps.
2. `sc_AA_collect_counts` (full + checkerboard scans, quadrant bucketing).
3. The detector + 6 class formulas (input: padded 8-bit luma + dims only).
4. Padding (multiple-of-8 edge replication) for non-aligned sources.
5. scm derivation incl. TUNE_IQ override.
6. Header derivation slice (intrabc/palette levels → allow_intrabc/sct) — table-testable.
7. Consumers in §above.

Gotchas: detection sees padded+preprocessed (denoise if enabled) picture; partial edge
blocks contain replicated pixels; `PictureDecisionResults.sc_class0/1/2` are dead
fields; overlay path omits sc_class5 (unreachable in allintra).
