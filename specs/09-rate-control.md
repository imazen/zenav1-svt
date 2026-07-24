# Rate Control

## Overview

SVT-AV1 implements four rate control modes that govern how quantization parameters are selected for each frame:

- **CQP (Constant QP)**: Fixed quality with hierarchical QP offsets. No rate model. Simplest mode.
- **CRF (Constant Rate Factor)**: Quality-targeted encoding using TPL (Temporal Prediction Lookahead) statistics to derive per-frame QP from a base quality setting. Produces variable bitrate.
- **VBR (Variable Bitrate)**: Target bitrate with two-pass statistics driving a rate model. QP varies to hit a bitrate target while distributing bits across GOPs.
- **CBR (Constant Bitrate)**: Strict bitrate compliance using a leaky-bucket buffer model. QP adjusted per-frame to maintain buffer level within bounds. Supports low-delay operation.

All modes share a common pipeline: frame-level QP selection, followed by optional superblock-level delta-QP adjustment (Variance Boost, TPL-based QPM, Cyclic Refresh), followed by delta-Q normalization and lambda derivation for RDO.

The rate control system also supports:
- Capped CRF (CRF with a maximum bitrate constraint)
- Multi-pass encoding (first pass collects statistics, second pass uses them)
- One-pass VBR with look-ahead (lap_rc)
- Recode loops to correct overshoot/undershoot after encoding
- Dynamic resize for CBR low-delay

## Source Files

| File | Description |
|------|-------------|
| `rc_process.c` / `.h` | Main rate control kernel thread. Dispatches to CRF/CQP or VBR/CBR paths. Contains lambda computation, QP conversion utilities, boost calculation, common RC init. |
| `rc_crf_cqp.c` | CRF and CQP qindex calculation. Frame-level QP scaling, capped CRF re-encode logic, coded frames statistics for sliding window. |
| `rc_vbr_cbr.c` | VBR and CBR qindex calculation. Buffer model, rate correction factors, active quality computation, recode loop, post-encode updates. |
| `rc_aq.c` | Adaptive quantization: Variance Boost, TPL-based SB QP derivation, Cyclic Refresh SB QP, ME-based lambda modulation, delta-Q normalization. |
| `initial_rc_process.c` / `.h` | Initial rate control kernel. Look-ahead queue management, TPL group formation, first-pass VBR parameter setup. |
| `pass2_strategy.c` / `.h` | Two-pass strategy: second pass initialization, RC stat processing, KF/GF group rate assignment, frame rate updates, post-encode VBR corrections. |
| `rc_tables.h` | MinQ lookup tables for KF (low/high motion), ARF/GF (low/high motion), inter frames, and RTC mode, at 8/10/12 bit depths. |
| `rc_tasks.c` / `.h` | Rate control task message types (`RC_INPUT`, `RC_PACKETIZATION_FEEDBACK_RESULT`, `RC_INPUT_SUPERRES_RECODE`). |
| `rc_results.c` / `.h` | Rate control results message (passes PCS wrapper and superres recode flag downstream). |
| `lambda_rate_tables.h` | SAD-domain lambda tables for 8-bit, 10-bit, and 12-bit mode decision, indexed by qindex [0..255]. |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| `test/e2e_test/SvtAv1E2ETest.cc` | End-to-end encoding with various rate control modes |
| `test/e2e_test/SvtAv1E2EParamsTest.cc` | Rate control parameter validation |
| `test/api_test/SvtAv1EncParamsTest.cc` | API parameter bounds for RC settings |
| `test/api_test/params.h` | Parameter definitions including RC mode enums |

## Data Structures

### RATE_CONTROL

Central state for rate control, stored in `EncodeContext`:

```
RATE_CONTROL {
    // Quality tracking
    last_boosted_qindex: int          // Last boosted (KF/GF/ARF) qindex
    gfu_boost: int                     // Golden frame boost factor
    kf_boost: int                      // Key frame boost factor
    arf_q: int                         // QP used for ALT reference frame
    avg_frame_qindex[FRAME_TYPES]: int // Running average qindex per frame type
    last_q[FRAME_TYPES]: int           // Last Q per frame type
    active_worst_quality: int          // Current worst quality bound
    active_best_quality[MAX_ARF_LAYERS+1]: int  // Best quality per pyramid level

    // Rate correction
    rate_correction_factors[MAX_TEMPORAL_LAYERS+1]: double  // Per-layer rate model correction

    // GOP structure
    baseline_gf_interval: int
    constrained_gf_group: int
    frames_to_key: int
    frames_since_key: int

    // Buffer model (CBR/VBR)
    buffer_level: int64               // Current buffer fullness
    bits_off_target: int64            // Cumulative over/undershoot
    vbr_bits_off_target: int64        // VBR-specific drift
    vbr_bits_off_target_fast: int64   // Fast redistribution pool
    starting_buffer_level: int64
    optimal_buffer_level: int64
    maximum_buffer_size: int64

    // Frame bandwidth
    avg_frame_bandwidth: int          // Average target bits per frame
    max_frame_bandwidth: int          // Maximum allowed bits for any frame

    // Rolling monitors
    rolling_target_bits: int
    rolling_actual_bits: int
    rate_error_estimate: int
    total_actual_bits: int64
    total_target_bits: int64

    // Quality bounds
    worst_quality: int                // Absolute worst qindex allowed
    best_quality: int                 // Absolute best qindex allowed

    // RC history (for CBR Q clamping)
    rc_1_frame: int                   // -1=undershoot, 1=overshoot, 0=neutral (prev frame)
    rc_2_frame: int                   // Same, two frames ago
    q_1_frame: int                    // Qindex of previous frame
    q_2_frame: int                    // Qindex of two frames ago

    // Cyclic refresh
    percent_refresh_adjustment: int
    rate_ratio_qdelta_adjustment: double

    // Coded frames statistics queue (for capped CRF sliding window)
    coded_frames_stat_queue: array of coded_frames_stats_entry
    coded_frames_stat_queue_head_index: uint32

    // ME distortion tracking
    cur_avg_base_me_dist: uint32
    prev_avg_base_me_dist: uint32
    avg_frame_low_motion: int

    // Dynamic resize state
    resize_state: RESIZE_STATE
    resize_avg_qp: int32
    resize_buffer_underflow: int32
    resize_count: int32
}
```

