//! Rate control — CQP, CRF, VBR, CBR modes.
//!
//! Spec 09 (rate-control.md): CQP/CRF/VBR/CBR modes.
//!
//! Ported from SVT-AV1's `rc_process.c` and related files.
//!
//! **CRF ≡ CQP for a single still frame (empirically verified 2026-07-24).**
//! SVT-AV1's default / guide-recommended still mode is CRF (`--rc 0 --aq-mode 2`,
//! `--crf 35`), but the aq-mode-2 deltaq (`svt_aom_sb_qp_derivation_tpl_la`,
//! rc_aq.c:899) only fires under `tpl_ctrls.enable && r0 != 0` — i.e. it needs
//! TPL lookahead, which a single still frame has none of (`r0` inits to 0,
//! pcs.c:1299; no future frames raise it). Proven with the built C encoder:
//! `--qp N` == `--cqp N` == `--crf N`, byte-for-byte, across preset {0,8} × qp
//! {20,40,55} (see `benchmarks/crf_cqp_equivalence_2026-07-24.md`). So
//! `RcMode::Crf` being identical to `Cqp` here is **correct-by-design, not a
//! stub** — the port already emits SVT-AV1's default-CRF bytes at `qp = N`.
//! (aq-mode 1/2 segment/TPL VAQ and VBR/CBR bitrate-targeting are multi-frame or
//! degenerate for one still; the fork variance-boost is the only still-frame
//! deltaq that fires, and it is `enable_variance_boost` / tune-IQ gated.)

/// Rate control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcMode {
    /// Constant QP — fixed quantizer, no rate control.
    Cqp,
    /// Constant Rate Factor — quality-targeting.
    Crf,
    /// Variable Bit Rate — target average bitrate.
    Vbr,
    /// Constant Bit Rate — strict bitrate limit.
    Cbr,
}

/// Rate control configuration.
#[derive(Debug, Clone)]
pub struct RcConfig {
    pub mode: RcMode,
    /// CQP/CRF target quality in the CLI domain (0-63), identical to the
    /// C encoder's `--qp`. This is NOT an AV1 qindex: the pipeline maps
    /// it through [`QUANTIZER_TO_QINDEX`] exactly once at frame setup and
    /// everything downstream (quantizer tables, frame-header base_q_idx,
    /// CDF q bucket, deblock picker) operates on the resulting qindex.
    pub qp: u8,
    /// C `extended_crf_qindex_offset` (issue #9 item 4, FRACTIONAL CRF):
    /// the quarter-step remainder of a fractional `--crf`, in QINDEX units.
    /// `--crf 35.25` is `qp = 35, offset = 1`; `35.5` -> 2; `35.75` -> 3
    /// (`str_to_crf`, enc_settings.c:1662-1669). Consumed exactly where C
    /// consumes it — `scs_qindex = clamp_qindex(quantizer_to_qindex[qp] +
    /// extended_crf_qindex_offset)` (rc_crf_cqp.c:471), with `picture_qp`
    /// re-derived as `(base_q_idx + 2) >> 2` (rc_process.c:861). `0` (the
    /// default) is byte-identical to the integer-qp encode. Use
    /// [`RcConfig::crf`] to fill both fields from one `f32`.
    ///
    /// Valid: `0..=3` for `qp < 63`; C's extended range 63.25..70 maps to
    /// `qp == 63` with an offset up to 28 (`verify_settings`,
    /// enc_settings.c:270). Anything else is refused at encode time.
    pub extended_crf_qindex_offset: u8,
    /// Target bitrate in kbps (for VBR/CBR).
    pub target_bitrate: u32,
    /// Maximum bitrate in kbps (for VBR/CBR).
    pub max_bitrate: u32,
    /// Buffer size in ms.
    pub buffer_size_ms: u32,
    /// Framerate for bitrate calculations.
    pub framerate: f64,
    /// Number of temporal layers.
    pub temporal_layers: u8,
    /// Adaptive-quantization mode, mirroring the C encoder's `--aq-mode`
    /// semantics for the frame-level decision: 0 = OFF (CQP is a straight
    /// `quantizer_to_qindex[qp]` lookup with NO content-adaptive QP shift
    /// — the C default for `--rc 0 --aq-mode 0` matched configs), non-zero
    /// = enable the Rust frame-level VAQ/TPL QP adjustments (a homegrown
    /// heuristic, NOT a port of C's aq-mode 1/2 segment-based VAQ).
    pub aq_mode: u8,
}

