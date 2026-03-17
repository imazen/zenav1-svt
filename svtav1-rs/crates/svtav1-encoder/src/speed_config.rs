//! Speed preset configuration — controls the speed/quality tradeoff.
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
}
