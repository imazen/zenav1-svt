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

/// C `svt_av1_qp_scale_compress_weight` (rc_process.c:48) — the MAINLINE
/// `SVT_QP_SCALE_WEIGHT` table (definitions.h:252), indexed by the uint8
/// `qp_scale_compress_strength` knob. The svt-av1-hdr fork replaces it
/// with `1.000 + strength * 0.125` on a `double` field
/// (definitions.h:249, under `SVT_HDR_MODE`) — callers pick the arm on
/// `hdr.is_fork()`.
pub const QP_SCALE_COMPRESS_WEIGHT: [f64; 4] = [1.0, 1.125, 1.25, 1.375];

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
pub fn assign_picture_qp(config: &RcConfig, _state: &RcState, temporal_layer: u8) -> u8 {
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
            // C `scs->static_config.qp` is IMMUTABLE under VBR/CBR — rate
            // control moves `ppcs->picture_qp` and `frm_hdr.base_q_idx`, never
            // the CLI-domain qp that every qp-keyed derivation (PD0 level
            // bands, coeff-level ladders, NSQ geometry thresholds) reads.
            // The ported driver's CBR qindex lives on `base_qindex`; letting
            // the legacy `state.qp` ramp leak into `pcs.qp` resolved
            // `pic_pd0_lvl` 5 (LVL_5's closed form) where C at the same CLI
            // qp resolves 4 (the real estimator) — measured on the LD+CBR
            // key frame of `g5 72x88 q40 p8` (port PD0 pc=2242107 vs C
            // pc=6411087 at org=(16,0) 16x16).
            config.qp
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

/// C `SVT_QP_SCALE_WEIGHT`. There are TWO definitions and this doc originally
/// cited the wrong one: definitions.h:249 is the **fork** (`SVT_HDR_MODE`)
/// form, `1.000 + qp_scale_compress_strength * 0.125`; **mainline** is :252,
/// a table lookup `svt_av1_qp_scale_compress_weight[strength]` with the table
/// `{1, 1.125, 1.25, 1.375}` (rc_process.c:48).
///
/// They agree exactly on the whole domain the CLI can produce — the table is
/// `1 + i * 0.125` for i in 0..=3 — so this function is correct for both
/// builds. The citation is corrected anyway, because "it happens to agree" and
/// "it is the same rule" are different claims and only the first one is true.
///
/// NOT YET CONFIRMED as the weight this path actually applies: see the
/// measured open question on [`cqp_qindex_calc`].
#[must_use]
pub fn qp_scale_weight(qp_scale_compress_strength: f64) -> f64 {
    1.0 + qp_scale_compress_strength * 0.125
}

/// C `svt_av1_convert_qindex_to_q` (rc_process.c:186): the AC quantizer step
/// scaled down to the "old Q" domain — /4 at 8-bit, /16 at 10-bit.
#[must_use]
pub fn convert_qindex_to_q(qindex: i32, bit_depth: u8) -> f64 {
    let i = qindex.clamp(0, 255) as usize;
    match bit_depth {
        8 => f64::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[i]) / 4.0,
        10 => f64::from(crate::bd10::AC_QLOOKUP_10[i]) / 16.0,
        other => panic!("convert_qindex_to_q: unsupported bit depth {other}"),
    }
}

/// C `svt_av1_compute_qdelta` (rc_process.c:201): map two q values back to
/// qindices by walking the ladder from MINQ and return their difference.
///
/// The linear walk and the `>= ` comparison are C's; both loops stop at
/// `MAXQ - 1` and leave the index at the last value tried, which is why a
/// qstart above the top of the table yields 254 rather than 255.
#[must_use]
pub fn compute_qdelta(qstart: f64, qtarget: f64, bit_depth: u8) -> i32 {
    const MINQ: i32 = 0;
    const MAXQ: i32 = 255;
    let mut start_index = MAXQ;
    let mut target_index = MAXQ;
    for i in MINQ..MAXQ {
        start_index = i;
        if convert_qindex_to_q(i, bit_depth) >= qstart {
            break;
        }
    }
    for i in MINQ..MAXQ {
        target_index = i;
        if convert_qindex_to_q(i, bit_depth) >= qtarget {
            break;
        }
    }
    target_index - start_index
}