impl RcConfig {
    /// A CRF/CQP config from a FRACTIONAL `--crf` value, exactly as C's
    /// `str_to_crf` (enc_settings.c:1655-1670) splits it: `qp = min(63,
    /// trunc(crf))`, `extended_crf_qindex_offset = trunc(crf * 4) - qp * 4`
    /// (quarter-qindex steps; 0..=3 below 63, up to 28 for the extended
    /// 63.25..70 range). Negative or NaN input is treated as 0.0, > 70 is
    /// clamped to 70 (C rejects those at `verify_settings`). Every other
    /// field is the default.
    pub fn crf(crf: f32) -> Self {
        let crf = if crf.is_nan() {
            0.0
        } else {
            crf.clamp(0.0, 70.0)
        };
        let extended_q_index = (crf * 4.0) as u32;
        let qp = (crf as u32).min(63);
        Self {
            mode: RcMode::Crf,
            qp: qp as u8,
            extended_crf_qindex_offset: (extended_q_index - qp * 4) as u8,
            ..Self::default()
        }
    }
}

impl Default for RcConfig {
    fn default() -> Self {
        Self {
            mode: RcMode::Crf,
            // C `DEFAULT_QP` (Source/Lib/Globals/enc_settings.h:22) — the
            // value `svt_av1_set_default_params` installs (enc_settings.c:1007)
            // and what `SvtAv1EncApp` encodes with when the caller passes no
            // --qp/--crf. The port previously defaulted to 30 with no cited
            // provenance.
            qp: 35,
            extended_crf_qindex_offset: 0,
            target_bitrate: 0,
            max_bitrate: 0,
            buffer_size_ms: 1000,
            // NOTE: C defaults to 60000/1000 = 60 fps (enc_settings.c:993-994),
            // but the byte-parity oracle pins 30/1 (capture_c_trace.c) and the
            // frame rate is only observable through the auto-derived
            // `seq_level_idx`. Keeping 30.0 keeps the port and the oracle on
            // the same matched config; a caller targeting C's own default sets
            // this to 60.0 explicitly.
            framerate: 30.0,
            temporal_layers: 1,
            aq_mode: 0,
        }
    }
}

/// Per-picture rate control state.
#[derive(Debug, Clone)]
pub struct RcState {
    /// Current QP assigned to this picture.
    pub qp: u8,
    /// Lambda value for RDO.
    pub lambda: f64,
    /// Accumulated bits in the VBV buffer.
    pub buffer_fullness: i64,
    /// Total bits encoded so far.
    pub total_bits: u64,
    /// Total frames encoded so far.
    pub total_frames: u64,
}

impl Default for RcState {
    fn default() -> Self {
        Self {
            qp: 30,
            lambda: 0.0,
            buffer_fullness: 0,
            total_bits: 0,
            total_frames: 0,
        }
    }
}

/// QP delta offsets for temporal layers.
/// Layer 0 (base) gets the base QP, higher layers get increased QP.
pub const TEMPORAL_LAYER_QP_DELTA: [i8; 6] = [0, 4, 8, 10, 12, 12];