### RateControlIntervalParamContext

Per-GOP rate control parameters (used in gop_constraint_rc mode):

```
RateControlIntervalParamContext {
    size: int
    processed_frame_number: int
    first_poc: uint64
    vbr_bits_off_target: int64
    vbr_bits_off_target_fast: int64
    rate_error_estimate: int
    total_actual_bits: int64
    total_target_bits: int64
    extend_minq: int
    extend_maxq: int
    extend_minq_fast: int
    rolling_target_bits: int
    rolling_actual_bits: int
    kf_group_bits: int64
    kf_group_error_left: double
    end_of_seq_seen: int
}
```

### CyclicRefresh

Per-frame cyclic refresh state for CBR adaptive QP:

```
CyclicRefresh {
    apply_cyclic_refresh: bool
    percent_refresh: int
    sb_start: uint32
    sb_end: uint32
    max_qdelta_perc: int
    rate_ratio_qdelta: double
    rate_boost_fac: int
    qindex_delta[3]: int              // Per segment: BASE, BOOST1, BOOST2
    actual_num_seg1_sbs: int
    actual_num_seg2_sbs: int
}
```

### coded_frames_stats_entry

Entry in the sliding window statistics queue for capped CRF:

```
coded_frames_stats_entry {
    picture_number: uint64
    frame_total_bit_actual: int64     // -1 if not yet coded
    end_of_sequence_flag: bool
}
```

### RateControlTaskTypes

```
enum RateControlTaskTypes {
    RC_INPUT = 0,                      // Normal frame for QP assignment
    RC_PACKETIZATION_FEEDBACK_RESULT,  // Post-encode feedback from packetization
    RC_INPUT_SUPERRES_RECODE,          // Re-encode after super-resolution decision
}
```

### rate_factor_level

```
enum rate_factor_level {
    INTER_NORMAL = 0,   // Regular inter frames
    INTER_LOW    = 1,   // Low complexity inter
    INTER_HIGH   = 2,   // High complexity inter
    GF_ARF_LOW   = 3,   // Internal ARF (lower boost)
    GF_ARF_STD   = 4,   // GF/ARF standard boost
    KF_STD       = 5,   // Key frame
}
```

Mapping from update type to rate factor level:

| Update Type | Rate Factor Level |
|---|---|
| KF_UPDATE | KF_STD |
| LF_UPDATE | INTER_NORMAL |
| GF_UPDATE | GF_ARF_STD |
| ARF_UPDATE | GF_ARF_STD |
| OVERLAY_UPDATE | INTER_NORMAL |
| INTNL_OVERLAY_UPDATE | INTER_NORMAL |
| INTNL_ARF_UPDATE | GF_ARF_LOW |

## Algorithms

### 1. Rate Control Mode Selection (CQP)

CQP is the simplest mode. When TPL is disabled, the encoder uses `cqp_qindex_calc()`:

1. Start with the configured `scs_qindex` (from user QP setting, mapped through `quantizer_to_qindex[]`).
2. For all-intra or flat IPP non-I-slice: return the base qindex unchanged.
3. For hierarchical encoding:
   - Determine `offset_idx` from temporal layer: IDR=0, else `min(temporal_layer+1, FIXED_QP_OFFSET_COUNT-1)`, non-ref=-1.
   - Compute `q_val_target = q_val - q_val * percents[hierarchical_levels <= 4][offset_idx] / 100`.
   - For low-delay non-base: apply `non_base_boost()` which measures intra-coded area in reference frames and reduces QP proportionally.
   - Convert back: `q = qindex + compute_qdelta(q_val, q_val_target, bit_depth)`.

