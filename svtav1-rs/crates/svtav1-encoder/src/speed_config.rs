//! Speed preset configuration — controls the speed/quality tradeoff.
//!
//! Spec 03: Speed preset controls (enc_mode_config.c).
//!
//! SVT-AV1 has 14 presets (0-13) controlling which tools are enabled
//! and how thoroughly they're searched. Lower presets are slower but
//! produce better quality; higher presets are faster.
//!
//! Ported from SVT-AV1's enc_mode_config.c.

/// Speed configuration derived from a preset number.
#[derive(Debug, Clone)]
pub struct SpeedConfig {
    /// Preset number (0-13).
    pub preset: u8,
    /// Maximum partition depth (0 = 128x128 only, 4 = down to 4x4).
    pub max_partition_depth: u8,
    /// Whether to enable ADST transform types.
    pub enable_adst: bool,
    /// Whether to enable identity transform.
    pub enable_identity_tx: bool,
    /// Whether to try all directional intra modes.
    pub enable_directional_modes: bool,
    /// Whether to enable CfL chroma prediction.
    pub enable_cfl: bool,
    /// Whether to enable filter-intra.
    pub enable_filter_intra: bool,
    /// Whether to enable palette mode.
    pub enable_palette: bool,
    /// Whether to enable OBMC.
    pub enable_obmc: bool,
    /// Whether to enable warped motion.
    pub enable_warped_motion: bool,
    /// Whether to enable compound inter prediction.
    pub enable_compound: bool,
    /// Whether to enable temporal filtering.
    pub enable_temporal_filter: bool,
    /// Whether to enable CDEF.
    pub enable_cdef: bool,
    /// Whether to enable loop restoration.
    pub enable_restoration: bool,
    /// Whether to use RDO for transform type selection.
    pub rdo_tx_decision: bool,
    /// Maximum number of intra candidates to evaluate.
    pub max_intra_candidates: u8,
    /// Sub-pixel ME precision (0=full-pel, 1=half, 2=quarter, 3=eighth).
    pub subpel_precision: u8,
    /// HME levels to use (0=none, 1=L2 only, 2=L1+L2, 3=L0+L1+L2).
    pub hme_levels: u8,
    /// ME search area width.
    pub me_search_width: u16,
    /// ME search area height.
    pub me_search_height: u16,
}

impl SpeedConfig {
    /// Create a speed configuration from a preset number (0-13).
    pub fn from_preset(preset: u8) -> Self {
        let p = preset.min(13);
        Self {
            preset: p,
            max_partition_depth: match p {
                0..=3 => 4, // Full depth
                4..=6 => 3, // Skip smallest
                7..=9 => 2, // Medium depth
                _ => 1,     // Shallow
            },
            enable_adst: p <= 10,
            enable_identity_tx: p <= 8,
            enable_directional_modes: p <= 10,
            enable_cfl: p <= 11,
            enable_filter_intra: p <= 6,
            enable_palette: p <= 4,
            enable_obmc: p <= 6,
            enable_warped_motion: p <= 8,
            enable_compound: p <= 10,
            enable_temporal_filter: p <= 12,
            enable_cdef: p <= 12,
            enable_restoration: p <= 10,
            rdo_tx_decision: p <= 6,
            max_intra_candidates: match p {
                0..=3 => 13, // All modes
                4..=6 => 7,  // Non-directional + some directional
                7..=9 => 4,  // DC, V, H, smooth
                _ => 2,      // DC, V only
            },
            subpel_precision: match p {
                0..=5 => 3,  // Eighth-pel
                6..=8 => 2,  // Quarter-pel
                9..=11 => 1, // Half-pel
                _ => 0,      // Full-pel
            },
            hme_levels: match p {
                0..=3 => 3, // Full HME
                4..=8 => 2, // Reduced HME
                _ => 1,     // Minimal
            },
            me_search_width: match p {
                0..=3 => 64,
                4..=6 => 48,
                7..=9 => 32,
                _ => 16,
            },
            me_search_height: match p {
                0..=3 => 64,
                4..=6 => 48,
                7..=9 => 32,
                _ => 16,
            },
        }
    }