/// CLI-QP (0..63) to AV1 qindex (0..255) mapping.
///
/// Verbatim port of C SVT-AV1 `quantizer_to_qindex[64]`
/// (Source/Lib/Codec/md_process.c:20, declared md_process.h:1396,
/// baseline v4.2.0-rc). C's `--qp` is 0..63 and is mapped through this
/// table before ANY internal use — quantizer step tables, frame-header
/// base_q_idx, default-CDF q bucket, deblock level picker all operate on
/// the resulting qindex. Entries are `4*qp` for qp <= 61, then 249, 255;
/// max 255 fits u8 exactly like the C uint8_t table.
pub const QUANTIZER_TO_QINDEX: [u8; 64] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, //
    64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108, 112, 116, 120, 124, //
    128, 132, 136, 140, 144, 148, 152, 156, 160, 164, 168, 172, 176, 180, 184, 188, //
    192, 196, 200, 204, 208, 212, 216, 220, 224, 228, 232, 236, 240, 244, 249, 255,
];

/// Convert a CLI-domain QP (0..63, C `--qp` semantics) to the AV1 qindex
/// (0..255) via [`QUANTIZER_TO_QINDEX`]. Inputs > 63 are clamped to 63
/// (the CLI boundary clamp — the only place the 0..63 range is enforced).
pub fn qp_to_qindex(qp: u8) -> u8 {
    QUANTIZER_TO_QINDEX[qp.min(63) as usize]
}

/// C `svt_av1_rc_calc_qindex_crf_cqp` (rc_crf_cqp.c:471-513) for a still:
/// `scs_qindex = clamp_qindex(quantizer_to_qindex[qp] +
/// extended_crf_qindex_offset)`; `cqp_qindex_calc` returns it unchanged on
/// `allintra` (:396-398); then the extended-CRF-range compression
/// `new_qindex += (MAXQ - new_qindex) * offset / 56` fires only at
/// `qp == MAX_QP_VALUE (63)` (:510-512). `clamp_qindex` clamps to the
/// qindexes of `min_qp_allowed..max_qp_allowed`, whose C defaults are 0..63
/// -> 0..255, i.e. a `u8` saturation here. With `offset == 0` this is
/// exactly [`qp_to_qindex`], so every integer-qp encode is unchanged.
///
/// The `qp == 63` arm is inert with the default qp clamps — 255 + anything
/// saturates to 255 and `(255 - 255) * k / 56 == 0` — and is kept because
/// dead-looking C stays translated (rust/CLAUDE.md).
pub fn qp_to_qindex_with_offset(qp: u8, extended_crf_qindex_offset: u8) -> u8 {
    let qp = qp.min(63);
    let mut q = (u32::from(QUANTIZER_TO_QINDEX[qp as usize])
        + u32::from(extended_crf_qindex_offset))
    .min(255);
    if qp == 63 && extended_crf_qindex_offset != 0 {
        q += (255 - q) * u32::from(extended_crf_qindex_offset) / 56;
        q = q.min(255);
    }
    q as u8
}

/// C `picture_qp` after rate control: `clamp_qp((base_q_idx + 2) >> 2)`
/// (rc_process.c:861). For every qindex [`qp_to_qindex`] produces this is
/// the exact inverse (`(4n + 2) >> 2 == n`, `251 >> 2 == 62`, `257 >> 2 ==
/// 64 -> 63`); for a fractional-CRF qindex it ROUNDS, which is what C's
/// CLI-domain consumers (lambda, the `picture_qp`-keyed level derivations)
/// then see.
pub fn picture_qp_from_qindex(qindex: u8) -> u8 {
    ((u32::from(qindex) + 2) >> 2).min(63) as u8
}

/// Inverse of [`qp_to_qindex`]: recover the CLI-domain QP (0..63) from a
/// qindex. `qindex >> 2` is the EXACT inverse for every value the table
/// produces (`4n >> 2 == n` for n <= 61, `249 >> 2 == 62`,
/// `255 >> 2 == 63`); for intermediate qindexes (future qindex-domain
/// deltas) it is the floor approximation. Used only to derive the interim
/// CLI-qp-scale lambda until C's qindex-driven lambda tables
/// (`lambda_rate_tables.h`) are ported.
pub fn qindex_to_qp(qindex: u8) -> u8 {
    qindex >> 2
}