An alternative path (`TUNE_CQP_CHROMA_SSIM`) uses qstep-ratio based calculation:
- Base layer: `qstep_ratio = (0.2 + (1 - awq/MAXQ) * qratio_grad) * compress_weight`. Derive `cqp_base_q` from this ratio.
- Non-leaf reference: iteratively average `arf_q` toward `active_worst_quality` for each pyramid level.
- Leaf: use `active_worst_quality` directly.

### 2. Rate Control Mode Selection (CRF)

CRF is selected when `rc_cfg.mode == AOM_Q` and TPL is enabled. Uses `crf_qindex_calc()`:

**r0 computation and scaling:**
1. For each frame, `svt_aom_generate_r0beta()` computes r0 from TPL statistics.
2. r0 is adjusted by `tpl_ctrls.r0_adjust_factor` to compensate for reduced TPL group size.
3. r0 is further divided by a hierarchical-level-dependent factor:
   - I-slices: `svt_av1_tpl_hl_islice_div_factor[hl]` (values: 1, 2, 2, 1, 1, 0.7 for 1L-6L)
   - Base frames: `svt_av1_tpl_hl_base_frame_div_factor[hl]` (values: 1, 3, 3, 2, 1, 1)

**Boost computation:**
- Key frames: `kf_boost = (3 or 4) * (75 + 17 * factor) / r0` where `factor = clamp(sqrt(frames_to_key), 4, 10)`. Resolution-dependent multiplier (3 for <=720p, 4 otherwise). Capped at `used_tpl_frame_num * 400`.
- Golden/ARF frames: `gfu_boost = (200 + 10 * factor) / r0` where `factor = clamp(sqrt(frames_to_key), min_factor, MAX_GFUBOOST_FACTOR)`.

**QP derivation (qstep-based path, used when `r0_qps` is set):**
1. Select weight from `svt_av1_r0_weight[]`: 0.75 for I_SLICE, 0.9 for base, 1.0 for non-base.
2. `qstep_ratio = sqrt(r0) * weight * qp_scale_compress_weight[strength]`.
3. If `qp_scale_compress_strength > 0`: clamp `qstep_ratio` to not exceed weight.
4. Find qindex with matching qstep: `svt_av1_get_q_index_from_qstep_ratio(base_qindex, qstep_ratio, bit_depth)`.
5. `active_best_quality = clamp(qindex_from_qstep_ratio, best_quality, base_qindex)`.
6. `active_worst_quality = (active_best + 3*active_worst + 2) / 4`.

**QP derivation (reference-based path, used when TPL not generating r0 for this layer):**
1. `active_best_quality = cq_level` (the user-configured qindex).
2. For internal ARF frames: inherit `arf_q` from reference frames, then interpolate toward `cq_level` using weights `svt_av1_non_base_qindex_weight_ref[]` and `svt_av1_non_base_qindex_weight_wq[]`.

**Post-adjustment:**
- Non-base frames: `active_best = max(active_best, arf_q)`.
- `adjust_active_best_and_worst_quality()`: applies `svt_av1_frame_type_qdelta()` to worst quality for non-intra frames.
- Final: `ppcs->top_index = active_worst`, `ppcs->bottom_index = active_best`.

### 3. Rate Control Mode Selection (VBR)

VBR uses two-pass or one-pass-with-look-ahead statistics. Combines a rate model with TPL.

**Rate allocation (`svt_av1_rc_process_rate_allocation`):**
1. On first frame or parameter change: initialize buffer sizes and call `svt_av1_rc_init()`.
2. Process TPL stats for KF/GF/ARF frames via `process_tpl_stats_frame_kf_gfu_boost()`.
3. Restore two-pass parameters (kf_group_bits, frames_to_key, etc.).
4. Call `svt_aom_process_rc_stat()` which:
   - For KF: runs `kf_group_rate_assingment()` to distribute bits across the KF group.
   - For new GF groups: runs `gf_group_rate_assingment()`.
5. Apply VBR rate correction (`vbr_rate_correction()`):
   - Distribute `vbr_bits_off_target` over remaining frames (limited to `VBR_PCT_ADJUSTMENT_LIMIT` = 50% of target).
   - Fast redistribution from `vbr_bits_off_target_fast` for massive local undershoot.

**QP selection (`rc_pick_q_and_bounds`):**
1. For base layer (temporal_layer == 0): qstep-ratio-based calculation (same as CRF, using `active_worst_quality` as base).
2. For non-base layers:
   - Low pyramid levels: use `get_active_best_quality()` with minQ lookup tables and boost interpolation.
   - High pyramid levels (>1): inherit from `rc->active_best_quality[pyramid_level-1]` and blend with `active_worst_quality` using weights.
   - For GF/ARF/internal ARF: `active_worst = (active_best + 3*active_worst + 2) / 4`.
3. `adjust_active_best_and_worst_quality_org()`: applies `extend_minq`, `extend_maxq` from drift correction.
4. `get_q()`: binary-search for Q matching target bits per MB using `av1_rc_regulate_q()`.