/// C `percents[2][FIXED_QP_OFFSET_COUNT]` (md_process.c:25), the libaom
/// offsets. **The first index is the BOOLEAN `hierarchical_levels <= 4`**, so
/// row 1 is the shallow-GOP row — easy to invert, and inverting it moves a
/// key frame's qindex by 3.
const QP_OFFSET_PERCENTS: [[i32; 6]; 2] = [[75, 70, 60, 20, 15, 0], [76, 60, 30, 15, 8, 4]];

/// C `cqp_qindex_calc` (rc_crf_cqp.c:393) — the **mainline** arm.
///
/// WHICH ARM, and why it is easy to get wrong: the function has
/// `#if TUNE_CQP_CHROMA_SSIM` / `#else` halves, and `TUNE_CQP_CHROMA_SSIM` is
/// 1 **only under `SVT_HDR_MODE`** (`EbDebugMacros.h:64-71`). Mainline v4.2.0
/// compiles the `#else`, which is what this port implements. Reading the `#if`
/// block and assuming it is live produces a function that is internally
/// consistent, passes a differential against the exported ladder primitive,
/// and still returns the wrong qindex — which is exactly what happened here
/// before the macro was checked.
///
/// VERIFIED against the real C encoder's written `base_q_idx`, 64x64 gradient
/// in video mode (`SVT_AVIF=0`), all four cells exact:
///
/// | cell | C | this fn |
/// |---|--:|--:|
/// | qp20 (qindex 80), hierarchical_levels 0 | 14 | 14 |
/// | qp40 (qindex 160), hier 0 | 67 | 67 |
/// | qp40 (qindex 160), hier 5 | 70 | 70 |
/// | qp55 (qindex 220), hier 0 | 143 | 143 |
///
/// Inputs C's `crf_qindex_calc` (rc_crf_cqp.c:183-360) reads off
/// pcs/ppcs/rc/scs — one-pass CRF/TPL qindex derivation.
#[derive(Debug, Clone)]
pub struct CrfQindexInputs {
    /// C `frame_is_intra_only(ppcs)`.
    pub is_intra_only: bool,
    /// C `ppcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `ppcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `ppcs->is_highest_layer` — `leaf_frame`.
    pub is_highest_layer: bool,
    /// C `ppcs->r0_qps` — the qstep-vs-ref-frame dispatch.
    pub r0_qps: bool,
    /// C `ppcs->r0` on entry; the adjusted value comes back in the output.
    pub r0: f64,
    /// C `ppcs->tpl_ctrls.r0_adjust_factor` (0 disables).
    pub r0_adjust_factor: f64,
    /// C `ppcs->used_tpl_frame_num`.
    pub used_tpl_frame_num: u32,
    /// C `ppcs->tpl_group_size`.
    pub tpl_group_size: u32,
    /// C `scs->lad_mg != 0`.
    pub scs_lad_mg: bool,
    /// C `scs->input_resolution` (`ResolutionRange`) — `<= INPUT_SIZE_720p_RANGE`
    /// selects the 3x boost numerator.
    pub input_resolution: i32,
    /// C `scs->static_config.encoder_bit_depth`.
    pub bit_depth: u8,
    /// C `ppcs->sc_class1`.
    pub sc_class1: bool,
    /// C `SVT_QP_SCALE_WEIGHT(static_config)`.
    pub qp_scale_weight: f64,
    /// C `SVT_QP_SCALE_ON(static_config)`.
    pub qp_scale_on: bool,
    /// C `rc->best_quality` / `rc->worst_quality` —
    /// `quantizer_to_qindex[min/max_qp_allowed]`.
    pub best_quality: i32,
    /// See [`CrfQindexInputs::best_quality`].
    pub worst_quality: i32,
    /// C `rc->arf_q` on entry = `ref_base_q_idx[L0][0]`, max'd with L1[0] for a
    /// B slice with `ref_list1_count_try`.
    pub arf_q: i32,
    /// C `ref_obj_l0->tmp_layer_idx` for the `is_intrl_arf_boost` arm.
    pub ref0_tmp_layer: u8,
    /// C `ref_obj_l1->tmp_layer_idx` when `slice_type == B_SLICE &&
    /// ref_list1_count_try` — `None` otherwise.
    pub ref1_tmp_layer: Option<u8>,
    /// C `pcs->ref_intra_percentage` (hier-5 w1 bump).
    pub ref_intra_percentage: i32,
}