/// Compute lambda from CLI-domain QP (0..63) for rate-distortion
/// optimization.
///
/// Lambda controls the tradeoff between distortion and rate.
/// Higher QP → higher lambda → accept more distortion to save bits.
///
/// DOMAIN NOTE: this HEVC-style closed form (`0.85 * 2^((qp-12)/3)`) is
/// calibrated for the CLI 0..63 scale — feeding a qindex (0..255) would
/// blow lambda up to ~2^80 and turn every RD decision into "cheapest
/// rate wins". Qindex-domain call sites must convert with
/// [`qindex_to_qp`] first. C instead derives lambda from qindex via
/// dedicated tables (`lambda_rate_tables.h`, av1_compute_rd_mult path);
/// porting those is a separate chunk — until then lambda intentionally
/// stays CLI-qp-driven and deterministic.
pub fn qp_to_lambda(qp: u8) -> f64 {
    let q = qp as f64;
    0.85 * 2.0_f64.powf((q - 12.0) / 3.0)
}

/// Assign QP for a picture based on its temporal layer and RC state.
///
/// Operates ENTIRELY in the CLI QP domain (0..63), like C's picture_qp:
/// hierarchical/temporal-layer deltas apply here, and the 0..63 clamps in
/// each arm are the CLI boundary clamp. The pipeline converts the result
/// to qindex via [`qp_to_qindex`] exactly once afterwards.
// `clippy::manual_checked_ops` post-dates the 1.89 MSRV floor's clippy, so the
// allow has to tolerate being unknown there (`cargo +1.89 clippy` otherwise
// reports `unknown lint` at this line).
#[allow(unknown_lints, clippy::manual_checked_ops)] // the `> 0` guard scopes a whole block, not a single
// division; `checked_div` cannot express it without restructuring hot RD control flow
pub fn assign_picture_qp(config: &RcConfig, state: &RcState, temporal_layer: u8) -> u8 {
    match config.mode {
        RcMode::Cqp => {
            // CQP: fixed QP + temporal layer offset
            let delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (config.qp as i16 + delta as i16).clamp(0, 63) as u8
        }
        RcMode::Crf => {
            // CRF: target quality with temporal offset
            let delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (config.qp as i16 + delta as i16).clamp(0, 63) as u8
        }
        RcMode::Vbr | RcMode::Cbr => {
            // VBR/CBR: adjust QP based on buffer fullness
            let target_bits_per_frame =
                (config.target_bitrate as f64 * 1000.0 / config.framerate) as i64;
            let avg_bits = if state.total_frames > 0 {
                (state.total_bits / state.total_frames) as i64
            } else {
                target_bits_per_frame
            };

            let delta = if avg_bits > target_bits_per_frame {
                // Over budget → increase QP
                1i8
            } else if avg_bits < target_bits_per_frame * 3 / 4 {
                // Under budget → decrease QP
                -1
            } else {
                0
            };

            let layer_delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (state.qp as i16 + delta as i16 + layer_delta as i16).clamp(0, 63) as u8
        }
    }
}