**Post-encode update (`svt_av1_rc_postencode_update` / `_gop_const`):**
1. Update rate correction factors based on actual vs. projected frame size.
2. Update rolling averages: `rolling_target_bits = (3*rolling + target) / 4`.
3. Track `vbr_bits_off_target` drift.
4. Adjust `extend_minq`/`extend_maxq` based on `rate_error_estimate`:
   - Undershoot (positive error) > `under_shoot_pct`: decrease maxq extension, increase minq extension.
   - Overshoot (negative error) < `-over_shoot_pct`: decrease minq extension, increase maxq extension.
5. Fast undershoot recovery: if frame is much smaller than expected, pool extra bits in `vbr_bits_off_target_fast`.

### 4. Rate Control Mode Selection (CBR)

CBR uses a leaky-bucket buffer model with no look-ahead.

**Buffer initialization (`set_rc_buffer_sizes`):**
```
starting_buffer_level = starting_buffer_level_ms * target_bit_rate / 1000
optimal_buffer_level  = (optimal_ms == 0) ? bandwidth/8 : optimal_ms * bandwidth / 1000
maximum_buffer_size   = (maximum_ms == 0) ? bandwidth/8 : maximum_ms * bandwidth / 1000
```

**Frame target calculation (`av1_calc_pframe_target_size_one_pass_cbr`):**
1. `diff = optimal_buffer_level - buffer_level`.
2. If `diff > 0` (buffer below optimal): reduce target by `min(diff/one_pct_bits, under_shoot_pct)` percent.
3. If `diff < 0` (buffer above optimal): increase target by `min(-diff/one_pct_bits, over_shoot_pct)` percent.
4. Clamp to `max_inter_bitrate_pct` if configured.
5. Floor at `max(avg_frame_bandwidth >> 4, FRAME_OVERHEAD_BITS)`.

**KF target (`av1_calc_iframe_target_size_one_pass_cbr`):**
- First frame: `starting_buffer_level / 2`.
- Otherwise: `kf_boost = max(32, 2*framerate - 16)`, scaled down if close to previous keyframe. Target = `(16 + kf_boost) * avg_frame_bandwidth / 16`, clamped by `max_intra_bitrate_pct`.

**Active worst quality (`calc_active_worst_quality_no_stats_cbr`):**
- Buffer above optimal: lower quality from `ambient_qp * 5/4` by up to 33%.
- Buffer between critical and optimal: raise from `ambient_qp` toward `worst_quality`.
- Buffer below critical (`optimal >> 3`): set to `worst_quality`.

**Active best quality (`calc_active_best_quality_no_stats_cbr`):**
- KF: use `get_kf_active_quality_tpl()` with minQ tables, adjusted for small resolutions.
- Inter (flat IPP): `arf_q = max(0, ref_base_q_idx - 30)`, then `rtc_minq[arf_q]`.
- Inter (hierarchical): inherit from reference with lowest temporal layer, interpolate toward worst quality per layer delta.

**Q clamping (`adjust_q_cbr` / `adjust_q_cbr_flat`):**
- If previous two frames showed opposite over/undershoot: clamp Q between `q_1_frame` and `q_2_frame`.
- Content-adaptive: if ME distortion decreased and buffer stable, reduce Q; if increased, dampen Q decrease.
- Limit delta from previous frame: `max_delta_down` per layer, `max_delta_up` = 90 (or 120 if overshoot + low buffer).

**Buffer update (`update_buffer_level`):**
```
if showable_frame:
    bits_off_target += avg_frame_bandwidth - encoded_frame_size
else:
    bits_off_target -= encoded_frame_size
bits_off_target = min(bits_off_target, maximum_buffer_size)
buffer_level = bits_off_target
```

### 5. Multi-Pass Encoding

**First pass (`ENC_FIRST_PASS`):**
Statistics collected per frame (stored in `FIRSTPASS_STATS`):
- `coded_error`: Sum of inter prediction error.
- `intra_error`: Sum of intra prediction error.
- `frame`: Frame number.
- `count`: Running count.
- `duration`: Frame duration.
- `stat_struct`: Contains per-frame statistics including `total_num_bits`, `temporal_layer_index`, `poc`.
- Motion vectors, percentage of zero-motion blocks, etc.

For one-pass VBR with look-ahead (`lap_rc`), `set_1pvbr_param()` synthesizes first-pass-like statistics from ME distortion:
```
coded_error = avg_me_dist * b64_total_count * weight / VBR_CODED_ERROR_FACTOR
```
where `weight` is 1.0 (or 1.5 for high variance or low resolution).

**Second pass (`ENC_SECOND_PASS`):**
`svt_av1_init_second_pass()`:
1. Accumulate all first-pass statistics.
2. Compute `modified_error_total` using bias/power function scaled by VBR section percentages.
3. Set `bits_left = duration * target_bit_rate`.
4. Initialize `modified_error_left` for progressive bit allocation.

During encoding, `svt_aom_process_rc_stat()`:
1. On KF: `kf_group_rate_assingment()` allocates bits for the entire KF group.
2. On new GF group: `gf_group_rate_assingment()` allocates bits for the GF group.
3. Frame targets derived from group allocations.

### 6. Adaptive Quantization

Three independent SB-level QP adjustment mechanisms, applied in sequence by `svt_av1_rc_init_sb_qindex()`:

