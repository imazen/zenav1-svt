//! The Ghost Robot configuration surface, field by field (plan 1.2).
//!
//! Every field of Ghost Robot's `EbSvtAv1EncConfiguration`
//! (`reference/svt-av1-hdr/Source/API/EbSvtAv1Enc.h`) has exactly one entry
//! here, saying how the port handles it. The test parses the C header, so a new
//! upstream field fails it until someone decides what the port does with it,
//! and a removed field fails it too.
//!
//! What the test checks is the SET of fields. The dispositions are claims:
//! - `Api` / `Hdr` name the knob that sets the field;
//! - `Refused` fields are rejected at non-default values, and
//!   `reference::tests::unported_c_options_are_refused_not_ignored` pins that;
//! - `Derived` and `DefaultOnly` rest on the identity gates, which run C at its
//!   defaults.
//!
//! Update the entry in the same change that changes the behaviour.

use std::collections::BTreeSet;

#[derive(Debug)]
#[allow(dead_code)] // The payload strings are documentation, read by people.
enum D {
    /// Settable through the public API (the named knob).
    Api(&'static str),
    /// Settable as the same-named field of `hdr_mode::HdrForkConfig`.
    Hdr,
    /// A `HdrForkConfig` field whose non-default values are refused.
    Refused,
    /// No knob: the port computes the value the way C does.
    Derived(&'static str),
    /// No knob: the port runs C's default.
    DefaultOnly(&'static str),
    /// Library plumbing with no encoder meaning here.
    NotApplicable(&'static str),
}
use D::*;

const SURFACE: &[(&str, D)] = &[
    (
        "enc_mode",
        Api("EncodePipeline::new_with_preset(NativePreset); AvifEncoder::with_native_preset"),
    ),
    (
        "intra_period_length",
        Api("EncodePipeline::new_with_preset(.., intra_period)"),
    ),
    ("intra_refresh_type", DefaultOnly("key-frame refresh")),
    (
        "hierarchical_levels",
        Api("EncodePipeline::new_with_preset(.., hierarchical_levels, ..)"),
    ),
    ("pred_structure", Api("EncodePipeline::with_pred_structure")),
    (
        "source_width",
        Derived("the frame width given to the constructor"),
    ),
    (
        "source_height",
        Derived("the frame height given to the constructor"),
    ),
    ("forced_max_frame_width", DefaultOnly("0: the source width")),
    (
        "forced_max_frame_height",
        DefaultOnly("0: the source height"),
    ),
    ("frame_rate_numerator", Api("RcConfig::framerate")),
    ("frame_rate_denominator", Api("RcConfig::framerate")),
    (
        "encoder_bit_depth",
        Api("EncodePipeline::with_bit_depth; AvifEncoder::with_bit_depth"),
    ),
    (
        "encoder_color_format",
        Api("EncodePipeline::with_chroma_format / with_chroma_420"),
    ),
    ("profile", Derived("from bit depth and chroma format")),
    ("tier", DefaultOnly("Main tier")),
    (
        "level",
        Derived("entropy::obu::compute_seq_level_idx, C's auto level"),
    ),
    (
        "color_primaries",
        Api("EncodePipeline::with_color_description; AvifEncoder::with_color_space"),
    ),
    (
        "transfer_characteristics",
        Api("EncodePipeline::with_color_description"),
    ),
    (
        "matrix_coefficients",
        Api("EncodePipeline::with_color_description"),
    ),
    ("color_range", Api("EncodePipeline::with_color_description")),
    (
        "mastering_display",
        NotApplicable("no metadata OBU; the AVIF animation facade carries mdcv as a container box"),
    ),
    (
        "content_light_level",
        NotApplicable("no metadata OBU; the AVIF animation facade carries clli as a container box"),
    ),
    (
        "chroma_sample_position",
        Api("EncodePipeline::with_chroma_sample_position"),
    ),
    (
        "rate_control_mode",
        Api("RcConfig::mode (CQP/CRF/CBR; VBR refused)"),
    ),
    ("qp", Api("RcConfig::qp; AvifEncoder::with_quality")),
    ("use_qp_file", NotApplicable("CLI input file")),
    ("target_bit_rate", Api("RcConfig::target_bitrate")),
    ("max_bit_rate", Api("RcConfig::max_bitrate")),
    ("max_qp_allowed", DefaultOnly("63")),
    ("min_qp_allowed", DefaultOnly("1")),
    ("vbr_min_section_pct", DefaultOnly("VBR is refused")),
    ("vbr_max_section_pct", DefaultOnly("VBR is refused")),
    ("under_shoot_pct", DefaultOnly("C's default")),
    ("over_shoot_pct", DefaultOnly("C's default")),
    ("mbr_over_shoot_pct", DefaultOnly("C's default")),
    ("starting_buffer_level_ms", DefaultOnly("C's default")),
    ("optimal_buffer_level_ms", DefaultOnly("C's default")),
    ("maximum_buffer_size_ms", Api("RcConfig::buffer_size_ms")),
    (
        "rc_stats_buffer",
        NotApplicable("two-pass statistics; multi-pass is not ported"),
    ),
    ("pass", DefaultOnly("single pass")),
    ("use_fixed_qindex_offsets", DefaultOnly("0")),
    (
        "qindex_offsets",
        DefaultOnly("unused while use_fixed_qindex_offsets = 0"),
    ),
    (
        "key_frame_chroma_qindex_offset",
        DefaultOnly("unused while use_fixed_qindex_offsets = 0"),
    ),
    (
        "key_frame_qindex_offset",
        DefaultOnly("unused while use_fixed_qindex_offsets = 0"),
    ),
    (
        "chroma_qindex_offsets",
        DefaultOnly("unused while use_fixed_qindex_offsets = 0"),
    ),
    ("luma_y_dc_qindex_offset", DefaultOnly("0")),
    (
        "chroma_u_dc_qindex_offset",
        DefaultOnly("0 (research override: EncodePipeline::chroma_q_override, __expert)"),
    ),
    (
        "chroma_u_ac_qindex_offset",
        DefaultOnly("0 (research override: EncodePipeline::chroma_q_override, __expert)"),
    ),
    (
        "chroma_v_dc_qindex_offset",
        DefaultOnly("0 (research override: EncodePipeline::chroma_q_override, __expert)"),
    ),
    (
        "chroma_v_ac_qindex_offset",
        DefaultOnly("0 (research override: EncodePipeline::chroma_q_override, __expert)"),
    ),
    ("enable_dlf_flag", DefaultOnly("1: deblocking on")),
    (
        "film_grain_denoise_strength",
        Api("EncodePipeline::film_grain; AvifEncoder::with_film_grain"),
    ),
    (
        "film_grain_denoise_apply",
        Api("EncodePipeline::film_grain"),
    ),
    ("cdef_level", DefaultOnly("-1: preset-derived")),
    (
        "enable_restoration_filtering",
        DefaultOnly("-1: preset-derived"),
    ),
    ("enable_mfmv", DefaultOnly("-1: preset-derived")),
    ("scene_change_detection", DefaultOnly("C's default")),
    ("tile_columns", Api("EncodePipeline::with_tile_cols_log2")),
    ("tile_rows", Api("EncodePipeline::with_tile_rows_log2")),
    ("look_ahead_distance", DefaultOnly("C's default")),
    ("recode_loop", DefaultOnly("C's default")),
    ("screen_content_mode", Hdr),
    (
        "aq_mode",
        Api("RcConfig::aq_mode (0 or 2; 1 and 3 refused)"),
    ),
    ("enable_tf", Api("EncodePipeline::enable_tf")),
    ("enable_overlays", DefaultOnly("false")),
    ("tune", Hdr),
    (
        "superres_mode",
        Api("EncodePipeline::with_superres (fixed-denominator mode)"),
    ),
    ("superres_denom", Api("EncodePipeline::with_superres")),
    ("superres_kf_denom", DefaultOnly("follows superres_denom")),
    ("superres_qthres", DefaultOnly("unused in fixed mode")),
    ("superres_kf_qthres", DefaultOnly("unused in fixed mode")),
    (
        "superres_auto_search_type",
        DefaultOnly("unused in fixed mode"),
    ),
    ("fast_decode", DefaultOnly("0")),
    ("sframe_dist", DefaultOnly("0: no S-frames")),
    ("sframe_mode", DefaultOnly("unused without S-frames")),
    (
        "level_of_parallelism",
        Api("EncodePipeline::with_thread_count; AvifEncoder::with_num_threads"),
    ),
    (
        "use_cpu_flags",
        NotApplicable(
            "SIMD tier is chosen at run time (archmage); tests pin tiers with lock_token_testing",
        ),
    ),
    ("stat_report", NotApplicable("C statistics printout")),
    ("recon_enabled", Api("EncodePipeline::with_recon_output")),
    (
        "force_key_frames",
        NotApplicable("per-frame key requests from the caller; frame types follow the GOP"),
    ),
    ("multiply_keyint", DefaultOnly("false")),
    ("resize_mode", DefaultOnly("0: no resize")),
    ("resize_denom", DefaultOnly("unused without resize")),
    ("resize_kf_denom", DefaultOnly("unused without resize")),
    ("enable_qm", Hdr),
    ("min_qm_level", Hdr),
    ("max_qm_level", Hdr),
    ("gop_constraint_rc", DefaultOnly("false")),
    ("lambda_scale_factors", DefaultOnly("C's defaults")),
    ("enable_dg", DefaultOnly("C's default")),
    ("startup_mg_size", DefaultOnly("0")),
    ("startup_qp_offset", DefaultOnly("0")),
    (
        "frame_scale_evts",
        NotApplicable("per-frame resize events from the caller"),
    ),
    ("enable_roi_map", DefaultOnly("false")),
    ("tf_strength", Hdr),
    (
        "fgs_table",
        NotApplicable(
            "caller-supplied film-grain table; the port estimates its own (EncodePipeline::film_grain)",
        ),
    ),
    ("enable_variance_boost", Hdr),
    ("variance_boost_strength", Hdr),
    ("variance_octile", Hdr),
    ("sharpness", Hdr),
    ("variance_boost_curve", Hdr),
    ("luminance_qp_bias", Refused),
    (
        "lossless",
        Api("RcConfig::qp = 0; AvifEncoder::with_lossless"),
    ),
    (
        "avif",
        Derived("the all-intra still path (intra_period == 1)"),
    ),
    ("min_chroma_qm_level", Hdr),
    ("max_chroma_qm_level", Hdr),
    ("rtc", DefaultOnly("false")),
    (
        "qp_scale_compress_strength_unused",
        NotApplicable("reserved by C"),
    ),
    ("sframe_posi", DefaultOnly("unused without S-frames")),
    ("sframe_qp", DefaultOnly("unused without S-frames")),
    ("sframe_qp_offset", DefaultOnly("unused without S-frames")),
    ("adaptive_film_grain", DefaultOnly("C's default")),
    ("max_tx_size", Hdr),
    (
        "extended_crf_qindex_offset",
        Api("RcConfig::extended_crf_qindex_offset"),
    ),
    ("ac_bias", Hdr),
    ("hbd_mds", Refused),
    ("enable_tf_key", Api("EncodePipeline::enable_tf_key")),
    ("max_intra_bitrate_pct", DefaultOnly("C's default")),
    ("max_inter_bitrate_pct", DefaultOnly("C's default")),
    ("enable_intrabc", Derived("screen-content detection, as C")),
    ("max_managed_refs", DefaultOnly("C's default")),
    ("max_hierarchical_levels", Refused),
    ("noise_norm_strength", Hdr),
    ("kf_tf_strength", Hdr),
    ("alt_lambda_factors", Hdr),
    ("sharp_tx", Hdr),
    ("qp_scale_compress_strength", Hdr),
    ("alt_ssim_tuning", Hdr),
    ("tx_bias", Hdr),
    ("complex_hvs", Hdr),
    ("noise_adaptive_filtering", Hdr),
    ("cdef_scaling", Hdr),
    ("noise_strength", Hdr),
    ("noise_strength_chroma", Hdr),
    ("color_range_provided", NotApplicable("CLI bookkeeping")),
    ("noise_size", Hdr),
    ("noise_chroma_from_luma", Hdr),
    ("enable_qmpsnr", Refused),
    ("padding", NotApplicable("ABI padding")),
];

/// Field names of `EbSvtAv1EncConfiguration` in the Ghost Robot header.
fn c_fields() -> BTreeSet<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/svt-av1-hdr/Source/API/EbSvtAv1Enc.h"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let start = text
        .find("typedef struct EbSvtAv1EncConfiguration")
        .expect("struct start");
    let end = start
        + text[start..]
            .find("} EbSvtAv1EncConfiguration;")
            .expect("struct end");
    // Strip comments, then read `type name;` / `type name[...];` declarations.
    let mut body = String::new();
    let mut rest = &text[start..end];
    while let Some(i) = rest.find("/*") {
        body.push_str(&rest[..i]);
        rest = &rest[i + rest[i..].find("*/").expect("unterminated comment") + 2..];
    }
    body.push_str(rest);
    let body: String = body
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = BTreeSet::new();
    for decl in body.split(';') {
        let decl = decl.split('{').next_back().unwrap_or(decl);
        let decl = decl.split('[').next().unwrap_or(decl).trim();
        if let Some(name) = decl
            .split(|c: char| c.is_whitespace() || c == '*')
            .rfind(|s| !s.is_empty())
        {
            if name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && decl.split_whitespace().count() >= 2
            {
                out.insert(name.to_string());
            }
        }
    }
    out
}

#[test]
fn every_ghost_robot_config_field_has_one_disposition() {
    let c = c_fields();
    let mut ours = BTreeSet::new();
    for (name, _) in SURFACE {
        assert!(ours.insert(name.to_string()), "{name} listed twice");
    }
    let missing: Vec<_> = c.difference(&ours).collect();
    let stale: Vec<_> = ours.difference(&c).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "config surface ledger out of date.\n  in the C header, not here: {missing:?}\n  here, not in the C header: {stale:?}"
    );
    // Sanity: the parser found the whole struct (139 fields at 9dabe3ca).
    assert!(
        c.len() >= 130,
        "parsed only {} fields; the header parser is broken",
        c.len()
    );
}