/// Temporal complexity estimation for TPL-like QP adjustment.
///
/// Computes the average SAD between the current frame and the reference.
/// Returns a QP adjustment: positive for complex (high-motion) frames,
/// negative for simple (static) frames. This implements a simplified
/// TPL that distributes bits based on temporal prediction difficulty.
///
/// DOMAIN NOTE: the returned delta is in CLI QP units (its ±2/±4
/// magnitudes were chosen on the 0..63 scale). It is applied to the
/// CLI-domain picture QP BEFORE the single qp→qindex conversion, so one
/// CLI step becomes ~4 qindex steps through the table — the sensible
/// qindex-domain effect without re-tuning the constants.
pub fn tpl_qp_adjustment(
    source: &[u8],
    reference: &[u8],
    width: usize,
    height: usize,
    src_stride: usize,
) -> i8 {
    if source.len() < width * height || reference.len() < width * height {
        return 0;
    }

    // Compute frame-level SAD (sum of absolute differences)
    let mut sad: u64 = 0;
    let n = width * height;
    for r in 0..height {
        for c in 0..width {
            let s = source[r * src_stride + c] as i32;
            let ref_val = reference[r * width + c] as i32;
            sad += (s - ref_val).unsigned_abs() as u64;
        }
    }

    let avg_sad = sad / n as u64;

    // Map average SAD to QP adjustment:
    // SAD < 2: very static → lower QP by 4 (spend more bits = better quality)
    // SAD 2-8: moderate → no adjustment
    // SAD 8-20: active → raise QP by 2 (save bits for key frames)
    // SAD > 20: high motion → raise QP by 4
    match avg_sad {
        0..=1 => -4,
        2..=4 => -2,
        5..=8 => 0,
        9..=20 => 2,
        _ => 4,
    }
}

/// Compute per-SB QP offsets based on spatial + temporal complexity.
///
/// Returns a flat array of QP deltas (one per SB in raster order).
/// Positive deltas = more complex = higher QP. Negative = simpler = lower QP.
///
/// DOMAIN NOTE: deltas are CLI-QP-scale (±2/±4). Currently unused by the
/// pipeline (per-SB delta_q signaling is not ported); when delta_q lands
/// these must be converted to qindex units (AV1 signals delta_q_res
/// steps of qindex), not applied to the CLI qp.
pub fn tpl_sb_qp_offsets(
    source: &[u8],
    reference: &[u8],
    width: usize,
    height: usize,
    src_stride: usize,
    sb_size: usize,
) -> alloc::vec::Vec<i8> {
    let sb_cols = width.div_ceil(sb_size);
    let sb_rows = height.div_ceil(sb_size);
    let mut offsets = alloc::vec![0i8; sb_cols * sb_rows];

    for sb_row in 0..sb_rows {
        for sb_col in 0..sb_cols {
            let x0 = sb_col * sb_size;
            let y0 = sb_row * sb_size;
            let cur_w = sb_size.min(width - x0);
            let cur_h = sb_size.min(height - y0);

            // Compute SB-level SAD
            let mut sad: u64 = 0;
            for r in 0..cur_h {
                for c in 0..cur_w {
                    let s = source[(y0 + r) * src_stride + x0 + c] as i32;
                    let ref_val = reference[(y0 + r) * width + x0 + c] as i32;
                    sad += (s - ref_val).unsigned_abs() as u64;
                }
            }
            let avg = sad / (cur_w * cur_h) as u64;

            offsets[sb_row * sb_cols + sb_col] = match avg {
                0..=2 => -2,
                3..=10 => 0,
                11..=25 => 2,
                _ => 4,
            };
        }
    }
    offsets
}

/// Update RC state after encoding a picture.
/// C `svt_aom_dc_quant_qtx(qindex, 0, bit_depth)` — the DC quantizer step.
///
/// The tables are the port's existing transcriptions (`DC_QLOOKUP_8`, and
/// `DC_QLOOKUP_10` for bd10); this is only the depth selection C's macro does.
fn dc_quant_qtx(qindex: i32, bit_depth: u8) -> f64 {
    let i = qindex.clamp(0, 255) as usize;
    match bit_depth {
        8 => f64::from(svtav1_dsp::quant_tables::DC_QLOOKUP_8[i]),
        10 => f64::from(crate::bd10::DC_QLOOKUP_10[i]),
        other => panic!("dc_quant_qtx: unsupported bit depth {other}"),
    }
}