#### 6a. Variance Boost (Spatial AQ)

Enabled when `enable_variance_boost` is set and mode is not CBR. Applied by `svt_av1_variance_adjust_qp()`:

1. Sets `delta_q_present = 1`.
2. For each SB, calls `av1_get_deltaq_sb_variance_boost()`:
   - Extracts 64 8x8 block variances, sorts them.
   - Samples at three octile positions (specified octile +/- 1), weighted 1:2:1.
   - Variance 0 treated as 1 (assumed fine gradient, not flat).
   - Three curve options for qstep_ratio:
     - **Curve 0 (default)**: `qstep_ratio = pow(1.018, strength_val * (-10*log2(var) + 80))` where `strength_val` from `{0, 0.65, 1.1, 1.6, 2.5}`.
     - **Curve 1 (low-medium contrast)**: `qstep_ratio = 0.25 * strength * (-log2(var) + 8) + 1`.
     - **Curve 2 (still picture)**: `qstep_ratio = 0.15 * strength * (-log2(var) + 10) + 1`.
   - Clamp to `[1, VAR_BOOST_MAX_QSTEP_RATIO_BOOST=8]`.
   - Convert to boost: `boost = (base_q_idx + K) * -qdelta / (255 + K)` where K=40 (curves 0,1) or K=544 (curve 2).
   - Cap at `VAR_BOOST_MAX_DELTAQ_RANGE = 80`.
3. `sb->qindex = clip(sb->qindex - boost, 1, MAXQ)`.
4. Normalize: center the SB qindex range around `min + range/2`, clamp offsets to `[-40, +40]`.

#### 6b. TPL-Based SB QP (QPM)

Enabled when `aq_mode == 2`, TPL is active, and r0 != 0. Applied by `svt_aom_sb_qp_derivation_tpl_la()`:

1. If `r0_delta_qp_quant`: sets `delta_q_present = 1`.
2. For each SB where `r0_delta_qp_md` and TPL is valid:
   - Read `beta = tpl_beta[sb_addr]` (ratio of intra cost to inter cost from TPL).
   - Compute delta: for inter, `newq = q / sqrt(sqrt(beta))` if beta > 1 (gentler reduction); for intra or beta <= 1, `newq = q / sqrt(beta)`.
   - Clamp offset to `[-35, +35]`.
   - `sb->qindex = clip(sb->qindex + offset, 1, MAXQ)`.
3. Call `sb_setup_lambda()` to compute per-SB lambda scaling factors from TPL rdmult data.

#### 6c. Cyclic Refresh (CBR only)

Enabled for CBR when `aq_mode` is set, SB size is 64, and conditions are met. Applied by `cyclic_sb_qp_derivation()`:

1. A rotating window (`sb_start` to `sb_end`) covers `percent_refresh`% of SBs each cycle.
2. Within the window:
   - SBs with ME distortion below average: assigned `CR_SEGMENT_ID_BOOST2` (large negative delta-Q).
   - SBs with ME distortion above average: assigned `CR_SEGMENT_ID_BOOST1` (moderate negative delta-Q).
3. Delta-Q computed via `svt_av1_compute_deltaq()` using `rate_ratio_qdelta`.
4. Boost factor increases quadratically with the deviation of segment-2 distortion from average: `boost = BOOST_MAX * (dev/100)^2`.
5. Post-encode: cyclic refresh parameters (`percent_refresh_adjustment`, `rate_ratio_qdelta_adjustment`) are adapted based on actual over/undershoot.

### 7. QP Assignment Pipeline

The complete QP assignment flow in `svt_aom_rate_control_kernel()`:

**Step 1: Frame statistics initialization (`rc_init_frame_stats`)**
- Generate r0/beta from TPL data.
- Initialize cyclic refresh if CBR.
- Collect reference frame statistics (intra %, skip %, HP %).
- Store average ME distortion.

**Step 2: Frame-level QP selection**
- CRF/CQP path (`svt_av1_rc_calc_qindex_crf_cqp`):
  1. Determine base qindex from user QP + extended CRF offset.
  2. If CRF (TPL enabled): call `crf_qindex_calc()`.
  3. If CQP (TPL disabled): call `cqp_qindex_calc()`.
  4. Apply fixed qindex offsets if configured.
  5. Apply extended CRF range compression.
  6. Apply luminance QP bias (darker scenes get lower QP).
  7. Apply S-frame QP offset.
  8. Apply capped CRF max rate adjustment.
  9. Compute chroma qindex deltas.
- VBR/CBR path:
  1. `svt_av1_rc_process_rate_allocation()`: compute frame target bits.
  2. `svt_av1_rc_calc_qindex_rate_control()`: pick Q from rate model + clamp by reference QP.

**Step 3: Overlay QP propagation**
- Alt-ref overlay frames inherit QP from their alt-ref frame.

**Step 4: Super-resolution check**
- If super-res is enabled, may trigger re-ME and return to step 2.