/// Outputs of [`crf_qindex_calc`] — C's return value plus the `rc`/`ppcs`
/// state it mutates.
#[derive(Debug, Clone, Copy)]
pub struct CrfQindexOutput {
    /// C's return — the frame's `active_best_quality`.
    pub qindex: i32,
    /// C `ppcs->top_index`.
    pub top_index: i32,
    /// C `ppcs->bottom_index`.
    pub bottom_index: i32,
    /// C `ppcs->r0` after the in-place adjustments.
    pub r0: f64,
    /// C `rc->arf_q` after the qstep arm's update.
    pub arf_q: i32,
    /// C `rc->kf_boost` (intra arm only, else 0).
    pub kf_boost: i32,
    /// C `rc->gfu_boost` (inter arm only, else 0).
    pub gfu_boost: i32,
}

/// C `crf_qindex_calc` (rc_crf_cqp.c:183-360) — one-pass qindex assignment
/// with TPL stats. `qindex` is C's `rc->active_worst_quality` argument.
#[must_use]
pub fn crf_qindex_calc(qindex: i32, i: &CrfQindexInputs) -> CrfQindexOutput {
    use crate::port_rc_process as p;
    let cq_level = qindex;
    // C declares `int32_t active_best_quality` uninitialized; both dispatch
    // arms assign it before use, so it needs no dead initializer here.
    let mut active_best_quality: i32;
    let mut active_worst_quality: i32 = qindex;
    let temporal_layer = i.temporal_layer_index;
    let hierarchical_levels = i.hierarchical_levels as usize;
    let leaf_frame = i.is_highest_layer;
    let is_intrl_arf_boost = temporal_layer > 0 && !leaf_frame;
    let rf_level = if i.is_intra_only {
        p::RateFactorLevel::KfStd
    } else if temporal_layer == 0 {
        p::RateFactorLevel::GfArfStd
    } else if !leaf_frame {
        p::RateFactorLevel::GfArfLow
    } else {
        p::RateFactorLevel::InterNormal
    };
    let bit_depth = i.bit_depth;
    let use_qstep_based_q_calc = i.r0_qps;
    let mut arf_q = i.arf_q;
    let mut r0 = i.r0;
    let mut kf_boost = 0i32;
    let mut gfu_boost = 0i32;

    // r0 scaling (rc_crf_cqp.c:232-279).
    if i.is_intra_only {
        if i.r0_adjust_factor != 0.0 {
            r0 /= i.r0_adjust_factor;
        }
        r0 /= p::TPL_HL_ISLICE_DIV_FACTOR[hierarchical_levels];
        // `frames_to_key == -1` — not available in one-pass.
        kf_boost = p::get_cqp_kf_boost_from_r0(r0, -1, i.input_resolution);
        let max_boost = (i.used_tpl_frame_num * 400) as i32; // KB = 400
        kf_boost = kf_boost.min(max_boost);
    } else {
        if use_qstep_based_q_calc && i.r0_adjust_factor != 0.0 {
            r0 /= i.r0_adjust_factor;
            r0 /= p::TPL_HL_BASE_FRAME_DIV_FACTOR[hierarchical_levels];
        }
        let num_stats_required_for_gfu_boost = i.tpl_group_size + (1u32 << hierarchical_levels);
        let mut min_boost_factor = (1f64) * f64::from(1u32 << (hierarchical_levels >> 1));
        if hierarchical_levels & 1 != 0 {
            min_boost_factor *= core::f64::consts::SQRT_2;
        }
        gfu_boost = p::get_gfu_boost_from_r0_lap(
            min_boost_factor,
            10.0, // MAX_GFUBOOST_FACTOR
            r0,
            num_stats_required_for_gfu_boost as i32,
        );
    }

    if use_qstep_based_q_calc {
        let r0_weight_idx = usize::from(!i.is_intra_only) + usize::from(temporal_layer != 0);
        debug_assert!(r0_weight_idx <= 2);
        let mut weight = p::R0_WEIGHT[r0_weight_idx];
        if i.scs_lad_mg && !i.is_intra_only && i.tpl_group_size < (2u32 << hierarchical_levels) {
            weight = (weight + 0.1).min(1.0);
        }
        let mut qstep_ratio = r0.sqrt() * weight * i.qp_scale_weight;
        if i.qp_scale_on {
            qstep_ratio = weight.min(qstep_ratio);
        }
        let qindex_from_qstep_ratio = q_index_from_qstep_ratio(qindex, qstep_ratio, bit_depth);
        #[cfg(feature = "std")]
        if std::env::var_os("SVTAV1_TPLDBG").is_some() {
            std::eprintln!(
                "TPLDBG intra={} tl={} r0={} weight={} qstep_ratio={} qstep_q={} qindex={}",
                i.is_intra_only,
                temporal_layer,
                r0,
                weight,
                qstep_ratio,
                qindex_from_qstep_ratio,
                qindex,
            );
        }
        if !i.is_intra_only {
            arf_q = qindex_from_qstep_ratio;
        }
        active_best_quality = qindex_from_qstep_ratio.clamp(i.best_quality, qindex);
        active_worst_quality = (active_best_quality + 3 * active_worst_quality + 2) / 4;
    } else {
        active_best_quality = cq_level;
        if is_intrl_arf_boost && !i.is_intra_only && !leaf_frame {
            let mut ref_tmp_layer = i.ref0_tmp_layer;
            if let Some(l1) = i.ref1_tmp_layer {
                ref_tmp_layer = ref_tmp_layer.max(l1);
            }
            active_best_quality = arf_q;
            let mut tmp_layer_delta = i32::from(temporal_layer) - i32::from(ref_tmp_layer);
            if rf_level == p::RateFactorLevel::GfArfLow {
                let mut w1 = p::NON_BASE_QINDEX_WEIGHT_REF[hierarchical_levels];
                let w2 = p::NON_BASE_QINDEX_WEIGHT_WQ[hierarchical_levels];
                if temporal_layer > 0 && hierarchical_levels == 5 {
                    w1 += i.ref_intra_percentage;
                }
                // C `while (tmp_layer_delta--)` — post-decrement: runs while
                // the OLD value is nonzero; C relies on delta >= 0.
                debug_assert!(tmp_layer_delta >= 0);
                while tmp_layer_delta != 0 {
                    tmp_layer_delta -= 1;
                    active_best_quality =
                        (w1 * active_best_quality + (w2 * cq_level) + ((w1 + w2) / 2)) / (w1 + w2);
                }
            }
        }
    }

    if temporal_layer != 0 {
        active_best_quality = active_best_quality.max(arf_q);
    }
    // `adjust_active_best_and_worst_quality` (rc_crf_cqp.c:167-182).
    if !i.is_intra_only {
        let qdelta = p::frame_type_qdelta(
            i.best_quality,
            i.worst_quality,
            rf_level,
            active_worst_quality,
            bit_depth,
            i.sc_class1,
        );
        active_worst_quality = (active_worst_quality + qdelta).max(active_best_quality);
    }
    active_best_quality = active_best_quality.clamp(i.best_quality, i.worst_quality);
    active_worst_quality = active_worst_quality.clamp(active_best_quality, i.worst_quality);

    CrfQindexOutput {
        qindex: active_best_quality,
        top_index: active_worst_quality,
        bottom_index: active_best_quality,
        r0,
        arf_q,
        kf_boost,
        gfu_boost,
    }
}