/// C `svt_av1_get_q_index_from_qstep_ratio` (rc_process.c:322).
///
/// Walks the qindex ladder from `leaf_qindex` until the DC quantizer step
/// crosses `leaf_qstep * qstep_ratio` — down when the ratio is < 1 (a finer
/// quantizer, i.e. a better frame), up otherwise. The linear walk is C's own;
/// a binary search would be equivalent on a monotone table but this stays a
/// transcription, and the table's monotonicity is not something this function
/// should be asserting on C's behalf.
#[must_use]
pub fn q_index_from_qstep_ratio(leaf_qindex: i32, qstep_ratio: f64, bit_depth: u8) -> i32 {
    const MINQ: i32 = 0;
    const MAXQ: i32 = 255;
    let leaf_qstep = dc_quant_qtx(leaf_qindex, bit_depth);
    let target_qstep = leaf_qstep * qstep_ratio;
    let mut qindex;
    if qstep_ratio < 1.0 {
        qindex = leaf_qindex;
        while qindex > MINQ {
            if dc_quant_qtx(qindex, bit_depth) <= target_qstep {
                break;
            }
            qindex -= 1;
        }
    } else {
        qindex = leaf_qindex;
        while qindex <= MAXQ {
            if dc_quant_qtx(qindex, bit_depth) >= target_qstep {
                break;
            }
            qindex += 1;
        }
    }
    qindex
}

/// C `SVT_QP_SCALE_WEIGHT` (definitions.h:249) for the mainline build:
/// `1.000 + qp_scale_compress_strength * 0.125`.
#[must_use]
pub fn qp_scale_weight(qp_scale_compress_strength: f64) -> f64 {
    1.0 + qp_scale_compress_strength * 0.125
}

/// C `cqp_qindex_calc` (rc_crf_cqp.c:393) — the `TUNE_CQP_CHROMA_SSIM = 1`
/// branch, which is the one v4.2.0 compiles (`EbDebugMacros.h:68`).
///
/// **This is the still-vs-video fork in the encode, and it is why a video-mode
/// key frame is not the still key frame this port already emits byte-
/// identically.** C returns `qindex` untouched when `scs->allintra` — the
/// entire existing 280/280 still envelope takes that early return — and only a
/// VIDEO-mode encode reaches the qstep-ratio scaling below. Measured on
/// `gradient 64x64 q40 p6`: 290 bytes still vs 930 bytes video, same pixels
/// (docs/INTER-ENCODE-PLAN.md §1b).
///
/// `is_ref` / `temporal_layer_index` / `hierarchical_levels` come from the GOP;
/// `cqp_base_q` is C's `scs->cqp_base_q`, written by the temporal-layer-0 arm
/// and read by the arf arm, so the caller owns it across frames.
#[must_use]
pub fn cqp_qindex_calc(
    qindex: i32,
    allintra: bool,
    slice_is_intra: bool,
    is_ref: bool,
    temporal_layer_index: u8,
    hierarchical_levels: u8,
    bit_depth: u8,
    qp_scale_compress_strength: f64,
    cqp_base_q: &mut i32,
) -> i32 {
    if allintra {
        return qindex;
    }
    if hierarchical_levels == 0 && !slice_is_intra {
        return qindex;
    }
    const MAXQ: f64 = 255.0;
    let active_worst_quality = qindex;
    if temporal_layer_index == 0 {
        let qratio_grad = if hierarchical_levels <= 4 { 0.3 } else { 0.2 };
        let qstep_ratio = (0.2 + (1.0 - f64::from(active_worst_quality) / MAXQ) * qratio_grad)
            * qp_scale_weight(qp_scale_compress_strength);
        let q = q_index_from_qstep_ratio(active_worst_quality, qstep_ratio, bit_depth);
        *cqp_base_q = q;
        q
    } else if is_ref && temporal_layer_index < hierarchical_levels {
        // C walks the arf ladder from `scs->cqp_base_q` toward the worst
        // quality once per temporal height.
        let mut this_height = i32::from(temporal_layer_index) + 1;
        let mut arf_q = *cqp_base_q;
        while this_height > 1 {
            arf_q = (arf_q + active_worst_quality + 1) / 2;
            this_height -= 1;
        }
        arf_q
    } else {
        active_worst_quality
    }
}