**Step 5: SB-level QP adjustment (`svt_av1_rc_init_sb_qindex`)**
1. Initialize all SBs to `base_q_idx`.
2. Apply Variance Boost (if enabled and not CBR).
3. Apply TPL-based QPM (if `aq_mode == 2` and TPL available).
4. Apply Cyclic Refresh (if CBR conditions met).
5. Normalize delta-Q to configured `delta_q_res` granularity.
6. Generate ME-based lambda modulation map (`generate_b64_me_qindex_map`).

**Step 6: Final QP clamping**
```
picture_qp = clamp((base_q_idx + 2) >> 2, min_qp_allowed, max_qp_allowed)
```

### 8. Delta-QP Coding

When `delta_q_present == 1` in the frame header:
- Each SB stores its own `qindex` value.
- The delta from `base_q_idx` is quantized to a multiple of `delta_q_res` (2, 4, or 8).
- `svt_av1_normalize_sb_delta_q()` adjusts SB qindex values to minimize rounding error:
  1. Compute `mask = ~(delta_q_res - 1)`.
  2. Find remainder: `base_q_idx & ~mask`.
  3. Adjustment = `(delta_q_res - remainder) - delta_q_res/2`.
  4. Each SB: `adjusted = clip(qindex + adjustment, 1, MAXQ)`, then `normalized = (adjusted & mask) + remainder`.
  5. Avoid qindex 0 (lossless mode not supported in lossy encoding).

### 9. Lambda Calculation for RDO

**Full lambda (SSE domain, `svt_aom_compute_rd_mult`):**
1. `q = dc_quant_qtx(qindex, 0, bit_depth)`.
2. Base rdmult depends on frame type:
   - KF: `rdmult = (3.3 + 0.0015*q) * q * q`
   - GF/ARF: `rdmult = (3.25 + 0.0015*q) * q * q`
   - Inter: `rdmult = (3.2 + 0.0015*q) * q * q`
3. Bit-depth normalization: 10-bit divides by 16, 12-bit divides by 256.
4. `update_lambda()` applies:
   - Frame type scaling: `rd_frame_type_factor[hbd][update_type]` (values 128-180, divided by 128).
   - RTC KF boost: extra `100/128` multiplier.
   - Stats-based SB lambda modulation based on `qdiff` from base qindex:
     - Negative qdiff (lower quality SB): factor 90-115 (of 128).
     - Positive qdiff (higher quality SB): factor 135-150 (of 128).

**Fast lambda (SAD domain, `svt_aom_compute_fast_lambda`):**
1. Look up from precomputed tables: `av1_lambda_mode_decision8_bit_sad[qindex]` or 10-bit/12-bit variants.
2. Apply same `update_lambda()` adjustments.

**Lambda assignment (`svt_aom_lambda_assign`):**
1. Compute both fast and full lambda from qindex.
2. For 10-bit with `multiply_lambda`: scale full by 16, fast by 4.
3. Apply user `lambda_scale_factors[update_type]` (shifted by 7 bits).

### 10. Buffer Model and Overflow/Underflow Handling

**Leaky bucket model (CBR):**
- Each frame, buffer gains `avg_frame_bandwidth` bits and loses `encoded_frame_size` bits.
- Buffer clipped to `maximum_buffer_size`.
- When buffer drops below `optimal_buffer_level >> 3` (critical level): force `worst_quality`.
- When buffer exceeds optimal: allow lower quality (higher QP headroom).

**VBR drift correction:**
- `vbr_bits_off_target += base_frame_target - projected_frame_size`.
- Distributed over a 16-frame window (capped at 50% of target per frame).
- Fast pool (`vbr_bits_off_target_fast`) for massive undershoot: up to `4 * avg_frame_bandwidth`, distributed at `1/8` rate.

**Recode loop (`recode_loop_update_q`):**
Triggered when encoded frame size falls outside tolerance bounds:
1. Compute bounds: `frame_target +/- max(100, recode_tolerance% * target)`.
2. If overshoot:
   - Raise `q_low = q + 1`.
   - Early loops: use `get_regulated_q_overshoot()` (update rate correction, re-regulate Q).
   - Later loops: bisect `(q_low + q_high) / 2`.
3. If undershoot:
   - Lower `q_high = q - 1`.
   - Similar strategy with `get_regulated_q_undershoot()`.
4. Clamp to `[q_low, q_high]`.
5. Loop if Q changed.

**Capped CRF re-encode (`capped_crf_reencode`):**
- If `projected_frame_size > max_frame_size` and base layer:
  - Binary search for new Q that would produce target bits.
  - Update `active_worst_quality`.
- If undershoot: progressively reduce `active_worst_quality` (by 1/5 to 1/12).

**Rate correction factor update (`av1_rc_update_rate_correction_factors`):**
1. Estimate bits at current Q: `projected_size = enumerator * correction_factor / q * MBs`.
2. Compute correction: `actual_size / projected_size`.
3. Dampen adjustment: `adjustment_limit = 0.25 + K * min(1, |log10(correction)|)` where K depends on mode.
4. Apply damped correction to `rate_correction_factors[level]`.
5. Clamp to `[MIN_BPB_FACTOR=0.005, MAX_BPB_FACTOR=50]`.