    /// Get the effective lambda multiplier for this preset.
    /// Lower presets use more precise (lower) lambda; higher presets
    /// use higher lambda to favor rate over distortion.
    pub fn lambda_scale(&self) -> f64 {
        match self.preset {
            0..=3 => 1.0,
            4..=6 => 1.1,
            7..=9 => 1.2,
            _ => 1.4,
        }
    }
}

/// Per-preset sequence-header tool bits — C-exact port of the allintra
/// derivations the SH writer signals (`enable_filter_intra` /
/// `enable_restoration`).
///
/// C reference: v4.2.0-rc with the relevant debug macros all = 1
/// (OPT_NSC_STILL_IMAGE / TUNE_SIMPLIFY_SETTINGS / FIX_MR_STILL_IMAGE,
/// EbDebugMacros.h:37/74/77). `allintra` is C `scs->allintra` =
/// `intra_period_length == 0 || avif || pred_structure == ALL_INTRA`
/// (enc_handle.c:4624) — our still-picture single-frame mode
/// (`intra_period <= 1`), the identity-harness `--avif 1` config.
///
/// allintra = true:
///
/// - filter_intra: `get_filter_intra_level_allintra`
///   (enc_mode_config.c:12679-12689): M0 -> 1, M1..M6 -> 2, M7+ -> 0.
///   SH bit = `level != 0` (`scs->seq_header.filter_intra_level =
///   is_filter_intra_used ? 1 : 0`, enc_mode_config.c:4017-4025).
/// - restoration: `svt_aom_get_enable_restoration_allintra`
///   (enc_mode_config.c:3944-3962), DEFAULT-config branch
///   `(wn > 0 || sg > 0)` with
///   wn = `svt_aom_get_wn_filter_level_allintra`
///   (enc_mode_config.c:1928-1948: <=M0 -> 3, <=M3 -> 3, <=M6 -> 4,
///   else 0) and
///   sg = `svt_aom_get_sg_filter_level_allintra`
///   (enc_mode_config.c:2000-2009: <=ENC_MR -> 1, else 0; ENC_MR = -1,
///   EbSvtAv1Enc.h:45, unreachable from the u8 preset domain).
///   Assigned to `scs->seq_header.enable_restoration` at
///   enc_mode_config.c:4056-4058.
///
/// Both land ON for presets 0..=6 and OFF for 7..=13 — which is why
/// M10/M13 were already byte-identical with the bits hardwired 0.
///
/// allintra = false (multi-frame): both off. C's default/rtc derivations
/// differ (`get_filter_intra_level_default` enables at <=M5,
/// `svt_aom_get_enable_restoration_default` is resolution-dependent) and
/// our inter FH/tile writers carry no restoration or filter-intra syntax
/// yet — signaling a tool without its frame/block syntax desyncs every
/// decoder, so the multi-frame path keeps the pre-threading behavior
/// until those ports land.
pub fn seq_tools_for_preset(preset: u8, allintra: bool) -> svtav1_entropy::obu::SeqTools {
    if !allintra {
        return svtav1_entropy::obu::SeqTools::default();
    }
    // get_filter_intra_level_allintra (enc_mode_config.c:12679).
    let filter_intra_level: u8 = if preset == 0 {
        1
    } else if preset <= 6 {
        2
    } else {
        0
    };
    // svt_aom_get_wn_filter_level_allintra (enc_mode_config.c:1928).
    let wn: u8 = if preset <= 3 {
        3 // covers the <=M0 and <=M3 arms (both 3 with OPT_NSC_STILL_IMAGE)
    } else if preset <= 6 {
        4
    } else {
        0
    };
    // svt_aom_get_sg_filter_level_allintra (enc_mode_config.c:2000):
    // 1 only for ENC_MR (-1) — not representable as a u8 preset.
    let sg: u8 = 0;
    // enable_intra_edge_filter (svt_aom_sig_deriv_pre_analysis_scs,
    // enc_mode_config.c:4036-4048): allintra sets it iff
    // `dist_based_ang_intra_level >= 1 || angular_pred_level[intra_level]
    // == 2 || == 3`. svt_aom_get_intra_mode_levels_allintra (:6907):
    // intra_level = 1 (<=M4), 2 (M5), 6 (M6), 7 (M7-M8), 8 (>=M9), always
    // with dist_based_ang_intra_level = 0; angular_pred_level table (:18)
    // = {0,1,2,2,3,4,4,4,4,0} -> only intra_level 2 (= preset 5) lands in
    // {2,3}. Verified by the instrumented config dump (M5DBG CFG ang=2 at
    // enc_mode 5, ang=1 at <=4, ang=4 at >=6).
    let enable_intra_edge_filter = preset == 5;
    svtav1_entropy::obu::SeqTools {
        enable_filter_intra: filter_intra_level != 0,
        enable_intra_edge_filter,
        enable_restoration: wn > 0 || sg > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_0_enables_everything() {
        let cfg = SpeedConfig::from_preset(0);
        assert!(cfg.enable_adst);
        assert!(cfg.enable_identity_tx);
        assert!(cfg.enable_filter_intra);
        assert!(cfg.enable_palette);
        assert!(cfg.enable_obmc);
        assert!(cfg.enable_warped_motion);
        assert!(cfg.enable_compound);
        assert!(cfg.rdo_tx_decision);
        assert_eq!(cfg.max_intra_candidates, 13);
        assert_eq!(cfg.subpel_precision, 3);
    }

    #[test]
    fn preset_13_minimal() {
        let cfg = SpeedConfig::from_preset(13);
        assert!(!cfg.enable_adst);
        assert!(!cfg.enable_filter_intra);
        assert!(!cfg.enable_palette);
        assert!(!cfg.enable_obmc);
        assert!(!cfg.enable_warped_motion);
        assert!(!cfg.rdo_tx_decision);
        assert_eq!(cfg.max_intra_candidates, 2);
        assert_eq!(cfg.subpel_precision, 0);
    }

    #[test]
    fn preset_monotonic() {
        // Higher presets should generally have fewer features
        let p4 = SpeedConfig::from_preset(4);
        let p8 = SpeedConfig::from_preset(8);
        let p12 = SpeedConfig::from_preset(12);

        assert!(p4.max_intra_candidates >= p8.max_intra_candidates);
        assert!(p8.max_intra_candidates >= p12.max_intra_candidates);
        assert!(p4.me_search_width >= p8.me_search_width);
    }

    #[test]
    fn preset_clamping() {
        let cfg = SpeedConfig::from_preset(99);
        assert_eq!(cfg.preset, 13);
    }

    /// The allintra tool-bit table across the whole preset domain — the C
    /// derivations put the on/off boundary at M6/M7 for BOTH tools (see
    /// seq_tools_for_preset provenance). The M10/M13 rows are the
    /// already-byte-identical configs and must stay all-off.
    #[test]
    fn seq_tools_allintra_c_table() {
        for p in 0..=13u8 {
            let t = seq_tools_for_preset(p, true);
            let expect_on = p <= 6;
            assert_eq!(t.enable_filter_intra, expect_on, "filter_intra M{p}");
            assert_eq!(t.enable_restoration, expect_on, "restoration M{p}");
        }
        // Multi-frame (non-allintra): both off at every preset until the
        // default-path derivations + inter syntax land.
        for p in 0..=13u8 {
            let t = seq_tools_for_preset(p, false);
            assert!(!t.enable_filter_intra && !t.enable_restoration);
        }
    }
}