pub fn update_rc_state(state: &mut RcState, bits_used: u64, new_qp: u8) {
    state.total_bits += bits_used;
    state.total_frames += 1;
    state.qp = new_qp;
    state.lambda = qp_to_lambda(new_qp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cqp_constant_base_qp() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..Default::default()
        };
        let state = RcState::default();
        let qp = assign_picture_qp(&config, &state, 0);
        assert_eq!(qp, 30);
    }

    #[test]
    fn cqp_temporal_layer_offset() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..Default::default()
        };
        let state = RcState::default();
        let qp0 = assign_picture_qp(&config, &state, 0);
        let qp1 = assign_picture_qp(&config, &state, 1);
        let qp2 = assign_picture_qp(&config, &state, 2);
        assert!(qp0 < qp1);
        assert!(qp1 < qp2);
    }

    #[test]
    fn qp_to_lambda_monotonic() {
        let l1 = qp_to_lambda(20);
        let l2 = qp_to_lambda(30);
        let l3 = qp_to_lambda(40);
        assert!(l1 < l2);
        assert!(l2 < l3);
    }

    #[test]
    fn update_state() {
        let mut state = RcState::default();
        update_rc_state(&mut state, 1000, 32);
        assert_eq!(state.total_bits, 1000);
        assert_eq!(state.total_frames, 1);
        assert_eq!(state.qp, 32);
        assert!(state.lambda > 0.0);
    }

    #[test]
    fn qp_clamping() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 62,
            ..Default::default()
        };
        let state = RcState::default();
        // Layer 2 delta = 8, so 62 + 8 = 70 → clamped to 63
        let qp = assign_picture_qp(&config, &state, 2);
        assert_eq!(qp, 63);
    }

    /// Spot-check the C table endpoints and the non-linear tail
    /// (md_process.c:20: ..., 240, 244, 249, 255).
    #[test]
    fn quantizer_to_qindex_matches_c() {
        assert_eq!(QUANTIZER_TO_QINDEX[0], 0);
        assert_eq!(QUANTIZER_TO_QINDEX[1], 4);
        assert_eq!(QUANTIZER_TO_QINDEX[20], 80);
        assert_eq!(QUANTIZER_TO_QINDEX[32], 128);
        assert_eq!(QUANTIZER_TO_QINDEX[40], 160);
        assert_eq!(QUANTIZER_TO_QINDEX[55], 220);
        assert_eq!(QUANTIZER_TO_QINDEX[60], 240);
        assert_eq!(QUANTIZER_TO_QINDEX[61], 244);
        assert_eq!(QUANTIZER_TO_QINDEX[62], 249);
        assert_eq!(QUANTIZER_TO_QINDEX[63], 255);
        // 4*qp for the linear region.
        for qp in 0..=61u8 {
            assert_eq!(QUANTIZER_TO_QINDEX[qp as usize], 4 * qp);
        }
        // Strictly monotonic over the whole range.
        for qp in 1..64usize {
            assert!(QUANTIZER_TO_QINDEX[qp] > QUANTIZER_TO_QINDEX[qp - 1]);
        }
    }

    #[test]
    fn qp_qindex_round_trip() {
        for qp in 0..=63u8 {
            assert_eq!(qindex_to_qp(qp_to_qindex(qp)), qp, "round trip at qp {qp}");
        }
        // CLI boundary clamp: out-of-range CLI qp saturates to 63 → 255.
        assert_eq!(qp_to_qindex(90), 255);
        assert_eq!(qp_to_qindex(255), 255);
    }
}