### 11. Key Frame Detection and QP Assignment

Key frame QP follows a boost-based model:

**For CRF:**
- `kf_boost = (3|4) * (75 + 17*sqrt_clamp(frames_to_key)) / r0`, resolution-dependent.
- Active best quality from `get_kf_active_quality_tpl()`: interpolates between `kf_low_motion_minq` and `kf_high_motion_minq` tables based on boost vs. `[BOOST_KF_LOW=400, BOOST_KF_HIGH=5000]`.

**For VBR:**
- Same boost calculation. KF gets allocated `svt_av1_calculate_boost_bits(frame_count, boost, total_group_bits)`:
  ```
  allocation_chunks = frame_count * 100 + boost
  kf_bits = boost * total_group_bits / allocation_chunks
  ```
- Static KF group detection: if `kf_zeromotion_pct >= 99`, use `active_best_quality` directly.

**For CBR:**
- `kf_boost = DEFAULT_KF_BOOST_RT = 2300`.
- KF target size boosted relative to `avg_frame_bandwidth`, scaled by `kf_boost` and distance from last KF.
- Special handling for forced key frames: use `last_boosted_qindex`.
- Short intra period: average KF Q with previous frame Q.

**Capped CRF KF handling (`svt_aom_crf_assign_max_rate`):**
- Sliding window of `rate_average_periodin_frames` frames.
- `max_frame_size = calculate_boost_bits(kf_interval, kf_boost, available_bits_in_window)`.
- If `kf_boost` above threshold: inflate max by 40%.
- Adjust `active_worst_quality` based on bit budget ratio vs. frame budget ratio.

### 12. Rate Correction Factor Model

The bits-per-MB model is:
```
bits_per_mb = enumerator * correction_factor / q
```

where:
- `q = convert_qindex_to_q(qindex, bit_depth)` (AC quantizer scaled by bit depth).
- `enumerator`: 1,400,000 for KEY_FRAME, 1,000,000 for INTER_FRAME (750,000/1,000,000 for screen content).
- `correction_factor`: per-temporal-layer (VBR) or per-frame-type (CBR) adaptive factor.

The correction factor is stored per rate_factor_level and updated after each frame based on actual vs. predicted size. The model uses a size-dependent scaling to normalize for resolution changes:
```
rcf_normalized = rcf * (current_width * current_height) / (reference_width * reference_height)
```

### 13. MinQ Lookup Tables

The `rc_tables.h` file provides precomputed MinQ lookup tables indexed by qindex [0..255], for each bit depth (8/10/12):

| Table | Purpose |
|-------|---------|
| `kf_low_motion_minq_cqp` | KF quality floor for low-motion content |
| `kf_high_motion_minq` | KF quality floor for high-motion content |
| `arfgf_low_motion_minq` | GF/ARF quality floor for low-motion |
| `arfgf_high_motion_minq` | GF/ARF quality floor for high-motion |
| `inter_minq` | General inter frame quality floor |
| `rtc_minq` | RTC/CBR inter frame quality floor |

Quality interpolation between low and high motion tables:
```
if boost > high_threshold: return low_motion_minq[q]
if boost < low_threshold:  return high_motion_minq[q]
gap = high - low
offset = high - boost
qdiff = high_motion_minq[q] - low_motion_minq[q]
adjustment = (offset * qdiff + gap/2) / gap
return low_motion_minq[q] + adjustment
```

### 14. Dynamic Resize (CBR)

For CBR low-delay single-pass, `dynamic_resize_one_pass_cbr()`:

1. Track `resize_avg_qp` and `resize_buffer_underflow` over a window of `min(30, 2*framerate)` frames.
2. Resize down (3/4 or 1/2) if buffer underflowed >25% of window frames and frame size permits.
3. Resize up if average QP is below thresholds (50% or 70% of worst_quality).
4. On resize: `svt_av1_resize_reset_rc()` resets buffer to optimal, adjusts rate correction factors.

## Key Functions

### Pipeline Entry Points

| Function | Description |
|----------|-------------|
| `svt_aom_rate_control_kernel()` | Main RC thread loop. Handles `RC_INPUT` (QP assignment) and `RC_PACKETIZATION_FEEDBACK_RESULT` (post-encode update). |
| `svt_aom_initial_rate_control_kernel()` | Initial RC thread. Manages look-ahead queue, TPL group formation, one-pass VBR parameter synthesis. |

### QP Assignment

| Function | Description |
|----------|-------------|
| `svt_av1_rc_calc_qindex_crf_cqp()` | Top-level CRF/CQP qindex calculation with all adjustments. |
| `crf_qindex_calc()` | CRF qindex from r0-based qstep ratio or reference-frame interpolation. |
| `cqp_qindex_calc()` | CQP qindex from fixed offset tables or qstep-ratio. |
| `svt_av1_rc_process_rate_allocation()` | VBR/CBR frame-level bit budget computation. |
| `svt_av1_rc_calc_qindex_rate_control()` | VBR/CBR qindex from rate model + reference QP limits. |
| `rc_pick_q_and_bounds()` | VBR Q selection with active quality bounds. |
| `rc_pick_q_and_bounds_no_stats_cbr()` | CBR Q selection from buffer model. |
| `av1_rc_regulate_q()` | Binary search for Q matching target bits/MB. |

