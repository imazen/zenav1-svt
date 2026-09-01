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
    /// Whether to try all directional intra modes.
    pub enable_directional_modes: bool,
    /// Whether to enable filter-intra.
    pub enable_filter_intra: bool,
    /// Whether to enable temporal filtering.
    pub enable_temporal_filter: bool,
    /// Whether to use RDO for transform type selection.
    pub rdo_tx_decision: bool,
    /// Maximum number of intra candidates to evaluate.
    pub max_intra_candidates: u8,
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
            enable_directional_modes: p <= 10,
            enable_filter_intra: p <= 6,
            enable_temporal_filter: p <= 12,
            rdo_tx_decision: p <= 6,
            max_intra_candidates: match p {
                0..=3 => 13, // All modes
                4..=6 => 7,  // Non-directional + some directional
                7..=9 => 4,  // DC, V, H, smooth
                _ => 2,      // DC, V only
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
/// allintra = false (multi-frame): C's own VIDEO-mode derivations — see
/// [`seq_tools_video`], which replaced a blanket `SeqTools::default()` (every
/// tool bit 0). The old comment here argued that "signaling a tool without its
/// frame/block syntax desyncs every decoder"; that caution is right in general
/// and wrong for four of the five bits it was suppressing, and the per-bit
/// analysis now lives on [`seq_tools_video`].
///
/// `luma_pixels` is the padded encode area, read ONLY on the video path (C's
/// Wiener/SGR level functions zero themselves at 8K and above). The allintra
/// derivations take no resolution argument at all, so every still cell
/// ignores it.
pub fn seq_tools_for_preset(
    preset: u8,
    allintra: bool,
    luma_pixels: usize,
) -> crate::entropy::obu::SeqTools {
    if !allintra {
        return seq_tools_video(preset, luma_pixels);
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
    // ONE definition, shared with the leaf funnel's own prediction — see
    // `crate::intra_arm::intra_edge_filter`. This used to be the literal
    // `preset == 5` and the funnel carried its own copy in
    // `FunnelCfg::for_preset`; the video arm signals the bit at EVERY preset,
    // and the two copies disagreed there.
    let enable_intra_edge_filter = crate::intra_arm::intra_edge_filter(
        crate::sc_detect::ScArm::Allintra,
        crate::rate_arm::eff_enc_mode(crate::sc_detect::ScArm::Allintra, preset),
    );
    crate::entropy::obu::SeqTools {
        film_grain_params_present: false,
        separate_uv_delta_q: false, // mainline (fork wiring pending chroma-q quant threading)
        enable_filter_intra: filter_intra_level != 0,
        enable_intra_edge_filter,
        // SB size is NOT a speed feature: C derives it from frame area +
        // preset + the force-64 clauses (Globals/enc_handle.c:4071-4111),
        // SH `enable_restoration` (a SeqTools field, NOT the pruned
        // SpeedConfig one): the sequence header advertises restoration iff
        // this preset's Wiener/SGR filter levels are non-zero.
        enable_restoration: wn > 0 || sg > 0,
        // which `seq_tools_for_preset` cannot see. The pipeline overwrites
        // this from its own `sb_size` right after calling us.
        use_128x128_superblock: false,
        // Superres is OFF by default in C (`superres_mode = SUPERRES_NONE`,
        // enc_settings.c:1095) and is a CONFIG knob, not a speed feature —
        // the pipeline sets it when a caller opts in (superres chunk B).
        enable_superres: false,
        // Signalling-only config knob (issue #9 item 5); the pipeline
        // overwrites it from `EncodePipeline::chroma_sample_position`.
        chroma_sample_position: 0,
        // The reduced (still) header writes NONE of the inter tool bits, so
        // these are inert here whatever they hold.
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_jnt_comp: false,
        enable_warped_motion: false,
        enable_ref_frame_mvs: false,
        enable_order_hint: true,
        enable_dual_filter: false,
        // GOP shape, not a speed feature — the pipeline overwrites it from
        // its own `gop.hierarchical_levels`. Only the non-reduced (video)
        // sequence header reads it.
        hierarchical_levels: 0,
    }
}

/// C `svt_aom_sig_deriv_pre_analysis_scs` (enc_mode_config.c:2780) — the
/// **non-allintra, non-rtc** arm with `enable_restoration_filtering = DEFAULT`
/// (the library default). Every value below is that function's, cited.
///
/// Each bit was checked against what it actually obligates before being
/// turned on, because signaling a tool whose frame/block syntax is unported
/// really does desync a decoder:
///
/// * `enable_intra_edge_filter` — decoder-side prediction behaviour, no
///   syntax at all (:2820).
/// * `enable_interintra_compound`, `enable_warped_motion`,
///   `enable_masked_compound`, `enable_jnt_comp` — gate per-block syntax in
///   INTER blocks only. A KEY frame codes none, so they are inert on the
///   frame this chunk targets.
/// * `enable_ref_frame_mvs` — gates the FH `use_ref_frame_mvs`, which spec
///   5.9.2 writes only for a non-intra frame.
/// * `enable_restoration` — this one DOES obligate every frame header to
///   carry `lr_params()`, key frames included. Safe because the port's FH
///   writer already emits it: the allintra branch above sets the same bit for
///   presets <= 6 and the still gates cover that path.
///
/// So a video-mode KEY frame can carry C's real tool bits today. The
/// per-block INTER syntax those bits gate is still unported, which is why
/// `pipeline.rs` continues to refuse an actual inter frame.
#[must_use]
pub fn seq_tools_video(preset: u8, luma_pixels: usize) -> crate::entropy::obu::SeqTools {
    let res_class = crate::pd0::input_resolution_class(luma_pixels);
    const RES_8K: u8 = 6; // INPUT_SIZE_8K_RANGE (definitions.h:1830)

    // `svt_aom_get_inter_intra_level` (:8803) is evaluated for
    // transition_present in {0, 1} and OR-ed (:2786-2791). At <= M1 it is 2
    // regardless; at <= M8 it is 2 for transition_present = 1 — which the loop
    // always reaches — so the OR is nonzero for every preset <= M8.
    let enable_interintra_compound = preset <= 8;

    // `get_filter_intra_level_default` (:8771): 1 at <= M1, 2 at <= M5, else
    // 0. The SH bit is `level != 0` (:2798).
    let filter_intra_level: u8 = if preset <= 1 {
        1
    } else if preset <= 5 {
        2
    } else {
        0
    };

    // `get_inter_compound_level` (:8758): 3 at <= M0, 4 at <= M2, else 0.
    // Both jnt_comp and masked_compound follow it (:2800-2806).
    let inter_compound = preset <= 2;

    // `svt_aom_get_wn_filter_level_default` (:1357) evaluated over
    // is_not_last_layer in {0, 1}, breaking on the first nonzero
    // (:2702-2707): the is_ref = 1 arm gives 4 at <= M3 and 5 at <= M8, so the
    // level is nonzero for every preset <= M8. Zeroed at 8K and above.
    let wn_nonzero = preset <= 8 && res_class < RES_8K;
    // `svt_aom_get_sg_filter_level_default` (:1402): 1 at <= MR, 3 at <= M3,
    // else 0; zeroed at 8K and above, and under `fast_decode` above 360p. The
    // port carries no fast_decode config, so that clause cannot fire — when
    // one is added it belongs in this expression.
    let sg_nonzero = preset <= 3 && res_class < RES_8K;

    crate::entropy::obu::SeqTools {
        film_grain_params_present: false,
        separate_uv_delta_q: false,
        enable_filter_intra: filter_intra_level != 0,
        // ":2820 — for non-still-image or non-all-intra configurations, keep
        // edge filter always ON". Routed through the shared derivation so the
        // header bit and the funnel's prediction cannot drift apart.
        enable_intra_edge_filter: crate::intra_arm::intra_edge_filter(
            crate::sc_detect::ScArm::Video { is_islice: true },
            preset,
        ),
        // `svt_aom_get_enable_restoration_default` (:2695) = sg > 0 || wn > 0.
        enable_restoration: wn_nonzero || sg_nonzero,
        // Overwritten by the pipeline from its own `sb_size`, as on the
        // allintra path.
        use_128x128_superblock: false,
        enable_superres: false,
        chroma_sample_position: 0,
        enable_interintra_compound,
        enable_masked_compound: inter_compound,
        enable_jnt_comp: inter_compound,
        // ":2849 — unconditional, in BOTH modes. The still header simply
        // never writes the bit."
        enable_warped_motion: true,
        // `sequence_control_set.c:103` sets it to 1 and nothing lowers it.
        enable_ref_frame_mvs: true,
        enable_order_hint: true,
        enable_dual_filter: false,
        // Overwritten by the pipeline from `gop.hierarchical_levels`.
        hierarchical_levels: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field asserted here is CONSUMED somewhere (issue #9 item 9
    /// pruned the ones that were not) — so this is a test of the live preset
    /// table, not of a decorative one.
    #[test]
    fn preset_0_enables_everything() {
        let cfg = SpeedConfig::from_preset(0);
        assert!(cfg.enable_adst);
        assert!(cfg.enable_filter_intra);
        assert!(cfg.enable_directional_modes);
        assert!(cfg.enable_temporal_filter);
        assert!(cfg.rdo_tx_decision);
        assert_eq!(cfg.max_intra_candidates, 13);
        assert_eq!(cfg.max_partition_depth, 4);
    }

    #[test]
    fn preset_13_minimal() {
        let cfg = SpeedConfig::from_preset(13);
        assert!(!cfg.enable_adst);
        assert!(!cfg.enable_filter_intra);
        assert!(!cfg.enable_directional_modes);
        assert!(!cfg.rdo_tx_decision);
        assert_eq!(cfg.max_intra_candidates, 2);
    }

    #[test]
    fn preset_monotonic() {
        // Higher presets should generally have fewer features
        let p4 = SpeedConfig::from_preset(4);
        let p8 = SpeedConfig::from_preset(8);
        let p12 = SpeedConfig::from_preset(12);

        assert!(p4.max_intra_candidates >= p8.max_intra_candidates);
        assert!(p8.max_intra_candidates >= p12.max_intra_candidates);
        assert!(p4.max_partition_depth >= p12.max_partition_depth);
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
            let t = seq_tools_for_preset(p, true, 64 * 64);
            let expect_on = p <= 6;
            assert_eq!(t.enable_filter_intra, expect_on, "filter_intra M{p}");
            assert_eq!(t.enable_restoration, expect_on, "restoration M{p}");
        }
        // Multi-frame (non-allintra): C's DEFAULT-path derivations, which
        // landed with the inter campaign's C1a. The comment this replaces said
        // "both off at every preset until the default-path derivations + inter
        // syntax land" — they have now landed, so the table below is C's, from
        // `svt_aom_sig_deriv_pre_analysis_scs` (enc_mode_config.c:2780).
        for p in 0..=13u8 {
            let t = seq_tools_for_preset(p, false, 64 * 64);
            // `get_filter_intra_level_default` (:8771): nonzero at <= M5.
            assert_eq!(t.enable_filter_intra, p <= 5, "video filter_intra M{p}");
            // wn nonzero at <= M8 (:1357, is_ref arm) OR sg nonzero at <= M3
            // (:1402); below 8K neither is clamped.
            assert_eq!(t.enable_restoration, p <= 8, "video restoration M{p}");
            // ":2820 — keep edge filter always ON" for non-allintra.
            assert!(t.enable_intra_edge_filter, "video intra_edge_filter M{p}");
            // `svt_aom_get_inter_intra_level` (:8803) is nonzero for
            // transition_present = 1 at every preset <= M8.
            assert_eq!(
                t.enable_interintra_compound,
                p <= 8,
                "video interintra M{p}"
            );
            // `get_inter_compound_level` (:8758): nonzero at <= M2, and both
            // masked_compound and jnt_comp follow it (:2800-2806).
            assert_eq!(t.enable_masked_compound, p <= 2, "video masked M{p}");
            assert_eq!(t.enable_jnt_comp, p <= 2, "video jnt_comp M{p}");
            // ":2849" and sequence_control_set.c:103 — both unconditional.
            assert!(t.enable_warped_motion, "video warped M{p}");
            assert!(t.enable_ref_frame_mvs, "video ref_frame_mvs M{p}");
            assert!(t.enable_order_hint, "video order_hint M{p}");
            assert!(!t.enable_dual_filter, "video dual_filter M{p}");
        }
        // The 8K clamp in both restoration level functions (:1368, :1413).
        // 8K_TH is 0x5028000 pixels; one above it must zero both.
        let t8k = seq_tools_for_preset(4, false, 0x5028000);
        assert!(
            !t8k.enable_restoration,
            "wn and sg both zero at >= INPUT_SIZE_8K_RANGE"
        );
    }
}