/// The still path is unaffected: C returns `qindex` untouched when
/// `scs->allintra`, which is the early return the entire 280/280 still
/// envelope takes.
///
/// `offset_idx` follows C: -1 when the picture is not a reference (target ==
/// source, no scaling), 0 for an IDR, else `min(temporal_layer_index + 1, 5)`.
/// The LOW_DELAY `non_base_boost` arm (rc_crf_cqp.c:371) applies only to
/// non-base temporal layers — `ld_non_base_boost` carries the caller's
/// `non_base_boost(pcs)` result (`None` for a non-low-delay pred structure or
/// a base-layer picture).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn cqp_qindex_calc(
    qindex: i32,
    allintra: bool,
    slice_is_intra: bool,
    is_ref: bool,
    idr_flag: bool,
    temporal_layer_index: u8,
    hierarchical_levels: u8,
    bit_depth: u8,
    ld_non_base_boost: Option<i8>,
) -> i32 {
    if allintra {
        return qindex;
    }
    if hierarchical_levels == 0 && !slice_is_intra {
        return qindex;
    }
    let q_val = convert_qindex_to_q(qindex, bit_depth);
    let offset_idx: i32 = if !is_ref {
        -1
    } else if idr_flag {
        0
    } else {
        i32::from(temporal_layer_index + 1).min(5)
    };
    let mut q_val_target = if offset_idx < 0 {
        q_val
    } else {
        let p = QP_OFFSET_PERCENTS[usize::from(hierarchical_levels <= 4)][offset_idx as usize];
        (q_val - (q_val * f64::from(p) / 100.0)).max(0.0)
    };
    // rc_crf_cqp.c:439-444 — LOW_DELAY only, and only non-base layers. The
    // boost shrinks the target q (a COARSER quantizer) by the L0 reference's
    // intra-coded fraction. Note the guard is `temporal_layer_index != 0`,
    // applied even when `offset_idx == -1` (non-ref non-base).
    if let Some(boost) = ld_non_base_boost
        && temporal_layer_index != 0
        && boost != 0
    {
        q_val_target = (q_val_target - (f64::from(boost) * q_val_target) / 100.0).max(0.0);
    }
    qindex + compute_qdelta(q_val, q_val_target, bit_depth)
}