### Adaptive Quantization

| Function | Description |
|----------|-------------|
| `svt_av1_rc_init_sb_qindex()` | Orchestrates all SB-level QP adjustments. |
| `svt_av1_variance_adjust_qp()` | Variance Boost: lower QP for low-variance SBs. |
| `svt_aom_sb_qp_derivation_tpl_la()` | TPL-based SB QP: adjust by beta (intra/inter cost ratio). |
| `cyclic_sb_qp_derivation()` | Cyclic Refresh SB QP for CBR. |
| `svt_av1_normalize_sb_delta_q()` | Round SB qindex to delta_q_res multiples. |
| `generate_b64_me_qindex_map()` | ME distortion-based lambda modulation map. |

### Lambda

| Function | Description |
|----------|-------------|
| `svt_aom_compute_rd_mult()` | SSE-domain lambda from qindex, with frame-type and SB-level scaling. |
| `svt_aom_compute_fast_lambda()` | SAD-domain lambda from lookup table, with same scaling. |
| `svt_aom_compute_rd_mult_based_on_qindex()` | Base rdmult computation without SB-level adjustments. |
| `svt_aom_lambda_assign()` | Assigns both fast and full lambda, applies user scale factors. |

### Post-Encode

| Function | Description |
|----------|-------------|
| `svt_av1_rc_postencode_update()` | Update rate correction factors, buffer level, rolling averages. |
| `svt_av1_rc_postencode_update_gop_const()` | Same for GOP-constrained RC mode. |
| `svt_av1_twopass_postencode_update()` | VBR drift correction, extend_minq/maxq adjustment. |
| `svt_av1_twopass_postencode_update_gop_const()` | Same for GOP-constrained mode with per-param tracking. |
| `recode_loop_update_q()` | Recode decision and Q adjustment after trial encode. |
| `capped_crf_reencode()` | Capped CRF re-encode Q adjustment based on frame size vs. max. |
| `svt_av1_coded_frames_stat_calc()` | Sliding window bit tracking for capped CRF. |

### Utility

| Function | Description |
|----------|-------------|
| `svt_av1_convert_qindex_to_q()` | Convert qindex to Q value (AC quantizer / bit-depth scale). |
| `svt_av1_compute_qdelta()` | Compute qindex delta between two Q values. |
| `svt_av1_compute_qdelta_by_rate()` | Compute qindex delta for a rate target ratio. |
| `svt_av1_rc_bits_per_mb()` | Estimate bits per macroblock at given Q. |
| `svt_av1_get_q_index_from_qstep_ratio()` | Find qindex matching a qstep ratio relative to leaf qindex. |
| `svt_av1_calculate_boost_bits()` | Allocate boost bits to KF/GF from group budget. |
| `svt_av1_get_cqp_kf_boost_from_r0()` | Compute KF boost from r0. |
| `svt_av1_get_gfu_boost_from_r0_lap()` | Compute GFU boost from r0 with clamped factor. |
| `svt_av1_rc_init()` | Initialize RATE_CONTROL structure. |
| `svt_aom_cyclic_refresh_init()` | Initialize per-frame cyclic refresh parameters. |
| `svt_av1_compute_deltaq()` | Compute cyclic refresh delta-Q with clamping. |

## Dependencies

| Dependency | What It Provides |
|------------|-----------------|
| `pcs.h` / `PictureControlSet` | Per-picture state: qindex, SB array, reference info, temporal layer. |
| `sequence_control_set.h` | Sequence-level config: bit depth, resolution, RC mode, intra period. |
| `encoder.h` / `EncodeContext` | Global encoder state: `RATE_CONTROL`, `TWO_PASS`, `RateControlCfg`. |
| `firstpass.h` | `FIRSTPASS_STATS` structure and accumulation functions. |
| `entropy_coding.h` | Quantizer tables: `quantizer_to_qindex[]`, `svt_aom_dc_quant_qtx()`, `svt_aom_ac_quant_qtx()`. |
| `rd_cost.h` | `svt_aom_dc_quant_qtx()` for lambda computation. |
| `segmentation.h` | `svt_aom_setup_segmentation()` for CRF segmentation maps. |
| `src_ops_process.c` | `svt_aom_generate_r0beta()` for TPL r0/beta computation. |
| `resize.h` | `svt_aom_init_resize_picture()`, `coded_to_superres_mi()`. |
| `reference_object.h` | `EbReferenceObject` with per-reference quality and distortion data. |

## SIMD Functions

The rate control module itself contains no SIMD-accelerated functions. All computation is scalar (QP selection, rate modeling, buffer management). The performance-critical inner loops that feed into rate control (ME distortion computation, variance calculation, TPL processing) are SIMD-accelerated in their respective modules but are not part of the RC source files listed here.

The lambda lookup tables (`lambda_rate_tables.h`) are precomputed and accessed by direct indexing, requiring no SIMD.