/// C `non_base_boost` (rc_crf_cqp.c:371) — static.
///
/// The L0 reference's intra-coded fraction `>> 2`. `sb_intra` is the stored
/// reference's per-SB intra flags (`EbReferenceObject::sb_intra`,
/// `coding_loop.c:1606`); an I_SLICE reference or an empty array (a picture
/// whose coded-area walk never armed the accumulator) contributes 0 — C's
/// own `sb_intra` read is likewise skipped for an I_SLICE reference, and an
/// unwritten array there holds its calloc'd zeros.
#[must_use]
pub fn non_base_boost(l0_is_islice: bool, l0_sb_intra: &[u8]) -> i8 {
    if l0_is_islice || l0_sb_intra.is_empty() {
        return 0;
    }
    let intra_sbs: u64 = l0_sb_intra.iter().map(|&v| u64::from(v)).sum();
    if intra_sbs == 0 {
        return 0;
    }
    // C: `intra_percentage = (l0_was_intra * 100) / pcs->sb_total_count` —
    // `sb_intra.len()` IS sb_total_count (one flag per SB, same dimensions).
    let intra_percentage = intra_sbs * 100 / l0_sb_intra.len() as u64;
    (intra_percentage >> 2) as i8
}

/// C `cqp_qindex_calc`'s **fork** (`SVT_HDR_MODE`) arm, kept because this repo
/// gates both modes byte-for-byte. Not reachable from the mainline pipeline.
///
/// `cqp_base_q` is C's `scs->cqp_base_q`: written by the temporal-layer-0 arm
/// and read by the arf-ladder arm, so the caller owns it across frames.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn cqp_qindex_calc_fork(
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

/// C's four r0 capability flags (`initial_rc_process.c:734-762`) — derived
/// per picture after `svt_aom_set_tpl_group`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct R0Flags {
    /// C `pcs->r0_gen` — run `generate_r0beta` for this picture.
    pub r0_gen: bool,
    /// C `pcs->r0_qps` — `crf_qindex_calc`'s qstep arm.
    pub r0_qps: bool,
    /// C `pcs->r0_delta_qp_md` — `sb_qp_derivation_tpl_la`'s gate.
    pub r0_delta_qp_md: bool,
    /// C `pcs->r0_delta_qp_quant` — `delta_q_present` in the frame header.
    pub r0_delta_qp_quant: bool,
}

/// C `initial_rc_process.c:733-762` — when TPL results are unavailable for
/// this temporal layer all four flags shut off; otherwise `r0_gen` is on and
/// the others depend on the configured hierarchical level.
///
/// `reduced_tpl_group` is `tpl_ctrls.reduced_tpl_group` (-1 = no reduction).
#[must_use]
pub fn r0_flags(
    tpl_enable: bool,
    reduced_tpl_group: i8,
    temporal_layer_index: u8,
    hierarchical_levels: u8,
    slice_is_i: bool,
) -> R0Flags {
    let tl = temporal_layer_index;
    if !tpl_enable || (reduced_tpl_group >= 0 && tl > reduced_tpl_group as u8) {
        return R0Flags::default();
    }
    let mut f = R0Flags {
        r0_gen: true,
        ..Default::default()
    };
    match hierarchical_levels {
        5 => {
            f.r0_qps = true;
            f.r0_delta_qp_md = tl <= 3;
            f.r0_delta_qp_quant = f.r0_delta_qp_md && tl == 0;
        }
        4 => {
            f.r0_qps = true;
            f.r0_delta_qp_md = tl <= 2;
            f.r0_delta_qp_quant = f.r0_delta_qp_md && tl == 0;
        }
        3 => {
            f.r0_qps = true;
            f.r0_delta_qp_md = tl <= 1;
            f.r0_delta_qp_quant = f.r0_delta_qp_md && slice_is_i;
        }
        _ => {
            f.r0_qps = tl == 0;
            f.r0_delta_qp_md = tl == 0;
            f.r0_delta_qp_quant = f.r0_delta_qp_md && slice_is_i;
        }
    }
    f
}
